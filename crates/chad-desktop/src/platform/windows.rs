use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};

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

/// shortcut: no tray on Windows yet (Phase 1 only asks for SNI); add a
/// Shell_NotifyIcon tray in Phase 5 polish.
pub struct Tray;

impl Tray {
    pub async fn spawn(_tx: OsTx) -> Self {
        Self
    }

    pub fn set(&self, _unread: bool, _mentions: u32) {}
}
