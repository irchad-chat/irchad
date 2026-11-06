//! Polls for running programs (to spot games) and for what a media player is
//! playing, while sharing either is on. One impl of each per OS.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::presence::Games;

const EVERY: Duration = Duration::from_secs(20);

/// What the last poll saw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// A game from Discord's list that's running.
    pub game: Option<String>,
    /// "Title by Artist" from a playing media player.
    pub music: Option<String>,
}

/// Polling; dropping it stops.
pub struct Poll {
    pub rx: watch::Receiver<Seen>,
    /// What it was started to look for: (games, music).
    pub looks_for: (bool, bool),
    task: JoinHandle<()>,
}

impl Drop for Poll {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Poll {
    /// Matches running programs against `games` once it has a list.
    pub fn start(
        games: watch::Receiver<Option<Arc<Games>>>,
        looks_for: (bool, bool),
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (tx, rx) = watch::channel(Seen::default());
        let task = tokio::spawn(async move {
            let mut media = Media::new();
            loop {
                let list = games.borrow().clone().filter(|_| looks_for.0);
                let game = match list {
                    Some(list) => tokio::task::spawn_blocking(move || {
                        list.running(&programs()).map(str::to_owned)
                    })
                    .await
                    .unwrap_or_default(),
                    None => None,
                };
                let music = if looks_for.1 {
                    tokio::time::timeout(Duration::from_secs(5), media.now_playing())
                        .await
                        .ok()
                        .flatten()
                } else {
                    None
                };
                let seen = Seen { game, music };
                let changed = tx.send_if_modified(|cur| {
                    let changed = *cur != seen;
                    if changed {
                        *cur = seen;
                    }
                    changed
                });
                if changed {
                    wake();
                }
                tokio::time::sleep(EVERY).await;
            }
        });
        Self {
            rx,
            looks_for,
            task,
        }
    }
}

#[cfg(target_os = "linux")]
use super::presence_linux::{Media, programs};
#[cfg(windows)]
use super::presence_windows::programs;

#[cfg(not(any(target_os = "linux", windows)))]
fn programs() -> Vec<String> {
    Vec::new()
}

// shortcut: no Windows media source (GlobalSystemMediaTransportControls needs
// the `windows` crate's WinRT bindings); add one if Windows users ask.
#[cfg(not(target_os = "linux"))]
struct Media;

#[cfg(not(target_os = "linux"))]
impl Media {
    fn new() -> Self {
        Self
    }

    #[allow(clippy::unused_async)]
    async fn now_playing(&mut self) -> Option<String> {
        None
    }
}

/// "Title by Artist, Artist" from a media player's text, made safe to share.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn track(title: &str, artists: &[String]) -> Option<String> {
    let title = chad_core::one_line(title, 80);
    let by = chad_core::one_line(&artists.join(", "), 60);
    match (title.is_empty(), by.is_empty()) {
        (true, _) => None,
        (false, true) => Some(title),
        (false, false) => Some(format!("{title} by {by}")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn track_line() {
        let t = |title: &str, a: &[&str]| {
            let a: Vec<String> = a.iter().map(|s| (*s).to_owned()).collect();
            super::track(title, &a)
        };
        assert_eq!(t("Song", &["A", "B"]).as_deref(), Some("Song by A, B"));
        assert_eq!(t("Song\n", &[]).as_deref(), Some("Song"));
        assert_eq!(t(" ", &["A"]), None);
    }
}
