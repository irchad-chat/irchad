//! State shared between the audio callbacks, the DSP thread and the call.
//!
//! ```text
//! mic ─capture()─▶ near ring ─▶ DSP thread: RNNoise? → APM → VAD/PTT gate ─▶ LiveKit source
//!                                     ▲ far ring (process_reverse_stream)    └▶ loopback source
//! remote tracks ─▶ source rings ─render()─▶ speakers
//!                                  └────▶ far ring
//! ```
//!
//! The audio callbacks only `try_lock`, so they never wait on anyone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
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
const VAD_HANG: Duration = Duration::from_millis(300);
/// shortcut: a fixed guess at playback + capture latency for the AEC. AEC3
/// estimates the real delay itself; feed it `pw_stream_get_time_n` if echo
/// survives on some hardware.
const STREAM_DELAY_MS: i32 = 40;
/// D14: LiveKit decides how many tracks we subscribe to.
const MAX_SOURCES: usize = 64;
const SILENT_DB: f32 = -100.0;
pub const CUE_LEN: usize = RATE as usize / 4;

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Source {
    /// Track SID, or `loopback`.
    pub key: String,
    pub identity: String,
    /// Screen share audio, which has its own volume.
    screen: bool,
    rx: Consumer<i16>,
    gain: f32,
}

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
}

impl Cue {
    /// Two notes rising for a join and falling for a leave, two quick low
    /// ones for mute, three for a share, two high taps for a viewer.
    fn samples(self) -> Vec<i16> {
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
}

pub struct Shared {
    pub notify: Arc<dyn Fn() + Send + Sync>,
    settings: Mutex<AudioSettings>,
    rt: Mutex<Rt>,
    level: AtomicU32,
    transmitting: AtomicBool,
    ptt: AtomicBool,
    muted: AtomicBool,
    deafened: AtomicBool,
    loopback: AtomicBool,
    /// (identity, screen share audio) → gain.
    volumes: Mutex<HashMap<(String, bool), f32>>,
    pub mic: Mutex<Option<Mic>>,
    call: Mutex<Option<(u64, CallState)>>,
    /// Bumped by every join, so a stale call can't clobber a newer one.
    pub generation: AtomicU64,
    dsp: Mutex<Option<(Arc<AtomicBool>, JoinHandle<()>)>>,
    /// The call's room, for screen share and watching.
    pub room: Mutex<Option<Arc<livekit::Room>>>,
    share: Mutex<Option<Share>>,
    shares: AtomicU64,
    /// A share whose capture ended before it was stored.
    ended: AtomicU64,
    /// Identity whose screen we watch.
    pub watching: Mutex<Option<String>>,
    pub screen: Mutex<Option<Arc<ScreenFrame>>>,
    /// We watch our own share: the encoder puts what it sends in `screen`.
    pub preview: AtomicBool,
}

impl Shared {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            notify,
            settings: Mutex::default(),
            rt: Mutex::default(),
            level: AtomicU32::new(SILENT_DB.to_bits()),
            transmitting: AtomicBool::new(false),
            ptt: AtomicBool::new(false),
            muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            loopback: AtomicBool::new(false),
            volumes: Mutex::default(),
            mic: Mutex::default(),
            call: Mutex::default(),
            generation: AtomicU64::new(0),
            dsp: Mutex::default(),
            room: Mutex::default(),
            share: Mutex::default(),
            shares: AtomicU64::new(0),
            ended: AtomicU64::new(0),
            watching: Mutex::default(),
            screen: Mutex::default(),
            preview: AtomicBool::new(false),
        }
    }

    pub fn next_share(&self) -> u64 {
        self.shares.fetch_add(1, Relaxed) + 1
    }

    pub fn set_share(&self, share: Share) {
        let mut s = lock(&self.share);
        let stored = self.ended.load(Relaxed) != share.id;
        if stored {
            *s = Some(share);
        }
        drop(s);
        if stored {
            self.play_cue(Cue::ShareStart);
        }
        (self.notify)();
    }

    /// Stops the share, or only share `id` if given.
    pub fn take_share(&self, id: Option<u64>) -> Option<Share> {
        let mut s = lock(&self.share);
        if let Some(id) = id
            && s.as_ref().is_none_or(|s| s.id != id)
        {
            self.ended.store(id, Relaxed);
            return None;
        }
        s.take()
    }

    pub fn sharing(&self) -> bool {
        lock(&self.share).is_some()
    }

    pub fn settings(&self) -> AudioSettings {
        lock(&self.settings).clone()
    }

    pub fn set_settings(&self, s: AudioSettings) {
        *lock(&self.settings) = s;
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
        for s in lock(&self.rt).sources.iter_mut() {
            if s.identity == identity && s.screen == screen {
                s.gain = gain;
            }
        }
    }

    /// A ring for one remote track; the caller pushes its audio. `screen`:
    /// it's screen share audio rather than a microphone.
    pub fn add_source(&self, key: &str, identity: &str, screen: bool) -> Option<Producer<i16>> {
        let (tx, rx) = RingBuffer::new(RING);
        let gain = self.volume(identity, screen);
        let mut rt = lock(&self.rt);
        if rt.sources.len() >= MAX_SOURCES {
            return None;
        }
        rt.sources.push(Source {
            key: key.to_owned(),
            identity: identity.to_owned(),
            screen,
            rx,
            gain,
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
        for s in rt.sources.iter_mut() {
            let excess = s.rx.slots().saturating_sub(MAX_QUEUE + out.len());
            for _ in 0..excess {
                let _ = s.rx.pop();
            }
            for o in out.iter_mut() {
                let Ok(x) = s.rx.pop() else { break };
                if !deaf {
                    *o = o.saturating_add((f32::from(x) * s.gain) as i16);
                }
            }
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
    let mut aec = shared.settings().echo_cancellation;
    let mut apm = AudioProcessingModule::new(aec, true, true, true);
    let mut denoiser = nnnoiseless::DenoiseState::new();
    let (mut frame, mut reference) = ([0i16; FRAME], [0i16; FRAME]);
    let (mut fin, mut fout) = ([0f32; FRAME], [0f32; FRAME]);
    let mut open_until: Option<Instant> = None;
    let mut loopback: Option<Producer<i16>> = None;
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
        if settings.echo_cancellation != aec {
            // The APM has no runtime toggle; a fresh one re-converges in a second or two.
            aec = settings.echo_cancellation;
            apm = AudioProcessingModule::new(aec, true, true, true);
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
        shared.level.store(db.to_bits(), Relaxed);
        if shared.transmitting.swap(open, Relaxed) != open {
            (shared.notify)();
        }
        if !open {
            frame.fill(0);
        }

        match (shared.loopback.load(Relaxed), &mut loopback) {
            (true, None) => loopback = shared.add_source("loopback", "", false),
            (false, Some(_)) => {
                loopback = None;
                shared.remove_source("loopback");
            }
            _ => {}
        }
        if let Some(lb) = &mut loopback {
            for &x in &frame {
                if lb.push(x).is_err() {
                    break;
                }
            }
        }
        let source = lock(&shared.mic).as_ref().map(|m| m.source.clone());
        if let Some(source) = source {
            let f = AudioFrame {
                data: frame.as_slice().into(),
                sample_rate: RATE,
                num_channels: 1,
                samples_per_channel: FRAME as u32,
            };
            // Unbuffered source (queue 0): this completes synchronously.
            if let Some(Err(e)) = source.capture_frame(&f).now_or_never() {
                tracing::warn!("sending mic audio: {e:?}");
            }
        }
    }
    if loopback.is_some() {
        shared.remove_source("loopback");
    }
}

fn dbfs(frame: &[i16]) -> f32 {
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
        let mut a = shared.add_source("a", "alice", false).unwrap();
        let mut b = shared.add_source("b", "bob", false).unwrap();
        // Bob's screen share audio, turned down apart from his voice.
        let mut c = shared.add_source("c", "bob", true).unwrap();
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
        for cue in [Join, Leave, Mute, Unmute, ShareStart, ShareEnd, Viewer] {
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
    fn levels() {
        assert_eq!(dbfs(&[0; 480]), SILENT_DB);
        assert!((dbfs(&[i16::MAX; 480])).abs() < 0.01);
        assert!((dbfs(&[3277; 480]) + 20.0).abs() < 0.1);
    }
}
