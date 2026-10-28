//! CAP 302 negotiation, including `cap-notify` (`NEW` / `DEL`) after registration.
//!
//! Sans-IO: feed every `CAP` line to [`Negotiator::handle`] and send whatever it
//! returns. Once [`Negotiator::ready`] is true the caller does SASL / `REGISTER`
//! if wanted, then sends [`Negotiator::end`].

use std::collections::{BTreeMap, BTreeSet};

use crate::Message;

/// Everything we request when the server offers it. ergo 2.19 offers all of these
/// (see `docs/notes/ergo-caps.md`). Never add `autoreplay-missed`-style replay caps (D11).
pub const WANTED: &[&str] = &[
    "account-notify",
    "account-tag",
    "away-notify",
    "batch",
    "cap-notify",
    "chghost",
    "draft/account-registration",
    "draft/channel-rename",
    "draft/chathistory",
    "draft/event-playback",
    "draft/extended-isupport",
    "draft/message-redaction",
    "draft/metadata-2",
    "draft/multiline",
    "draft/persistence",
    "draft/pre-away",
    "draft/read-marker",
    "echo-message",
    "extended-join",
    "extended-monitor",
    "invite-notify",
    "labeled-response",
    "message-tags",
    "multi-prefix",
    "no-implicit-names",
    "sasl",
    "server-time",
    "setname",
    "standard-replies",
];

/// 512 minus CRLF; we send no tags on CAP lines.
const MAX_LINE: usize = 510;
/// ergo offers ~40. Bounds a hostile server's endless `CAP LS *` (D14).
const MAX_AVAILABLE: usize = 256;

#[derive(Debug, Default)]
pub struct Negotiator {
    /// Offered by the server, with the value after `=` (`""` if none).
    available: BTreeMap<String, String>,
    enabled: BTreeSet<String>,
    ls_done: bool,
    pending_reqs: usize,
    ended: bool,
}

impl Negotiator {
    pub fn new() -> Self {
        Self::default()
    }

    /// The first line to send, before `NICK` / `USER`.
    pub fn start() -> Message {
        Message::new("CAP", ["LS", "302"])
    }

    pub fn end(&mut self) -> Message {
        self.ended = true;
        Message::new("CAP", ["END"])
    }

    /// The initial LS is complete and every REQ has been answered.
    pub fn ready(&self) -> bool {
        self.ls_done && self.pending_reqs == 0 && !self.ended
    }

    pub fn enabled(&self, cap: &str) -> bool {
        self.enabled.contains(cap)
    }

    /// The server's value for an offered cap, e.g. `"PLAIN,SCRAM-SHA-256"` for `sasl`.
    pub fn value(&self, cap: &str) -> Option<&str> {
        self.available.get(cap).map(String::as_str)
    }

    /// Handles one `CAP` message; anything else is ignored. Returns lines to send.
    pub fn handle(&mut self, msg: &Message) -> Vec<Message> {
        if msg.command != "CAP" {
            return Vec::new();
        }
        // params: <nick> <subcmd> [*] <list>
        let (Some(sub), Some(list)) = (msg.param(1), msg.params.last()) else {
            return Vec::new();
        };
        let more = msg.params.len() == 4 && msg.param(2) == Some("*");
        match sub {
            "LS" => {
                self.offer(list);
                if more || self.ls_done {
                    return Vec::new();
                }
                self.ls_done = true;
                let offered: Vec<String> = self.available.keys().cloned().collect();
                self.request(offered.iter().map(String::as_str))
            }
            "NEW" => {
                let new: Vec<String> = parse(list).map(|(k, _)| k.to_owned()).collect();
                self.offer(list);
                self.request(new.iter().map(String::as_str))
            }
            "DEL" => {
                for (k, _) in parse(list) {
                    self.available.remove(k);
                    self.enabled.remove(k);
                }
                Vec::new()
            }
            "ACK" => {
                self.pending_reqs = self.pending_reqs.saturating_sub(1);
                for (k, _) in parse(list) {
                    match k.strip_prefix('-') {
                        Some(k) => self.enabled.remove(k),
                        None => self.enabled.insert(k.to_owned()),
                    };
                }
                Vec::new()
            }
            "NAK" => {
                self.pending_reqs = self.pending_reqs.saturating_sub(1);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn offer(&mut self, list: &str) {
        for (k, v) in parse(list) {
            if self.available.len() >= MAX_AVAILABLE && !self.available.contains_key(k) {
                continue;
            }
            self.available.insert(k.to_owned(), v.to_owned());
        }
    }

    /// REQs for the wanted subset of `offered`, split to fit the line limit.
    fn request<'a>(&mut self, offered: impl Iterator<Item = &'a str>) -> Vec<Message> {
        let prefix = "CAP REQ :".len();
        let mut lines: Vec<String> = Vec::new();
        for cap in offered.filter(|c| WANTED.contains(c) && !self.enabled.contains(*c)) {
            match lines.last_mut() {
                Some(l) if prefix + l.len() + 1 + cap.len() <= MAX_LINE => {
                    l.push(' ');
                    l.push_str(cap);
                }
                _ => lines.push(cap.to_owned()),
            }
        }
        self.pending_reqs += lines.len();
        lines
            .into_iter()
            .map(|l| Message::new("CAP", ["REQ".to_owned(), l]))
            .collect()
    }
}

fn parse(list: &str) -> impl Iterator<Item = (&str, &str)> {
    list.split(' ')
        .filter(|s| !s.is_empty())
        .map(|s| s.split_once('=').unwrap_or((s, "")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(n: &mut Negotiator, line: &str) -> Vec<String> {
        n.handle(&Message::parse(line).unwrap())
            .iter()
            .map(|m| m.to_line().unwrap())
            .collect()
    }

    #[test]
    fn multiline_ls_then_ack_then_end() {
        let mut n = Negotiator::new();
        assert_eq!(Negotiator::start().to_line().unwrap(), "CAP LS 302");
        assert!(
            feed(
                &mut n,
                ":chat.localhost CAP * LS * :account-notify batch znc.in/playback"
            )
            .is_empty()
        );
        assert!(!n.ready());
        let reqs = feed(
            &mut n,
            ":chat.localhost CAP * LS :sasl=PLAIN,EXTERNAL,SCRAM-SHA-256 server-time",
        );
        assert_eq!(reqs, ["CAP REQ :account-notify batch sasl server-time"]);
        assert!(!n.ready());
        assert_eq!(n.value("sasl"), Some("PLAIN,EXTERNAL,SCRAM-SHA-256"));

        feed(
            &mut n,
            ":chat.localhost CAP * ACK :account-notify batch sasl server-time",
        );
        assert!(n.ready() && n.enabled("sasl") && !n.enabled("znc.in/playback"));
        assert_eq!(n.end().to_line().unwrap(), "CAP END");
        assert!(!n.ready());
    }

    #[test]
    fn nak_enables_nothing_but_unblocks() {
        let mut n = Negotiator::new();
        feed(&mut n, "CAP * LS :batch echo-message");
        feed(&mut n, "CAP * NAK :batch echo-message");
        assert!(n.ready() && !n.enabled("batch"));
    }

    #[test]
    fn cap_notify_new_and_del() {
        let mut n = Negotiator::new();
        feed(&mut n, "CAP * LS :batch");
        feed(&mut n, "CAP * ACK :batch");
        n.end();

        assert_eq!(
            feed(&mut n, "CAP alice NEW :away-notify ergo.chat/nope"),
            ["CAP REQ away-notify"]
        );
        feed(&mut n, "CAP alice ACK away-notify");
        assert!(n.enabled("away-notify"));

        // Re-advertising something already enabled doesn't re-request it.
        assert!(feed(&mut n, "CAP alice NEW :away-notify sasl=PLAIN").len() == 1);
        assert_eq!(n.value("sasl"), Some("PLAIN"));

        feed(&mut n, "CAP alice DEL :away-notify");
        assert!(!n.enabled("away-notify") && n.value("away-notify").is_none());
        feed(&mut n, "CAP alice ACK :-batch");
        assert!(!n.enabled("batch"));
    }

    #[test]
    fn full_ergo_ls_requests_everything_wanted_within_line_limit() {
        let ls = "account-notify account-tag away-notify batch cap-notify chghost draft/account-registration=before-connect draft/channel-rename draft/chathistory draft/event-playback draft/extended-isupport draft/languages=17,en,~bs draft/message-redaction draft/metadata-2=before-connect,max-subs=100,max-keys=100 draft/metadata-3=before-connect,max-subs=100,max-keys=100 draft/multiline=max-bytes=4096,max-lines=100 draft/persistence draft/pre-away draft/read-marker draft/relaymsg=/ draft/whoami echo-message ergo.chat/nope extended-join extended-monitor invite-notify labeled-response message-tags multi-prefix no-implicit-names sasl=PLAIN,EXTERNAL,SCRAM-SHA-256 server-time setname standard-replies userhost-in-names znc.in/playback znc.in/self-message";
        let mut n = Negotiator::new();
        let reqs = n.handle(&Message::new("CAP", ["*", "LS", ls]));
        let lines: Vec<String> = reqs.iter().map(|m| m.to_line().unwrap()).collect();
        assert!(lines.iter().all(|l| l.len() <= MAX_LINE));
        let requested: BTreeSet<&str> = reqs.iter().flat_map(|m| m.params[1].split(' ')).collect();
        assert_eq!(requested, WANTED.iter().copied().collect());

        // A REQ list longer than one line gets split.
        let long: Vec<&str> = std::iter::repeat_n(WANTED, 3).flatten().copied().collect();
        let reqs = Negotiator::new().request(long.into_iter());
        assert!(reqs.len() > 1);
        assert!(reqs.iter().all(|m| m.to_line().unwrap().len() <= MAX_LINE));
    }

    #[test]
    fn endless_ls_is_bounded() {
        let mut n = Negotiator::new();
        for i in 0..1000 {
            feed(&mut n, &format!("CAP * LS * :junk{i}a junk{i}b"));
        }
        assert_eq!(n.available.len(), MAX_AVAILABLE);
    }
}
