//! Custom emoji: image files an admin put in `[emoji] dir`, listed for anyone
//! and served at `/emoji/<file>`. A message carries one as plain `:name:`.
//! The files in its `stickers` subdirectory are stickers, in the same list.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderValue, header},
    middleware::Next,
    response::Response,
};
use chad_proto::chadd::{Emoji, MAX_EMOJI, MAX_STICKERS, emoji_name};

use crate::routes::AppState;

/// They're drawn at text height; this is generous even for a GIF.
const MAX_BYTES: u64 = 1 << 20;
/// How long a listing is reused. There's no auth, so a scan per call is out.
const KEEP: Duration = Duration::from_secs(10);

/// The last listing and when it was made.
#[derive(Default)]
pub struct Listing(Mutex<Option<(Instant, Vec<Emoji>)>>);

/// The emoji in `dir`, then the stickers in `dir/stickers` whose name no
/// emoji has: `:name:` is one thing.
fn list_dir(dir: &Path) -> Vec<Emoji> {
    let mut emoji = list_files(dir, false, MAX_EMOJI);
    let mut stickers = list_files(&dir.join("stickers"), true, MAX_STICKERS);
    stickers.retain(|s| emoji.binary_search_by(|e| e.name.cmp(&s.name)).is_err());
    emoji.append(&mut stickers);
    emoji
}

fn list_files(dir: &Path, sticker: bool, max: usize) -> Vec<Emoji> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // No stickers is the usual case.
        if !sticker {
            tracing::warn!("emoji: can't read {}", dir.display());
        }
        return Vec::new();
    };
    let mut emoji: Vec<Emoji> = entries
        .flatten()
        .filter(|e| {
            e.metadata()
                .is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES)
        })
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|file| {
            Some(Emoji {
                name: emoji_name(&file)?.to_owned(),
                file,
                sticker,
            })
        })
        .collect();
    // `a.png` and `a.gif` are one name: the first file wins.
    emoji.sort_by(|a, b| (&a.name, &a.file).cmp(&(&b.name, &b.file)));
    emoji.dedup_by(|b, a| a.name == b.name);
    emoji.truncate(max);
    emoji
}

pub async fn list(State(state): State<Arc<AppState>>) -> Json<Vec<Emoji>> {
    let Some(dir) = state.config.emoji.as_ref().map(|e| e.dir.clone()) else {
        return Json(Vec::new());
    };
    let fresh = |l: &Option<(Instant, Vec<Emoji>)>| {
        l.as_ref()
            .filter(|(at, _)| at.elapsed() < KEEP)
            .map(|(_, list)| list.clone())
    };
    let kept = fresh(&state.emoji.0.lock().unwrap_or_else(|e| e.into_inner()));
    if let Some(list) = kept {
        return Json(list);
    }
    // A directory scan with a stat per file: not on the async workers.
    let list = tokio::task::spawn_blocking(move || list_dir(&dir))
        .await
        .unwrap_or_default();
    *state.emoji.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    Json(list)
}

/// Over `files::headers`: unlike an upload, a file here can be replaced.
pub async fn headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_named_images_only() {
        let dir = std::env::temp_dir().join(format!("chadd-emoji-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("dir.png")).unwrap();
        for f in [
            "bigint.png",
            "party_parrot.gif",
            "party_parrot.png",
            "Caps.png",
            "x.png",
            "has space.png",
            "notes.txt",
            "photo.jpeg",
            ".hidden.png",
        ] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        std::fs::create_dir_all(dir.join("stickers")).unwrap();
        for f in ["wave.png", "bigint.png"] {
            std::fs::write(dir.join("stickers").join(f), b"x").unwrap();
        }
        let files: Vec<_> = list_dir(&dir)
            .into_iter()
            .map(|e| (e.file, e.sticker))
            .collect();
        let f = |f: &str, s: bool| (f.to_owned(), s);
        assert_eq!(
            files,
            [
                f("bigint.png", false),
                f("party_parrot.gif", false),
                f("wave.png", true)
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
