//! Which video encoder actually runs on this machine, with no server: two
//! peer connections in one process, a moving 1080p test pattern sent from
//! one to the other with the codec and encoder backend that screen share
//! would ask for, then the sender's outbound-rtp stats.
//!
//! ```sh
//! cargo run --release -p chad-media --example encoder_check
//! ```
//!
//! Env: CODEC (av1 | h264 | h265 | vp9, default av1), BACKEND (nvenc | vaapi |
//! software | auto, default nvenc), CHECK_SECS (5). On Windows, NVENC is only
//! compiled in when the build saw cuda.h (`scripts/cuda-headers.ps1`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use livekit::webrtc::MediaType;
use livekit::webrtc::media_stream_track::MediaStreamTrack;
use livekit::webrtc::peer_connection::{AnswerOptions, OfferOptions, PeerConnection};
use livekit::webrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
use livekit::webrtc::peer_connection_factory::{PeerConnectionFactory, RtcConfiguration};
use livekit::webrtc::rtp_sender::VideoEncoderBackend;
use livekit::webrtc::rtp_transceiver::{RtpTransceiverDirection, RtpTransceiverInit};
use livekit::webrtc::stats::RtcStats;
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::VideoResolution;
use livekit::webrtc::video_source::native::NativeVideoSource;

const W: u32 = 1920;
const H: u32 = 1080;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let codec = std::env::var("CODEC")
        .unwrap_or("av1".into())
        .to_lowercase();
    let mime = match codec.as_str() {
        "h264" => "video/H264",
        "h265" => "video/H265",
        "vp9" => "video/VP9",
        _ => "video/AV1",
    };
    let backend = match std::env::var("BACKEND").as_deref() {
        Ok("vaapi") => VideoEncoderBackend::Vaapi,
        Ok("software") => VideoEncoderBackend::Software,
        Ok("auto") => VideoEncoderBackend::Auto,
        _ => VideoEncoderBackend::Nvenc,
    };
    let secs: u64 = std::env::var("CHECK_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let available: Vec<_> = VideoEncoderBackend::list_available().into_iter().collect();
    println!("available backends: {available:?}");

    let factory = PeerConnectionFactory::with_options(false, false);
    let tx = factory.create_peer_connection(RtcConfiguration::default())?;
    let rx = factory.create_peer_connection(RtcConfiguration::default())?;
    trickle(&tx, &rx);
    trickle(&rx, &tx);

    let source = NativeVideoSource::new(
        VideoResolution {
            width: W,
            height: H,
        },
        true,
    );
    let track = factory.create_video_track("check", source.clone());
    let transceiver = tx.add_transceiver(
        MediaStreamTrack::Video(track),
        RtpTransceiverInit {
            direction: RtpTransceiverDirection::SendOnly,
            stream_ids: vec!["check".into()],
            send_encodings: vec![],
        },
    )?;
    let mut codecs: Vec<_> = factory
        .get_rtp_sender_capabilities(MediaType::Video)
        .codecs
        .into_iter()
        .filter(|c| c.mime_type.eq_ignore_ascii_case(mime))
        .collect();
    // The NVENC factory only offers H.264 constrained baseline (42e01f).
    codecs.sort_by_key(|c| {
        !c.sdp_fmtp_line
            .as_deref()
            .is_some_and(|f| f.contains("42e01f"))
    });
    anyhow::ensure!(
        !codecs.is_empty(),
        "{mime} isn't in the sender capabilities"
    );
    transceiver.set_codec_preferences(codecs)?;
    transceiver.sender().set_video_encoder_backend(backend);

    let offer = tx.create_offer(OfferOptions::default()).await?;
    tx.set_local_description(offer.clone()).await?;
    rx.set_remote_description(offer).await?;
    let answer = rx.create_answer(AnswerOptions::default()).await?;
    rx.set_local_description(answer.clone()).await?;
    tx.set_remote_description(answer).await?;

    // A gradient that scrolls, so the encoder has real work every frame.
    let start = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(1000 / 60));
    let mut n = 0u32;
    while start.elapsed() < Duration::from_secs(secs) {
        tick.tick().await;
        let mut buf = I420Buffer::new(W, H);
        let (sy, _, _) = buf.strides();
        let (y, u, v) = buf.data_mut();
        for row in 0..H {
            let line = &mut y[(row * sy) as usize..][..W as usize];
            for (x, px) in line.iter_mut().enumerate() {
                *px = (x as u32 + row + n * 8) as u8;
            }
        }
        u.fill(128);
        v.fill((n % 256) as u8);
        let mut frame = VideoFrame::new(VideoRotation::VideoRotation0, buf);
        frame.timestamp_us = start.elapsed().as_micros() as i64;
        source.capture_frame(&frame);
        n += 1;
    }

    let mut found = false;
    for s in tx.get_stats().await? {
        if let RtcStats::OutboundRtp(o) = s
            && o.stream.kind == "video"
        {
            found = true;
            println!(
                "encoder: {} (hardware: {}), codec {mime}, asked for {backend:?}; \
                 {} frames encoded, {}x{} at {:.0} fps, {:.2} ms/frame encode, \
                 limited by {:?}",
                o.outbound.encoder_implementation,
                o.outbound.power_efficient_encoder,
                o.outbound.frames_encoded,
                o.outbound.frame_width,
                o.outbound.frame_height,
                o.outbound.frames_per_second,
                o.outbound.total_encode_time * 1000.0 / f64::from(o.outbound.frames_encoded.max(1)),
                o.outbound.quality_limitation_reason,
            );
        }
    }
    anyhow::ensure!(
        found,
        "no outbound video stats (did the connection come up?)"
    );
    tx.close();
    rx.close();
    Ok(())
}

/// Hands `from`'s ICE candidates straight to `to`.
fn trickle(from: &PeerConnection, to: &PeerConnection) {
    let to = Arc::new(to.clone());
    let rt = tokio::runtime::Handle::current();
    from.on_ice_candidate(Some(Box::new(move |c| {
        let to = to.clone();
        rt.spawn(async move {
            if let Err(e) = to.add_ice_candidate(c).await {
                eprintln!("add_ice_candidate: {e:?}");
            }
        });
    })));
}
