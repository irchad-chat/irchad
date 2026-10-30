//! Calls to chadd: a session token from the account password, then LiveKit
//! join tokens, uploads, link previews and proxied images with it. Responses
//! are bounded (D14).

use std::sync::Arc;
use std::time::{Duration, Instant};

use chad_proto::chadd::{
    self, ApiError, LinkPreview, LoginRequest, LoginResponse, PreviewRequest, UploadResponse,
    VoiceTokenRequest, VoiceTokenResponse,
};
use serde::{Serialize, de::DeserializeOwned};
use url::Url;

use crate::discovery;

const MAX_BODY: usize = 16 << 10;
/// chadd's own cap on proxied images.
pub const MAX_MEDIA: usize = 8 << 20;

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

/// Logs in on first use and again when the session runs out or is refused.
/// Shared by every call on a network, across reconnects.
pub struct Client {
    pub api: Url,
    account: String,
    /// `None` for a guest: no chadd session.
    password: Option<String>,
    session: Arc<tokio::sync::Mutex<Option<Session>>>,
}

impl Client {
    pub fn new(
        api: Url,
        account: String,
        password: Option<String>,
        session: Arc<tokio::sync::Mutex<Option<Session>>>,
    ) -> Self {
        Self {
            api,
            account,
            password,
            session,
        }
    }

    /// `f` with a session token, logging in first if there's none, or retrying
    /// once with a fresh one if chadd refuses it.
    // Owned arguments, not `AsyncFn`: its futures can't be proven `Send` yet.
    pub async fn call<T, F>(&self, f: impl Fn(Url, String) -> F) -> Result<T, String>
    where
        F: Future<Output = Result<T, Error>>,
    {
        let Some(password) = &self.password else {
            return Err("That needs an account. Sign up to use it.".into());
        };
        let live = |s: &Session| s.until > Instant::now();
        let current = self.session.lock().await.clone().filter(live);
        if let Some(s) = &current {
            match f(self.api.clone(), s.token.clone()).await {
                Err(Error::Unauthorized(_)) => {}
                r => return r.map_err(Error::message),
            }
        }
        let token = {
            // Held while logging in, so concurrent calls log in once.
            let mut session = self.session.lock().await;
            let fresh = session
                .clone()
                .filter(|s| live(s) && current.as_ref().is_none_or(|c| c.token != s.token));
            match fresh {
                Some(s) => s.token,
                None => {
                    let s = login(&self.api, &self.account, password)
                        .await
                        .map_err(Error::message)?;
                    *session = Some(s.clone());
                    s.token
                }
            }
        };
        f(self.api.clone(), token).await.map_err(Error::message)
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

/// Uploads a file; the URL it's served at.
pub async fn upload(api: &Url, session: &str, name: &str, bytes: Vec<u8>) -> Result<Url, Error> {
    // RFC 6266's `filename*` carries any name; chadd sanitizes it anyway.
    let quoted: String = name
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    let body = Body::Raw {
        bytes,
        disposition: format!("attachment; filename*=UTF-8''{quoted}"),
    };
    let r: UploadResponse =
        parse(&request(api, chadd::UPLOAD, Some(session), body, MAX_BODY).await?)?;
    Ok(r.url)
}

pub async fn preview(api: &Url, session: &str, url: &str) -> Result<LinkPreview, Error> {
    let req = PreviewRequest {
        url: url.to_owned(),
    };
    post(api, chadd::PREVIEW, Some(session), &req).await
}

/// The image at `url`, fetched by chadd.
pub async fn media(api: &Url, session: &str, url: &str) -> Result<Vec<u8>, Error> {
    let req = PreviewRequest {
        url: url.to_owned(),
    };
    let json = serde_json::to_vec(&req).map_err(|e| Error::Other(e.to_string()))?;
    request(
        api,
        chadd::MEDIA,
        Some(session),
        Body::Json(json),
        MAX_MEDIA,
    )
    .await
}

enum Body {
    Json(Vec<u8>),
    Raw { bytes: Vec<u8>, disposition: String },
}

async fn post<T: DeserializeOwned>(
    api: &Url,
    path: &str,
    bearer: Option<&str>,
    body: &impl Serialize,
) -> Result<T, Error> {
    let json = serde_json::to_vec(body).map_err(|e| Error::Other(e.to_string()))?;
    parse(&request(api, path, bearer, Body::Json(json), MAX_BODY).await?)
}

fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes).map_err(|e| {
        tracing::warn!("chadd: {e}");
        Error::Other("The server sent something odd. Try again later.".into())
    })
}

/// The body of a 2xx answer, at most `max` bytes.
async fn request(
    api: &Url,
    path: &str,
    bearer: Option<&str>,
    body: Body,
    max: usize,
) -> Result<Vec<u8>, Error> {
    let url = chadd::endpoint(api, path);
    let unreachable = |e: reqwest::Error| {
        tracing::warn!("chadd {url}: {e:?}");
        Error::Other("Couldn't reach the server. Try again in a bit.".into())
    };
    let req = discovery::client(&url).map_err(unreachable)?.post(&url);
    let mut req = match body {
        Body::Json(json) => req
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(json),
        Body::Raw { bytes, disposition } => req
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(reqwest::header::CONTENT_DISPOSITION, disposition)
            // Big files on slow uplinks: well past the default timeout.
            .timeout(Duration::from_secs(600))
            .body(bytes),
    };
    if let Some(t) = bearer {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(unreachable)?;
    let status = resp.status();
    let bytes = discovery::body(resp, max.max(MAX_BODY))
        .await
        .map_err(unreachable)?;
    if status.is_success() {
        return bytes.ok_or_else(|| Error::Other("That's too big.".into()));
    }
    let bytes = bytes.unwrap_or_default();
    let message = serde_json::from_slice::<ApiError>(&bytes)
        .map(|e| e.error.chars().take(200).collect())
        .unwrap_or_else(|_| format!("The server said no ({status})."));
    Err(if status == reqwest::StatusCode::UNAUTHORIZED {
        Error::Unauthorized(message)
    } else {
        Error::Other(message)
    })
}
