//! Push-to-talk that works while another app has focus.
//!
//! Picked at runtime: on Wayland, the GlobalShortcuts portal (the desktop owns
//! the binding); on X11, XInput2 raw events (see press and release without
//! grabbing); on Windows, the key state. Without any of those it only works
//! while IrChad is focused, and the settings say so.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use chad_media::Voice;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PttBind {
    Key(egui::Key),
    /// Side mouse buttons: back and forward.
    Mouse4,
    Mouse5,
}

impl Default for PttBind {
    fn default() -> Self {
        Self::Key(egui::Key::Backtick)
    }
}

impl PttBind {
    pub fn name(self) -> String {
        match self {
            Self::Key(k) => k.name().to_owned(),
            Self::Mouse4 => "Mouse 4".into(),
            Self::Mouse5 => "Mouse 5".into(),
        }
    }

    /// Whether egui sees it held, for the focused-only fallback.
    pub fn down(self, i: &egui::InputState) -> bool {
        match self {
            Self::Key(k) => i.key_down(k),
            Self::Mouse4 => i.pointer.button_down(egui::PointerButton::Extra1),
            Self::Mouse5 => i.pointer.button_down(egui::PointerButton::Extra2),
        }
    }
}

/// `(X11 keysym, XDG shortcut trigger name, Windows virtual key)`.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub fn keymap(key: egui::Key) -> Option<(u32, String, u16)> {
    use egui::Key as K;
    let name = key.name();
    let c = name.chars().next().unwrap_or_default();
    Some(match key {
        _ if name.len() == 1 && c.is_ascii_alphabetic() => {
            let lower = c.to_ascii_lowercase();
            (lower as u32, lower.to_string(), c as u16)
        }
        _ if name.len() == 1 && c.is_ascii_digit() => (c as u32, c.to_string(), c as u16),
        _ if name.starts_with('F')
            && name[1..]
                .parse::<u32>()
                .is_ok_and(|n| (1..=24).contains(&n)) =>
        {
            let n = name[1..].parse::<u32>().unwrap_or(1);
            (0xffbe + n - 1, name.to_owned(), 0x70 + n as u16 - 1)
        }
        K::Space => (0x20, "space".into(), 0x20),
        K::Backtick => (0x60, "grave".into(), 0xc0),
        K::Insert => (0xff63, "Insert".into(), 0x2d),
        K::Home => (0xff50, "Home".into(), 0x24),
        K::End => (0xff57, "End".into(), 0x23),
        K::PageUp => (0xff55, "Prior".into(), 0x21),
        K::PageDown => (0xff56, "Next".into(), 0x22),
        K::Pause => (0xff13, "Pause".into(), 0x13),
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PttStatus {
    Starting,
    /// Works everywhere; the text says how it's bound.
    Global(String),
    /// Only while IrChad is focused, and why.
    FocusedOnly(String),
}

/// A running push-to-talk listener. Dropping it stops listening.
pub struct Ptt {
    pub bind: PttBind,
    status: Arc<Mutex<PttStatus>>,
    stop: Arc<AtomicBool>,
    voice: Voice,
}

impl Ptt {
    pub fn start(voice: Voice, bind: PttBind, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let status = Arc::new(Mutex::new(PttStatus::Starting));
        let stop = Arc::new(AtomicBool::new(false));
        let report = {
            let status = status.clone();
            move |s: PttStatus| {
                tracing::info!("push-to-talk: {s:?}");
                *status.lock().unwrap_or_else(|e| e.into_inner()) = s;
                wake();
            }
        };
        listen(voice.clone(), bind, stop.clone(), Box::new(report));
        Self {
            bind,
            status,
            stop,
            voice,
        }
    }

    pub fn status(&self) -> PttStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn focused_only(&self) -> bool {
        matches!(self.status(), PttStatus::FocusedOnly(_))
    }
}

impl Drop for Ptt {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        self.voice.set_ptt(false);
    }
}

pub type Report = Box<dyn Fn(PttStatus) + Send + Sync>;

#[cfg(target_os = "linux")]
use super::ptt_linux::listen;
#[cfg(windows)]
use super::ptt_windows::listen;

#[cfg(not(any(target_os = "linux", windows)))]
fn listen(_voice: Voice, _bind: PttBind, _stop: Arc<AtomicBool>, report: Report) {
    report(PttStatus::FocusedOnly(
        "Global shortcuts aren't supported on this system.".into(),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_everywhere() {
        assert_eq!(keymap(egui::Key::A), Some((0x61, "a".into(), 0x41)));
        assert_eq!(keymap(egui::Key::Num7), Some((0x37, "7".into(), 0x37)));
        assert_eq!(keymap(egui::Key::F1), Some((0xffbe, "F1".into(), 0x70)));
        assert_eq!(keymap(egui::Key::F24), Some((0xffd5, "F24".into(), 0x87)));
        assert_eq!(keymap(egui::Key::Backtick).unwrap().1, "grave");
        assert_eq!(keymap(egui::Key::F25), None);
        assert_eq!(keymap(egui::Key::Enter), None);
    }
}
