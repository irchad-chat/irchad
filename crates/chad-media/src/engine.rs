//! State shared between the audio callbacks, the DSP thread and the call.
//!
//! ```text
//! mic ─capture()─▶ near ring ─▶ DSP thread: RNNoise? → APM → VAD/PTT gate ─▶ LiveKit source
//!                                     ▲ far ring (process_reverse_stream)    ├▶ whisper track (instead, D71)
//!                                                                            └▶ loopback source
//!                    soundboard clip ─▶ DSP thread (same 10 ms pace) ─▶ soundboard track
//!                                                                     └▶ own soundboard source
//! remote tracks ─▶ source rings ─render()─▶ speakers
//!                                  └────▶ far ring
//! ```
//!
//! The audio callbacks only `try_lock`, so they never wait on anyone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use livekit::track::LocalAudioTrack;
use livekit::webrtc::{
    audio_frame::AudioFrame, audio_source::native::NativeAudioSource,
    native::apm::AudioProcessingModule,
};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::screen::{ScreenFrame, Share};
use crate::{AudioSettings, CallState, FRAME, InputMode, RATE};

/// Most audio a remote source may queue before we skip ahead (latency cap).
const MAX_QUEUE: usize = RATE as usize / 10;
const RING: usize = RATE as usize / 5;
/// Voice activity stays open this long after the level drops.
pub(crate) const VAD_HANG: Duration = Duration::from_millis(300);
/// shortcut: a fixed guess at playback + capture latency for the AEC. AEC3
/// estimates the real delay itself; feed it `pw_stream_get_time_n` if echo
/// survives on some hardware.
const STREAM_DELAY_MS: i32 = 40;
/// D14: LiveKit decides how many tracks we subscribe to.
const MAX_SOURCES: usize = 64;
const SILENT_DB: f32 = -100.0;
pub const CUE_LEN: usize = RATE as usize / 4;

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What a source carries; each has its own volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mic,
    Screen,
    /// Soundboard clips: one volume for everyone's.
    Board,
}

pub struct Source {
    /// Track SID, [`LOOPBACK`] or [`OWN_BOARD`].
    pub key: String,
    pub identity: String,
    kind: Kind,
    rx: Consumer<i16>,
    gain: f32,
    /// Latency cap; more than [`MAX_QUEUE`] for a source that delays itself.
    max_queue: usize,
}

impl Source {
    fn mix(&mut self, out: &mut [i16], deaf: bool) {
        let excess = self.rx.slots().saturating_sub(self.max_queue + out.len());
        for _ in 0..excess {
            let _ = self.rx.pop();
        }
        for o in out.iter_mut() {
            let Ok(x) = self.rx.pop() else { break };
            if !deaf {
                *o = o.saturating_add((f32::from(x) * self.gain) as i16);
            }
        }
    }
}

/// The mic test's source key.
const LOOPBACK: &str = "loopback";
/// Our own soundboard clips, heard the way others hear them.
const OWN_BOARD: &str = "board";

#[derive(Default)]
pub struct Rt {
    sources: Vec<Source>,
    near: Option<Producer<i16>>,
    far: Option<Producer<i16>>,
    /// The chime playing now, and how far into it we are.
    cue: Option<(Vec<i16>, usize)>,
}

/// A short sound in the call we're in.
#[derive(Clone, Copy, Debug)]
pub enum Cue {
    /// Someone joined or left (us included).
    Join,
    Leave,
    /// We muted or deafened, or undid it: the same sound for both.
    Mute,
    Unmute,
    /// Someone (us included) started or stopped sharing their screen.
    ShareStart,
    ShareEnd,
    /// Someone else started watching our share, or the share we watch.
    Viewer,
    /// Someone starts whispering to us; plays before their voice (D71).
    Whisper,
}

impl Cue {
    /// Two notes rising for a join and falling for a leave, two quick low
    /// ones for mute, three for a share, two high taps for a viewer, and a
    /// tap leaping an octave for a whisper.
    pub(crate) fn samples(self) -> Vec<i16> {
        const HALF: usize = CUE_LEN / 2;
        const THIRD: usize = CUE_LEN / 3;
        const SHORT: usize = CUE_LEN / 5;
        const TAP: usize = CUE_LEN / 6;
        // (Hz, samples); 0 Hz rests.
        let notes: &[(f32, usize)] = match self {
            Cue::Join => &[(659.25, HALF), (880.0, HALF)],
            Cue::Leave => &[(880.0, HALF), (659.25, HALF)],
            Cue::Mute => &[(587.33, SHORT), (440.0, SHORT)],
            Cue::Unmute => &[(440.0, SHORT), (587.33, SHORT)],
            Cue::ShareStart => &[(523.25, THIRD), (659.25, THIRD), (783.99, THIRD)],
            Cue::ShareEnd => &[(783.99, THIRD), (659.25, THIRD), (523.25, THIRD)],
            Cue::Viewer => &[(1174.66, TAP), (0.0, TAP / 2), (1174.66, TAP)],
            Cue::Whisper => &[(987.77, TAP), (0.0, TAP / 2), (1975.53, TAP)],
        };
        notes
            .iter()
            .flat_map(|&(hz, len)| {
                (0..len).map(move |i| {
                    let t = i as f32 / RATE as f32;
                    // Quick attack, then decay, so the notes don't click.
                    let env = (i as f32 / 240.0).min(1.0) * (1.0 - i as f32 / len as f32);
                    ((std::f32::consts::TAU * hz * t).sin() * env * 0.15 * 32767.0) as i16
                })
            })
            .collect()
    }
}

/// The call's end of the mic: where the DSP thread sends, and the track to
/// mark muted.
pub struct Mic {
    pub source: NativeAudioSource,
    pub track: LocalAudioTrack,
    /// The soundboard track's source; `None` if the server wouldn't take it.
    pub board: Option<NativeAudioSource>,
    /// The whisper track's source (D71); `None` if the server wouldn't take it.
    pub whisper: Option<NativeAudioSource>,
}

pub struct Shared {
    pub notify: Arc<dyn Fn() + Send + Sync>,
    settings: Mutex<AudioSettings>,
    rt: Mutex<Rt>,
    level: AtomicU32,
    transmitting: AtomicBool,
    ptt: AtomicBool,
    /// The whisper key is down.
    whisper_key: AtomicBool,
    /// The mic goes to the whisper track right now.
    whispering: AtomicBool,
    /// Only the whisper targets in the room may hear the whisper track: the
    /// call set its permissions so (D71). Nothing is whispered otherwise.
    pub whisper_ready: AtomicBool,
    /// Accounts the whisper key talks to.
    pub whisper_to: Mutex<Vec<String>>,
    /// The whisper list changed: the call redoes its permissions.
    pub whisper_changed: tokio::sync::Notify,
    muted: AtomicBool,
    deafened: AtomicBool,
    loopback: AtomicBool,
    /// (identity, screen share audio) → gain.
    volumes: Mutex<HashMap<(String, bool), f32>>,
    /// The soundboard clip we're playing into the call, and how far in.
    pub clip: Mutex<Option<(Arc<[i16]>, usize)>>,
    pub mic: Mutex<Option<Mic>>,
    call: Mutex<Option<(u64, CallState)>>,
    /// Bumped by every join, so a stale call can't clobber a newer one.
    pub generation: AtomicU64,
    dsp: Mutex<Option<(Arc<AtomicBool>, JoinHandle<()>)>>,
    /// The call's room, for screen share and watching.
    pub room: Mutex<Option<Arc<livekit::Room>>>,
    share: Mutex<Option<Share>>,
    camera: Mutex<Option<Share>>,
    shares: AtomicU64,
    /// A share (`[0]`) or camera (`[1]`) whose capture ended before it was stored.
    ended: [AtomicU64; 2],
    /// A camera is being turned on.
    pub camera_starting: AtomicBool,
    /// Identity whose video we watch, and whether it's their camera.
    pub watching: Mutex<Option<(String, bool)>>,
    pub screen: Mutex<Option<Arc<ScreenFrame>>>,
    /// We watch our own share or camera ([`PREVIEW_SCREEN`],
    /// [`PREVIEW_CAMERA`]; 0 for neither): its encoder puts what it sends in `screen`.
    pub preview: AtomicU8,
}

pub const PREVIEW_SCREEN: u8 = 1;
pub const PREVIEW_CAMERA: u8 = 2;

impl Shared {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            notify,
            settings: Mutex::default(),
            rt: Mutex::default(),
            level: AtomicU32::new(SILENT_DB.to_bits()),
            transmitting: AtomicBool::new(false),
            ptt: AtomicBool::new(false),
            whisper_key: AtomicBool::new(false),
            whispering: AtomicBool::new(false),
            whisper_ready: AtomicBool::new(false),
            whisper_to: Mutex::default(),
            whisper_changed: tokio::sync::Notify::new(),
            muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            loopback: AtomicBool::new(false),
            volumes: Mutex::default(),
            clip: Mutex::default(),
            mic: Mutex::default(),
            call: Mutex::default(),
            generation: AtomicU64::new(0),
            dsp: Mutex::default(),
            room: Mutex::default(),
            share: Mutex::default(),
            camera: Mutex::default(),
            shares: AtomicU64::new(0),
            ended: [AtomicU64::new(0), AtomicU64::new(0)],
            camera_starting: AtomicBool::new(false),
            watching: Mutex::default(),
            screen: Mutex::default(),
            preview: AtomicU8::new(0),
        }
    }

    pub fn next_share(&self) -> u64 {
        self.shares.fetch_add(1, Relaxed) + 1
    }

    fn slot(&self, camera: bool) -> &Mutex<Option<Share>> {
        if camera { &self.camera } else { &self.share }
    }

    /// Stores a share (or camera) that's up, unless its capture already
    /// ended or its call did (then dropping it unpublishes and stops it).
    pub fn set_share(&self, share: Share, camera: bool) {
        let mut s = lock(self.slot(camera));
        let live = lock(&self.room)
            .as_ref()
            .is_some_and(|r| Arc::ptr_eq(r, &share.room));
        let stored = live && self.ended[usize::from(camera)].load(Relaxed) != share.id;
        if stored {
            *s = Some(share);
        }
        drop(s);
        if stored {
            self.play_cue(Cue::ShareStart);
        }
        (self.notify)();
    }

    /// Stops the share (or camera), or only share `id` if given.
    pub fn take_share(&self, id: Option<u64>, camera: bool) -> Option<Share> {
        let mut s = lock(self.slot(camera));
        if let Some(id) = id
            && s.as_ref().is_none_or(|s| s.id != id)
        {
            self.ended[usize::from(camera)].store(id, Relaxed);
            return None;
        }
        s.take()
    }

    pub fn sharing(&self) -> bool {
        lock(&self.share).is_some()
    }

    pub fn camera_on(&self) -> bool {
        lock(&self.camera).is_some()
    }

    pub fn settings(&self) -> AudioSettings {
        lock(&self.settings).clone()
    }

    pub fn set_settings(&self, s: AudioSettings) {
        let board = s.soundboard_volume.clamp(0.0, 2.0);
        *lock(&self.settings) = s;
        for src in lock(&self.rt).sources.iter_mut() {
            if src.kind == Kind::Board {
                src.gain = board;
            }
        }
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Relaxed))
    }

    pub fn transmitting(&self) -> bool {
        self.transmitting.load(Relaxed)
    }

    pub fn set_ptt(&self, down: bool) {
        self.ptt.store(down, Relaxed);
    }

    pub fn whispering(&self) -> bool {
        self.whispering.load(Relaxed)
    }

    pub fn set_whisper(&self, down: bool) {
        self.whisper_key.store(down, Relaxed);
    }

    pub fn muted(&self) -> bool {
        self.muted.load(Relaxed) || self.deafened()
    }

    pub fn deafened(&self) -> bool {
        self.deafened.load(Relaxed)
    }

    pub fn set_muted(&self, muted: bool) {
        let was = self.muted.swap(muted, Relaxed);
        self.mute_track();
        self.mute_cue(was, muted);
    }

    pub fn set_deafened(&self, deafened: bool) {
        let was = self.deafened.swap(deafened, Relaxed);
        self.mute_track();
        self.mute_cue(was, deafened);
    }

    fn mute_cue(&self, was: bool, now: bool) {
        if was != now && lock(&self.call).is_some() {
            self.play_cue(if now { Cue::Mute } else { Cue::Unmute });
        }
    }

    /// Shows others our mute state too.
    pub fn mute_track(&self) {
        if let Some(mic) = &*lock(&self.mic) {
            if self.muted() {
                mic.track.mute();
            } else {
                mic.track.unmute();
            }
        }
        self.update_call(|c| {
            for p in c.participants.iter_mut().filter(|p| p.local) {
                p.muted = self.muted();
            }
        });
    }

    pub fn set_loopback(&self, on: bool) {
        self.loopback.store(on, Relaxed);
    }

    pub fn volume(&self, identity: &str, screen: bool) -> f32 {
        lock(&self.volumes)
            .get(&(identity.to_owned(), screen))
            .copied()
            .unwrap_or(1.0)
    }

    pub fn set_volume(&self, identity: &str, screen: bool, gain: f32) {
        lock(&self.volumes).insert((identity.to_owned(), screen), gain);
        let kind = if screen { Kind::Screen } else { Kind::Mic };
        for s in lock(&self.rt).sources.iter_mut() {
            if s.identity == identity && s.kind == kind {
                s.gain = gain;
            }
        }
    }

    /// A ring for one remote track; the caller pushes its audio.
    pub fn add_source(&self, key: &str, identity: &str, kind: Kind) -> Option<Producer<i16>> {
        self.add_delayed_source(key, identity, kind, 0)
    }

    /// Like [`Shared::add_source`], with room for `delay` more samples queued.
    pub fn add_delayed_source(
        &self,
        key: &str,
        identity: &str,
        kind: Kind,
        delay: usize,
    ) -> Option<Producer<i16>> {
        let (tx, rx) = RingBuffer::new(RING + delay);
        let gain = match kind {
            Kind::Board => self.settings().soundboard_volume.clamp(0.0, 2.0),
            _ => self.volume(identity, kind == Kind::Screen),
        };
        let mut rt = lock(&self.rt);
        if rt.sources.len() >= MAX_SOURCES {
            return None;
        }
        rt.sources.push(Source {
            key: key.to_owned(),
            identity: identity.to_owned(),
            kind,
            rx,
            gain,
            max_queue: MAX_QUEUE + delay,
        });
        Some(tx)
    }

    pub fn play_cue(&self, cue: Cue) {
        let samples = cue.samples();
        lock(&self.rt).cue = Some((samples, 0));
    }

    pub fn remove_source(&self, key: &str) {
        lock(&self.rt).sources.retain(|s| s.key != key);
    }

    pub fn call(&self) -> Option<CallState> {
        lock(&self.call).as_ref().map(|(_, c)| c.clone())
    }

    pub fn current(&self, generation: u64) -> bool {
        self.generation.load(Relaxed) == generation
    }

    pub fn set_call(&self, generation: u64, state: Option<CallState>) {
        if self.current(generation) {
            *lock(&self.call) = state.map(|s| (generation, s));
            (self.notify)();
        }
    }

    pub fn update_call(&self, f: impl FnOnce(&mut CallState)) {
        if let Some((_, c)) = &mut *lock(&self.call) {
            f(c);
        }
        (self.notify)();
    }

    /// Playback callback: mixes every source into `out`, and keeps a copy as
    /// the echo canceller's reference.
    pub fn render(&self, out: &mut [i16]) {
        out.fill(0);
        let Ok(mut rt) = self.rt.try_lock() else {
            return;
        };
        let deaf = self.deafened();
        for s in rt.sources.iter_mut().filter(|s| s.key != LOOPBACK) {
            s.mix(out, deaf);
        }
        if let Some((cue, pos)) = &mut rt.cue {
            for (o, &x) in out.iter_mut().zip(&cue[*pos..]) {
                *o = o.saturating_add(x);
            }
            *pos += out.len();
            if *pos >= cue.len() {
                rt.cue = None;
            }
        }
        if let Some(far) = &mut rt.far {
            for &x in out.iter() {
                if far.push(x).is_err() {
                    break;
                }
            }
        }
        // After the AEC reference: our own voice in it makes the AEC suppress
        // the mic test as echo.
        for s in rt.sources.iter_mut().filter(|s| s.key == LOOPBACK) {
            s.mix(out, deaf);
        }
    }

    /// Capture callback.
    pub fn capture(&self, input: &[i16]) {
        let Ok(mut rt) = self.rt.try_lock() else {
            return;
        };
        if let Some(near) = &mut rt.near {
            for &x in input {
                if near.push(x).is_err() {
                    break;
                }
            }
        }
    }
}

pub fn start_dsp(shared: Arc<Shared>) {
    let (near_tx, near) = RingBuffer::new(RING);
    let (far_tx, far) = RingBuffer::new(RING);
    {
        let mut rt = lock(&shared.rt);
        rt.near = Some(near_tx);
        rt.far = Some(far_tx);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let s = shared.clone();
    let st = stop.clone();
    match std::thread::Builder::new()
        .name("irchad-dsp".into())
        .spawn(move || dsp(&s, near, far, &st))
    {
        Ok(handle) => *lock(&shared.dsp) = Some((stop, handle)),
        Err(e) => tracing::error!("starting the audio thread: {e}"),
    }
}

pub fn stop_dsp(shared: &Shared) {
    if let Some((stop, handle)) = lock(&shared.dsp).take() {
        stop.store(true, Relaxed);
        let _ = handle.join();
    }
    let mut rt = lock(&shared.rt);
    rt.near = None;
    rt.far = None;
    drop(rt);
    shared.level.store(SILENT_DB.to_bits(), Relaxed);
    shared.transmitting.store(false, Relaxed);
}

fn dsp(shared: &Shared, mut near: Consumer<i16>, mut far: Consumer<i16>, stop: &AtomicBool) {
    let apm_config = |s: &AudioSettings| (s.echo_cancellation, s.auto_gain, s.noise_suppression);
    let mut config = apm_config(&shared.settings());
    let new_apm = |(aec, agc, ns)| AudioProcessingModule::new(aec, agc, true, ns);
    let mut apm = new_apm(config);
    let mut denoiser = nnnoiseless::DenoiseState::new();
    let (mut frame, mut reference) = ([0i16; FRAME], [0i16; FRAME]);
    let (mut fin, mut fout) = ([0f32; FRAME], [0f32; FRAME]);
    let mut open_until: Option<Instant> = None;
    let mut whisper_until: Option<Instant> = None;
    let mut whisper = [0i16; FRAME];
    let mut loopback: Option<Producer<i16>> = None;
    let mut own_board = shared.add_source(OWN_BOARD, "", Kind::Board);
    let mut board = [0i16; FRAME];
    while !stop.load(Relaxed) {
        while far.slots() >= FRAME {
            reference
                .iter_mut()
                .for_each(|s| *s = far.pop().unwrap_or(0));
            let _ = apm.process_reverse_stream(&mut reference, RATE as i32, 1);
        }
        if near.slots() < FRAME {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        frame.iter_mut().for_each(|s| *s = near.pop().unwrap_or(0));
        let settings = shared.settings();
        if apm_config(&settings) != config {
            // The APM has no runtime toggle; a fresh one re-converges in a second or two.
            config = apm_config(&settings);
            apm = new_apm(config);
        }
        if settings.rnnoise {
            fin.iter_mut()
                .zip(&frame)
                .for_each(|(f, &s)| *f = f32::from(s));
            denoiser.process_frame(&mut fout, &fin);
            frame
                .iter_mut()
                .zip(&fout)
                .for_each(|(s, &f)| *s = f.clamp(-32768.0, 32767.0) as i16);
        }
        let _ = apm.set_stream_delay_ms(STREAM_DELAY_MS);
        let _ = apm.process_stream(&mut frame, RATE as i32, 1);
        // After the APM, so its gain control doesn't undo ours.
        let gain = settings.input_gain.clamp(0.0, 2.0);
        if gain != 1.0 {
            frame
                .iter_mut()
                .for_each(|s| *s = (f32::from(*s) * gain).clamp(-32768.0, 32767.0) as i16);
        }

        let db = dbfs(&frame);
        let now = Instant::now();
        let ptt = shared.ptt.load(Relaxed);
        let open = match settings.mode {
            InputMode::VoiceActivity => {
                if db > settings.vad_threshold_db {
                    open_until = Some(now + VAD_HANG);
                }
                open_until.is_some_and(|t| now < t)
            }
            InputMode::PushToTalk => {
                if ptt {
                    open_until = Some(now + Duration::from_millis(settings.ptt_release_ms.into()));
                }
                ptt || open_until.is_some_and(|t| now < t)
            }
        } && !shared.muted();
        let release = Duration::from_millis(settings.ptt_release_ms.into());
        let whispering = gate_whisper(shared, &mut whisper_until, now, release);
        // A whisper takes the mic from everyone else.
        let open = open && !whispering;
        shared.level.store(db.to_bits(), Relaxed);
        let was = (
            shared.transmitting.swap(open, Relaxed),
            shared.whispering.swap(whispering, Relaxed),
        );
        if was != (open, whispering) {
            (shared.notify)();
        }
        if whispering {
            whisper = frame;
        } else {
            whisper.fill(0);
        }
        if !open {
            frame.fill(0);
        }

        match (shared.loopback.load(Relaxed), &mut loopback) {
            (true, None) => loopback = shared.add_source(LOOPBACK, "", Kind::Mic),
            (false, Some(_)) => {
                loopback = None;
                shared.remove_source(LOOPBACK);
            }
            _ => {}
        }
        if let Some(lb) = &mut loopback {
            let heard = if whispering { &whisper } else { &frame };
            for &x in heard {
                if lb.push(x).is_err() {
                    break;
                }
            }
        }
        let (source, board_source, whisper_source) = match &*lock(&shared.mic) {
            Some(m) => (Some(m.source.clone()), m.board.clone(), m.whisper.clone()),
            None => (None, None, None),
        };
        if let Some(source) = source {
            send(&source, &frame, "mic");
        }
        if let Some(w) = whisper_source {
            send(&w, &whisper, "whisper");
        }
        // Paced by the mic, like the mic: a clip only plays in a call, unmuted.
        if next_clip(shared, board_source.is_some(), &mut board) {
            if let Some(b) = &board_source {
                send(b, &board, "soundboard");
            }
            if let Some(own) = &mut own_board {
                for &x in &board {
                    if own.push(x).is_err() {
                        break;
                    }
                }
            }
        }
    }
    if loopback.is_some() {
        shared.remove_source(LOOPBACK);
    }
    shared.remove_source(OWN_BOARD);
}

/// Whether this frame goes to the whisper track: the key is down (or was,
/// within `release`), we aren't muted, and only targets may hear it.
fn gate_whisper(
    shared: &Shared,
    until: &mut Option<Instant>,
    now: Instant,
    release: Duration,
) -> bool {
    if shared.whisper_key.load(Relaxed) {
        *until = Some(now + release);
    }
    let held = shared.whisper_key.load(Relaxed) || until.is_some_and(|t| now < t);
    held && !shared.muted() && shared.whisper_ready.load(Relaxed)
}

fn send(source: &NativeAudioSource, frame: &[i16], what: &str) {
    let f = AudioFrame {
        data: frame.into(),
        sample_rate: RATE,
        num_channels: 1,
        samples_per_channel: FRAME as u32,
    };
    // Unbuffered source (queue 0): this completes synchronously.
    if let Some(Err(e)) = source.capture_frame(&f).now_or_never() {
        tracing::warn!("sending {what} audio: {e:?}");
    }
}

/// The clip's next frame into `out`, zero-padded at its end. False, and the
/// clip dropped, when nothing plays or it can't be heard (`live`: in a call).
fn next_clip(shared: &Shared, live: bool, out: &mut [i16; FRAME]) -> bool {
    let mut clip = lock(&shared.clip);
    let Some((samples, pos)) = clip.as_mut().filter(|_| live && !shared.muted()) else {
        *clip = None;
        return false;
    };
    let n = (samples.len() - *pos).min(FRAME);
    out[..n].copy_from_slice(&samples[*pos..*pos + n]);
    out[n..].fill(0);
    *pos += n;
    if *pos >= samples.len() {
        *clip = None;
    }
    true
}

pub(crate) fn dbfs(frame: &[i16]) -> f32 {
    let sum: f64 = frame.iter().map(|&x| f64::from(x).powi(2)).sum();
    let rms = (sum / frame.len() as f64).sqrt() / 32768.0;
    if rms <= 0.0 {
        SILENT_DB
    } else {
        (20.0 * rms.log10()).max(f64::from(SILENT_DB)) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_with_gain_and_feeds_the_far_end() {
        let shared = Shared::new(Arc::new(|| {}));
        let (far_tx, mut far) = RingBuffer::new(RING);
        lock(&shared.rt).far = Some(far_tx);
        let mut a = shared.add_source("a", "alice", Kind::Mic).unwrap();
        let mut b = shared.add_source("b", "bob", Kind::Mic).unwrap();
        // Bob's screen share audio, turned down apart from his voice.
        let mut c = shared.add_source("c", "bob", Kind::Screen).unwrap();
        shared.set_volume("bob", false, 0.5);
        shared.set_volume("bob", true, 0.25);
        for _ in 0..4 {
            a.push(1000).unwrap();
            b.push(1000).unwrap();
            c.push(1000).unwrap();
        }
        let mut out = [0i16; 4];
        shared.render(&mut out);
        assert_eq!(out, [1750; 4]);
        assert_eq!(far.pop(), Ok(1750));
        shared.remove_source("c");

        shared.set_deafened(true);
        a.push(1000).unwrap();
        shared.render(&mut out);
        assert_eq!(out, [0; 4]);
        assert!(shared.muted());
        shared.remove_source("a");
        assert_eq!(lock(&shared.rt).sources.len(), 1);
    }

    #[test]
    fn mic_test_stays_out_of_the_aec_reference() {
        let shared = Shared::new(Arc::new(|| {}));
        let (far_tx, mut far) = RingBuffer::new(RING);
        lock(&shared.rt).far = Some(far_tx);
        let mut lb = shared.add_source(LOOPBACK, "", Kind::Mic).unwrap();
        let mut a = shared.add_source("a", "alice", Kind::Mic).unwrap();
        lb.push(500).unwrap();
        a.push(1000).unwrap();
        let mut out = [0i16; 1];
        shared.render(&mut out);
        assert_eq!(out, [1500]);
        assert_eq!(far.pop(), Ok(1000));
    }

    #[test]
    fn soundboard_has_its_own_volume() {
        let shared = Shared::new(Arc::new(|| {}));
        let mut voice = shared.add_source("v", "alice", Kind::Mic).unwrap();
        let mut board = shared.add_source("s", "alice", Kind::Board).unwrap();
        let mut s = shared.settings();
        s.soundboard_volume = 0.25;
        shared.set_settings(s);
        voice.push(1000).unwrap();
        board.push(1000).unwrap();
        let mut out = [0i16; 1];
        shared.render(&mut out);
        assert_eq!(out, [1250]);
    }

    #[test]
    fn clips_play_through_once_and_only_live() {
        let shared = Shared::new(Arc::new(|| {}));
        let mut out = [0i16; FRAME];
        *lock(&shared.clip) = Some((vec![7; FRAME + 10].into(), 0));
        assert!(!next_clip(&shared, false, &mut out));
        assert!(lock(&shared.clip).is_none(), "outside a call it's dropped");

        *lock(&shared.clip) = Some((vec![7; FRAME + 10].into(), 0));
        assert!(next_clip(&shared, true, &mut out));
        assert_eq!(out, [7; FRAME]);
        assert!(next_clip(&shared, true, &mut out));
        assert_eq!((out[9], out[10]), (7, 0));
        assert!(!next_clip(&shared, true, &mut out));

        *lock(&shared.clip) = Some((vec![7; FRAME].into(), 0));
        shared.set_muted(true);
        assert!(!next_clip(&shared, true, &mut out));
    }

    #[test]
    fn cues_play_once() {
        let shared = Shared::new(Arc::new(|| {}));
        shared.play_cue(Cue::Join);
        let mut out = vec![0i16; 480];
        let mut heard = 0;
        for _ in 0..CUE_LEN / 480 + 2 {
            shared.render(&mut out);
            heard += out.iter().filter(|&&x| x != 0).count();
        }
        assert!(heard > CUE_LEN * 9 / 10, "{heard}");
        assert!(lock(&shared.rt).cue.is_none());
    }

    #[test]
    fn cues_fit_the_leave_delay() {
        use Cue::*;
        for cue in [
            Join, Leave, Mute, Unmute, ShareStart, ShareEnd, Viewer, Whisper,
        ] {
            assert!(cue.samples().len() <= CUE_LEN, "{cue:?}");
        }
    }

    #[test]
    fn mute_cues_only_in_a_call_and_on_change() {
        let shared = Shared::new(Arc::new(|| {}));
        shared.set_muted(true);
        assert!(lock(&shared.rt).cue.is_none());
        shared.set_muted(false);
        shared.set_call(0, Some(CallState::default()));
        shared.set_deafened(true);
        assert!(lock(&shared.rt).cue.is_some());
        lock(&shared.rt).cue = None;
        shared.set_deafened(true);
        assert!(lock(&shared.rt).cue.is_none());
    }

    #[test]
    fn whispers_only_when_ready_and_unmuted() {
        let shared = Shared::new(Arc::new(|| {}));
        let (now, release) = (Instant::now(), Duration::from_millis(200));
        let mut until = None;
        shared.set_whisper(true);
        assert!(
            !gate_whisper(&shared, &mut until, now, release),
            "no targets yet"
        );
        shared.whisper_ready.store(true, Relaxed);
        assert!(gate_whisper(&shared, &mut until, now, release));
        shared.set_whisper(false);
        assert!(
            gate_whisper(&shared, &mut until, now, release),
            "release tail"
        );
        assert!(!gate_whisper(&shared, &mut until, now + release, release));
        shared.set_whisper(true);
        shared.set_muted(true);
        assert!(!gate_whisper(&shared, &mut until, now, release));
    }

    #[test]
    fn a_delayed_source_keeps_its_delay() {
        let shared = Shared::new(Arc::new(|| {}));
        let delay = Cue::Whisper.samples().len();
        let mut tx = shared
            .add_delayed_source("w", "alice", Kind::Mic, delay)
            .unwrap();
        for _ in 0..delay {
            tx.push(0).unwrap();
        }
        tx.push(1000).unwrap();
        let mut out = vec![0i16; delay + 1];
        shared.render(&mut out);
        assert_eq!(
            out[delay], 1000,
            "the voice comes after the silence, not instead"
        );
    }

    #[test]
    fn levels() {
        assert_eq!(dbfs(&[0; 480]), SILENT_DB);
        assert!((dbfs(&[i16::MAX; 480])).abs() < 0.01);
        assert!((dbfs(&[3277; 480]) + 20.0).abs() < 0.1);
    }
}
