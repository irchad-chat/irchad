//! Wayland: an ext-idle-notify-v1 notification after a minute of no input,
//! so "idle since" is a minute before it fires. X11: XScreenSaver's idle time.

use std::time::{Duration, Instant};

use super::idle::Since;

const STEP: Duration = Duration::from_secs(60);

/// False when there's no source on this desktop.
pub fn watch(since: Since, wake: Box<dyn Fn() + Send + Sync>) -> bool {
    #[cfg(feature = "wayland")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        match wayland::Watch::new(since.clone(), wake) {
            Ok(w) => {
                std::thread::spawn(move || {
                    if let Err(e) = w.run() {
                        tracing::warn!("idle notify: {e}");
                    }
                });
                return true;
            }
            // GNOME before 48, for one. XWayland only sees X input, so don't fall back to it.
            Err(e) => {
                tracing::info!("no ext-idle-notify-v1: {e}");
                return false;
            }
        }
    }
    #[cfg(feature = "x11")]
    if std::env::var_os("DISPLAY").is_some() {
        std::thread::spawn(move || {
            if let Err(e) = x11(&since, &*wake) {
                tracing::info!("XScreenSaver: {e}");
            }
        });
        return true;
    }
    let _ = (since, wake);
    false
}

#[cfg(feature = "wayland")]
mod wayland {
    use super::*;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{wl_registry, wl_seat};
    use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
    use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{
        self, ExtIdleNotificationV1,
    };
    use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;

    pub struct State {
        since: Since,
        wake: Box<dyn Fn() + Send + Sync>,
    }

    pub struct Watch {
        queue: EventQueue<State>,
        state: State,
        _notification: ExtIdleNotificationV1,
    }

    impl Watch {
        pub fn new(
            since: Since,
            wake: Box<dyn Fn() + Send + Sync>,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            let conn = Connection::connect_to_env()?;
            let (globals, queue) = registry_queue_init::<State>(&conn)?;
            let qh = queue.handle();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ())?;
            let notifier: ExtIdleNotifierV1 = globals.bind(&qh, 1..=1, ())?;
            let ms = u32::try_from(STEP.as_millis()).unwrap_or(u32::MAX);
            let notification = notifier.get_idle_notification(ms, &seat, &qh, ());
            Ok(Self {
                queue,
                state: State { since, wake },
                _notification: notification,
            })
        }

        pub fn run(mut self) -> Result<(), wayland_client::DispatchError> {
            loop {
                self.queue.blocking_dispatch(&mut self.state)?;
            }
        }
    }

    impl Dispatch<ExtIdleNotificationV1, ()> for State {
        fn event(
            state: &mut Self,
            _: &ExtIdleNotificationV1,
            event: ext_idle_notification_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            let now = match event {
                ext_idle_notification_v1::Event::Idled => Instant::now().checked_sub(STEP),
                _ => None,
            };
            *state.since.lock().unwrap_or_else(|e| e.into_inner()) = now;
            (state.wake)();
        }
    }

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &wl_registry::WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    wayland_client::delegate_noop!(State: ignore wl_seat::WlSeat);
    wayland_client::delegate_noop!(State: ExtIdleNotifierV1);
}

#[cfg(feature = "x11")]
fn x11(since: &Since, wake: &(dyn Fn() + Send + Sync)) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::screensaver::ConnectionExt as _;

    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    loop {
        let info = conn.screensaver_query_info(root)?.reply()?;
        let idle = Duration::from_millis(info.ms_since_user_input.into());
        let now = (idle >= STEP)
            .then(|| Instant::now().checked_sub(idle))
            .flatten();
        let mut s = since.lock().unwrap_or_else(|e| e.into_inner());
        if s.is_some() != now.is_some() {
            *s = now;
            drop(s);
            wake();
        }
        std::thread::sleep(Duration::from_secs(10));
    }
}
