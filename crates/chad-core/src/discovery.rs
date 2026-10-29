//! `GET <origin>/.well-known/irchad.json`, bounded per D14.

use std::time::Duration;

use chad_proto::discovery::{Discovery, VERSION, WELL_KNOWN_PATH};
use url::Url;

/// The real document is ~300 bytes.
const MAX_BODY: usize = 64 << 10;
const MAX_ICON: usize = 1 << 20;

pub async fn fetch(origin: &str) -> Result<Discovery, String> {
    let host = origin.trim_start_matches("https://");
    let unreachable = |e: reqwest::Error| {
        tracing::warn!("discovery {origin}: {e:?}");
        format!("Couldn't reach {host}. Check the address and your connection.")
    };
    let url = format!("{origin}{WELL_KNOWN_PATH}");
    match get(&url, MAX_BODY).await {
        Ok(Some(body)) => parse(&body).ok_or_else(|| not_irchad(host)),
        Ok(None) => Err(not_irchad(host)),
        Err(e) => Err(unreachable(e)),
    }
}

/// The discovery document's `icon`, at most 1 MiB. Decoding is the UI's job.
pub async fn fetch_icon(url: &Url) -> Result<Vec<u8>, String> {
    match get(url.as_str(), MAX_ICON).await {
        Ok(Some(body)) => Ok(body),
        Ok(None) => Err("icon too big".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// `Ok(None)` when the body is over `max`.
async fn get(url: &str, max: usize) -> Result<Option<Vec<u8>>, reqwest::Error> {
    let resp = client(url)?.get(url).send().await?.error_for_status()?;
    body(resp, max).await
}

/// HTTPS only, short timeouts, and `*.localhost` resolved by hand.
pub fn client(url: &str) -> Result<reqwest::Client, reqwest::Error> {
    let mut client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(3))
        .https_only(true);
    // glibc can't resolve `chat.localhost` without nss-myhostname (RFC 6761).
    if let Some(host) = Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        && chad_irc::ws::is_localhost(&host)
    {
        client = client.resolve(&host, ([127, 0, 0, 1], 0).into());
    }
    client.build()
}

/// The body, or `Ok(None)` past `max` bytes (D14).
pub async fn body(
    mut resp: reqwest::Response,
    max: usize,
) -> Result<Option<Vec<u8>>, reqwest::Error> {
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if body.len() + chunk.len() > max {
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

fn not_irchad(host: &str) -> String {
    format!("{host} isn't an IrChad server.")
}

pub fn parse(body: &[u8]) -> Option<Discovery> {
    let d: Discovery = serde_json::from_slice(body).ok()?;
    (d.version == VERSION && d.irc.websocket.scheme() == "wss").then_some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_v1_and_wss() {
        let doc = |v: u32, ws: &str| {
            format!(
                r#"{{"version":{v},"name":"N","irc":{{"websocket":"{ws}"}},"chadd":{{"api":"https://x/api"}},"livekit":{{"url":"wss://x/lk"}},"registration":"open"}}"#
            )
        };
        assert!(parse(doc(1, "wss://x/irc").as_bytes()).is_some());
        assert!(parse(doc(2, "wss://x/irc").as_bytes()).is_none());
        assert!(parse(doc(1, "ws://x/irc").as_bytes()).is_none());
        assert!(parse(b"<html>").is_none());
    }
}
