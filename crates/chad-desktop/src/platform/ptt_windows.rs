//! Windows: the key's async state, which works whatever app is focused.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

use super::ptt::{Binds, PttBind, PttStatus, Report, Sink, keymap};

const VK_CONTROL: u16 = 0x11;
const VK_SHIFT: u16 = 0x10;
const VK_MENU: u16 = 0x12;

// shortcut: polls GetAsyncKeyState every 5 ms instead of raw input (which needs
// a message window); move to RegisterRawInputDevices if the polling shows up.
pub fn listen(sink: Sink, binds: Binds, stop: Arc<AtomicBool>, report: Report) {
    // Per bind: (what it does, virtual key, modifier keys, held).
    let mut keys = Vec::new();
    for &(action, combo) in &binds {
        let vk: u16 = match combo.bind {
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
        let m = combo.mods;
        let mods: Vec<u16> = [(m.ctrl, VK_CONTROL), (m.shift, VK_SHIFT), (m.alt, VK_MENU)]
            .into_iter()
            .filter_map(|(on, vk)| on.then_some(vk))
            .collect();
        keys.push((action, vk, mods, false));
    }
    let names: Vec<String> = binds.iter().map(|(_, b)| b.name()).collect();
    report(PttStatus::Global(format!(
        "{} works in every app.",
        names.join(" and ")
    )));
    std::thread::spawn(move || {
        // SAFETY: plain Win32 call, no pointers.
        let held = |vk: u16| unsafe { GetAsyncKeyState(i32::from(vk)) } as u16 & 0x8000 != 0;
        while !stop.load(Relaxed) {
            for (action, vk, mods, was) in &mut keys {
                // Modifiers only gate the press: letting go of one first still releases.
                let down = held(*vk) && (*was || mods.iter().all(|m| held(*m)));
                if down != *was {
                    sink.set(*action, down);
                    *was = down;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
}
