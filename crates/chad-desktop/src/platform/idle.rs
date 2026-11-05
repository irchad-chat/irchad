//! How long the user has been away from the whole desktop, not just our
//! window, for auto-away. Picked at runtime: ext-idle-notify-v1 on Wayland,
//! the XScreenSaver extension on X11, GetLastInputInfo on Windows. Without
//! any of those, input to our own window is all we see.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// When the user went idle, or `None` while they're active.
pub type Since = Arc<Mutex<Option<Instant>>>;

pub struct Idle {
    since: Since,
    /// Set when no desktop source works.
    local: bool,
    last_input: Instant,
}

impl Idle {
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let since = Since::default();
        let local = !watch(since.clone(), Box::new(wake));
        Self {
            since,
            local,
            last_input: Instant::now(),
        }
    }

    /// Call every frame with whether our window saw input.
    pub fn idle_for(&mut self, input: bool) -> Duration {
        if input {
            self.last_input = Instant::now();
        }
        let desktop = chad_core::lock(&self.since).map(|t| t.elapsed());
        match desktop {
            _ if input => Duration::ZERO,
            Some(d) => d,
            None if self.local => self.last_input.elapsed(),
            None => Duration::ZERO,
        }
    }
}

#[cfg(target_os = "linux")]
use super::idle_linux::watch;
#[cfg(windows)]
use super::idle_windows::watch;

#[cfg(not(any(target_os = "linux", windows)))]
fn watch(_since: Since, _wake: Box<dyn Fn() + Send + Sync>) -> bool {
    false
}
