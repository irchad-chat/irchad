use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    response::Html,
    routing::get,
};
use chad_proto::discovery::{self, Discovery};
use url::Url;

use crate::config::Config;

pub struct AppState {
    config: Config,
    discovery: Discovery,
}

impl AppState {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let discovery = config.discovery()?;
        Ok(Self { config, discovery })
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(discovery::WELL_KNOWN_PATH, get(well_known))
        .route("/invite", get(invite))
        .route("/invite/", get(invite))
        .route("/invite/{code}", get(invite_code))
        .with_state(state)
}

async fn well_known(State(state): State<Arc<AppState>>) -> Json<Discovery> {
    Json(state.discovery.clone())
}

async fn invite(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Html<String> {
    Html(invite_page(&state, None, query.get("channel")))
}

async fn invite_code(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Html<String> {
    Html(invite_page(&state, Some(&code), query.get("channel")))
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

fn invite_page(state: &AppState, code: Option<&str>, channel: Option<&String>) -> String {
    let join = join_url(&state.config.origin, code, channel);
    let name = escape(&state.discovery.name);
    let host = escape(state.config.origin.host_str().unwrap_or_default());
    let join = escape(join.as_str());
    let download = match &state.config.download_url {
        Some(u) => format!(
            r#"<a class="secondary" href="{}">Download IrChad</a>"#,
            escape(u.as_str())
        ),
        None => String::new(),
    };
    let channel = match channel.filter(|c| !c.is_empty()) {
        Some(c) => format!("<p>You'll land in <b>{}</b>.</p>", escape(c)),
        None => String::new(),
    };
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Join {name}</title>
<style>
  :root {{ color-scheme: light dark; --bg: #f2f3f5; --card: #fff; --fg: #1e1f22; --muted: #5c5e66; --accent: #5865f2; }}
  @media (prefers-color-scheme: dark) {{ :root {{ --bg: #1e1f22; --card: #2b2d31; --fg: #f2f3f5; --muted: #b5bac1; }} }}
  body {{ margin: 0; min-height: 100vh; display: grid; place-items: center; background: var(--bg); color: var(--fg); font: 16px/1.5 system-ui, sans-serif; }}
  main {{ background: var(--card); padding: 2rem; border-radius: 12px; max-width: 26rem; margin: 1rem; text-align: center; }}
  h1 {{ margin: 0 0 .25rem; font-size: 1.5rem; }}
  .host {{ color: var(--muted); margin: 0 0 1.5rem; }}
  a {{ display: block; padding: .75rem; border-radius: 6px; text-decoration: none; font-weight: 600; margin-top: .75rem; }}
  .primary {{ background: var(--accent); color: #fff; }}
  .secondary {{ color: var(--accent); }}
</style>
</head>
<body>
<main>
  <p>You've been invited to join</p>
  <h1>{name}</h1>
  <p class="host">{host}</p>
  {channel}
  <a class="primary" href="{join}">Open in IrChad</a>
  {download}
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> Router {
        let config = Config::parse(
            r#"
            origin = "https://chat.example.com"
            name = "Chad <Network>"
            "#,
        )
        .unwrap();
        router(Arc::new(AppState::new(config).unwrap()))
    }

    async fn get_body(uri: &str) -> (u16, String) {
        let res = app()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
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
}
