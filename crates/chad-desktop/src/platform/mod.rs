//! OS integration: single instance with URL forwarding, the `irchad://` scheme,
//! notifications, the tray and push-to-talk. One impl per target.

use std::sync::{Arc, OnceLock, mpsc};

pub mod idle;
#[cfg(target_os = "linux")]
mod idle_linux;
#[cfg(windows)]
mod idle_windows;
pub mod ptt;
#[cfg(target_os = "linux")]
mod ptt_linux;
#[cfg(windows)]
mod ptt_windows;
pub mod rpc;
#[cfg(target_os = "linux")]
mod rpc_linux;
#[cfg(windows)]
mod rpc_windows;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{Tray, register_scheme, single_instance};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{Tray, autostart, register_scheme, single_instance};

#[cfg(not(any(target_os = "linux", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "linux", windows)))]
pub use unsupported::{Tray, register_scheme, single_instance};

pub const APP_ID: &str = "party.gentoo.IrChad";

/// Window icon for X11 and Windows (Wayland takes it from the desktop entry).
/// `assets/icon.png` is `packaging/linux/party.gentoo.IrChad.svg` at 256 px.
pub fn window_icon() -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| {
        let png = include_bytes!("../../assets/icon.png");
        Arc::new(eframe::icon_data::from_png_bytes(png).unwrap_or_default())
    })
    .clone()
}

/// From the OS to the app.
#[derive(Debug)]
pub enum Os {
    /// An `irchad://` link or invite URL, from a second launch.
    Open(String),
    Show,
    /// From the tray menu.
    #[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
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
        let mut n = notify_rust::Notification::new();
        n.appname("IrChad").summary(&summary).body(&body);
        #[cfg(windows)]
        n.app_id(APP_ID);
        let r = n.show();
        if let Err(e) = r {
            tracing::warn!("notification: {e}");
        }
    });
}

/// Tray tooltip.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub fn tray_title(mentions: u32) -> String {
    match mentions {
        0 => "IrChad".into(),
        1 => "IrChad: 1 mention".into(),
        n => format!("IrChad: {n} mentions"),
    }
}

/// Tray icon: the logo's small-size cut (a speech bubble with a `#` cut out,
/// drawn on a 16-unit grid), with a red dot for mentions or a white one for
/// unread. ARGB32, network byte order (what SNI wants).
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub fn tray_icon(size: usize, unread: bool, mentions: bool) -> Vec<u8> {
    const GREEN: [f32; 3] = [64.0, 160.0, 43.0]; // Catppuccin Latte green, readable on dark and light panels
    let dot = match (mentions, unread) {
        (true, _) => Some([210.0, 15.0, 57.0]),
        (false, true) => Some([255.0, 255.0, 255.0]),
        _ => None,
    };
    let rect = |u: f32, v: f32, x0: f32, y0: f32, x1: f32, y1: f32| {
        (x0..=x1).contains(&u) && (y0..=y1).contains(&v)
    };
    // Colour of the point (u, v) in grid units, or None if transparent.
    let sample = |u: f32, v: f32| -> Option<[f32; 3]> {
        if let Some(c) = dot {
            let d = (u - 13.0).hypot(v - 3.0);
            if d <= 2.5 {
                return Some(c);
            }
            if d <= 3.5 {
                return None; // gap so the dot reads at 16 px
            }
        }
        let hash = rect(u, v, 5.0, 3.0, 7.0, 11.0)
            || rect(u, v, 9.0, 3.0, 11.0, 11.0)
            || rect(u, v, 4.0, 4.0, 12.0, 6.0)
            || rect(u, v, 4.0, 8.0, 12.0, 10.0);
        if hash {
            return None;
        }
        // Rounded rect (1,2)–(15,12), radius 3: distance from the inner rect.
        let (dx, dy) = (
            (4.0 - u).max(u - 12.0).max(0.0),
            (5.0 - v).max(v - 9.0).max(0.0),
        );
        let bubble = rect(u, v, 1.0, 2.0, 15.0, 12.0) && dx.hypot(dy) <= 3.0;
        // Tail triangle (3,11) (2,15) (7,12).
        let side = |ax: f32, ay: f32, bx: f32, by: f32| (bx - ax) * (v - ay) - (by - ay) * (u - ax);
        let (s1, s2, s3) = (
            side(3.0, 11.0, 2.0, 15.0),
            side(2.0, 15.0, 7.0, 12.0),
            side(7.0, 12.0, 3.0, 11.0),
        );
        let tail = (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0);
        (bubble || tail).then_some(GREEN)
    };
    const SS: usize = 4; // 4×4 supersampling for anti-aliased edges
    let scale = 16.0 / size as f32;
    let mut px = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let (mut sum, mut n) = ([0.0f32; 3], 0);
            for sy in 0..SS {
                for sx in 0..SS {
                    let u = (x as f32 + (sx as f32 + 0.5) / SS as f32) * scale;
                    let v = (y as f32 + (sy as f32 + 0.5) / SS as f32) * scale;
                    if let Some(c) = sample(u, v) {
                        (0..3).for_each(|i| sum[i] += c[i]);
                        n += 1;
                    }
                }
            }
            if n > 0 {
                let a = (n * 255 / (SS * SS)) as u8;
                let c = sum.map(|s| (s / n as f32).round() as u8);
                px[(y * size + x) * 4..][..4].copy_from_slice(&[a, c[0], c[1], c[2]]);
            }
        }
    }
    px
}

#[cfg(test)]
mod tray_tests {
    use super::tray_icon;

    #[test]
    fn window_icon_decodes() {
        let icon = super::window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
    }

    fn at(px: &[u8], size: usize, x: usize, y: usize) -> [u8; 4] {
        px[(y * size + x) * 4..][..4].try_into().unwrap()
    }

    #[test]
    fn draws_bubble_hash_and_dot() {
        let px = tray_icon(32, false, false);
        assert_eq!(px.len(), 32 * 32 * 4);
        assert_eq!(at(&px, 32, 0, 0)[0], 0, "corner is transparent");
        assert_eq!(
            at(&px, 32, 4, 14),
            [255, 64, 160, 43],
            "bubble body is green"
        );
        assert_eq!(at(&px, 32, 12, 14)[0], 0, "hash is cut out");
        assert_eq!(
            at(&px, 32, 16, 14),
            [255, 64, 160, 43],
            "hash centre is filled"
        );
        assert_eq!(at(&px, 32, 26, 6), [255, 64, 160, 43], "no dot when read");
        assert_eq!(
            at(&tray_icon(32, true, true), 32, 26, 6),
            [255, 210, 15, 57],
            "mention dot"
        );
        assert_eq!(
            at(&tray_icon(32, true, false), 32, 26, 6),
            [255, 255, 255, 255],
            "unread dot"
        );
        for s in [16, 22, 64] {
            assert_eq!(tray_icon(s, true, false).len(), s * s * 4);
        }
    }
}
