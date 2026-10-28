//! `RPL_ISUPPORT` (005) tokens and casemapping.
//!
//! Feed every `005` to [`Isupport::handle`]; with `draft/extended-isupport` they also
//! arrive in a `draft/isupport` batch before registration and later at runtime.
//! A hostile server (D14) can send endless 005s, so distinct keys are capped.

use std::collections::BTreeMap;

use crate::Message;

/// ergo sends ~35. Anything near this is abuse.
const MAX_KEYS: usize = 256;

#[derive(Debug, Default, Clone)]
pub struct Isupport {
    tokens: BTreeMap<String, String>,
}

impl Isupport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Handles one `005`; anything else is ignored. `-KEY` removes a token.
    pub fn handle(&mut self, msg: &Message) {
        if msg.command != "005" || msg.params.len() < 3 {
            return;
        }
        // params: <nick> <token>… :are supported by this server
        for tok in &msg.params[1..msg.params.len() - 1] {
            if let Some(key) = tok.strip_prefix('-') {
                self.tokens.remove(key);
                continue;
            }
            let (k, v) = tok.split_once('=').unwrap_or((tok, ""));
            if k.is_empty() || (self.tokens.len() >= MAX_KEYS && !self.tokens.contains_key(k)) {
                continue;
            }
            self.tokens.insert(k.to_owned(), unescape(v));
        }
    }

    /// The value of a token, `""` if it has none, `None` if absent.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.tokens.get(key).map(String::as_str)
    }

    pub fn casemapping(&self) -> Casemapping {
        match self.get("CASEMAPPING") {
            Some("ascii") => Casemapping::Ascii,
            Some("strict-rfc1459") => Casemapping::StrictRfc1459,
            Some("rfc8265") => Casemapping::Rfc8265,
            _ => Casemapping::Rfc1459,
        }
    }

    pub fn chantypes(&self) -> &str {
        self.get("CHANTYPES").unwrap_or("#&")
    }

    pub fn is_channel(&self, target: &str) -> bool {
        target.starts_with(|c| self.chantypes().contains(c))
    }

    /// `(mode, prefix)` pairs, highest rank first, e.g. `('o', '@')`.
    pub fn prefix(&self) -> Vec<(char, char)> {
        let spec = self.get("PREFIX").unwrap_or("(ov)@+");
        let Some((modes, symbols)) = spec.strip_prefix('(').and_then(|s| s.split_once(')')) else {
            return Vec::new();
        };
        if modes.chars().count() != symbols.chars().count() {
            return Vec::new();
        }
        modes.chars().zip(symbols.chars()).collect()
    }

    /// Max messages per `CHATHISTORY` request; `None` if chathistory isn't offered.
    pub fn chathistory_limit(&self) -> Option<u32> {
        let v = self.get("CHATHISTORY").or(self.get("draft/CHATHISTORY"))?;
        // 0 means "no limit"; ergo's page size is the sane fallback.
        Some(v.parse().ok().filter(|&n| n > 0).unwrap_or(1000))
    }

    /// Call on disconnect: the next server may differ.
    pub fn clear(&mut self) {
        self.tokens.clear();
    }
}

/// Decodes `\xHH` escapes. Invalid sequences are kept as typed.
fn unescape(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 2..i + 4)
            .filter(|_| b[i] == b'\\' && b[i + 1] == b'x');
        match hex.and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()) {
            Some(byte) => {
                out.push(byte);
                i += 4;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Casemapping {
    Ascii,
    /// The spec's default when `CASEMAPPING` is absent.
    Rfc1459,
    StrictRfc1459,
    /// shortcut: Unicode lowercase only, no PRECIS width/NFKC mapping; ergo's dev and
    /// default config use `ascii`. Upgrade if a supported network runs `precis`.
    Rfc8265,
}

impl Casemapping {
    /// The folded form to use as a map key for nicks and channel names.
    pub fn fold(self, s: &str) -> String {
        match self {
            Self::Rfc8265 => s.to_lowercase(),
            _ => s.chars().map(|c| self.fold_char(c)).collect(),
        }
    }

    pub fn eq(self, a: &str, b: &str) -> bool {
        self.fold(a) == self.fold(b)
    }

    fn fold_char(self, c: char) -> char {
        match (self, c) {
            (_, 'A'..='Z') => c.to_ascii_lowercase(),
            (Self::Rfc1459 | Self::StrictRfc1459, '[') => '{',
            (Self::Rfc1459 | Self::StrictRfc1459, ']') => '}',
            (Self::Rfc1459 | Self::StrictRfc1459, '\\') => '|',
            (Self::Rfc1459, '~') => '^',
            _ => c,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(i: &mut Isupport, line: &str) {
        i.handle(&Message::parse(line).unwrap());
    }

    #[test]
    fn ergo_005() {
        let mut i = Isupport::new();
        assert_eq!(i.casemapping(), Casemapping::Rfc1459);
        assert_eq!(i.prefix(), [('o', '@'), ('v', '+')]);
        assert_eq!(i.chathistory_limit(), None);
        feed(
            &mut i,
            ":s 005 alice CASEMAPPING=ascii CHANTYPES=# CHATHISTORY=1000 EXCEPTS :are supported by this server",
        );
        feed(
            &mut i,
            ":s 005 alice PREFIX=(qaohv)~&@%+ NETWORK=Ergo\\x20Test :are supported by this server",
        );
        assert_eq!(i.casemapping(), Casemapping::Ascii);
        assert!(i.is_channel("#dev") && !i.is_channel("&x") && !i.is_channel(""));
        assert_eq!(i.chathistory_limit(), Some(1000));
        assert_eq!(i.get("EXCEPTS"), Some(""));
        assert_eq!(i.get("NETWORK"), Some("Ergo Test"));
        assert_eq!(i.prefix()[0], ('q', '~'));

        feed(&mut i, ":s 005 alice -EXCEPTS :are supported");
        assert_eq!(i.get("EXCEPTS"), None);
    }

    #[test]
    fn hostile_005_is_bounded_and_tolerated() {
        let mut i = Isupport::new();
        feed(&mut i, ":s 005 a PREFIX=(ov)@ NETWORK=\\x \\xZZ= = :x");
        assert!(i.prefix().is_empty());
        assert_eq!(i.get("NETWORK"), Some("\\x"));
        for n in 0..MAX_KEYS * 2 {
            feed(&mut i, &format!(":s 005 a K{n} :x"));
        }
        assert_eq!(i.tokens.len(), MAX_KEYS);
        feed(&mut i, ":s 005 a NETWORK=still :x");
        assert_eq!(i.get("NETWORK"), Some("still"));
    }

    #[test]
    fn casemappings() {
        assert!(Casemapping::Ascii.eq("Alice[]", "alice[]"));
        assert!(!Casemapping::Ascii.eq("a[", "a{"));
        assert!(Casemapping::Rfc1459.eq("Nick[\\]~", "nick{|}^"));
        assert!(!Casemapping::StrictRfc1459.eq("~", "^"));
        assert_eq!(Casemapping::Ascii.fold("ÉA"), "Éa");
        assert!(Casemapping::Rfc8265.eq("Éa", "éA"));
    }
}
