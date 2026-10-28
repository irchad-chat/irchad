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
    /// Where users download the client, shown on the invite page.
    pub download_url: Option<Url>,
    #[serde(default)]
    pub endpoints: Endpoints,
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
            },
            livekit: discovery::Livekit {
                url: or(&self.endpoints.livekit, &ws_origin, "/livekit")?,
            },
            registration: self.registration,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_endpoints_from_origin() {
        let c = Config::parse(
            r#"
            origin = "https://chat.example.com"
            name = "Chad Network"
            "#,
        )
        .unwrap();
        let d = c.discovery().unwrap();
        assert_eq!(d.irc.websocket.as_str(), "wss://chat.example.com/irc");
        assert_eq!(d.chadd.api.as_str(), "https://chat.example.com/api");
        assert_eq!(d.livekit.url.as_str(), "wss://chat.example.com/livekit");
        assert_eq!(d.registration, Registration::Open);
    }

    #[test]
    fn endpoint_overrides_win() {
        let c = Config::parse(
            r#"
            origin = "https://chat.example.com"
            name = "x"
            registration = "closed"
            [endpoints]
            livekit = "wss://lk.example.com"
            "#,
        )
        .unwrap();
        let d = c.discovery().unwrap();
        assert_eq!(d.livekit.url.as_str(), "wss://lk.example.com/");
        assert_eq!(d.registration, Registration::Closed);
    }
}
