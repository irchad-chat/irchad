//! Where a screen share falls behind. Once a second the capture counters and
//! libwebrtc's sender stats are compared, and a second in which a stage did
//! less than what fed it gets a log line naming the stage. A stutter someone
//! reports can then be placed from their exported log: capture, convert,
//! encoder, pacer or network. If none is named, it happened past our upload.
//!
//! The viewer's side, [`StreamStats`], is shown on request over the video,
//! and a second of it that went wrong is logged the same way (see `rough`),
//! so a report can be matched up with the sender's log by time.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use livekit::prelude::Room;
use livekit::track::RemoteVideoTrack;
use livekit::webrtc::stats::{QualityLimitationReason, RtcStats};

use crate::engine::{Shared, lock};

/// What the capture side did since the monitor last looked. Written by the
/// frame sink on the capture thread.
#[derive(Default)]
pub(crate) struct Capture {
    /// Frames handed to libwebrtc.
    frames: AtomicU32,
    /// The longest wait between two of them, in ms.
    gap_ms: AtomicU32,
    /// The longest one took to convert, scale and hand over, in ms.
    work_ms: AtomicU32,
}

impl Capture {
    pub(crate) fn frame(&self, gap: Duration, work: Duration) {
        let ms = |d: Duration| u32::try_from(d.as_millis()).unwrap_or(u32::MAX);
        self.frames.fetch_add(1, Relaxed);
        self.gap_ms.fetch_max(ms(gap), Relaxed);
        self.work_ms.fetch_max(ms(work), Relaxed);
    }
}

/// libwebrtc's running totals for the share's video, at one moment.
#[derive(Default, Clone)]
struct Sample {
    source_frames: u32,
    encoded: u32,
    key: u32,
    sent: u32,
    huge: u32,
    encode_s: f64,
    queue_s: f64,
    packets: u64,
    bytes: u64,
    nack: u32,
    pli: u32,
    target_bps: f64,
    limit: QualityLimitationReason,
    available_bps: f64,
    rtt_s: f64,
    lost: f64,
    encoder: String,
    hardware: bool,
    size: (u32, u32),
    fps: f64,
}

impl Sample {
    /// `track`: the share's video track, as other tracks (a camera) may be
    /// published on the same connection.
    fn read(stats: &[RtcStats], track: &str) -> Option<Self> {
        let source = stats.iter().find_map(|s| match s {
            RtcStats::MediaSource(m) if m.source.track_identifier == track => Some(m),
            _ => None,
        })?;
        let o = stats.iter().find_map(|s| match s {
            RtcStats::OutboundRtp(o) if o.outbound.media_source_id == source.rtc.id => Some(o),
            _ => None,
        })?;
        let mut sample = Self {
            source_frames: source.video.frames,
            encoded: o.outbound.frames_encoded,
            key: o.outbound.key_frames_encoded,
            sent: o.outbound.frames_sent,
            huge: o.outbound.huge_frames_sent,
            encode_s: o.outbound.total_encode_time,
            queue_s: o.outbound.total_packet_send_delay,
            packets: o.sent.packets_sent,
            bytes: o.sent.bytes_sent,
            nack: o.outbound.nack_count,
            pli: o.outbound.pli_count,
            target_bps: o.outbound.target_bitrate,
            limit: o.outbound.quality_limitation_reason,
            encoder: o.outbound.encoder_implementation.clone(),
            hardware: o.outbound.power_efficient_encoder,
            size: (o.outbound.frame_width, o.outbound.frame_height),
            fps: o.outbound.frames_per_second,
            ..Self::default()
        };
        for s in stats {
            match s {
                RtcStats::CandidatePair(c) if c.candidate_pair.nominated => {
                    sample.available_bps = c.candidate_pair.available_outgoing_bitrate;
                    sample.rtt_s = c.candidate_pair.current_round_trip_time;
                }
                RtcStats::RemoteInboundRtp(r) if r.remote_inbound.local_id == o.rtc.id => {
                    sample.lost = r.remote_inbound.fraction_lost;
                }
                _ => {}
            }
        }
        Some(sample)
    }
}

/// One second of a share: what each stage took in and put out.
#[derive(Debug, Default, PartialEq)]
struct Second {
    captured: u32,
    gap_ms: u32,
    work_ms: u32,
    /// Frames libwebrtc took from us, encoded, and sent.
    source: u32,
    encoded: u32,
    key: u32,
    sent: u32,
    huge: u32,
    /// Per encoded frame.
    encode_ms: f64,
    /// Per packet, waiting in the pacer.
    queue_ms: f64,
    kbps: f64,
    target_kbps: f64,
    available_kbps: f64,
    nack: u32,
    pli: u32,
    lost: f64,
    rtt_ms: f64,
    limit: QualityLimitationReason,
}

impl Second {
    fn between(then: &Sample, now: &Sample, captured: (u32, u32, u32)) -> Self {
        let encoded = now.encoded.saturating_sub(then.encoded);
        let packets = now.packets.saturating_sub(then.packets);
        let per = |total: f64, n: u64| {
            if n == 0 {
                0.0
            } else {
                total.max(0.0) * 1000.0 / n as f64
            }
        };
        Self {
            captured: captured.0,
            gap_ms: captured.1,
            work_ms: captured.2,
            source: now.source_frames.saturating_sub(then.source_frames),
            encoded,
            key: now.key.saturating_sub(then.key),
            sent: now.sent.saturating_sub(then.sent),
            huge: now.huge.saturating_sub(then.huge),
            encode_ms: per(now.encode_s - then.encode_s, u64::from(encoded)),
            queue_ms: per(now.queue_s - then.queue_s, packets),
            kbps: now.bytes.saturating_sub(then.bytes) as f64 * 8.0 / 1000.0,
            target_kbps: now.target_bps / 1000.0,
            available_kbps: now.available_bps / 1000.0,
            nack: now.nack.saturating_sub(then.nack),
            pli: now.pli.saturating_sub(then.pli),
            lost: now.lost,
            rtt_ms: now.rtt_s * 1000.0,
            limit: now.limit,
        }
    }

    /// The first stage that fell behind in this second, if any. `fps`: what
    /// the share asks for.
    ///
    /// A still screen is not behind: some capturers only deliver on change,
    /// so few frames alone means nothing. Each test is a stage doing less
    /// than it was given, or taking longer than a frame lasts.
    /// `was`: frames captured the second before. A screen that was moving
    /// and all but stopped is a stall's onset, logged once; a screen that
    /// stays still isn't.
    fn behind(&self, fps: u32, was: u32) -> Option<&'static str> {
        let frame_ms = 1000.0 / f64::from(fps.max(1));
        let moving = self.captured >= fps / 4 || was >= fps / 2;
        if moving && f64::from(self.gap_ms) >= (4.0 * frame_ms).max(100.0) {
            // Frames were coming and then weren't: the capturer, or whatever
            // it waits on (the compositor, a busy GPU).
            Some("capture")
        } else if f64::from(self.work_ms) > 2.0 * frame_ms {
            Some("convert")
        } else if self.source >= 4 && self.encoded + self.source / 4 < self.source
            || self.encode_ms > 1.5 * frame_ms
        {
            // libwebrtc drops frames ahead of a slow encoder, and ahead of a
            // full pacer too: the queue and bitrate figures say which.
            Some("encoder")
        } else if self.queue_ms > 100.0
            || self.huge > 0
            || self.sent + 4 + self.encoded / 4 < self.encoded
            || (self.target_kbps > 0.0 && self.encoded >= 4 && self.kbps > 1.5 * self.target_kbps)
        {
            // Encoded faster than it may be sent: the encoder overshot its
            // target, or the target just fell.
            Some("pacer")
        } else if self.pli > 0
            || self.nack >= 10
            || self.lost > 0.02
            || self.limit != QualityLimitationReason::None
            || (self.available_kbps > 0.0 && self.available_kbps < 0.8 * self.target_kbps)
        {
            Some("network")
        } else {
            None
        }
    }
}

/// Starts watching a share. Stops by itself once the share's frame sink (the
/// owner of `capture`) is gone. `track`: the video track's id.
pub(crate) fn watch(room: Arc<Room>, capture: Weak<Capture>, track: String, fps: u32) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last: Option<Sample> = None;
        let mut samples = 0u32;
        let mut was = 0;
        loop {
            tick.tick().await;
            let Some(capture) = capture.upgrade() else {
                return;
            };
            let captured = (
                capture.frames.swap(0, Relaxed),
                capture.gap_ms.swap(0, Relaxed),
                capture.work_ms.swap(0, Relaxed),
            );
            drop(capture);
            let was = std::mem::replace(&mut was, captured.0);
            let Ok(stats) = room.get_stats().await else {
                continue;
            };
            let Some(now) = Sample::read(&stats.publisher_stats, &track) else {
                continue;
            };
            samples += 1;
            // Which encoder libwebrtc actually picked, once it's running.
            if samples == 5 {
                tracing::info!(
                    "screen share encoder: {} (hardware: {}), {}x{} at {:.0} fps",
                    now.encoder,
                    now.hardware,
                    now.size.0,
                    now.size.1,
                    now.fps,
                );
            }
            if let Some(then) = &last {
                let s = Second::between(then, &now, captured);
                if let Some(stage) = s.behind(fps, was) {
                    tracing::info!(
                        "screen share behind at {stage}: captured {} (gap {} ms, convert {} ms), \
                         encoder {} in {} out {} key at {:.1} ms, sent {} ({} huge) at {:.0} of \
                         {:.0} kbps, queued {:.0} ms, {} nack {} pli {:.1}% lost, rtt {:.0} ms, \
                         {:.0} kbps available, limited by {:?}",
                        s.captured,
                        s.gap_ms,
                        s.work_ms,
                        s.source,
                        s.encoded,
                        s.key,
                        s.encode_ms,
                        s.sent,
                        s.huge,
                        s.kbps,
                        s.target_kbps,
                        s.queue_ms,
                        s.nack,
                        s.pli,
                        s.lost * 100.0,
                        s.rtt_ms,
                        s.available_kbps,
                        s.limit,
                    );
                }
            }
            last = Some(now);
        }
    });
}

/// How the video we watch arrives and decodes, over the last second. For
/// "it's blurry for me but not for them": the frame size and QP say what the
/// sender encoded, the loss, PLI and freezes what the network did to it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamStats {
    /// The codec's name, e.g. "AV1".
    pub codec: String,
    pub decoder: String,
    /// libwebrtc counts the decoder as power-efficient (hardware).
    pub hardware: bool,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub kbps: f64,
    /// The average quantizer of the frames decoded: higher is blurrier. Its
    /// scale depends on the codec (H.264 0–51, AV1 and VP9 0–255).
    pub qp: Option<f64>,
    /// Of the packets that should have arrived.
    pub lost_pct: f64,
    pub dropped: u32,
    pub decode_ms: f64,
    /// How long frames waited in the jitter buffer.
    pub buffer_ms: f64,
    pub jitter_ms: f64,
    /// Since we started watching.
    pub keyframes: u32,
    pub plis: u32,
    pub nacks: u32,
    pub freezes: u32,
    pub rtt_ms: Option<f64>,
}

/// The longest decoder or codec name kept: they come from libwebrtc and the
/// SDP, which the SFU writes (D14).
const MAX_NAME: usize = 48;

/// libwebrtc's running totals for the watched video, at one moment.
#[derive(Default)]
struct Inbound {
    codec: String,
    decoder: String,
    hardware: bool,
    size: (u32, u32),
    fps: f64,
    bytes: u64,
    packets: u64,
    lost: i64,
    decoded: u32,
    keyframes: u32,
    dropped: u32,
    qp_sum: u64,
    decode_s: f64,
    buffer_s: f64,
    buffered: u64,
    jitter_s: f64,
    plis: u32,
    nacks: u32,
    freezes: u32,
    rtt_s: Option<f64>,
}

impl Inbound {
    /// From a receiver's stats: its one video inbound-rtp, its codec, and the
    /// transport's round trip.
    fn read(stats: &[RtcStats]) -> Option<Self> {
        let i = stats.iter().find_map(|s| match s {
            RtcStats::InboundRtp(i) if i.stream.kind == "video" => Some(i),
            _ => None,
        })?;
        let codec = stats.iter().find_map(|s| match s {
            RtcStats::Codec(c) if c.rtc.id == i.stream.codec_id => Some(&c.codec.mime_type),
            _ => None,
        });
        let codec = codec.map_or("", |m| m.strip_prefix("video/").unwrap_or(m));
        let rtt_s = stats.iter().find_map(|s| match s {
            RtcStats::CandidatePair(c)
                if c.candidate_pair.nominated && c.candidate_pair.responses_received > 0 =>
            {
                Some(c.candidate_pair.current_round_trip_time)
            }
            _ => None,
        });
        let v = &i.inbound;
        Some(Self {
            codec: codec.chars().take(MAX_NAME).collect(),
            decoder: v.decoder_implementation.chars().take(MAX_NAME).collect(),
            hardware: v.power_efficient_decoder,
            size: (v.frame_width, v.frame_height),
            fps: v.frames_per_second,
            bytes: v.bytes_received,
            packets: i.received.packets_received,
            lost: i.received.packets_lost,
            decoded: v.frames_decoded,
            keyframes: v.key_frames_decoded,
            dropped: v.frames_dropped,
            qp_sum: v.qp_sum,
            decode_s: v.total_decode_time,
            buffer_s: v.jitter_buffer_delay,
            buffered: v.jitter_buffer_emitted_count,
            jitter_s: i.received.jitter,
            plis: v.pli_count,
            nacks: v.nack_count,
            freezes: v.freeze_count,
            rtt_s,
        })
    }

    /// The second from `then` to `self`, `secs` long.
    fn since(&self, then: &Self, secs: f64) -> StreamStats {
        let decoded = self.decoded.saturating_sub(then.decoded);
        let per_frame = |total: f64| {
            if decoded == 0 {
                0.0
            } else {
                total.max(0.0) * 1000.0 / f64::from(decoded)
            }
        };
        let received = self.packets.saturating_sub(then.packets);
        let lost = u64::try_from(self.lost.saturating_sub(then.lost)).unwrap_or(0);
        let expected = received.saturating_add(lost);
        let buffered = self.buffered.saturating_sub(then.buffered);
        StreamStats {
            codec: self.codec.clone(),
            decoder: self.decoder.clone(),
            hardware: self.hardware,
            width: self.size.0,
            height: self.size.1,
            fps: self.fps,
            kbps: self.bytes.saturating_sub(then.bytes) as f64 * 8.0 / 1000.0 / secs.max(0.1),
            qp: (decoded > 0 && self.qp_sum > 0)
                .then(|| self.qp_sum.saturating_sub(then.qp_sum) as f64 / f64::from(decoded)),
            lost_pct: if expected == 0 {
                0.0
            } else {
                lost as f64 * 100.0 / expected as f64
            },
            dropped: self.dropped.saturating_sub(then.dropped),
            decode_ms: per_frame(self.decode_s - then.decode_s),
            buffer_ms: if buffered == 0 {
                0.0
            } else {
                (self.buffer_s - then.buffer_s).max(0.0) * 1000.0 / buffered as f64
            },
            jitter_ms: self.jitter_s * 1000.0,
            keyframes: self.keyframes,
            plis: self.plis,
            nacks: self.nacks,
            freezes: self.freezes,
            rtt_ms: self.rtt_s.map(|s| s * 1000.0),
        }
    }
}

/// What went wrong in a second of watching, if anything: what a viewer saw
/// as a stutter. `freezes` and `plis`: new this second. The sender's log,
/// [`watch`], says whether it started there.
fn rough(s: &StreamStats, freezes: u32, plis: u32) -> Option<&'static str> {
    if freezes > 0 {
        Some("freeze")
    } else if plis > 0 {
        // We lost a frame we couldn't repair and asked for a keyframe.
        Some("pli")
    } else if s.lost_pct > 2.0 {
        Some("loss")
    } else if s.dropped > 0 {
        Some("dropped")
    } else if s.fps > 0.0 && s.decode_ms > 2000.0 / s.fps {
        Some("decode")
    } else if s.buffer_ms > 150.0 {
        // Frames waited this long for the ones before them: late packets.
        Some("buffer")
    } else {
        None
    }
}

/// Aborts the task when dropped, so the stats poller ends with its viewer.
pub(crate) struct AbortOnDrop(pub tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Once a second while `target` is what we watch: logs a second that went
/// wrong (see [`rough`]), and while the stats are shown, puts `track`'s
/// receive stats in `shared.stream_stats`.
pub(crate) async fn inbound(track: RemoteVideoTrack, target: (String, bool), shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last: Option<(Instant, Inbound)> = None;
    loop {
        tick.tick().await;
        if lock(&shared.watching).as_ref() != Some(&target) {
            last = None;
            continue;
        }
        let Ok(stats) = track.get_stats().await else {
            continue;
        };
        let Some(now) = Inbound::read(&stats) else {
            continue;
        };
        let at = Instant::now();
        if let Some((then_at, then)) = &last {
            let s = now.since(then, at.duration_since(*then_at).as_secs_f64());
            let (freezes, plis) = (
                now.freezes.saturating_sub(then.freezes),
                now.plis.saturating_sub(then.plis),
            );
            if let Some(why) = rough(&s, freezes, plis) {
                tracing::info!(
                    "watched {} rough ({why}): {:?} {}x{} at {:.0} fps, {:.0} kbps, lost {:.1}%, \
                     {} dropped, decode {:.1} ms, buffered {:.0} ms, jitter {:.0} ms, \
                     {freezes} freeze {plis} pli, rtt {:.0} ms, decoder {:?} {}",
                    if target.1 { "camera" } else { "stream" },
                    s.codec,
                    s.width,
                    s.height,
                    s.fps,
                    s.kbps,
                    s.lost_pct,
                    s.dropped,
                    s.decode_ms,
                    s.buffer_ms,
                    s.jitter_ms,
                    s.rtt_ms.unwrap_or(0.0),
                    s.decoder,
                    if s.hardware {
                        "(hardware)"
                    } else {
                        "(software)"
                    },
                );
            }
            if !shared.stats_wanted.load(Relaxed) {
                last = Some((at, now));
                continue;
            }
            let mut slot = lock(&shared.stream_stats);
            // Checked again under the lock, like `room::watch` clears it.
            if lock(&shared.watching).as_ref() != Some(&target) {
                continue;
            }
            *slot = Some((target.clone(), s));
            drop(slot);
            (shared.notify)();
        }
        last = Some((at, now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_second() {
        let then = Inbound {
            bytes: 0,
            packets: 1000,
            lost: 10,
            decoded: 100,
            qp_sum: 3000,
            decode_s: 0.2,
            buffer_s: 1.0,
            buffered: 100,
            ..Inbound::default()
        };
        let now = Inbound {
            bytes: 1_000_000,
            packets: 1990,
            lost: 20,
            decoded: 160,
            qp_sum: 4800,
            decode_s: 0.32,
            buffer_s: 4.0,
            buffered: 160,
            ..Inbound::default()
        };
        let s = now.since(&then, 1.0);
        assert_eq!(s.kbps, 8000.0);
        assert_eq!(s.qp, Some(30.0));
        assert!((s.lost_pct - 1.0).abs() < 1e-9);
        assert!((s.decode_ms - 2.0).abs() < 1e-9);
        assert!((s.buffer_ms - 50.0).abs() < 1e-9);
        // Counters that went backwards (a new stream) don't underflow.
        let s = then.since(&now, 1.0);
        assert_eq!((s.kbps, s.lost_pct, s.decode_ms), (0.0, 0.0, 0.0));
    }

    /// A healthy second of a 60 fps share at 8 Mbps.
    fn smooth() -> Second {
        Second {
            captured: 60,
            gap_ms: 17,
            work_ms: 3,
            source: 60,
            encoded: 60,
            sent: 60,
            encode_ms: 4.0,
            queue_ms: 5.0,
            kbps: 7900.0,
            target_kbps: 8000.0,
            available_kbps: 9000.0,
            rtt_ms: 20.0,
            ..Second::default()
        }
    }

    #[test]
    fn rough_seconds() {
        let fine = StreamStats {
            fps: 60.0,
            decode_ms: 3.0,
            buffer_ms: 40.0,
            lost_pct: 0.5,
            ..StreamStats::default()
        };
        assert_eq!(rough(&fine, 0, 0), None);
        assert_eq!(rough(&fine, 1, 0), Some("freeze"));
        assert_eq!(rough(&fine, 0, 1), Some("pli"));
        let late = StreamStats {
            buffer_ms: 400.0,
            ..fine.clone()
        };
        assert_eq!(rough(&late, 0, 0), Some("buffer"));
        let slow = StreamStats {
            decode_ms: 40.0,
            ..fine
        };
        assert_eq!(rough(&slow, 0, 0), Some("decode"));
    }

    #[test]
    fn places_the_stage() {
        assert_eq!(smooth().behind(60, 60), None);
        // A still screen, on a capturer that only delivers changes.
        let still = Second {
            captured: 1,
            gap_ms: 5000,
            source: 1,
            encoded: 1,
            sent: 1,
            kbps: 40.0,
            ..smooth()
        };
        assert_eq!(still.behind(60, 1), None);
        // ...but a moving screen that stalls is.
        assert_eq!(still.behind(60, 60), Some("capture"));
        // Frames trickling in, from the log that prompted this check.
        let trickle = Second {
            captured: 16,
            gap_ms: 300,
            work_ms: 23,
            ..smooth()
        };
        assert_eq!(trickle.behind(60, 13), Some("capture"));
        // A frame or two missed, as a game's own pacing does.
        let jitter = Second {
            captured: 50,
            gap_ms: 36,
            ..smooth()
        };
        assert_eq!(jitter.behind(60, 50), None);

        let stage = |s: Second| s.behind(60, 60);
        let s = smooth();
        assert_eq!(
            stage(Second {
                captured: 30,
                gap_ms: 480,
                ..smooth()
            }),
            Some("capture")
        );
        assert_eq!(
            stage(Second {
                work_ms: 40,
                ..smooth()
            }),
            Some("convert")
        );
        assert_eq!(
            stage(Second {
                encoded: 20,
                sent: 20,
                ..smooth()
            }),
            Some("encoder")
        );
        assert_eq!(
            stage(Second {
                encode_ms: 30.0,
                ..smooth()
            }),
            Some("encoder")
        );
        assert_eq!(
            stage(Second {
                kbps: 2.0 * s.target_kbps,
                ..smooth()
            }),
            Some("pacer")
        );
        assert_eq!(
            stage(Second {
                queue_ms: 800.0,
                ..smooth()
            }),
            Some("pacer")
        );
        assert_eq!(stage(Second { pli: 1, ..smooth() }), Some("network"));
        assert_eq!(
            stage(Second {
                available_kbps: 3000.0,
                ..smooth()
            }),
            Some("network")
        );
    }
}
