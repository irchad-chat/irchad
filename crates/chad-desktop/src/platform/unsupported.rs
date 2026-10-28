//! macOS and friends: every launch is its own instance and there's no tray.

use super::OsTx;

pub struct Instance;

pub async fn single_instance(_url: Option<String>, _tx: OsTx) -> Option<Instance> {
    Some(Instance)
}

pub fn register_scheme() {}

pub struct Tray;

impl Tray {
    pub async fn spawn(_tx: OsTx) -> Self {
        Self
    }

    pub fn set(&self, _unread: bool, _mentions: u32) {}
}
