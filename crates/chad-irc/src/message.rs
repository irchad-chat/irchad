//! One IRC line: `[@tags] [:source] COMMAND [params...] [:trailing]`.
//!
//! Follows <https://modern.ircdocs.horse/#message-format> and the IRCv3
//! message-tags escaping rules. Anything that parses also serializes, and
//! reparsing the result gives back an equal [`Message`].

use std::fmt::Write as _;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("line has no command")]
    NoCommand,
    #[error("invalid command {0:?}")]
    BadCommand(String),
    #[error("line contains CR, LF or NUL")]
    ForbiddenChar,
    #[error("invalid tag key {0:?}")]
    BadTagKey(String),
    #[error("invalid source {0:?}")]
    BadSource(String),
    #[error("param {0} can't be sent as a middle param")]
    BadMiddleParam(usize),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Message {
    /// In wire order. A missing value and an empty value are the same thing, so both are `""`.
    pub tags: Vec<(String, String)>,
    pub source: Option<String>,
    pub command: String,
    pub params: Vec<String>,
}

impl Message {
    pub fn new(
        command: impl Into<String>,
        params: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            command: command.into(),
            params: params.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    pub fn with_tag(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.tags.push((key.into(), value.into()));
        self
    }

    /// Value of a tag; the last one wins if a key repeats.
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn param(&self, i: usize) -> Option<&str> {
        self.params.get(i).map(String::as_str)
    }

    /// The nick part of a `nick!user@host` source.
    pub fn source_nick(&self) -> Option<&str> {
        let s = self.source.as_deref()?;
        Some(s.split(['!', '@']).next().unwrap_or(s))
    }

    /// Parses one line. A trailing `\r\n` or `\n` is stripped.
    pub fn parse(line: &str) -> Result<Self, Error> {
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.contains(['\r', '\n', '\0']) {
            return Err(Error::ForbiddenChar);
        }
        let mut rest = line.trim_start_matches(' ');
        let mut msg = Message::default();

        if let Some(r) = rest.strip_prefix('@') {
            let (raw, r) = split_token(r);
            for item in raw.split(';').filter(|s| !s.is_empty()) {
                let (k, v) = item.split_once('=').unwrap_or((item, ""));
                // Skip, don't reject: ergo relays client tags its looser check
                // accepts (`+a.b`, `+a/b/c`), and dropping the whole line would
                // let any plain client hide its messages from IrChad users.
                if valid_tag_key(k) {
                    msg.tags.push((k.into(), unescape_tag(v)));
                }
            }
            rest = r;
        }
        if let Some(r) = rest.strip_prefix(':') {
            let (src, r) = split_token(r);
            if src.is_empty() {
                return Err(Error::BadSource(src.into()));
            }
            msg.source = Some(src.into());
            rest = r;
        }

        let (cmd, mut rest) = split_token(rest);
        if cmd.is_empty() {
            return Err(Error::NoCommand);
        }
        if !cmd.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(Error::BadCommand(cmd.into()));
        }
        msg.command = cmd.into();

        while !rest.is_empty() {
            if let Some(trailing) = rest.strip_prefix(':') {
                msg.params.push(trailing.into());
                break;
            }
            let (p, r) = split_token(rest);
            msg.params.push(p.into());
            rest = r;
        }
        Ok(msg)
    }

    /// Serializes without the line ending. Fails on anything that would
    /// change meaning on the wire, such as a CR/LF in a param.
    pub fn to_line(&self) -> Result<String, Error> {
        let mut out = String::new();
        if !self.tags.is_empty() {
            out.push('@');
            for (i, (k, v)) in self.tags.iter().enumerate() {
                if !valid_tag_key(k) {
                    return Err(Error::BadTagKey(k.clone()));
                }
                if v.contains('\0') {
                    return Err(Error::ForbiddenChar);
                }
                if i > 0 {
                    out.push(';');
                }
                out.push_str(k);
                if !v.is_empty() {
                    out.push('=');
                    escape_tag(v, &mut out);
                }
            }
            out.push(' ');
        }
        if let Some(src) = &self.source {
            if src.is_empty() || src.contains([' ', '\r', '\n', '\0']) {
                return Err(Error::BadSource(src.clone()));
            }
            let _ = write!(out, ":{src} ");
        }
        if self.command.is_empty() || !self.command.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(Error::BadCommand(self.command.clone()));
        }
        out.push_str(&self.command);

        let last = self.params.len().saturating_sub(1);
        for (i, p) in self.params.iter().enumerate() {
            if p.contains(['\r', '\n', '\0']) {
                return Err(Error::ForbiddenChar);
            }
            let needs_colon = p.is_empty() || p.contains(' ') || p.starts_with(':');
            out.push(' ');
            if needs_colon {
                if i != last {
                    return Err(Error::BadMiddleParam(i));
                }
                out.push(':');
            }
            out.push_str(p);
        }
        Ok(out)
    }
}

impl FromStr for Message {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        Self::parse(s)
    }
}

/// Splits off one space-delimited token, skipping any run of spaces after it.
fn split_token(s: &str) -> (&str, &str) {
    match s.split_once(' ') {
        Some((tok, rest)) => (tok, rest.trim_start_matches(' ')),
        None => (s, ""),
    }
}

/// `[+][vendor/]name`, where vendor is a hostname and name is letters, digits and `-`.
fn valid_tag_key(k: &str) -> bool {
    let k = k.strip_prefix('+').unwrap_or(k);
    let (vendor, name) = k.rsplit_once('/').unwrap_or(("", k));
    let name_ok = !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let vendor_ok = vendor
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    name_ok && vendor_ok && !k.starts_with('/')
}

fn unescape_tag(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        // A lone trailing backslash is dropped.
        match chars.next() {
            Some(':') => out.push(';'),
            Some('s') => out.push(' '),
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn escape_tag(v: &str, out: &mut String) {
    for c in v.chars() {
        match c {
            ';' => out.push_str("\\:"),
            ' ' => out.push_str("\\s"),
            '\\' => out.push_str("\\\\"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ergo_chathistory_line() {
        let m = Message::parse(
            "@account=alice;msgid=3rttne5aup4dknrrwejhmgx8i6;time=2026-10-28T01:54:21.180Z;batch=4;+chad/e2e=AwogQUJD;+draft/reply=fake :alice!~u@vzgc9c3e6jsn4.irc PRIVMSG bob :[probe dm tag]\r\n",
        )
        .unwrap();
        assert_eq!(m.tag("account"), Some("alice"));
        assert_eq!(m.tag("+chad/e2e"), Some("AwogQUJD"));
        assert_eq!(m.tag("+draft/reply"), Some("fake"));
        assert_eq!(m.source_nick(), Some("alice"));
        assert_eq!(m.command, "PRIVMSG");
        assert_eq!(m.params, ["bob", "[probe dm tag]"]);
    }

    #[test]
    fn parses_without_tags_or_source() {
        let m = Message::parse("PING").unwrap();
        assert_eq!(
            (m.source, m.command.as_str(), m.params.len()),
            (None, "PING", 0)
        );
        let m =
            Message::parse(":chat.localhost 005 alice CASEMAPPING=ascii :are supported").unwrap();
        assert_eq!(m.source_nick(), Some("chat.localhost"));
        assert_eq!(m.params, ["alice", "CASEMAPPING=ascii", "are supported"]);
    }

    #[test]
    fn tolerates_extra_spaces_and_empty_trailing() {
        let m = Message::parse(":a!b@c   PRIVMSG  #x   :").unwrap();
        assert_eq!(m.params, ["#x", ""]);
        let m = Message::parse("CAP * LS ").unwrap();
        assert_eq!(m.params, ["*", "LS"]);
    }

    #[test]
    fn tag_escaping() {
        let m = Message::parse(r"@a=x\:y\sz\\w\r\n;b;c=;d=trail\ :s CMD").unwrap();
        assert_eq!(m.tag("a"), Some("x;y z\\w\r\n"));
        assert_eq!(m.tag("b"), Some(""));
        assert_eq!(m.tag("c"), Some(""));
        assert_eq!(m.tag("d"), Some("trail"));
        assert_eq!(Message::parse(r"@x=\q CMD").unwrap().tag("x"), Some("q"));
        let line = Message::new("TAGMSG", ["#c"])
            .with_tag("+t", "a; b\\")
            .to_line()
            .unwrap();
        assert_eq!(line, r"@+t=a\:\sb\\ TAGMSG #c");
    }

    #[test]
    fn last_duplicate_tag_wins() {
        assert_eq!(Message::parse("@a=1;a=2 CMD").unwrap().tag("a"), Some("2"));
    }

    #[test]
    fn serializes_trailing_only_when_needed() {
        let line = |p: &[&str]| {
            Message::new("PRIVMSG", p.iter().copied())
                .to_line()
                .unwrap()
        };
        assert_eq!(line(&["#c", "hi"]), "PRIVMSG #c hi");
        assert_eq!(line(&["#c", "hi there"]), "PRIVMSG #c :hi there");
        assert_eq!(line(&["#c", ":)"]), "PRIVMSG #c ::)");
        assert_eq!(line(&["#c", ""]), "PRIVMSG #c :");
    }

    #[test]
    fn rejects_injection_and_bad_input() {
        let inject = Message::new("PRIVMSG", ["#c", "hi\r\nQUIT"]);
        assert_eq!(inject.to_line(), Err(Error::ForbiddenChar));
        assert_eq!(
            Message::new("PRIVMSG", ["a b", "x"]).to_line(),
            Err(Error::BadMiddleParam(0))
        );
        assert_eq!(
            Message::new("A B", [""; 0]).to_line(),
            Err(Error::BadCommand("A B".into()))
        );
        assert_eq!(
            Message::parse("PRIVMSG #c :a\rb"),
            Err(Error::ForbiddenChar)
        );
        assert_eq!(Message::parse(""), Err(Error::NoCommand));
        assert_eq!(Message::parse(":src"), Err(Error::NoCommand));
        assert_eq!(Message::parse("@a=b"), Err(Error::NoCommand));
        let m = Message::parse("@bad=key=x;b@d;+a.b=1;+a/b/c;=v PRIVMSG #c :hi").unwrap();
        assert_eq!(m.tags, [("bad".into(), "key=x".into())]);
        assert_eq!(m.params, ["#c", "hi"]);
        assert!(Message::parse(": CMD").is_err());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    fn tag_key() -> impl Strategy<Value = String> {
        "\\+?([a-z0-9.-]{1,10}/)?[a-zA-Z0-9-]{1,10}"
    }

    fn middle() -> impl Strategy<Value = String> {
        "[^ :\r\n\0][^ \r\n\0]{0,15}"
    }

    fn message() -> impl Strategy<Value = Message> {
        (
            prop::collection::vec((tag_key(), "[^\0]{0,20}"), 0..4),
            prop::option::of("[^ \r\n\0]{1,20}"),
            "[A-Z]{1,10}|[0-9]{3}",
            prop::collection::vec(middle(), 0..5),
            prop::option::of("[^\r\n\0]{0,30}"),
        )
            .prop_map(|(tags, source, command, mut params, trailing)| {
                params.extend(trailing);
                Message {
                    tags,
                    source,
                    command,
                    params,
                }
            })
    }

    proptest! {
        #[test]
        fn roundtrip(m in message()) {
            let line = m.to_line().unwrap();
            prop_assert_eq!(Message::parse(&line).unwrap(), m);
        }

        #[test]
        fn parse_never_panics_and_reserializes(s in "\\PC{0,80}") {
            if let Ok(m) = Message::parse(&s) {
                let again = Message::parse(&m.to_line().unwrap()).unwrap();
                prop_assert_eq!(again, m);
            }
        }
    }
}
