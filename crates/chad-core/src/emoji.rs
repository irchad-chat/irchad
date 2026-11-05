//! `:shortcode:` (GitHub's names) in a composer, shared by every UI.

/// Replaces each complete `:shortcode:` in `text`. `cursor` is a char index
/// that is moved to stay after the same text.
pub fn replace_shortcodes(text: &mut String, cursor: &mut usize) -> bool {
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
        match emojis::get_by_shortcode(name).filter(|_| valid) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcodes() {
        let mut t = "hi :smile: at 12:30:45 :nope: :+1: x".to_owned();
        let mut cursor = t.chars().count();
        assert!(replace_shortcodes(&mut t, &mut cursor));
        assert_eq!(t, "hi 😄 at 12:30:45 :nope: 👍 x");
        assert_eq!(cursor, t.chars().count());

        // Cursor before the code stays put.
        let mut t = "ab :heart:".to_owned();
        let mut cursor = 1;
        replace_shortcodes(&mut t, &mut cursor);
        assert_eq!((t.as_str(), cursor), ("ab ❤️", 1));

        let mut t = "no codes: here".to_owned();
        assert!(!replace_shortcodes(&mut t, &mut 0));
        let mut t = "é:é:".to_owned();
        assert!(!replace_shortcodes(&mut t, &mut 0));
    }
}
