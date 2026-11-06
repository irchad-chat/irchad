//! Our processes from `/proc`, and MPRIS players on the session bus.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

use zbus::zvariant::OwnedValue;

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
        let pid = e.path();
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
    }
    out
}

/// MPRIS players, over one session bus connection kept between polls.
pub struct Media(Option<zbus::Connection>);

impl Media {
    pub fn new() -> Self {
        Self(None)
    }

    /// Every MPRIS player's id (see [`player_id`]), and the first one in
    /// `allowed` that's playing, as "Title by Artist". Others aren't asked.
    pub async fn now_playing(&mut self, allowed: &[String]) -> (Vec<String>, Option<String>) {
        if self.0.is_none() {
            self.0 = zbus::Connection::session().await.ok();
        }
        let Some(conn) = self.0.as_ref() else {
            return Default::default();
        };
        let names = match zbus::fdo::DBusProxy::new(conn).await {
            Ok(dbus) => dbus.list_names().await,
            Err(e) => Err(e.into()),
        };
        let Ok(names) = names else {
            self.0 = None; // reconnect next time
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
                let found = tokio::time::timeout(PLAYER_WAIT, playing(conn, name.as_str())).await;
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

/// `name`'s track, if it's playing one.
async fn playing(conn: &zbus::Connection, name: &str) -> Option<String> {
    let p = player(conn, name).await.ok()?;
    let status: String = p.get_property("PlaybackStatus").await.ok()?;
    if status != "Playing" {
        return None;
    }
    let meta: HashMap<String, OwnedValue> = p.get_property("Metadata").await.ok()?;
    let title = meta
        .get("xesam:title")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| String::try_from(v).ok())?;
    let artists = meta
        .get("xesam:artist")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .unwrap_or_default();
    super::presence::track(&title, &artists)
}

async fn player<'a>(conn: &zbus::Connection, name: &'a str) -> zbus::Result<zbus::Proxy<'a>> {
    zbus::proxy::Builder::new(conn)
        .destination(name)?
        .path("/org/mpris/MediaPlayer2")?
        .interface("org.mpris.MediaPlayer2.Player")?
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
        println!(
            "now playing: {:?}",
            super::Media::new().now_playing(&[]).await
        );
    }
}
