//! Staying connected, without I/O, so every transport (tokio's in [`crate::ws`],
//! the browser's in chad-web) behaves the same: answer `PING`, ping a quiet
//! server, give up on a silent one, and space the reconnects.

use std::time::Duration;

use crate::Message;

/// Silence before we PING, then how long we wait for anything at all.
pub const IDLE: Duration = Duration::from_secs(90);
pub const PONG_WAIT: Duration = Duration::from_secs(30);
/// After we send, how long the server may stay quiet before we PING: a dead
/// link swallows writes, and someone is waiting on that message.
pub const SENT_WAIT: Duration = Duration::from_secs(10);

/// The transport waits [`Keepalive::wait`] for each line. Lines go through
/// [`Keepalive::line`]; a wait that runs out calls [`Keepalive::silence`].
#[derive(Debug, Default)]
pub struct Keepalive {
    pinged: bool,
}

/// The server stopped responding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeout;

impl Keepalive {
    pub fn new() -> Self {
        Self::default()
    }

    /// How long to wait for the next line.
    pub fn wait(&self) -> Duration {
        if self.pinged { PONG_WAIT } else { IDLE }
    }

    /// Something came in, even a line that didn't parse: the server's there.
    pub fn heard(&mut self) {
        self.pinged = false;
    }

    /// A line came in. A `PING` gets its `PONG` back and goes no further.
    pub fn line(&mut self, m: &Message) -> Option<Message> {
        self.heard();
        (m.command == "PING").then(|| Message::new("PONG", m.params.clone()))
    }

    /// [`Self::wait`] ran out: the first time a `PING` to send, then [`Timeout`].
    pub fn silence(&mut self) -> Result<Message, Timeout> {
        if std::mem::replace(&mut self.pinged, true) {
            return Err(Timeout);
        }
        Ok(Message::new("PING", ["keepalive"]))
    }
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

    #[test]
    fn pongs_pings_then_times_out() {
        let mut k = Keepalive::new();
        let ping = Message::parse("PING :abc").unwrap();
        let pong = k.line(&ping).unwrap();
        assert_eq!(pong.to_line().unwrap(), "PONG abc");
        assert!(k.line(&Message::parse("PRIVMSG #a :hi").unwrap()).is_none());

        assert_eq!(k.wait(), IDLE);
        assert_eq!(k.silence().unwrap().command, "PING");
        assert_eq!(k.wait(), PONG_WAIT);
        assert_eq!(k.silence(), Err(Timeout));

        // Anything at all counts as an answer.
        let mut k = Keepalive::new();
        k.silence().unwrap();
        k.line(&Message::parse("NOTICE * :hi").unwrap());
        assert_eq!(k.wait(), IDLE);
        assert!(k.silence().is_ok());
    }
}
