//! Running programs from a ToolHelp snapshot, with full paths where Windows
//! lets us read them, and media sessions from GlobalSystemMediaTransportControls.

use std::time::Duration;

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

const MAX_PROCESSES: usize = 4096;

pub fn programs() -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: plain Win32 calls; `e` is sized as the API asks and the snapshot
    // handle is closed on every path.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snap, &mut e) != 0;
        while more && out.len() < MAX_PROCESSES {
            // A bare name could be a protected system program (explorer.exe)
            // that some game shares a name with: only full paths count.
            out.extend(full_path(e.th32ProcessID));
            more = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

/// SAFETY: as above; the process handle is closed before returning.
unsafe fn full_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]))
    }
}

/// Per player call, so one that hangs doesn't hide the others.
const PLAYER_WAIT: Duration = Duration::from_secs(1);
/// Players listed for the settings page; other programs pick how many there are.
const MAX_PLAYERS: usize = 32;
/// Sessions looked at per poll, and the longest app id kept (others pick both).
const MAX_SESSIONS: usize = 256;
const MAX_ID: usize = 256;

/// Media sessions from WinRT's GlobalSystemMediaTransportControls (what the
/// taskbar's media flyout shows), over one manager kept between polls.
pub struct Media(Option<SessionManager>);

impl Media {
    pub fn new() -> Self {
        // Tokio's threads join no COM apartment, and the poll moves between
        // them at each await. Keeping the process's implicit MTA alive lets
        // WinRT run on all of them. The cookie is never given back: once is
        // enough, and it only matters until exit.
        // SAFETY: no preconditions; the cookie is just dropped.
        if let Err(e) = unsafe { CoIncrementMTAUsage() } {
            tracing::debug!("media: CoIncrementMTAUsage: {e}");
        }
        Self(None)
    }

    /// Every session's app id, and the first one in `allowed` that's
    /// playing, as "Title by Artist". Others aren't asked.
    pub async fn now_playing(&mut self, allowed: &[String]) -> (Vec<String>, Option<String>) {
        if self.0.is_none() {
            self.0 = manager().await;
        }
        let Some(mgr) = self.0.as_ref() else {
            return Default::default();
        };
        // The list view and its iterator aren't `Send`: copy out before awaiting.
        let sessions: Option<Vec<Session>> = mgr
            .GetSessions()
            .ok()
            .map(|list| list.into_iter().take(MAX_SESSIONS).collect());
        let Some(sessions) = sessions else {
            self.0 = None; // ask for a new manager next time
            return Default::default();
        };
        let mut players: Vec<String> = Vec::new();
        let mut track = None;
        for s in &sessions {
            let Some(id) = player_id(s) else {
                continue;
            };
            if track.is_none() && allowed.contains(&id) {
                track = tokio::time::timeout(PLAYER_WAIT, playing(s))
                    .await
                    .ok()
                    .flatten();
            }
            if !players.contains(&id) && players.len() < MAX_PLAYERS {
                players.push(id);
            }
        }
        players.sort();
        (players, track)
    }
}

async fn manager() -> Option<SessionManager> {
    let req = SessionManager::RequestAsync().ok()?;
    match req.await {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::debug!("media: no session manager: {e}");
            None
        }
    }
}

/// The app's id (AUMID), e.g. `Spotify.exe` or a Store app's
/// `Family_hash!App`; it's stable across restarts, so it's what gets ticked.
/// Any app may pick it, so an empty or overlong one is skipped.
fn player_id(s: &Session) -> Option<String> {
    let id = s.SourceAppUserModelId().ok()?;
    (!id.is_empty() && id.len() <= MAX_ID).then(|| id.to_string_lossy())
}

/// `s`'s track, if it's playing one.
async fn playing(s: &Session) -> Option<String> {
    let status = s.GetPlaybackInfo().ok()?.PlaybackStatus().ok()?;
    if status != Status::Playing {
        return None;
    }
    let props = s.TryGetMediaPropertiesAsync().ok()?.await.ok()?;
    let title = props.Title().ok()?.to_string_lossy();
    let artist = props
        .Artist()
        .map(|a| a.to_string_lossy())
        .unwrap_or_default();
    let artists: Vec<String> = Some(artist).filter(|a| !a.is_empty()).into_iter().collect();
    super::presence::track(&title, &artists)
}

#[cfg(test)]
mod tests {
    /// Prints what GSMTC says is playing from any player.
    #[tokio::test]
    #[ignore = "needs a desktop session"]
    async fn print_now_playing() {
        let mut m = super::Media::new();
        let (players, _) = m.now_playing(&[]).await;
        let (_, track) = m.now_playing(&players).await;
        println!("players {players:?}, playing {track:?}");
    }
}
