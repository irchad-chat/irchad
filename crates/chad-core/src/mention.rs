//! `@`-mentions that survive renames (D66).
//!
//! The text keeps the nick as typed, which plain IRC clients highlight on.
//! The `+chad/mentions` client tag says which account each nick stood for:
//! `nick account,nick account`. IrChad draws each such nick as a pill with
//! the account's current name, and highlights you when it names your account.
//!
//! The tag is untrusted like everything else: a pair counts only if its nick
//! appears in the text as a whole word, and only the first [`MAX`] are read.

use std::ops::Range;

use crate::Mention;

pub const TAG: &str = "+chad/mentions";
/// Most mentions read from or put in one message.
pub const MAX: usize = 16;
/// Longest nick or account taken from the tag.
const MAX_NAME: usize = 64;

/// The tag's value for `ms`.
pub fn encode(ms: &[Mention]) -> String {
    ms.iter()
        .take(MAX)
        .map(|m| format!("{} {}", m.nick, m.account))
        .collect::<Vec<_>>()
        .join(",")
}

/// The mentions in a tag's `value` whose nick is in `text`.
pub fn decode(value: &str, text: &str) -> Vec<Mention> {
    let mut out: Vec<Mention> = Vec::new();
    for pair in value.split(',').take(MAX) {
        let Some((nick, account)) = pair.split_once(' ') else {
            continue;
        };
        if !name(nick)
            || !name(account)
            || out.iter().any(|m| m.nick.eq_ignore_ascii_case(nick))
            || find(text, nick).next().is_none()
        {
            continue;
        }
        out.push(Mention {
            nick: nick.to_owned(),
            account: account.to_owned(),
        });
    }
    out
}

/// A plausible nick or account.
fn name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME
        && !s.starts_with(':')
        && !s.contains(|c: char| c == ' ' || c == ',' || c.is_control())
}

/// Byte ranges of `name` in `text` as a whole word, ignoring ASCII case.
pub fn find<'a>(text: &'a str, name: &str) -> impl Iterator<Item = Range<usize>> + 'a {
    let word =
        |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || "_-[]\\`^{}|".contains(c));
    let (lower, name) = (text.to_ascii_lowercase(), name.to_ascii_lowercase());
    let len = name.len();
    let hits: Vec<usize> = if len == 0 {
        Vec::new()
    } else {
        lower
            .match_indices(&name)
            .map(|(i, _)| i)
            .filter(|&i| {
                !word(lower[..i].chars().next_back()) && !word(lower[i + len..].chars().next())
            })
            .collect()
    };
    hits.into_iter().map(move |i| i..i + len)
}

/// `text` cut into plain pieces and mentions, in order.
pub fn split<'a>(text: &'a str, ms: &'a [Mention]) -> Vec<(&'a str, Option<&'a Mention>)> {
    let mut hits: Vec<(Range<usize>, &Mention)> = ms
        .iter()
        .flat_map(|m| find(text, &m.nick).map(move |r| (r, m)))
        .collect();
    hits.sort_by_key(|(r, _)| (r.start, std::cmp::Reverse(r.end)));
    let mut out = Vec::new();
    let mut at = 0;
    for (r, m) in hits {
        // Overlaps (one nick inside another): the first, longest one wins.
        if r.start < at {
            continue;
        }
        if r.start > at {
            out.push((&text[at..r.start], None));
        }
        out.push((&text[r.clone()], Some(m)));
        at = r.end;
    }
    if at < text.len() || out.is_empty() {
        out.push((&text[at..], None));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(nick: &str, account: &str) -> Mention {
        Mention {
            nick: nick.into(),
            account: account.into(),
        }
    }

    #[test]
    fn round_trip() {
        let ms = [m("bob", "bob"), m("Al[1]", "alice")];
        let v = encode(&ms);
        assert_eq!(v, "bob bob,Al[1] alice");
        assert_eq!(decode(&v, "hi bob and al[1]!"), ms);
    }

    #[test]
    fn hostile_tags() {
        // Not in the text, malformed, repeated, or too many.
        assert_eq!(decode("eve eve", "hi bob"), []);
        assert_eq!(decode("bob", "bob"), []);
        assert_eq!(decode(" bob,bob \u{7},:bob x,bob a b", "bob"), []);
        assert_eq!(decode("bob a,BOB b", "bob"), [m("bob", "a")]);
        assert_eq!(decode("bobby bob", "bob"), []);
        let many = vec!["x x"; 1000].join(",");
        assert_eq!(decode(&many, "x").len(), 1);
        let long = format!("{} a", "b".repeat(65));
        assert_eq!(decode(&long, &"b".repeat(65)), []);
    }

    #[test]
    fn splits() {
        let ms = [m("bob", "b"), m("al", "a")];
        let parts: Vec<_> = split("hé bob, bobby al", &ms)
            .into_iter()
            .map(|(t, m)| (t, m.map(|m| m.account.as_str())))
            .collect();
        assert_eq!(
            parts,
            [
                ("hé ", None),
                ("bob", Some("b")),
                (", bobby ", None),
                ("al", Some("a"))
            ]
        );
        assert_eq!(split("", &ms), [("", None)]);
        assert_eq!(split("plain", &[]), [("plain", None)]);
    }
}
