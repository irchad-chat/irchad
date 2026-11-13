//! Running programs from a ToolHelp snapshot, with full paths where Windows
//! lets us read them, and media sessions from GlobalSystemMediaTransportControls.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use windows::ApplicationModel::AppInfo;
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    MediaPropertiesChangedEventArgs as MediaArgs, PlaybackInfoChangedEventArgs as PlaybackArgs,
    SessionsChangedEventArgs as SessionsArgs,
};
use windows::Media::MediaPlaybackType;
use windows::Win32::System::Com::CoIncrementMTAUsage;

use super::presence::{Raw, Track};
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
pub(super) unsafe fn full_path(pid: u32) -> Option<String> {
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
pub struct Media {
    mgr: Option<(SessionManager, i64)>,
    /// Sessions we listen to, with their tokens (media, playback).
    hooked: Vec<(Session, i64, i64)>,
    /// Poked from WinRT's threads when anything changes.
    changes: Arc<Notify>,
}

impl Drop for Media {
    fn drop(&mut self) {
        self.unhook();
    }
}

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
        Self {
            mgr: None,
            hooked: Vec::new(),
            changes: Arc::new(Notify::new()),
        }
    }

    /// Waits for a session to change track or state, or to come or go.
    pub async fn changed(&mut self) {
        self.changes.notified().await;
        // A track change comes as a burst of events: one poll for all of them.
        tokio::time::sleep(super::presence::SETTLE).await;
    }

    fn unhook(&mut self) {
        for (s, a, b) in self.hooked.drain(..) {
            let _ = s.RemoveMediaPropertiesChanged(a);
            let _ = s.RemovePlaybackInfoChanged(b);
        }
        if let Some((m, t)) = self.mgr.take() {
            let _ = m.RemoveSessionsChanged(t);
        }
    }

    /// Every session's app id, and the first one in `allowed` that's
    /// playing. Others aren't asked.
    pub async fn now_playing(&mut self, allowed: &[String]) -> (Vec<String>, Option<Track>) {
        if self.mgr.is_none() {
            if let Some(m) = manager().await {
                let token = m.SessionsChanged(&poke::<SessionManager, SessionsArgs>(&self.changes));
                self.mgr = token.ok().map(|t| (m, t));
            }
        }
        let Some((mgr, _)) = self.mgr.as_ref() else {
            return Default::default();
        };
        // The list view and its iterator aren't `Send`: copy out before awaiting.
        let sessions: Option<Vec<Session>> = mgr
            .GetSessions()
            .ok()
            .map(|list| list.into_iter().take(MAX_SESSIONS).collect());
        let Some(sessions) = sessions else {
            self.unhook(); // ask for a new manager next time
            return Default::default();
        };
        self.hook(&sessions);
        let mut players: Vec<String> = Vec::new();
        let mut track = None;
        for s in &sessions {
            let Some(id) = player_id(s) else {
                continue;
            };
            if track.is_none() && allowed.contains(&id) {
                track = tokio::time::timeout(PLAYER_WAIT, playing(s, &id))
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

impl Media {
    /// Listens to sessions we haven't yet, and lets go of ended ones.
    fn hook(&mut self, sessions: &[Session]) {
        let gone: Vec<usize> = (0..self.hooked.len())
            .filter(|i| !sessions.contains(&self.hooked[*i].0))
            .collect();
        for i in gone.into_iter().rev() {
            let (s, a, b) = self.hooked.swap_remove(i);
            let _ = s.RemoveMediaPropertiesChanged(a);
            let _ = s.RemovePlaybackInfoChanged(b);
        }
        for s in sessions {
            if self.hooked.iter().any(|(h, ..)| h == s) {
                continue;
            }
            let a = s.MediaPropertiesChanged(&poke::<Session, MediaArgs>(&self.changes));
            let b = s.PlaybackInfoChanged(&poke::<Session, PlaybackArgs>(&self.changes));
            if let (Ok(a), Ok(b)) = (a, b) {
                self.hooked.push((s.clone(), a, b));
            }
        }
    }
}

/// A WinRT event handler that wakes [`Media::changed`].
fn poke<S, A>(n: &Arc<Notify>) -> TypedEventHandler<S, A>
where
    S: windows::core::RuntimeType + 'static,
    A: windows::core::RuntimeType + 'static,
{
    let n = n.clone();
    TypedEventHandler::new(move |_, _| {
        n.notify_one();
        Ok(())
    })
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

/// 100 ns ticks from 1601 to 1970, for WinRT's `DateTime`.
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

/// `s`'s track, if it's playing one. `id` is its app id.
async fn playing(s: &Session, id: &str) -> Option<Track> {
    let info = s.GetPlaybackInfo().ok()?;
    if info.PlaybackStatus().ok()? != Status::Playing {
        return None;
    }
    let video = info
        .PlaybackType()
        .and_then(|t| t.Value())
        .is_ok_and(|t| t == MediaPlaybackType::Video);
    let props = s.TryGetMediaPropertiesAsync().ok()?.await.ok()?;
    let title = props.Title().ok()?.to_string_lossy();
    let artist = props
        .Artist()
        .map(|a| a.to_string_lossy())
        .unwrap_or_default();
    let artists: Vec<String> = Some(artist).filter(|a| !a.is_empty()).into_iter().collect();
    // The position was `Position` at `LastUpdatedTime`; ticks are 100 ns.
    let (mut position_ms, mut length_ms) = (None, None);
    if let Ok(t) = s.GetTimelineProperties() {
        let (pos, end, start, at) = (
            t.Position(),
            t.EndTime(),
            t.StartTime(),
            t.LastUpdatedTime(),
        );
        if let (Ok(pos), Ok(end), Ok(start), Ok(at)) = (pos, end, start, at) {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
            let at_ms = at.UniversalTime.saturating_sub(UNIX_EPOCH_TICKS) / 10_000;
            let since = now_ms.saturating_sub(at_ms).max(0);
            position_ms = Some((pos.Duration / 10_000).saturating_add(since));
            length_ms = Some(end.Duration.saturating_sub(start.Duration) / 10_000);
        }
    }
    let identity = AppInfo::GetFromAppUserModelId(&windows::core::HSTRING::from(id))
        .and_then(|a| a.DisplayInfo())
        .and_then(|d| d.DisplayName())
        .ok()
        .map(|n| n.to_string_lossy());
    Track::new(Raw {
        title: &title,
        artists: &artists,
        id,
        identity: identity.as_deref(),
        url: None,
        position_ms,
        length_ms,
        video,
    })
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
