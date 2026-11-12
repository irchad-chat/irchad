//! The browser's WebSocket as a chad-irc transport. Stand-in for
//! `chad_irc::ws::Connection`, which is tokio/rustls and can't run here.
//!
//! Callback-driven: frames land in `inbox` and the UI drains it each frame.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use chad_irc::Message;
use wasm_bindgen::prelude::*;
use web_sys::{CloseEvent, MessageEvent, WebSocket};

/// Same cap as the native transport (D14: don't trust the server).
const MAX_FRAME: usize = 16 << 10;

pub enum Incoming {
    Open,
    Line(Message),
    Closed(String),
}

pub struct Socket {
    ws: WebSocket,
    pub inbox: Rc<RefCell<VecDeque<Incoming>>>,
    // Kept alive for as long as the socket; dropping them unhooks the callbacks.
    _cbs: [Closure<dyn FnMut(JsValue)>; 3],
}

impl Socket {
    /// `wss://host/irc` with subprotocol `text.ircv3.net`. `wake` is called on
    /// every event, e.g. `ctx.request_repaint()`.
    pub fn connect(url: &str, wake: impl Fn() + Clone + 'static) -> Result<Self, JsValue> {
        let ws = WebSocket::new_with_str(url, "text.ircv3.net")?;
        let inbox = Rc::new(RefCell::new(VecDeque::new()));

        let (q, w) = (inbox.clone(), wake.clone());
        let on_open = Closure::<dyn FnMut(JsValue)>::new(move |_| {
            q.borrow_mut().push_back(Incoming::Open);
            w();
        });

        let (q, w) = (inbox.clone(), wake.clone());
        let on_message = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let Some(text) = e.unchecked_into::<MessageEvent>().data().as_string() else {
                return; // binary frame: not IRC
            };
            if text.len() > MAX_FRAME {
                tracing::warn!("dropping oversized frame ({} bytes)", text.len());
                return;
            }
            match Message::parse(&text) {
                Ok(m) => q.borrow_mut().push_back(Incoming::Line(m)),
                Err(e) => tracing::warn!("skipping bad line from server: {e}"),
            }
            w();
        });

        let (q, w) = (inbox.clone(), wake);
        let on_close = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let reason = e.unchecked_into::<CloseEvent>().reason();
            q.borrow_mut().push_back(Incoming::Closed(reason));
            w();
        });

        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        Ok(Self {
            ws,
            inbox,
            _cbs: [on_open, on_message, on_close],
        })
    }

    pub fn send(&self, msg: &Message) {
        match msg.to_line() {
            Ok(line) => {
                if let Err(e) = self.ws.send_with_str(&line) {
                    tracing::warn!("send failed: {e:?}");
                }
            }
            Err(e) => tracing::warn!("not sending unencodable message: {e}"),
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.ws.set_onopen(None);
        self.ws.set_onmessage(None);
        self.ws.set_onclose(None);
        let _ = self.ws.close();
    }
}
