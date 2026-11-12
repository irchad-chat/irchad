//! IrChad's Rust core for the browser, exposed to the Vue app in `web/` through
//! wasm-bindgen. chad-core speaks the IRC; this crate gives it the browser's
//! WebSocket and timers.
#![cfg(target_arch = "wasm32")]

mod client;
mod irc;
mod timer;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    tracing_wasm::set_as_global_default();
}
