//! libwebrtc's own desktop capturers, behind our picker: on X11 XShm + XDamage
//! for screens and XComposite for windows; on Windows, Windows.Graphics.Capture
//! (D21). Frames come back BGRA on the thread that asks for them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant};

use livekit::webrtc::desktop_capturer::{
    CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions,
};

use crate::screen::{Frame, FrameSink, OnEnd, PixelFormat, ScreenSource, ShareSource, SourceId};

fn capturer(window: bool) -> Option<DesktopCapturer> {
    let kind = if window {
        DesktopCaptureSourceType::Window
    } else {
        DesktopCaptureSourceType::Screen
    };
    let mut options = DesktopCapturerOptions::new(kind);
    options.set_include_cursor(true);
    DesktopCapturer::new(options)
}

/// Screens first, then windows.
pub fn sources() -> Vec<ShareSource> {
    let mut out = Vec::new();
    for window in [false, true] {
        let Some(c) = capturer(window) else { continue };
        for (i, s) in c.get_source_list().into_iter().enumerate() {
            let title = match s.title() {
                t if !t.is_empty() => t,
                _ if window => continue,
                _ => format!("Screen {}", i + 1),
            };
            out.push(ShareSource {
                id: SourceId { window, id: s.id() },
                title,
            });
        }
    }
    out
}

struct Running(Arc<AtomicBool>);

impl ScreenSource for Running {}

impl Drop for Running {
    fn drop(&mut self) {
        self.0.store(true, Relaxed);
    }
}

pub fn start(
    source: SourceId,
    fps: u32,
    mut sink: FrameSink,
    on_end: OnEnd,
) -> Result<Box<dyn ScreenSource>, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready) = std::sync::mpsc::channel();
    let st = stop.clone();
    std::thread::Builder::new()
        .name("irchad-capture".into())
        .spawn(move || {
            // The capturer isn't Send: it lives and dies on this thread.
            let picked = capturer(source.window).and_then(|c| {
                let s = c
                    .get_source_list()
                    .into_iter()
                    .find(|s| s.id() == source.id)?;
                Some((c, s))
            });
            let Some((mut c, s)) = picked else {
                let _ = ready_tx.send(false);
                return;
            };
            let _ = ready_tx.send(true);
            let failed = Arc::new(AtomicBool::new(false));
            let f = failed.clone();
            c.start_capture(Some(s), move |r| match r {
                Ok(frame) => {
                    let (Ok(width), Ok(height)) =
                        (u32::try_from(frame.width()), u32::try_from(frame.height()))
                    else {
                        return;
                    };
                    sink(Frame {
                        data: frame.data(),
                        width,
                        height,
                        stride: frame.stride(),
                        format: PixelFormat::Bgra,
                    });
                }
                Err(CaptureError::Permanent) => f.store(true, Relaxed),
                Err(CaptureError::Temporary) => {}
            });
            let interval = Duration::from_micros(1_000_000 / u64::from(fps.max(1)));
            while !st.load(Relaxed) {
                let t = Instant::now();
                c.capture_frame();
                if failed.load(Relaxed) {
                    tracing::info!("screen capture ended");
                    on_end();
                    return;
                }
                std::thread::sleep(interval.saturating_sub(t.elapsed()));
            }
        })
        .map_err(|e| e.to_string())?;
    if ready.recv().unwrap_or(false) {
        Ok(Box::new(Running(stop)))
    } else {
        Err("That screen or window is gone.".into())
    }
}
