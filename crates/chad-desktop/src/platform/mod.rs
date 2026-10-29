//! OS integration: single instance with URL forwarding, the `irchad://` scheme,
//! notifications, the tray and push-to-talk. One impl per target.

use std::sync::{Arc, OnceLock, mpsc};

pub mod ptt;
#[cfg(target_os = "linux")]
mod ptt_linux;
#[cfg(windows)]
mod ptt_windows;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{Tray, register_scheme, single_instance};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{Tray, register_scheme, single_instance};

#[cfg(not(any(target_os = "linux", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "linux", windows)))]
pub use unsupported::{Tray, register_scheme, single_instance};

pub const APP_ID: &str = "party.gentoo.IrChad";

/// From the OS to the app.
#[derive(Debug)]
pub enum Os {
    /// An `irchad://` link or invite URL, from a second launch.
    Open(String),
    Show,
    /// From the tray menu; Windows has no tray yet.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Quit,
}

/// Set once the window exists, so OS callbacks can wake the UI.
pub type Wake = Arc<OnceLock<egui::Context>>;

#[derive(Clone)]
pub struct OsTx {
    tx: mpsc::Sender<Os>,
    wake: Wake,
}

impl OsTx {
    pub fn new(tx: mpsc::Sender<Os>, wake: Wake) -> Self {
        Self { tx, wake }
    }

    pub fn send(&self, msg: Os) {
        let _ = self.tx.send(msg);
        if let Some(ctx) = self.wake.get() {
            ctx.request_repaint();
        }
    }
}

/// A desktop notification (freedesktop Notifications / Windows toast).
pub fn notify(summary: String, body: String) {
    tokio::task::spawn_blocking(move || {
        let r = notify_rust::Notification::new()
            .appname("IrChad")
            .summary(&summary)
            .body(&body)
            .show();
        if let Err(e) = r {
            tracing::warn!("notification: {e}");
        }
    });
}

/// Tray icon: a disc, with a red dot for mentions or a white one for unread.
/// ARGB32, network byte order (what SNI wants).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn tray_icon(size: usize, unread: bool, mentions: bool) -> Vec<u8> {
    let mut px = vec![0u8; size * size * 4];
    let r = size as f32 / 2.0;
    let dot = size as f32 * 0.22;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let inside = |cx: f32, cy: f32, rad: f32| (fx - cx).hypot(fy - cy) <= rad;
            let c = if (mentions || unread) && inside(size as f32 - dot, size as f32 - dot, dot) {
                if mentions {
                    [255, 237, 66, 69]
                } else {
                    [255, 255, 255, 255]
                }
            } else if inside(r, r, r - 1.0) {
                [255, 88, 101, 242]
            } else {
                [0, 0, 0, 0]
            };
            px[(y * size + x) * 4..][..4].copy_from_slice(&c);
        }
    }
    px
}
