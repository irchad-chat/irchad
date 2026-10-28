//! Phase 0 spike: LiveKit Rust SDK against the dev stack.
//!
//! 1. "spike-sub" joins room `spike`, "spike-pub" joins and publishes a
//!    440 Hz sine via `NativeAudioSource`.
//! 2. spike-sub receives it via `NativeAudioStream`, checks it's still a
//!    440 Hz tone, and (Linux) plays it through a native PipeWire stream.
//! 3. Offline: libwebrtc's APM cancels a synthetic echo, fed with our own
//!    far-end reference, which is how AEC works when we own audio I/O.
//!
//! ```sh
//! SSL_CERT_FILE=<system bundle + Caddy root> \
//!   cargo run --release -p chad-media --example livekit_spike
//! ```
//!
//! Env: LIVEKIT_URL (default wss://localhost:8443/livekit), SPIKE_SECS (10).
//! Findings: docs/notes/livekit-sdk.md.

use std::{
    collections::VecDeque,
    f32::consts::TAU,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Context;
use futures_util::StreamExt;
use livekit::{
    options::TrackPublishOptions,
    prelude::*,
    track::{LocalAudioTrack, LocalTrack, RemoteTrack, TrackSource},
    webrtc::{
        audio_frame::AudioFrame,
        audio_source::{AudioSourceOptions, RtcAudioSource, native::NativeAudioSource},
        audio_stream::native::NativeAudioStream,
        native::apm::AudioProcessingModule,
    },
};
use livekit_api::access_token::{AccessToken, VideoGrants};

const RATE: u32 = 48_000;
const TONE_HZ: f32 = 440.0;
const FRAME: usize = (RATE / 100) as usize; // 10 ms

/// Shared playback buffer. A real implementation uses a lock-free SPSC ring.
type Playback = Arc<Mutex<VecDeque<i16>>>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,livekit_spike=info".into()),
        )
        .init();
    let url = std::env::var("LIVEKIT_URL").unwrap_or("wss://localhost:8443/livekit".into());
    let secs: u64 = std::env::var("SPIKE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    apm_check()?;

    let playback: Playback = Arc::default();
    #[cfg(target_os = "linux")]
    let pw_frames = pipewire_out::spawn(playback.clone());

    let (sub, mut sub_events) = Room::connect(&url, &token("spike-sub")?, RoomOptions::default())
        .await
        .context("spike-sub connect")?;
    println!("spike-sub connected to room {:?}", sub.name());

    let (publisher, _pub_events) =
        Room::connect(&url, &token("spike-pub")?, RoomOptions::default())
            .await
            .context("spike-pub connect")?;
    let source = NativeAudioSource::new(AudioSourceOptions::default(), RATE, 1, 100);
    let track = LocalAudioTrack::create_audio_track("tone", RtcAudioSource::Native(source.clone()));
    publisher
        .local_participant()
        .publish_track(
            LocalTrack::Audio(track),
            TrackPublishOptions {
                source: TrackSource::Microphone,
                ..Default::default()
            },
        )
        .await
        .context("publish")?;
    println!("spike-pub published 440 Hz tone");

    let tone = tokio::spawn(async move {
        let mut phase = 0.0f32;
        let mut buf = vec![0i16; FRAME];
        loop {
            for s in buf.iter_mut() {
                *s = (phase.sin() * 0.3 * i16::MAX as f32) as i16;
                phase = (phase + TAU * TONE_HZ / RATE as f32) % TAU;
            }
            let frame = AudioFrame {
                data: buf.as_slice().into(),
                sample_rate: RATE,
                num_channels: 1,
                samples_per_channel: FRAME as u32,
            };
            // capture_frame paces us: it waits while the source's queue is full.
            if source.capture_frame(&frame).await.is_err() {
                break;
            }
        }
    });

    let received = Arc::new(Mutex::new(Vec::<i16>::new()));
    let recv = received.clone();
    let pb = playback.clone();
    let rx = tokio::spawn(async move {
        while let Some(ev) = sub_events.recv().await {
            if let RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(t),
                participant,
                ..
            } = ev
            {
                println!(
                    "spike-sub subscribed to {:?} from {}",
                    t.name(),
                    participant.identity()
                );
                let mut stream = NativeAudioStream::new(t.rtc_track(), RATE as i32, 1);
                while let Some(frame) = stream.next().await {
                    recv.lock().unwrap().extend_from_slice(&frame.data);
                    let mut q = pb.lock().unwrap();
                    q.extend(frame.data.iter().copied());
                    // Bound latency: drop the oldest audio past 200 ms.
                    let excess = q.len().saturating_sub(RATE as usize / 5);
                    q.drain(..excess);
                }
            }
        }
    });

    tokio::time::sleep(Duration::from_secs(secs)).await;
    tone.abort();
    rx.abort();

    let samples = received.lock().unwrap().clone();
    let got_secs = samples.len() as f32 / RATE as f32;
    // Skip the first second (jitter buffer warm-up, Opus ramp-in).
    let tail = &samples[samples.len().min(RATE as usize)..];
    let (freq, rms) = analyze(tail);
    println!(
        "received {got_secs:.2}s of audio ({} samples); after warm-up: ~{freq:.1} Hz, rms {rms:.0}",
        samples.len()
    );
    let tone_ok = (freq - TONE_HZ).abs() < 5.0 && rms > 1000.0;
    println!(
        "RESULT tone round-trip: {}",
        if tone_ok { "ok" } else { "FAILED" }
    );
    #[cfg(target_os = "linux")]
    {
        let n = pw_frames.load(std::sync::atomic::Ordering::Relaxed);
        println!(
            "RESULT pipewire playback: {} ({n} frames pulled by the graph)",
            if n > RATE as usize { "ok" } else { "FAILED" }
        );
    }

    publisher.close().await.ok();
    sub.close().await.ok();
    Ok(())
}

fn token(identity: &str) -> anyhow::Result<String> {
    // Dev credentials from deploy/livekit.yaml.
    Ok(
        AccessToken::with_api_key("devkey", "chad-dev-livekit-secret-0123456789abcdef")
            .with_identity(identity)
            .with_grants(VideoGrants {
                room_join: true,
                room: "spike".into(),
                ..Default::default()
            })
            .to_jwt()?,
    )
}

/// Dominant frequency by zero crossings, and RMS.
fn analyze(s: &[i16]) -> (f32, f32) {
    if s.len() < 2 {
        return (0.0, 0.0);
    }
    let crossings = s.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
    let freq = crossings as f32 / 2.0 / (s.len() as f32 / RATE as f32);
    let rms = (s.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt();
    (freq, rms as f32)
}

/// AEC with our own I/O: the far-end (what we play) goes to
/// process_reverse_stream, the mic goes to process_stream. Here the "mic" is
/// a delayed, attenuated copy of the far end, i.e. pure echo, so a working AEC
/// should remove most of its energy (ERLE).
fn apm_check() -> anyhow::Result<()> {
    let mut apm = AudioProcessingModule::new(true, false, false, false);
    apm.set_stream_delay_ms(20)?;
    let delay = (RATE as usize / 1000) * 20;
    let mut rng = 0x1234_5678u32;
    let mut far_hist = VecDeque::from(vec![0i16; delay]);
    let (mut e_in, mut e_out) = (0f64, 0f64);
    let total_frames = 1000; // 10 s
    for f in 0..total_frames {
        // Speech-ish far end: noise shaped by a slow envelope.
        let mut far: Vec<i16> = (0..FRAME)
            .map(|i| {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                let t = (f * FRAME + i) as f32 / RATE as f32;
                let env = 0.5 + 0.5 * (TAU * 3.0 * t).sin();
                ((rng as i32 >> 20) as f32 * env) as i16
            })
            .collect();
        let mut near: Vec<i16> = far
            .iter()
            .map(|&x| {
                far_hist.push_back(x);
                (far_hist.pop_front().unwrap() as f32 * 0.5) as i16
            })
            .collect();
        let before: f64 = near.iter().map(|&x| (x as f64).powi(2)).sum();
        apm.process_reverse_stream(&mut far, RATE as i32, 1)?;
        apm.process_stream(&mut near, RATE as i32, 1)?;
        if f >= total_frames - 200 {
            e_in += before;
            e_out += near.iter().map(|&x| (x as f64).powi(2)).sum::<f64>();
        }
    }
    let erle = 10.0 * (e_in / e_out.max(1.0)).log10();
    println!(
        "RESULT APM echo cancellation (own I/O): ERLE {erle:.1} dB over last 2 s ({})",
        if erle > 10.0 { "ok" } else { "FAILED" }
    );
    Ok(())
}

#[cfg(target_os = "linux")]
mod pipewire_out {
    use std::sync::{Arc, atomic::AtomicUsize, atomic::Ordering};

    use pipewire as pw;
    use pw::{properties::properties, spa};

    use super::{Playback, RATE};

    /// Native PipeWire playback stream on its own thread. Returns a counter
    /// of frames the graph has pulled from us.
    pub fn spawn(playback: Playback) -> Arc<AtomicUsize> {
        let pulled = Arc::new(AtomicUsize::new(0));
        let counter = pulled.clone();
        std::thread::spawn(move || {
            if let Err(e) = run(playback, counter) {
                eprintln!("pipewire: {e}");
            }
        });
        pulled
    }

    unsafe extern "C" {
        /// `bool webrtc::InitializePipeWire()`: dlopens libpipewire and fills
        /// libwebrtc's lazy `pw_*` stubs. libwebrtc.a defines those stubs as
        /// weak symbols, and because they're statically linked they take
        /// precedence over libpipewire.so, so pipewire-rs calls them too.
        /// Until this runs, every stub is a null function pointer.
        #[link_name = "_ZN6webrtc18InitializePipeWireEv"]
        fn webrtc_initialize_pipewire() -> bool;
    }

    fn run(playback: Playback, pulled: Arc<AtomicUsize>) -> Result<(), pw::Error> {
        // SAFETY: no arguments; idempotent (function-local static).
        let stubs = unsafe { webrtc_initialize_pipewire() };
        println!("libwebrtc pipewire stubs initialized: {stubs}");
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;
        let stream = pw::stream::StreamBox::new(
            &core,
            "irchad-spike",
            properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_ROLE => "Communication",
                *pw::keys::MEDIA_CATEGORY => "Playback",
                *pw::keys::APP_NAME => "IrChad spike",
                *pw::keys::NODE_NAME => "irchad-spike",
            },
        )?;
        let _listener = stream
            .add_local_listener_with_user_data(())
            .state_changed(|_, _, old, new| println!("pipewire stream: {old:?} -> {new:?}"))
            .process(move |stream, _| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let data = &mut buffer.datas_mut()[0];
                let n = if let Some(slice) = data.data() {
                    let n = slice.len() / 2;
                    let mut q = playback.lock().unwrap();
                    for i in 0..n {
                        let s = q.pop_front().unwrap_or(0);
                        slice[i * 2..i * 2 + 2].copy_from_slice(&s.to_le_bytes());
                    }
                    n
                } else {
                    0
                };
                pulled.fetch_add(n, Ordering::Relaxed);
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = 2;
                *chunk.size_mut() = (n * 2) as _;
            })
            .register()?;

        let mut info = spa::param::audio::AudioInfoRaw::new();
        info.set_format(spa::param::audio::AudioFormat::S16LE);
        info.set_rate(RATE);
        info.set_channels(1);
        let mut position = [0; spa::param::audio::MAX_CHANNELS];
        position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
        info.set_position(position);
        let bytes = spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &spa::pod::Value::Object(spa::pod::Object {
                type_: spa::sys::SPA_TYPE_OBJECT_Format,
                id: spa::sys::SPA_PARAM_EnumFormat,
                properties: info.into(),
            }),
        )
        .expect("serialize format pod")
        .0
        .into_inner();
        let mut params = [spa::pod::Pod::from_bytes(&bytes).expect("format pod")];
        stream.connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )?;
        mainloop.run();
        Ok(())
    }
}
