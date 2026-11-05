//! The GIF picker's search (D30, D31): chadd asks KLIPY's API so KLIPY sees
//! neither who searched nor what for, and the key stays on the server. The
//! answer is hostile like any other (D14): bounded, and every item checked
//! before it's passed on.

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
/// Items carry a few renditions and an inline blurred preview each.
const MAX_BODY: usize = 1 << 20;
const MAX_QUERY: usize = 100;
const MAX_PAGE: u32 = 1000;
const MAX_ID: usize = 64;
const MAX_URL: usize = 2048;
const MAX_DIM: u64 = 4096;
/// Renditions to post, best first: big enough to look right, small enough
/// for `/api/media`.
const POST: [&str; 2] = ["md", "hd"];
const PREVIEW: [&str; 3] = ["sm", "xs", "md"];

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
    // `pos` is the page number we handed out as `next`.
    let page = req
        .pos
        .and_then(|p| p.parse::<u32>().ok())
        .filter(|p| (1..=MAX_PAGE).contains(p))
        .unwrap_or(1);
    let kind = if query.is_empty() {
        "trending"
    } else {
        "search"
    };
    // `<api>/<key>/gifs/<kind>`; the key is percent-encoded as a segment.
    let mut url = cfg.api.clone();
    url.path_segments_mut()
        .map_err(|()| unavailable())?
        .pop_if_empty()
        .extend([cfg.key.as_str(), "gifs", kind]);
    {
        let (page, limit) = (page.to_string(), LIMIT.to_string());
        let mut q = url.query_pairs_mut();
        q.append_pair("page", &page)
            .append_pair("per_page", &limit)
            .append_pair("content_filter", &cfg.content_filter)
            .append_pair("format_filter", "gif");
        if !query.is_empty() {
            q.append_pair("q", &query);
        }
    }
    let mut res = state
        .http
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
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
    if json.get("result").and_then(Value::as_bool) == Some(false) {
        tracing::warn!("gifs: KLIPY answered result=false");
        return Err(unavailable());
    }
    Ok(Json(parse(&json, page)))
}

fn unavailable() -> (StatusCode, Json<ApiError>) {
    fail(
        StatusCode::BAD_GATEWAY,
        "Couldn't get GIFs right now. Try again later.",
    )
}

/// KLIPY's `{result, data: {data: [..], has_next}}` for `page`, keeping only
/// items that check out.
pub fn parse(json: &Value, page: u32) -> GifPage {
    let data = json.get("data");
    let gifs = data
        .and_then(|d| d.get("data"))
        .and_then(Value::as_array)
        .map(|r| r.iter().take(LIMIT * 2).filter_map(gif).collect())
        .unwrap_or_default();
    let more = data
        .and_then(|d| d.get("has_next"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let next = (more && page < MAX_PAGE).then(|| (page + 1).to_string());
    GifPage { gifs, next }
}

fn gif(item: &Value) -> Option<Gif> {
    // Ads, stickers and the like aren't GIFs we can post.
    if item
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| t != "gif")
    {
        return None;
    }
    let id = item.get("id").and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })?;
    if id.is_empty() || id.len() > MAX_ID || id.chars().any(char::is_control) {
        return None;
    }
    let file = item.get("file")?.as_object()?;
    let rendition = |size: &str| -> Option<(Url, u32, u32, u64)> {
        let f = file.get(size)?.get("gif")?;
        let url = f.get("url")?.as_str().and_then(media_url)?;
        let bytes = f.get("size").and_then(Value::as_u64).unwrap_or(0);
        let dim = |k: &str| {
            f.get(k)
                .and_then(Value::as_u64)
                .filter(|d| (1..=MAX_DIM).contains(d))
                .map_or(0, |d| d as u32)
        };
        Some((url, dim("width"), dim("height"), bytes))
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
    let title = item
        .get("title")
        .and_then(Value::as_str)
        .map(fetch::clean)
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

    fn gif_at(url: &str, w: u64, h: u64, size: u64) -> Value {
        json!({ "gif": { "url": url, "width": w, "height": h, "size": size } })
    }

    fn page_of(items: Value, has_next: bool) -> Value {
        json!({ "result": true, "data": {
            "data": items, "current_page": 1, "per_page": 20, "has_next": has_next
        } })
    }

    #[test]
    fn parses_klipy() {
        let s = "https://static.klipy.com/ii/935d/14/af";
        let page = parse(
            &page_of(
                json!([
                    {
                        "id": 8041071659142944u64,
                        "slug": "hello-hi-662",
                        "title": "Hello &amp; hi",
                        "type": "gif",
                        "blur_preview": "data:image/jpeg;base64,AAAA",
                        "file": {
                            "hd": gif_at(&format!("{s}/hd.gif"), 498, 498, 9 << 20),
                            "md": gif_at(&format!("{s}/md.gif"), 320, 320, 900_000),
                            "sm": gif_at(&format!("{s}/sm.gif"), 220, 124, 90_000),
                            "xs": gif_at(&format!("{s}/xs.gif"), 90, 90, 9_000),
                        }
                    },
                    {
                        "id": "abc",
                        "title": "only hd",
                        "file": { "hd": gif_at(&format!("{s}/b.gif"), 0, 99999, 0) }
                    },
                    { "id": 1, "type": "ad", "file": { "md": gif_at(&format!("{s}/ad.gif"), 1, 1, 1) } }
                ]),
                true,
            ),
            3,
        );
        assert_eq!(page.next.as_deref(), Some("4"));
        assert_eq!(page.gifs.len(), 2);
        let a = &page.gifs[0];
        assert_eq!(a.id, "8041071659142944");
        assert_eq!(a.title, "Hello & hi");
        assert_eq!(a.url.as_str(), format!("{s}/md.gif"));
        assert_eq!(a.preview.as_str(), format!("{s}/sm.gif"));
        assert_eq!((a.width, a.height), (220, 124));
        let b = &page.gifs[1];
        assert_eq!(b.preview, b.url);
        assert_eq!((b.width, b.height), (0, 0));
    }

    #[test]
    fn drops_what_doesnt_check_out() {
        let bad = json!([
            { "id": "1", "file": { "md": gif_at("http://x.com/a.gif", 1, 1, 1) } },
            { "id": "2", "file": { "md": gif_at("https://127.0.0.1/a.gif", 1, 1, 1) } },
            { "id": "3", "file": { "md": gif_at("https://x.com/a.html", 1, 1, 1) } },
            { "id": "4", "file": { "md": gif_at("https://u:p@x.com/a.gif", 1, 1, 1) } },
            { "id": "5", "file": { "hd": gif_at("https://x.com/a.gif", 1, 1, 9 << 20) } },
            { "id": "", "file": { "md": gif_at("https://x.com/a.gif", 1, 1, 1) } },
            { "id": "x".repeat(100), "file": { "md": gif_at("https://x.com/a.gif", 1, 1, 1) } },
            { "id": "8", "file": { "md": { "mp4": { "url": "https://x.com/a.mp4" } } } },
            { "id": "9", "file": [] },
            { "id": "10" },
            "junk",
            null
        ]);
        let page = parse(&page_of(bad, false), 1);
        assert!(page.gifs.is_empty());
        assert_eq!(page.next, None);
        for junk in [
            json!(null),
            json!([]),
            json!({ "data": 5 }),
            json!({ "data": { "data": 5, "has_next": "yes" } }),
        ] {
            assert_eq!(parse(&junk, 1), GifPage::default());
        }
        // No paging past the cap.
        assert_eq!(parse(&page_of(json!([]), true), MAX_PAGE).next, None);
    }

    #[test]
    fn caps_the_page() {
        let item = json!({ "id": "1", "file": { "md": gif_at("https://x.com/a.gif", 1, 1, 1) } });
        let page = parse(&page_of(json!(vec![item; 500]), false), 1);
        assert_eq!(page.gifs.len(), LIMIT * 2);
    }
}
