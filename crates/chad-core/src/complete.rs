//! Nick completion in the composer: `@` opens a list of matching nicks, and
//! Tab completes the word before the cursor, cycling on repeated presses.
//!
//! Either way the nick goes in as plain text (`nick: ` at the start of the
//! message, `nick ` elsewhere), which is what IRC clients highlight on.

use std::collections::HashSet;

use crate::{BufferInfo, BufferKind, ChatMessage, Member};

/// Most suggestions to show in the `@` list.
const MAX_SUGGESTIONS: usize = 50;
/// How far back the log is searched for recent speakers.
const RECENT_MSGS: usize = 200;

/// Every nick that can be completed, recent speakers first and then the
/// rest alphabetically, without our own. `recent` is newest first.
pub fn candidates<'a>(
    nicks: impl IntoIterator<Item = &'a str>,
    recent: impl IntoIterator<Item = &'a str>,
    own: &str,
) -> Vec<String> {
    let mut all: Vec<&str> = nicks.into_iter().collect();
    let present: HashSet<String> = all.iter().map(|n| n.to_ascii_lowercase()).collect();
    let mut seen = HashSet::from([own.to_ascii_lowercase()]);
    let mut out = Vec::new();
    for n in recent.into_iter().take(RECENT_MSGS) {
        let k = n.to_ascii_lowercase();
        // Only people still here.
        if present.contains(&k) && seen.insert(k) {
            out.push(n.to_owned());
        }
    }
    all.sort_by_cached_key(|n| n.to_ascii_lowercase());
    for n in all {
        if seen.insert(n.to_ascii_lowercase()) {
            out.push(n.to_owned());
        }
    }
    out
}

/// What can be completed in a buffer: its members, or the other side of a DM
/// before the member list is in, recent speakers first.
pub fn completable(
    members: &Option<Vec<Member>>,
    msgs: &[ChatMessage],
    info: &BufferInfo,
    own: &str,
) -> Vec<String> {
    let recent = msgs.iter().rev().map(|m| m.nick.as_str());
    match members {
        Some(ms) => candidates(ms.iter().map(|m| m.nick.as_str()), recent, own),
        None if info.kind == BufferKind::Dm => candidates([info.target.as_str()], recent, own),
        None => Vec::new(),
    }
}

/// The `@` list for `word` (which includes the `@`): nicks starting with the
/// rest, then ones containing it. `None` when `word` isn't an `@` word.
pub fn suggest<'a>(cands: &'a [String], word: &str) -> Option<Vec<&'a str>> {
    let q = word.strip_prefix('@')?.to_ascii_lowercase();
    let (mut start, mut inside) = (Vec::new(), Vec::new());
    for n in cands {
        let k = n.to_ascii_lowercase();
        if k.starts_with(&q) {
            start.push(n.as_str());
        } else if k.contains(&q) {
            inside.push(n.as_str());
        }
    }
    start.extend(inside);
    start.truncate(MAX_SUGGESTIONS);
    Some(start)
}

/// What a completed nick is inserted as.
pub fn insert(nick: &str, at_start: bool) -> String {
    if at_start {
        format!("{nick}: ")
    } else {
        format!("{nick} ")
    }
}

/// An ongoing Tab completion, so the next press moves to the next nick.
pub struct Tab {
    /// Char range of the inserted text.
    start: usize,
    end: usize,
    matches: Vec<String>,
    i: usize,
}

/// Tab (or Shift+Tab, `back`) at char index `cursor`. Completes the word
/// before the cursor to the first matching nick, or swaps the last
/// completion for the next one. Returns the new cursor if `text` changed.
pub fn tab(
    text: &mut String,
    cursor: usize,
    state: &mut Option<Tab>,
    cands: &[String],
    back: bool,
) -> Option<usize> {
    let byte = |text: &str, ci: usize| text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b);
    // Still sitting right after our last completion: cycle.
    if let Some(t) = state
        && t.end == cursor
        && !t.matches.is_empty()
    {
        let (a, b) = (byte(text, t.start), byte(text, t.end));
        if text[a..b] == insert(&t.matches[t.i], t.start == 0) {
            let n = t.matches.len();
            t.i = if back {
                (t.i + n - 1) % n
            } else {
                (t.i + 1) % n
            };
            let new = insert(&t.matches[t.i], t.start == 0);
            text.replace_range(a..b, &new);
            t.end = t.start + new.chars().count();
            return Some(t.end);
        }
    }
    *state = None;
    let b = byte(text, cursor);
    let a = text[..b]
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let word = text[a..b].strip_prefix('@').unwrap_or(&text[a..b]);
    if word.is_empty() {
        return None;
    }
    let q = word.to_ascii_lowercase();
    let matches: Vec<String> = cands
        .iter()
        .filter(|n| n.to_ascii_lowercase().starts_with(&q))
        .cloned()
        .collect();
    let first = if back {
        matches.len().checked_sub(1)?
    } else {
        0
    };
    let new = insert(matches.get(first)?, a == 0);
    text.replace_range(a..b, &new);
    let start = text[..a].chars().count();
    let end = start + new.chars().count();
    *state = Some(Tab {
        start,
        end,
        matches,
        i: first,
    });
    Some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cands() -> Vec<String> {
        candidates(
            ["zed", "Alice", "albert", "me", "bob"],
            ["bob", "gone", "me", "bob", "albert"],
            "Me",
        )
    }

    #[test]
    fn order() {
        assert_eq!(cands(), ["bob", "albert", "Alice", "zed"]);
    }

    #[test]
    fn at_list() {
        let c = cands();
        assert_eq!(suggest(&c, "@AL"), Some(vec!["albert", "Alice"]));
        assert_eq!(suggest(&c, "@e"), Some(vec!["albert", "Alice", "zed"]));
        assert_eq!(suggest(&c, "@").map(|v| v.len()), Some(4));
        assert_eq!(suggest(&c, "al"), None);
    }

    #[test]
    fn tab_cycles() {
        let c = cands();
        let mut t = "al".to_owned();
        let mut s = None;
        assert_eq!(tab(&mut t, 2, &mut s, &c, false), Some(8));
        assert_eq!(t, "albert: ");
        assert_eq!(tab(&mut t, 8, &mut s, &c, false), Some(7));
        assert_eq!(t, "Alice: ");
        assert_eq!(tab(&mut t, 7, &mut s, &c, false), Some(8));
        assert_eq!(t, "albert: ");
        assert_eq!(tab(&mut t, 8, &mut s, &c, true), Some(7));
        assert_eq!(t, "Alice: ");

        // Mid-message, before other text, and through an `@`.
        let mut t = "hé @b x".to_owned();
        let mut s = None;
        assert_eq!(tab(&mut t, 5, &mut s, &c, false), Some(7));
        assert_eq!(t, "hé bob  x");

        // Nothing to complete.
        let mut s = None;
        assert_eq!(tab(&mut "hi ".to_owned(), 3, &mut s, &c, false), None);
        assert_eq!(tab(&mut "qq".to_owned(), 2, &mut s, &c, false), None);
    }

    #[test]
    fn tab_restarts_after_edit() {
        let c = cands();
        let mut t = "z".to_owned();
        let mut s = None;
        tab(&mut t, 1, &mut s, &c, false);
        assert_eq!(t, "zed: ");
        // Typed on: a new word, a new completion.
        t.push_str("hi b");
        assert_eq!(tab(&mut t, 9, &mut s, &c, false), Some(12));
        assert_eq!(t, "zed: hi bob ");
    }
}
