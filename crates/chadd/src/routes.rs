use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, header},
    response::Html,
    routing::{get, post},
};
use chad_proto::discovery::{self, Discovery};
use url::Url;

use crate::api;
use crate::config::Config;

pub struct AppState {
    pub config: Config,
    discovery: Discovery,
    pub http: reqwest::Client,
    /// Signs session tokens. Random per process: a restart signs everyone out
    /// of chadd (not IRC), and clients just log in again.
    pub session_secret: String,
    pub throttle: api::Throttle,
}

impl AppState {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let discovery = config.discovery()?;
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        Ok(Self {
            config,
            discovery,
            http,
            session_secret: key.iter().map(|b| format!("{b:02x}")).collect(),
            throttle: api::Throttle::default(),
        })
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(discovery::WELL_KNOWN_PATH, get(well_known))
        .route("/", get(invite))
        .route("/invite", get(invite))
        .route("/invite/", get(invite))
        .route("/invite/{code}", get(invite_code))
        .route("/api/login", post(api::login))
        .route("/api/voice/token", post(api::voice_token))
        .with_state(state)
}

async fn well_known(State(state): State<Arc<AppState>>) -> Json<Discovery> {
    Json(state.discovery.clone())
}

async fn invite(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Html<String> {
    Html(invite_page(
        &state,
        None,
        query.get("channel"),
        user_agent(&headers),
    ))
}

async fn invite_code(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(code): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Html<String> {
    Html(invite_page(
        &state,
        Some(&code),
        query.get("channel"),
        user_agent(&headers),
    ))
}

fn user_agent(headers: &HeaderMap) -> &str {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

/// Builds `irchad://join?origin=…[&code=…][&channel=…]`. Codes carry no
/// authority in v1 (D5); they and `channel` are only hints for the client.
fn join_url(origin: &Url, code: Option<&str>, channel: Option<&String>) -> Url {
    let mut url = Url::parse("irchad://join").expect("static URL");
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("origin", origin.origin().ascii_serialization().as_str());
        if let Some(code) = code.filter(|c| !c.is_empty()) {
            q.append_pair("code", code);
        }
        if let Some(channel) = channel.filter(|c| !c.is_empty()) {
            q.append_pair("channel", channel);
        }
    }
    url
}

/// Client builds: (name, nightly asset, icon, User-Agent needle). Android
/// comes before Linux because its User-Agent contains both.
const PLATFORMS: [(&str, Option<&str>, &str, &str); 3] = [
    ("Android", None, ANDROID, "Android"),
    (
        "Windows",
        Some("irchad-windows-x86_64.exe"),
        WINDOWS,
        "Windows",
    ),
    ("Linux", Some("irchad-linux-x86_64"), LINUX, "Linux"),
];

fn invite_page(
    state: &AppState,
    code: Option<&str>,
    channel: Option<&String>,
    user_agent: &str,
) -> String {
    let join = join_url(&state.config.origin, code, channel);
    let name = escape(&state.discovery.name);
    let host = escape(state.config.origin.host_str().unwrap_or_default());
    let join = escape(join.as_str());
    let channel = match channel.filter(|c| !c.is_empty()) {
        Some(c) => format!(
            r#"<p class="muted">You'll land in <b>{}</b>.</p>"#,
            escape(c)
        ),
        None => String::new(),
    };
    let base = state.config.downloads.as_str().trim_end_matches('/');
    let link = |label: &str, asset: Option<&str>, icon: &str, class: &str| match asset {
        Some(a) => format!(
            r#"<a class="{class}" href="{}/{a}" download>{icon}{label}</a>"#,
            escape(base)
        ),
        None => format!(r#"<span class="{class} soon">{icon}{label} · soon</span>"#),
    };
    // Unknown OS (macOS, bots): offer Linux, the first-class platform.
    let mine = PLATFORMS
        .iter()
        .position(|p| user_agent.contains(p.3))
        .unwrap_or(PLATFORMS.len() - 1);
    let (os, asset, icon, _) = PLATFORMS[mine];
    let download = link(
        &format!("Download for {os}"),
        asset,
        &format!("{DOWNLOAD}{icon}"),
        "btn secondary",
    );
    let others: String = PLATFORMS
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != mine)
        .map(|(_, (os, asset, icon, _))| link(os, *asset, icon, "chip"))
        .collect();
    let hint = if os == "Linux" {
        r#"<p class="hint">Then <code>chmod +x irchad-linux-x86_64</code> and run it.</p>"#
    } else {
        ""
    };
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Join {name}</title>
<style>
  :root {{ color-scheme: light dark; --bg: #f2f3f5; --glow: #5865f233; --card: #ffffffcc; --line: #0000001a; --fg: #1e1f22; --muted: #5c5e66; --accent: #5865f2; --accent-2: #8b5cf6; }}
  @media (prefers-color-scheme: dark) {{ :root {{ --bg: #111214; --glow: #5865f240; --card: #1e1f22cc; --line: #ffffff14; --fg: #f2f3f5; --muted: #949ba4; }} }}
  * {{ box-sizing: border-box; }}
  body {{ margin: 0; min-height: 100vh; display: grid; place-items: center; padding: 1rem;
    background: radial-gradient(60rem 30rem at 50% -10%, var(--glow), transparent), var(--bg);
    color: var(--fg); font: 16px/1.5 system-ui, sans-serif; }}
  main {{ width: 100%; max-width: 26rem; padding: 2.5rem 2rem 1.5rem; text-align: center;
    background: var(--card); border: 1px solid var(--line); border-radius: 20px;
    backdrop-filter: blur(12px); box-shadow: 0 20px 60px #0000002e; }}
  svg {{ width: 1.25em; height: 1.25em; fill: currentColor; flex: none; }}
  .logo {{ display: inline-grid; place-items: center; width: 4rem; height: 4rem; border-radius: 18px;
    color: #fff; background: linear-gradient(135deg, var(--accent), var(--accent-2)); font-size: 1.6rem; }}
  .kicker {{ margin: 1.25rem 0 0; color: var(--muted); font-size: .875rem; text-transform: uppercase; letter-spacing: .08em; }}
  h1 {{ margin: .25rem 0 0; font-size: 1.75rem; line-height: 1.2; overflow-wrap: anywhere; }}
  .muted {{ color: var(--muted); margin: .25rem 0 0; }}
  .actions {{ display: grid; gap: .625rem; margin-top: 1.75rem; }}
  .btn {{ display: flex; align-items: center; justify-content: center; gap: .5rem; padding: .875rem 1rem;
    border-radius: 12px; font-weight: 600; text-decoration: none; transition: transform .1s, filter .15s; }}
  .btn:hover {{ filter: brightness(1.1); }} .btn:active {{ transform: scale(.98); }}
  .primary {{ color: #fff; background: linear-gradient(135deg, var(--accent), var(--accent-2)); }}
  .secondary {{ color: var(--fg); border: 1px solid var(--line); background: transparent; }}
  .hint {{ margin: .5rem 0 0; color: var(--muted); font-size: .8rem; }}
  code {{ font-size: .95em; }}
  .others {{ display: flex; justify-content: center; flex-wrap: wrap; gap: .5rem; margin-top: 1.25rem; }}
  .chip {{ display: inline-flex; align-items: center; gap: .375rem; padding: .375rem .75rem; border-radius: 999px;
    border: 1px solid var(--line); color: var(--muted); font-size: .875rem; text-decoration: none; }}
  a.chip:hover {{ color: var(--fg); }}
  .soon {{ opacity: .55; cursor: default; }}
  footer {{ margin-top: 1.5rem; color: var(--muted); font-size: .75rem; }}
  a:focus-visible {{ outline: 2px solid var(--accent); outline-offset: 2px; }}
</style>
</head>
<body>
<main>
  <span class="logo">{CHATS}</span>
  <p class="kicker">You're invited to</p>
  <h1>{name}</h1>
  <p class="muted">{host}</p>
  {channel}
  <div class="actions">
    <a class="btn primary" href="{join}">{SIGN_IN}Click to Join {name}</a>
    {download}
  </div>
  {hint}
  <div class="others">{others}</div>
  <footer>Nightly builds of IrChad · rebuilt on every change</footer>
</main>
</body>
</html>
"#
    )
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

// Phosphor Icons (regular), MIT, from @phosphor-icons/core 2.1.1. Inlined so
// the page loads nothing from third parties.
const LINUX: &str = r#"<svg viewBox="0 0 256 256"><path d="M229,214.25A8,8,0,0,1,217.76,213C216.39,211.27,184,169.86,184,88A56,56,0,0,0,72,88c0,81.86-32.37,123.27-33.75,125a8,8,0,0,1-12.51-10c.15-.2,7.69-9.9,15.13-28.74C47.77,156.8,56,127.64,56,88a72,72,0,0,1,144,0c0,39.64,8.23,68.8,15.13,86.28,7.48,18.94,15.06,28.64,15.14,28.74A8,8,0,0,1,229,214.25ZM100,88a12,12,0,1,0,12,12A12,12,0,0,0,100,88Zm68,12a12,12,0,1,0-12,12A12,12,0,0,0,168,100ZM99.58,128.84a8,8,0,0,0-7.15,14.31l32,16a7.94,7.94,0,0,0,7.15,0l32-16a8,8,0,0,0-7.16-14.31L128,143.05ZM128,176a54.07,54.07,0,0,0-47,28.11,8,8,0,1,0,14,7.78,37.35,37.35,0,0,1,66,0,8,8,0,0,0,14-7.78A54.07,54.07,0,0,0,128,176Z"/></svg>"#;
const ANDROID: &str = r#"<svg viewBox="0 0 256 256"><path d="M176,148a12,12,0,1,1-12-12A12,12,0,0,1,176,148ZM92,136a12,12,0,1,0,12,12A12,12,0,0,0,92,136Zm148,24v24a16,16,0,0,1-16,16H32a16,16,0,0,1-16-16V161.13A113.38,113.38,0,0,1,51.4,78.72L26.34,53.66A8,8,0,0,1,37.66,42.34L63.82,68.5a111.43,111.43,0,0,1,128.55-.19l26-26a8,8,0,0,1,11.32,11.32L204.82,78.5c.75.71,1.5,1.43,2.24,2.17A111.25,111.25,0,0,1,240,160Zm-16,0a96,96,0,0,0-96-96h-.34C74.91,64.18,32,107.75,32,161.13V184H224Z"/></svg>"#;
const WINDOWS: &str = r#"<svg viewBox="0 0 256 256"><path d="M208,136H128a8,8,0,0,0-8,8v57.45a8,8,0,0,0,6.57,7.88l80,14.54A7.61,7.61,0,0,0,208,224a8,8,0,0,0,8-8V144A8,8,0,0,0,208,136Zm-8,70.41-64-11.63V152h64ZM96,136H32a8,8,0,0,0-8,8v40a8,8,0,0,0,6.57,7.87l64,11.64a8.54,8.54,0,0,0,1.43.13,8,8,0,0,0,8-8V144A8,8,0,0,0,96,136Zm-8,50.05-48-8.73V152H88ZM213.13,33.86a8,8,0,0,0-6.56-1.73l-80,14.55A8,8,0,0,0,120,54.55V112a8,8,0,0,0,8,8h80a8,8,0,0,0,8-8V40A8,8,0,0,0,213.13,33.86ZM200,104H136V61.22l64-11.63ZM101.13,54.22a8,8,0,0,0-6.56-1.73l-64,11.64A8,8,0,0,0,24,72v40a8,8,0,0,0,8,8H96a8,8,0,0,0,8-8V60.36A8,8,0,0,0,101.13,54.22ZM88,104H40V78.68L88,70Z"/></svg>"#;
const DOWNLOAD: &str = r#"<svg viewBox="0 0 256 256"><path d="M224,144v64a8,8,0,0,1-8,8H40a8,8,0,0,1-8-8V144a8,8,0,0,1,16,0v56H208V144a8,8,0,0,1,16,0Zm-101.66,5.66a8,8,0,0,0,11.32,0l40-40a8,8,0,0,0-11.32-11.32L136,124.69V32a8,8,0,0,0-16,0v92.69L93.66,98.34a8,8,0,0,0-11.32,11.32Z"/></svg>"#;
const SIGN_IN: &str = r#"<svg viewBox="0 0 256 256"><path d="M141.66,133.66l-40,40a8,8,0,0,1-11.32-11.32L116.69,136H24a8,8,0,0,1,0-16h92.69L90.34,93.66a8,8,0,0,1,11.32-11.32l40,40A8,8,0,0,1,141.66,133.66ZM200,32H136a8,8,0,0,0,0,16h56V208H136a8,8,0,0,0,0,16h64a8,8,0,0,0,8-8V40A8,8,0,0,0,200,32Z"/></svg>"#;
const CHATS: &str = r#"<svg viewBox="0 0 256 256"><path d="M232.07,186.76a80,80,0,0,0-62.5-114.17A80,80,0,1,0,23.93,138.76l-7.27,24.71a16,16,0,0,0,19.87,19.87l24.71-7.27a80.39,80.39,0,0,0,25.18,7.35,80,80,0,0,0,108.34,40.65l24.71,7.27a16,16,0,0,0,19.87-19.86ZM62,159.5a8.28,8.28,0,0,0-2.26.32L32,168l8.17-27.76a8,8,0,0,0-.63-6,64,64,0,1,1,26.26,26.26A8,8,0,0,0,62,159.5Zm153.79,28.73L224,216l-27.76-8.17a8,8,0,0,0-6,.63,64.05,64.05,0,0,1-85.87-24.88A79.93,79.93,0,0,0,174.7,89.71a64,64,0,0,1,41.75,92.48A8,8,0,0,0,215.82,188.23Z"/></svg>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> Router {
        let config = Config::parse(&format!(
            r#"
            origin = "https://chat.example.com"
            name = "Chad <Network>"
            {}
            "#,
            crate::config::tests::KEYS
        ))
        .unwrap();
        router(Arc::new(AppState::new(config).unwrap()))
    }

    async fn get_body(uri: &str) -> (u16, String) {
        get_as(uri, "").await
    }

    async fn get_as(uri: &str, user_agent: &str) -> (u16, String) {
        let req = Request::get(uri).header("user-agent", user_agent);
        let res = app()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status().as_u16();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn well_known_round_trips() {
        let (status, body) = get_body("/.well-known/irchad.json").await;
        assert_eq!(status, 200);
        let d: Discovery = serde_json::from_str(&body).unwrap();
        assert_eq!(d.name, "Chad <Network>");
    }

    #[tokio::test]
    async fn invite_page_links_to_scheme_handler() {
        let (status, body) = get_body("/invite/abc?channel=%23general").await;
        assert_eq!(status, 200);
        assert!(body.contains(
            "irchad://join?origin=https%3A%2F%2Fchat.example.com&amp;code=abc&amp;channel=%23general"
        ));
        assert!(body.contains("Chad &lt;Network&gt;"));

        let (status, body) = get_body("/invite").await;
        assert_eq!(status, 200);
        assert!(body.contains(r#"href="irchad://join?origin=https%3A%2F%2Fchat.example.com""#));
    }

    #[tokio::test]
    async fn landing_offers_the_visitors_build() {
        let nightly = "https://git.gentoo.party/sam/irchad-ng/releases/download/nightly";
        let (status, body) = get_as("/", "Mozilla/5.0 (X11; Linux x86_64)").await;
        assert_eq!(status, 200);
        assert!(body.contains("Click to Join Chad &lt;Network&gt;"));
        assert!(body.contains(&format!(
            r#"class="btn secondary" href="{nightly}/irchad-linux-x86_64""#
        )));

        let (_, body) = get_as("/", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)").await;
        assert!(body.contains(&format!(
            r#"class="btn secondary" href="{nightly}/irchad-windows-x86_64.exe""#
        )));

        let (_, body) = get_as("/", "Mozilla/5.0 (Linux; Android 14)").await;
        assert!(body.contains("Download for Android · soon"));
    }
}
