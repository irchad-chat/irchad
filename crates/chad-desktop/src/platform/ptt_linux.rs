//! Wayland: GlobalShortcuts portal. X11: XInput2 raw key/button events.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use futures_util::StreamExt;

use super::ptt::{Binds, PttBind, PttStatus, Report, Sink, keymap};

pub fn listen(sink: Sink, binds: Binds, stop: Arc<AtomicBool>, report: Report) {
    let release = {
        let binds = binds.clone();
        move |sink: &Sink| {
            for (action, _) in &binds {
                sink.set(*action, false);
            }
        }
    };
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland");
    if wayland {
        tokio::spawn(async move {
            if let Err(e) = portal(&sink, &binds, &stop, &report).await {
                tracing::info!("GlobalShortcuts portal: {e}");
                report(PttStatus::FocusedOnly(
                    "Your desktop doesn't offer global shortcuts to apps.".into(),
                ));
            }
            release(&sink);
        });
    } else if cfg!(feature = "x11") && std::env::var_os("DISPLAY").is_some() {
        #[cfg(feature = "x11")]
        std::thread::spawn(move || {
            if let Err(e) = x11(&sink, &binds, &stop, &report) {
                tracing::warn!("XInput2: {e}");
                report(PttStatus::FocusedOnly(
                    "Couldn't watch the keyboard.".into(),
                ));
            }
            release(&sink);
        });
    } else {
        report(PttStatus::FocusedOnly("No display server found.".into()));
    }
}

/// All binds in one session, so the desktop asks once.
async fn portal(
    sink: &Sink,
    binds: &Binds,
    stop: &AtomicBool,
    report: &Report,
) -> ashpd::Result<()> {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    let gs = GlobalShortcuts::new().await?;
    let session = gs.create_session(Default::default()).await?;
    let shortcuts: Vec<_> = binds
        .iter()
        .map(|(action, combo)| {
            // Only a hint: the desktop decides, and the user rebinds in its settings.
            let trigger = match combo.bind {
                PttBind::Key(k) => keymap(k).map(|(_, name, _)| combo.mods.trigger() + &name),
                PttBind::Mouse4 | PttBind::Mouse5 => None,
            };
            NewShortcut::new(action.id(), action.label()).preferred_trigger(trigger.as_deref())
        })
        .collect();
    let bound = gs
        .bind_shortcuts(&session, &shortcuts, None, Default::default())
        .await?
        .response()?;
    let how: Vec<String> = binds
        .iter()
        .map(|(action, _)| {
            let how = bound
                .shortcuts()
                .iter()
                .find(|s| s.id() == action.id())
                .map(|s| s.trigger_description().to_owned())
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| "set in your desktop's shortcut settings".into());
            if binds.len() > 1 {
                format!("{} {how}", action.label())
            } else {
                how
            }
        })
        .collect();
    report(PttStatus::Global(format!(
        "System shortcut: {}",
        how.join(", ")
    )));

    let mut on = gs.receive_activated().await?;
    let mut off = gs.receive_deactivated().await?;
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    let set = |id: &str, down: bool| {
        for (action, _) in binds.iter().filter(|(a, _)| a.id() == id) {
            sink.set(*action, down);
        }
    };
    while !stop.load(Relaxed) {
        tokio::select! {
            Some(a) = on.next() => set(a.shortcut_id(), true),
            Some(d) = off.next() => set(d.shortcut_id(), false),
            _ = tick.tick() => {}
        }
    }
    let _ = session.close().await;
    Ok(())
}

#[cfg(feature = "x11")]
fn x11(
    sink: &Sink,
    binds: &Binds,
    stop: &AtomicBool,
    report: &Report,
) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xinput::{self, ConnectionExt as _, EventMask, XIEventMask};
    use x11rb::protocol::xproto::ConnectionExt as _;

    // Control, Shift and Alt, left and right.
    const MOD_SYMS: [[u32; 2]; 3] = [[0xffe3, 0xffe4], [0xffe1, 0xffe2], [0xffe9, 0xffea]];

    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    conn.xinput_xi_query_version(2, 2)?.reply()?;
    let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
    let map = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
    let per = usize::from(map.keysyms_per_keycode.max(1));
    // Every key code that types `sym`, in any shift level.
    let codes = |sym: u32| -> Vec<u32> {
        map.keysyms
            .iter()
            .enumerate()
            .filter(|(_, s)| **s == sym)
            .map(|(i, _)| u32::from(min) + (i / per) as u32)
            .collect()
    };
    // Per modifier: its key codes, and which of them are down.
    let mut mods: Vec<(Vec<u32>, Vec<u32>)> = MOD_SYMS
        .iter()
        .map(|syms| (syms.iter().flat_map(|s| codes(*s)).collect(), Vec::new()))
        .collect();
    // Per bind: (what it does, a mouse button, its key code or button number,
    // modifiers it needs as indexes into `mods`, held).
    let mut watch = Vec::new();
    let mut mask = XIEventMask::from(0u32);
    for &(action, combo) in binds {
        let (m, detail) = match combo.bind {
            PttBind::Key(k) => {
                let Some((sym, _, _)) = keymap(k) else {
                    report(PttStatus::FocusedOnly(format!(
                        "{} can't be a global key.",
                        k.name()
                    )));
                    return Ok(());
                };
                let Some(&code) = codes(sym).first() else {
                    report(PttStatus::FocusedOnly(format!(
                        "{} isn't on this keyboard.",
                        k.name()
                    )));
                    return Ok(());
                };
                (
                    XIEventMask::RAW_KEY_PRESS | XIEventMask::RAW_KEY_RELEASE,
                    code,
                )
            }
            PttBind::Mouse4 => (
                XIEventMask::RAW_BUTTON_PRESS | XIEventMask::RAW_BUTTON_RELEASE,
                8,
            ),
            PttBind::Mouse5 => (
                XIEventMask::RAW_BUTTON_PRESS | XIEventMask::RAW_BUTTON_RELEASE,
                9,
            ),
        };
        let c = combo.mods;
        let needs: Vec<usize> = [c.ctrl, c.shift, c.alt]
            .into_iter()
            .enumerate()
            .filter_map(|(i, on)| on.then_some(i))
            .collect();
        if !needs.is_empty() {
            mask |= XIEventMask::RAW_KEY_PRESS | XIEventMask::RAW_KEY_RELEASE;
        }
        watch.push((
            action,
            matches!(combo.bind, PttBind::Mouse4 | PttBind::Mouse5),
            detail,
            needs,
            false,
        ));
        mask |= m;
    }
    conn.xinput_xi_select_events(
        root,
        &[EventMask {
            deviceid: xinput::Device::ALL_MASTER.into(),
            mask: vec![mask],
        }],
    )?
    .check()?;
    conn.flush()?;
    let names: Vec<String> = binds.iter().map(|(_, b)| b.name()).collect();
    report(PttStatus::Global(format!(
        "{} works in every app (X11).",
        names.join(" and ")
    )));
    while !stop.load(Relaxed) {
        while let Some(ev) = conn.poll_for_event()? {
            let (button, d, down) = match ev {
                Event::XinputRawKeyPress(e) => (false, e.detail, true),
                Event::XinputRawKeyRelease(e) => (false, e.detail, false),
                Event::XinputRawButtonPress(e) => (true, e.detail, true),
                Event::XinputRawButtonRelease(e) => (true, e.detail, false),
                _ => continue,
            };
            if !button {
                for (codes, held) in &mut mods {
                    if codes.contains(&d) {
                        held.retain(|c| *c != d);
                        if down {
                            held.push(d);
                        }
                    }
                }
            }
            for (action, _, _, needs, was) in watch.iter_mut().filter(|w| (w.1, w.2) == (button, d))
            {
                // Modifiers only gate the press: letting go of one first still releases.
                let now = down && (*was || needs.iter().all(|i| !mods[*i].1.is_empty()));
                if now != *was {
                    sink.set(*action, now);
                    *was = now;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
