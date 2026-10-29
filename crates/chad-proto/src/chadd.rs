//! chadd's HTTP API, under the discovery document's `chadd.api`.
//! Errors are a non-2xx status with an [`ApiError`] body.

use serde::{Deserialize, Serialize};

/// `POST <api>/login`
pub const LOGIN: &str = "login";
/// `POST <api>/voice/token`, with `Authorization: Bearer <session token>`.
pub const VOICE_TOKEN: &str = "voice/token";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    /// A trailing `@device` is ignored.
    pub account: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginResponse {
    /// Opaque session token for later calls.
    pub token: String,
    /// The account as the server spells it.
    pub account: String,
    /// Seconds until `token` expires.
    pub expires_in: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceTokenRequest {
    pub channel: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceTokenResponse {
    /// LiveKit join token for the discovery document's `livekit.url`.
    pub token: String,
    pub room: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    /// In words a user understands.
    pub error: String,
}

/// `<api>/<path>`, whether or not `api` ends in a slash.
pub fn endpoint(api: &url::Url, path: &str) -> String {
    format!("{}/{path}", api.as_str().trim_end_matches('/'))
}
