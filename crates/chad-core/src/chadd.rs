//! Calls to chadd: a session token from the account password, then LiveKit
//! join tokens, uploads, link previews and proxied images with it. Responses
//! are bounded (D14).

use std::sync::Arc;
use std::time::{Duration, Instant};

use chad_proto::chadd::{
    self, ApiError, Game, GifPage, GifSearch, LinkPreview, LoginRequest, LoginResponse,
    PreviewRequest, RedactedRequest, RedactedResponse, Sound, UploadResponse, VoiceTokenRequest,
    VoiceTokenResponse,
};
use serde::{Serialize, de::DeserializeOwned};
use url::Url;

use crate::discovery;

const MAX_BODY: usize = 16 << 10;
/// A page of GIFs: a few URLs each, 40 at most.
const MAX_GIFS: usize = 256 << 10;
/// chadd's own cap on proxied images.
pub const MAX_MEDIA: usize = 8 << 20;
/// chadd's own caps on soundboard clips and the list.
const MAX_SOUND: usize = 4 << 20;
const MAX_SOUNDS: usize = 64 << 10;
/// A full page of the longest msgids, as JSON.
const MAX_REDACTED: usize = 256 << 10;
/// Pages of tombstones per connect; the rest waits for the next one.
const REDACTED_PAGES: usize = 10;

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

/// Has chadd delete the account with its messages and uploads (D69). Done once
/// chadd has queued it; the account is gone moments later.
pub async fn delete_account(api: &Url, account: &str, password: &str) -> Result<(), Error> {
    let req = LoginRequest {
        account: account.to_owned(),
        password: password.to_owned(),
    };
    let json = serde_json::to_vec(&req).map_err(|e| Error::Other(e.to_string()))?;
    request(api, chadd::ACCOUNT_DELETE, None, Body::Json(json), MAX_BODY)
        .await
        .map(drop)
}

/// Msgids chadd deleted after `since` (D69), and the cursor to keep. With no
/// `since` (a new device), just the cursor. `None` if chadd can't be asked or
/// answers nonsense: nothing to redact this time.
pub async fn redacted(api: &Url, since: Option<u64>) -> Option<(Vec<String>, u64)> {
    catch_up(since, |since| async move {
        let json = serde_json::to_vec(&RedactedRequest { since }).ok()?;
        let bytes = request(api, chadd::REDACTED, None, Body::Json(json), MAX_REDACTED)
            .await
            .ok()?;
        parse(&bytes).ok()
    })
    .await
}

/// `redacted`'s paging and checks, over any way of fetching a page.
async fn catch_up<F: Future<Output = Option<RedactedResponse>>>(
    since: Option<u64>,
    mut page: impl FnMut(Option<u64>) -> F,
) -> Option<(Vec<String>, u64)> {
    let mut ids = Vec::new();
    let mut cursor = since;
    for _ in 0..REDACTED_PAGES {
        let r = page(cursor).await?;
        if r.ids.len() > chadd::REDACTED_PAGE {
            return None;
        }
        let Some(c) = cursor else {
            return Some((ids, r.next));
        };
        // Behind us: chadd lost its log. Start over from where it is now.
        if r.next < c {
            return Some((ids, r.next));
        }
        ids.extend(r.ids.into_iter().filter(|i| chadd::valid_msgid(i)));
        cursor = Some(r.next);
        if r.next - c < chadd::REDACTED_PAGE as u64 {
            break;
        }
    }
    cursor.map(|c| (ids, c))
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

/// A page of GIFs from chadd's provider.
pub async fn gifs(
    api: &Url,
    session: &str,
    query: String,
    pos: Option<String>,
) -> Result<GifPage, Error> {
    let req = GifSearch { query, pos };
    let json = serde_json::to_vec(&req).map_err(|e| Error::Other(e.to_string()))?;
    let mut page: GifPage =
        parse(&request(api, chadd::GIFS, Some(session), Body::Json(json), MAX_GIFS).await?)?;
    page.gifs.truncate(40);
    page.gifs
        .retain(|g| g.url.scheme() == "https" && g.preview.scheme() == "https" && g.id.len() <= 64);
    for g in &mut page.gifs {
        g.title = g
            .title
            .chars()
            .filter(|c| !c.is_control())
            .take(300)
            .collect();
    }
    page.next = page.next.filter(|n| n.len() <= 128);
    Ok(page)
}

/// The soundboard; names and files checked (D14).
pub async fn sounds(api: &Url, session: &str) -> Result<Vec<Sound>, Error> {
    let mut sounds: Vec<Sound> = parse(
        &request(
            api,
            chadd::SOUNDS,
            Some(session),
            Body::Json(b"{}".to_vec()),
            MAX_SOUNDS,
        )
        .await?,
    )?;
    sounds.truncate(200);
    sounds.retain(|s| s.file.len() <= 100 && !s.file.contains(['/', '\\']));
    for s in &mut sounds {
        s.name = s
            .name
            .chars()
            .filter(|c| !c.is_control())
            .take(40)
            .collect();
    }
    Ok(sounds)
}

/// Discord's game list; ids and names checked (D14).
pub async fn games(api: &Url, session: &str) -> Result<Vec<Game>, Error> {
    let mut games: Vec<Game> = parse(
        &request(
            api,
            chadd::GAMES,
            Some(session),
            Body::Json(b"{}".to_vec()),
            chad_proto::MAX_GAMES_BYTES,
        )
        .await?,
    )?;
    games.truncate(200_000);
    games.retain(|g| (1..=20).contains(&g.id.len()) && g.id.bytes().all(|b| b.is_ascii_digit()));
    for g in &mut games {
        g.name = chad_proto::one_line(&g.name, 100);
        g.executables.truncate(16);
        g.executables
            .retain_mut(|e| chadd::executable(e).map(|n| *e = n).is_some());
    }
    Ok(games)
}

/// One soundboard clip's bytes.
pub async fn sound(api: &Url, session: &str, file: &str) -> Result<Vec<u8>, Error> {
    let req = Sound {
        name: String::new(),
        file: file.to_owned(),
    };
    let json = serde_json::to_vec(&req).map_err(|e| Error::Other(e.to_string()))?;
    request(
        api,
        chadd::SOUND,
        Some(session),
        Body::Json(json),
        MAX_SOUND,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A log of `n` msgids, served the way chadd pages it.
    fn log(n: u64) -> impl FnMut(Option<u64>) -> std::future::Ready<Option<RedactedResponse>> {
        move |since| {
            let next = since.map_or(n, |s| {
                (s + chadd::REDACTED_PAGE as u64).min(n).max(s.min(n))
            });
            let ids = since.map_or(0, |s| s.min(n))..next;
            std::future::ready(Some(RedactedResponse {
                ids: ids.map(|i| format!("m{i}")).collect(),
                next,
            }))
        }
    }

    #[tokio::test]
    async fn catches_up() {
        // A new device: only the cursor.
        assert_eq!(catch_up(None, log(5)).await, Some((vec![], 5)));
        let (ids, next) = catch_up(Some(3), log(5)).await.unwrap();
        assert_eq!((ids, next), (vec!["m3".to_owned(), "m4".to_owned()], 5));
        assert_eq!(catch_up(Some(5), log(5)).await, Some((vec![], 5)));
        // Pages until the end, at most REDACTED_PAGES of them.
        let (ids, next) = catch_up(Some(0), log(2500)).await.unwrap();
        assert_eq!((ids.len(), next), (2500, 2500));
        let (ids, next) = catch_up(Some(0), log(50_000)).await.unwrap();
        assert_eq!((ids.len(), next), (10_000, 10_000));
        // chadd lost its log: resync, redact nothing.
        assert_eq!(catch_up(Some(9), log(5)).await, Some((vec![], 5)));
    }

    #[tokio::test]
    async fn distrusts_chadd() {
        let page = |ids: Vec<&str>, next| {
            let ids: Vec<String> = ids.into_iter().map(str::to_owned).collect();
            move |_| {
                std::future::ready(Some(RedactedResponse {
                    ids: ids.clone(),
                    next,
                }))
            }
        };
        let (ids, _) = catch_up(Some(0), page(vec!["ok", "", "a b", "x:y"], 4))
            .await
            .unwrap();
        assert_eq!(ids, ["ok"]);
        let flood = vec!["m"; chadd::REDACTED_PAGE + 1];
        assert_eq!(catch_up(Some(0), page(flood, 2000)).await, None);
        assert_eq!(catch_up(Some(0), |_| std::future::ready(None)).await, None);
    }
}
