//! IRC over WebSocket (`text.ircv3.net`): one line per text frame, no CRLF.
//!
//! This is the one place in the crate that does I/O. It answers server `PING`s,
//! pings an idle server and fails with [`Error::Timeout`] if it stays silent.
//! Reconnecting is the caller's loop; [`Backoff`] spaces the attempts.
//!
//! TLS is rustls with the OS roots (`SSL_CERT_FILE` is honoured, which is how the
//! dev stack's Caddy CA gets trusted). Frames are capped per D14.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{self, Message as Frame};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::Message;

/// 8191 bytes of tags plus 512 for the rest, with headroom. Bigger is hostile.
const MAX_FRAME: usize = 16 << 10;
/// Silence before we PING, then how long we wait for anything at all.
const IDLE: Duration = Duration::from_secs(90);
const PONG_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    WebSocket(#[from] tungstenite::Error),
    #[error("the server stopped responding")]
    Timeout,
    #[error(transparent)]
    Message(#[from] crate::message::Error),
}

pub struct Connection {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    pinged: bool,
}

impl Connection {
    /// `url` is the discovery document's `irc.websocket`, e.g. `wss://host/irc`.
    pub async fn connect(url: &str) -> Result<Self, Error> {
        let mut req = url.into_client_request()?;
        req.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static("text.ircv3.net"),
        );
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_FRAME))
            .max_frame_size(Some(MAX_FRAME));
        let (ws, _) = tokio_tungstenite::connect_async_tls_with_config(
            req,
            Some(config),
            false,
            Some(Connector::Rustls(tls_config())),
        )
        .await?;
        Ok(Self { ws, pinged: false })
    }

    pub async fn send(&mut self, msg: &Message) -> Result<(), Error> {
        self.ws.send(Frame::text(msg.to_line()?)).await?;
        Ok(())
    }

    /// The next line, or `None` once the server closed the connection.
    /// Unparsable lines are logged and skipped.
    pub async fn recv(&mut self) -> Result<Option<Message>, Error> {
        loop {
            let wait = if self.pinged { PONG_WAIT } else { IDLE };
            let frame = match tokio::time::timeout(wait, self.ws.next()).await {
                Err(_) if self.pinged => return Err(Error::Timeout),
                Err(_) => {
                    self.pinged = true;
                    self.send(&Message::new("PING", ["keepalive"])).await?;
                    continue;
                }
                Ok(None) => return Ok(None),
                Ok(Some(frame)) => frame?,
            };
            self.pinged = false;
            let text = match frame {
                Frame::Text(t) => t,
                Frame::Close(_) => return Ok(None),
                _ => continue,
            };
            match Message::parse(&text) {
                Ok(m) if m.command == "PING" => {
                    self.send(&Message::new("PONG", m.params)).await?;
                }
                Ok(m) => return Ok(Some(m)),
                Err(e) => tracing::warn!("skipping bad line from server: {e}"),
            }
        }
    }

    pub async fn close(mut self) -> Result<(), Error> {
        self.ws.close(None).await?;
        Ok(())
    }
}

fn tls_config() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    let found = rustls_native_certs::load_native_certs();
    for e in &found.errors {
        tracing::warn!("loading system certificates: {e}");
    }
    roots.add_parsable_certificates(found.certs);
    // Explicit provider: the dependency graph may enable both ring and aws-lc-rs.
    let config = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .expect("ring supports the default TLS versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    Arc::new(config)
}

/// Exponential reconnect delay, 1 s doubling to 60 s, with jitter so a server
/// restart doesn't get every client back in the same second.
#[derive(Debug, Default)]
pub struct Backoff {
    attempt: u32,
}

impl Backoff {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_delay(&mut self) -> Duration {
        let base = Duration::from_secs(1 << self.attempt.min(6)).min(Duration::from_secs(60));
        self.attempt = self.attempt.saturating_add(1);
        // Somewhere in [base/2, base].
        let jitter = f64::from(getrandom::u32().unwrap_or(0)) / f64::from(u32::MAX);
        base.mul_f64(0.5 + jitter / 2.0)
    }

    /// Call once a connection got through registration.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_caps_and_resets() {
        let mut b = Backoff::new();
        let d: Vec<Duration> = (0..10).map(|_| b.next_delay()).collect();
        assert!(d[0] >= Duration::from_millis(500) && d[0] <= Duration::from_secs(1));
        assert!(d[3] >= Duration::from_secs(4) && d[3] <= Duration::from_secs(8));
        assert!(d[9] >= Duration::from_secs(30) && d[9] <= Duration::from_secs(60));
        b.reset();
        assert!(b.next_delay() <= Duration::from_secs(1));
    }
}
