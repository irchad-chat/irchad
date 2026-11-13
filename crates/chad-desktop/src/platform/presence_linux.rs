//! Our processes from `/proc`, and MPRIS players on the session bus.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

use futures_util::{FutureExt, StreamExt};
use zbus::zvariant::OwnedValue;

use super::presence::Track;

/// Bounds on what a poll reads; other programs choose these sizes.
const MAX_PROCESSES: usize = 4096;
const MAX_CMDLINE: u64 = 4096;
/// Per player call, so one that hangs doesn't hide the others.
const PLAYER_WAIT: Duration = Duration::from_secs(1);
/// Players listed for the settings page; other programs pick how many there are.
const MAX_PLAYERS: usize = 32;
const PREFIX: &str = "org.mpris.MediaPlayer2.";

/// Each of our processes' executable, and its first argument when that looks
/// like a Windows program (Wine and Proton games run as `wine64-preloader`).
pub fn programs() -> Vec<String> {
    let Ok(me) = std::fs::metadata("/proc/self").map(|m| m.uid()) else {
        return Vec::new();
    };
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let ours = dir.flatten().filter(|e| {
        e.file_name()
            .to_str()
            .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
            && e.metadata().is_ok_and(|m| m.uid() == me)
    });
    let mut out = Vec::new();
    for e in ours.take(MAX_PROCESSES) {
        out.extend(paths(&e.path()));
    }
    out
}

/// A process's executable, and its first argument when that looks like a
/// Windows program. `pid` is its `/proc` directory.
pub(super) fn paths(pid: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(exe) = std::fs::read_link(pid.join("exe")) {
        out.push(exe.to_string_lossy().into_owned());
    }
    let mut cmd = Vec::new();
    let read = std::fs::File::open(pid.join("cmdline"))
        .and_then(|f| f.take(MAX_CMDLINE).read_to_end(&mut cmd));
    if read.is_ok() {
        let arg0 = cmd.split(|b| *b == 0).next().unwrap_or_default();
        let arg0 = String::from_utf8_lossy(arg0);
        if arg0.to_ascii_lowercase().ends_with(".exe") {
            out.push(arg0.into_owned());
        }
    }
    out
}

/// Signals queued between polls; a player spamming more just loses the oldest.
const MAX_QUEUED: usize = 64;

type Changes = futures_util::stream::Select<zbus::MessageStream, zbus::MessageStream>;

/// MPRIS players, over one session bus connection kept between polls, and
/// their change signals on it.
pub struct Media {
    conn: Option<zbus::Connection>,
    changes: Option<Changes>,
}

impl Media {
    pub fn new() -> Self {
        Self {
            conn: None,
            changes: None,
        }
    }

    /// Waits for a player to change track, state or position, or to come or
    /// go. Never returns without a bus.
    pub async fn changed(&mut self) {
        let Some(c) = self.changes.as_mut() else {
            return std::future::pending().await;
        };
        if c.next().await.is_none() {
            self.changes = None;
            return std::future::pending().await;
        }
        // A track change comes as a burst of signals: one poll for all of them.
        tokio::time::sleep(super::presence::SETTLE).await;
        while let Some(Some(_)) = c.next().now_or_never() {}
    }

    /// Every MPRIS player's id (see [`player_id`]), and the first one in
    /// `allowed` that's playing. Others aren't asked.
    pub async fn now_playing(&mut self, allowed: &[String]) -> (Vec<String>, Option<Track>) {
        if self.conn.is_none() {
            self.conn = zbus::Connection::session().await.ok();
            if let Some(conn) = &self.conn {
                self.changes = changes(conn).await.ok();
            }
        }
        let Some(conn) = self.conn.as_ref() else {
            return Default::default();
        };
        let names = match zbus::fdo::DBusProxy::new(conn).await {
            Ok(dbus) => dbus.list_names().await,
            Err(e) => Err(e.into()),
        };
        let Ok(names) = names else {
            self.conn = None; // reconnect next time
            self.changes = None;
            return Default::default();
        };
        let mut players: Vec<String> = Vec::new();
        let mut track = None;
        for name in &names {
            let Some(id) = player_id(name.as_str()) else {
                continue;
            };
            if !players.iter().any(|p| p == id) && players.len() < MAX_PLAYERS {
                players.push(id.to_owned());
            }
            if track.is_none() && allowed.iter().any(|a| a == id) {
                let found =
                    tokio::time::timeout(PLAYER_WAIT, playing(conn, name.as_str(), id)).await;
                track = found.ok().flatten();
            }
        }
        players.sort();
        (players, track)
    }
}

/// A player's bus name without its per-instance suffix, so the choice sticks
/// across restarts: `org.mpris.MediaPlayer2.firefox.instance_1_23` is `firefox`.
fn player_id(bus: &str) -> Option<&str> {
    bus.strip_prefix(PREFIX)?
        .split('.')
        .next()
        .filter(|id| !id.is_empty())
}

/// Every player's property changes and seeks (all on MPRIS's one object
/// path), and players starting or quitting.
async fn changes(conn: &zbus::Connection) -> zbus::Result<Changes> {
    let players = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(PATH)?
        .build();
    let owners = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")?
        .interface("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .arg0ns("org.mpris.MediaPlayer2")?
        .build();
    let a = zbus::MessageStream::for_match_rule(players, conn, Some(MAX_QUEUED)).await?;
    let b = zbus::MessageStream::for_match_rule(owners, conn, Some(MAX_QUEUED)).await?;
    Ok(futures_util::stream::select(a, b))
}

const PATH: &str = "/org/mpris/MediaPlayer2";

/// `name`'s track, if it's playing one. `id` is its [`player_id`].
async fn playing(conn: &zbus::Connection, name: &str, id: &str) -> Option<Track> {
    let p = proxy(conn, name, "org.mpris.MediaPlayer2.Player")
        .await
        .ok()?;
    let status: String = p.get_property("PlaybackStatus").await.ok()?;
    if status != "Playing" {
        return None;
    }
    let meta: HashMap<String, OwnedValue> = p.get_property("Metadata").await.ok()?;
    let get = |k: &str| meta.get(k).and_then(|v| v.try_clone().ok());
    let title = get("xesam:title").and_then(|v| String::try_from(v).ok())?;
    let artists = get("xesam:artist")
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .unwrap_or_default();
    let url = get("xesam:url").and_then(|v| String::try_from(v).ok());
    // Spec says x (i64), but some players send t (u64). Microseconds.
    let length = get("mpris:length").and_then(|v| {
        i64::try_from(v.try_clone().ok()?)
            .ok()
            .or_else(|| u64::try_from(v).ok().and_then(|l| i64::try_from(l).ok()))
    });
    let position: Option<i64> = p.get_property("Position").await.ok();
    let root = proxy(conn, name, "org.mpris.MediaPlayer2").await.ok();
    let identity: Option<String> = match root {
        Some(r) => r.get_property("Identity").await.ok(),
        None => None,
    };
    super::presence::Track::new(super::presence::Raw {
        title: &title,
        artists: &artists,
        id,
        identity: identity.as_deref(),
        url: url.as_deref(),
        position_ms: position.map(|p| p / 1000),
        length_ms: length.map(|l| l / 1000),
        video: false,
    })
}

async fn proxy<'a>(
    conn: &zbus::Connection,
    name: &'a str,
    interface: &'a str,
) -> zbus::Result<zbus::Proxy<'a>> {
    zbus::proxy::Builder::new(conn)
        .destination(name)?
        .path(PATH)?
        .interface(interface)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}

#[cfg(test)]
mod tests {
    #[test]
    fn player_ids() {
        use super::player_id;
        assert_eq!(player_id("org.mpris.MediaPlayer2.spotify"), Some("spotify"));
        assert_eq!(
            player_id("org.mpris.MediaPlayer2.firefox.instance_1_23"),
            Some("firefox")
        );
        assert_eq!(player_id("org.mpris.MediaPlayer2."), None);
        assert_eq!(player_id("org.freedesktop.Notifications"), None);
    }

    #[test]
    fn sees_ourselves() {
        let me = std::env::current_exe().unwrap();
        assert!(super::programs().iter().any(|p| *p == me.to_string_lossy()));
    }
}

#[cfg(test)]
mod live {
    /// Prints what MPRIS says is playing; needs a session bus.
    #[tokio::test]
    #[ignore = "needs a session bus"]
    async fn prints_now_playing() {
        let mut m = super::Media::new();
        let (players, _) = m.now_playing(&[]).await;
        let (_, track) = m.now_playing(&players).await;
        println!("players {players:?}, playing {track:?}");
    }

    /// A player's change signal wakes `changed`; needs a session bus.
    #[tokio::test]
    #[ignore = "needs a session bus"]
    async fn wakes_on_change() {
        let mut m = super::Media::new();
        m.now_playing(&[]).await;
        let other = zbus::Connection::session().await.unwrap();
        let props = std::collections::HashMap::<&str, zbus::zvariant::Value>::new();
        other
            .emit_signal(
                None::<&str>,
                super::PATH,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &("org.mpris.MediaPlayer2.Player", props, Vec::<&str>::new()),
            )
            .await
            .unwrap();
        let woke = tokio::time::timeout(std::time::Duration::from_secs(3), m.changed()).await;
        assert!(woke.is_ok());
    }
}
