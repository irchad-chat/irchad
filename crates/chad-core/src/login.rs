//! Getting onto a network, without I/O, so the native driver (`conn.rs`, tokio)
//! and the browser's (chad-web) speak the same IRC.
//!
//! [`Login::start`], then every line from the server into [`Login::handle`],
//! sending [`Login::out`] after each, until a [`Step`] says it's over. Once
//! it's [`Step::Done`], [`Net::attach`] and give every further line to [`live`].
//! Answering `PING` is the transport's job ([`chad_irc::keepalive`]).

use chad_irc::Message;
use chad_irc::batch::Item;
use chad_irc::cap::Negotiator;
use chad_irc::register::{self, Reply};
use chad_irc::sasl::{self, Progress, Sasl};
use chad_proto::discovery::{Discovery, Registration};

use crate::ConnState;
use crate::net::{
    ACTIVITY_KEY, AVATAR_KEY, CHANNELS_KEY, DEVICES_KEY, DISPLAY_NAME_KEY, Net, RECEIPTS_KEY,
    VOICE_KEY, failure,
};

/// Who's logging in.
#[derive(Debug, Clone, Default)]
pub struct Account {
    /// The account, or a guest's nick.
    pub name: String,
    /// Empty for a guest.
    pub password: String,
    pub guest: bool,
    /// This install's device, for ergo's per-device login (D11).
    pub device: String,
    /// `Some(email)` to sign up instead of logging in.
    pub sign_up: Option<Option<String>>,
    /// Signed up already and waiting on the emailed code: don't sign up again.
    pub verifying: bool,
}

/// What [`Login::handle`] made of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Keep going.
    Continue,
    /// SASL (or NickServ, on a classic network) took the password: worth saving.
    LoggedIn,
    /// Signed up; the server emailed a code for [`Login::verify`]. The state's
    /// already set on `net`.
    NeedsVerification(String),
    /// Signed up and verified. `out` holds a `QUIT`: log in again with the new
    /// account so the device ID is in place (D11).
    Registered,
    /// Registered, as this nick.
    Done(String),
    /// The server ended it; a retry may work.
    Lost(String),
    /// Retrying won't help until the user changes something.
    Fatal(String),
}

pub struct Login {
    acct: Account,
    /// Lines to send, in order. Take them after every call.
    pub out: Vec<Message>,
    auth_started: bool,
    nick_tries: u32,
    sasl: Option<Sasl>,
    sasl_ok: bool,
}

impl Login {
    /// On a fresh connection: resets `net`'s CAP and ISUPPORT state and queues
    /// `CAP LS`, `NICK` and `USER`.
    pub fn start(net: &mut Net, d: &Discovery, acct: Account) -> Result<Self, String> {
        if acct.sign_up.is_some() && d.registration == Registration::Closed {
            return Err(format!("{} isn't taking sign-ups.", d.name));
        }
        net.caps = Negotiator::new();
        net.isupport.clear();
        let name = acct.name.as_str();
        let out = vec![
            Negotiator::start(),
            Message::new("NICK", [name]),
            Message::new("USER", [name, "0", "*", name]),
        ];
        Ok(Self {
            acct,
            out,
            auth_started: false,
            nick_tries: 0,
            sasl: None,
            sasl_ok: false,
        })
    }

    /// The code from the sign-up email.
    pub fn verify(&mut self, code: &str) {
        self.out
            .push(register::verify(&self.acct.name, code.trim()));
    }

    pub fn handle(&mut self, net: &mut Net, m: Message) -> Step {
        let mut out = net.caps.handle(&m);
        net.isupport.handle(&m);
        let mut logged_in = false;
        let mut verifying = None;

        if net.caps.ready() && !self.auth_started {
            self.auth_started = true;
            match self.start_auth(net, &mut out) {
                Ok(s) => self.sasl = s,
                Err(e) => return Step::Fatal(e),
            }
        }
        if let Some(s) = &mut self.sasl {
            match s.handle(&m) {
                Progress::Continue(more) => out.extend(more),
                Progress::LoggedIn => {
                    self.sasl = None;
                    self.sasl_ok = true;
                    out.push(net.caps.end());
                    logged_in = true;
                }
                Progress::Failed(e) => return Step::Fatal(sasl_error(e)),
            }
        }
        match Reply::parse(&m) {
            Some(Reply::Registered { .. } | Reply::Verified { .. }) => {
                self.out = vec![Message::new("QUIT", ["signed up"])];
                return Step::Registered;
            }
            Some(Reply::NeedsVerification { message, .. }) => {
                net.set_state(ConnState::NeedsVerification {
                    message: message.clone(),
                });
                verifying = Some(message);
            }
            Some(Reply::Failed { command, .. }) if command == "VERIFY" => {
                net.error("That code didn't work. Check the email and try again.");
            }
            Some(Reply::Failed { message, .. }) => {
                let plain = failure(&Item::Message(m.clone()));
                return Step::Fatal(plain.unwrap_or(message));
            }
            None => {}
        }
        // Classic (D72): the welcome, MOTD and server notices go to the server buffer.
        let numeric = m.command.len() == 3 && m.command.bytes().all(|b| b.is_ascii_digit());
        if net.classic()
            && ((numeric && !matches!(m.command.as_str(), "005" | "221")) || m.command == "NOTICE")
        {
            let _ = net.handle(m.clone());
        }
        let account = self.acct.name.as_str();
        match m.command.as_str() {
            "001" => {
                m.param(0).unwrap_or(account).clone_into(&mut net.info.nick);
                // Classic without SASL (or without CAP at all): NickServ, the old way.
                if net.classic() && !self.acct.guest && !self.sasl_ok {
                    self.auth_started = true;
                    let text = format!("IDENTIFY {account} {}", self.acct.password);
                    out.push(Message::new("PRIVMSG", ["NickServ", text.as_str()]));
                    logged_in = true;
                }
            }
            // RPL_LOGGEDIN: `<nick> <mask> <account> :You are now logged in as ...`.
            "900" => {
                net.account = m
                    .param(2)
                    .filter(|a| !a.is_empty() && a.len() <= 64)
                    .map(str::to_owned);
            }
            "376" | "422" => return Step::Done(net.info.nick.clone()),
            // Guests and classic networks take the next free `nick1`..`nick9`.
            "433" if (self.acct.guest || net.classic()) && self.nick_tries < 9 => {
                self.nick_tries += 1;
                let nick = format!("{account}{}", self.nick_tries);
                out.push(Message::new("NICK", [nick]));
            }
            "433" if self.acct.guest || net.classic() => {
                return Step::Fatal("That nickname is taken.".into());
            }
            "433" => return Step::Fatal("That username is taken.".into()),
            "432" => return Step::Fatal("That name isn't allowed here.".into()),
            "MODE" | "221" => {
                let _ = net.handle(m);
            }
            "ERROR" => return Step::Lost(m.params.last().cloned().unwrap_or_default()),
            _ => {}
        }
        self.out.extend(out);
        match verifying {
            Some(message) => Step::NeedsVerification(message),
            None if logged_in => Step::LoggedIn,
            None => Step::Continue,
        }
    }

    /// Once CAP LS is in: subscribe to metadata, then start SASL or a sign-up.
    /// Returns the SASL exchange, if one started.
    fn start_auth(&self, net: &mut Net, out: &mut Vec<Message>) -> Result<Option<Sasl>, String> {
        let acct = &self.acct;
        // Before CAP END, so the reattach burst includes everyone's value.
        if net.caps.enabled("draft/metadata-2") {
            let keys = [
                VOICE_KEY,
                DISPLAY_NAME_KEY,
                AVATAR_KEY,
                ACTIVITY_KEY,
                DEVICES_KEY,
                CHANNELS_KEY,
                RECEIPTS_KEY,
            ];
            out.push(Message::new(
                "METADATA",
                ["*", "SUB"].into_iter().chain(keys),
            ));
        }
        if acct.guest {
            out.push(net.caps.end());
            return Ok(None);
        }
        if let Some(email) = &acct.sign_up {
            if !net.caps.enabled("draft/account-registration") {
                return Err(format!("{} isn't taking sign-ups.", net.info.name));
            }
            // Already signed up: VERIFY works from a fresh connection.
            if !acct.verifying {
                out.push(register::register("*", email.as_deref(), &acct.password));
            }
            return Ok(None);
        }
        if !net.caps.enabled("sasl") {
            if net.classic() {
                // NickServ IDENTIFY once registered (`001`).
                out.push(net.caps.end());
                return Ok(None);
            }
            return Err(format!("{} doesn't support logging in.", net.info.name));
        }
        // `@device` is ergo's per-device login (D11); elsewhere it's just the account.
        let user = if net.classic() {
            acct.name.clone()
        } else {
            format!("{}@{}", acct.name, acct.device)
        };
        let mechs = net.caps.value("sasl").unwrap_or_default();
        let mut s = Sasl::new(&user, &acct.password, mechs).map_err(sasl_error)?;
        out.push(s.start());
        Ok(Some(s))
    }
}

/// A nick or account name someone typed: one IRC word, not a channel.
pub fn valid_nick(nick: &str) -> bool {
    !nick.is_empty()
        && !nick.starts_with(['#', ':'])
        && !nick.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// What the discovery document tells `net` about its network, before connecting.
pub fn discovered(net: &mut Net, d: &Discovery, guest: bool) {
    net.info.name.clone_from(&d.name);
    net.info.icon.clone_from(&d.icon);
    net.home = d.home.clone().filter(|h| net.is_home_name(h));
    net.info.uploads = d
        .chadd
        .uploads
        .as_ref()
        .filter(|_| !guest)
        .map(|u| u.max_bytes);
    net.info.gifs = d
        .chadd
        .gifs
        .as_ref()
        .filter(|_| !guest)
        .map(|g| g.provider.chars().take(40).collect());
    net.info.games = d.chadd.games && !guest;
    net.info.emoji = d.chadd.emoji;
}

/// Registered: a line from the server into `net`. `Err` ends the connection.
pub fn live(net: &mut Net, m: Message) -> Result<(), String> {
    if m.command == "ERROR" {
        return Err(m.params.last().cloned().unwrap_or_default());
    }
    net.handle(m)
        .map_err(|_| "The server sent more than we can hold.".into())
}

fn sasl_error(e: sasl::Error) -> String {
    match e {
        sasl::Error::Rejected => "Wrong username or password.".into(),
        sasl::Error::NoMechanism => "This server's login method isn't supported.".into(),
        other => {
            tracing::warn!("SASL: {other}");
            "Logging in failed. Try again later.".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::{ConnState, NetworkInfo};

    fn net() -> Net {
        Net::new(
            Store::memory().unwrap(),
            NetworkInfo {
                network: "https://x".into(),
                name: "X".into(),
                icon: None,
                nick: String::new(),
                state: ConnState::Connecting,
                guest: false,
                oper: false,
                uploads: None,
                gifs: None,
                games: false,
                emoji: false,
                receipts: false,
            },
        )
    }

    fn discovery(registration: &str) -> Discovery {
        let doc = format!(
            r#"{{"version":1,"name":"X","irc":{{"websocket":"wss://x/irc"}},"chadd":{{"api":"https://x/api"}},"livekit":{{"url":"wss://x/lk"}},"registration":"{registration}"}}"#
        );
        crate::discovery::parse(doc.as_bytes()).unwrap()
    }

    fn acct(name: &str, guest: bool) -> Account {
        Account {
            name: name.into(),
            password: if guest { String::new() } else { "pw".into() },
            guest,
            device: "laptop".into(),
            ..Default::default()
        }
    }

    /// Feeds `line`, returning the step and what to send.
    fn feed(l: &mut Login, n: &mut Net, line: &str) -> (Step, Vec<String>) {
        let step = l.handle(n, Message::parse(line).unwrap());
        let out = l.out.drain(..).map(|m| m.to_line().unwrap()).collect();
        (step, out)
    }

    #[test]
    fn sasl_login_to_end_of_motd() {
        let mut n = net();
        let mut l = Login::start(&mut n, &discovery("open"), acct("bob", false)).unwrap();
        let first: Vec<_> = l.out.drain(..).map(|m| m.command).collect();
        assert_eq!(first, ["CAP", "NICK", "USER"]);

        let (_, out) = feed(&mut l, &mut n, ":s CAP * LS :sasl=PLAIN");
        assert_eq!(out, ["CAP REQ sasl"]);
        let (_, out) = feed(&mut l, &mut n, ":s CAP * ACK :sasl");
        assert_eq!(out, ["AUTHENTICATE PLAIN"]);
        let (_, out) = feed(&mut l, &mut n, "AUTHENTICATE +");
        // `bob@laptop`: ergo's per-device login (D11).
        assert_eq!(out, ["AUTHENTICATE AGJvYkBsYXB0b3AAcHc="]); // "\0bob@laptop\0pw"
        let (step, out) = feed(&mut l, &mut n, ":s 903 bob :SASL authentication successful");
        assert_eq!((step, out), (Step::LoggedIn, vec!["CAP END".to_owned()]));

        feed(&mut l, &mut n, ":s 001 bob :Welcome");
        assert_eq!(n.info.nick, "bob");
        let (step, _) = feed(&mut l, &mut n, ":s 376 bob :End of MOTD");
        assert_eq!(step, Step::Done("bob".into()));
    }

    #[test]
    fn wrong_password_is_fatal() {
        let mut n = net();
        let mut l = Login::start(&mut n, &discovery("open"), acct("bob", false)).unwrap();
        feed(&mut l, &mut n, ":s CAP * LS :sasl=PLAIN");
        feed(&mut l, &mut n, ":s CAP * ACK :sasl");
        feed(&mut l, &mut n, "AUTHENTICATE +");
        let (step, _) = feed(&mut l, &mut n, ":s 904 bob :SASL authentication failed");
        assert_eq!(step, Step::Fatal("Wrong username or password.".into()));
    }

    #[test]
    fn guest_takes_next_free_nick() {
        let mut n = net();
        let mut l = Login::start(&mut n, &discovery("open"), acct("zed", true)).unwrap();
        l.out.clear();
        let (_, out) = feed(&mut l, &mut n, ":s CAP * LS :draft/metadata-2");
        assert_eq!(out, ["CAP REQ draft/metadata-2"]);
        let (_, out) = feed(&mut l, &mut n, ":s CAP * ACK :draft/metadata-2");
        assert!(out[0].starts_with("METADATA * SUB "), "{out:?}");
        assert_eq!(out[1], "CAP END");
        for i in 1..=9 {
            let (step, out) = feed(&mut l, &mut n, ":s 433 * zed :Nickname is in use");
            assert_eq!((step, out), (Step::Continue, vec![format!("NICK zed{i}")]));
        }
        let (step, _) = feed(&mut l, &mut n, ":s 433 * zed9 :Nickname is in use");
        assert_eq!(step, Step::Fatal("That nickname is taken.".into()));
    }

    #[test]
    fn account_name_taken_is_fatal() {
        let mut n = net();
        let mut l = Login::start(&mut n, &discovery("open"), acct("bob", false)).unwrap();
        let (step, _) = feed(&mut l, &mut n, ":s 433 * bob :Nickname is in use");
        assert_eq!(step, Step::Fatal("That username is taken.".into()));
    }

    #[test]
    fn sign_up_needs_open_registration() {
        let mut n = net();
        let a = Account {
            sign_up: Some(None),
            ..acct("bob", false)
        };
        let e = Login::start(&mut n, &discovery("closed"), a).err();
        assert_eq!(e.as_deref(), Some("X isn't taking sign-ups."));
    }

    #[test]
    fn error_is_lost_and_live_stops_on_it() {
        let mut n = net();
        let mut l = Login::start(&mut n, &discovery("open"), acct("bob", false)).unwrap();
        let (step, _) = feed(&mut l, &mut n, "ERROR :Closing link");
        assert_eq!(step, Step::Lost("Closing link".into()));
        let e = live(&mut n, Message::parse("ERROR :bye").unwrap());
        assert_eq!(e, Err("bye".into()));
    }
}
