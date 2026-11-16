//! Voice: one audio engine for the whole app, and at most one LiveKit call.
//!
//! The engine owns audio I/O (PipeWire on Linux, cpal/WASAPI elsewhere). It
//! mixes remote participants itself and runs the mic through libwebrtc's APM
//! with that mix as the echo canceller's far end (`docs/notes/livekit-sdk.md`).
//! The mic is only open while in a call or testing the input.

mod camera;
mod engine;
mod monitor;
mod platform;
mod room;
mod screen;
mod sound;

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

pub use monitor::StreamStats;
pub use room::{Call, wait_closed};
pub use screen::{
    AudioApp, ContentHint, Encoder, Preset, Public, ScreenFrame, ScreenSettings, ShareSource,
    SourceId, Target, Targets, hardware_decoder, hardware_encoder,
};
pub use sound::{MAX_SECS as MAX_SOUND_SECS, load as load_sound};

/// Everything runs at 48 kHz mono, 10 ms frames.
pub(crate) const RATE: u32 = 48_000;
pub(crate) const FRAME: usize = (RATE / 100) as usize;
/// Most accounts on the whisper list.
pub const MAX_WHISPER_TARGETS: usize = 50;

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
    /// The APM's acoustic echo canceller, fed the playback mix.
    pub echo_cancellation: bool,
    /// The APM's own noise suppression.
    pub noise_suppression: bool,
    /// RNNoise before the APM's own noise suppression.
    pub rnnoise: bool,
    /// The APM's automatic gain control. Off by default: it pumps the level
    /// up and down between words.
    pub auto_gain: bool,
    /// Mic volume, 0.0..=2.0, after processing: what others hear.
    pub input_gain: f32,
    pub bitrate_kbps: u32,
    /// Everyone's soundboard clips (ours too), 0.0..=2.0, apart from voices.
    pub soundboard_volume: f32,
    /// A [`Devices::cameras`] id; `None` takes the first camera.
    pub camera: Option<String>,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            mode: InputMode::VoiceActivity,
            vad_threshold_db: -50.0,
            ptt_release_ms: 200,
            echo_cancellation: true,
            noise_suppression: true,
            rnnoise: false,
            auto_gain: false,
            input_gain: 1.0,
            bitrate_kbps: 64,
            soundboard_volume: 1.0,
            camera: None,
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
    pub cameras: Vec<Device>,
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
    /// A guest's own name with " (guest)" (D86); `None` for members.
    pub name: Option<String>,
    pub speaking: bool,
    pub muted: bool,
    pub local: bool,
    /// Has a screen share up.
    pub sharing: bool,
    /// The share has a public link: its stream's room (D87).
    pub stream: Option<String>,
    /// The call's audio goes out with that stream: everyone here is public.
    pub stream_audio: bool,
    /// How many others watch that share.
    pub watchers: u32,
    /// Has their camera on.
    pub camera: bool,
    /// Whispers to us; for ourselves, whispers to our list (D71).
    pub whispering: bool,
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

    /// Holds the whisper key: only [`Voice::set_whisper_targets`] hear us (D71).
    pub fn set_whisper(&self, down: bool) {
        self.shared.set_whisper(down);
    }

    /// Whether the mic goes to the whisper targets right now.
    pub fn whispering(&self) -> bool {
        self.shared.whispering()
    }

    /// The accounts the whisper key talks to, when they're in the call.
    pub fn set_whisper_targets(&self, mut targets: Vec<String>) {
        targets.sort();
        targets.dedup();
        targets.truncate(MAX_WHISPER_TARGETS);
        let mut t = engine::lock(&self.shared.whisper_to);
        if *t != targets {
            *t = targets;
            drop(t);
            self.shared.whisper_changed.notify_one();
        }
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

    /// Plays a [`load_sound`] clip into the call, replacing one that's
    /// playing. Everyone in the call hears it at their soundboard volume, us
    /// too. Errs, in words for the user, when it can't be heard.
    pub fn play_sound(&self, clip: Arc<[i16]>) -> Result<(), &'static str> {
        match &*engine::lock(&self.shared.mic) {
            None => return Err("Join voice to play sounds."),
            Some(m) if m.board.is_none() => {
                return Err("This server doesn't let IrChad play sounds into voice yet.");
            }
            Some(_) if self.muted() => return Err("Unmute to play sounds."),
            Some(_) => {}
        }
        *engine::lock(&self.shared.clip) = Some((clip, 0));
        Ok(())
    }

    pub fn stop_sound(&self) {
        *engine::lock(&self.shared.clip) = None;
    }

    pub fn playing_sound(&self) -> bool {
        engine::lock(&self.shared.clip).is_some()
    }

    /// Plays the processed mic back, to hear what others would.
    pub fn set_testing(&self, on: bool) {
        self.shared.set_loopback(on);
        self.update(|u| u.1 = on);
    }

    pub fn testing(&self) -> bool {
        crate::engine::lock(&self.users).1
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
    /// call, replacing any share. With `public`, it goes to that stream's
    /// room instead, which the call watches too (D87). Returns the portal's
    /// new restore token.
    pub async fn share(
        &self,
        target: Target,
        app: Option<String>,
        settings: ScreenSettings,
        public: Option<Public>,
    ) -> Result<Option<String>, String> {
        screen::start(self, target, app, settings, public).await
    }

    pub fn stop_share(&self) {
        self.end(None, false);
    }

    pub fn sharing(&self) -> bool {
        self.shared.sharing()
    }

    /// Our share has a public link (D87).
    pub fn streaming(&self) -> bool {
        engine::lock(&self.shared.share)
            .as_ref()
            .is_some_and(|s| s.public.is_some())
    }

    /// The stream room of the share we watch, while we still need a token
    /// for it (D87): hand one to [`Voice::watch_stream`].
    pub fn stream_wanted(&self) -> Option<String> {
        room::stream_wanted(&self.shared)
    }

    /// Watches the share in stream room `room` with a token for it.
    pub async fn watch_stream(&self, room: String, url: &str, token: &str) -> Result<(), String> {
        room::watch_stream(&self.shared, room, url, token).await
    }

    /// The capture behind share `id` ended by itself.
    pub(crate) fn end_share(&self, id: u64, camera: bool) {
        self.end(Some(id), camera);
    }

    /// Sends the camera picked in [`AudioSettings::camera`] into the call.
    /// Does nothing while another start is under way.
    pub async fn start_camera(&self) -> Result<(), String> {
        use std::sync::atomic::Ordering::Relaxed;
        if self.shared.camera_starting.swap(true, Relaxed) {
            return Ok(());
        }
        (self.shared.notify)();
        let r = camera::start(self).await;
        self.shared.camera_starting.store(false, Relaxed);
        (self.shared.notify)();
        r
    }

    /// [`Voice::start_camera`] is under way (the portal may be asking).
    pub fn camera_starting(&self) -> bool {
        self.shared
            .camera_starting
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn stop_camera(&self) {
        self.end(None, true);
    }

    pub fn camera_on(&self) -> bool {
        self.shared.camera_on()
    }

    fn end(&self, id: Option<u64>, camera: bool) {
        if let Some(share) = self.shared.take_share(id, camera) {
            if !camera {
                self.backend.capture_app(None);
                self.shared.set_call_mix(None, Some(share.id));
            }
            drop(share);
            self.shared.play_cue(engine::Cue::ShareEnd);
            let kind = if camera {
                engine::PREVIEW_CAMERA
            } else {
                engine::PREVIEW_SCREEN
            };
            if self
                .shared
                .preview
                .load(std::sync::atomic::Ordering::Relaxed)
                == kind
            {
                self.watch(None);
            }
            (self.shared.notify)();
        }
    }

    /// Subscribes to `identity`'s screen share (and its audio), or stops
    /// watching. Only watch while the video is on screen: it costs bandwidth.
    pub fn watch(&self, identity: Option<String>) {
        room::watch(&self.shared, identity.map(|i| (i, false)));
    }

    /// Like [`Voice::watch`], for `identity`'s camera instead.
    pub fn watch_camera(&self, identity: String) {
        room::watch(&self.shared, Some((identity, true)));
    }

    /// Whose video we watch.
    pub fn watching(&self) -> Option<String> {
        engine::lock(&self.shared.watching)
            .as_ref()
            .map(|w| w.0.clone())
    }

    /// The video we watch is a camera, not a screen share.
    pub fn watching_camera(&self) -> bool {
        engine::lock(&self.shared.watching)
            .as_ref()
            .is_some_and(|w| w.1)
    }

    /// The latest frame of the video being watched.
    pub fn screen(&self) -> Option<Arc<ScreenFrame>> {
        engine::lock(&self.shared.screen).clone()
    }

    /// Shows the stats of the video we watch over it, or hides them.
    pub fn set_stream_stats(&self, on: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        self.shared.stats_wanted.store(on, Relaxed);
        (self.shared.notify)();
    }

    /// Whether [`Voice::set_stream_stats`] is on.
    pub fn stream_stats_wanted(&self) -> bool {
        self.shared
            .stats_wanted
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The watched video's stats while they're wanted and it's still the one
    /// watched: a new target's stats take a second to arrive.
    pub fn stream_stats(&self) -> Option<StreamStats> {
        let w = engine::lock(&self.shared.watching);
        engine::lock(&self.shared.stream_stats)
            .as_ref()
            .filter(|(t, _)| Some(t) == w.as_ref())
            .map(|(_, s)| s.clone())
    }

    fn running(&self) -> bool {
        let u = *crate::engine::lock(&self.users);
        u.0 > 0 || u.1
    }

    /// Starts or stops audio I/O when the reasons to run change.
    pub(crate) fn update(&self, f: impl FnOnce(&mut (u32, bool))) {
        let mut u = crate::engine::lock(&self.users);
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
