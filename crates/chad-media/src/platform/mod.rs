//! Audio I/O behind one trait: PipeWire on Linux, cpal (WASAPI) elsewhere.

use std::sync::Arc;

use crate::Devices;
use crate::engine::Shared;
use crate::screen::{AppSink, AudioApp, FrameSink, OnEnd, ScreenSource, Targets};

#[cfg(target_os = "linux")]
mod camera;
pub mod desktop;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod loopback;
#[cfg(windows)]
pub(crate) mod mf;
#[cfg(not(target_os = "linux"))]
mod other;
#[cfg(target_os = "linux")]
pub mod portal;

pub trait Backend: Send + Sync {
    /// (Re)opens capture and playback on these devices (`None`: the default),
    /// feeding [`Shared::capture`] and pulling [`Shared::render`].
    fn start(&self, input: Option<String>, output: Option<String>);
    /// Closes both; the mic is off.
    fn stop(&self);
    fn devices(&self) -> Devices;
    /// Apps playing audio (never ourselves), for screen share.
    fn apps(&self) -> Vec<AudioApp> {
        Vec::new()
    }
    /// Sends an [`AudioApp::id`]'s output to the sink as 48 kHz interleaved
    /// stereo, until called again; `None` stops.
    fn capture_app(&self, _app: Option<(String, AppSink)>) {}
}

/// The portal on Wayland, our picker on X11 (plus the portal where it serves
/// X11), our picker on Windows. Decided at runtime.
pub async fn targets() -> Targets {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return Targets {
                portal: true,
                sources: Vec::new(),
            };
        }
        let portal = portal::available().await;
        let sources = tokio::task::spawn_blocking(desktop::sources)
            .await
            .unwrap_or_default();
        Targets { portal, sources }
    }
    #[cfg(not(target_os = "linux"))]
    Targets {
        portal: false,
        sources: tokio::task::spawn_blocking(desktop::sources)
            .await
            .unwrap_or_default(),
    }
}

pub fn backend(shared: Arc<Shared>) -> Arc<dyn Backend> {
    #[cfg(target_os = "linux")]
    return Arc::new(linux::PipeWire::spawn(shared));
    #[cfg(not(target_os = "linux"))]
    return Arc::new(other::Cpal::spawn(shared));
}

/// Opens camera `id` (a [`crate::Devices::cameras`] id; `None`: the first).
pub async fn camera(
    id: Option<String>,
    sink: FrameSink,
    on_end: OnEnd,
) -> Result<Box<dyn ScreenSource>, String> {
    #[cfg(target_os = "linux")]
    return camera::start(id, sink, on_end).await;
    #[cfg(windows)]
    return mf::start(id, sink, on_end).await;
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (id, sink, on_end);
        Err("Cameras aren't supported here.".into())
    }
}
