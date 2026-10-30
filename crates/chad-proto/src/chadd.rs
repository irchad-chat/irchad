//! chadd's HTTP API, under the discovery document's `chadd.api`.
//! Errors are a non-2xx status with an [`ApiError`] body.

use serde::{Deserialize, Serialize};

/// `POST <api>/login`
pub const LOGIN: &str = "login";
/// `POST <api>/voice/token`, with `Authorization: Bearer <session token>`.
pub const VOICE_TOKEN: &str = "voice/token";
/// `POST <api>/upload`: the file is the body, its name in `Content-Disposition`.
/// Auth is a Bearer session token or HTTP Basic with the IRC account, so it
/// doubles as the soju-style `draft/FILEHOST`. `201 Created` with `Location`
/// and an [`UploadResponse`].
pub const UPLOAD: &str = "upload";
/// `POST <api>/preview` with a [`PreviewRequest`] and a Bearer session token:
/// chadd fetches the page, so posters never learn readers' IPs (D25).
pub const PREVIEW: &str = "preview";
/// `POST <api>/media` with a [`PreviewRequest`] and a Bearer session token:
/// the image at `url`, fetched by chadd. The body is the image itself.
pub const MEDIA: &str = "media";

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
pub struct UploadResponse {
    /// Where the file is served, ready to paste into a message.
    pub url: url::Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewRequest {
    pub url: String,
}

/// What a page says about itself (OpenGraph, else `<title>`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkPreview {
    pub title: Option<String>,
    pub description: Option<String>,
    pub site: Option<String>,
    /// Fetch through [`MEDIA`], not directly.
    pub image: Option<url::Url>,
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
