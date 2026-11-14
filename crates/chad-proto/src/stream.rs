//! Public streams (D87): a member's screen share, copied into a room of its
//! own that anyone with the link can watch. The code is a meeting-style code
//! (D86) that chadd signs under a different key.

use serde::{Deserialize, Serialize};

use crate::meeting::valid_code;

/// Every stream's LiveKit room is this followed by its code. No `#`, so no
/// channel's room can collide with it.
pub const ROOM_PREFIX: &str = "stream-";

/// The code in a stream's room name, lowercased; `None` for other rooms.
pub fn code_of_room(room: &str) -> Option<String> {
    let code = room.strip_prefix(ROOM_PREFIX)?;
    valid_code(code).then(|| code.to_ascii_lowercase())
}

/// The room of a stream code, if it is one.
pub fn room_of(code: &str) -> Option<String> {
    valid_code(code).then(|| format!("{ROOM_PREFIX}{}", code.to_ascii_lowercase()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateStreamResponse {
    /// The link to hand out: `<origin>/watch/<code>`.
    pub url: url::Url,
    /// Unix seconds after which the link stops working.
    pub expires: u64,
    /// Where to publish, and the token that may only publish the screen
    /// (and the call's audio) into `room`.
    pub livekit: url::Url,
    pub room: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamTokenRequest {
    pub code: String,
}

/// A viewer's way in: subscribe only, and hidden from the streamer. Call
/// members watch a public share this way too, with their session, which
/// spares them the per-IP throttle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamTokenResponse {
    pub livekit: url::Url,
    pub token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms() {
        let code = "0123456789ABCDEFGHIJKLMNOPQRSTUV0123";
        assert_eq!(
            room_of(code).as_deref(),
            Some("stream-0123456789abcdefghijklmnopqrstuv0123")
        );
        assert_eq!(room_of("short"), None);
        let room = room_of(code).unwrap();
        assert_eq!(code_of_room(&room), Some(code.to_ascii_lowercase()));
        assert_eq!(code_of_room("#general"), None);
        assert_eq!(code_of_room("stream-short"), None);
    }
}
