//! Nick completion in the composer: `@` opens a list of matching people, and
//! Tab completes the word before the cursor, cycling on repeated presses.
//!
//! Both match nicks and display names, but either way the nick goes in as
//! plain text (`nick: ` at the start of the message, `nick ` elsewhere), which
//! is what IRC clients highlight on. The composer remembers what was picked so
//! the message can carry `+chad/mentions` (D66), which IrChad draws with the
//! person's current name.

use std::collections::{HashMap, HashSet};

use crate::view::{Profile, key};
use crate::{BufferInfo, BufferKind, ChatMessage, Member};

/// Most suggestions to show in the `@` list.
const MAX_SUGGESTIONS: usize = 50;
/// How far back the log is searched for recent speakers.
const RECENT_MSGS: usize = 200;

/// Someone who can be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cand {
    pub nick: String,
    pub display_name: Option<String>,
}

impl Cand {
    /// Their name in the list: the display name if they set one.
    pub fn shown(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.nick)
    }

    /// The nick or display name, lowercased, passes `f`.
    fn any(&self, f: impl Fn(&str) -> bool) -> bool {
        f(&self.nick.to_lowercase())
            || (self.display_name.as_deref()).is_some_and(|d| f(&d.to_lowercase()))
    }
}

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
/// before the member list is in, recent speakers first, with their display
/// names from `profiles`.
pub fn completable(
    members: Option<&[Member]>,
    msgs: &[ChatMessage],
    info: &BufferInfo,
    own: &str,
    profiles: &HashMap<String, Profile>,
) -> Vec<Cand> {
    let recent = msgs.iter().rev().map(|m| m.nick.as_str());
    let nicks = match members {
        Some(ms) => candidates(ms.iter().map(|m| m.nick.as_str()), recent, own),
        None if info.kind == BufferKind::Dm => candidates([info.target.as_str()], recent, own),
        None => Vec::new(),
    };
    nicks
        .into_iter()
        .map(|nick| Cand {
            display_name: profiles
                .get(&key(&nick))
                .and_then(|p| p.display_name.clone()),
            nick,
        })
        .collect()
}

/// The `@` list for `word` (which includes the `@`): people whose nick or
/// display name starts with the rest, then ones containing it. `None` when
/// `word` isn't an `@` word.
pub fn suggest<'a>(cands: &'a [Cand], word: &str) -> Option<Vec<&'a Cand>> {
    let q = word.strip_prefix('@')?.to_lowercase();
    let (mut start, mut inside) = (Vec::new(), Vec::new());
    for c in cands {
        if c.any(|k| k.starts_with(&q)) {
            start.push(c);
        } else if c.any(|k| k.contains(&q)) {
            inside.push(c);
        }
    }
    start.extend(inside);
    start.truncate(MAX_SUGGESTIONS);
    Some(start)
}

/// What a completed name is inserted as.
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

impl Tab {
    /// The nick it put in last.
    pub fn nick(&self) -> &str {
        &self.matches[self.i]
    }
}

/// Tab (or Shift+Tab, `back`) at char index `cursor`. Completes the word
/// before the cursor to the first match by nick or display name, or swaps
/// the last completion for the next one. Returns the new cursor if `text`
/// changed.
pub fn tab(
    text: &mut String,
    cursor: usize,
    state: &mut Option<Tab>,
    cands: &[Cand],
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
    let q = word.to_lowercase();
    let matches: Vec<String> = cands
        .iter()
        .filter(|c| c.any(|k| k.starts_with(&q)))
        .map(|c| c.nick.clone())
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

    fn nicks() -> Vec<String> {
        candidates(
            ["zed", "Alice", "albert", "me", "bob"],
            ["bob", "gone", "me", "bob", "albert"],
            "Me",
        )
    }

    fn cands() -> Vec<Cand> {
        nicks()
            .into_iter()
            .map(|nick| Cand {
                nick,
                display_name: None,
            })
            .collect()
    }

    fn shown(l: Option<Vec<&Cand>>) -> Option<Vec<&str>> {
        l.map(|l| l.into_iter().map(Cand::shown).collect())
    }

    #[test]
    fn order() {
        assert_eq!(nicks(), ["bob", "albert", "Alice", "zed"]);
    }

    #[test]
    fn at_list() {
        let c = cands();
        assert_eq!(shown(suggest(&c, "@AL")), Some(vec!["albert", "Alice"]));
        assert_eq!(
            shown(suggest(&c, "@e")),
            Some(vec!["albert", "Alice", "zed"])
        );
        assert_eq!(suggest(&c, "@").map(|v| v.len()), Some(4));
        assert_eq!(suggest(&c, "al"), None);
    }

    #[test]
    fn display_names() {
        let mut c = cands();
        c[0].display_name = Some("Robért the Great".to_owned());
        // Found by nick or display name and listed as the latter, but the
        // nick goes in.
        assert_eq!(shown(suggest(&c, "@bo")), Some(vec!["Robért the Great"]));
        assert_eq!(shown(suggest(&c, "@ROBÉ")), Some(vec!["Robért the Great"]));
        assert_eq!(shown(suggest(&c, "@great")), Some(vec!["Robért the Great"]));
        let mut t = "hi rob".to_owned();
        let mut s = None;
        assert_eq!(tab(&mut t, 6, &mut s, &c, false), Some(7));
        assert_eq!(t, "hi bob ");
        assert_eq!(s.as_ref().map(Tab::nick), Some("bob"));
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
