//! chadd's HTTP API, under the discovery document's `chadd.api`.
//! Errors are a non-2xx status with an [`ApiError`] body.

use serde::{Deserialize, Serialize};

/// `POST <api>/login`
pub const LOGIN: &str = "login";
/// `POST <api>/voice/token`, with `Authorization: Bearer <session token>`.
pub const VOICE_TOKEN: &str = "voice/token";
/// `POST <api>/meeting` with a Bearer session token: a new meeting, as a
/// [`crate::meeting::CreateMeetingResponse`] (D86).
pub const MEETING: &str = "meeting";
/// `POST <api>/meeting/token` with a [`crate::meeting::MeetingTokenRequest`],
/// no auth: a guest's [`VoiceTokenResponse`] for a meeting's room (D86).
pub const MEETING_TOKEN: &str = "meeting/token";
/// `POST <api>/stream` with a Bearer session token: a public link for our
/// screen share, as a [`crate::stream::CreateStreamResponse`] (D87).
pub const STREAM: &str = "stream";
/// `POST <api>/stream/token` with a [`crate::stream::StreamTokenRequest`], no
/// auth: a viewer's [`crate::stream::StreamTokenResponse`] (D87).
pub const STREAM_TOKEN: &str = "stream/token";
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
/// `POST <api>/gifs` with a [`GifSearch`] and a Bearer session token: a page
/// of GIFs from the provider chadd is set up with, which never learns who
/// searched (D30). Fetch `preview` through [`MEDIA`], not directly.
pub const GIFS: &str = "gifs";
/// `POST <api>/sounds` with a Bearer session token: the network's soundboard,
/// a list of [`Sound`]s. Empty when the server has none.
pub const SOUNDS: &str = "sounds";
/// `POST <api>/sound` with a [`Sound`] (only `file` matters) and a Bearer
/// session token: the clip's bytes (mp3, ogg, wav or flac).
pub const SOUND: &str = "sound";
/// `POST <api>/emoji`, no auth: the network's custom emoji, a list of
/// [`Emoji`]s. A message shows one as `:name:`, which is all plain IRC clients
/// see. The images are at `<origin>/emoji/<file>`, a sticker's at
/// `<origin>/emoji/stickers/<file>`.
pub const EMOJI: &str = "emoji";
/// Most custom emoji a network has.
pub const MAX_EMOJI: usize = 500;
/// Most stickers a network has, in the same list.
pub const MAX_STICKERS: usize = 200;
/// `POST <api>/games` with a Bearer session token: Discord's detectable games
/// as [`Game`]s, which chadd downloads itself so clients never tell Discord
/// what they play. `404` when the server doesn't offer it.
pub const GAMES: &str = "games";
/// `POST <api>/account/delete` with a [`LoginRequest`] (no session: the password
/// is asked again), or with a Bearer session and an empty body for an account
/// that signs in with Discord (D96). `202 Accepted` once chadd has queued the deletion; it then
/// unregisters the account, redacts its messages and deletes its uploads (D69).
pub const ACCOUNT_DELETE: &str = "account/delete";
/// `POST <api>/redacted` with a [`RedactedRequest`], no auth: msgids chadd has
/// deleted, so clients can drop their local copies (D69).
pub const REDACTED: &str = "redacted";
/// `GET <api>/oauth/discord/start?challenge=<S256 of a PKCE verifier>`, opened
/// in the browser. Ends in a redirect to `irchad://oauth?origin=<origin>&code=<code>`
/// (or `&error=<message>`), whose code goes to [`OAUTH_EXCHANGE`] with the verifier (D96).
/// With `&web=1` it ends at the web client's page with `#oauth=<code>` (or `#oauth_error=`) (D97).
pub const OAUTH_DISCORD_START: &str = "oauth/discord/start";
/// `POST <api>/oauth/exchange` with an [`OAuthExchangeRequest`]: an [`OAuthOutcome`].
pub const OAUTH_EXCHANGE: &str = "oauth/exchange";
/// `POST <api>/oauth/register` with an [`OAuthRegisterRequest`]: makes the
/// account, then [`OAuthOutcome::SignedIn`]. `409` if the name can't be had.
pub const OAUTH_REGISTER: &str = "oauth/register";
/// `POST <api>/oauth/refresh` with a [`RefreshRequest`]: a [`RefreshResponse`]
/// before every IRC connect. `401` means sign in with Discord again.
pub const OAUTH_REFRESH: &str = "oauth/refresh";
/// `POST <api>/oauth/password` with a [`PasswordRequest`] and a Bearer session:
/// sets the account's password, for other IRC clients and FILEHOST (D96).
pub const OAUTH_PASSWORD: &str = "oauth/password";
/// Most msgids in one [`RedactedResponse`].
pub const REDACTED_PAGE: usize = 1000;

/// A msgid as chadd stores and serves it: what ergo generates (base32-ish
/// tokens), with room to spare, and nothing that could break an IRC line.
pub fn valid_msgid(s: &str) -> bool {
    (1..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic() && b != b':')
}

/// A custom emoji's file: `<name>.<png|jpg|gif|webp>`, the name 2 to 32 of
/// `a-z`, `0-9` and `_`. Its name, if it is one.
pub fn emoji_name(file: &str) -> Option<&str> {
    let (name, ext) = file.rsplit_once('.')?;
    let ok = (2..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && matches!(ext, "png" | "jpg" | "gif" | "webp");
    ok.then_some(name)
}

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
pub struct OAuthExchangeRequest {
    /// From the `irchad://oauth` redirect.
    pub code: String,
    /// The PKCE verifier whose S256 went to [`OAUTH_DISCORD_START`].
    pub verifier: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OAuthOutcome {
    /// Keep `refresh` in the secret store; it's this device's login.
    SignedIn { account: String, refresh: String },
    /// No account yet: ask for a name (prefilled with `suggestion`), then
    /// [`OAUTH_REGISTER`] with `ticket`.
    NeedsName { suggestion: String, ticket: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthRegisterRequest {
    pub ticket: String,
    pub account: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshRequest {
    pub refresh: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshResponse {
    pub account: String,
    /// For SASL `IRCV3BEARER`; good for a minute.
    pub irc_token: String,
    /// A session token, as [`LoginResponse::token`].
    pub session: String,
    /// Seconds until `session` expires.
    pub expires_in: u64,
    /// Replaces the one sent: save it.
    pub refresh: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasswordRequest {
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedRequest {
    /// `next` from the last response. Leave out to get only the current
    /// cursor: a new device has nothing cached to redact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedResponse {
    /// At most [`REDACTED_PAGE`]. More may follow: ask again with `next`.
    pub ids: Vec<String>,
    pub next: u64,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GifSearch {
    /// Empty for what's trending.
    pub query: String,
    /// A previous page's `next`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GifPage {
    pub gifs: Vec<Gif>,
    /// Pass as `pos` for the next page; `None` at the end.
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gif {
    pub id: String,
    /// For hover text and screen readers; may be empty.
    pub title: String,
    /// A small rendition for the picker.
    pub preview: url::Url,
    /// What goes in the message. Its path ends in an image extension, so
    /// clients show it inline.
    pub url: url::Url,
    /// Of `preview`, in pixels.
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sound {
    /// What the button says.
    pub name: String,
    /// For [`SOUND`].
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Emoji {
    /// Written `:name:` in a message.
    pub name: String,
    /// Under `<origin>/emoji/` (`<origin>/emoji/stickers/` for a sticker);
    /// see [`emoji_name`].
    pub file: String,
    /// A sticker: drawn big when a message is nothing but its `:name:`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sticker: bool,
}

/// A Discord application: Rich Presence names the game only by `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    /// Lowercase `/`-separated path suffixes of its Windows and Linux
    /// executables, e.g. `game/darksoulsiii.exe`; a running one means it's played.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub executables: Vec<String>,
}

/// A [`Game::executables`] entry as it should be: lowercase, `/`-separated,
/// no empty parts or `..`, at most 100 bytes. `None` if it can't be, and for
/// Discord's `>` entries, which also need the program's arguments to match
/// (`>hl2.exe` is every Source game) and so can't be told apart by name.
pub fn executable(raw: &str) -> Option<String> {
    if raw.starts_with('>') {
        return None;
    }
    let e = raw.replace('\\', "/").to_lowercase();
    let ok = (1..=100).contains(&e.len())
        && !e.chars().any(char::is_control)
        && e.split('/').all(|p| !p.is_empty() && p != "..");
    ok.then_some(e)
}

#[cfg(test)]
mod tests {
    #[test]
    fn executables_are_normalized() {
        use super::executable;
        assert_eq!(
            executable("Game\\DarkSoulsIII.exe").unwrap(),
            "game/darksoulsiii.exe"
        );
        for bad in ["", "/abs.exe", "a//b", "../x", "a\u{7}", ">hl2.exe"] {
            assert_eq!(executable(bad), None, "{bad}");
        }
    }
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
