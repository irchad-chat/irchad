//! Voice: one audio engine for the whole app, and at most one LiveKit call.
//!
//! The engine owns audio I/O (PipeWire on Linux, cpal/WASAPI elsewhere). It
//! mixes remote participants itself and runs the mic through libwebrtc's APM
//! with that mix as the echo canceller's far end (`docs/notes/livekit-sdk.md`).
//! The mic is only open while in a call or testing the input.

mod engine;
mod platform;
mod room;
mod screen;

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

pub use room::Call;
pub use screen::{
    AudioApp, ContentHint, Preset, ScreenFrame, ScreenSettings, ShareSource, SourceId, Target,
    Targets,
};

/// Everything runs at 48 kHz mono, 10 ms frames.
pub(crate) const RATE: u32 = 48_000;
pub(crate) const FRAME: usize = (RATE / 100) as usize;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// A [`Device::id`]; `None` follows the system default.
    pub input: Option<String>,
    pub output: Option<String>,
    pub mode: InputMode,
    /// Voice activity opens the mic above this level (dBFS).
    pub vad_threshold_db: f32,
    /// Push-to-talk keeps sending this long after the key is released.
    pub ptt_release_ms: u32,
    /// RNNoise before the APM's own noise suppression.
    pub rnnoise: bool,
    /// Mic volume, 0.0..=2.0, after processing: what others hear.
    pub input_gain: f32,
    pub bitrate_kbps: u32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            mode: InputMode::VoiceActivity,
            vad_threshold_db: -50.0,
            ptt_release_ms: 200,
            rnnoise: false,
            input_gain: 1.0,
            bitrate_kbps: 64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputMode {
    VoiceActivity,
    PushToTalk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// PipeWire `node.name`, or the device name elsewhere.
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Devices {
    pub inputs: Vec<Device>,
    pub outputs: Vec<Device>,
}

/// The current call, for the UI.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallState {
    pub room: String,
    pub reconnecting: bool,
    /// Ourselves first.
    pub participants: Vec<Participant>,
    /// Round trip to the SFU.
    pub rtt_ms: Option<u32>,
    pub quality: Quality,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Participant {
    /// The ergo account (chadd mints tokens with identity = account).
    pub identity: String,
    pub speaking: bool,
    pub muted: bool,
    pub local: bool,
    /// Has a screen share up.
    pub sharing: bool,
    /// How many others watch that share.
    pub watchers: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Quality {
    #[default]
    Unknown,
    Excellent,
    Good,
    Poor,
    Lost,
}

/// Cheap to clone; every clone is the same engine.
#[derive(Clone)]
pub struct Voice {
    pub(crate) shared: Arc<engine::Shared>,
    backend: Arc<dyn platform::Backend>,
    /// Why the engine runs: (calls, testing).
    users: Arc<Mutex<(u32, bool)>>,
}

impl Voice {
    /// `notify` is called when something the UI shows changes (devices, call state).
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(engine::Shared::new(notify));
        let backend = platform::backend(shared.clone());
        Self {
            shared,
            backend,
            users: Arc::default(),
        }
    }

    pub fn settings(&self) -> AudioSettings {
        self.shared.settings()
    }

    pub fn set_settings(&self, s: AudioSettings) {
        let old = self.shared.settings();
        let devices_changed = (&old.input, &old.output) != (&s.input, &s.output);
        self.shared.set_settings(s.clone());
        if devices_changed && self.running() {
            self.backend.start(s.input, s.output);
        }
    }

    pub fn devices(&self) -> Devices {
        self.backend.devices()
    }

    /// Mic level after processing, in dBFS (−100 when silent or closed).
    pub fn input_level(&self) -> f32 {
        self.shared.level()
    }

    /// Whether the mic is being sent (voice activity or push-to-talk).
    pub fn transmitting(&self) -> bool {
        self.shared.transmitting()
    }

    pub fn set_ptt(&self, down: bool) {
        self.shared.set_ptt(down);
    }

    pub fn muted(&self) -> bool {
        self.shared.muted()
    }

    pub fn deafened(&self) -> bool {
        self.shared.deafened()
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.set_muted(muted);
    }

    /// Deafening also mutes; undeafening restores the mute from before.
    pub fn set_deafened(&self, deafened: bool) {
        self.shared.set_deafened(deafened);
    }

    /// Per participant, 0.0..=2.0.
    pub fn volume(&self, identity: &str) -> f32 {
        self.shared.volume(identity, false)
    }

    pub fn set_volume(&self, identity: &str, gain: f32) {
        self.shared
            .set_volume(identity, false, gain.clamp(0.0, 2.0));
    }

    /// A participant's screen share audio, apart from their voice; 0.0..=2.0.
    pub fn screen_volume(&self, identity: &str) -> f32 {
        self.shared.volume(identity, true)
    }

    pub fn set_screen_volume(&self, identity: &str, gain: f32) {
        self.shared.set_volume(identity, true, gain.clamp(0.0, 2.0));
    }

    /// Plays the processed mic back, to hear what others would.
    pub fn set_testing(&self, on: bool) {
        self.shared.set_loopback(on);
        self.update(|u| u.1 = on);
    }

    pub fn testing(&self) -> bool {
        self.users.lock().unwrap_or_else(|e| e.into_inner()).1
    }

    pub fn call(&self) -> Option<CallState> {
        self.shared.call()
    }

    /// Joins a LiveKit room, leaving any other call.
    pub async fn join(&self, url: &str, token: &str) -> Result<Call, String> {
        room::join(self, url, token).await
    }

    /// What [`Voice::share`] can capture here.
    pub async fn share_targets(&self) -> Targets {
        platform::targets().await
    }

    /// Apps playing audio right now, for [`Voice::share`].
    pub fn audio_apps(&self) -> Vec<AudioApp> {
        self.backend.apps()
    }

    /// Shares a screen or window (and an [`AudioApp::id`]'s audio) into the
    /// call, replacing any share. Returns the portal's new restore token.
    pub async fn share(
        &self,
        target: Target,
        app: Option<String>,
        settings: ScreenSettings,
    ) -> Result<Option<String>, String> {
        screen::start(self, target, app, settings).await
    }

    pub fn stop_share(&self) {
        self.end(None);
    }

    pub fn sharing(&self) -> bool {
        self.shared.sharing()
    }

    /// The capture behind share `id` ended by itself.
    pub(crate) fn end_share(&self, id: u64) {
        self.end(Some(id));
    }

    fn end(&self, id: Option<u64>) {
        if let Some(share) = self.shared.take_share(id) {
            self.backend.capture_app(None);
            drop(share);
            (self.shared.notify)();
        }
    }

    /// Subscribes to `identity`'s screen share (and its audio), or stops
    /// watching. Only watch while the video is on screen: it costs bandwidth.
    pub fn watch(&self, identity: Option<String>) {
        room::watch(&self.shared, identity);
    }

    pub fn watching(&self) -> Option<String> {
        engine::lock(&self.shared.watching).clone()
    }

    /// The latest frame of the share being watched.
    pub fn screen(&self) -> Option<Arc<ScreenFrame>> {
        engine::lock(&self.shared.screen).clone()
    }

    fn running(&self) -> bool {
        let u = *self.users.lock().unwrap_or_else(|e| e.into_inner());
        u.0 > 0 || u.1
    }

    /// Starts or stops audio I/O when the reasons to run change.
    pub(crate) fn update(&self, f: impl FnOnce(&mut (u32, bool))) {
        let mut u = self.users.lock().unwrap_or_else(|e| e.into_inner());
        let before = u.0 > 0 || u.1;
        f(&mut u);
        let after = u.0 > 0 || u.1;
        drop(u);
        match (before, after) {
            (false, true) => {
                let s = self.shared.settings();
                engine::start_dsp(self.shared.clone());
                self.backend.start(s.input, s.output);
            }
            (true, false) => {
                self.backend.stop();
                engine::stop_dsp(&self.shared);
            }
            _ => {}
        }
    }
}
