//! SASL over `AUTHENTICATE`: SCRAM-SHA-256 (RFC 5802 / 7677), falling back to PLAIN.
//!
//! Sans-IO, like [`crate::cap`]: send [`Sasl::start`], feed every message to
//! [`Sasl::handle`] until it reports [`Progress::LoggedIn`] or [`Progress::Failed`].

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::Message;

type HmacSha256 = Hmac<Sha256>;

/// `AUTHENTICATE` payloads are sent in base64 chunks of this many bytes.
const CHUNK: usize = 400;
/// Guards against a hostile server pinning our CPU. ergo uses 4096.
const MAX_ITERATIONS: u32 = 1 << 20;
/// SCRAM server messages are ~100 bytes; a server streaming chunks forever is hostile.
const MAX_PAYLOAD: usize = 8 << 10;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("the server offers no SASL mechanism we support")]
    NoMechanism,
    #[error("wrong account name or password")]
    Rejected,
    #[error("already logged in")]
    AlreadyAuthenticated,
    #[error("authentication aborted")]
    Aborted,
    #[error("SASL protocol error: {0}")]
    Protocol(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanism {
    ScramSha256,
    Plain,
}

impl Mechanism {
    fn name(self) -> &'static str {
        match self {
            Self::ScramSha256 => "SCRAM-SHA-256",
            Self::Plain => "PLAIN",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Progress {
    /// Send these (possibly none) and keep feeding messages.
    Continue(Vec<Message>),
    LoggedIn,
    Failed(Error),
}

#[derive(Debug)]
enum State {
    MechSent,
    ScramFirstSent {
        nonce: String,
        bare: String,
    },
    ScramFinalSent {
        server_key: Vec<u8>,
        auth_msg: String,
    },
    Sent,
}

#[derive(Debug)]
pub struct Sasl {
    user: String,
    password: String,
    mech: Mechanism,
    plain_ok: bool,
    state: State,
    /// Incoming `AUTHENTICATE` chunks not yet terminated.
    buf: String,
    nonce: String,
}

impl Sasl {
    /// `mechs` is the `sasl` cap value (`"PLAIN,SCRAM-SHA-256"`); empty means unknown,
    /// in which case we try SCRAM and then PLAIN. `user` may carry a device ID (`bob@laptop`).
    pub fn new(user: &str, password: &str, mechs: &str) -> Result<Self, Error> {
        let has = |m: Mechanism| mechs.is_empty() || mechs.split(',').any(|s| s == m.name());
        let plain_ok = has(Mechanism::Plain);
        let mech = if has(Mechanism::ScramSha256) {
            Mechanism::ScramSha256
        } else if plain_ok {
            Mechanism::Plain
        } else {
            return Err(Error::NoMechanism);
        };
        let mut nonce = [0u8; 18];
        getrandom::fill(&mut nonce).map_err(|_| Error::Protocol("no randomness for nonce"))?;
        Ok(Self {
            user: user.into(),
            password: password.into(),
            mech,
            plain_ok,
            state: State::MechSent,
            buf: String::new(),
            nonce: B64.encode(nonce),
        })
    }

    pub fn mechanism(&self) -> Mechanism {
        self.mech
    }

    pub fn start(&mut self) -> Message {
        self.state = State::MechSent;
        Message::new("AUTHENTICATE", [self.mech.name()])
    }

    pub fn handle(&mut self, msg: &Message) -> Progress {
        match msg.command.as_str() {
            "AUTHENTICATE" => {
                let chunk = msg.param(0).unwrap_or("+");
                if chunk != "+" {
                    self.buf.push_str(chunk);
                }
                if self.buf.len() > MAX_PAYLOAD {
                    self.buf.clear();
                    return Progress::Failed(Error::Protocol("oversized payload"));
                }
                if chunk.len() == CHUNK {
                    return Progress::Continue(Vec::new());
                }
                let data = std::mem::take(&mut self.buf);
                match B64.decode(data) {
                    Ok(payload) => match self.step(&payload) {
                        Ok(out) => Progress::Continue(out),
                        Err(e) => Progress::Failed(e),
                    },
                    Err(_) => Progress::Failed(Error::Protocol("bad base64")),
                }
            }
            "903" => Progress::LoggedIn,
            // Fall back only if SCRAM was refused before we sent a proof, so a wrong
            // password costs one attempt against ergo's login throttle, not two.
            "904"
                if self.mech == Mechanism::ScramSha256
                    && self.plain_ok
                    && matches!(self.state, State::MechSent | State::ScramFirstSent { .. }) =>
            {
                self.mech = Mechanism::Plain;
                Progress::Continue(vec![self.start()])
            }
            "904" => Progress::Failed(Error::Rejected),
            "905" | "906" => Progress::Failed(Error::Aborted),
            "907" => Progress::Failed(Error::AlreadyAuthenticated),
            _ => Progress::Continue(Vec::new()),
        }
    }

    fn step(&mut self, payload: &[u8]) -> Result<Vec<Message>, Error> {
        let reply = match (&self.state, self.mech) {
            (State::MechSent, Mechanism::Plain) => {
                // authzid stays empty: ergo compares it to the authcid *with* the device ID.
                self.state = State::Sent;
                format!("\0{}\0{}", self.user, self.password)
            }
            (State::MechSent, Mechanism::ScramSha256) => {
                let user = self.user.replace('=', "=3D").replace(',', "=2C");
                let bare = format!("n={user},r={}", self.nonce);
                let first = format!("n,,{bare}");
                self.state = State::ScramFirstSent {
                    nonce: self.nonce.clone(),
                    bare,
                };
                first
            }
            (State::ScramFirstSent { nonce, bare }, _) => {
                let server_first =
                    std::str::from_utf8(payload).map_err(|_| Error::Protocol("not UTF-8"))?;
                let attr = |k: &str| {
                    server_first
                        .split(',')
                        .find_map(|kv| kv.strip_prefix(k)?.strip_prefix('='))
                };
                let rnonce = attr("r").ok_or(Error::Protocol("no nonce"))?;
                if !rnonce.starts_with(nonce.as_str()) || rnonce.len() == nonce.len() {
                    return Err(Error::Protocol("server nonce doesn't extend ours"));
                }
                let salt = attr("s")
                    .and_then(|s| B64.decode(s).ok())
                    .ok_or(Error::Protocol("bad salt"))?;
                let iters: u32 = attr("i")
                    .and_then(|i| i.parse().ok())
                    .filter(|i| (1..=MAX_ITERATIONS).contains(i))
                    .ok_or(Error::Protocol("bad iteration count"))?;

                // shortcut: no SASLprep, so non-ASCII passwords must already be NFKC; add the
                // `stringprep` crate if users hit it.
                let mut salted = [0u8; 32];
                pbkdf2::pbkdf2_hmac::<Sha256>(self.password.as_bytes(), &salt, iters, &mut salted);
                let client_key = hmac(&salted, b"Client Key");
                let server_key = hmac(&salted, b"Server Key");
                let stored_key = Sha256::digest(&client_key);

                let without_proof = format!("c=biws,r={rnonce}");
                let auth_msg = format!("{bare},{server_first},{without_proof}");
                let sig = hmac(&stored_key, auth_msg.as_bytes());
                let proof: Vec<u8> = client_key.iter().zip(&sig).map(|(a, b)| a ^ b).collect();
                self.state = State::ScramFinalSent {
                    server_key,
                    auth_msg,
                };
                format!("{without_proof},p={}", B64.encode(proof))
            }
            (
                State::ScramFinalSent {
                    server_key,
                    auth_msg,
                },
                _,
            ) => {
                let server_final =
                    std::str::from_utf8(payload).map_err(|_| Error::Protocol("not UTF-8"))?;
                if server_final.starts_with("e=") {
                    return Err(Error::Rejected);
                }
                let v = server_final
                    .strip_prefix("v=")
                    .and_then(|v| B64.decode(v).ok())
                    .ok_or(Error::Protocol("bad server-final"))?;
                // A server that can't prove it knows our key is not the server we registered with.
                let mut mac = HmacSha256::new_from_slice(server_key).expect("any key length");
                mac.update(auth_msg.as_bytes());
                mac.verify_slice(&v)
                    .map_err(|_| Error::Protocol("server signature mismatch"))?;
                self.state = State::Sent;
                String::new()
            }
            (State::Sent, _) => return Err(Error::Protocol("unexpected AUTHENTICATE")),
        };
        Ok(authenticate(reply.as_bytes()))
    }
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// One payload as `AUTHENTICATE` lines: 400-byte base64 chunks, `+` for empty or
/// to terminate an exact multiple of 400.
fn authenticate(payload: &[u8]) -> Vec<Message> {
    let b64 = B64.encode(payload);
    let mut out: Vec<Message> = b64
        .as_bytes()
        .chunks(CHUNK)
        .map(|c| {
            Message::new(
                "AUTHENTICATE",
                [std::str::from_utf8(c).expect("base64 is ASCII")],
            )
        })
        .collect();
    if b64.len().is_multiple_of(CHUNK) {
        out.push(Message::new("AUTHENTICATE", ["+"]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &str) -> Message {
        Message::parse(s).unwrap()
    }

    fn sent(p: Progress) -> Vec<String> {
        match p {
            Progress::Continue(m) => m.iter().map(|m| m.to_line().unwrap()).collect(),
            other => panic!("expected Continue, got {other:?}"),
        }
    }

    fn decoded(lines: &[String]) -> String {
        let b64 = lines[0].strip_prefix("AUTHENTICATE ").unwrap();
        String::from_utf8(B64.decode(b64).unwrap()).unwrap()
    }

    fn auth(payload: &str) -> Message {
        Message::new("AUTHENTICATE", [B64.encode(payload)])
    }

    /// RFC 7677 section 3 test vector.
    #[test]
    fn scram_rfc7677_vector() {
        let mut s = Sasl::new("user", "pencil", "PLAIN,EXTERNAL,SCRAM-SHA-256").unwrap();
        s.nonce = "rOprNGfwEbeRWgbNEkqO".into();
        assert_eq!(s.start().to_line().unwrap(), "AUTHENTICATE SCRAM-SHA-256");

        let first = sent(s.handle(&line("AUTHENTICATE +")));
        assert_eq!(decoded(&first), "n,,n=user,r=rOprNGfwEbeRWgbNEkqO");

        let final_ = sent(s.handle(&auth(
            "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096",
        )));
        assert_eq!(
            decoded(&final_),
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );

        let ack = sent(s.handle(&auth("v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=")));
        assert_eq!(ack, ["AUTHENTICATE +"]);
        assert_eq!(
            s.handle(&line(
                ":chat.localhost 900 user user!u@h user :You are now logged in"
            )),
            Progress::Continue(vec![])
        );
        assert_eq!(
            s.handle(&line(":chat.localhost 903 user :ok")),
            Progress::LoggedIn
        );
    }

    #[test]
    fn scram_rejects_forged_server_signature_and_foreign_nonce() {
        let mut s = Sasl::new("user", "pencil", "SCRAM-SHA-256").unwrap();
        s.nonce = "rOprNGfwEbeRWgbNEkqO".into();
        s.start();
        s.handle(&line("AUTHENTICATE +"));
        sent(s.handle(&auth(
            "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096",
        )));
        assert_eq!(
            s.handle(&auth("v=AAAATRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=")),
            Progress::Failed(Error::Protocol("server signature mismatch"))
        );

        let mut s = Sasl::new("user", "pencil", "SCRAM-SHA-256").unwrap();
        s.start();
        s.handle(&line("AUTHENTICATE +"));
        assert!(matches!(
            s.handle(&auth("r=someoneelse,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096")),
            Progress::Failed(Error::Protocol(_))
        ));
    }

    #[test]
    fn plain_with_device_id_and_empty_authzid() {
        let mut s = Sasl::new("bob@laptop", "hunter2", "PLAIN").unwrap();
        assert_eq!(s.mechanism(), Mechanism::Plain);
        s.start();
        let out = sent(s.handle(&line("AUTHENTICATE +")));
        assert_eq!(decoded(&out), "\0bob@laptop\0hunter2");
        assert_eq!(
            s.handle(&line("904 * :bad")),
            Progress::Failed(Error::Rejected)
        );
    }

    #[test]
    fn falls_back_to_plain_only_before_proof() {
        let mut s = Sasl::new("bob", "pw", "").unwrap();
        s.start();
        let out = sent(s.handle(&line("904 * :SASL authentication failed")));
        assert_eq!(out, ["AUTHENTICATE PLAIN"]);
        assert_eq!(s.mechanism(), Mechanism::Plain);

        let mut s = Sasl::new("bob", "pw", "PLAIN,SCRAM-SHA-256").unwrap();
        s.start();
        s.handle(&line("AUTHENTICATE +"));
        let nonce = s.nonce.clone();
        sent(s.handle(&auth(&format!("r={nonce}xyz,s=AAAA,i=4096"))));
        assert_eq!(
            s.handle(&line("904 * :bad")),
            Progress::Failed(Error::Rejected)
        );

        assert_eq!(
            Sasl::new("bob", "pw", "EXTERNAL").unwrap_err(),
            Error::NoMechanism
        );
    }

    #[test]
    fn long_payloads_chunk_both_ways() {
        let lines = authenticate(&[b'x'; 300]); // 400 base64 bytes exactly
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].params, ["+"]);
        assert_eq!(authenticate(&[b'x'; 301]).len(), 2);
        assert_eq!(authenticate(b"")[0].params, ["+"]);

        // Incoming: a 400-byte chunk waits for the rest.
        let mut s = Sasl::new("u", "p", "PLAIN").unwrap();
        s.state = State::Sent;
        let b64 = B64.encode([b'x'; 300]);
        assert_eq!(
            s.handle(&Message::new("AUTHENTICATE", [b64.as_str()])),
            Progress::Continue(vec![])
        );
        assert_eq!(s.buf.len(), 400);

        // A server that never terminates the payload is cut off (D14).
        let endless = (0..100)
            .map(|_| s.handle(&Message::new("AUTHENTICATE", [b64.as_str()])))
            .find(|p| matches!(p, Progress::Failed(_)));
        assert!(endless.is_some() && s.buf.is_empty());
    }
}
