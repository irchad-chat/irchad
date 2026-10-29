//! Audio I/O behind one trait: PipeWire on Linux, cpal (WASAPI) elsewhere.

use std::sync::Arc;

use crate::Devices;
use crate::engine::Shared;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(target_os = "linux"))]
mod other;

pub trait Backend: Send + Sync {
    /// (Re)opens capture and playback on these devices (`None`: the default),
    /// feeding [`Shared::capture`] and pulling [`Shared::render`].
    fn start(&self, input: Option<String>, output: Option<String>);
    /// Closes both; the mic is off.
    fn stop(&self);
    fn devices(&self) -> Devices;
}

pub fn backend(shared: Arc<Shared>) -> Arc<dyn Backend> {
    #[cfg(target_os = "linux")]
    return Arc::new(linux::PipeWire::spawn(shared));
    #[cfg(not(target_os = "linux"))]
    return Arc::new(other::Cpal::spawn(shared));
}
