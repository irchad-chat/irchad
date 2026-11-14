//! Ephemeral meetings (D86): a private channel and its LiveKit room, named
//! after a code chadd signs, so guests can get into the call without an account.

use serde::{Deserialize, Serialize};

/// Every meeting's channel is this followed by its code.
pub const PREFIX: &str = "#meet-";
/// Characters in a code: 22 bytes in lowercase base32hex, unpadded.
pub const CODE_LEN: usize = 36;

/// Whether `code` looks like a meeting code. Only chadd can tell a real one.
pub fn valid_code(code: &str) -> bool {
    code.len() == CODE_LEN
        && code
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'v').contains(&b.to_ascii_lowercase()))
}

/// The meeting code in a channel name, lowercased; `None` for other channels.
pub fn code_of(channel: &str) -> Option<String> {
    let head = channel.get(..PREFIX.len())?;
    let code = &channel[PREFIX.len()..];
    (head.eq_ignore_ascii_case(PREFIX) && valid_code(code)).then(|| code.to_ascii_lowercase())
}

pub fn is_meeting(channel: &str) -> bool {
    code_of(channel).is_some()
}

/// The channel of a meeting code, if it is one.
pub fn channel_of(code: &str) -> Option<String> {
    valid_code(code).then(|| format!("{PREFIX}{}", code.to_ascii_lowercase()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateMeetingResponse {
    pub channel: String,
    /// The link to hand out: `<origin>/meet/<code>`.
    pub url: url::Url,
    /// Unix seconds after which the link stops working.
    pub expires: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingTokenRequest {
    pub channel: String,
    /// What the guest is called in the call.
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "0123456789abcdefghijklmnopqrstuv0123";

    #[test]
    fn codes_and_channels() {
        let ch = channel_of(CODE).unwrap();
        assert_eq!(ch, format!("#meet-{CODE}"));
        assert_eq!(code_of(&ch).as_deref(), Some(CODE));
        assert_eq!(
            code_of(&ch.to_ascii_uppercase()).as_deref(),
            Some(CODE),
            "IRC channel names are case-insensitive"
        );
        assert!(!is_meeting("#general"));
        assert!(!is_meeting("#meet-short"));
        assert!(!is_meeting(&format!("#meet-{}w", &CODE[1..])));
        assert!(!is_meeting(&format!("#meet-{CODE}0")));
        assert!(!is_meeting("#méet-0123456789abcdefghijklmnopqrstuv012"));
        assert_eq!(channel_of("nope"), None);
    }
}
