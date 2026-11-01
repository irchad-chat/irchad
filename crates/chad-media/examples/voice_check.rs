//! End-to-end check of [`chad_media::Voice`] against the dev stack: joins a
//! room with the real audio engine (your mic and speakers), while a second,
//! raw LiveKit participant publishes a quiet tone and listens to us, mic and
//! soundboard apart.
//!
//! ```sh
//! SSL_CERT_FILE=<system bundle + Caddy root> \
//!   cargo run -p chad-media --example voice_check
//! ```
//!
//! Env: LIVEKIT_URL (default wss://localhost:8443/livekit), CHECK_SECS (6).

use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Context;
use futures_util::StreamExt;
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::track::{LocalAudioTrack, LocalTrack, RemoteTrack, TrackSource};
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::{
    AudioSourceOptions, RtcAudioSource, native::NativeAudioSource,
};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit_api::access_token::{AccessToken, VideoGrants};

const RATE: u32 = 48_000;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let url = std::env::var("LIVEKIT_URL").unwrap_or("wss://localhost:8443/livekit".into());
    let secs: u64 = std::env::var("CHECK_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);

    let voice = chad_media::Voice::new(Arc::new(|| {}));
    let mut call = voice
        .join(&url, &token("check-a")?)
        .await
        .map_err(anyhow::Error::msg)?;
    println!("check-a joined through Voice");
    // A one-second soundboard clip, sent once check-b listens.
    let clip: Arc<[i16]> = (0..RATE)
        .map(|i| ((i as f32 * TAU * 660.0 / RATE as f32).sin() * 3000.0) as i16)
        .collect();

    let (other, mut events) = Room::connect(&url, &token("check-b")?, RoomOptions::default())
        .await
        .context("check-b connect")?;
    let source = NativeAudioSource::new(AudioSourceOptions::default(), RATE, 1, 100);
    let track = LocalAudioTrack::create_audio_track("tone", RtcAudioSource::Native(source.clone()));
    other
        .local_participant()
        .publish_track(
            LocalTrack::Audio(track),
            TrackPublishOptions {
                source: TrackSource::Microphone,
                ..Default::default()
            },
        )
        .await?;
    let tone = tokio::spawn(async move {
        let mut phase = 0.0f32;
        let mut buf = vec![0i16; 480];
        loop {
            for s in buf.iter_mut() {
                *s = (phase.sin() * 0.03 * f32::from(i16::MAX)) as i16;
                phase = (phase + TAU * 440.0 / RATE as f32) % TAU;
            }
            let frame = AudioFrame {
                data: buf.as_slice().into(),
                sample_rate: RATE,
                num_channels: 1,
                samples_per_channel: 480,
            };
            if source.capture_frame(&frame).await.is_err() {
                break;
            }
        }
    });
    let heard = Arc::new(AtomicUsize::new(0));
    let board = Arc::new(AtomicUsize::new(0));
    let (h, b) = (heard.clone(), board.clone());
    let listen = tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            if let RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(t),
                publication,
                ..
            } = ev
            {
                // The soundboard idles silent between clips: count its loud samples.
                let sb = publication.name() == "soundboard";
                let n = if sb { b.clone() } else { h.clone() };
                tokio::spawn(async move {
                    let mut stream = NativeAudioStream::new(t.rtc_track(), RATE as i32, 1);
                    while let Some(f) = stream.next().await {
                        let c = if sb {
                            f.data.iter().filter(|x| x.abs() > 500).count()
                        } else {
                            f.data.len()
                        };
                        n.fetch_add(c, Ordering::Relaxed);
                    }
                });
            }
        }
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    voice.play_sound(clip).map_err(anyhow::Error::msg)?;

    tokio::select! {
        () = tokio::time::sleep(Duration::from_secs(secs)) => {}
        r = call.ended() => anyhow::bail!("call ended early: {r:?}"),
    }
    let state = voice.call().context("no call state")?;
    println!("call state: {state:?}");
    println!("mic level now: {:.1} dBFS", voice.input_level());
    let got = heard.load(Ordering::Relaxed) as f32 / RATE as f32;
    let ok_peers = state.participants.iter().any(|p| p.identity == "check-b");
    let ok_audio = got > secs as f32 / 2.0;
    let got_board = board.load(Ordering::Relaxed) as f32 / RATE as f32;
    println!(
        "RESULT participants: {}",
        if ok_peers { "ok" } else { "FAILED" }
    );
    println!(
        "RESULT check-b heard {got:.1}s of our mic: {}",
        if ok_audio { "ok" } else { "FAILED" }
    );
    println!(
        "RESULT check-b heard {got_board:.2}s of loud soundboard: {}",
        if got_board > 0.3 { "ok" } else { "FAILED" }
    );
    println!(
        "RESULT rtt: {}",
        state
            .rtt_ms
            .map_or("FAILED".into(), |r| format!("{r} ms (ok)"))
    );
    tone.abort();
    listen.abort();
    drop(call);
    other.close().await.ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    anyhow::ensure!(voice.call().is_none(), "call state left behind");
    Ok(())
}

fn token(identity: &str) -> anyhow::Result<String> {
    // Dev credentials from deploy/livekit.yaml.
    Ok(
        AccessToken::with_api_key("devkey", "chad-dev-livekit-secret-0123456789abcdef")
            .with_identity(identity)
            .with_grants(VideoGrants {
                room_join: true,
                room: "voice-check".into(),
                // As chadd mints them (crates/chadd/src/api.rs).
                can_publish_sources: [
                    "microphone",
                    "screen_share",
                    "screen_share_audio",
                    "unknown",
                ]
                .map(Into::into)
                .to_vec(),
                ..Default::default()
            })
            .to_jwt()?,
    )
}
