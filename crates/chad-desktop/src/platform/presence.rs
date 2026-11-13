//! Polls for running programs (to spot games) and for what a media player is
//! playing, while sharing either is on. One impl of each per OS.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::presence::Games;

const EVERY: Duration = Duration::from_secs(20);
/// After a player's first change signal, how long to wait for the rest.
pub(super) const SETTLE: Duration = Duration::from_millis(500);
/// A track's start may move this much between polls without counting as a seek.
const DRIFT: u64 = 2;
/// The most of a player's own name that goes into "on <player>".
const MAX_PLAYER: usize = 24;

/// What the last poll saw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// A game from Discord's list that's running.
    pub game: Option<String>,
    /// What a media player is playing.
    pub music: Option<Track>,
    /// Every media player seen, by id, to pick from in settings.
    pub players: Vec<String>,
}

/// Polling; dropping it stops.
pub struct Poll {
    pub rx: watch::Receiver<Seen>,
    /// What it was started to look for: (games, music).
    pub looks_for: (bool, bool),
    /// The media players it shares from.
    pub players: Vec<String>,
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
        players: Vec<String>,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (tx, rx) = watch::channel(Seen::default());
        let allowed = players.clone();
        let task = tokio::spawn(async move {
            let mut media = Media::new();
            let mut game = None;
            let mut games_at = tokio::time::Instant::now();
            loop {
                // Players signal changes, so only games wait for the timer.
                if tokio::time::Instant::now() >= games_at {
                    games_at = tokio::time::Instant::now() + EVERY;
                    let list = games.borrow().clone().filter(|_| looks_for.0);
                    game = match list {
                        Some(list) => tokio::task::spawn_blocking(move || {
                            list.running(&programs()).map(str::to_owned)
                        })
                        .await
                        .unwrap_or_default(),
                        None => None,
                    };
                }
                let (players, mut music) = if looks_for.1 {
                    tokio::time::timeout(Duration::from_secs(5), media.now_playing(&allowed))
                        .await
                        .unwrap_or_default()
                } else {
                    Default::default()
                };
                if let (Some(new), Some(old)) = (&mut music, &tx.borrow().music) {
                    new.steady(old);
                }
                let seen = Seen {
                    game: game.clone(),
                    music,
                    players,
                };
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
                tokio::select! {
                    () = tokio::time::sleep_until(games_at) => {}
                    () = media.changed(), if looks_for.1 => {}
                }
            }
        });
        Self {
            rx,
            looks_for,
            players,
            task,
        }
    }
}

#[cfg(target_os = "linux")]
use super::presence_linux::{Media, programs};
#[cfg(windows)]
use super::presence_windows::{Media, programs};

#[cfg(not(any(target_os = "linux", windows)))]
fn programs() -> Vec<String> {
    Vec::new()
}

#[cfg(not(any(target_os = "linux", windows)))]
struct Media;

#[cfg(not(any(target_os = "linux", windows)))]
impl Media {
    fn new() -> Self {
        Self
    }

    #[allow(clippy::unused_async)]
    async fn now_playing(&mut self, _allowed: &[String]) -> (Vec<String>, Option<Track>) {
        Default::default()
    }

    async fn changed(&mut self) {
        std::future::pending().await
    }
}

/// What a media player is playing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// "Title by Artist".
    pub line: String,
    /// The player's own name ("Spotify"), for "on Spotify".
    pub player: Option<String>,
    /// Likely video (a browser, or the player says so): "Watching".
    pub video: bool,
    pub music: chad_core::Music,
}

/// What a player told us, before it's made safe to share.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub(super) struct Raw<'a> {
    pub title: &'a str,
    pub artists: &'a [String],
    /// Its id, as in settings.
    pub id: &'a str,
    /// Its own name for itself, if it has one.
    pub identity: Option<&'a str>,
    pub url: Option<&'a str>,
    pub position_ms: Option<i64>,
    pub length_ms: Option<i64>,
    /// The player said it's video.
    pub video: bool,
}

#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
impl Track {
    pub(super) fn new(r: Raw) -> Option<Self> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Self::at(r, now)
    }

    fn at(r: Raw, now: u64) -> Option<Self> {
        let line = track(r.title, r.artists)?;
        let url = r.url.and_then(chad_core::music::link);
        // Without a position there's nowhere to put the bar.
        let (start, length) = match (r.position_ms, r.length_ms) {
            (Some(p), Some(l)) if p >= 0 && l > 0 => {
                let l = u32::try_from(l / 1000).unwrap_or(u32::MAX);
                let start = now.saturating_sub(p.cast_unsigned() / 1000);
                (
                    start,
                    if l > chad_core::music::MAX_LENGTH {
                        0
                    } else {
                        l
                    },
                )
            }
            _ => (now, 0),
        };
        let player = r
            .identity
            .map(|i| chad_core::one_line(i, MAX_PLAYER))
            .filter(|i| !i.is_empty())
            .or_else(|| player_name(r.id));
        let music_site = url.as_ref().and_then(|u| u.host_str()).is_some_and(|h| {
            h.starts_with("music.")
                || ["spotify", "soundcloud", "bandcamp", "tidal", "deezer"]
                    .iter()
                    .any(|s| h.contains(s))
        });
        // shortcut: a browser without a music link counts as video, so Spotify's
        // web player in Firefox (no xesam:url) says "Watching"; good enough.
        let video = r.video || (browser(r.id) && !music_site);
        Some(Self {
            line,
            player,
            video,
            music: chad_core::Music { start, length, url },
        })
    }

    /// Keeps `old`'s start for the same track unless it moved more than a
    /// rounding wobble, so a poll doesn't republish for nothing.
    fn steady(&mut self, old: &Self) {
        let same = self.line == old.line && self.music.length == old.music.length;
        if same && self.music.start.abs_diff(old.music.start) <= DRIFT {
            self.music.start = old.music.start;
        }
    }
}

/// A web browser's player id (MPRIS bus name or Windows app id).
fn browser(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    [
        "firefox",
        "chromium",
        "chrome",
        "brave",
        "vivaldi",
        "opera",
        "edge",
        "librewolf",
        "zen",
        "floorp",
        "waterfox",
        "epiphany",
        "falkon",
        "qutebrowser",
        "plasma-browser",
        // Firefox's Windows app id.
        "308046b0af4a39cb",
    ]
    .iter()
    .any(|b| id.contains(b))
}

/// A readable name from a player id when it has none of its own:
/// `spotify` and `Spotify.exe` are "Spotify"; a bare hash is nothing.
fn player_name(id: &str) -> Option<String> {
    let id = id.rsplit('!').next().unwrap_or(id);
    let id = id.strip_suffix(".exe").unwrap_or(id);
    let id = chad_core::one_line(id, MAX_PLAYER);
    if id.is_empty() || id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut c = id.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect())
}

/// "Title by Artist, Artist" from a media player's text, made safe to share.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
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

    #[test]
    fn tracks() {
        use super::{Raw, Track};
        let a = vec!["A".to_owned()];
        let raw = |id, url, pos, len| Raw {
            title: "Song",
            artists: &a,
            id,
            identity: None,
            url,
            position_ms: pos,
            length_ms: len,
            video: false,
        };
        let t = Track::at(
            raw(
                "spotify",
                Some("https://open.spotify.com/track/x"),
                Some(30_500),
                Some(200_000),
            ),
            1000,
        )
        .unwrap();
        assert_eq!((t.music.start, t.music.length), (970, 200));
        assert_eq!(t.player.as_deref(), Some("Spotify"));
        assert!(!t.video && t.music.url.is_some());
        // Browsers watch, unless it's a music site; local files get no link.
        let t = Track::at(raw("firefox", Some("file:///x.mp3"), None, Some(1)), 1000).unwrap();
        assert!(t.video && t.music.url.is_none());
        assert_eq!((t.music.start, t.music.length), (1000, 0));
        let t = Track::at(
            raw(
                "chromium",
                Some("https://music.youtube.com/watch?v=1"),
                None,
                None,
            ),
            1,
        )
        .unwrap();
        assert!(!t.video);
        assert_eq!(super::player_name("308046B0AF4A39CB"), None);
        assert_eq!(
            super::player_name("Spotify.exe").as_deref(),
            Some("Spotify")
        );
        // A poll a second later is the same track, a seek isn't.
        let mut b = t.clone();
        b.music.start += 1;
        b.steady(&t);
        assert_eq!(b, t);
        b.music.start += 60;
        b.steady(&t);
        assert_ne!(b, t);
    }
}
