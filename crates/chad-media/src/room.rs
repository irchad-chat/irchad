//! One LiveKit call: connect, publish the mic, play everyone else, report state.
//! Whispers (D71) are a second mic track that the SFU only lets the targets have.
//! The SDK reconnects by itself; a [`Call`] ends when the room is gone for good
//! or when it's dropped.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::participant::ParticipantTrackPermission;
use livekit::prelude::*;
use livekit::track::{LocalAudioTrack, LocalTrack, RemoteTrack, RemoteVideoTrack, TrackSource};
use livekit::webrtc::audio_source::{
    AudioSourceOptions, RtcAudioSource, native::NativeAudioSource,
};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::stats::RtcStats;
use livekit::webrtc::video_frame::VideoBuffer;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::engine::{
    CUE_LEN, Cue, Kind, Mic, PREVIEW_CAMERA, PREVIEW_SCREEN, Shared, VAD_HANG, dbfs, lock,
};
use crate::screen::{CALL_AUDIO, MAX_DIM, STREAM, STREAM_AUDIO, ScreenFrame, stream_room};
use crate::{CallState, Participant, Quality, RATE, Voice};

/// Participant attribute naming whose screen share someone watches.
const WATCHING: &str = "watching";
/// The soundboard track's name. Its source is `Unknown`: LiveKit takes one
/// track per known source, and the mic has `Microphone`.
const BOARD: &str = "soundboard";
/// The whisper track's name; `Unknown` source too.
const WHISPER: &str = "whisper";
/// A whisper this long after the last one starts anew, with its cue.
const WHISPER_GAP: Duration = Duration::from_secs(1);

/// D14: the SFU decides who's in the room.
const MAX_PARTICIPANTS: usize = 500;
const MAX_IDENTITY: usize = 64;
/// A decoded remote mic above this is talking (senders zero their gated audio).
const TALKING_DB: f32 = -60.0;
/// How often the speaking dots refresh.
const SPEAKING_POLL: Duration = Duration::from_millis(50);

/// A joined call. Dropping it leaves.
pub struct Call {
    room: Arc<Room>,
    voice: Voice,
    generation: u64,
    task: JoinHandle<()>,
    ended: Option<oneshot::Receiver<Option<String>>>,
}

impl Call {
    /// Waits until the call is over by itself: `Some(reason)` if it dropped,
    /// `None` after a normal hang-up.
    pub async fn ended(&mut self) -> Option<String> {
        match self.ended.as_mut() {
            Some(rx) => {
                let r = rx.await.unwrap_or(None);
                self.ended = None;
                r
            }
            None => std::future::pending().await,
        }
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.task.abort();
        let shared = &self.voice.shared;
        if shared.current(self.generation) {
            self.voice.stop_share();
            self.voice.stop_camera();
            *lock(&shared.room) = None;
            *lock(&shared.watching) = None;
            let viewer = lock(&shared.viewer).take();
            drop(viewer);
            *lock(&shared.screen) = None;
            *lock(&shared.mic) = None;
            shared.whisper_ready.store(false, Relaxed);
            shared.set_call(self.generation, None);
            shared.play_cue(Cue::Leave);
        }
        let room = self.room.clone();
        tokio::spawn(async move {
            let _ = room.close().await;
        });
        // Keep the speakers open until the leave chime has played.
        let voice = self.voice.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(
                (CUE_LEN as u64 * 1000 / u64::from(RATE)) + 100,
            ))
            .await;
            voice.update(|u| u.0 = u.0.saturating_sub(1));
        });
    }
}

pub async fn join(voice: &Voice, url: &str, token: &str) -> Result<Call, String> {
    // Screen shares cost bandwidth, so we subscribe to tracks ourselves.
    let mut options = RoomOptions::default();
    options.auto_subscribe = false;
    let (room, mut events) = Room::connect(url, token, options).await.map_err(|e| {
        tracing::warn!("LiveKit connect: {e}");
        "Couldn't connect to voice.".to_owned()
    })?;
    let room = Arc::new(room);
    // Our own APM does the processing (D: we own audio I/O), and the DSP thread
    // paces us, so the source needs no queue.
    let new_source = || {
        NativeAudioSource::new(
            AudioSourceOptions {
                echo_cancellation: false,
                noise_suppression: false,
                auto_gain_control: false,
            },
            RATE,
            1,
            0,
        )
    };
    let source = new_source();
    let track = LocalAudioTrack::create_audio_track("mic", RtcAudioSource::Native(source.clone()));
    let shared = voice.shared.clone();
    let kbps = shared.settings().bitrate_kbps.clamp(16, 256);
    room.local_participant()
        .publish_track(
            LocalTrack::Audio(track.clone()),
            TrackPublishOptions {
                source: TrackSource::Microphone,
                dtx: true,
                red: true,
                audio_encoding: Some(AudioEncoding {
                    max_bitrate: u64::from(kbps) * 1000,
                }),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| {
            tracing::warn!("publishing the mic: {e}");
            "Couldn't send your microphone.".to_owned()
        })?;
    // Its own track, so listeners can turn clips down apart from voices.
    let board = new_source();
    let board_track =
        LocalAudioTrack::create_audio_track(BOARD, RtcAudioSource::Native(board.clone()));
    let board = match room
        .local_participant()
        .publish_track(
            LocalTrack::Audio(board_track),
            TrackPublishOptions {
                source: TrackSource::Unknown,
                dtx: true,
                audio_encoding: Some(AudioEncoding {
                    max_bitrate: 64_000,
                }),
                ..Default::default()
            },
        )
        .await
    {
        Ok(_) => Some(board),
        Err(e) => {
            // An older chadd's token doesn't allow it; voice works without.
            tracing::warn!("publishing the soundboard track: {e}");
            None
        }
    };

    // Silent until permissions say only the targets may have it (D71).
    let whisper = new_source();
    let whisper_track =
        LocalAudioTrack::create_audio_track(WHISPER, RtcAudioSource::Native(whisper.clone()));
    let whisper = match room
        .local_participant()
        .publish_track(
            LocalTrack::Audio(whisper_track),
            TrackPublishOptions {
                source: TrackSource::Unknown,
                dtx: true,
                red: true,
                audio_encoding: Some(AudioEncoding {
                    max_bitrate: u64::from(kbps) * 1000,
                }),
                ..Default::default()
            },
        )
        .await
    {
        Ok(_) => Some(whisper),
        Err(e) => {
            tracing::warn!("publishing the whisper track: {e}");
            None
        }
    };

    let generation = shared.generation.fetch_add(1, Relaxed) + 1;
    shared.whisper_ready.store(false, Relaxed);
    *lock(&shared.mic) = Some(Mic {
        source,
        track,
        board,
        whisper,
    });
    *lock(&shared.room) = Some(room.clone());
    *lock(&shared.watching) = None;
    for p in room.remote_participants().values() {
        let identity = short(&p.identity().to_string());
        for publication in p.track_publications().values() {
            subscribe(&shared, publication, &identity);
        }
    }
    shared.mute_track();
    shared.set_call(
        generation,
        Some(CallState {
            room: room.name(),
            ..Default::default()
        }),
    );
    voice.update(|u| u.0 += 1);
    shared.play_cue(Cue::Join);

    let (end_tx, end_rx) = oneshot::channel();
    let r = room.clone();
    let task = tokio::spawn(async move {
        let mut streams: HashMap<String, JoinHandle<()>> = HashMap::new();
        let mut stats = tokio::time::interval(Duration::from_secs(2));
        let mut levels = tokio::time::interval(SPEAKING_POLL);
        levels.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Speaking comes from our own audio, not the SFU's slow active-speaker reports.
        let epoch = Instant::now();
        let local_id = short(&r.local_participant().identity().to_string());
        // Per mic or whisper track: (identity, last loud, a whisper).
        let mut talking: HashMap<String, (String, Arc<AtomicU64>, bool)> = HashMap::new();
        let mut speakers = (HashSet::new(), HashSet::new());
        let mut seen = None;
        let mut grants = None;
        permit(&shared, &r, &mut grants).await;
        snapshot(&shared, generation, &r, &mut seen, &speakers);
        let reason = loop {
            let ev = tokio::select! {
                ev = events.recv() => ev,
                () = shared.whisper_changed.notified() => {
                    permit(&shared, &r, &mut grants).await;
                    snapshot(&shared, generation, &r, &mut seen, &speakers);
                    continue;
                }
                _ = stats.tick() => {
                    let rtt = rtt_ms(&r).await;
                    if shared.current(generation) {
                        shared.update_call(|c| c.rtt_ms = rtt);
                    }
                    continue;
                }
                _ = levels.tick() => {
                    let now = speaking(&shared, &local_id, &talking, epoch);
                    if now != speakers && shared.current(generation) {
                        shared.update_call(|c| {
                            for p in &mut c.participants {
                                p.speaking = now.0.contains(&p.identity);
                                p.whispering = now.1.contains(&p.identity);
                            }
                        });
                    }
                    speakers = now;
                    continue;
                }
            };
            let Some(ev) = ev else {
                break Some("Lost the voice connection.".to_owned());
            };
            match ev {
                RoomEvent::TrackSubscribed {
                    track: RemoteTrack::Audio(t),
                    publication,
                    participant,
                } => {
                    let sid = publication.sid().to_string();
                    let identity = short(&participant.identity().to_string());
                    let whisper = publication.source() == TrackSource::Unknown
                        && publication.name() == WHISPER;
                    // A whisper plays at their voice's volume.
                    let kind = match publication.source() {
                        TrackSource::Microphone => Kind::Mic,
                        TrackSource::ScreenshareAudio => Kind::Screen,
                        _ if whisper => Kind::Mic,
                        _ => Kind::Board,
                    };
                    let cue = whisper.then(|| shared.clone());
                    let delay = if whisper { whisper_cue_len() } else { 0 };
                    if let Some(tx) = shared.add_delayed_source(
                        &sid,
                        &identity,
                        kind,
                        delay,
                        !whisper && kind != Kind::Screen,
                    ) {
                        let loud = Arc::new(AtomicU64::new(0));
                        if kind == Kind::Mic {
                            talking.insert(sid.clone(), (identity, loud.clone(), whisper));
                        }
                        let h = tokio::spawn(play(t, tx, loud, epoch, cue));
                        if let Some(old) = streams.insert(sid, h) {
                            old.abort();
                        }
                    }
                }
                RoomEvent::TrackSubscribed {
                    track: RemoteTrack::Video(t),
                    publication,
                    participant,
                } => {
                    let target = (
                        short(&participant.identity().to_string()),
                        publication.source() == TrackSource::Camera,
                    );
                    let h = tokio::spawn(view(t, target, shared.clone()));
                    if let Some(old) = streams.insert(publication.sid().to_string(), h) {
                        old.abort();
                    }
                }
                RoomEvent::TrackPublished {
                    publication,
                    participant,
                } => subscribe(
                    &shared,
                    &publication,
                    &short(&participant.identity().to_string()),
                ),
                RoomEvent::TrackUnsubscribed {
                    publication,
                    participant,
                    ..
                } => {
                    let sid = publication.sid().to_string();
                    if let Some(h) = streams.remove(&sid) {
                        h.abort();
                    }
                    talking.remove(&sid);
                    shared.remove_source(&sid);
                    // Only the video on the stage; switching already cleared it.
                    let camera = publication.source() == TrackSource::Camera;
                    if (camera || publication.source() == TrackSource::Screenshare)
                        && *lock(&shared.watching)
                            == Some((short(&participant.identity().to_string()), camera))
                    {
                        *lock(&shared.screen) = None;
                    }
                }
                RoomEvent::ParticipantConnected(_) => shared.play_cue(Cue::Join),
                RoomEvent::ParticipantDisconnected(_) => shared.play_cue(Cue::Leave),
                RoomEvent::Reconnecting => shared.update_call(|c| c.reconnecting = true),
                RoomEvent::Reconnected => shared.update_call(|c| c.reconnecting = false),
                RoomEvent::Disconnected { reason } => break end_reason(reason),
                _ => {}
            }
            // Someone came or went, or our tracks changed.
            permit(&shared, &r, &mut grants).await;
            snapshot(&shared, generation, &r, &mut seen, &speakers);
        };
        for (sid, h) in streams {
            h.abort();
            shared.remove_source(&sid);
        }
        let _ = end_tx.send(reason);
    });
    Ok(Call {
        room,
        voice: voice.clone(),
        generation,
        task,
        ended: Some(end_rx),
    })
}

/// Pushes one remote track into its mixer ring, and stamps `loud` (ms since
/// `epoch`) whenever it carries voice. With `cue`, it's a whisper: each one
/// starts with [`Cue::Whisper`], its voice held back until the cue is done.
async fn play(
    track: RemoteAudioTrack,
    mut tx: rtrb::Producer<i16>,
    loud: Arc<AtomicU64>,
    epoch: Instant,
    cue: Option<Arc<Shared>>,
) {
    let mut stream = NativeAudioStream::new(track.rtc_track(), RATE as i32, 1);
    let mut onset = Onset::default();
    while let Some(frame) = stream.next().await {
        let is_loud = !frame.data.is_empty() && dbfs(&frame.data) > TALKING_DB;
        if is_loud {
            loud.store(epoch.elapsed().as_millis() as u64, Relaxed);
        }
        let mut data: &[i16] = &frame.data;
        if let Some(shared) = &cue {
            let (lead, skip) = onset.frame(is_loud, Instant::now(), data.len());
            if lead > 0 && !shared.deafened() {
                shared.play_cue(Cue::Whisper);
                for _ in 0..lead {
                    if tx.push(0).is_err() {
                        break;
                    }
                }
            }
            data = &data[skip..];
        }
        for &x in data {
            if tx.push(x).is_err() {
                break;
            }
        }
    }
}

fn whisper_cue_len() -> usize {
    Cue::Whisper.samples().len()
}

/// Where whispers start, to put the cue before them.
#[derive(Default)]
struct Onset {
    last_loud: Option<Instant>,
    /// Silence we put in for a cue and haven't taken back out yet.
    debt: usize,
}

impl Onset {
    /// For one frame of `len` samples: how much silence to put before it (the
    /// cue's length when a whisper starts), and how much of it to skip (quiet
    /// samples, to pay back that silence between whispers).
    fn frame(&mut self, loud: bool, now: Instant, len: usize) -> (usize, usize) {
        if loud {
            let new = self
                .last_loud
                .is_none_or(|t| now.duration_since(t) >= WHISPER_GAP);
            self.last_loud = Some(now);
            if new && self.debt == 0 {
                self.debt = whisper_cue_len();
                return (self.debt, 0);
            }
            (0, 0)
        } else {
            let skip = self.debt.min(len);
            self.debt -= skip;
            (0, skip)
        }
    }
}

/// Microphones always; a screen share and its audio, or a camera, only while watched.
fn subscribe(shared: &Shared, publication: &RemoteTrackPublication, identity: &str) {
    let watched = |camera: bool| {
        lock(&shared.watching)
            .as_ref()
            .is_some_and(|w| w.0 == identity && w.1 == camera)
    };
    // A whisper only reaches us if the SFU lets it: we're a target (D71).
    let want = match publication.source() {
        TrackSource::Microphone => true,
        TrackSource::Unknown => [BOARD, WHISPER].contains(&publication.name().as_str()),
        TrackSource::Screenshare | TrackSource::ScreenshareAudio => watched(false),
        TrackSource::Camera => watched(true),
    };
    if want != publication.is_subscribed() {
        publication.set_subscribed(want);
    }
}

/// Watches (identity, their camera rather than their screen), or nothing.
pub fn watch(shared: &Shared, target: Option<(String, bool)>) {
    let target = target.map(|(i, camera)| (short(&i), camera));
    {
        let mut w = lock(&shared.watching);
        if *w == target {
            return;
        }
        w.clone_from(&target);
    }
    // A public stream's room is for one share; a new target needs its own.
    let viewer = lock(&shared.viewer).take();
    drop(viewer);
    // Off before clearing, so the encoder can't put a frame of ours back.
    shared.preview.store(0, Relaxed);
    *lock(&shared.screen) = None;
    let Some(room) = lock(&shared.room).clone() else {
        return;
    };
    let own = target
        .as_ref()
        .is_some_and(|t| t.0 == short(&room.local_participant().identity().to_string()));
    let camera = target.as_ref().is_some_and(|t| t.1);
    if own {
        let kind = if camera {
            PREVIEW_CAMERA
        } else {
            PREVIEW_SCREEN
        };
        shared.preview.store(kind, Relaxed);
    }
    for p in room.remote_participants().values() {
        let id = short(&p.identity().to_string());
        for publication in p.track_publications().values() {
            subscribe(shared, publication, &id);
        }
    }
    // Tells the sharer how many watch (D22). Previewing our own isn't
    // watching, and cameras don't count watchers.
    let identity = target.filter(|t| !own && !t.1).map(|t| t.0);
    tokio::spawn(async move {
        let attrs = HashMap::from([(WATCHING.to_owned(), identity.unwrap_or_default())]);
        if let Err(e) = room.local_participant().set_attributes(attrs).await {
            tracing::debug!("setting the watching attribute: {e}");
        }
    });
    (shared.notify)();
}

/// A public share we watch from its stream's room (D87). Dropping it leaves.
pub(crate) struct Viewer {
    room: Arc<Room>,
    task: JoinHandle<()>,
    shared: Arc<Shared>,
}

/// Its audio's mixer keys start with this.
const VIEWER_KEY: &str = "stream:";

impl Drop for Viewer {
    fn drop(&mut self) {
        self.task.abort();
        self.shared.remove_sources(VIEWER_KEY);
        *lock(&self.shared.screen) = None;
        let room = self.room.clone();
        tokio::spawn(async move {
            let _ = room.close().await;
        });
    }
}

/// The stream room of the (someone else's) share we watch, if we aren't in it yet.
pub fn stream_wanted(shared: &Shared) -> Option<String> {
    let (identity, camera) = lock(&shared.watching).clone()?;
    if camera {
        return None;
    }
    let room = shared
        .call()?
        .participants
        .into_iter()
        .find(|p| !p.local && p.identity == identity)?
        .stream?;
    let have = lock(&shared.viewer)
        .as_ref()
        .is_some_and(|v| v.room.name() == room);
    (!have).then_some(room)
}

/// Joins stream room `room` to watch the share in it, if it's still the one
/// we want: its screen and app audio, not the call audio (we're in the call).
pub async fn watch_stream(
    shared: &Arc<Shared>,
    room: String,
    url: &str,
    token: &str,
) -> Result<(), String> {
    let mut options = RoomOptions::default();
    options.auto_subscribe = false;
    let (lk, mut events) = Room::connect(url, token, options).await.map_err(|e| {
        tracing::warn!("stream connect: {e}");
        "Couldn't open the stream.".to_owned()
    })?;
    let lk = Arc::new(lk);
    let Some(identity) = lock(&shared.watching).clone().map(|w| w.0) else {
        let _ = lk.close().await;
        return Ok(());
    };
    if stream_wanted(shared).as_deref() != Some(room.as_str()) || lk.name() != room {
        let _ = lk.close().await;
        return Ok(());
    }
    let want = |p: &RemoteTrackPublication| {
        let ok = matches!(
            p.source(),
            TrackSource::Screenshare | TrackSource::ScreenshareAudio
        ) && p.name() != CALL_AUDIO;
        if ok && !p.is_subscribed() {
            p.set_subscribed(true);
        }
    };
    for p in lk.remote_participants().values().take(MAX_PARTICIPANTS) {
        p.track_publications().values().for_each(want);
    }
    let (s, r) = (shared.clone(), lk.clone());
    let task = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        let epoch = Instant::now();
        while let Some(ev) = events.recv().await {
            match ev {
                RoomEvent::TrackPublished { publication, .. } => want(&publication),
                RoomEvent::TrackSubscribed {
                    track: RemoteTrack::Video(t),
                    publication,
                    ..
                } if publication.source() == TrackSource::Screenshare => {
                    tasks.spawn(view(t, (identity.clone(), false), s.clone()));
                }
                RoomEvent::TrackSubscribed {
                    track: RemoteTrack::Audio(t),
                    publication,
                    ..
                } if publication.source() == TrackSource::ScreenshareAudio => {
                    let key = format!("{VIEWER_KEY}{}", publication.sid());
                    if let Some(tx) = s.add_source(&key, &identity, Kind::Screen) {
                        let loud = Arc::new(AtomicU64::new(0));
                        tasks.spawn(play(t, tx, loud, epoch, None));
                    }
                }
                RoomEvent::Disconnected { .. } => break,
                _ => {}
            }
        }
        s.remove_sources(VIEWER_KEY);
        let mut v = lock(&s.viewer);
        // The room went away by itself. Dropped outside the lock, as Viewer's
        // drop takes others; aborting this task now is harmless, it's done.
        let old = v
            .as_ref()
            .is_some_and(|v| Arc::ptr_eq(&v.room, &r))
            .then(|| v.take());
        drop(v);
        drop(old);
        (s.notify)();
    });
    let old = lock(&shared.viewer).replace(Viewer {
        room: lk,
        task,
        shared: shared.clone(),
    });
    drop(old);
    (shared.notify)();
    Ok(())
}

/// Decodes the watched share or camera, `target` (identity, camera), into
/// [`ScreenFrame`]s for the UI while it's still the one watched.
async fn view(track: RemoteVideoTrack, target: (String, bool), shared: Arc<Shared>) {
    let mut stream = NativeVideoStream::new(track.rtc_track());
    let mut seq = 0;
    while let Some(frame) = stream.next().await {
        let b = frame.buffer.to_i420();
        let (width, height) = (b.width(), b.height());
        // D14: whatever a peer encodes, we hold at most one bounded frame.
        if !(1..=MAX_DIM).contains(&width) || !(1..=MAX_DIM).contains(&height) {
            continue;
        }
        let (sy, su, sv) = b.strides();
        let (y, u, v) = b.data();
        seq += 1;
        let frame = Arc::new(ScreenFrame {
            width,
            height,
            y: y.to_vec(),
            u: u.to_vec(),
            v: v.to_vec(),
            strides: [sy, su, sv],
            seq,
        });
        // Checked under the screen lock, like `watch` clears it.
        let mut screen = lock(&shared.screen);
        if lock(&shared.watching).as_ref() != Some(&target) {
            continue;
        }
        *screen = Some(frame);
        drop(screen);
        (shared.notify)();
    }
}

/// What the last snapshot saw of others, to cue what changed.
struct Seen {
    sharing: HashSet<String>,
    /// Watchers per sharer, not counting us.
    watchers: HashMap<String, u32>,
}

/// Who's talking right now, and who of them whispers: us while the mic gate
/// (or whisper) is open, others while their decoded audio was loud within
/// [`VAD_HANG`].
fn speaking(
    shared: &Shared,
    local_id: &str,
    talking: &HashMap<String, (String, Arc<AtomicU64>, bool)>,
    epoch: Instant,
) -> (HashSet<String>, HashSet<String>) {
    let now = epoch.elapsed().as_millis() as u64;
    let hang = VAD_HANG.as_millis() as u64;
    let live: Vec<_> = talking
        .values()
        .filter(|(_, loud, _)| {
            let t = loud.load(Relaxed);
            t != 0 && now.saturating_sub(t) < hang
        })
        .collect();
    let us = |on: bool| on.then(|| local_id.to_owned());
    let whispering = shared.whispering();
    (
        live.iter()
            .map(|(id, ..)| id.clone())
            .chain(us(shared.transmitting() || whispering))
            .collect(),
        live.iter()
            .filter(|(.., w)| *w)
            .map(|(id, ..)| id.clone())
            .chain(us(whispering))
            .collect(),
    )
}

/// What each remote may subscribe to: `None` for everything, all tracks.
type Grants = Option<Vec<(ParticipantIdentity, bool, Vec<TrackSid>)>>;

/// Who may subscribe to which of `ours` (sid, the whisper track) (D71):
/// targets to all, everyone else to all but the whisper track. `None` (all
/// for all) while no target is here, as then nothing gets whispered.
fn whisper_permissions(
    remotes: &[ParticipantIdentity],
    targets: &[String],
    ours: &[(TrackSid, bool)],
) -> Grants {
    let is_target = |p: &ParticipantIdentity| targets.contains(&short(p.as_str()));
    if !ours.iter().any(|o| o.1) || !remotes.iter().any(is_target) {
        return None;
    }
    let open: Vec<TrackSid> = ours.iter().filter(|o| !o.1).map(|o| o.0.clone()).collect();
    // D14: past the cap, a remote gets nothing (it isn't shown either).
    let mut grants: Vec<_> = remotes
        .iter()
        .take(MAX_PARTICIPANTS)
        .map(|p| {
            let t = is_target(p);
            (p.clone(), t, if t { Vec::new() } else { open.clone() })
        })
        .collect();
    grants.sort();
    Some(grants)
}

/// Tells the SFU who may have our whisper track, when that changed; only
/// then may the DSP thread whisper.
async fn permit(shared: &Shared, room: &Room, last: &mut Option<Grants>) {
    let local = room.local_participant();
    let mut ours: Vec<(TrackSid, bool)> = local
        .track_publications()
        .values()
        .map(|p| (p.sid(), p.name() == WHISPER))
        .collect();
    ours.sort();
    let mut remotes: Vec<ParticipantIdentity> = room.remote_participants().into_keys().collect();
    remotes.sort();
    let targets = lock(&shared.whisper_to).clone();
    let grants = whisper_permissions(&remotes, &targets, &ours);
    if last.as_ref() == Some(&grants) {
        return;
    }
    // Closed while the SFU learns who may listen.
    shared.whisper_ready.store(false, Relaxed);
    let ready = grants.is_some();
    let permissions = grants
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(
            |(participant_identity, allow_all, allowed_track_sids)| ParticipantTrackPermission {
                participant_identity,
                allow_all,
                allowed_track_sids,
            },
        )
        .collect();
    if let Err(e) = local
        .set_track_subscription_permissions(!ready, permissions)
        .await
    {
        tracing::warn!("setting track permissions: {e}");
        *last = None;
        return;
    }
    shared.whisper_ready.store(ready, Relaxed);
    *last = Some(grants);
}

/// A guest picks their own name, so it's marked as such (D86). D14: one line, 32 chars.
fn guest_name(identity: &str, name: &str) -> Option<String> {
    if !identity.starts_with("guest:") {
        return None;
    }
    let name: String = name.chars().filter(|c| !c.is_control()).take(32).collect();
    let name = name.trim();
    Some(format!(
        "{} (guest)",
        if name.is_empty() { "Guest" } else { name }
    ))
}

/// Rebuilds the participant list from the room.
fn snapshot(
    shared: &Shared,
    generation: u64,
    room: &Room,
    seen: &mut Option<Seen>,
    (speakers, whisperers): &(HashSet<String>, HashSet<String>),
) {
    if !shared.current(generation) {
        return;
    }
    let local = room.local_participant();
    let local_id = short(&local.identity().to_string());
    let remotes = room.remote_participants();
    let mut others: HashMap<String, u32> = HashMap::new();
    for w in remotes
        .values()
        .take(MAX_PARTICIPANTS)
        .filter_map(|p| p.attributes().get(WATCHING).map(|w| short(w)))
    {
        *others.entry(w).or_default() += 1;
    }
    // Our own preview doesn't count.
    let ours = lock(&shared.watching)
        .clone()
        .filter(|w| !w.1 && w.0 != local_id)
        .map(|w| w.0);
    let mut watchers = others.clone();
    if let Some(w) = &ours {
        *watchers.entry(w.clone()).or_default() += 1;
    }
    let own_stream = lock(&shared.share).as_ref().and_then(|s| {
        let (_, audio) = s.public.as_ref()?;
        Some((s.room.name(), *audio))
    });
    let mut participants = vec![Participant {
        speaking: speakers.contains(&local_id),
        muted: shared.muted(),
        local: true,
        sharing: shared.sharing(),
        stream: own_stream.as_ref().map(|s| s.0.clone()),
        stream_audio: own_stream.is_some_and(|s| s.1),
        watchers: watchers.get(&local_id).copied().unwrap_or(0),
        camera: shared.camera_on(),
        whispering: whisperers.contains(&local_id),
        name: guest_name(&local_id, &local.name()),
        identity: local_id,
    }];
    let mut remote: Vec<Participant> = remotes
        .values()
        .take(MAX_PARTICIPANTS)
        .map(|p| {
            let identity = short(&p.identity().to_string());
            let pubs = p.track_publications();
            let attrs = p.attributes();
            // D14: only a room chadd would name, and only that.
            let stream = attrs.get(STREAM).filter(|r| stream_room(r)).cloned();
            let stream_audio =
                stream.is_some() && attrs.get(STREAM_AUDIO).is_some_and(|a| a == "1");
            Participant {
                speaking: speakers.contains(&identity),
                muted: pubs
                    .values()
                    .filter(|t| t.source() == TrackSource::Microphone)
                    .all(livekit::prelude::RemoteTrackPublication::is_muted),
                local: false,
                sharing: stream.is_some()
                    || pubs
                        .values()
                        .any(|t| t.source() == TrackSource::Screenshare),
                stream,
                stream_audio,
                camera: pubs
                    .values()
                    .any(|t| t.source() == TrackSource::Camera && !t.is_muted()),
                watchers: watchers.get(&identity).copied().unwrap_or(0),
                whispering: whisperers.contains(&identity),
                name: guest_name(&identity, &p.name()),
                identity,
            }
        })
        .collect();
    remote.sort_by(|a, b| a.identity.cmp(&b.identity));

    let sharing: HashSet<String> = remote
        .iter()
        .filter(|p| p.sharing)
        .map(|p| p.identity.clone())
        .collect();
    // Not on the first look, or joining would cue every share already up.
    if let Some(prev) = seen {
        // Someone leaving mid-share gets the leave cue instead.
        let ended = prev
            .sharing
            .iter()
            .any(|id| !sharing.contains(id) && remote.iter().any(|p| p.identity == *id));
        let me = &participants[0];
        let ours_shared = me.sharing.then_some(&me.identity);
        let viewer = ours_shared.into_iter().chain(&ours).any(|id| {
            others.get(id).copied().unwrap_or(0) > prev.watchers.get(id).copied().unwrap_or(0)
        });
        if !sharing.is_subset(&prev.sharing) {
            shared.play_cue(Cue::ShareStart);
        } else if ended {
            shared.play_cue(Cue::ShareEnd);
        } else if viewer {
            shared.play_cue(Cue::Viewer);
        }
    }
    *seen = Some(Seen {
        sharing,
        watchers: others,
    });
    // The one we watch stopped sharing, turned their camera off, or left.
    let gone = lock(&shared.watching).clone().is_some_and(|(id, camera)| {
        id != participants[0].identity
            && !remote
                .iter()
                .any(|p| p.identity == id && if camera { p.camera } else { p.sharing })
    });
    participants.extend(remote);
    if gone {
        watch(shared, None);
    }
    let quality = match local.connection_quality() {
        ConnectionQuality::Excellent => Quality::Excellent,
        ConnectionQuality::Good => Quality::Good,
        ConnectionQuality::Poor => Quality::Poor,
        ConnectionQuality::Lost => Quality::Lost,
    };
    shared.update_call(|c| {
        c.participants = participants;
        c.quality = quality;
    });
}

/// The selected ICE candidate pair's current round trip.
async fn rtt_ms(room: &Room) -> Option<u32> {
    let stats = room.get_stats().await.ok()?;
    stats
        .publisher_stats
        .iter()
        .chain(&stats.subscriber_stats)
        .find_map(|s| match s {
            // Nominated and answered; on loopback the round trip really is 0 ms.
            RtcStats::CandidatePair(c)
                if c.candidate_pair.nominated && c.candidate_pair.responses_received > 0 =>
            {
                Some((c.candidate_pair.current_round_trip_time * 1000.0).round() as u32)
            }
            _ => None,
        })
}

fn end_reason(reason: livekit::DisconnectReason) -> Option<String> {
    use livekit::DisconnectReason as R;
    match reason {
        R::ClientInitiated => None,
        R::DuplicateIdentity => Some("You joined voice from somewhere else.".into()),
        R::ParticipantRemoved | R::RoomDeleted => Some("You were removed from voice.".into()),
        other => {
            tracing::info!("voice disconnected: {other:?}");
            Some("Lost the voice connection.".into())
        }
    }
}

fn short(s: &str) -> String {
    s.chars().take(MAX_IDENTITY).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(s: &str) -> TrackSid {
        TrackSid::try_from(format!("TR_{s}")).unwrap()
    }

    #[test]
    fn only_targets_may_have_the_whisper() {
        let ours = [
            (sid("mic"), false),
            (sid("whisper"), true),
            (sid("cam"), false),
        ];
        let remotes: Vec<ParticipantIdentity> = vec!["bob".into(), "carol".into()];
        let g = whisper_permissions(&remotes, &["bob".into()], &ours).unwrap();
        assert_eq!(
            g,
            [
                ("bob".into(), true, vec![]),
                ("carol".into(), false, vec![sid("mic"), sid("cam")]),
            ]
        );
    }

    #[test]
    fn all_may_have_everything_without_a_target_here() {
        let ours = [(sid("mic"), false), (sid("whisper"), true)];
        let remotes: Vec<ParticipantIdentity> = vec!["carol".into()];
        assert_eq!(whisper_permissions(&remotes, &[], &ours), None);
        assert_eq!(whisper_permissions(&remotes, &["bob".into()], &ours), None);
        // No whisper track: nothing to keep from anyone.
        let mic = [(sid("mic"), false)];
        assert_eq!(whisper_permissions(&remotes, &["carol".into()], &mic), None);
    }

    #[test]
    fn past_the_cap_gets_nothing() {
        let ours = [(sid("mic"), false), (sid("whisper"), true)];
        let remotes: Vec<ParticipantIdentity> = (0..MAX_PARTICIPANTS + 5)
            .map(|i| format!("p{i:04}").into())
            .collect();
        let g = whisper_permissions(&remotes, &["p0000".into()], &ours).unwrap();
        assert_eq!(g.len(), MAX_PARTICIPANTS);
        assert!(g.iter().all(|(p, t, _)| *t == (p.as_str() == "p0000")));
    }

    #[test]
    fn the_cue_goes_before_each_whisper_and_comes_back_out_between() {
        let cue = whisper_cue_len();
        let mut o = Onset::default();
        let t = Instant::now();
        assert_eq!(o.frame(true, t, 480), (cue, 0), "the first whisper");
        assert_eq!(o.frame(true, t + Duration::from_millis(10), 480), (0, 0));
        // Quiet within the gap pays the silence back.
        let mut paid = 0;
        for i in 0..cue / 480 + 1 {
            paid += o
                .frame(false, t + Duration::from_millis(20 + i as u64), 480)
                .1;
        }
        assert_eq!(paid, cue);
        assert_eq!(o.frame(false, t, 480), (0, 0));
        // Talking again soon is the same whisper.
        assert_eq!(o.frame(true, t + Duration::from_millis(500), 480), (0, 0));
        assert_eq!(o.frame(true, t + Duration::from_secs(2), 480), (cue, 0));
    }
}
