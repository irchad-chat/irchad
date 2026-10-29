//! One LiveKit call: connect, publish the mic, play everyone else, report state.
//! The SDK reconnects by itself; a [`Call`] ends when the room is gone for good
//! or when it's dropped.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use futures_util::StreamExt;
use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::*;
use livekit::track::{LocalAudioTrack, LocalTrack, RemoteTrack, TrackSource};
use livekit::webrtc::audio_source::{
    AudioSourceOptions, RtcAudioSource, native::NativeAudioSource,
};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::stats::RtcStats;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::engine::{CUE_LEN, Cue, Mic, Shared, lock};
use crate::{CallState, Participant, Quality, RATE, Voice};

/// D14: the SFU decides who's in the room.
const MAX_PARTICIPANTS: usize = 500;
const MAX_IDENTITY: usize = 64;

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
    let (room, mut events) = Room::connect(url, token, RoomOptions::default())
        .await
        .map_err(|e| {
            tracing::warn!("LiveKit connect: {e}");
            "Couldn't connect to voice.".to_owned()
        })?;
    let room = Arc::new(room);
    // Our own APM does the processing (D: we own audio I/O), and the DSP thread
    // paces us, so the source needs no queue.
    let source = NativeAudioSource::new(
        AudioSourceOptions {
            echo_cancellation: false,
            noise_suppression: false,
            auto_gain_control: false,
        },
        RATE,
        1,
        0,
    );
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

    let generation = shared.generation.fetch_add(1, Relaxed) + 1;
    *lock(&shared.mic) = Some(Mic { source, track });
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
        snapshot(&shared, generation, &r);
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
                    if let Some(tx) = shared.add_source(&sid, &identity) {
                        let h = tokio::spawn(play(t, tx));
                        if let Some(old) = streams.insert(sid, h) {
                            old.abort();
                        }
                    }
                }
                RoomEvent::TrackUnsubscribed { publication, .. } => {
                    let sid = publication.sid().to_string();
                    if let Some(h) = streams.remove(&sid) {
                        h.abort();
                    }
                    shared.remove_source(&sid);
                }
                RoomEvent::ParticipantConnected(_) => shared.play_cue(Cue::Join),
                RoomEvent::ParticipantDisconnected(_) => shared.play_cue(Cue::Leave),
                RoomEvent::Reconnecting => shared.update_call(|c| c.reconnecting = true),
                RoomEvent::Reconnected => shared.update_call(|c| c.reconnecting = false),
                RoomEvent::Disconnected { reason } => break end_reason(reason),
                _ => {}
            }
            snapshot(&shared, generation, &r);
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

/// Pushes one remote track into its mixer ring.
async fn play(track: RemoteAudioTrack, mut tx: rtrb::Producer<i16>) {
    let mut stream = NativeAudioStream::new(track.rtc_track(), RATE as i32, 1);
    while let Some(frame) = stream.next().await {
        for &x in frame.data.iter() {
            if tx.push(x).is_err() {
                break;
            }
        }
    }
}

/// Rebuilds the participant list from the room.
fn snapshot(shared: &Shared, generation: u64, room: &Room) {
    if !shared.current(generation) {
        return;
    }
    let local = room.local_participant();
    let mut participants = vec![Participant {
        identity: short(&local.identity().to_string()),
        speaking: local.is_speaking() && !shared.muted(),
        muted: shared.muted(),
        local: true,
    }];
    let mut remote: Vec<Participant> = room
        .remote_participants()
        .values()
        .take(MAX_PARTICIPANTS)
        .map(|p| Participant {
            identity: short(&p.identity().to_string()),
            speaking: p.is_speaking(),
            muted: p
                .track_publications()
                .values()
                .filter(|t| t.source() == TrackSource::Microphone)
                .all(|t| t.is_muted()),
            local: false,
        })
        .collect();
    remote.sort_by(|a, b| a.identity.cmp(&b.identity));
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
