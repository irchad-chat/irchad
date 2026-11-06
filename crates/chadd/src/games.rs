//! Discord's detectable games, downloaded by chadd (`discord_games`) so clients
//! can name the game behind a Rich Presence update without asking Discord.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use chad_proto::{MAX_GAMES_BYTES, chadd::Game, one_line};
use serde::Deserialize;

use crate::api::{self, fail};
use crate::routes::AppState;
use crate::{fetch, lock};

const URL: &str = "https://discord.com/api/v9/applications/detectable";
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const RETRY: Duration = Duration::from_secs(60 * 60);
/// The upstream list is a few MB; this leaves room to grow without trusting it.
const MAX_BYTES: usize = 64 << 20;
const MAX_GAMES: usize = 200_000;
const MAX_NAME: usize = 100;

/// The trimmed list as JSON, ready to serve.
#[derive(Default)]
pub struct Games(Mutex<Option<Bytes>>);

/// Downloads the list now and daily after, keeping the last good copy.
pub async fn refresh(state: Arc<AppState>) {
    loop {
        let wait = match download(&state.http).await {
            Ok(json) => {
                *lock(&state.games.0) = Some(json);
                EVERY
            }
            Err(e) => {
                tracing::warn!("discord games: {e:#}");
                RETRY
            }
        };
        tokio::time::sleep(wait).await;
    }
}

async fn download(http: &reqwest::Client) -> anyhow::Result<Bytes> {
    let mut res = http
        .get(URL)
        .timeout(Duration::from_secs(120))
        .send()
        .await?
        .error_for_status()?;
    let body = fetch::capped(&mut res, MAX_BYTES + 1).await?;
    anyhow::ensure!(body.len() <= MAX_BYTES, "list is over {MAX_BYTES} bytes");
    let games = trim(&body)?;
    tracing::info!(games = games.len(), "discord games downloaded");
    Ok(fit(games)?.into())
}

/// The list as JSON, cut down until clients will take it.
fn fit(mut games: Vec<Game>) -> serde_json::Result<Vec<u8>> {
    loop {
        let json = serde_json::to_vec(&games)?;
        if json.len() <= MAX_GAMES_BYTES {
            return Ok(json);
        }
        tracing::warn!(
            bytes = json.len(),
            "discord games: list too big, cutting it"
        );
        games.truncate(games.len() * 9 / 10);
    }
}

/// Keeps `id` and `name` of each well-formed entry.
fn trim(body: &[u8]) -> anyhow::Result<Vec<Game>> {
    #[derive(Deserialize)]
    struct Raw {
        id: serde_json::Value,
        name: serde_json::Value,
    }
    let raw: Vec<Raw> = serde_json::from_slice(body)?;
    Ok(raw
        .into_iter()
        .filter_map(|r| {
            let id = r.id.as_str()?;
            let name = one_line(r.name.as_str()?, MAX_NAME);
            let ok = (1..=20).contains(&id.len())
                && id.bytes().all(|b| b.is_ascii_digit())
                && !name.is_empty();
            ok.then(|| Game {
                id: id.to_owned(),
                name,
            })
        })
        .take(MAX_GAMES)
        .collect())
}

pub async fn list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let account = match api::session_account(&state, &headers) {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };
    if !state.games_throttle.allow(account.to_ascii_lowercase()) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "Slow down a little.").into_response();
    }
    match lock(&state.games.0).clone() {
        Some(json) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
        None => fail(StatusCode::NOT_FOUND, "No game list here.").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_sane_entries() {
        let body = br#"[
            {"id": "123", "name": " Celeste ", "executables": [{"name": "celeste.exe", "os": "win32"}]},
            {"id": 5, "name": "numeric id"},
            {"id": "12a", "name": "bad id"},
            {"id": "9", "name": "\u0007"},
            {"id": "10", "name": "Tab\there"}
        ]"#;
        let games = trim(body).unwrap();
        assert_eq!(
            games,
            [
                Game {
                    id: "123".into(),
                    name: "Celeste".into()
                },
                Game {
                    id: "10".into(),
                    name: "Tabhere".into()
                },
            ]
        );
        assert!(trim(b"{}").is_err());
    }

    #[test]
    fn fits_the_client_cap() {
        let name = "x".repeat(MAX_NAME);
        let games: Vec<Game> = (0..MAX_GAMES_BYTES / MAX_NAME + 10)
            .map(|i| Game {
                id: i.to_string(),
                name: name.clone(),
            })
            .collect();
        let json = fit(games).unwrap();
        assert!(json.len() <= MAX_GAMES_BYTES);
        assert!(serde_json::from_slice::<Vec<Game>>(&json).unwrap().len() > 1000);
    }
}
