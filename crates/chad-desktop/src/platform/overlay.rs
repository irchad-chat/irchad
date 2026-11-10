//! Which game has focus, for the overlay (D74): the focused window's program
//! matched against Discord's game list and the user's own executables, polled
//! off the UI thread. One `foreground` per OS.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use tokio::sync::watch;

use crate::presence::{Games, custom_game};

const EVERY: Duration = Duration::from_millis(500);

/// A focused game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub game: String,
    /// Its window: x, y, width, height in physical pixels, when known.
    pub rect: Option<[i32; 4]>,
    /// The DPI of the monitor it's on (96 is 100%), when known. Points on
    /// that monitor are what the overlay's window is measured in there.
    pub dpi: Option<u32>,
}

/// Whether the overlay can work here, or why not.
#[cfg(windows)]
pub fn support() -> Result<(), &'static str> {
    Ok(())
}

/// Whether the overlay can work here, or why not.
#[cfg(target_os = "linux")]
pub fn support() -> Result<(), &'static str> {
    // Focus comes from X11 either way: on Wayland, XWayland's (D75).
    let x11 = cfg!(feature = "x11") && std::env::var_os("DISPLAY").is_some();
    if wayland() {
        #[cfg(feature = "wayland")]
        let layer = super::overlay_wayland::available();
        #[cfg(not(feature = "wayland"))]
        let layer = false;
        return match (layer, x11) {
            (false, _) => {
                Err("Your desktop can't show overlays (it has no wlr-layer-shell; GNOME doesn't).")
            }
            (true, false) => Err("The overlay needs XWayland to spot games on Wayland."),
            (true, true) => Ok(()),
        };
    }
    if x11 {
        return Ok(());
    }
    Err("No display server found.")
}

/// Whether this is a Wayland session; the overlay is a layer surface then.
#[cfg(target_os = "linux")]
pub fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland")
}

/// Whether the overlay can work here, or why not.
#[cfg(not(any(target_os = "linux", windows)))]
pub fn support() -> Result<(), &'static str> {
    Err("The overlay isn't supported on this system.")
}

/// Watching focus; dropping it stops.
pub struct Focus {
    pub rx: watch::Receiver<Option<Target>>,
    /// The user's executables it matches too.
    pub custom: Vec<String>,
    stop: Arc<AtomicBool>,
}

impl Drop for Focus {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
    }
}

impl Focus {
    pub fn start(
        games: watch::Receiver<Option<Arc<Games>>>,
        custom: Vec<String>,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (tx, rx) = watch::channel(None);
        let stop = Arc::new(AtomicBool::new(false));
        let theirs = (stop.clone(), custom.clone());
        std::thread::spawn(move || {
            let (stop, custom) = theirs;
            let mut fg = Foreground::new();
            while !stop.load(Relaxed) {
                let target = fg.get().and_then(|(paths, rect, dpi)| {
                    let list = games.borrow().clone();
                    let game = list
                        .and_then(|g| g.running(&paths).map(str::to_owned))
                        .or_else(|| custom_game(&paths, &custom))?;
                    Some(Target { game, rect, dpi })
                });
                if tx.send_if_modified(|t| {
                    let changed = *t != target;
                    *t = target;
                    changed
                }) {
                    wake();
                }
                std::thread::sleep(EVERY);
            }
        });
        Self { rx, custom, stop }
    }
}

/// Finishes our overlay window, titled `title`, once per window: see-through
/// where egui draws nothing, no border or rounded corners, out of Alt+Tab,
/// and never activated by a click. Then puts it at `place` (x, y, width,
/// height in physical pixels) when it isn't there, on top again.
///
/// Placing it here, not through egui, is for mixed-DPI desktops: egui turns
/// points into pixels with the scale of the monitor a window is on *now*, so
/// a window made on IrChad's monitor lands in the wrong spot on the game's.
///
/// The see-through part is `CreateHook`'s, at creation; this only checks the
/// color key. The overlay never draws pure white, which the key hides.
///
/// Returns whether to look again soon: it had something to fix, or the window
/// is new (an idle overlay draws no more frames).
#[cfg(windows)]
pub fn finish_window(title: &str, place: Option<[i32; 4]>) -> bool {
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
        DwmSetWindowAttribute,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, GWL_STYLE, GetLayeredWindowAttributes, GetWindowLongPtrW, GetWindowRect,
        GetWindowThreadProcessId, HWND_TOPMOST, IsWindowVisible, LWA_COLORKEY, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetLayeredWindowAttributes,
        SetWindowLongPtrW, SetWindowPos, WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP,
        WS_SYSMENU, WS_THICKFRAME,
    };

    /// The window whose DWM attributes are set; a new one (after hiding)
    /// gets them again.
    static DONE: AtomicUsize = AtomicUsize::new(0);
    /// When `DONE` got its window.
    static BORN: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    /// How long a new window is looked at every frame.
    const SETTLE: Duration = Duration::from_secs(1);
    let mut fixed = false;
    let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
    // SAFETY: plain Win32 calls on a window of ours; every pointer outlives
    // its call and the attribute sizes match their types.
    unsafe {
        // Another IrChad (a second profile) may have a window by that name.
        let mut w = std::ptr::null_mut();
        let ours = loop {
            w = FindWindowExW(std::ptr::null_mut(), w, std::ptr::null(), title.as_ptr());
            if w.is_null() {
                // Not made yet.
                return true;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(w, &mut pid);
            if pid == std::process::id() {
                break w;
            }
        };
        if let Some([x, y, w, h]) = place {
            let mut r = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let there = GetWindowRect(ours, &mut r) != 0
                && [r.left, r.top, r.right - r.left, r.bottom - r.top] == [x, y, w, h];
            if !there {
                SetWindowPos(ours, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
                fixed = true;
            }
        }
        // A plain popup: winit keeps `WS_CAPTION` and the system menu on
        // undecorated windows, and DWM gives those a drop shadow.
        let frame =
            (WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX) as isize;
        let style = GetWindowLongPtrW(ours, GWL_STYLE);
        if style & frame != 0 || style & WS_POPUP as isize == 0 {
            SetWindowLongPtrW(ours, GWL_STYLE, (style & !frame) | WS_POPUP as isize);
            SetWindowPos(
                ours,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            fixed = true;
        }
        // The extended style and the key are set once, by `CreateHook`, and
        // the style is frozen from then on. Only the key is checked here, in
        // case something resets it.
        let (mut key, mut alpha, mut flags) = (0u32, 0u8, 0u32);
        let keyed = GetLayeredWindowAttributes(ours, &mut key, &mut alpha, &mut flags) != 0
            && flags & LWA_COLORKEY != 0
            && key == WHITE;
        if !keyed {
            tracing::debug!("overlay: color key set again");
            SetLayeredWindowAttributes(ours, WHITE, 0, LWA_COLORKEY);
            fixed = true;
        }
        let mut born = BORN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if DONE.swap(ours as usize, Relaxed) == ours as usize {
            let settling = born.is_none_or(|b| b.elapsed() < SETTLE);
            return fixed || settling || IsWindowVisible(ours) == 0;
        }
        *born = Some(Instant::now());
        // Never paint a caption (already done by `CreateHook` for a window
        // made under it).
        wrap_proc(ours);
        let none = DWMWA_COLOR_NONE;
        DwmSetWindowAttribute(
            ours,
            DWMWA_BORDER_COLOR as u32,
            (&raw const none).cast(),
            size_of::<u32>() as u32,
        );
        let square = DWMWCP_DONOTROUND;
        DwmSetWindowAttribute(
            ours,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&raw const square).cast(),
            size_of::<i32>() as u32,
        );
    }
    true
}

/// While alive, a window this thread creates titled `title` gets
/// `no_caption` as its window procedure at `HCBT_CREATEWND`, and with it the
/// overlay's whole extended style and color key on `WM_CREATE`, before winit
/// or wgpu touch it: layered and click-through, out of Alt+Tab and the
/// taskbar, never activated, and its white GDI redirection surface keyed out.
/// That surface sits above the DX12 DirectComposition content (wgpu's target
/// isn't topmost). The extended style is frozen after that.
///
/// Changing the extended style of a window that's already up is what left it
/// white, some of the time and always the first time: with winit and
/// `finish_window` restyling it after creation, DWM sometimes kept the
/// surface unkeyed, the key reading back as set. With every later change
/// refused, 0 in 12. `WS_EX_NOREDIRECTIONBITMAP` would be better, but
/// Windows takes it only from `CreateWindowEx` itself: not from the hook's
/// CREATESTRUCT, and `SetWindowLong` refuses any change to that bit
/// (ERROR_INVALID_PARAMETER). egui-winit doesn't expose it. Styles set in
/// the hook itself are overwritten by `CreateWindowEx`; on `WM_CREATE` they
/// stay (checked on a scratch window).
#[cfg(windows)]
pub struct CreateHook(windows_sys::Win32::UI::WindowsAndMessaging::HHOOK);

#[cfg(windows)]
static HOOK_TITLE: std::sync::Mutex<Vec<u16>> = std::sync::Mutex::new(Vec::new());

#[cfg(windows)]
impl CreateHook {
    pub fn new(title: &str) -> Self {
        use windows_sys::Win32::System::Threading::GetCurrentThreadId;
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_CBT};
        *HOOK_TITLE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = title.encode_utf16().collect();
        // SAFETY: a hook on our own thread, removed on drop; `on_create`
        // matches HOOKPROC.
        Self(unsafe {
            SetWindowsHookExW(
                WH_CBT,
                Some(on_create),
                std::ptr::null_mut(),
                GetCurrentThreadId(),
            )
        })
    }
}

#[cfg(windows)]
impl Drop for CreateHook {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the hook `new` installed.
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(self.0) };
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn on_create(
    code: i32,
    wp: windows_sys::Win32::Foundation::WPARAM,
    lp: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CBT_CREATEWNDW, CallNextHookEx, HCBT_CREATEWND,
    };
    // SAFETY: for HCBT_CREATEWND, wParam is the new window and lParam a valid
    // CBT_CREATEWNDW for the duration of the call; the name is read up to its
    // NUL, no further than our title's length.
    unsafe {
        if code == HCBT_CREATEWND as i32 {
            let cs = (*(lp as *const CBT_CREATEWNDW)).lpcs;
            let want = HOOK_TITLE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A class atom or ordinal is a small number, not a string.
            let name = if cs.is_null() {
                std::ptr::null()
            } else {
                (*cs).lpszName
            };
            let ours = name as usize > 0xffff && {
                let mut i = 0;
                while i < want.len() && *name.add(i) == want[i] {
                    i += 1;
                }
                i == want.len() && *name.add(i) == 0
            };
            if ours {
                // Styles set here don't last (CreateWindowEx sets its own
                // after the hook), so `no_caption` does it on WM_CREATE.
                wrap_proc(wp as windows_sys::Win32::Foundation::HWND);
                CREATED_UNDER_HOOK.store(true, Relaxed);
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
    }
}

/// Nothing to do: only Windows has a redirection surface.
#[cfg(not(windows))]
pub struct CreateHook;

#[cfg(not(windows))]
impl CreateHook {
    pub fn new(_title: &str) -> Self {
        Self
    }
}

/// The color key: the redirection surface's white (COLORREF).
#[cfg(windows)]
const WHITE: u32 = 0x00ff_ffff;

/// Set by `CreateHook` for the window it wraps, so `no_caption` styles it on
/// `WM_CREATE`.
#[cfg(windows)]
static CREATED_UNDER_HOOK: AtomicBool = AtomicBool::new(false);

/// Whether `no_caption` refuses changes to the window's extended style: from
/// its `WM_CREATE` on, for a window made under `CreateHook`.
#[cfg(windows)]
static EX_FROZEN: AtomicBool = AtomicBool::new(false);

/// winit's window procedure for the overlay, which `no_caption` wraps.
#[cfg(windows)]
static WINIT_PROC: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Puts `no_caption` in front of winit's window procedure, once.
///
/// # Safety
/// `w` is a window of this thread.
#[cfg(windows)]
unsafe fn wrap_proc(w: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWLP_WNDPROC, GetWindowLongPtrW, SetWindowLongPtrW,
    };
    let mine = no_caption as *const () as isize;
    // SAFETY: per the contract; the procedure stored is winit's, kept for
    // `no_caption` to call.
    unsafe {
        let prev = GetWindowLongPtrW(w, GWLP_WNDPROC);
        if prev != mine {
            WINIT_PROC.store(prev as usize, Relaxed);
            CREATED_UNDER_HOOK.store(false, Relaxed);
            EX_FROZEN.store(false, Relaxed);
            SetWindowLongPtrW(w, GWLP_WNDPROC, mine);
        }
    }
}

/// The overlay's window procedure: winit's, minus non-client painting, and
/// with the styles the overlay needs kept on whatever winit sets.
///
/// `WM_NCACTIVATE` still reaches winit (it tracks focus with it), with an
/// lParam of -1, which tells `DefWindowProc` not to repaint the frame.
/// `WM_STYLECHANGING` is edited before winit sees it: the extended style
/// stays as `CreateHook` set it (changing it can leave the window white),
/// and the style stays a plain popup with no caption (which would get a DWM
/// drop shadow). A click doesn't activate it (`WM_MOUSEACTIVATE`), should
/// one ever reach it.
#[cfg(windows)]
unsafe extern "system" fn no_caption(
    w: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wp: windows_sys::Win32::Foundation::WPARAM,
    lp: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, DefWindowProcW, GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, LWA_COLORKEY,
        MA_NOACTIVATE, STYLESTRUCT, SetLayeredWindowAttributes, SetWindowLongPtrW, WM_CREATE,
        WM_MOUSEACTIVATE, WM_NCACTIVATE, WM_NCPAINT, WM_STYLECHANGING, WNDPROC, WS_CAPTION,
        WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
        WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
    };
    let prev = WINIT_PROC.load(Relaxed);
    // SAFETY: `prev` is the procedure `GetWindowLongPtrW` returned for this
    // window before ours replaced it, so a valid WNDPROC (or 0, `None`). For
    // WM_STYLECHANGING, lParam is a valid, writable STYLESTRUCT.
    unsafe {
        let winit: WNDPROC = std::mem::transmute::<usize, WNDPROC>(prev);
        if winit.is_none() {
            return DefWindowProcW(w, msg, wp, lp);
        }
        match msg {
            WM_NCPAINT => 0,
            WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
            WM_NCACTIVATE => CallWindowProcW(winit, w, msg, wp, -1),
            WM_CREATE => {
                let r = CallWindowProcW(winit, w, msg, wp, lp);
                if CREATED_UNDER_HOOK.load(Relaxed) {
                    let want =
                        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
                    let ex = (GetWindowLongPtrW(w, GWL_EXSTYLE) | want as isize)
                        & !(WS_EX_APPWINDOW as isize);
                    SetWindowLongPtrW(w, GWL_EXSTYLE, ex);
                    SetLayeredWindowAttributes(w, WHITE, 0, LWA_COLORKEY);
                    EX_FROZEN.store(true, Relaxed);
                    tracing::debug!(
                        ex = format_args!("{:x}", GetWindowLongPtrW(w, GWL_EXSTYLE)),
                        "overlay: window styled at creation"
                    );
                }
                r
            }
            WM_STYLECHANGING => {
                let s = &mut *(lp as *mut STYLESTRUCT);
                if wp as i32 == GWL_EXSTYLE && EX_FROZEN.load(Relaxed) {
                    s.styleNew = s.styleOld;
                } else if wp as i32 == GWL_STYLE {
                    let frame =
                        WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
                    s.styleNew = (s.styleNew & !frame) | WS_POPUP;
                }
                CallWindowProcW(winit, w, msg, wp, lp)
            }
            _ => CallWindowProcW(winit, w, msg, wp, lp),
        }
    }
}

/// Nothing to finish: winit's own see-through windows work, and egui places them.
#[cfg(not(windows))]
pub fn finish_window(_title: &str, _place: Option<[i32; 4]>) -> bool {
    false
}

/// The focused window's program, as its possible paths, its rectangle, and
/// its monitor's DPI.
type Seen = (Vec<String>, Option<[i32; 4]>, Option<u32>);

#[cfg(windows)]
struct Foreground;

#[cfg(windows)]
impl Foreground {
    fn new() -> Self {
        Self
    }

    fn get(&mut self) -> Option<Seen> {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
        };
        // SAFETY: plain Win32 calls; `pid` and `r` outlive them, and
        // `full_path` closes its handle.
        unsafe {
            let w = GetForegroundWindow();
            if w.is_null() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(w, &mut pid);
            if pid == 0 || pid == std::process::id() {
                return None;
            }
            let path = super::presence_windows::full_path(pid)?;
            let mut r = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let rect = (GetWindowRect(w, &mut r) != 0)
                .then(|| [r.left, r.top, r.right - r.left, r.bottom - r.top]);
            let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(w);
            Some((vec![path], rect, (dpi != 0).then_some(dpi)))
        }
    }
}

/// X11: `_NET_ACTIVE_WINDOW`'s `_NET_WM_PID`, over one connection kept
/// between polls. On Wayland that's XWayland's, which KWin and wlroots clear
/// while a Wayland window has focus. Hyprland doesn't (nor
/// `_NET_WM_STATE_FOCUSED`), so there its own IPC is asked instead.
///
/// shortcut: native-Wayland games are spotted only on Hyprland; ask Sway's
/// IPC or a KWin script when someone needs them.
#[cfg(all(target_os = "linux", feature = "x11"))]
struct Foreground(Option<X11>);

/// Hyprland's IPC socket, when this is a Hyprland session.
#[cfg(all(target_os = "linux", feature = "x11"))]
fn hyprland_socket() -> Option<std::path::PathBuf> {
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let run = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(
        std::path::Path::new(&run)
            .join("hypr")
            .join(sig)
            .join(".socket.sock"),
    )
}

/// The focused window's process, from Hyprland's `activewindow`.
#[cfg(all(target_os = "linux", feature = "x11"))]
fn hyprland(socket: &std::path::Path) -> Result<Option<Seen>, Box<dyn std::error::Error>> {
    use std::io::{Read as _, Write as _};
    /// Bounds the reply: another program writes it.
    const MAX_REPLY: u64 = 64 * 1024;
    let mut s = std::os::unix::net::UnixStream::connect(socket)?;
    s.set_read_timeout(Some(Duration::from_secs(1)))?;
    s.set_write_timeout(Some(Duration::from_secs(1)))?;
    s.write_all(b"j/activewindow")?;
    let mut reply = Vec::new();
    s.take(MAX_REPLY).read_to_end(&mut reply)?;
    // `{}` when nothing has focus.
    let v: serde_json::Value = serde_json::from_slice(&reply)?;
    let pid = v["pid"].as_u64().and_then(|p| u32::try_from(p).ok());
    let Some(pid) = pid.filter(|p| *p != 0 && *p != std::process::id()) else {
        return Ok(None);
    };
    let paths = super::presence_linux::paths(&std::path::Path::new("/proc").join(pid.to_string()));
    // The rect isn't needed: the layer surface sits in the output's corner.
    Ok((!paths.is_empty()).then_some((paths, None, None)))
}

#[cfg(all(target_os = "linux", feature = "x11"))]
struct X11 {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
    active: u32,
    pid: u32,
}

#[cfg(all(target_os = "linux", feature = "x11"))]
impl Foreground {
    fn new() -> Self {
        Self(None)
    }

    fn get(&mut self) -> Option<Seen> {
        if let Some(socket) = hyprland_socket() {
            return hyprland(&socket)
                .inspect_err(|e| tracing::info!("overlay: Hyprland: {e}"))
                .ok()
                .flatten();
        }
        if self.0.is_none() {
            self.0 = X11::connect()
                .inspect_err(|e| tracing::info!("overlay: X11: {e}"))
                .ok();
        }
        let seen = self.0.as_ref()?.get();
        match seen {
            Ok(s) => s,
            Err(e) => {
                // Reconnect next time.
                tracing::info!("overlay: X11: {e}");
                self.0 = None;
                None
            }
        }
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
impl X11 {
    fn connect() -> Result<Self, Box<dyn std::error::Error>> {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::ConnectionExt as _;
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots.get(screen).ok_or("no screen")?.root;
        let active = conn
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")?
            .reply()?
            .atom;
        let pid = conn.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
        Ok(Self {
            conn,
            root,
            active,
            pid,
        })
    }

    fn get(&self) -> Result<Option<Seen>, Box<dyn std::error::Error>> {
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
        let c = &self.conn;
        let reply = c
            .get_property(false, self.root, self.active, AtomEnum::WINDOW, 0, 1)?
            .reply()?;
        let Some(win) = reply
            .value32()
            .and_then(|mut v| v.next())
            .filter(|w| *w != 0)
        else {
            return Ok(None);
        };
        let reply = c
            .get_property(false, win, self.pid, AtomEnum::CARDINAL, 0, 1)?
            .reply()?;
        let Some(pid) = reply
            .value32()
            .and_then(|mut v| v.next())
            .filter(|p| *p != 0)
        else {
            return Ok(None);
        };
        if pid == std::process::id() {
            return Ok(None);
        }
        let paths =
            super::presence_linux::paths(&std::path::Path::new("/proc").join(pid.to_string()));
        if paths.is_empty() {
            return Ok(None);
        }
        let geo = c.get_geometry(win)?.reply()?;
        let at = c.translate_coordinates(win, self.root, 0, 0)?.reply()?;
        let rect = [
            i32::from(at.dst_x),
            i32::from(at.dst_y),
            i32::from(geo.width),
            i32::from(geo.height),
        ];
        // shortcut: X11 has one scale for the whole screen, the main window's.
        Ok(Some((paths, Some(rect), None)))
    }
}

#[cfg(not(any(windows, all(target_os = "linux", feature = "x11"))))]
struct Foreground;

#[cfg(not(any(windows, all(target_os = "linux", feature = "x11"))))]
impl Foreground {
    fn new() -> Self {
        Self
    }

    #[allow(clippy::unused_self)]
    fn get(&mut self) -> Option<Seen> {
        None
    }
}
