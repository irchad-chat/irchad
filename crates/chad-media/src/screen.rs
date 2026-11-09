//! Screen share: a capture backend ([`ScreenSource`]) feeds BGRA/RGBA frames,
//! we convert them to I420, scale them to the preset and publish one video
//! track without simulcast, plus an app's audio if one was picked. Watching
//! someone else's share decodes into [`ScreenFrame`]s for the UI.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use livekit::options::{
    DegradationPreference, TrackPublishOptions, VideoCodec, VideoEncoderBackend, VideoEncoding,
};
use livekit::prelude::*;
use livekit::track::{LocalAudioTrack, LocalTrack, LocalVideoTrack, TrackSource};
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::{
    AudioSourceOptions, RtcAudioSource, native::NativeAudioSource,
};
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::stats::RtcStats;
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution, native::NativeVideoSource};
use serde::{Deserialize, Serialize};

use crate::engine::{PREVIEW_SCREEN, Shared, lock};
use crate::{RATE, Voice};

/// Bigger than any real screen; D14 for what peers send us, sanity for what
/// a capture backend hands over.
pub(crate) const MAX_DIM: u32 = 8192;
/// `livekit.TrackSource.SCREEN_SHARE` in the protocol; the SDK doesn't export the enum.
const PROTO_SCREEN_SHARE: i32 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenSettings {
    pub preset: Preset,
    pub hint: ContentHint,
    /// Encoder ceiling.
    pub max_kbps: u32,
}

impl Default for ScreenSettings {
    fn default() -> Self {
        Self {
            preset: Preset::P1080p60,
            hint: ContentHint::Motion,
            max_kbps: 8000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    P720p30,
    P1080p60,
    /// Native size, up to 60 fps.
    Source,
}

impl Preset {
    pub fn label(self) -> &'static str {
        match self {
            Self::P720p30 => "720p 30 fps",
            Self::P1080p60 => "1080p 60 fps",
            Self::Source => "Source",
        }
    }

    fn bounds(self) -> Option<(u32, u32)> {
        match self {
            Self::P720p30 => Some((1280, 720)),
            Self::P1080p60 => Some((1920, 1080)),
            Self::Source => None,
        }
    }

    pub(crate) fn fps(self) -> u32 {
        match self {
            Self::P720p30 => 30,
            _ => 60,
        }
    }
}

/// What the encoder gives up first when bandwidth runs out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentHint {
    /// Text and UI: keep resolution, drop frames.
    Detail,
    /// Games and video: keep the frame rate, drop resolution.
    Motion,
}

/// One screen or window a [`ScreenSource`] can capture without the portal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareSource {
    pub id: SourceId,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceId {
    pub window: bool,
    pub id: u64,
}

/// What can be shared here.
#[derive(Debug, Clone, Default)]
pub struct Targets {
    /// The ScreenCast portal works (Wayland, and X11 on desktops that serve it).
    pub portal: bool,
    /// Screens and windows for our own picker (X11, Windows).
    pub sources: Vec<ShareSource>,
}

#[derive(Debug, Clone)]
pub enum Target {
    /// The portal's own picker; skipped while `restore_token` still holds.
    Portal {
        restore_token: Option<String>,
    },
    Source(SourceId),
    /// A moving 1080p test pattern, for checks without a desktop.
    #[doc(hidden)]
    Pattern,
}

/// An app playing audio, for screen share audio. Never ourselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioApp {
    /// PipeWire `object.serial`, or a process id on Windows.
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// B, G, R, A/x bytes (libyuv "ARGB").
    Bgra,
    /// R, G, B, A/x bytes (libyuv "ABGR"). Only PipeWire produces it.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Rgba,
    /// Y0, U, Y1, V: what most webcams send uncompressed. Only PipeWire produces it.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Yuy2,
    /// The Y plane, then interleaved U, V at half size, both `stride` wide.
    Nv12,
}

/// A captured frame. `stride` is the bytes per row (of the Y plane for NV12).
pub struct Frame<'a> {
    pub data: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
}

impl Frame<'_> {
    /// Sizes that libyuv can read without running off the end.
    pub(crate) fn valid(&self) -> bool {
        if !(1..=MAX_DIM).contains(&self.width) || !(1..=MAX_DIM).contains(&self.height) {
            return false;
        }
        // YUV rows hold whole pixel pairs.
        let row = match self.format {
            PixelFormat::Bgra | PixelFormat::Rgba => self.width * 4,
            PixelFormat::Yuy2 => self.width.next_multiple_of(2) * 2,
            PixelFormat::Nv12 => self.width.next_multiple_of(2),
        };
        self.stride >= row && self.data.len() >= self.len()
    }

    /// The bytes [`Frame::valid`] vouched for.
    fn len(&self) -> usize {
        let rows = match self.format {
            PixelFormat::Nv12 => self.height + self.height.div_ceil(2),
            _ => self.height,
        };
        self.stride as usize * rows as usize
    }
}

pub type FrameSink = Box<dyn FnMut(Frame<'_>) + Send>;
/// Called once if the capture stops by itself (window closed, portal revoked).
pub type OnEnd = Box<dyn FnOnce() + Send>;

/// A running capture. Dropping it stops capturing.
pub trait ScreenSource: Send {}

/// A share in progress, owned by [`crate::engine::Shared`].
pub(crate) struct Share {
    pub id: u64,
    pub _capture: Box<dyn ScreenSource>,
    pub room: Arc<Room>,
    pub tracks: Vec<TrackSid>,
    /// Capture threads end shares too, outside the runtime.
    pub rt: tokio::runtime::Handle,
}

impl Drop for Share {
    fn drop(&mut self) {
        let (room, tracks) = (self.room.clone(), std::mem::take(&mut self.tracks));
        self.rt.spawn(async move {
            for sid in tracks {
                let _ = room.local_participant().unpublish_track(&sid).await;
            }
        });
    }
}

/// A decoded frame of the share being watched, I420 with its strides.
pub struct ScreenFrame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub strides: [u32; 3],
    /// Bumped per frame, so the UI uploads each one once.
    pub seq: u64,
}

pub(crate) async fn start(
    voice: &Voice,
    target: Target,
    app: Option<String>,
    settings: ScreenSettings,
) -> Result<Option<String>, String> {
    let shared = &voice.shared;
    let room = lock(&shared.room)
        .clone()
        .ok_or("Join voice to share your screen.")?;
    // LiveKit doesn't answer a publish it won't allow; it just times out.
    let allowed = room.local_participant().permission().is_none_or(|p| {
        p.can_publish
            && (p.can_publish_sources.is_empty()
                || p.can_publish_sources.contains(&PROTO_SCREEN_SHARE))
    });
    if !allowed {
        return Err(
            "This server doesn't allow screen sharing. Rejoin voice after it's updated.".into(),
        );
    }
    voice.stop_share();
    let id = shared.next_share();

    let fps = settings.preset.fps();
    let nominal = settings.preset.bounds().unwrap_or((1920, 1080));
    let source = NativeVideoSource::new(
        VideoResolution {
            width: nominal.0,
            height: nominal.1,
        },
        true,
    );
    let sink = encoder(
        source.clone(),
        settings.preset,
        shared.clone(),
        PREVIEW_SCREEN,
    );
    let v = voice.clone();
    let on_end: OnEnd = Box::new(move || v.end_share(id, false));
    let (capture, token): (Box<dyn ScreenSource>, Option<String>) = match target {
        #[cfg(target_os = "linux")]
        Target::Portal { restore_token } => {
            crate::platform::portal::start(restore_token, fps, sink, on_end).await?
        }
        #[cfg(not(target_os = "linux"))]
        Target::Portal { .. } => return Err("Screen sharing isn't available here.".into()),
        Target::Source(s) => (crate::platform::desktop::start(s, fps, sink, on_end)?, None),
        Target::Pattern => (pattern(fps, sink), None),
    };

    let (codec, backend) = codec();
    let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source));
    let publication = room
        .local_participant()
        .publish_track(
            LocalTrack::Video(track),
            TrackPublishOptions {
                source: TrackSource::Screenshare,
                simulcast: false,
                video_codec: codec,
                video_encoder: backend,
                video_encoding: Some(VideoEncoding {
                    max_bitrate: u64::from(settings.max_kbps.clamp(500, 50_000)) * 1000,
                    max_framerate: f64::from(fps),
                }),
                degradation_preference: Some(match settings.hint {
                    ContentHint::Detail => DegradationPreference::MaintainResolution,
                    ContentHint::Motion => DegradationPreference::MaintainFramerate,
                }),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| {
            tracing::warn!("publishing the screen: {e}");
            "Couldn't share your screen.".to_owned()
        })?;
    let mut tracks = vec![publication.sid()];
    tracing::info!("screen share: {codec:?}, asked for the {backend:?} encoder");
    log_encoder(room.clone());

    if let Some(app) = app {
        let source = NativeAudioSource::new(
            AudioSourceOptions {
                echo_cancellation: false,
                noise_suppression: false,
                auto_gain_control: false,
            },
            RATE,
            2,
            0,
        );
        let track = LocalAudioTrack::create_audio_track(
            "screen-audio",
            RtcAudioSource::Native(source.clone()),
        );
        match room
            .local_participant()
            .publish_track(
                LocalTrack::Audio(track),
                TrackPublishOptions {
                    source: TrackSource::ScreenshareAudio,
                    dtx: false,
                    red: false,
                    audio_encoding: Some(livekit::options::AudioEncoding {
                        max_bitrate: 128_000,
                    }),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(p) => {
                tracks.push(p.sid());
                voice.backend.capture_app(Some((app, app_sink(source))));
            }
            Err(e) => tracing::warn!("publishing app audio: {e}"),
        }
    }

    shared.set_share(
        Share {
            id,
            _capture: capture,
            room,
            tracks,
            rt: tokio::runtime::Handle::current(),
        },
        false,
    );
    Ok(token)
}

struct Pattern(Arc<std::sync::atomic::AtomicBool>);

impl ScreenSource for Pattern {}

impl Drop for Pattern {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Scrolling bars with a moving block: busy enough to load the encoder.
fn pattern(fps: u32, mut sink: FrameSink) -> Box<dyn ScreenSource> {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let st = stop.clone();
    let _ = std::thread::Builder::new()
        .name("irchad-pattern".into())
        .spawn(move || {
            let (w, h) = (1920u32, 1080u32);
            let mut data = vec![0u8; (w * h * 4) as usize];
            let interval = Duration::from_micros(1_000_000 / u64::from(fps));
            let mut t = 0u32;
            while !st.load(std::sync::atomic::Ordering::Relaxed) {
                let start = Instant::now();
                for (i, px) in data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let (x, y) = (i as u32 % w, i as u32 / w);
                    let block = (x + 1920 - t * 8 % 1920) % 1920 < 200 && y % 400 < 200;
                    let v = if block {
                        255
                    } else {
                        ((x + t * 4) % 256) as u8
                    };
                    *px = [v, (y % 256) as u8, (t % 256) as u8, 255];
                }
                sink(Frame {
                    data: &data,
                    width: w,
                    height: h,
                    stride: w * 4,
                    format: PixelFormat::Bgra,
                });
                t += 1;
                std::thread::sleep(interval.saturating_sub(start.elapsed()));
            }
        });
    Box::new(Pattern(stop))
}

/// Captured frames → I420 at the preset's size and rate → the video source,
/// and to our own preview while it's open. `kind`: which preview that is.
pub(crate) fn encoder(
    source: NativeVideoSource,
    preset: Preset,
    shared: Arc<Shared>,
    kind: u8,
) -> FrameSink {
    let interval = Duration::from_micros(1_000_000 / u64::from(preset.fps()));
    let mut next = Instant::now();
    let mut seq = 0;
    Box::new(move |f: Frame<'_>| {
        let now = Instant::now();
        // A little slack, so a 60 Hz source isn't cut to 30.
        if now + interval / 4 < next || !f.valid() {
            return;
        }
        next = next.max(now) + interval;
        let mut i420 = I420Buffer::new(f.width, f.height);
        let (sy, su, sv) = i420.strides();
        let (y, u, v) = i420.data_mut();
        let (w, h) = (f.width as i32, f.height as i32);
        let data = &f.data[..f.len()];
        match f.format {
            PixelFormat::Bgra => {
                yuv_helper::argb_to_i420(data, f.stride, y, sy, u, su, v, sv, w, h);
            }
            PixelFormat::Rgba => {
                yuv_helper::abgr_to_i420(data, f.stride, y, sy, u, su, v, sv, w, h);
            }
            PixelFormat::Nv12 => {
                let (py, puv) = data.split_at(f.stride as usize * f.height as usize);
                yuv_helper::nv12_to_i420(py, f.stride, puv, f.stride, y, sy, u, su, v, sv, w, h);
            }
            PixelFormat::Yuy2 => yuy2_to_i420(&f, [y, u, v], [sy, su, sv]),
        }
        if let Some((w, h)) = fit(f.width, f.height, preset.bounds()) {
            i420 = i420.scale(w as i32, h as i32);
        }
        if shared.preview.load(Relaxed) == kind {
            seq += 1;
            preview(&shared, &i420, seq, kind);
        }
        source.capture_frame(&VideoFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: 0,
            frame_metadata: None,
            buffer: i420,
        });
    })
}

/// Our own share as we send it, for the preview.
fn preview(shared: &Shared, b: &I420Buffer, seq: u64, kind: u8) {
    let (sy, su, sv) = b.strides();
    let (y, u, v) = b.data();
    let frame = Arc::new(ScreenFrame {
        width: b.width(),
        height: b.height(),
        y: y.to_vec(),
        u: u.to_vec(),
        v: v.to_vec(),
        strides: [sy, su, sv],
        seq,
    });
    let mut screen = lock(&shared.screen);
    // Checked again under the lock: watching someone else turns it off first.
    if shared.preview.load(Relaxed) == kind {
        *screen = Some(frame);
    }
    drop(screen);
    (shared.notify)();
}

/// libyuv's YUY2 → I420 isn't exported, so by hand: chroma from even rows.
/// `f` is [`Frame::valid`].
fn yuy2_to_i420(f: &Frame<'_>, [y, u, v]: [&mut [u8]; 3], [sy, su, sv]: [u32; 3]) {
    let (w, stride) = (f.width as usize, f.stride as usize);
    for row in 0..f.height as usize {
        let src = &f.data[row * stride..][..w.next_multiple_of(2) * 2];
        let dst = &mut y[row * sy as usize..][..w];
        for (d, s) in dst.iter_mut().zip(src.iter().step_by(2)) {
            *d = *s;
        }
        if row % 2 == 0 {
            let (cu, cv) = (
                &mut u[row / 2 * su as usize..][..w.div_ceil(2)],
                &mut v[row / 2 * sv as usize..][..w.div_ceil(2)],
            );
            for ((px, cu), cv) in src.as_chunks::<4>().0.iter().zip(cu).zip(cv) {
                (*cu, *cv) = (px[1], px[3]);
            }
        }
    }
}

/// Hands an NV12 buffer to the sink. Its UV plane may start below a Y plane
/// padded to 16 rows (1080 → 1088); then the planes are joined in `joined`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn nv12(
    data: &[u8],
    w: u32,
    h: u32,
    pitch: u32,
    joined: &mut Vec<u8>,
    sink: &mut FrameSink,
) {
    let frame = |data| Frame {
        data,
        width: w,
        height: h,
        stride: pitch,
        format: PixelFormat::Nv12,
    };
    let (y, uv) = (
        pitch as usize * h as usize,
        pitch as usize * h.div_ceil(2) as usize,
    );
    let padded = pitch as usize * h.next_multiple_of(16) as usize;
    if padded != y && data.len() >= padded + uv {
        joined.clear();
        joined.extend_from_slice(&data[..y]);
        joined.extend_from_slice(&data[padded..padded + uv]);
        sink(frame(joined));
    } else {
        // `Frame::valid` (in the encoder) checks the length.
        sink(frame(data));
    }
}

/// The size to scale to so `w`×`h` fits `bounds` (even, aspect kept), if smaller.
fn fit(w: u32, h: u32, bounds: Option<(u32, u32)>) -> Option<(u32, u32)> {
    let (bw, bh) = bounds?;
    if w <= bw && h <= bh {
        return None;
    }
    let k = (f64::from(bw) / f64::from(w)).min(f64::from(bh) / f64::from(h));
    let even = |x: f64| ((x as u32) & !1).max(2);
    Some((even(f64::from(w) * k), even(f64::from(h) * k)))
}

/// Interleaved stereo from the app capture → 10 ms frames for LiveKit.
pub type AppSink = Box<dyn FnMut(&[i16]) + Send>;

fn app_sink(source: NativeAudioSource) -> AppSink {
    const TEN_MS: usize = RATE as usize / 100 * 2;
    let mut buf = Vec::with_capacity(TEN_MS * 2);
    Box::new(move |samples: &[i16]| {
        buf.extend_from_slice(samples);
        let (chunks, rest) = buf.as_chunks::<TEN_MS>();
        for chunk in chunks {
            let f = AudioFrame {
                data: chunk.into(),
                sample_rate: RATE,
                num_channels: 2,
                samples_per_channel: (TEN_MS / 2) as u32,
            };
            // Unbuffered source (queue 0): this completes synchronously.
            if let Some(Err(e)) = source.capture_frame(&f).now_or_never() {
                tracing::warn!("sending app audio: {e:?}");
            }
        }
        let rest = rest.len();
        buf.drain(..buf.len() - rest);
    })
}

/// Hardware encoder by GPU vendor (`docs/decisions.md` D23). The SDK falls
/// back to software if the one we ask for isn't there; [`log_encoder`] says.
fn codec() -> (VideoCodec, VideoEncoderBackend) {
    match gpu_vendor() {
        Some(0x10de) => (VideoCodec::AV1, VideoEncoderBackend::Nvenc),
        Some(0x1002 | 0x8086) => (VideoCodec::H264, VideoEncoderBackend::Vaapi),
        _ => (VideoCodec::VP9, VideoEncoderBackend::Software),
    }
}

/// PCI vendor of the first render node that's a GPU we know.
#[cfg(target_os = "linux")]
fn gpu_vendor() -> Option<u32> {
    let mut nodes: Vec<_> = std::fs::read_dir("/sys/class/drm")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("renderD"))
        })
        .collect();
    nodes.sort();
    nodes.iter().find_map(|p| {
        let v = std::fs::read_to_string(p.join("device/vendor")).ok()?;
        let v = u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok()?;
        matches!(v, 0x10de | 0x1002 | 0x8086).then_some(v)
    })
}

// shortcut: no GPU probe off Linux, so Windows encodes VP9 in software; use
// DXGI adapter vendor ids if NVENC builds there.
#[cfg(not(target_os = "linux"))]
fn gpu_vendor() -> Option<u32> {
    None
}

/// Logs which encoder libwebrtc actually picked, once it's running.
fn log_encoder(room: Arc<Room>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let Ok(stats) = room.get_stats().await else {
            return;
        };
        for s in &stats.publisher_stats {
            if let RtcStats::OutboundRtp(o) = s
                && o.stream.kind == "video"
            {
                tracing::info!(
                    "screen share encoder: {} (hardware: {}), {}x{} at {:.0} fps",
                    o.outbound.encoder_implementation,
                    o.outbound.power_efficient_encoder,
                    o.outbound.frame_width,
                    o.outbound.frame_height,
                    o.outbound.frames_per_second,
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_presets() {
        let b = Preset::P1080p60.bounds();
        assert_eq!(fit(1920, 1080, b), None);
        assert_eq!(fit(3840, 2160, b), Some((1920, 1080)));
        assert_eq!(fit(2560, 1440, Preset::P720p30.bounds()), Some((1280, 720)));
        // Ultrawide: width-bound.
        assert_eq!(fit(3440, 1440, b), Some((1920, 802)));
        assert_eq!(fit(5000, 1000, Preset::Source.bounds()), None);
    }

    #[test]
    fn nv12_skips_padding() {
        // 2x18: Y padded to 32 rows, then 9 UV rows.
        let (w, h, pitch) = (2u32, 18u32, 2u32);
        let mut data = vec![1u8; (pitch * 32) as usize];
        data.extend(vec![7u8; (pitch * 9) as usize]);
        let got = Arc::new(std::sync::Mutex::new(Vec::new()));
        let g = got.clone();
        let mut sink: FrameSink = Box::new(move |f: Frame<'_>| {
            assert!(f.valid());
            *g.lock().unwrap() = f.data[f.data.len() - 2..].to_vec();
        });
        nv12(&data, w, h, pitch, &mut Vec::new(), &mut sink);
        assert_eq!(*got.lock().unwrap(), vec![7, 7]);
    }

    #[test]
    fn yuy2_converts() {
        // 3x2: odd width, so the last pixel pair is half padding.
        let row = [10, 100, 20, 200, 30, 101, 0, 201];
        let data = [row, row.map(|b| b + 1)].concat();
        let f = Frame {
            data: &data,
            width: 3,
            height: 2,
            stride: 8,
            format: PixelFormat::Yuy2,
        };
        assert!(f.valid());
        let mut b = I420Buffer::new(3, 2);
        let (sy, su, sv) = b.strides();
        let (y, u, v) = b.data_mut();
        yuy2_to_i420(&f, [y, u, v], [sy, su, sv]);
        let (y, u, v) = b.data();
        assert_eq!(&y[..3], &[10, 20, 30]);
        assert_eq!(&y[sy as usize..][..3], &[11, 21, 31]);
        assert_eq!(&u[..2], &[100, 101]);
        assert_eq!(&v[..2], &[200, 201]);
        let short = Frame {
            data: &data[..15],
            ..f
        };
        assert!(!short.valid());
    }

    #[test]
    fn rejects_short_frames() {
        let data = vec![0u8; 16 * 4 * 4];
        let f = |w, h, stride, n| Frame {
            data: &data[..n],
            width: w,
            height: h,
            stride,
            format: PixelFormat::Bgra,
        };
        assert!(f(16, 4, 64, 256).valid());
        assert!(!f(16, 4, 64, 255).valid());
        assert!(!f(17, 4, 64, 256).valid());
        assert!(!f(0, 4, 64, 256).valid());
        assert!(!f(MAX_DIM + 1, 1, u32::MAX, 256).valid());
    }
}

/// The copy and conversion costs in `docs/notes/screenshare-perf.md`.
/// `cargo test --release -p chad-media convert_cost -- --ignored --nocapture`
#[cfg(test)]
mod perf {
    use super::*;

    #[test]
    #[ignore]
    fn convert_cost() {
        let per_frame = |t: Instant| t.elapsed().as_secs_f64() * 1000.0 / 60.0;
        for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
            let data = vec![77u8; (w * h * 4) as usize];
            let t = Instant::now();
            for _ in 0..60 {
                let mut i420 = I420Buffer::new(w, h);
                let (sy, su, sv) = i420.strides();
                let (y, u, v) = i420.data_mut();
                yuv_helper::argb_to_i420(&data, w * 4, y, sy, u, su, v, sv, w as i32, h as i32);
                if let Some((fw, fh)) = fit(w, h, Preset::P1080p60.bounds()) {
                    i420 = i420.scale(fw as i32, fh as i32);
                }
                std::hint::black_box(i420);
            }
            println!(
                "{w}x{h} BGRA -> I420 at <= 1080p: {:.2} ms/frame",
                per_frame(t)
            );
            let mut copy = vec![0u8; data.len()];
            let t = Instant::now();
            for _ in 0..60 {
                copy.copy_from_slice(&data);
                std::hint::black_box(&copy);
            }
            println!("{w}x{h} frame copy: {:.2} ms/frame", per_frame(t));
        }
    }
}
