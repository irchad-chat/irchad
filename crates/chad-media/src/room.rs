//! One LiveKit call: connect, publish the mic, play everyone else, report state.
//! The SDK reconnects by itself; a [`Call`] ends when the room is gone for good
//! or when it's dropped.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use livekit::options::{AudioEncoding, TrackPublishOptions};
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

use crate::engine::{CUE_LEN, Cue, Kind, Mic, Shared, VAD_HANG, dbfs, lock};
use crate::screen::{MAX_DIM, ScreenFrame};
use crate::{CallState, Participant, Quality, RATE, Voice};

/// Participant attribute naming whose screen share someone watches.
const WATCHING: &str = "watching";
/// The soundboard track's name. Its source is `Unknown`: LiveKit takes one
/// track per known source, and the mic has `Microphone`.
const BOARD: &str = "soundboard";

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
            *lock(&shared.room) = None;
            *lock(&shared.watching) = None;
            *lock(&shared.screen) = None;
            *lock(&shared.mic) = None;
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

    let generation = shared.generation.fetch_add(1, Relaxed) + 1;
    *lock(&shared.mic) = Some(Mic {
        source,
        track,
        board,
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
        let mut talking: HashMap<String, (String, Arc<AtomicU64>)> = HashMap::new();
        let mut speakers = HashSet::new();
        let mut seen = None;
        snapshot(&shared, generation, &r, &mut seen, &speakers);
        let reason = loop {
            let ev = tokio::select! {
                ev = events.recv() => ev,
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
                                p.speaking = now.contains(&p.identity);
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
                    let kind = match publication.source() {
                        TrackSource::Microphone => Kind::Mic,
                        TrackSource::ScreenshareAudio => Kind::Screen,
                        _ => Kind::Board,
                    };
                    if let Some(tx) = shared.add_source(&sid, &identity, kind) {
                        let loud = Arc::new(AtomicU64::new(0));
                        if kind == Kind::Mic {
                            talking.insert(sid.clone(), (identity, loud.clone()));
                        }
                        let h = tokio::spawn(play(t, tx, loud, epoch));
                        if let Some(old) = streams.insert(sid, h) {
                            old.abort();
                        }
                    }
                }
                RoomEvent::TrackSubscribed {
                    track: RemoteTrack::Video(t),
                    publication,
                    ..
                } => {
                    let h = tokio::spawn(view(t, shared.clone()));
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
                RoomEvent::TrackUnsubscribed { publication, .. } => {
                    let sid = publication.sid().to_string();
                    if let Some(h) = streams.remove(&sid) {
                        h.abort();
                    }
                    talking.remove(&sid);
                    shared.remove_source(&sid);
                    if publication.source() == TrackSource::Screenshare {
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
/// `epoch`) whenever it carries voice.
async fn play(
    track: RemoteAudioTrack,
    mut tx: rtrb::Producer<i16>,
    loud: Arc<AtomicU64>,
    epoch: Instant,
) {
    let mut stream = NativeAudioStream::new(track.rtc_track(), RATE as i32, 1);
    while let Some(frame) = stream.next().await {
        if !frame.data.is_empty() && dbfs(&frame.data) > TALKING_DB {
            loud.store(epoch.elapsed().as_millis() as u64, Relaxed);
        }
        for &x in frame.data.iter() {
            if tx.push(x).is_err() {
                break;
            }
        }
    }
}

/// Microphones always; a screen share and its audio only while watched.
fn subscribe(shared: &Shared, publication: &RemoteTrackPublication, identity: &str) {
    let want = match publication.source() {
        TrackSource::Microphone => true,
        TrackSource::Unknown => publication.name() == BOARD,
        TrackSource::Screenshare | TrackSource::ScreenshareAudio => {
            lock(&shared.watching).as_deref() == Some(identity)
        }
        _ => false,
    };
    if want != publication.is_subscribed() {
        publication.set_subscribed(want);
    }
}

pub fn watch(shared: &Shared, identity: Option<String>) {
    let identity = identity.map(|i| short(&i));
    {
        let mut w = lock(&shared.watching);
        if *w == identity {
            return;
        }
        w.clone_from(&identity);
    }
    // Off before clearing, so the encoder can't put a frame of ours back.
    shared.preview.store(false, Relaxed);
    *lock(&shared.screen) = None;
    let Some(room) = lock(&shared.room).clone() else {
        return;
    };
    let own = identity.as_deref()
        == Some(short(&room.local_participant().identity().to_string()).as_str());
    shared.preview.store(own, Relaxed);
    for p in room.remote_participants().values() {
        let id = short(&p.identity().to_string());
        for publication in p.track_publications().values() {
            subscribe(shared, publication, &id);
        }
    }
    // Tells the sharer how many watch (D22). Previewing our own isn't watching.
    let identity = identity.filter(|_| !own);
    tokio::spawn(async move {
        let attrs = HashMap::from([(WATCHING.to_owned(), identity.unwrap_or_default())]);
        if let Err(e) = room.local_participant().set_attributes(attrs).await {
            tracing::debug!("setting the watching attribute: {e}");
        }
    });
    (shared.notify)();
}

/// Decodes the watched share into [`ScreenFrame`]s for the UI.
async fn view(track: RemoteVideoTrack, shared: Arc<Shared>) {
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
        *lock(&shared.screen) = Some(Arc::new(ScreenFrame {
            width,
            height,
            y: y.to_vec(),
            u: u.to_vec(),
            v: v.to_vec(),
            strides: [sy, su, sv],
            seq,
        }));
        (shared.notify)();
    }
}

/// What the last snapshot saw of others, to cue what changed.
struct Seen {
    sharing: HashSet<String>,
    /// Watchers per sharer, not counting us.
    watchers: HashMap<String, u32>,
}

/// Who's talking right now: us while the mic gate is open, others while their
/// decoded audio was loud within [`VAD_HANG`].
fn speaking(
    shared: &Shared,
    local_id: &str,
    talking: &HashMap<String, (String, Arc<AtomicU64>)>,
    epoch: Instant,
) -> HashSet<String> {
    let now = epoch.elapsed().as_millis() as u64;
    let hang = VAD_HANG.as_millis() as u64;
    talking
        .values()
        .filter(|(_, loud)| {
            let t = loud.load(Relaxed);
            t != 0 && now.saturating_sub(t) < hang
        })
        .map(|(id, _)| id.clone())
        .chain(shared.transmitting().then(|| local_id.to_owned()))
        .collect()
}

/// Rebuilds the participant list from the room.
fn snapshot(
    shared: &Shared,
    generation: u64,
    room: &Room,
    seen: &mut Option<Seen>,
    speakers: &HashSet<String>,
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
    let ours = lock(&shared.watching).clone().filter(|w| *w != local_id);
    let mut watchers = others.clone();
    if let Some(w) = &ours {
        *watchers.entry(w.clone()).or_default() += 1;
    }
    let mut participants = vec![Participant {
        speaking: speakers.contains(&local_id),
        muted: shared.muted(),
        local: true,
        sharing: shared.sharing(),
        watchers: watchers.get(&local_id).copied().unwrap_or(0),
        identity: local_id,
    }];
    let mut remote: Vec<Participant> = remotes
        .values()
        .take(MAX_PARTICIPANTS)
        .map(|p| {
            let identity = short(&p.identity().to_string());
            let pubs = p.track_publications();
            Participant {
                speaking: speakers.contains(&identity),
                muted: pubs
                    .values()
                    .filter(|t| t.source() == TrackSource::Microphone)
                    .all(|t| t.is_muted()),
                local: false,
                sharing: pubs
                    .values()
                    .any(|t| t.source() == TrackSource::Screenshare),
                watchers: watchers.get(&identity).copied().unwrap_or(0),
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
    participants.extend(remote);
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
