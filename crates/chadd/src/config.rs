use std::{net::SocketAddr, path::Path};

use anyhow::Context;
use chad_proto::discovery::{self, Discovery, Registration};
use serde::Deserialize;
use url::Url;

/// chadd's TOML config. Every endpoint URL defaults to a path under `origin`,
/// matching the Caddy routing in `deploy/Caddyfile`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    /// Public origin of the network, e.g. `https://chat.example.com`.
    pub origin: Url,
    pub name: String,
    pub icon: Option<Url>,
    /// The channel every chad is put in, e.g. `#lobby`.
    pub home: Option<String>,
    /// Invite page colors; anything left out keeps the logo's palette.
    #[serde(default)]
    pub colors: Colors,
    #[serde(default = "default_registration")]
    pub registration: Registration,
    /// Where the landing page links client builds: `<downloads>/{latest,nightly}/<asset>`,
    /// asset names as in `.gitea/workflows/release.yml`.
    #[serde(default = "default_downloads")]
    pub downloads: Url,
    #[serde(default)]
    pub endpoints: Endpoints,
    pub ergo: Ergo,
    pub livekit: Livekit,
    /// Leave out to turn uploads off.
    pub uploads: Option<Uploads>,
    /// Leave out to turn the GIF picker off.
    pub gifs: Option<Gifs>,
    /// Leave out for no soundboard.
    pub sounds: Option<Sounds>,
    /// Leave out to keep nothing on disk; concurrent fetches are still shared.
    pub cache: Option<Cache>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Colors {
    #[serde(default)]
    pub light: Palette,
    #[serde(default)]
    pub dark: Palette,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    pub background: Option<Color>,
    /// The card behind the invite.
    pub surface: Option<Color>,
    pub text: Option<Color>,
    pub muted: Option<Color>,
    /// The join button is a gradient from `accent` to `accent_2`.
    pub accent: Option<Color>,
    pub accent_2: Option<Color>,
    /// Text on the join button.
    pub on_accent: Option<Color>,
}

impl Palette {
    /// `--name: value;` pairs, each unset field taking `defaults` (field order).
    pub fn css(&self, defaults: [&str; 7]) -> String {
        let fields = [
            ("bg", &self.background),
            ("card", &self.surface),
            ("fg", &self.text),
            ("muted", &self.muted),
            ("accent", &self.accent),
            ("accent-2", &self.accent_2),
            ("on-accent", &self.on_accent),
        ];
        fields
            .iter()
            .zip(defaults)
            .map(|((name, c), d)| format!("--{name}: {};", c.as_ref().map_or(d, |c| &c.0)))
            .collect()
    }
}

/// A `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` color; nothing else reaches the page's CSS.
#[derive(Debug, Deserialize)]
#[serde(try_from = "String")]
pub struct Color(String);

impl TryFrom<String> for Color {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        match s.strip_prefix('#') {
            Some(hex)
                if [3, 4, 6, 8].contains(&hex.len())
                    && hex.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                Ok(Self(s))
            }
            _ => Err(format!("{s:?} is not a #hex color")),
        }
    }
}

/// Link previews and `/api/media` images, kept on disk for a day. Must not be
/// under `uploads.dir`, which is served publicly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cache {
    pub dir: std::path::PathBuf,
    /// Least recently used items go past this.
    #[serde(default = "default_cache_bytes")]
    pub max_bytes: u64,
    /// Bigger items are served but not kept.
    #[serde(default = "default_cache_item")]
    pub max_item: u64,
}

fn default_cache_bytes() -> u64 {
    1 << 30
}

fn default_cache_item() -> u64 {
    8 << 20
}

/// The soundboard: every mp3, ogg, wav or flac file in `dir`, named after the
/// file. Admins add and remove files by hand; no restart needed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sounds {
    pub dir: std::path::PathBuf,
}

/// KLIPY's GIF API, searched on behalf of clients (D30, D31).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gifs {
    /// KLIPY's app key. Stays on the server.
    pub key: String,
    /// Requests go to `<api>/<key>/gifs/{search,trending}`.
    #[serde(default = "default_gifs_api")]
    pub api: Url,
    /// `off`, `low`, `medium` or `high`.
    #[serde(default = "default_content_filter")]
    pub content_filter: String,
}

fn default_gifs_api() -> Url {
    Url::parse("https://api.klipy.com/api/v1/").expect("static URL")
}

fn default_content_filter() -> String {
    "medium".into()
}

/// Files go to `<dir>/<id>/<name>` and are served at `<origin>/uploads/…`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uploads {
    pub dir: std::path::PathBuf,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: u64,
}

fn default_max_bytes() -> u64 {
    25 << 20
}

/// ergo's HTTP API, for checking passwords (`docs/notes/ergo-api.md`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ergo {
    /// e.g. `http://ergo:8089`. Every API call is fully privileged, so keep it
    /// off the public internet.
    pub api: Url,
    /// One of ergo's `api.bearer-tokens`.
    pub token: String,
}

/// Must match the LiveKit server's `keys`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Livekit {
    pub key: String,
    pub secret: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoints {
    pub irc_websocket: Option<Url>,
    pub chadd_api: Option<Url>,
    pub livekit: Option<Url>,
}

fn default_listen() -> SocketAddr {
    ([0, 0, 0, 0], 8080).into()
}

fn default_downloads() -> Url {
    Url::parse("https://downloads.gentoo.party/irchad/").expect("static URL")
}

fn default_registration() -> Registration {
    Registration::Open
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut config: Self =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        // Lets one config file serve whatever host/port the deployment uses.
        if let Ok(origin) = std::env::var("CHADD_ORIGIN") {
            config.origin = origin.parse().context("parsing CHADD_ORIGIN")?;
        }
        config.validate()?;
        Ok(config)
    }

    #[cfg(test)]
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let config: Self = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        let config = self;
        anyhow::ensure!(
            matches!(config.origin.scheme(), "https" | "http"),
            "origin must be an http(s) URL"
        );
        // Validate derived URLs up front rather than on first request.
        config.discovery()?;
        anyhow::ensure!(
            config.home.as_ref().is_none_or(|h| h.starts_with('#')
                && h.len() <= 64
                && !h.contains(|c: char| c == ',' || c.is_whitespace() || c.is_control())),
            "home must be a channel name like \"#lobby\""
        );
        anyhow::ensure!(
            config.livekit.secret.len() >= 32,
            "livekit.secret must be at least 32 characters"
        );
        if let Some(g) = &config.gifs {
            anyhow::ensure!(
                g.api.scheme() == "https" && !g.api.cannot_be_a_base(),
                "gifs.api must be an https URL"
            );
            anyhow::ensure!(
                matches!(g.content_filter.as_str(), "off" | "low" | "medium" | "high"),
                "gifs.content_filter must be off, low, medium or high"
            );
        }
        Ok(())
    }

    pub fn discovery(&self) -> anyhow::Result<Discovery> {
        let ws_origin = {
            let mut u = self.origin.clone();
            let scheme = if u.scheme() == "https" { "wss" } else { "ws" };
            u.set_scheme(scheme)
                .map_err(|()| anyhow::anyhow!("cannot derive websocket URL from origin"))?;
            u
        };
        let or = |o: &Option<Url>, base: &Url, path: &str| match o {
            Some(u) => Ok(u.clone()),
            None => base.join(path),
        };
        Ok(Discovery {
            version: discovery::VERSION,
            name: self.name.clone(),
            icon: self.icon.clone(),
            home: self.home.clone(),
            irc: discovery::Irc {
                websocket: or(&self.endpoints.irc_websocket, &ws_origin, "/irc")?,
            },
            chadd: discovery::Chadd {
                api: or(&self.endpoints.chadd_api, &self.origin, "/api")?,
                uploads: self.uploads.as_ref().map(|u| discovery::Uploads {
                    max_bytes: u.max_bytes,
                }),
                gifs: self.gifs.as_ref().map(|_| discovery::Gifs {
                    provider: "KLIPY".into(),
                }),
            },
            livekit: discovery::Livekit {
                url: or(&self.endpoints.livekit, &ws_origin, "/livekit")?,
            },
            registration: self.registration,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub const KEYS: &str = r#"
        [ergo]
        api = "http://ergo:8089"
        token = "t"
        [livekit]
        key = "devkey"
        secret = "0123456789abcdef0123456789abcdef"
    "#;

    #[test]
    fn derives_endpoints_from_origin() {
        let c = Config::parse(&format!(
            r#"
            origin = "https://chat.example.com"
            name = "Chad Network"
            {KEYS}
            "#
        ))
        .unwrap();
        let d = c.discovery().unwrap();
        assert_eq!(d.irc.websocket.as_str(), "wss://chat.example.com/irc");
        assert_eq!(d.chadd.api.as_str(), "https://chat.example.com/api");
        assert_eq!(d.livekit.url.as_str(), "wss://chat.example.com/livekit");
        assert_eq!(d.registration, Registration::Open);
    }

    #[test]
    fn endpoint_overrides_win() {
        let c = Config::parse(&format!(
            r#"
            origin = "https://chat.example.com"
            name = "x"
            registration = "closed"
            [endpoints]
            livekit = "wss://lk.example.com"
            {KEYS}
            "#
        ))
        .unwrap();
        let d = c.discovery().unwrap();
        assert_eq!(d.livekit.url.as_str(), "wss://lk.example.com/");
        assert_eq!(d.registration, Registration::Closed);
    }

    #[test]
    fn colors_are_hex_only() {
        let parse = |c: &str| {
            Config::parse(&format!(
                "origin = \"https://x.example\"\nname = \"x\"\n[colors.dark]\naccent = \"{c}\"\n{KEYS}"
            ))
        };
        let c = parse("#a6e3a1").unwrap();
        let css = c.colors.dark.css(["d"; 7]);
        assert!(css.contains("--accent: #a6e3a1;") && css.contains("--bg: d;"));
        assert!(parse("red").is_err());
        assert!(parse("#fff;}body{").is_err());
    }
}
