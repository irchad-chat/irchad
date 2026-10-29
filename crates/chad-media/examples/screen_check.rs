//! End-to-end check of screen share against the dev stack, no desktop needed:
//! one [`chad_media::Voice`] shares a moving 1080p test pattern, a second one
//! watches it, and we report frames, resolution, the watcher count and CPU
//! per thread (for `docs/notes/screenshare-perf.md`).
//!
//! ```sh
//! SSL_CERT_FILE=<system bundle + Caddy root> \
//!   cargo run --release -p chad-media --example screen_check
//! ```
//!
//! Env: LIVEKIT_URL (default wss://localhost:8443/livekit), CHECK_SECS (10),
//! PRESET (720p30 | 1080p60 | source).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chad_media::{Preset, ScreenSettings, Target, Voice};
use livekit_api::access_token::{AccessToken, VideoGrants};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,chad_media=info".into()),
        )
        .init();
    let url = std::env::var("LIVEKIT_URL").unwrap_or("wss://localhost:8443/livekit".into());
    let secs: u64 = std::env::var("CHECK_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let preset = match std::env::var("PRESET").as_deref() {
        Ok("720p30") => Preset::P720p30,
        Ok("source") => Preset::Source,
        _ => Preset::P1080p60,
    };

    let sharer = Voice::new(Arc::new(|| {}));
    let viewer = Voice::new(Arc::new(|| {}));
    let _a = sharer
        .join(&url, &token("share-a")?)
        .await
        .map_err(anyhow::Error::msg)?;
    let _b = viewer
        .join(&url, &token("share-b")?)
        .await
        .map_err(anyhow::Error::msg)?;
    let settings = ScreenSettings {
        preset,
        ..Default::default()
    };
    sharer
        .share(Target::Pattern, None, settings)
        .await
        .map_err(anyhow::Error::msg)?;
    println!("share-a sharing a test pattern at {}", preset.label());

    // Wait until share-b sees the share, then watch it.
    let t = Instant::now();
    while !viewer.call().is_some_and(|c| {
        c.participants
            .iter()
            .any(|p| p.identity == "share-a" && p.sharing)
    }) {
        anyhow::ensure!(
            t.elapsed() < Duration::from_secs(5),
            "share never showed up"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    viewer.watch(Some("share-a".into()));

    tokio::time::sleep(Duration::from_secs(2)).await;
    let cpu0 = threads();
    let (t0, mut last, mut frames, mut size) = (Instant::now(), 0, 0u64, (0, 0));
    while t0.elapsed() < Duration::from_secs(secs) {
        if let Some(f) = viewer.screen()
            && f.seq != last
        {
            last = f.seq;
            frames += 1;
            size = (f.width, f.height);
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let wall = t0.elapsed().as_secs_f64();
    let cpu1 = threads();

    let watchers = sharer
        .call()
        .and_then(|c| c.participants.into_iter().find(|p| p.local))
        .map_or(0, |p| p.watchers);
    let fps = frames as f64 / wall;
    println!(
        "RESULT received {frames} frames in {wall:.1}s ({fps:.1} fps) at {}x{}: {}",
        size.0,
        size.1,
        if fps > f64::from(preset_fps(preset)) * 0.6 {
            "ok"
        } else {
            "FAILED"
        }
    );
    println!(
        "RESULT watchers seen by the sharer: {watchers} {}",
        if watchers == 1 { "(ok)" } else { "(FAILED)" }
    );

    let mut by_name: HashMap<String, f64> = HashMap::new();
    for (tid, (name, ticks)) in &cpu1 {
        let before = cpu0.get(tid).map_or(0, |c| c.1);
        *by_name.entry(name.clone()).or_default() += (ticks - before) as f64;
    }
    let hz = 100.0; // USER_HZ
    let total: f64 = by_name.values().sum::<f64>() / hz / wall * 100.0;
    println!("CPU (both ends, % of one core): {total:.0}%");
    let mut top: Vec<_> = by_name.into_iter().collect();
    top.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (name, ticks) in top.iter().take(8) {
        println!("  {:>5.1}%  {name}", ticks / hz / wall * 100.0);
    }

    viewer.watch(None);
    sharer.stop_share();
    Ok(())
}

fn preset_fps(p: Preset) -> u32 {
    if p == Preset::P720p30 { 30 } else { 60 }
}

/// tid → (thread name, utime + stime ticks), from /proc.
fn threads() -> HashMap<u32, (String, u64)> {
    let mut out = HashMap::new();
    let Ok(dir) = std::fs::read_dir("/proc/self/task") else {
        return out;
    };
    for e in dir.flatten() {
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else {
            continue;
        };
        let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else {
            continue;
        };
        let name = stat[open + 1..close].to_owned();
        let f: Vec<&str> = stat[close + 2..].split(' ').collect();
        let ticks = f.get(11).and_then(|u| u.parse::<u64>().ok()).unwrap_or(0)
            + f.get(12).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        if let Ok(tid) = e.file_name().to_string_lossy().parse() {
            out.insert(tid, (name, ticks));
        }
    }
    out
}

fn token(identity: &str) -> anyhow::Result<String> {
    // Dev credentials from deploy/livekit.yaml.
    Ok(
        AccessToken::with_api_key("devkey", "chad-dev-livekit-secret-0123456789abcdef")
            .with_identity(identity)
            .with_grants(VideoGrants {
                room_join: true,
                room: "screen-check".into(),
                can_update_own_metadata: Some(true),
                ..Default::default()
            })
            .to_jwt()?,
    )
}
