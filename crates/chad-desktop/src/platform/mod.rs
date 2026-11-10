//! OS integration: single instance with URL forwarding, the `irchad://` scheme,
//! notifications, the tray and push-to-talk. One impl per target.

use std::sync::{Arc, OnceLock, mpsc};

pub mod idle;
#[cfg(target_os = "linux")]
mod idle_linux;
#[cfg(windows)]
mod idle_windows;
pub mod overlay;
pub mod presence;
#[cfg(target_os = "linux")]
mod presence_linux;
#[cfg(windows)]
mod presence_windows;
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

/// Tray icon: the logo (`party.gentoo.IrChad.svg`), tile and all so it reads
/// on light and dark panels, with a red dot for mentions or a white one for
/// unread. ARGB32, network byte order (what SNI wants).
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub fn tray_icon(size: usize, unread: bool, mentions: bool) -> Vec<u8> {
    // The logo's Catppuccin Mocha green and base.
    const GREEN: [f32; 3] = [166.0, 227.0, 161.0];
    const BASE: [f32; 3] = [30.0, 30.0, 46.0];
    let dot = match (mentions, unread) {
        (true, _) => Some([210.0, 15.0, 57.0]),
        (false, true) => Some([255.0, 255.0, 255.0]),
        _ => None,
    };
    let scale = 256.0 / size as f32;
    // The SVG's 13-unit strokes (inside its 1.15× group), but never thinner than 2 px.
    let half = 6.5f32.max(scale / 1.15);
    let seg = |u: f32, v: f32, ax: f32, ay: f32, bx: f32, by: f32| {
        let (dx, dy) = (bx - ax, by - ay);
        let t = (((u - ax) * dx + (v - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        (u - ax - t * dx).hypot(v - ay - t * dy) <= half
    };
    // Colour of the point (x, y) in SVG units, or None if transparent.
    let sample = |x: f32, y: f32| -> Option<[f32; 3]> {
        if let Some(c) = dot {
            let d = (x - 215.0).hypot(y - 41.0);
            if d <= 41.0 {
                return Some(c);
            }
            if d <= 55.0 {
                return None; // gap so the dot reads at 16 px
            }
        }
        // The tile: (0,0)–(256,256), radius 56.
        let (dx, dy) = (
            (56.0 - x).max(x - 200.0).max(0.0),
            (56.0 - y).max(y - 200.0).max(0.0),
        );
        if dx.hypot(dy) > 56.0 {
            return None;
        }
        // Into the bubble group's units: translate(13 8.4) scale(1.15).
        let (u, v) = ((x - 13.0) / 1.15, (y - 8.4) / 1.15);
        let hash = seg(u, v, 90.0, 56.0, 80.0, 124.0)
            || seg(u, v, 124.0, 56.0, 114.0, 124.0)
            || seg(u, v, 62.0, 78.0, 142.0, 78.0)
            || seg(u, v, 58.0, 104.0, 138.0, 104.0);
        if hash {
            return Some(BASE);
        }
        // Rounded rect (26,30)–(174,148), radius 34: distance from the inner rect.
        let (dx, dy) = (
            (60.0 - u).max(u - 140.0).max(0.0),
            (64.0 - v).max(v - 114.0).max(0.0),
        );
        let bubble =
            (26.0..=174.0).contains(&u) && (30.0..=148.0).contains(&v) && dx.hypot(dy) <= 34.0;
        // Tail triangle (60,140) (46,178) (100,146).
        let side = |ax: f32, ay: f32, bx: f32, by: f32| (bx - ax) * (v - ay) - (by - ay) * (u - ax);
        let (s1, s2, s3) = (
            side(60.0, 140.0, 46.0, 178.0),
            side(46.0, 178.0, 100.0, 146.0),
            side(100.0, 146.0, 60.0, 140.0),
        );
        let tail = (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0);
        Some(if bubble || tail { GREEN } else { BASE })
    };
    const SS: usize = 4; // 4×4 supersampling for anti-aliased edges
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
            at(&px, 32, 6, 13),
            [255, 166, 227, 161],
            "bubble body is green"
        );
        assert_eq!(at(&px, 32, 13, 13), [255, 30, 30, 46], "hash is base");
        assert_eq!(
            at(&px, 32, 16, 14),
            [255, 166, 227, 161],
            "hash centre is filled"
        );
        assert_eq!(at(&px, 32, 26, 5), [255, 30, 30, 46], "no dot when read");
        assert_eq!(
            at(&tray_icon(32, true, true), 32, 26, 5),
            [255, 210, 15, 57],
            "mention dot"
        );
        assert_eq!(
            at(&tray_icon(32, true, false), 32, 26, 5),
            [255, 255, 255, 255],
            "unread dot"
        );
        for s in [16, 22, 64] {
            assert_eq!(tray_icon(s, true, false).len(), s * s * 4);
        }
    }
}
