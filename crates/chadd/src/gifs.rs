//! The GIF picker's search (D30): chadd asks a Tenor v2-shaped API (KLIPY's
//! by default) so the provider sees neither who searched nor what for, and
//! the key stays on the server. The answer is hostile like any other (D14):
//! bounded, and every item checked before it's passed on.

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use chad_proto::chadd::{ApiError, Gif, GifPage, GifSearch};
use serde_json::Value;
use url::Url;

use crate::api::{self, ApiResult, fail};
use crate::fetch::{self, MAX_IMAGE};
use crate::routes::AppState;

/// Per page. Each one is a thumbnail through `/api/media` too.
const LIMIT: usize = 20;
const MAX_BODY: usize = 256 << 10;
const MAX_QUERY: usize = 100;
const MAX_POS: usize = 128;
const MAX_ID: usize = 64;
const MAX_URL: usize = 2048;
const MAX_DIM: u64 = 4096;
/// Renditions to post, best first: big enough to look right, small enough
/// for `/api/media`.
const POST: [&str; 2] = ["mediumgif", "gif"];
const PREVIEW: [&str; 3] = ["tinygif", "nanogif", "mediumgif"];

pub async fn search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<GifSearch>,
) -> ApiResult<GifPage> {
    let Some(cfg) = &state.config.gifs else {
        return Err(fail(StatusCode::NOT_FOUND, "GIFs are off on this server."));
    };
    let account = api::session_account(&state, &headers)?;
    if !state.gifs.allow(account.to_ascii_lowercase()) {
        return Err(fail(StatusCode::TOO_MANY_REQUESTS, "Slow down a little."));
    }
    let query: String = req.query.trim().chars().take(MAX_QUERY).collect();
    let pos = req.pos.filter(|p| !p.is_empty() && p.len() <= MAX_POS);
    let path = if query.is_empty() {
        "featured"
    } else {
        "search"
    };
    let mut url = cfg.api.join(path).map_err(|_| unavailable())?;
    {
        let limit = LIMIT.to_string();
        let mut q = url.query_pairs_mut();
        q.append_pair("key", &cfg.key)
            .append_pair("client_key", "irchad")
            .append_pair("limit", &limit)
            .append_pair("contentfilter", &cfg.content_filter)
            .append_pair("media_filter", "gif,mediumgif,tinygif,nanogif");
        if !query.is_empty() {
            q.append_pair("q", &query);
        }
        if let Some(pos) = &pos {
            q.append_pair("pos", pos);
        }
    }
    let mut res = state
        .http
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| {
            // Without the URL: it has the key in it.
            tracing::warn!("gifs: {}", e.without_url());
            unavailable()
        })?;
    let body = fetch::capped(&mut res, MAX_BODY + 1)
        .await
        .map_err(|_| unavailable())?;
    if body.len() > MAX_BODY {
        tracing::warn!("gifs: answer over {MAX_BODY} bytes");
        return Err(unavailable());
    }
    let json: Value = serde_json::from_slice(&body).map_err(|e| {
        tracing::warn!("gifs: {e}");
        unavailable()
    })?;
    Ok(Json(parse(&json)))
}

fn unavailable() -> (StatusCode, Json<ApiError>) {
    fail(
        StatusCode::BAD_GATEWAY,
        "Couldn't get GIFs right now. Try again later.",
    )
}

/// A Tenor v2 `{results, next}`, keeping only items that check out.
pub fn parse(json: &Value) -> GifPage {
    let gifs = json
        .get("results")
        .and_then(Value::as_array)
        .map(|r| r.iter().take(LIMIT * 2).filter_map(gif).collect())
        .unwrap_or_default();
    let next = json
        .get("next")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty() && *n != "0" && n.len() <= MAX_POS)
        .filter(|n| n.chars().all(|c| c.is_ascii_graphic()))
        .map(str::to_owned);
    GifPage { gifs, next }
}

fn gif(item: &Value) -> Option<Gif> {
    let id = item.get("id").and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })?;
    if id.is_empty() || id.len() > MAX_ID || id.chars().any(char::is_control) {
        return None;
    }
    let formats = item.get("media_formats")?.as_object()?;
    let rendition = |name: &str| -> Option<(Url, u32, u32, u64)> {
        let f = formats.get(name)?;
        let url = f.get("url")?.as_str().and_then(media_url)?;
        let size = f.get("size").and_then(Value::as_u64).unwrap_or(0);
        let dims = f.get("dims").and_then(Value::as_array);
        let dim = |i: usize| {
            dims.and_then(|d| d.get(i))
                .and_then(Value::as_u64)
                .filter(|d| (1..=MAX_DIM).contains(d))
                .map_or(0, |d| d as u32)
        };
        Some((url, dim(0), dim(1), size))
    };
    // An unknown size (0) is left to `/api/media`'s own cap.
    let (url, ..) = POST
        .iter()
        .filter_map(|n| rendition(n))
        .find(|r| r.3 as usize <= MAX_IMAGE)?;
    let (preview, width, height, _) =
        PREVIEW
            .iter()
            .find_map(|n| rendition(n))
            .unwrap_or((url.clone(), 0, 0, 0));
    let title = ["title", "content_description"]
        .iter()
        .filter_map(|k| item.get(*k)?.as_str())
        .map(fetch::clean)
        .find(|t| !t.is_empty())
        .unwrap_or_default();
    Some(Gif {
        id,
        title,
        preview,
        url,
        width,
        height,
    })
}

/// An https link to a name whose path ends like an image, so clients show it
/// inline. Fetched through `/api/media` later, which checks the address.
fn media_url(s: &str) -> Option<Url> {
    if s.len() > MAX_URL {
        return None;
    }
    let u = Url::parse(s).ok()?;
    let ext = u.path().rsplit_once('.')?.1.to_ascii_lowercase();
    (u.scheme() == "https"
        && matches!(u.host(), Some(url::Host::Domain(_)))
        && u.username().is_empty()
        && u.password().is_none()
        && matches!(ext.as_str(), "gif" | "webp" | "png" | "jpg" | "jpeg"))
    .then_some(u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fmt(url: &str, w: u64, h: u64, size: u64) -> Value {
        json!({ "url": url, "dims": [w, h], "duration": 0, "size": size })
    }

    #[test]
    fn parses_tenor_v2() {
        let page = parse(&json!({
            "results": [
                {
                    "id": "123",
                    "title": "",
                    "content_description": "Cat &amp; keyboard",
                    "media_formats": {
                        "gif": fmt("https://media.klipy.com/a/full.gif", 498, 280, 9 << 20),
                        "mediumgif": fmt("https://media.klipy.com/a/medium.gif", 320, 180, 900_000),
                        "tinygif": fmt("https://media.klipy.com/a/tiny.gif", 220, 124, 90_000),
                    }
                },
                {
                    "id": 456,
                    "title": "only gif",
                    "media_formats": { "gif": fmt("https://media.klipy.com/b.gif", 0, 99999, 0) }
                }
            ],
            "next": "CAgQ"
        }));
        assert_eq!(page.next.as_deref(), Some("CAgQ"));
        assert_eq!(page.gifs.len(), 2);
        let a = &page.gifs[0];
        assert_eq!(a.id, "123");
        assert_eq!(a.title, "Cat & keyboard");
        // The full GIF is over /api/media's cap.
        assert_eq!(a.url.as_str(), "https://media.klipy.com/a/medium.gif");
        assert_eq!(a.preview.as_str(), "https://media.klipy.com/a/tiny.gif");
        assert_eq!((a.width, a.height), (220, 124));
        let b = &page.gifs[1];
        assert_eq!(b.id, "456");
        assert_eq!(b.preview, b.url);
        assert_eq!((b.width, b.height), (0, 0));
    }

    #[test]
    fn drops_what_doesnt_check_out() {
        let bad = [
            json!({ "id": "1", "media_formats": { "gif": fmt("http://x.com/a.gif", 1, 1, 1) } }),
            json!({ "id": "2", "media_formats": { "gif": fmt("https://127.0.0.1/a.gif", 1, 1, 1) } }),
            json!({ "id": "3", "media_formats": { "gif": fmt("https://x.com/a.html", 1, 1, 1) } }),
            json!({ "id": "4", "media_formats": { "gif": fmt("https://u:p@x.com/a.gif", 1, 1, 1) } }),
            json!({ "id": "5", "media_formats": { "gif": fmt("https://x.com/a.gif", 1, 1, 9 << 20) } }),
            json!({ "id": "", "media_formats": { "gif": fmt("https://x.com/a.gif", 1, 1, 1) } }),
            json!({ "id": "x".repeat(100), "media_formats": { "gif": fmt("https://x.com/a.gif", 1, 1, 1) } }),
            json!({ "id": "8", "media_formats": [] }),
            json!({ "id": "9" }),
            json!("junk"),
            json!(null),
        ];
        let page = parse(&json!({ "results": bad, "next": "" }));
        assert!(page.gifs.is_empty());
        assert_eq!(page.next, None);
        for junk in [json!(null), json!([]), json!({ "results": 5, "next": 5 })] {
            assert_eq!(parse(&junk), GifPage::default());
        }
        let page = parse(&json!({ "next": "a\nb" }));
        assert_eq!(page.next, None);
    }

    #[test]
    fn caps_the_page() {
        let item =
            json!({ "id": "1", "media_formats": { "gif": fmt("https://x.com/a.gif", 1, 1, 1) } });
        let page = parse(&json!({ "results": vec![item; 500] }));
        assert_eq!(page.gifs.len(), LIMIT * 2);
    }
}
