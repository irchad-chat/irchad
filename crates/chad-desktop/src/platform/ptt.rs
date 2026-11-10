//! Push-to-talk, the whisper key (D71) and the overlay's show/hide combo
//! (D74), working while another app has focus.
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

/// Modifiers a [`Combo`] needs held. Extra ones held too don't matter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Mods {
    pub const NONE: Self = Self {
        ctrl: false,
        shift: false,
        alt: false,
    };

    pub fn from_egui(m: egui::Modifiers) -> Self {
        Self {
            ctrl: m.ctrl,
            shift: m.shift,
            alt: m.alt,
        }
    }

    pub fn held(self, m: egui::Modifiers) -> bool {
        (!self.ctrl || m.ctrl) && (!self.shift || m.shift) && (!self.alt || m.alt)
    }

    /// "Ctrl+Shift+" for people.
    pub fn prefix(self) -> String {
        self.join(["Ctrl+", "Shift+", "Alt+"])
    }

    /// "CTRL+SHIFT+" for an XDG shortcut trigger.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn trigger(self) -> String {
        self.join(["CTRL+", "SHIFT+", "ALT+"])
    }

    fn join(self, names: [&str; 3]) -> String {
        [self.ctrl, self.shift, self.alt]
            .into_iter()
            .zip(names)
            .filter_map(|(on, n)| on.then_some(n))
            .collect()
    }
}

/// A key or side button, with modifiers held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Combo {
    pub mods: Mods,
    pub bind: PttBind,
}

impl Default for Combo {
    /// The overlay's.
    fn default() -> Self {
        Self {
            mods: Mods {
                ctrl: true,
                shift: true,
                alt: false,
            },
            bind: PttBind::Key(egui::Key::O),
        }
    }
}

impl From<PttBind> for Combo {
    fn from(bind: PttBind) -> Self {
        Self {
            mods: Mods::NONE,
            bind,
        }
    }
}

impl Combo {
    pub fn name(self) -> String {
        self.mods.prefix() + &self.bind.name()
    }

    pub fn down(self, i: &egui::InputState) -> bool {
        self.bind.down(i) && self.mods.held(i.modifiers)
    }
}

/// What a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Talk,
    /// Talk to the whisper list only.
    Whisper,
    /// Show or hide the game overlay, on press.
    Overlay,
}

impl Action {
    /// The portal's shortcut id.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn id(self) -> &'static str {
        match self {
            Self::Talk => "push-to-talk",
            Self::Whisper => "whisper",
            Self::Overlay => "toggle-overlay",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Talk => "Push to talk",
            Self::Whisper => "Whisper",
            Self::Overlay => "Show or hide the overlay",
        }
    }
}

/// Where key presses go: voice for talking, a flag for the overlay.
#[derive(Clone)]
pub struct Sink {
    pub voice: Voice,
    /// The overlay is toggled on.
    pub overlay: Arc<AtomicBool>,
    /// The overlay combo is held, so a key repeat doesn't toggle again.
    held: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Sink {
    pub fn new(
        voice: Voice,
        overlay: Arc<AtomicBool>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            voice,
            overlay,
            held: Arc::default(),
            wake: Arc::new(wake),
        }
    }

    pub fn set(&self, action: Action, down: bool) {
        match action {
            Action::Talk => self.voice.set_ptt(down),
            Action::Whisper => self.voice.set_whisper(down),
            Action::Overlay if !down => self.held.store(false, Relaxed),
            Action::Overlay => {
                if !self.held.swap(true, Relaxed) {
                    self.overlay.fetch_xor(true, Relaxed);
                    (self.wake)();
                }
            }
        }
    }
}

/// The keys to listen for, each with what it does.
pub type Binds = Vec<(Action, Combo)>;

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

/// A running push-to-talk, whisper and overlay key listener. Dropping it
/// stops listening.
pub struct Ptt {
    pub binds: Binds,
    pub sink: Sink,
    status: Arc<Mutex<PttStatus>>,
    stop: Arc<AtomicBool>,
}

impl Ptt {
    pub fn start(sink: Sink, binds: Binds) -> Self {
        let status = Arc::new(Mutex::new(PttStatus::Starting));
        let stop = Arc::new(AtomicBool::new(false));
        let report = {
            let status = status.clone();
            let wake = sink.wake.clone();
            move |s: PttStatus| {
                tracing::info!("global keys: {s:?}");
                *chad_core::lock(&status) = s;
                wake();
            }
        };
        listen(sink.clone(), binds.clone(), stop.clone(), Box::new(report));
        Self {
            binds,
            sink,
            status,
            stop,
        }
    }

    pub fn status(&self) -> PttStatus {
        chad_core::lock(&self.status).clone()
    }

    pub fn focused_only(&self) -> bool {
        matches!(self.status(), PttStatus::FocusedOnly(_))
    }
}

impl Drop for Ptt {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        for (action, _) in &self.binds {
            self.sink.set(*action, false);
        }
    }
}

pub type Report = Box<dyn Fn(PttStatus) + Send + Sync>;

#[cfg(target_os = "linux")]
use super::ptt_linux::listen;
#[cfg(windows)]
use super::ptt_windows::listen;

#[cfg(not(any(target_os = "linux", windows)))]
fn listen(_sink: Sink, _binds: Binds, _stop: Arc<AtomicBool>, report: Report) {
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

    #[test]
    fn combos() {
        let c = Combo::default();
        assert_eq!(c.name(), "Ctrl+Shift+O");
        assert_eq!(c.mods.trigger(), "CTRL+SHIFT+");
        assert!(c.mods.held(egui::Modifiers::CTRL | egui::Modifiers::SHIFT));
        assert!(!c.mods.held(egui::Modifiers::CTRL));
        assert!(Mods::NONE.held(egui::Modifiers::ALT));
        assert_eq!(Combo::from(PttBind::Mouse4).name(), "Mouse 4");
    }
}
