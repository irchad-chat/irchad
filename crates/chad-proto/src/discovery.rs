//! `GET <origin>/.well-known/irchad.json`

use serde::{Deserialize, Serialize};
use url::Url;

pub const WELL_KNOWN_PATH: &str = "/.well-known/irchad.json";
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<Url>,
    pub irc: Irc,
    pub chadd: Chadd,
    pub livekit: Livekit,
    pub registration: Registration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Irc {
    pub websocket: Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chadd {
    pub api: Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Livekit {
    pub url: Url,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Registration {
    Open,
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_architecture_example() {
        let json = r#"{
          "version": 1,
          "name": "Chad Network",
          "icon": "https://chat.example.com/icon.png",
          "irc":     { "websocket": "wss://chat.example.com/irc" },
          "chadd":   { "api": "https://chat.example.com/api" },
          "livekit": { "url": "wss://chat.example.com/livekit" },
          "registration": "open"
        }"#;
        let d: Discovery = serde_json::from_str(json).unwrap();
        assert_eq!(d.version, VERSION);
        assert_eq!(d.registration, Registration::Open);
        assert_eq!(d.irc.websocket.as_str(), "wss://chat.example.com/irc");
    }
}
