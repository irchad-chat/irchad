//! Public streams (D87): a member's screen share in a room of its own, which
//! anyone with the link watches. Stateless like meetings (D86), whose codes
//! and settings it reuses; the room closes when the streamer leaves.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
};
use chad_proto::chadd::ApiError;
use chad_proto::stream::{
    self, CreateStreamRequest, CreateStreamResponse, StreamTokenRequest, StreamTokenResponse,
};
use livekit_api::access_token::{AccessToken, VideoGrants};

use crate::api::{self, ApiResult, fail};
use crate::config::Meetings;
use crate::meeting::{mint, now, verify};
use crate::routes::{self, AppState};

/// Stream codes are signed under their own key, so a meeting code can't be
/// passed off as one or the other way round.
fn key(m: &Meetings) -> String {
    format!("{}\0stream", m.secret)
}

fn off() -> (StatusCode, Json<ApiError>) {
    fail(StatusCode::NOT_FOUND, "Streams are turned off here.")
}

fn ended() -> (StatusCode, Json<ApiError>) {
    fail(StatusCode::NOT_FOUND, "This stream link has expired.")
}

/// The streamer may only put their screen (and its audio) into the room.
fn publisher(room: String) -> VideoGrants {
    VideoGrants {
        room_join: true,
        room,
        can_publish: Some(true),
        can_subscribe: Some(false),
        can_publish_data: Some(false),
        // `unknown` is the call's audio, when the streamer sends it.
        can_publish_sources: vec![
            "screen_share".into(),
            "screen_share_audio".into(),
            "unknown".into(),
        ],
        ..Default::default()
    }
}

/// Viewers only watch, and don't show up as participants.
fn viewer(room: String) -> VideoGrants {
    VideoGrants {
        room_join: true,
        room,
        can_publish: Some(false),
        can_subscribe: Some(true),
        can_publish_data: Some(false),
        hidden: true,
        ..Default::default()
    }
}

fn livekit(state: &AppState) -> url::Url {
    state.discovery.livekit.url.clone()
}

/// `POST /api/stream`: a member makes a public link for their screen share.
pub async fn create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateStreamRequest>,
) -> ApiResult<CreateStreamResponse> {
    let account = api::session_account(&state, &headers)?;
    let name = req
        .name
        .map(|n| chad_proto::one_line(&n, stream::MAX_NAME))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| account.clone());
    let m = state.config.meetings.as_ref().ok_or_else(off)?;
    if !state.meetings.allow(account.to_ascii_lowercase()) {
        return Err(fail(
            StatusCode::TOO_MANY_REQUESTS,
            "That's a lot of streams. Wait a minute and try again.",
        ));
    }
    let expires = now() + u64::from(m.ttl_hours) * 3600;
    let expires32 = u32::try_from(expires).map_err(|e| api::internal(&e))?;
    let code = mint(&key(m), expires32).map_err(|e| api::internal(&*e))?;
    let room = stream::room_of(&code).ok_or_else(|| api::internal(&"bad code"))?;
    let url = state
        .config
        .origin
        .join(&format!("/watch/{code}"))
        .map_err(|e| api::internal(&e))?;
    let lk = &state.config.livekit;
    let token = AccessToken::with_api_key(&lk.key, &lk.secret)
        .with_identity(&account)
        .with_name(&name)
        .with_ttl(api::VOICE_TTL)
        .with_grants(publisher(room.clone()))
        .to_jwt()
        .map_err(|e| api::internal(&e))?;
    tracing::info!(%account, %name, "stream started");
    Ok(Json(CreateStreamResponse {
        url,
        expires,
        livekit: livekit(&state),
        room,
        token,
    }))
}

/// `POST /api/stream/token`: anyone with the link may watch.
pub async fn token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<StreamTokenRequest>,
) -> ApiResult<StreamTokenResponse> {
    let m = state.config.meetings.as_ref().ok_or_else(off)?;
    // Call members watching a public share have a session; anyone else is throttled.
    let member = api::session_account(&state, &headers).is_ok();
    if !member && !state.guests.allow(api::client_ip(&headers)) {
        return Err(fail(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many tries. Wait a minute and try again.",
        ));
    }
    verify(&key(m), &req.code, now()).ok_or_else(ended)?;
    let room = stream::room_of(&req.code).ok_or_else(ended)?;
    // Random, so viewers don't kick each other out; `:` keeps it apart from accounts.
    let mut id = [0u8; 4];
    getrandom::fill(&mut id).map_err(|e| api::internal(&e))?;
    let lk = &state.config.livekit;
    let token = AccessToken::with_api_key(&lk.key, &lk.secret)
        .with_identity(&format!("viewer:{}", hex::encode(id)))
        .with_ttl(api::VOICE_TTL)
        .with_grants(viewer(room))
        .to_jwt()
        .map_err(|e| api::internal(&e))?;
    Ok(Json(StreamTokenResponse {
        livekit: livekit(&state),
        token,
        network: state.discovery.name.clone(),
    }))
}

/// `GET /watch/{code}`: a preview card that goes on to the web client's
/// viewer, or "expired".
pub async fn page(State(state): State<Arc<AppState>>, Path(code): Path<String>) -> Response {
    let valid = state
        .config
        .meetings
        .as_ref()
        .and_then(|m| verify(&key(m), &code, now()));
    let web = state.config.web().ok().map(|mut u| {
        u.query_pairs_mut()
            .append_pair("watch", &code.to_ascii_lowercase());
        u
    });
    match (valid, web) {
        (Some(_), Some(u)) => Html(routes::stream_page(&state, &code, &u)).into_response(),
        _ => Html(routes::stream_ended_page(&state)).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn codes_dont_cross_over() {
        let m = Meetings {
            secret: SECRET.into(),
            ttl_hours: 1,
        };
        let meeting = mint(&m.secret, 2000).unwrap();
        let stream = mint(&key(&m), 2000).unwrap();
        assert_eq!(verify(&key(&m), &stream, 0), Some(2000));
        assert_eq!(verify(&key(&m), &meeting, 0), None);
        assert_eq!(verify(&m.secret, &stream, 0), None);
    }

    #[test]
    fn viewers_only_watch() {
        let g = viewer("stream-x".into());
        assert_eq!(g.can_publish, Some(false));
        assert_eq!(g.can_publish_data, Some(false));
        assert!(g.hidden && !g.room_admin);
        let g = publisher("stream-x".into());
        assert_eq!(g.can_subscribe, Some(false));
        assert!(
            !g.can_publish_sources
                .iter()
                .any(|s| s == "microphone" || s == "camera")
        );
    }
}
