//! `setTimeout`, cancelled on drop.

use std::time::Duration;

use wasm_bindgen::prelude::*;

pub struct Timer {
    id: i32,
    _cb: Closure<dyn FnMut()>,
}

impl Timer {
    /// Calls `f` once after `after`. `f` runs inside the timer's own callback,
    /// so it mustn't drop this `Timer`: defer anything that might.
    pub fn new(after: Duration, f: impl FnMut() + 'static) -> Option<Self> {
        let cb = Closure::<dyn FnMut()>::new(f);
        let ms = i32::try_from(after.as_millis()).unwrap_or(i32::MAX);
        let id = web_sys::window()?
            .set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), ms)
            .ok()?;
        Some(Self { id, _cb: cb })
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(w) = web_sys::window() {
            w.clear_timeout_with_handle(self.id);
        }
    }
}
