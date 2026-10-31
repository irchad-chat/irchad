//! Uploads: `POST /api/upload` stores the body, `/uploads/<id>/<name>` serves
//! it. The type comes from the bytes, never the client, and only images and
//! video are served inline; everything else downloads as an attachment.

use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::Engine;
use chad_proto::chadd::UploadResponse;
#[cfg(test)]
use url::Url;

use crate::api::{self, fail};
use crate::routes::AppState;

/// Served inline, by extension. Uploads only get these extensions when the
/// bytes say so.
const INLINE: [(&str, &str); 6] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
];

/// The media type (as an `INLINE` extension) the leading bytes say, if any.
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    let at = |i: usize, m: &[u8]| b.get(i..i + m.len()) == Some(m);
    if at(0, b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if at(0, b"\xff\xd8\xff") {
        Some("jpg")
    } else if at(0, b"GIF87a") || at(0, b"GIF89a") {
        Some("gif")
    } else if at(0, b"RIFF") && at(8, b"WEBP") {
        Some("webp")
    } else if at(4, b"ftyp") {
        Some("mp4")
    } else if at(0, b"\x1a\x45\xdf\xa3") {
        Some("webm")
    } else {
        None
    }
}

/// A file name that is safe in a path and a URL: `[A-Za-z0-9._-]`, no leading
/// dot, and for media the extension the bytes call for.
fn file_name(given: &str, ext: Option<&str>) -> String {
    let given = given.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut name: String = given
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    name = name.trim_start_matches('.').to_owned();
    if let Some(ext) = ext {
        let stem = match name.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => stem.to_owned(),
            _ => name.clone(),
        };
        name = format!("{}.{ext}", if stem.is_empty() { "file" } else { &stem });
    } else if let Some((_, e)) = name.rsplit_once('.')
        && INLINE.iter().any(|(i, _)| i.eq_ignore_ascii_case(e))
    {
        // Claims to be media but isn't: don't let it be served inline.
        name.push_str(".bin");
    }
    if name.is_empty() { "file".into() } else { name }
}

/// `filename="…"` (or `filename*=UTF-8''…`, taken as-is) from Content-Disposition.
fn disposition_name(headers: &HeaderMap) -> String {
    let v = headers
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    v.split(';')
        .filter_map(|p| p.trim().split_once('='))
        .find(|(k, _)| k.eq_ignore_ascii_case("filename") || k.eq_ignore_ascii_case("filename*"))
        .map(|(_, v)| {
            let v = v.trim().trim_matches('"');
            match v.strip_prefix("UTF-8''") {
                Some(enc) => percent_decode(enc),
                None => v.to_owned(),
            }
        })
        .unwrap_or_default()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (b[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Bearer session token, or Basic with the IRC account (FILEHOST clients).
async fn account(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<String, (StatusCode, Json<chad_proto::chadd::ApiError>)> {
    let basic = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .and_then(|b| String::from_utf8(b).ok());
    match basic {
        Some(creds) => {
            let (user, pass) = creds.split_once(':').unwrap_or((&creds, ""));
            api::check_password(state, headers, user, pass).await
        }
        None => api::session_account(state, headers),
    }
}

pub async fn upload(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match store(&state, &headers, body).await {
        Ok(url) => {
            let location = HeaderValue::from_str(url.as_str()).ok();
            let mut res = (StatusCode::CREATED, Json(UploadResponse { url })).into_response();
            if let Some(l) = location {
                res.headers_mut().insert(header::LOCATION, l);
            }
            res
        }
        Err(e) => e.into_response(),
    }
}

async fn store(
    state: &AppState,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<url::Url, (StatusCode, Json<chad_proto::chadd::ApiError>)> {
    let Some(cfg) = &state.config.uploads else {
        return Err(fail(
            StatusCode::NOT_FOUND,
            "This server doesn't take uploads.",
        ));
    };
    let account = account(state, headers).await?;
    if !state.uploads.allow(account.to_ascii_lowercase()) {
        return Err(fail(
            StatusCode::TOO_MANY_REQUESTS,
            "That's a lot of uploads. Wait a minute.",
        ));
    }
    if body.is_empty() {
        return Err(fail(StatusCode::BAD_REQUEST, "That file is empty."));
    }
    let name = file_name(&disposition_name(headers), sniff(&body));
    let mut id = [0u8; 16];
    getrandom::fill(&mut id).map_err(|_| internal())?;
    let id: String = id.iter().map(|b| format!("{b:02x}")).collect();
    let dir = cfg.dir.join(&id);
    let write = async {
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(dir.join(&name), &body).await
    };
    if let Err(e) = write.await {
        tracing::error!("upload to {}: {e}", dir.display());
        return Err(internal());
    }
    tracing::info!(%account, %id, %name, bytes = body.len(), "upload");
    state
        .config
        .origin
        .join(&format!("/uploads/{id}/{name}"))
        .map_err(|_| internal())
}

fn internal() -> (StatusCode, Json<chad_proto::chadd::ApiError>) {
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Couldn't save that file.",
    )
}

/// Wraps the static file service: media inline, everything else as an
/// attachment, and never anything a browser would run on our origin.
pub async fn headers(req: Request, next: Next) -> Response {
    let ext = req
        .uri()
        .path()
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase());
    let inline = INLINE.iter().find(|(e, _)| Some(*e) == ext.as_deref());
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    let (kind, disposition) = match inline {
        Some((_, kind)) => (*kind, "inline"),
        None => ("application/octet-stream", "attachment"),
    };
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static(disposition),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_by_magic() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("png"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(sniff(b"\0\0\0\x20ftypisom"), Some("mp4"));
        assert_eq!(sniff(b"<html><script>"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn names() {
        assert_eq!(percent_decode("My%20Cat%2"), "My Cat%2");
        assert_eq!(file_name("cat.PNG", Some("jpg")), "cat.jpg");
        assert_eq!(file_name("../../etc/passwd", None), "passwd");
        assert_eq!(file_name("C:\\x\\évil name.html", None), "_vil_name.html");
        assert_eq!(file_name(".htaccess", None), "htaccess");
        assert_eq!(file_name("", Some("png")), "file.png");
        assert_eq!(file_name("...", None), "file");
        assert_eq!(file_name("noext", Some("gif")), "noext.gif");
        assert_eq!(file_name("fake.PNG", None), "fake.PNG.bin");
    }

    #[tokio::test]
    async fn upload_then_serve() {
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config::parse(&format!(
            r#"
            origin = "https://chat.example.com"
            name = "x"
            [ergo]
            api = "{}"
            token = "t"
            [livekit]
            key = "devkey"
            secret = "0123456789abcdef0123456789abcdef"
            [uploads]
            dir = '{}'
            max_bytes = 64
            "#,
            crate::api::http_tests::fake_ergo().await,
            dir.path().display()
        ))
        .unwrap();
        let app = crate::routes::router(Arc::new(AppState::new(config).unwrap()));
        let upload = |auth: &str, body: &'static [u8]| {
            Request::post("/api/upload")
                .header("authorization", auth)
                .header(
                    "content-disposition",
                    r#"attachment; filename="My Cat.jpeg""#,
                )
                .body(Body::from(body))
                .unwrap()
        };
        let res = app
            .clone()
            .oneshot(upload("Basic eDp5", b"\x89PNG\r\n\x1a\nxx"))
            .await
            .unwrap();
        assert_eq!(res.status(), 401);
        // alice:alicepass
        let basic = "Basic QWxpY2U6YWxpY2VwYXNz";
        let res = app
            .clone()
            .oneshot(upload(basic, &[0u8; 65]))
            .await
            .unwrap();
        assert_eq!(res.status(), 413);
        let res = app
            .clone()
            .oneshot(upload(basic, b"\x89PNG\r\n\x1a\nxx"))
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
        let location = res.headers()["location"].to_str().unwrap().to_owned();
        assert!(location.starts_with("https://chat.example.com/uploads/"));
        assert!(location.ends_with("/My_Cat.png"), "{location}");

        let path = Url::parse(&location).unwrap().path().to_owned();
        let res = app
            .clone()
            .oneshot(Request::get(&path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(res.headers()["content-type"], "image/png");
        assert_eq!(res.headers()["x-content-type-options"], "nosniff");
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"\x89PNG\r\n\x1a\nxx");

        let res = app
            .clone()
            .oneshot(upload(basic, b"<script>"))
            .await
            .unwrap();
        let location = res.headers()["location"].to_str().unwrap().to_owned();
        let path = Url::parse(&location).unwrap().path().to_owned();
        let res = app
            .oneshot(Request::get(&path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.headers()["content-type"], "application/octet-stream");
        assert_eq!(res.headers()["content-disposition"], "attachment");
    }
}
