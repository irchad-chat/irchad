//! End-to-end check of whispers (D71) against the dev stack: check-a joins
//! through [`chad_media::Voice`] and whispers to check-b. Raw LiveKit
//! participants try to subscribe to everything, as a hostile client would:
//! check-c must never get the whisper track, but must get the mic; check-d
//! joins later and must still get the mic; then check-c becomes a target and
//! must get the whisper.
//!
//! ```sh
//! SSL_CERT_FILE=<system bundle + Caddy root> \
//!   cargo run -p chad-media --example whisper_check
//! ```
//!
//! Env: LIVEKIT_URL (default wss://localhost:8443/livekit).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use livekit::prelude::*;
use livekit_api::access_token::{AccessToken, VideoGrants};

type Got = Arc<Mutex<HashSet<String>>>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let url = std::env::var("LIVEKIT_URL").unwrap_or("wss://localhost:8443/livekit".into());

    let (b, b_got) = listener(&url, "check-b").await?;
    let (c, c_got) = listener(&url, "check-c").await?;
    let voice = chad_media::Voice::new(Arc::new(|| {}));
    voice.set_whisper_targets(vec!["check-b".into()]);
    let mut call = voice
        .join(&url, &token("check-a")?)
        .await
        .map_err(anyhow::Error::msg)?;
    println!("check-a joined through Voice, whispering to check-b");
    let settle = || tokio::time::sleep(Duration::from_secs(3));
    tokio::select! {
        () = settle() => {}
        r = call.ended() => anyhow::bail!("call ended early: {r:?}"),
    }
    let (d, d_got) = listener(&url, "check-d").await?;
    settle().await;

    let mut ok = true;
    let mut result = |what: &str, pass: bool| {
        println!("RESULT {what}: {}", if pass { "ok" } else { "FAILED" });
        ok &= pass;
    };
    let has = |g: &Got, t: &str| g.lock().unwrap().contains(t);
    result("check-b got the mic", has(&b_got, "mic"));
    result("check-b got the whisper", has(&b_got, "whisper"));
    result("check-c got the mic", has(&c_got, "mic"));
    result("check-c did not get the whisper", !has(&c_got, "whisper"));
    result("check-d (late) got the mic", has(&d_got, "mic"));
    result("check-d did not get the whisper", !has(&d_got, "whisper"));

    voice.set_whisper_targets(vec!["check-b".into(), "check-c".into()]);
    settle().await;
    result(
        "check-c got the whisper once a target",
        has(&c_got, "whisper"),
    );
    result("check-d still not", !has(&d_got, "whisper"));

    drop(call);
    for r in [b, c, d] {
        r.close().await.ok();
    }
    anyhow::ensure!(ok, "some checks failed");
    Ok(())
}

/// Joins and subscribes to every track (the default), recording the names of
/// check-a's tracks it got.
async fn listener(url: &str, identity: &str) -> anyhow::Result<(Room, Got)> {
    let (room, mut events) = Room::connect(url, &token(identity)?, RoomOptions::default())
        .await
        .with_context(|| format!("{identity} connect"))?;
    let got = Got::default();
    let g = got.clone();
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            if let RoomEvent::TrackSubscribed {
                publication,
                participant,
                ..
            } = ev
                && participant.identity().as_str() == "check-a"
            {
                g.lock().unwrap().insert(publication.name());
            }
        }
    });
    Ok((room, got))
}

fn token(identity: &str) -> anyhow::Result<String> {
    // Dev credentials from deploy/livekit.yaml.
    Ok(
        AccessToken::with_api_key("devkey", "chad-dev-livekit-secret-0123456789abcdef")
            .with_identity(identity)
            .with_grants(VideoGrants {
                room_join: true,
                room: "whisper-check".into(),
                // As chadd mints them (crates/chadd/src/api.rs).
                can_publish_sources: [
                    "microphone",
                    "camera",
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
