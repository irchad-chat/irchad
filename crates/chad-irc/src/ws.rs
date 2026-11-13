//! IRC over WebSocket (`text.ircv3.net`, one line per text frame, no CRLF), or
//! classic IRC over TCP/TLS (CRLF lines) for non-IrChad networks (D72).
//!
//! This is the one place in the crate that does I/O. [`Keepalive`] answers
//! server `PING`s, pings an idle server and fails with [`Error::Timeout`] if it
//! stays silent. Reconnecting is the caller's loop; [`crate::keepalive::Backoff`]
//! spaces the attempts.
//!
//! TLS is rustls with the OS roots (`SSL_CERT_FILE` is honoured, which is how the
//! dev stack's Caddy CA gets trusted). Frames are capped per D14.

use std::sync::Arc;

use futures_util::{SinkExt as _, StreamExt as _};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Uri};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{self, Message as Frame};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::Message;
use crate::keepalive::{Keepalive, SENT_WAIT};

/// 8191 bytes of tags plus 512 for the rest, with headroom. Bigger is hostile.
const MAX_FRAME: usize = 16 << 10;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    WebSocket(#[from] tungstenite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("the server stopped responding")]
    Timeout,
    #[error(transparent)]
    Message(#[from] crate::message::Error),
}

/// `localhost` and `*.localhost` (RFC 6761), which always mean this machine.
pub fn is_localhost(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost")
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

enum Inner {
    Ws(Box<WebSocketStream<MaybeTlsStream<TcpStream>>>),
    /// Classic IRC over TCP or TLS: CRLF-terminated lines.
    Line(Lines),
}

pub struct Connection {
    inner: Inner,
    alive: Keepalive,
    /// When the keepalive acts. A deadline, not a per-`recv` timeout: `recv` is
    /// raced and restarted constantly, which would put it off forever.
    deadline: Instant,
}

impl Connection {
    /// `wss://host/irc` (an IrChad server's `irc.websocket`), or a classic
    /// `ircs://host:6697` / `irc://host:6667`.
    pub async fn connect(url: &str) -> Result<Self, Error> {
        let inner = match url.split_once("://").map(|(s, _)| s) {
            Some("ircs") => Inner::Line(Lines::connect(url, true).await?),
            Some("irc") => Inner::Line(Lines::connect(url, false).await?),
            _ => Inner::Ws(Box::new(connect_ws(url).await?)),
        };
        let alive = Keepalive::new();
        Ok(Self {
            inner,
            deadline: Instant::now() + alive.wait(),
            alive,
        })
    }

    /// Sends `msg`. A link that died quietly takes our writes without
    /// complaint, so a send makes the keepalive check sooner.
    pub async fn send(&mut self, msg: &Message) -> Result<(), Error> {
        self.deadline = self.deadline.min(Instant::now() + SENT_WAIT);
        self.write(msg).await
    }

    fn rearm(&mut self) {
        self.deadline = Instant::now() + self.alive.wait();
    }

    async fn write(&mut self, msg: &Message) -> Result<(), Error> {
        let line = msg.to_line()?;
        match &mut self.inner {
            Inner::Ws(ws) => ws.send(Frame::text(line)).await?,
            Inner::Line(l) => l.send(&line).await?,
        }
        Ok(())
    }

    /// The next line, or `None` once the server closed the connection.
    /// Unparsable lines are logged and skipped.
    pub async fn recv(&mut self) -> Result<Option<Message>, Error> {
        loop {
            let text = match tokio::time::timeout_at(self.deadline, self.next_text()).await {
                Err(_) => {
                    let ping = self.alive.silence().map_err(|_| Error::Timeout)?;
                    self.write(&ping).await?;
                    self.rearm();
                    continue;
                }
                Ok(r) => match r? {
                    Some(t) => t,
                    None => return Ok(None),
                },
            };
            let parsed = Message::parse(&text);
            match parsed {
                Ok(m) => {
                    let pong = self.alive.line(&m);
                    self.rearm();
                    match pong {
                        Some(pong) => self.write(&pong).await?,
                        None => return Ok(Some(m)),
                    }
                }
                Err(e) => {
                    self.alive.heard();
                    self.rearm();
                    tracing::warn!("skipping bad line from server: {e}");
                }
            }
        }
    }

    /// Cancel-safe: `recv` is raced against commands and timeouts.
    async fn next_text(&mut self) -> Result<Option<String>, Error> {
        match &mut self.inner {
            Inner::Ws(ws) => loop {
                match ws.next().await {
                    None => return Ok(None),
                    Some(frame) => match frame? {
                        Frame::Text(t) => return Ok(Some(t.as_str().to_owned())),
                        Frame::Close(_) => return Ok(None),
                        _ => continue,
                    },
                }
            },
            Inner::Line(l) => Ok(l.read().await?),
        }
    }

    pub async fn close(self) -> Result<(), Error> {
        match self.inner {
            Inner::Ws(mut ws) => WebSocketStream::close(&mut ws, None).await?,
            Inner::Line(mut l) => l.io.shutdown().await?,
        }
        Ok(())
    }
}

async fn tcp(host: &str, port: u16) -> std::io::Result<TcpStream> {
    // RFC 6761: `*.localhost` is loopback, but glibc only knows that with
    // nss-myhostname, and the dev origin is `chat.localhost`.
    let tcp = if is_localhost(host) {
        TcpStream::connect(("127.0.0.1", port)).await
    } else {
        TcpStream::connect((host, port)).await
    }?;
    tcp.set_nodelay(true)?;
    Ok(tcp)
}

async fn connect_ws(url: &str) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, Error> {
    let mut req = url.into_client_request()?;
    req.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("text.ircv3.net"),
    );
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME));
    let uri = req.uri();
    let host = uri.host().unwrap_or_default().trim_matches(['[', ']']);
    let port = uri.port_u16().unwrap_or(443);
    let tcp = tcp(host, port).await.map_err(tungstenite::Error::Io)?;
    let (ws, _) = tokio_tungstenite::client_async_tls_with_config(
        req,
        tcp,
        Some(config),
        Some(Connector::Rustls(tls_config())),
    )
    .await?;
    Ok(ws)
}

/// CRLF line framing with the same [`MAX_FRAME`] cap as WebSocket frames.
struct Lines {
    io: BufReader<Box<dyn Io>>,
    buf: Vec<u8>,
    /// Inside an oversized line: drop bytes until its `\n`.
    skipping: bool,
}

impl Lines {
    async fn connect(url: &str, tls: bool) -> Result<Self, Error> {
        let bad = || std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad server URL");
        let uri: Uri = url.parse().map_err(|_| bad())?;
        let host = uri.host().ok_or_else(bad)?.trim_matches(['[', ']']);
        let port = uri.port_u16().unwrap_or(if tls { 6697 } else { 6667 });
        let tcp = tcp(host, port).await?;
        let io: Box<dyn Io> = if tls {
            let name = ServerName::try_from(host.to_owned()).map_err(|_| bad())?;
            let connector = tokio_rustls::TlsConnector::from(tls_config());
            Box::new(connector.connect(name, tcp).await?)
        } else {
            Box::new(tcp)
        };
        Ok(Self::new(io))
    }

    fn new(io: Box<dyn Io>) -> Self {
        Self {
            io: BufReader::new(io),
            buf: Vec::new(),
            skipping: false,
        }
    }

    async fn send(&mut self, line: &str) -> std::io::Result<()> {
        let io = self.io.get_mut();
        io.write_all(format!("{line}\r\n").as_bytes()).await?;
        io.flush().await
    }

    /// The next line without its terminator; `None` at EOF. State lives in
    /// `self` between awaits, so a cancelled call loses nothing.
    async fn read(&mut self) -> std::io::Result<Option<String>> {
        loop {
            let chunk = self.io.fill_buf().await?;
            if chunk.is_empty() {
                return Ok(None);
            }
            let (take, end) = match chunk.iter().position(|&b| b == b'\n') {
                Some(i) => (i + 1, true),
                None => (chunk.len(), false),
            };
            if !self.skipping {
                self.buf.extend_from_slice(&chunk[..take]);
            }
            self.io.consume(take);
            if self.buf.len() > MAX_FRAME {
                self.buf.clear();
                self.skipping = true;
            }
            if !end {
                continue;
            }
            if std::mem::take(&mut self.skipping) {
                tracing::warn!("skipping oversized line from server");
                continue;
            }
            let line = std::mem::take(&mut self.buf);
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.is_empty() {
                continue;
            }
            return Ok(Some(decode(line)));
        }
    }
}

/// UTF-8, else latin-1: older networks still carry it, and every byte maps.
fn decode(line: &[u8]) -> String {
    match std::str::from_utf8(line) {
        Ok(s) => s.to_owned(),
        Err(_) => line.iter().map(|&b| char::from(b)).collect(),
    }
}

/// rustls over the OS roots: the system's own verifier on iOS, whose trust
/// store isn't readable (D58); `rustls-native-certs` everywhere else.
pub fn tls_config() -> Arc<rustls::ClientConfig> {
    // Explicit provider: the dependency graph may enable both ring and aws-lc-rs.
    let builder = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .expect("ring supports the default TLS versions");
    #[cfg(target_os = "ios")]
    let builder = {
        use rustls_platform_verifier::BuilderVerifierExt;
        builder
            .with_platform_verifier()
            .expect("the iOS verifier takes ring")
    };
    #[cfg(not(target_os = "ios"))]
    let builder = {
        let mut roots = rustls::RootCertStore::empty();
        let found = rustls_native_certs::load_native_certs();
        for e in &found.errors {
            tracing::warn!("loading system certificates: {e}");
        }
        roots.add_parsable_certificates(found.certs);
        builder.with_root_certificates(roots)
    };
    Arc::new(builder.with_no_client_auth())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localhost_names() {
        for h in [
            "localhost",
            "chat.localhost",
            "Chat.LocalHost.",
            "a.b.localhost",
        ] {
            assert!(is_localhost(h), "{h}");
        }
        for h in ["localhost.com", "notlocalhost", "example.com", ""] {
            assert!(!is_localhost(h), "{h}");
        }
    }

    #[tokio::test]
    async fn lines_split_cap_and_decode() {
        let (client, mut server) = tokio::io::duplex(64);
        let mut lines = Lines::new(Box::new(client));
        let big = vec![b'x'; MAX_FRAME + 10];
        tokio::spawn(async move {
            server.write_all(b"PING a\r\nPI").await.unwrap();
            server.write_all(b"NG b\n\r\n").await.unwrap();
            server.write_all(&big).await.unwrap();
            server
                .write_all(b"\r\nPRIVMSG #c caf\xe9\r\nlast")
                .await
                .unwrap();
        });
        assert_eq!(lines.read().await.unwrap().as_deref(), Some("PING a"));
        assert_eq!(lines.read().await.unwrap().as_deref(), Some("PING b"));
        assert_eq!(
            lines.read().await.unwrap().as_deref(),
            Some("PRIVMSG #c café")
        );
        // An unterminated tail at EOF is dropped.
        assert_eq!(lines.read().await.unwrap(), None);
    }
}
