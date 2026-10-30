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
    #[serde(default = "default_registration")]
    pub registration: Registration,
    /// Where the landing page links client builds: `<downloads>/<asset>`,
    /// asset names as in `.gitea/workflows/nightly.yml`.
    #[serde(default = "default_downloads")]
    pub downloads: Url,
    #[serde(default)]
    pub endpoints: Endpoints,
    pub ergo: Ergo,
    pub livekit: Livekit,
    /// Leave out to turn uploads off.
    pub uploads: Option<Uploads>,
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
    Url::parse("https://git.gentoo.party/sam/irchad-ng/releases/download/nightly/")
        .expect("static URL")
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
            config.livekit.secret.len() >= 32,
            "livekit.secret must be at least 32 characters"
        );
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
            irc: discovery::Irc {
                websocket: or(&self.endpoints.irc_websocket, &ws_origin, "/irc")?,
            },
            chadd: discovery::Chadd {
                api: or(&self.endpoints.chadd_api, &self.origin, "/api")?,
                uploads: self.uploads.as_ref().map(|u| discovery::Uploads {
                    max_bytes: u.max_bytes,
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
}
