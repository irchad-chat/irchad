//! Windows: the time of the last input anywhere in the session.

use std::time::{Duration, Instant};

use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

use super::idle::Since;

const STEP: Duration = Duration::from_secs(60);

pub fn watch(since: Since, wake: Box<dyn Fn() + Send + Sync>) -> bool {
    std::thread::spawn(move || {
        loop {
            let mut info = LASTINPUTINFO {
                cbSize: size_of::<LASTINPUTINFO>() as u32,
                dwTime: 0,
            };
            // SAFETY: `info` is a valid LASTINPUTINFO with cbSize set.
            let ok = unsafe { GetLastInputInfo(&mut info) } != 0;
            // SAFETY: no arguments. Both are 32-bit tick counts that wrap together.
            let idle =
                Duration::from_millis(unsafe { GetTickCount() }.wrapping_sub(info.dwTime).into());
            let now = (ok && idle >= STEP)
                .then(|| Instant::now().checked_sub(idle))
                .flatten();
            // Scoped: the UI locks this every frame, so never hold it across the sleep.
            let changed = {
                let mut s = since.lock().unwrap_or_else(|e| e.into_inner());
                let changed = s.is_some() != now.is_some();
                if changed {
                    *s = now;
                }
                changed
            };
            if changed {
                wake();
            }
            std::thread::sleep(Duration::from_secs(10));
        }
    });
    true
}
