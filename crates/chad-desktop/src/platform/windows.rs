use std::cell::{Cell, OnceCell};
use std::ptr::{null, null_mut};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFY_ICON_MESSAGE,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, MF_STRING, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON,
    SendMessageW, SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TrackPopupMenu,
    TranslateMessage, WM_APP, WM_CLOSE, WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW,
};

use super::{Os, OsTx};

/// Per user, so two accounts on one PC each get their own instance.
fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let user: String = user.chars().filter(char::is_ascii_alphanumeric).collect();
    format!(r"\\.\pipe\party.gentoo.IrChad-{user}")
}

pub struct Instance;

/// `None` when another instance is running and got `url`: exit.
pub async fn single_instance(url: Option<String>, tx: OsTx) -> Option<Instance> {
    let name = pipe_name();
    let first = match ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(&name)
    {
        Ok(s) => s,
        Err(_) => {
            // Someone holds the name: hand them the URL (or just "show").
            match ClientOptions::new().open(&name) {
                Ok(mut c) => {
                    let line = format!("{}\n", url.unwrap_or_default().replace('\n', ""));
                    if c.write_all(line.as_bytes()).await.is_ok() {
                        return None;
                    }
                }
                Err(e) => tracing::warn!("couldn't reach the running IrChad: {e}"),
            }
            return Some(Instance);
        }
    };
    tokio::spawn(async move {
        let mut server = first;
        loop {
            let connected = server.connect().await;
            let next = match ServerOptions::new()
                .reject_remote_clients(true)
                .create(&name)
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("single instance pipe: {e}");
                    return;
                }
            };
            let conn = std::mem::replace(&mut server, next);
            if connected.is_err() {
                continue;
            }
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut line = String::new();
                // Bounded: a URL, not a stream.
                let mut r = BufReader::new(tokio::io::AsyncReadExt::take(conn, 4096));
                if r.read_line(&mut line).await.is_ok() {
                    let url = line.trim();
                    if !url.is_empty() {
                        tx.send(Os::Open(url.to_owned()));
                    }
                    tx.send(Os::Show);
                }
            });
        }
    });
    Some(Instance)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn set(key: &str, name: Option<&str>, value: &str) {
    let key = wide(key);
    let name = name.map(wide);
    let value = wide(value);
    // SAFETY: all strings are NUL-terminated and outlive the call; the size is in bytes.
    let r = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    if r != 0 {
        tracing::warn!("registry: setting a value failed ({r})");
    }
}

/// `irchad://` → this exe, per user (HKCU, no admin needed).
pub fn register_scheme() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let base = r"Software\Classes\irchad";
    set(base, None, "URL:IrChad");
    set(base, Some("URL Protocol"), "");
    set(
        &format!(r"{base}\shell\open\command"),
        None,
        &format!("\"{}\" \"%1\"", exe.display()),
    );
    // Toasts show this name; without a registered AUMID they claim to be PowerShell.
    set(
        &format!(r"Software\Classes\AppUserModelId\{}", super::APP_ID),
        Some("DisplayName"),
        "IrChad",
    );
}

/// Starts IrChad at sign-in (HKCU Run key). Rewritten each launch, like the
/// scheme, so a moved exe still starts.
pub fn autostart(on: bool) {
    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    if on {
        if let Ok(exe) = std::env::current_exe() {
            set(RUN, Some("IrChad"), &format!("\"{}\"", exe.display()));
        }
        return;
    }
    let (key, name) = (wide(RUN), wide("IrChad"));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
}

const WM_TRAY: u32 = WM_APP + 1;
const WM_BADGE: u32 = WM_APP + 2;

// The tray window's state; only its own thread touches these.
thread_local! {
    static TX: OnceCell<OsTx> = const { OnceCell::new() };
    static BADGE: Cell<(bool, u32)> = const { Cell::new((false, 0)) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

/// Notification area icon, owned by a hidden window on its own thread.
/// Holds the HWND as an integer so the handle is `Send`.
pub struct Tray(Option<usize>);

impl Tray {
    pub async fn spawn(tx: OsTx) -> Self {
        let (hwnd_tx, hwnd_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            TX.with(|t| {
                let _ = t.set(tx);
            });
            let hwnd = create_window();
            let _ = hwnd_tx.send((!hwnd.is_null()).then_some(hwnd as usize));
            if hwnd.is_null() {
                return;
            }
            notify(hwnd, NIM_ADD);
            let mut msg = MSG::default();
            // SAFETY: `msg` is a valid out pointer; this thread owns the window.
            while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
                // SAFETY: `msg` came from GetMessageW.
                unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        });
        let hwnd = hwnd_rx.recv().ok().flatten();
        if hwnd.is_none() {
            tracing::info!("no tray: couldn't create its window");
        }
        Self(hwnd)
    }

    /// The icon is up, so a hidden window can be brought back.
    pub fn present(&self) -> bool {
        self.0.is_some()
    }

    pub fn set(&self, unread: bool, mentions: u32) {
        if let Some(h) = self.0 {
            // SAFETY: plain values; a stale HWND just fails.
            unsafe { PostMessageW(h as HWND, WM_BADGE, usize::from(unread), mentions as isize) };
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if let Some(h) = self.0 {
            // Synchronous, so the icon is gone before the process is.
            // SAFETY: plain values; a stale HWND just fails.
            unsafe { SendMessageW(h as HWND, WM_CLOSE, 0, 0) };
        }
    }
}

fn create_window() -> HWND {
    let class = wide("IrChadTray");
    let taskbar = wide("TaskbarCreated");
    // SAFETY: the strings are NUL-terminated and outlive the calls; the class
    // is registered before the window that uses it.
    unsafe {
        let hinst = GetModuleHandleW(null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        // Explorer restarts drop every icon; this says when to add ours back.
        TASKBAR_CREATED.set(RegisterWindowMessageW(taskbar.as_ptr()));
        // Top level (not HWND_MESSAGE) and never shown: message-only windows
        // miss the TaskbarCreated broadcast.
        CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        )
    }
}

fn notify(hwnd: HWND, msg: NOTIFY_ICON_MESSAGE) {
    let (unread, mentions) = BADGE.get();
    // SAFETY: plain Win32 call, no pointers.
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 256) as usize;
    // ARGB → BGRA, the byte order of a 32-bit DIB.
    let bgra: Vec<u8> = super::tray_icon(size, unread, mentions > 0)
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[a, r, g, b]| [b, g, r, a])
        .collect();
    // AND mask rows are WORD aligned; all zero since alpha does the masking.
    let mask = vec![0u8; size.div_ceil(16) * 2 * size];
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        ..Default::default()
    };
    for (d, s) in nid
        .szTip
        .iter_mut()
        .zip(super::tray_title(mentions).encode_utf16().take(127))
    {
        *d = s;
    }
    // SAFETY: the buffers match the size and depth given; the shell copies the
    // icon, so it's ours to destroy after.
    unsafe {
        nid.hIcon = CreateIcon(
            GetModuleHandleW(null()),
            size as i32,
            size as i32,
            1,
            32,
            mask.as_ptr(),
            bgra.as_ptr(),
        );
        Shell_NotifyIconW(msg, &nid);
        if !nid.hIcon.is_null() {
            DestroyIcon(nid.hIcon);
        }
    }
}

/// The right-click menu; returns the chosen item (0: none).
fn menu(hwnd: HWND) -> i32 {
    let (show, quit) = (wide("Show IrChad"), wide("Quit"));
    // SAFETY: the strings outlive the menu, which is destroyed before returning.
    unsafe {
        let m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, 1, show.as_ptr());
        AppendMenuW(m, MF_STRING, 2, quit.as_ptr());
        let mut pt = POINT::default();
        GetCursorPos(&mut pt);
        // Without this the menu stays up when clicking elsewhere.
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd, null());
        DestroyMenu(m);
        cmd
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let send = |m| {
        TX.with(|t| {
            if let Some(tx) = t.get() {
                tx.send(m);
            }
        })
    };
    match msg {
        // Version 0 callbacks: the mouse message is the whole LPARAM.
        WM_TRAY => match lp as u32 {
            WM_LBUTTONUP => send(Os::Show),
            WM_RBUTTONUP => match menu(hwnd) {
                1 => send(Os::Show),
                2 => send(Os::Quit),
                _ => {}
            },
            _ => {}
        },
        WM_BADGE => {
            BADGE.set((wp != 0, lp as u32));
            notify(hwnd, NIM_MODIFY);
        }
        WM_DESTROY => {
            let nid = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                ..Default::default()
            };
            // SAFETY: `nid` is valid for the call.
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &nid);
                PostQuitMessage(0);
            }
        }
        m if m != 0 && m == TASKBAR_CREATED.get() => notify(hwnd, NIM_ADD),
        // SAFETY: forwarding the arguments we were given.
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    0
}
