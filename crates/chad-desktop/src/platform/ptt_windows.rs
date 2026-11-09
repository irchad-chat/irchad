//! Windows: the key's async state, which works whatever app is focused.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use chad_media::Voice;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

use super::ptt::{Binds, PttBind, PttStatus, Report, keymap};

// shortcut: polls GetAsyncKeyState every 5 ms instead of raw input (which needs
// a message window); move to RegisterRawInputDevices if the polling shows up.
pub fn listen(voice: Voice, binds: Binds, stop: Arc<AtomicBool>, report: Report) {
    // Per bind: (what it does, virtual key, held).
    let mut keys = Vec::new();
    for &(action, bind) in &binds {
        let vk: u16 = match bind {
            PttBind::Key(k) => match keymap(k) {
                Some((_, _, vk)) => vk,
                None => {
                    return report(PttStatus::FocusedOnly(format!(
                        "{} can't be a global key.",
                        k.name()
                    )));
                }
            },
            PttBind::Mouse4 => 0x05, // VK_XBUTTON1
            PttBind::Mouse5 => 0x06, // VK_XBUTTON2
        };
        keys.push((action, vk, false));
    }
    let names: Vec<String> = binds.iter().map(|(_, b)| b.name()).collect();
    report(PttStatus::Global(format!(
        "{} works in every app.",
        names.join(" and ")
    )));
    std::thread::spawn(move || {
        while !stop.load(Relaxed) {
            for (action, vk, was) in &mut keys {
                // SAFETY: plain Win32 call, no pointers.
                let down = unsafe { GetAsyncKeyState(i32::from(*vk)) } as u16 & 0x8000 != 0;
                if down != *was {
                    action.set(&voice, down);
                    *was = down;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
}
