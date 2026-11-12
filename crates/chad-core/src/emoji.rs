//! `:shortcode:` (GitHub's names) in a composer, and the network's custom
//! emoji, shared by every UI.
//!
//! A custom emoji is an image on chadd, named by its file. In a message or a
//! reaction it's plain `:name:` text, which is all a plain IRC client shows;
//! IrChad draws the image wherever the name is in the network's set.

use url::Url;

/// One of a network's custom emoji.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEmoji {
    pub name: String,
    /// On the network's own origin.
    pub url: Url,
}

/// chadd's list as a set, sorted by name. The list is server input (D14):
/// only names and files of the agreed shape count, and the URL is ours to
/// build. A name that's also a standard shortcode is the custom emoji here
/// (`replace_shortcodes` leaves it alone).
pub fn custom(list: Vec<chad_proto::chadd::Emoji>, network: &str) -> Vec<CustomEmoji> {
    let Ok(origin) = Url::parse(network) else {
        return Vec::new();
    };
    let mut set: Vec<CustomEmoji> = list
        .into_iter()
        .take(chad_proto::chadd::MAX_EMOJI)
        .filter(|e| chad_proto::chadd::emoji_name(&e.file) == Some(e.name.as_str()))
        .filter_map(|e| {
            Some(CustomEmoji {
                url: origin.join(&format!("/emoji/{}", e.file)).ok()?,
                name: e.name,
            })
        })
        .collect();
    set.sort_by(|a, b| a.name.cmp(&b.name));
    set.dedup_by(|b, a| a.name == b.name);
    set
}

/// The emoji `code` (`:name:`) stands for, as in a reaction. `set` is sorted.
pub fn lookup<'a>(code: &str, set: &'a [CustomEmoji]) -> Option<&'a CustomEmoji> {
    let name = code.strip_prefix(':')?.strip_suffix(':')?;
    let i = set.binary_search_by(|e| e.name.as_str().cmp(name)).ok()?;
    set.get(i)
}

/// `text` in pieces, each `:name:` from `set` its own piece with its emoji.
pub fn split<'a>(text: &'a str, set: &'a [CustomEmoji]) -> Vec<(&'a str, Option<&'a CustomEmoji>)> {
    let mut out = Vec::new();
    let (mut pos, mut from) = (0, 0);
    while !set.is_empty()
        && let Some(a) = text[from..].find(':').map(|i| from + i)
        && let Some(b) = text[a + 1..].find(':').map(|i| a + 1 + i)
    {
        match lookup(&text[a..=b], set) {
            Some(e) => {
                if pos < a {
                    out.push((&text[pos..a], None));
                }
                out.push((&text[a..=b], Some(e)));
                pos = b + 1;
                from = pos;
            }
            // The second colon may open the next code.
            None => from = b,
        }
    }
    if pos < text.len() {
        out.push((&text[pos..], None));
    }
    out
}

/// Whether `text` is nothing but emoji (and spaces), standard or from `set`,
/// to draw it big.
pub fn only_emoji(text: &str, set: &[CustomEmoji]) -> bool {
    let mut any = false;
    for (piece, e) in split(text, set) {
        if e.is_some() {
            any = true;
            continue;
        }
        for c in piece.chars() {
            match c {
                // Joiner, variation selectors, keycap, skin tones, tags.
                '\u{200d}'
                | '\u{fe0e}'
                | '\u{fe0f}'
                | '\u{20e3}'
                | '\u{1f3fb}'..='\u{1f3ff}'
                | '\u{e0020}'..='\u{e007f}' => {}
                // Flag letters.
                '\u{1f1e6}'..='\u{1f1ff}' => any = true,
                c if c.is_whitespace() => {}
                c if emojis::get(c.encode_utf8(&mut [0; 4])).is_some() => any = true,
                _ => return false,
            }
        }
    }
    any
}

/// Replaces each complete `:shortcode:` in `text` with its Unicode emoji,
/// except the network's `custom` ones, which stay as they are. `cursor` is a
/// char index that is moved to stay after the same text.
pub fn replace_shortcodes(text: &mut String, cursor: &mut usize, custom: &[CustomEmoji]) -> bool {
    let mut out = String::with_capacity(text.len());
    let (mut pos, mut changed, mut shrunk) = (0, false, 0usize);
    while let Some(a) = text[pos..].find(':').map(|i| pos + i) {
        let Some(b) = text[a + 1..].find(':').map(|i| a + 1 + i) else {
            break;
        };
        let name = &text[a + 1..b];
        let valid = (1..=40).contains(&name.len())
            && name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_+-".contains(&c));
        let standard = valid && lookup(&text[a..=b], custom).is_none();
        match emojis::get_by_shortcode(name).filter(|_| standard) {
            Some(e) => {
                out.push_str(&text[pos..a]);
                out.push_str(e.as_str());
                if text[..=b].chars().count() <= *cursor {
                    shrunk += (name.len() + 2).saturating_sub(e.as_str().chars().count());
                }
                pos = b + 1;
                changed = true;
            }
            // The second colon may open the next code.
            None => {
                out.push_str(&text[pos..b]);
                pos = b;
            }
        }
    }
    if changed {
        out.push_str(&text[pos..]);
        *text = out;
        *cursor = cursor.saturating_sub(shrunk);
    }
    changed
}

/// [`replace_shortcodes`] with `cursor` in UTF-16 units, as a browser's
/// text field counts.
pub fn replace_shortcodes_utf16(
    text: &mut String,
    cursor: &mut usize,
    custom: &[CustomEmoji],
) -> bool {
    let mut units = 0;
    let mut chars = text
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= *cursor
        })
        .count();
    let changed = replace_shortcodes(text, &mut chars, custom);
    *cursor = text.chars().take(chars).map(char::len_utf16).sum();
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcodes_utf16() {
        // "😀" is two UTF-16 units, one char.
        let mut t = "😀 :smile: x".to_owned();
        let mut cursor = "😀 :smile:".encode_utf16().count();
        assert!(replace_shortcodes_utf16(&mut t, &mut cursor, &[]));
        assert_eq!(t, "😀 😄 x");
        assert_eq!(cursor, "😀 😄".encode_utf16().count());
        let mut t = "😀 :x".to_owned();
        let mut cursor = 99;
        assert!(!replace_shortcodes_utf16(&mut t, &mut cursor, &[]));
        assert_eq!(cursor, 5);
    }

    #[test]
    fn shortcodes() {
        let mut t = "hi :smile: at 12:30:45 :nope: :+1: x".to_owned();
        let mut cursor = t.chars().count();
        assert!(replace_shortcodes(&mut t, &mut cursor, &[]));
        assert_eq!(t, "hi 😄 at 12:30:45 :nope: 👍 x");
        assert_eq!(cursor, t.chars().count());

        // Cursor before the code stays put.
        let mut t = "ab :heart:".to_owned();
        let mut cursor = 1;
        replace_shortcodes(&mut t, &mut cursor, &[]);
        assert_eq!((t.as_str(), cursor), ("ab ❤️", 1));

        let mut t = "no codes: here".to_owned();
        assert!(!replace_shortcodes(&mut t, &mut 0, &[]));
        let mut t = "é:é:".to_owned();
        assert!(!replace_shortcodes(&mut t, &mut 0, &[]));
    }

    fn set() -> Vec<CustomEmoji> {
        let e = |name: &str, file: &str| chad_proto::chadd::Emoji {
            name: name.into(),
            file: file.into(),
        };
        custom(
            vec![
                e("blob_cat", "blob_cat.gif"),
                e("bigint", "bigint.png"),
                // Not the file's name, a path, a script.
                e("other", "bigint.png"),
                e("up", "../up.png"),
                e("evil", "evil.svg"),
                // Also a standard shortcode: here it's the network's.
                e("parrot", "parrot.png"),
                e("bigint", "bigint.png"),
            ],
            "https://chat.example",
        )
    }

    #[test]
    fn custom_set_is_checked() {
        let set = set();
        let names: Vec<_> = set.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["bigint", "blob_cat", "parrot"]);
        let mut t = ":parrot: :smile: :bigint:".to_owned();
        assert!(replace_shortcodes(&mut t, &mut 0, &set));
        assert_eq!(t, ":parrot: 😄 :bigint:");
        assert_eq!(set[0].url.as_str(), "https://chat.example/emoji/bigint.png");
        assert!(custom(Vec::new(), "not a url").is_empty());
        assert!(lookup(":blob_cat:", &set).is_some());
        assert!(lookup("blob_cat", &set).is_none() && lookup(":", &set).is_none());
    }

    #[test]
    fn splits_custom_emoji() {
        let set = set();
        let pieces = |t: &str| -> Vec<(String, bool)> {
            split(t, &set)
                .into_iter()
                .map(|(s, e)| (s.to_owned(), e.is_some()))
                .collect()
        };
        let p = |s: &str, e: bool| (s.to_owned(), e);
        assert_eq!(
            pieces("a :bigint::blob_cat: é 12:30:bigint:"),
            [
                p("a ", false),
                p(":bigint:", true),
                p(":blob_cat:", true),
                p(" é 12:30", false),
                p(":bigint:", true),
            ]
        );
        assert_eq!(pieces("no :nope: here:"), [p("no :nope: here:", false)]);
        assert_eq!(pieces(":bigint"), [p(":bigint", false)]);
        assert!(pieces("").is_empty());
        assert_eq!(split(":bigint:", &[]), [(":bigint:", None)]);
    }

    #[test]
    fn only_emoji_text() {
        let set = set();
        for t in ["😄", " 👍🏽 ❤️ ", "👨‍👩‍👧", "🇳🇱", ":bigint: 😄"] {
            assert!(only_emoji(t, &set), "{t}");
        }
        for t in ["", "  ", "hi 😄", "1", ":nope:", ":bigint: x", "😄!"] {
            assert!(!only_emoji(t, &set), "{t}");
        }
    }
}
