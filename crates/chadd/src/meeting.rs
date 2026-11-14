//! Ephemeral meetings (D86): signed codes, the guest token, the link's page.
//!
//! A code is `id[10] ‖ expiry[4, unix seconds BE] ‖ HMAC-SHA256(id ‖ expiry)[..8]`
//! in lowercase base32hex. chadd keeps no state: the room and channel vanish
//! when empty, and the link works until it expires.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Html,
};
use chad_proto::chadd::VoiceTokenResponse;
use chad_proto::meeting::{self, CreateMeetingResponse, MeetingTokenRequest};
use data_encoding::BASE32_DNSSEC;
use hmac::{Hmac, Mac};
use livekit_api::access_token::AccessToken;
use sha2::Sha256;

use crate::api::{self, ApiResult, fail};
use crate::routes::{self, AppState};

const ID: usize = 10;
const MAC: usize = 8;
const LEN: usize = ID + 4 + MAC;
/// Longest guest name, in characters.
const MAX_NAME: usize = 32;

fn mac(secret: &str, body: &[u8]) -> Hmac<Sha256> {
    let mut m = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key");
    m.update(body);
    m
}

/// A new code that works until `expires` (unix seconds).
pub fn mint(secret: &str, expires: u32) -> anyhow::Result<String> {
    let mut b = [0u8; LEN];
    getrandom::fill(&mut b[..ID]).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
    b[ID..ID + 4].copy_from_slice(&expires.to_be_bytes());
    let tag = mac(secret, &b[..ID + 4]).finalize().into_bytes();
    b[ID + 4..].copy_from_slice(&tag[..MAC]);
    Ok(BASE32_DNSSEC.encode(&b))
}

/// When `code` expires, if chadd signed it and it hasn't yet.
pub fn verify(secret: &str, code: &str, now: u64) -> Option<u32> {
    if !meeting::valid_code(code) {
        return None;
    }
    let b = BASE32_DNSSEC
        .decode(code.to_ascii_lowercase().as_bytes())
        .ok()?;
    let b: [u8; LEN] = b.try_into().ok()?;
    mac(secret, &b[..ID + 4])
        .verify_truncated_left(&b[ID + 4..])
        .ok()?;
    let expires = u32::from_be_bytes(b[ID..ID + 4].try_into().ok()?);
    (u64::from(expires) > now).then_some(expires)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn off() -> (StatusCode, Json<chad_proto::chadd::ApiError>) {
    fail(StatusCode::NOT_FOUND, "Meetings are turned off here.")
}

fn expired() -> (StatusCode, Json<chad_proto::chadd::ApiError>) {
    fail(StatusCode::NOT_FOUND, "This meeting link has expired.")
}

/// `POST /api/meeting`: a member starts a meeting.
pub async fn create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<CreateMeetingResponse> {
    let account = api::session_account(&state, &headers)?;
    let m = state.config.meetings.as_ref().ok_or_else(off)?;
    if !state.meetings.allow(account.to_ascii_lowercase()) {
        return Err(fail(
            StatusCode::TOO_MANY_REQUESTS,
            "That's a lot of meetings. Wait a minute and try again.",
        ));
    }
    let expires = now() + u64::from(m.ttl_hours) * 3600;
    let expires32 = u32::try_from(expires).map_err(|e| api::internal(&e))?;
    let code = mint(&m.secret, expires32).map_err(|e| api::internal(&*e))?;
    let channel = meeting::channel_of(&code).ok_or_else(|| api::internal(&"bad code"))?;
    let url = state
        .config
        .origin
        .join(&format!("/meet/{code}"))
        .map_err(|e| api::internal(&e))?;
    tracing::info!(%account, "meeting started");
    Ok(Json(CreateMeetingResponse {
        channel,
        url,
        expires,
    }))
}

/// `POST /api/meeting/token`: anyone with the link gets into the call.
pub async fn token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<MeetingTokenRequest>,
) -> ApiResult<VoiceTokenResponse> {
    let m = state.config.meetings.as_ref().ok_or_else(off)?;
    if !state.guests.allow(api::client_ip(&headers)) {
        return Err(fail(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many tries. Wait a minute and try again.",
        ));
    }
    let code = meeting::code_of(&req.channel).ok_or_else(expired)?;
    verify(&m.secret, &code, now()).ok_or_else(expired)?;
    let name = chad_proto::one_line(&req.name, MAX_NAME);
    if name.is_empty() {
        return Err(fail(StatusCode::BAD_REQUEST, "Pick a name."));
    }
    // Random, so two guests with one name don't kick each other out, and the
    // `:` keeps it apart from every account name.
    let mut id = [0u8; 4];
    getrandom::fill(&mut id).map_err(|e| api::internal(&e))?;
    let identity = format!("guest:{}", hex::encode(id));
    let room = api::voice_room(&req.channel).ok_or_else(expired)?;
    let lk = &state.config.livekit;
    let token = AccessToken::with_api_key(&lk.key, &lk.secret)
        .with_identity(&identity)
        .with_name(&name)
        .with_ttl(api::VOICE_TTL)
        .with_grants(api::grants(room.clone()))
        .to_jwt()
        .map_err(|e| api::internal(&e))?;
    Ok(Json(VoiceTokenResponse { token, room }))
}

/// `GET /meet/{code}`: the link's page, to the web client or the app.
pub async fn page(State(state): State<Arc<AppState>>, Path(code): Path<String>) -> Html<String> {
    let valid = state
        .config
        .meetings
        .as_ref()
        .and_then(|m| verify(&m.secret, &code, now()));
    Html(routes::meeting_page(
        &state,
        valid.map(|_| code.to_ascii_lowercase()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn round_trip() {
        let code = mint(SECRET, 2000).unwrap();
        assert_eq!(code.len(), meeting::CODE_LEN);
        assert_eq!(verify(SECRET, &code, 1999), Some(2000));
        assert_eq!(verify(SECRET, &code.to_ascii_uppercase(), 1999), Some(2000));
        assert!(meeting::is_meeting(&meeting::channel_of(&code).unwrap()));
    }

    #[test]
    fn refuses_expired_forged_and_junk() {
        let code = mint(SECRET, 2000).unwrap();
        assert_eq!(verify(SECRET, &code, 2000), None, "expired");
        assert_eq!(verify("another secret entirely, 32 chars", &code, 0), None);
        for i in 0..code.len() {
            let mut b = code.clone().into_bytes();
            b[i] = if b[i] == b'0' { b'1' } else { b'0' };
            let tampered = String::from_utf8(b).unwrap();
            assert_eq!(verify(SECRET, &tampered, 0), None, "byte {i}");
        }
        for junk in [
            "",
            "x",
            &code[1..],
            &format!("{code}0"),
            &"w".repeat(36),
            &"é".repeat(18),
        ] {
            assert_eq!(verify(SECRET, junk, 0), None, "{junk:?}");
        }
    }
}
