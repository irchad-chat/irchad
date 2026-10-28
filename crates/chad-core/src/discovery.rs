//! `GET <origin>/.well-known/irchad.json`, bounded per D14.

use std::time::Duration;

use chad_proto::discovery::{Discovery, VERSION, WELL_KNOWN_PATH};

/// The real document is ~300 bytes.
const MAX_BODY: usize = 64 << 10;

pub async fn fetch(origin: &str) -> Result<Discovery, String> {
    let host = origin.trim_start_matches("https://");
    let unreachable = |e: reqwest::Error| {
        tracing::warn!("discovery {origin}: {e:?}");
        format!("Couldn't reach {host}. Check the address and your connection.")
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(3))
        .https_only(true)
        .build()
        .map_err(unreachable)?;
    let mut resp = client
        .get(format!("{origin}{WELL_KNOWN_PATH}"))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(unreachable)?;
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(unreachable)? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(not_irchad(host));
        }
        body.extend_from_slice(&chunk);
    }
    parse(&body).ok_or_else(|| not_irchad(host))
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
