//! Link previews and the image proxy (D25): chadd fetches what's linked, so
//! people posting links never learn who read them. The URLs come from
//! anyone on the network, so only public addresses are fetched.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chad_proto::chadd::{ApiError, LinkPreview, PreviewRequest};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::Url;

use crate::api::{self, ApiResult, fail};
use crate::files::sniff;
use crate::routes::AppState;

/// Enough for any `<head>`.
const MAX_PAGE: usize = 512 << 10;
pub const MAX_IMAGE: usize = 8 << 20;
const MAX_FIELD: usize = 300;

/// Global unicast only: no loopback, private, link-local, CGNAT, multicast or
/// reserved ranges, so a link can't reach chadd's own network.
pub fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || a == 0
                || a >= 240
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && v4.octets()[2] == 0))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public(IpAddr::V4(v4));
            }
            let seg = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg & 0xfe00) == 0xfc00
                || (seg & 0xffc0) == 0xfe80
                || seg == 0x2001 && v6.segments()[1] == 0xdb8)
        }
    }
}

/// Resolves names, then drops every address that isn't [`public`].
struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .filter(|a| public(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err("no public address".into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// http(s) to a name or a public IP literal (those skip the resolver).
fn allowed(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && match url.host() {
            Some(url::Host::Domain(_)) => true,
            Some(url::Host::Ipv4(ip)) => public(ip.into()),
            Some(url::Host::Ipv6(ip)) => public(ip.into()),
            None => false,
        }
}

pub fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(4))
        .dns_resolver(Arc::new(PublicOnly))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() >= 5 || !allowed(a.url()) {
                a.stop()
            } else {
                a.follow()
            }
        }))
        .user_agent("Mozilla/5.0 (compatible; IrChad link preview)")
        .build()
}

/// At most `max` bytes of the body at `url`, and its final URL.
async fn get(state: &AppState, url: &str, max: usize) -> Result<(Url, Vec<u8>), StatusCode> {
    let url = Url::parse(url).map_err(|_| StatusCode::BAD_REQUEST)?;
    if !allowed(&url) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut res = state
        .fetcher
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| {
            tracing::info!("fetch: {e}");
            StatusCode::BAD_GATEWAY
        })?;
    let url = res.url().clone();
    let body = capped(&mut res, max)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok((url, body))
}

/// At most `max` bytes of `res`'s body; the rest is never read.
pub async fn capped(res: &mut reqwest::Response, max: usize) -> reqwest::Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await? {
        let room = max - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if body.len() >= max {
            break;
        }
    }
    Ok(body)
}

fn throttle(state: &AppState, headers: &HeaderMap) -> Result<String, (StatusCode, Json<ApiError>)> {
    let account = api::session_account(state, headers)?;
    if !state.fetches.allow(account.to_ascii_lowercase()) {
        return Err(fail(StatusCode::TOO_MANY_REQUESTS, "Slow down a little."));
    }
    Ok(account)
}

pub async fn preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<PreviewRequest>,
) -> ApiResult<LinkPreview> {
    throttle(&state, &headers)?;
    let (url, body) = get(&state, &req.url, MAX_PAGE)
        .await
        .map_err(|s| fail(s, "Couldn't fetch that link."))?;
    let html = String::from_utf8_lossy(&body);
    Ok(Json(parse_html(&html, &url)))
}

pub async fn media(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<PreviewRequest>,
) -> Response {
    if let Err(e) = throttle(&state, &headers) {
        return e.into_response();
    }
    let body = match get(&state, &req.url, MAX_IMAGE + 1).await {
        Ok((_, b)) if b.len() <= MAX_IMAGE => b,
        Ok(_) => {
            return fail(StatusCode::PAYLOAD_TOO_LARGE, "That image is too big.").into_response();
        }
        Err(s) => return fail(s, "Couldn't fetch that image.").into_response(),
    };
    let kind = match sniff(&body) {
        Some("png") => "image/png",
        Some("jpg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => {
            return fail(StatusCode::UNSUPPORTED_MEDIA_TYPE, "That isn't an image.")
                .into_response();
        }
    };
    let mut res = Body::from(body).into_response();
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    res
}

/// OpenGraph tags, falling back to `<title>` and `description`. Tolerant of
/// junk, and bounded by the input it's given.
pub fn parse_html(html: &str, base: &Url) -> LinkPreview {
    let mut p = LinkPreview::default();
    let lower = html.to_ascii_lowercase();
    let mut rest = 0;
    while let Some(i) = lower[rest..].find("<meta") {
        let start = rest + i + 5;
        let Some(end) = lower[start..].find('>').map(|e| start + e) else {
            break;
        };
        let tag = &html[start..end];
        rest = end;
        let (key, content) = (
            attr(tag, "property").or_else(|| attr(tag, "name")),
            attr(tag, "content"),
        );
        let (Some(key), Some(content)) = (key, content) else {
            continue;
        };
        let slot = match key.to_ascii_lowercase().as_str() {
            "og:title" => &mut p.title,
            "og:description" => &mut p.description,
            "description" if p.description.is_none() => &mut p.description,
            "og:site_name" => &mut p.site,
            "og:image" | "og:image:url" if p.image.is_none() => {
                p.image = base
                    .join(&content)
                    .ok()
                    .filter(|u| matches!(u.scheme(), "http" | "https"));
                continue;
            }
            _ => continue,
        };
        let v = clean(&content);
        if !v.is_empty() {
            *slot = Some(v);
        }
    }
    if p.title.is_none()
        && let Some(i) = lower.find("<title")
        && let Some(open) = lower[i..].find('>').map(|o| i + o + 1)
        && let Some(close) = lower[open..].find("</title").map(|c| open + c)
    {
        p.title = Some(clean(&html[open..close])).filter(|t| !t.is_empty());
    }
    p
}

/// `name="value"`, `name='value'` or `name=value` in a tag's attributes.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let after = lower[from..].trim_start();
        if !before_ok || !after.starts_with('=') {
            continue;
        }
        let v = tag[tag.len() - after.len() + 1..].trim_start();
        return Some(match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or_default().to_owned(),
            _ => v
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or_default()
                .to_owned(),
        });
    }
    None
}

/// Entities decoded, whitespace collapsed, length capped.
pub fn clean(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.bytes().take(10).position(|b| b == b';');
        let decoded = end.and_then(|e| {
            let ent = &rest[1..e];
            let c = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ => ent
                    .strip_prefix("#x")
                    .or_else(|| ent.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    let out: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    out.chars()
        .filter(|c| !c.is_control())
        .take(MAX_FIELD)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_addresses() {
        for bad in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!public(bad.parse().unwrap()), "{bad}");
        }
        for good in ["1.1.1.1", "93.184.216.34", "2606:4700::1111"] {
            assert!(public(good.parse().unwrap()), "{good}");
        }
        let url = |s| Url::parse(s).unwrap();
        assert!(!allowed(&url("http://127.0.0.1/")));
        assert!(!allowed(&url("http://[::1]/")));
        assert!(!allowed(&url("file:///etc/passwd")));
        assert!(allowed(&url("https://example.com/")));
    }

    #[test]
    fn parses_opengraph() {
        let base = Url::parse("https://example.com/a/b").unwrap();
        let html = r#"<html><head><TITLE> Fallback </TITLE>
            <meta property="og:site_name" content="">
            <meta property="og:title" content="Cats &amp; dogs">
            <meta name='description' content='plain desc'>
            <meta content="og desc" property=og:description />
            <meta property="og:image" content="/img.png">
            <meta property="og:site_name" content="Ex&#x41;mple">
            <meta name="broken content=">
            <meta"#;
        let p = parse_html(html, &base);
        assert_eq!(p.title.as_deref(), Some("Cats & dogs"));
        assert_eq!(p.description.as_deref(), Some("og desc"));
        assert_eq!(p.site.as_deref(), Some("ExAmple"));
        assert_eq!(p.image.unwrap().as_str(), "https://example.com/img.png");

        let p = parse_html(
            r#"<meta property="og:title" content=" "><title>T</title>"#,
            &base,
        );
        assert_eq!(p.title.as_deref(), Some("T"));
        let p = parse_html("<title>Just\n a  title</title>", &base);
        assert_eq!(p.title.as_deref(), Some("Just a title"));
        // Junk doesn't panic.
        for junk in [
            "<meta",
            "<meta content=",
            "&",
            "<title>",
            "<meta property=\"og:title\" content=\"&#xffffffff;\">",
            "<meta name=x content='é",
        ] {
            parse_html(junk, &base);
        }
    }
}
