//! Where a screen share falls behind. Once a second the capture counters and
//! libwebrtc's sender stats are compared, and a second in which a stage did
//! less than what fed it gets a log line naming the stage. A stutter someone
//! reports can then be placed from their exported log: capture, convert,
//! encoder, pacer or network. If none is named, it happened past our upload.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Weak};
use std::time::Duration;

use livekit::prelude::Room;
use livekit::webrtc::stats::{QualityLimitationReason, RtcStats};

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
    fn behind(&self, fps: u32) -> Option<&'static str> {
        let frame_ms = 1000.0 / f64::from(fps.max(1));
        let moving = self.captured >= fps / 4;
        if moving && self.gap_ms >= 400 {
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
                if let Some(stage) = s.behind(fps) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn places_the_stage() {
        assert_eq!(smooth().behind(60), None);
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
        assert_eq!(still.behind(60), None);

        let stage = |s: Second| s.behind(60);
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
