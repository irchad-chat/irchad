//! The soundboard: files an admin put in `[sounds] dir`, listed and served to
//! signed-in users. Clients play them into voice themselves.

use std::path::Path;
use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use chad_proto::chadd::Sound;

use crate::api::{self, ApiResult, fail};
use crate::routes::AppState;

const EXTENSIONS: [&str; 4] = ["mp3", "ogg", "wav", "flac"];
const MAX_SOUNDS: usize = 200;
/// Clients play the first 10 s; this is plenty for that at any sane bitrate.
pub const MAX_BYTES: u64 = 4 << 20;

/// A plain file name in the directory, with a sound extension.
fn valid(file: &str) -> bool {
    file.len() <= 100
        && !file.starts_with('.')
        && file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
        && file
            .rsplit_once('.')
            .is_some_and(|(_, e)| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

fn list_dir(dir: &Path) -> Vec<Sound> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        tracing::warn!("sounds: can't read {}", dir.display());
        return Vec::new();
    };
    let mut sounds: Vec<Sound> = entries
        .flatten()
        .filter(|e| {
            e.metadata()
                .is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES)
        })
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|f| valid(f))
        .map(|file| Sound {
            name: file
                .rsplit_once('.')
                .map_or(file.as_str(), |(stem, _)| stem)
                .replace(['_', '-'], " "),
            file,
        })
        .collect();
    sounds.sort_by_cached_key(|a| a.name.to_lowercase());
    sounds.truncate(MAX_SOUNDS);
    sounds
}

pub async fn list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Vec<Sound>> {
    api::session_account(&state, &headers)?;
    let Some(dir) = state.config.sounds.as_ref().map(|s| s.dir.clone()) else {
        return Ok(Json(Vec::new()));
    };
    // A directory scan with a stat per file: not on the async workers.
    let sounds = tokio::task::spawn_blocking(move || list_dir(&dir))
        .await
        .unwrap_or_default();
    Ok(Json(sounds))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<Sound>,
) -> Response {
    let account = match api::session_account(&state, &headers) {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };
    if !state.fetches.allow(account.to_ascii_lowercase()) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "Slow down a little.").into_response();
    }
    let gone = || fail(StatusCode::NOT_FOUND, "That sound is gone.").into_response();
    let Some(cfg) = state.config.sounds.as_ref().filter(|_| valid(&req.file)) else {
        return gone();
    };
    let path = cfg.dir.join(&req.file);
    match tokio::fs::metadata(&path).await {
        Ok(m) if m.is_file() && m.len() <= MAX_BYTES => {}
        _ => return gone(),
    }
    match tokio::fs::read(&path).await {
        Ok(b) => (
            [(header::CONTENT_TYPE, "application/octet-stream")],
            Bytes::from(b),
        )
            .into_response(),
        Err(_) => gone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_sound_files() {
        assert!(valid("air_horn.mp3"));
        assert!(valid("Sad Trombone.OGG"));
        for bad in [
            "../x.mp3",
            ".hidden.mp3",
            "a/b.mp3",
            "x.exe",
            "noext",
            "a\\b.wav",
        ] {
            assert!(!valid(bad), "{bad}");
        }
    }

    #[test]
    fn lists_sorted_and_named() {
        let dir = std::env::temp_dir().join(format!("chadd-sounds-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub.mp3")).unwrap();
        std::fs::write(dir.join("b_bruh.mp3"), b"x").unwrap();
        std::fs::write(dir.join("Air-horn.wav"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let names: Vec<_> = list_dir(&dir).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["Air horn", "b bruh"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
