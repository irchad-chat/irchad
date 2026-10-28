//! mIRC formatting codes and links → styled spans. Message text is server
//! input (D14): unknown control characters are dropped, never rendered.

use egui::Color32;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub mono: bool,
    pub fg: Option<Color32>,
    pub bg: Option<Color32>,
    /// `http(s)://` only, so a click can't open anything but a browser.
    pub link: bool,
}

/// The 16 standard mIRC colours.
/// shortcut: the extended 16–98 palette renders as the default colour; add it if anyone uses it.
const PALETTE: [Color32; 16] = [
    Color32::from_rgb(255, 255, 255),
    Color32::from_rgb(0, 0, 0),
    Color32::from_rgb(0, 0, 127),
    Color32::from_rgb(0, 147, 0),
    Color32::from_rgb(255, 0, 0),
    Color32::from_rgb(127, 0, 0),
    Color32::from_rgb(156, 0, 156),
    Color32::from_rgb(252, 127, 0),
    Color32::from_rgb(255, 255, 0),
    Color32::from_rgb(0, 252, 0),
    Color32::from_rgb(0, 147, 147),
    Color32::from_rgb(0, 255, 255),
    Color32::from_rgb(0, 0, 252),
    Color32::from_rgb(255, 0, 255),
    Color32::from_rgb(127, 127, 127),
    Color32::from_rgb(210, 210, 210),
];

pub fn parse(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut cur = Span::default();
    let mut chars = text.chars().peekable();
    let flush = |cur: &mut Span, out: &mut Vec<Span>| {
        if !cur.text.is_empty() {
            let style = Span {
                text: String::new(),
                ..cur.clone()
            };
            out.push(std::mem::replace(cur, style));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '\x02' | '\x1d' | '\x1f' | '\x1e' | '\x11' | '\x16' | '\x0f' | '\x03' | '\x04' => {
                flush(&mut cur, &mut out);
            }
            _ => {}
        }
        match c {
            '\x02' => cur.bold = !cur.bold,
            '\x1d' => cur.italic = !cur.italic,
            '\x1f' => cur.underline = !cur.underline,
            '\x1e' => cur.strike = !cur.strike,
            '\x11' => cur.mono = !cur.mono,
            '\x16' => std::mem::swap(&mut cur.fg, &mut cur.bg),
            '\x0f' => cur = Span::default(),
            '\x03' => {
                let fg = digits(&mut chars);
                let bg = if fg.is_some() && chars.peek() == Some(&',') {
                    let mut look = chars.clone();
                    look.next();
                    if look.peek().is_some_and(char::is_ascii_digit) {
                        chars.next();
                        digits(&mut chars)
                    } else {
                        None
                    }
                } else {
                    None
                };
                match fg {
                    None => (cur.fg, cur.bg) = (None, None),
                    Some(f) => {
                        cur.fg = PALETTE.get(f).copied();
                        if let Some(b) = bg {
                            cur.bg = PALETTE.get(b).copied();
                        }
                    }
                }
            }
            '\x04' => {
                let fg = hex(&mut chars);
                let bg = if fg.is_some() && chars.peek() == Some(&',') {
                    let mut look = chars.clone();
                    look.next();
                    if hex(&mut look).is_some() {
                        chars.next();
                        hex(&mut chars)
                    } else {
                        None
                    }
                } else {
                    None
                };
                match fg {
                    None => (cur.fg, cur.bg) = (None, None),
                    Some(f) => {
                        cur.fg = Some(f);
                        if bg.is_some() {
                            cur.bg = bg;
                        }
                    }
                }
            }
            '\n' | '\t' => cur.text.push(c),
            c if c.is_control() => {}
            c => cur.text.push(c),
        }
    }
    flush(&mut cur, &mut out);
    out.into_iter().flat_map(split_links).collect()
}

/// Text only, for notifications and reply previews.
pub fn strip(text: &str) -> String {
    parse(text).into_iter().map(|s| s.text).collect()
}

fn digits(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<usize> {
    let mut n = None;
    for _ in 0..2 {
        match chars.peek().and_then(|c| c.to_digit(10)) {
            Some(d) => {
                n = Some(n.unwrap_or(0) * 10 + d as usize);
                chars.next();
            }
            None => break,
        }
    }
    n
}

fn hex(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<Color32> {
    let mut look = chars.clone();
    let s: String = (0..6)
        .map_while(|_| look.next_if(char::is_ascii_hexdigit))
        .collect();
    if s.len() != 6 {
        return None;
    }
    *chars = look;
    let v = u32::from_str_radix(&s, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn split_links(span: Span) -> Vec<Span> {
    let mut out = Vec::new();
    let mut rest = span.text.as_str();
    while let Some(start) = find_url(rest) {
        let end = rest[start..]
            .find(char::is_whitespace)
            .map_or(rest.len(), |e| start + e);
        let mut url = &rest[start..end];
        url = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"', '>']);
        if url.ends_with(')') && !url.contains('(') {
            url = &url[..url.len() - 1];
        }
        let end = start + url.len();
        if start > 0 {
            out.push(Span {
                text: rest[..start].to_owned(),
                ..span.clone()
            });
        }
        out.push(Span {
            text: url.to_owned(),
            link: true,
            ..span.clone()
        });
        rest = &rest[end..];
    }
    if !rest.is_empty() || out.is_empty() {
        out.push(Span {
            text: rest.to_owned(),
            ..span
        });
    }
    out
}

/// Start of the next `http://` or `https://` URL with something after the scheme.
fn find_url(s: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = s[from..].find("http") {
        let i = from + i;
        let tail = &s[i..];
        let after = tail
            .strip_prefix("https://")
            .or_else(|| tail.strip_prefix("http://"));
        let at_word_start = s[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        if let Some(a) = after
            && at_word_start
            && a.chars().next().is_some_and(|c| !c.is_whitespace())
        {
            return Some(i);
        }
        from = i + 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(s: &str) -> Vec<(String, bool)> {
        parse(s).into_iter().map(|s| (s.text, s.link)).collect()
    }

    #[test]
    fn styles_toggle_and_reset() {
        let s = parse("a\x02b\x1dc\x0fd");
        assert_eq!(s.len(), 4);
        assert!(!s[0].bold && s[1].bold && s[2].bold && s[2].italic);
        assert!(!s[3].bold && !s[3].italic);
    }

    #[test]
    fn colours() {
        let s = parse("\x034,12red on blue\x03 plain \x0399x \x04ff8800hex");
        assert_eq!(s[0].fg, Some(PALETTE[4]));
        assert_eq!(s[0].bg, Some(PALETTE[12]));
        assert_eq!(s[1].fg, None);
        assert_eq!(s[1].text, " plain ");
        assert_eq!(s[2].fg, None); // 99 = default
        assert_eq!(s[3].fg, Some(Color32::from_rgb(0xff, 0x88, 0)));
        // A comma not followed by a digit is text.
        assert_eq!(strip("\x034,hi"), ",hi");
        assert_eq!(strip("\x03123"), "3");
    }

    #[test]
    fn links() {
        assert_eq!(
            texts("see https://example.com/a?b=1. ok"),
            vec![
                ("see ".into(), false),
                ("https://example.com/a?b=1".into(), true),
                (". ok".into(), false)
            ]
        );
        assert_eq!(
            texts("(http://x.org/y)"),
            vec![
                ("(".into(), false),
                ("http://x.org/y".into(), true),
                (")".into(), false)
            ]
        );
        assert_eq!(
            texts("https://en.wikipedia.org/wiki/A_(b)")[0].0,
            "https://en.wikipedia.org/wiki/A_(b)"
        );
        assert_eq!(
            texts("xhttps://no https:// http"),
            vec![("xhttps://no https:// http".into(), false)]
        );
        assert!(texts("file:///etc/passwd").iter().all(|(_, l)| !l));
    }

    #[test]
    fn hostile_input_is_dropped_not_rendered() {
        assert_eq!(strip("a\x07b\x00c\x1bd\u{7f}e"), "abcde");
        assert_eq!(strip("\x03\x03\x03,\x04zz\x16\x16"), ",zz");
        assert!(parse("").is_empty());
    }
}
