//! Wayland: GlobalShortcuts portal. X11: XInput2 raw key/button events.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use chad_media::Voice;
use futures_util::StreamExt;

use super::ptt::{PttBind, PttStatus, Report, keymap};

const ID: &str = "push-to-talk";

pub fn listen(voice: Voice, bind: PttBind, stop: Arc<AtomicBool>, report: Report) {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland");
    if wayland {
        tokio::spawn(async move {
            if let Err(e) = portal(&voice, bind, &stop, &report).await {
                tracing::info!("GlobalShortcuts portal: {e}");
                report(PttStatus::FocusedOnly(
                    "Your desktop doesn't offer global shortcuts to apps.".into(),
                ));
            }
            voice.set_ptt(false);
        });
    } else if cfg!(feature = "x11") && std::env::var_os("DISPLAY").is_some() {
        #[cfg(feature = "x11")]
        std::thread::spawn(move || {
            if let Err(e) = x11(&voice, bind, &stop, &report) {
                tracing::warn!("XInput2: {e}");
                report(PttStatus::FocusedOnly(
                    "Couldn't watch the keyboard.".into(),
                ));
            }
            voice.set_ptt(false);
        });
    } else {
        report(PttStatus::FocusedOnly("No display server found.".into()));
    }
}

async fn portal(
    voice: &Voice,
    bind: PttBind,
    stop: &AtomicBool,
    report: &Report,
) -> ashpd::Result<()> {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    let gs = GlobalShortcuts::new().await?;
    let session = gs.create_session(Default::default()).await?;
    // Only a hint: the desktop decides, and the user rebinds in its settings.
    let trigger = match bind {
        PttBind::Key(k) => keymap(k).map(|(_, name, _)| name),
        PttBind::Mouse4 | PttBind::Mouse5 => None,
    };
    let shortcut = NewShortcut::new(ID, "Push to talk").preferred_trigger(trigger.as_deref());
    let bound = gs
        .bind_shortcuts(&session, &[shortcut], None, Default::default())
        .await?
        .response()?;
    let how = bound
        .shortcuts()
        .iter()
        .find(|s| s.id() == ID)
        .map(|s| s.trigger_description().to_owned())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "set in your desktop's shortcut settings".into());
    report(PttStatus::Global(format!("System shortcut: {how}")));

    let mut on = gs.receive_activated().await?;
    let mut off = gs.receive_deactivated().await?;
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    while !stop.load(Relaxed) {
        tokio::select! {
            Some(a) = on.next() => if a.shortcut_id() == ID { voice.set_ptt(true) },
            Some(d) = off.next() => if d.shortcut_id() == ID { voice.set_ptt(false) },
            _ = tick.tick() => {}
        }
    }
    let _ = session.close().await;
    Ok(())
}

#[cfg(feature = "x11")]
fn x11(
    voice: &Voice,
    bind: PttBind,
    stop: &AtomicBool,
    report: &Report,
) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xinput::{self, ConnectionExt as _, EventMask, XIEventMask};
    use x11rb::protocol::xproto::ConnectionExt as _;

    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    conn.xinput_xi_query_version(2, 2)?.reply()?;
    let (mask, detail) = match bind {
        PttBind::Key(k) => {
            let Some((sym, _, _)) = keymap(k) else {
                report(PttStatus::FocusedOnly(format!(
                    "{} can't be a global key.",
                    k.name()
                )));
                return Ok(());
            };
            let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
            let map = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
            let per = usize::from(map.keysyms_per_keycode.max(1));
            let Some(i) = map.keysyms.iter().position(|&s| s == sym) else {
                report(PttStatus::FocusedOnly(format!(
                    "{} isn't on this keyboard.",
                    k.name()
                )));
                return Ok(());
            };
            let code = u32::from(min) + (i / per) as u32;
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
    conn.xinput_xi_select_events(
        root,
        &[EventMask {
            deviceid: xinput::Device::ALL_MASTER.into(),
            mask: vec![mask],
        }],
    )?
    .check()?;
    conn.flush()?;
    report(PttStatus::Global(format!(
        "{} works in every app (X11).",
        bind.name()
    )));
    while !stop.load(Relaxed) {
        while let Some(ev) = conn.poll_for_event()? {
            let (d, down) = match ev {
                Event::XinputRawKeyPress(e) => (e.detail, true),
                Event::XinputRawKeyRelease(e) => (e.detail, false),
                Event::XinputRawButtonPress(e) => (e.detail, true),
                Event::XinputRawButtonRelease(e) => (e.detail, false),
                _ => continue,
            };
            if d == detail {
                voice.set_ptt(down);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
