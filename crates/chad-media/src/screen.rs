//! Screen share: a capture backend ([`ScreenSource`]) feeds BGRA/RGBA frames,
//! we convert them to I420, scale them to the preset and publish one video
//! track without simulcast, plus an app's audio if one was picked. Watching
//! someone else's share decodes into [`ScreenFrame`]s for the UI.

use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, OnceLock};
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
    pub encoder: Encoder,
}

impl Default for ScreenSettings {
    fn default() -> Self {
        Self {
            preset: Preset::P1080p60,
            hint: ContentHint::Motion,
            max_kbps: 8000,
            encoder: Encoder::Auto,
        }
    }
}

/// Which video encoder a share asks for (D92).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Encoder {
    /// The GPU's best: AV1 on NVENC or a Radeon (Windows) that has it, H.264
    /// on any other GPU encoder, else H.264 in software.
    #[default]
    Auto,
    /// H.264 on whatever encodes it, GPU first. Every browser plays it, and
    /// viewers decode it more cheaply than AV1.
    Compatible,
    /// H.264 on the CPU, for when a GPU encoder misbehaves.
    Software,
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
    /// Where the tracks went: the call's room, or a public stream's own (D87).
    pub room: Arc<Room>,
    pub tracks: Vec<TrackSid>,
    /// Capture threads end shares too, outside the runtime.
    pub rt: tokio::runtime::Handle,
    /// A public stream's: the call's room, and whether the call's audio goes too.
    pub public: Option<(Arc<Room>, bool)>,
}

impl Drop for Share {
    fn drop(&mut self) {
        let (room, tracks) = (self.room.clone(), std::mem::take(&mut self.tracks));
        let public = self.public.take();
        crate::room::closing(self.rt.spawn(async move {
            let Some((call, _)) = public else {
                for sid in tracks {
                    let _ = room.local_participant().unpublish_track(&sid).await;
                }
                return;
            };
            let _ = room.close().await;
            let off = HashMap::from([
                (STREAM.to_owned(), String::new()),
                (STREAM_AUDIO.to_owned(), String::new()),
            ]);
            if let Err(e) = call.local_participant().set_attributes(off).await {
                tracing::debug!("clearing the stream attributes: {e}");
            }
        }));
    }
}

/// Where to send a share that gets a public link (D87), instead of the call.
#[derive(Debug, Clone)]
pub struct Public {
    pub livekit: String,
    /// chadd's publish-only token for `room`.
    pub token: String,
    pub room: String,
    /// Mix the call's voices (and ours) into the stream too.
    pub call_audio: bool,
}

/// Attributes on the streamer in the call's room: the stream's room, and "1"
/// when the call's audio goes out with it.
pub(crate) const STREAM: &str = "stream";
pub(crate) const STREAM_AUDIO: &str = "stream_audio";
/// The call-audio track's name in a stream's room; call members skip it.
pub(crate) const CALL_AUDIO: &str = "call-audio";

/// Whether `room` names a stream's room: what chadd makes, `stream-<code>`.
// shortcut: mirrors chad_proto::stream::code_of_room; chad-media doesn't depend on chad-proto.
pub(crate) fn stream_room(room: &str) -> bool {
    room.strip_prefix("stream-").is_some_and(|c| {
        c.len() == 36
            && c.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'v').contains(&b))
    })
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
    public: Option<Public>,
) -> Result<Option<String>, String> {
    let shared = &voice.shared;
    let call = lock(&shared.room)
        .clone()
        .ok_or("Join voice to share your screen.")?;
    if public.as_ref().is_some_and(|p| !stream_room(&p.room)) {
        return Err("The server sent something odd. Try again later.".into());
    }
    // LiveKit doesn't answer a publish it won't allow; it just times out.
    let allowed = call.local_participant().permission().is_none_or(|p| {
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
    let watched = Arc::new(crate::monitor::Capture::default());
    let sink = encoder(
        source.clone(),
        settings.preset,
        shared.clone(),
        PREVIEW_SCREEN,
        Some(watched.clone()),
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

    // A public share goes only to its stream's room, which call members
    // watch too: one encode, one upload (D87).
    let room = match &public {
        None => call.clone(),
        Some(p) => {
            let mut options = RoomOptions::default();
            options.auto_subscribe = false;
            let (room, events) =
                Room::connect(&p.livekit, &p.token, options)
                    .await
                    .map_err(|e| {
                        tracing::warn!("stream connect: {e}");
                        "Couldn't start the stream.".to_owned()
                    })?;
            let v = voice.clone();
            tokio::spawn(stream_ended(events, move || v.end_share(id, false)));
            Arc::new(room)
        }
    };
    let close = |room: Arc<Room>| {
        if public.is_some() {
            crate::room::closing(tokio::spawn(async move {
                let _ = room.close().await;
            }));
        }
    };

    let (codec, backend) = codec(settings.encoder);
    let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source));
    let track_id = track.rtc_track().id();
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
        .await;
    let publication = match publication {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("publishing the screen: {e}");
            close(room);
            return Err("Couldn't share your screen.".to_owned());
        }
    };
    let mut tracks = vec![publication.sid()];
    tracing::info!("screen share: {codec:?}, asked for the {backend:?} encoder");
    // The sink holds the only strong reference, so this ends with the capture.
    crate::monitor::watch(room.clone(), Arc::downgrade(&watched), track_id, fps);
    drop(watched);

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

    let mut call_audio = false;
    if let Some(p) = &public {
        if p.call_audio {
            call_audio = publish_call_audio(shared, &room, id).await;
        }
        let attrs = HashMap::from([
            (STREAM.to_owned(), p.room.clone()),
            (
                STREAM_AUDIO.to_owned(),
                if call_audio { "1" } else { "" }.to_owned(),
            ),
        ]);
        if let Err(e) = call.local_participant().set_attributes(attrs).await {
            // Without it, nobody in the call knows where to watch.
            tracing::warn!("setting the stream attributes: {e}");
            close(room);
            shared.set_call_mix(None, Some(id));
            return Err("Couldn't start the stream.".into());
        }
    }

    shared.set_share(
        Share {
            id,
            _capture: capture,
            room,
            tracks,
            rt: tokio::runtime::Handle::current(),
            public: public.map(|_| (call, call_audio)),
        },
        false,
    );
    Ok(token)
}

/// The call's mix as a track in the stream's room, fed by the DSP thread.
async fn publish_call_audio(shared: &Arc<Shared>, room: &Room, id: u64) -> bool {
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
    let track =
        LocalAudioTrack::create_audio_track(CALL_AUDIO, RtcAudioSource::Native(source.clone()));
    let published = room
        .local_participant()
        .publish_track(
            LocalTrack::Audio(track),
            TrackPublishOptions {
                source: TrackSource::Unknown,
                dtx: true,
                audio_encoding: Some(livekit::options::AudioEncoding {
                    max_bitrate: 96_000,
                }),
                ..Default::default()
            },
        )
        .await;
    match published {
        Ok(_) => {
            shared.set_call_mix(Some(source), Some(id));
            true
        }
        Err(e) => {
            tracing::warn!("publishing the call audio: {e}");
            false
        }
    }
}

/// Runs `end` once a stream's room is gone (we closed it, or the server did).
async fn stream_ended(
    mut events: tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
    end: impl FnOnce(),
) {
    while let Some(ev) = events.recv().await {
        if let RoomEvent::Disconnected { reason } = ev {
            tracing::info!("stream room closed: {reason:?}");
            break;
        }
    }
    end();
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
/// `watched`: a share's counters for [`crate::monitor`].
pub(crate) fn encoder(
    source: NativeVideoSource,
    preset: Preset,
    shared: Arc<Shared>,
    kind: u8,
    watched: Option<Arc<crate::monitor::Capture>>,
) -> FrameSink {
    let interval = Duration::from_micros(1_000_000 / u64::from(preset.fps()));
    let mut next = Instant::now();
    let mut last = Instant::now();
    let mut seq = 0;
    Box::new(move |f: Frame<'_>| {
        let now = Instant::now();
        // A little slack, so a 60 Hz source isn't cut to 30.
        if now + interval / 4 < next || !f.valid() {
            return;
        }
        next = next.max(now) + interval;
        let gap = now - last;
        last = now;
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
        if let Some(watched) = &watched {
            watched.frame(gap, now.elapsed());
        }
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

/// The GPU encoder [`Encoder::Auto`] uses, by what libwebrtc's probes find
/// (D92, D104): NVENC (AV1 on Ada and later, H.264 before), then VAAPI (Linux
/// only), then AMD's AMF (Windows only, where it is the generic `Hardware`
/// backend). Probed once, on first use: NVENC's probe wakes the GPU, which
/// shouldn't happen just because IrChad started.
fn hardware() -> Option<(VideoCodec, VideoEncoderBackend)> {
    static HW: OnceLock<Option<(VideoCodec, VideoEncoderBackend)>> = OnceLock::new();
    *HW.get_or_init(|| {
        let have: Vec<_> = VideoEncoderBackend::list_available().into_iter().collect();
        tracing::debug!("video encoders available: {have:?}");
        if have.contains(&VideoEncoderBackend::Nvenc) {
            // Pre-Ada cards have NVENC without AV1, and AV1 would then be libaom.
            let codec = if webrtc_sys::webrtc::ffi::nvenc_av1_supported() {
                VideoCodec::AV1
            } else {
                VideoCodec::H264
            };
            Some((codec, VideoEncoderBackend::Nvenc))
        } else if have.contains(&VideoEncoderBackend::Vaapi) {
            Some((VideoCodec::H264, VideoEncoderBackend::Vaapi))
        } else if webrtc_sys::webrtc::ffi::amf_supported() {
            // Older Radeons pad AV1 to sizes a viewer can't crop, so they
            // (and the ones without AV1) get H.264.
            let codec = if webrtc_sys::webrtc::ffi::amf_av1_supported() {
                VideoCodec::AV1
            } else {
                VideoCodec::H264
            };
            Some((codec, VideoEncoderBackend::Hardware))
        } else {
            None
        }
    })
}

/// What [`Encoder::Auto`] encodes with here, for the settings page. Blocks
/// on the first call while the GPU is probed.
pub fn hardware_encoder() -> Option<&'static str> {
    hardware().map(|hw| match hw {
        (VideoCodec::AV1, VideoEncoderBackend::Nvenc) => "NVIDIA NVENC, AV1",
        (_, VideoEncoderBackend::Nvenc) => "NVIDIA NVENC, H.264",
        (VideoCodec::AV1, _) => "AMD AMF, AV1",
        (_, VideoEncoderBackend::Hardware) => "AMD AMF, H.264",
        _ => "VAAPI, H.264",
    })
}

/// What received video decodes with here: NVDEC on an Nvidia driver, then
/// AMD's AMF decoder (Windows only), in the merged decoder factory's order.
/// Probed once, on first use, like [`hardware_encoder`].
pub fn hardware_decoder() -> Option<&'static str> {
    static HW: OnceLock<Option<&'static str>> = OnceLock::new();
    *HW.get_or_init(|| {
        let (nvdec, amf) = (
            webrtc_sys::webrtc::ffi::nvdec_supported(),
            webrtc_sys::webrtc::ffi::amf_decoder_supported(),
        );
        tracing::debug!("video decoders available: nvdec {nvdec}, amf {amf}");
        if nvdec {
            Some("NVIDIA NVDEC")
        } else if amf {
            Some("AMD AMF")
        } else {
            None
        }
    })
}

/// The codec and encoder a share asks for. The SDK falls back to software if
/// it can't open that encoder after all; [`crate::monitor`] logs which ran.
///
/// For debugging, `IRCHAD_SCREEN_ENCODER=software|vaapi|nvenc|amf` overrides the
/// backend (keeping the codec).
fn codec(choice: Encoder) -> (VideoCodec, VideoEncoderBackend) {
    let software = (VideoCodec::H264, VideoEncoderBackend::Software);
    let (codec, backend) = match choice {
        Encoder::Auto => hardware().unwrap_or(software),
        Encoder::Compatible => hardware().map_or(software, |(_, b)| (VideoCodec::H264, b)),
        Encoder::Software => software,
    };
    let backend = match std::env::var("IRCHAD_SCREEN_ENCODER").as_deref() {
        Ok("software") => VideoEncoderBackend::Software,
        Ok("nvenc") => VideoEncoderBackend::Nvenc,
        Ok("vaapi") => VideoEncoderBackend::Vaapi,
        Ok("amf") => VideoEncoderBackend::Hardware,
        _ => backend,
    };
    (codec, backend)
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
