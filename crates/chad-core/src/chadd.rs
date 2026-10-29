//! Calls to chadd: a session token from the account password, then LiveKit
//! join tokens with it. Responses are bounded (D14).

use std::time::{Duration, Instant};

use chad_proto::chadd::{
    self, ApiError, LoginRequest, LoginResponse, VoiceTokenRequest, VoiceTokenResponse,
};
use serde::{Serialize, de::DeserializeOwned};
use url::Url;

use crate::discovery;

const MAX_BODY: usize = 16 << 10;

/// A chadd session token and when to stop using it.
#[derive(Debug, Clone)]
pub struct Session {
    pub token: String,
    pub until: Instant,
}

pub enum Error {
    /// The session is no good (expired, or chadd restarted).
    Unauthorized(String),
    Other(String),
}

impl Error {
    pub fn message(self) -> String {
        match self {
            Self::Unauthorized(m) | Self::Other(m) => m,
        }
    }
}

pub async fn login(api: &Url, account: &str, password: &str) -> Result<Session, Error> {
    let req = LoginRequest {
        account: account.to_owned(),
        password: password.to_owned(),
    };
    let r: LoginResponse = post(api, chadd::LOGIN, None, &req).await?;
    // A minute early, so it doesn't run out mid-request.
    let ttl = Duration::from_secs(r.expires_in.min(86_400)).saturating_sub(Duration::from_secs(60));
    Ok(Session {
        token: r.token,
        until: Instant::now() + ttl,
    })
}

pub async fn voice_token(
    api: &Url,
    session: &str,
    channel: &str,
) -> Result<VoiceTokenResponse, Error> {
    let req = VoiceTokenRequest {
        channel: channel.to_owned(),
    };
    post(api, chadd::VOICE_TOKEN, Some(session), &req).await
}

async fn post<T: DeserializeOwned>(
    api: &Url,
    path: &str,
    bearer: Option<&str>,
    body: &impl Serialize,
) -> Result<T, Error> {
    let url = chadd::endpoint(api, path);
    let unreachable = |e: reqwest::Error| {
        tracing::warn!("chadd {url}: {e:?}");
        Error::Other("Couldn't reach the server. Try again in a bit.".into())
    };
    let json = serde_json::to_vec(body).map_err(|e| Error::Other(e.to_string()))?;
    let mut req = discovery::client(&url)
        .map_err(unreachable)?
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(json);
    if let Some(t) = bearer {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(unreachable)?;
    let status = resp.status();
    let bytes = discovery::body(resp, MAX_BODY)
        .await
        .map_err(unreachable)?
        .unwrap_or_default();
    if status.is_success() {
        return serde_json::from_slice(&bytes).map_err(|e| {
            tracing::warn!("chadd {url}: {e}");
            Error::Other("The server sent something odd. Try again later.".into())
        });
    }
    let message = serde_json::from_slice::<ApiError>(&bytes)
        .map(|e| e.error.chars().take(200).collect())
        .unwrap_or_else(|_| format!("The server said no ({status})."));
    Err(if status == reqwest::StatusCode::UNAUTHORIZED {
        Error::Unauthorized(message)
    } else {
        Error::Other(message)
    })
}
