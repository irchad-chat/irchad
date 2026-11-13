//! The song or video behind a "Listening to"/"Watching" activity: metadata
//! `chad/music`, as `<start> <length> [url]`. `start` is the unix second the
//! track would have begun to be where it is now (a seek moves it), `length` its
//! seconds (0 when the player doesn't say), and `url` a link to it on a known
//! music or video site.

use url::Url;

/// The longest track we draw progress for; past that it's a stream or a lie.
pub const MAX_LENGTH: u32 = 24 * 60 * 60;
const MAX_URL: usize = 512;
/// Sites a song link may point at; anything else isn't shared or shown.
const HOSTS: &[&str] = &[
    "open.spotify.com",
    "music.youtube.com",
    "www.youtube.com",
    "youtube.com",
    "m.youtube.com",
    "youtu.be",
    "soundcloud.com",
    "music.apple.com",
    "tidal.com",
    "listen.tidal.com",
    "www.deezer.com",
    "bandcamp.com",
    "www.twitch.tv",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Music {
    pub start: u64,
    pub length: u32,
    pub url: Option<Url>,
}

impl Music {
    /// A `chad/music` value from the server: hostile, so every part is checked.
    pub fn parse(v: &str) -> Option<Self> {
        let mut it = v.split(' ');
        let start = it.next()?.parse().ok()?;
        let length = it.next()?.parse().ok().filter(|l| *l <= MAX_LENGTH)?;
        let url = it.next().and_then(link);
        it.next().is_none().then_some(Self { start, length, url })
    }

    pub fn encode(&self) -> String {
        match &self.url {
            Some(u) => format!("{} {} {u}", self.start, self.length),
            None => format!("{} {}", self.start, self.length),
        }
    }

    /// Seconds in and the track's length at unix second `now`, when it has one.
    pub fn progress(&self, now: u64) -> Option<(u32, u32)> {
        let at = now.saturating_sub(self.start).min(u64::from(self.length));
        (self.length > 0).then_some((at as u32, self.length))
    }
}

/// `s` as a song link, if it's https on one of [`HOSTS`] (or a bandcamp artist).
pub fn link(s: &str) -> Option<Url> {
    if s.len() > MAX_URL {
        return None;
    }
    let u = Url::parse(s).ok()?;
    let host = u.host_str()?;
    let known = HOSTS.contains(&host) || host.ends_with(".bandcamp.com");
    (u.scheme() == "https" && u.port().is_none() && u.username().is_empty() && known).then_some(u)
}

/// `m:ss`, or `h:mm:ss` from an hour.
pub fn clock(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_checks() {
        let m = Music::parse("1000 200 https://open.spotify.com/track/x").unwrap();
        assert_eq!((m.start, m.length), (1000, 200));
        assert_eq!(Music::parse(&m.encode()), Some(m.clone()));
        assert_eq!(m.progress(1050), Some((50, 200)));
        assert_eq!(m.progress(999_999), Some((200, 200)));
        assert_eq!(m.progress(10), Some((0, 200)));
        assert_eq!(Music::parse("5 0").unwrap().progress(9), None);
        // Unknown hosts, http and junk lose the link, not the progress.
        for bad in [
            "http://open.spotify.com/x",
            "https://evil.example/x",
            "https://open.spotify.com:444/x",
            "https://u@open.spotify.com/x",
            "file:///etc/passwd",
        ] {
            assert_eq!(Music::parse(&format!("1 2 {bad}")).unwrap().url, None);
        }
        assert!(link("https://artist.bandcamp.com/track/y").is_some());
        assert!(link("https://notbandcamp.com/").is_none());
        for bad in ["", "x 1", "1", "-1 2", "1 99999999", "1 2 u extra", "1  2"] {
            assert_eq!(Music::parse(bad), None, "{bad:?}");
        }
        assert_eq!(clock(65), "1:05");
        assert_eq!(clock(3725), "1:02:05");
    }
}
