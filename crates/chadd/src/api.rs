//! `/api/*`: session login against ergo, and LiveKit join tokens.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
};
use chad_proto::chadd::{
    ApiError, LoginRequest, LoginResponse, VoiceTokenRequest, VoiceTokenResponse,
};
use livekit_api::access_token::{AccessToken, TokenVerifier, VideoGrants};
use serde::Deserialize;

use crate::routes::AppState;

/// Session tokens are LiveKit-style JWTs under this issuer and a secret only
/// this process knows, so LiveKit never accepts one as a join token.
const SESSION_ISSUER: &str = "chadd-session";
const SESSION_TTL: Duration = Duration::from_secs(3600);
/// Only has to last until the join; LiveKit refreshes it from then on.
const VOICE_TTL: Duration = Duration::from_secs(600);
/// Login attempts per IP and per account per window.
const MAX_ATTEMPTS: u32 = 10;
const WINDOW: Duration = Duration::from_secs(60);

pub type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn fail(status: StatusCode, error: &str) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            error: error.into(),
        }),
    )
}

/// Fixed-window counters; ergo's own login throttle only covers IRC.
#[derive(Default)]
pub struct Throttle(Mutex<HashMap<String, (Instant, u32)>>);

impl Throttle {
    /// Counts an attempt; false once `key` is over the limit.
    fn allow(&self, key: String) -> bool {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if map.len() > 10_000 {
            map.retain(|_, (start, _)| now - *start < WINDOW);
        }
        let (start, n) = map.entry(key).or_insert((now, 0));
        if now - *start >= WINDOW {
            (*start, *n) = (now, 0);
        }
        *n += 1;
        *n <= MAX_ATTEMPTS
    }
}

#[derive(Deserialize)]
struct CheckAuth {
    success: bool,
    #[serde(rename = "accountName")]
    account_name: Option<String>,
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> ApiResult<LoginResponse> {
    // ergo's check_auth doesn't strip SASL device IDs (`docs/notes/ergo-api.md`).
    let account = req.account.split('@').next().unwrap_or_default().trim();
    if account.is_empty() || account.len() > 64 || req.password.len() > 1024 {
        return Err(fail(
            StatusCode::UNAUTHORIZED,
            "Wrong username or password.",
        ));
    }
    // Caddy sets X-Forwarded-For; without it every request looks the same.
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .unwrap_or("direct")
        .trim()
        .to_owned();
    let throttled = !state.throttle.allow(format!("ip:{ip}"))
        | !state
            .throttle
            .allow(format!("account:{}", account.to_ascii_lowercase()));
    if throttled {
        return Err(fail(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many tries. Wait a minute and try again.",
        ));
    }

    let ergo = &state.config.ergo;
    let url = ergo.api.join("v1/check_auth").map_err(|_| unavailable())?;
    let res = state
        .http
        .post(url)
        .bearer_auth(&ergo.token)
        .json(&serde_json::json!({ "accountName": account, "passphrase": req.password }))
        .send()
        .await
        .and_then(|r| r.error_for_status());
    let check: CheckAuth = match res {
        Ok(r) => r.json().await.map_err(|e| {
            tracing::error!("ergo check_auth: {e}");
            unavailable()
        })?,
        Err(e) => {
            tracing::error!("ergo check_auth: {e}");
            return Err(unavailable());
        }
    };
    let account = match check {
        CheckAuth {
            success: true,
            account_name: Some(a),
        } => a,
        _ => {
            return Err(fail(
                StatusCode::UNAUTHORIZED,
                "Wrong username or password.",
            ));
        }
    };
    let token = AccessToken::with_api_key(SESSION_ISSUER, &state.session_secret)
        .with_identity(&account)
        .with_ttl(SESSION_TTL)
        .to_jwt()
        .map_err(|e| internal(&e))?;
    Ok(Json(LoginResponse {
        token,
        account,
        expires_in: SESSION_TTL.as_secs(),
    }))
}

pub async fn voice_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<VoiceTokenRequest>,
) -> ApiResult<VoiceTokenResponse> {
    let account = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(|t| {
            TokenVerifier::with_api_key(SESSION_ISSUER, &state.session_secret)
                .verify(t)
                .ok()
        })
        .map(|c| c.sub)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "Sign in again."))?;
    let room = voice_room(&req.channel)
        .ok_or_else(|| fail(StatusCode::BAD_REQUEST, "That isn't a channel."))?;
    // shortcut: ergo's API can't read channel metadata, so `chad/voice=off` is
    // enforced by the client only. Membership isn't checked either; check both
    // here if ergo's API grows a way (or chadd gets an IRC connection).
    let lk = &state.config.livekit;
    let token = AccessToken::with_api_key(&lk.key, &lk.secret)
        .with_identity(&account)
        .with_name(&account)
        .with_ttl(VOICE_TTL)
        .with_grants(VideoGrants {
            room_join: true,
            room: room.clone(),
            can_publish: Some(true),
            can_subscribe: Some(true),
            can_publish_data: Some(false),
            can_publish_sources: vec![
                "microphone".into(),
                "screen_share".into(),
                "screen_share_audio".into(),
            ],
            // The `watching` attribute behind "N watching" (D22).
            can_update_own_metadata: Some(true),
            ..Default::default()
        })
        .to_jwt()
        .map_err(|e| internal(&e))?;
    Ok(Json(VoiceTokenResponse { token, room }))
}

/// A channel's room: the name casefolded the way ergo's default `ascii`
/// casemapping does.
// shortcut: other casemappings fold non-ASCII letters too, so two spellings of
// such a channel get separate rooms; read CASEMAPPING if that ever matters.
fn voice_room(channel: &str) -> Option<String> {
    let ok = channel.starts_with('#')
        && channel.len() > 1
        && channel.len() <= 64
        && !channel.contains(|c: char| c == ' ' || c == ',' || c.is_control());
    ok.then(|| channel.to_ascii_lowercase())
}

fn unavailable() -> (StatusCode, Json<ApiError>) {
    fail(
        StatusCode::BAD_GATEWAY,
        "Couldn't check your password right now. Try again later.",
    )
}

fn internal(e: &dyn std::fmt::Display) -> (StatusCode, Json<ApiError>) {
    tracing::error!("minting a token: {e}");
    fail(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms() {
        assert_eq!(voice_room("#Dev").as_deref(), Some("#dev"));
        for bad in [
            "dev",
            "#",
            "#a b",
            "#a,b",
            "#a\x07",
            &format!("#{}", "x".repeat(64)),
        ] {
            assert_eq!(voice_room(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn throttle() {
        let t = Throttle::default();
        for _ in 0..MAX_ATTEMPTS {
            assert!(t.allow("a".into()));
        }
        assert!(!t.allow("a".into()));
        assert!(t.allow("b".into()));
    }
}

#[cfg(test)]
mod http_tests {
    use axum::{Router, body::Body, http::Request, routing::post};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::config::Config;
    use crate::routes::router;

    /// ergo's check_auth: only alice/alicepass passes.
    async fn fake_ergo() -> String {
        let app = Router::new().route(
            "/v1/check_auth",
            post(|Json(v): Json<serde_json::Value>| async move {
                let ok = v["accountName"] == "Alice" && v["passphrase"] == "alicepass";
                Json(serde_json::json!({ "success": ok, "accountName": ok.then_some("alice") }))
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await });
        format!("http://{addr}")
    }

    async fn call(
        app: &Router,
        uri: &str,
        bearer: Option<&str>,
        body: serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let mut req = Request::post(uri).header("content-type", "application/json");
        if let Some(t) = bearer {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status().as_u16();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn login_then_voice_token() {
        let config = Config::parse(&format!(
            r#"
            origin = "https://chat.example.com"
            name = "x"
            [ergo]
            api = "{}"
            token = "t"
            [livekit]
            key = "devkey"
            secret = "0123456789abcdef0123456789abcdef"
            "#,
            fake_ergo().await
        ))
        .unwrap();
        let app = router(Arc::new(AppState::new(config).unwrap()));

        let (status, _) = call(
            &app,
            "/api/login",
            None,
            serde_json::json!({"account": "Alice", "password": "nope"}),
        )
        .await;
        assert_eq!(status, 401);
        let (status, v) = call(
            &app,
            "/api/login",
            None,
            serde_json::json!({"account": "Alice@laptop", "password": "alicepass"}),
        )
        .await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["account"], "alice");
        let session = v["token"].as_str().unwrap().to_owned();

        let (status, _) = call(
            &app,
            "/api/voice/token",
            Some("forged"),
            serde_json::json!({"channel": "#Dev"}),
        )
        .await;
        assert_eq!(status, 401);
        let (status, _) = call(
            &app,
            "/api/voice/token",
            Some(&session),
            serde_json::json!({"channel": "dev"}),
        )
        .await;
        assert_eq!(status, 400);
        let (status, v) = call(
            &app,
            "/api/voice/token",
            Some(&session),
            serde_json::json!({"channel": "#Dev"}),
        )
        .await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["room"], "#dev");
        let claims = TokenVerifier::with_api_key("devkey", "0123456789abcdef0123456789abcdef")
            .verify(v["token"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            (claims.sub.as_str(), claims.video.room.as_str()),
            ("alice", "#dev")
        );
        // A session token is no LiveKit token, and vice versa.
        let (status, _) = call(
            &app,
            "/api/voice/token",
            Some(v["token"].as_str().unwrap()),
            serde_json::json!({"channel": "#dev"}),
        )
        .await;
        assert_eq!(status, 401);
    }
}
