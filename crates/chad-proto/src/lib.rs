//! Wire types shared by the client and chadd, so the two can't drift.

pub mod chadd;
pub mod discovery;
pub mod meeting;

/// Server or peer text made safe to show on one line: control characters
/// dropped, at most `max` characters, trimmed.
pub fn one_line(s: &str, max: usize) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).take(max).collect();
    s.trim().to_owned()
}

/// The most JSON chadd sends for `POST <api>/games`, and clients accept.
pub const MAX_GAMES_BYTES: usize = 8 << 20;

#[cfg(test)]
mod tests {
    #[test]
    fn one_line_strips_controls_caps_and_trims() {
        assert_eq!(super::one_line(" a\u{7}b\nc ", 10), "abc");
        assert_eq!(super::one_line("abcdef", 3), "abc");
        assert_eq!(super::one_line("ab   cd", 3), "ab");
    }
}
