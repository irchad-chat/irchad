//! The JS-facing API, shaped like chad-ffi's: methods in, one `onChange`
//! callback out, then the page reads snapshots (`state`, `buffers`, `messages`).
//!
//! This file is only the browser's driver: the socket, the timers and the
//! bridge to JS. The IRC is chad-core's [`Login`] and [`Net`], the same code
//! the desktop and mobile apps run, and chad-irc's [`Keepalive`].

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use chad_core::login::{self, Account, Login, Step};
use chad_core::view::View;
use chad_core::{
    BufferKind, ConnState, MessageKind, Net, NetCommand, NetworkInfo, SendStatus, Store, format,
};
use chad_irc::Message;
use chad_irc::keepalive::{Backoff, Keepalive};
use chad_proto::discovery::{Discovery, WELL_KNOWN_PATH};
use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::irc::{Incoming, Socket};
use crate::timer::Timer;

#[wasm_bindgen(typescript_custom_section)]
const TS: &str = r#"
export type Rgb = [number, number, number];
export type Span = {
  text: string; bold: boolean; italic: boolean; underline: boolean; strike: boolean;
  mono: boolean; fg: Rgb | null; bg: Rgb | null; link: boolean;
};
export type NetState = {
  state: "connecting" | "verifying" | "connected" | "disconnected" | "failed";
  error: string | null; nick: string; name: string; guest: boolean;
};
export type Buffer = {
  target: string; kind: "channel" | "dm" | "server"; topic: string; unread: number; mentions: number; typing: string[];
};
export type Msg = {
  key: string; time: number; nick: string; action: boolean; notice: boolean; own: boolean;
  mention: boolean; pending: boolean; failed: string | null; redacted: boolean; edited: boolean; spans: Span[];
};
"#;

/// The real document is ~300 bytes (as `chad-core`'s `discovery`).
const MAX_DISCOVERY: usize = 64 << 10;
/// localStorage key for this browser's device, for ergo's per-device login (D11).
const DEVICE_KEY: &str = "irchad.device";

#[derive(Serialize)]
struct State<'a> {
    state: &'static str,
    error: Option<&'a str>,
    nick: &'a str,
    name: &'a str,
    guest: bool,
}

#[derive(Serialize)]
struct Buffer<'a> {
    target: &'a str,
    kind: &'static str,
    topic: &'a str,
    unread: u32,
    mentions: u32,
    typing: Vec<&'a str>,
}

#[derive(Serialize)]
struct Msg<'a> {
    key: String,
    time: i64,
    nick: &'a str,
    action: bool,
    notice: bool,
    own: bool,
    mention: bool,
    pending: bool,
    failed: Option<&'a str>,
    redacted: bool,
    edited: bool,
    spans: Vec<format::Span>,
}

type Shared = Rc<RefCell<Inner>>;
type W = Weak<RefCell<Inner>>;

/// One connection attempt.
struct Conn {
    sock: Socket,
    /// The socket's open; until then lines wait in `out`.
    open: bool,
    /// Registering, until [`Step::Done`]; then lines go to [`login::live`].
    login: Option<Login>,
    alive: Keepalive,
    /// Which arming of `timer` is current, so a stale firing does nothing.
    tick: u32,
    timer: Option<Timer>,
}

enum End {
    Lost(String),
    Fatal(String),
    /// Signed up: log in again as the new account.
    Again,
}

struct Inner {
    net: Net,
    view: View,
    acct: Account,
    /// The last discovery document that worked, if the next fetch doesn't.
    discovery: Option<Discovery>,
    conn: Option<Conn>,
    /// Bumped whenever `conn` goes, so callbacks from an old socket or timer
    /// that were already queued do nothing.
    epoch: u32,
    backoff: Backoff,
    retry: Option<Timer>,
    on_change: js_sys::Function,
}

#[wasm_bindgen]
pub struct Client {
    inner: Shared,
}

#[wasm_bindgen]
impl Client {
    /// `network` is the origin, `https://chat.example.com`: the page's own in
    /// production. `onChange` is called after anything changed; read the
    /// snapshots then.
    #[wasm_bindgen(constructor)]
    pub fn new(
        network: &str,
        #[wasm_bindgen(unchecked_param_type = "() => void")] on_change: js_sys::Function,
    ) -> Result<Client, JsError> {
        let url = url::Url::parse(network).map_err(|_| JsError::new("not a URL"))?;
        if url.scheme() != "https" {
            return Err(JsError::new("the network must be an https:// origin"));
        }
        let network = url.origin().ascii_serialization();
        let info = NetworkInfo {
            name: url.host_str().unwrap_or_default().to_owned(),
            network,
            icon: None,
            nick: String::new(),
            state: ConnState::Disconnected {
                error: String::new(),
            },
            guest: false,
            oper: false,
            uploads: None,
            gifs: None,
            games: false,
            emoji: false,
            receipts: false,
        };
        // shortcut: memory only, so every page load replays history over
        // chathistory; use sqlite-wasm-rs's OPFS VFS to keep it.
        let store = Store::memory().map_err(|e| JsError::new(&e.to_string()))?;
        let net = Net::new(store, info);
        Ok(Client {
            inner: Rc::new(RefCell::new(Inner {
                net,
                view: View::default(),
                acct: Account::default(),
                discovery: None,
                conn: None,
                epoch: 0,
                backoff: Backoff::new(),
                retry: None,
                on_change,
            })),
        })
    }

    /// Log in, or join as a guest with an empty `password`. Replaces any
    /// current connection.
    pub fn connect(&self, nick: &str, password: &str) -> Result<(), JsError> {
        let nick = nick.trim();
        if !login::valid_nick(nick) {
            return Err(JsError::new(
                "Names can't have spaces or start with # or :.",
            ));
        }
        run(&self.inner, |i, w| {
            let guest = password.is_empty();
            i.acct = Account {
                name: nick.to_owned(),
                password: password.to_owned(),
                guest,
                device: device_id(),
                ..Default::default()
            };
            i.net.guest = guest;
            i.net.info.guest = guest;
            nick.clone_into(&mut i.net.info.nick);
            i.backoff.reset();
            i.open(w);
        });
        Ok(())
    }

    #[wasm_bindgen(unchecked_return_type = "NetState")]
    pub fn state(&self) -> JsValue {
        let i = self.inner.borrow();
        let info = &i.net.info;
        let (state, error) = match &info.state {
            ConnState::Connecting => ("connecting", None),
            ConnState::NeedsVerification { message } => ("verifying", Some(message.as_str())),
            ConnState::Connected => ("connected", None),
            ConnState::Disconnected { error } => ("disconnected", Some(error.as_str())),
            ConnState::Failed { error } => ("failed", Some(error.as_str())),
        };
        to_js(&State {
            state,
            error: error.filter(|e| !e.is_empty()),
            nick: &info.nick,
            name: &info.name,
            guest: info.guest,
        })
    }

    #[wasm_bindgen(unchecked_return_type = "Buffer[]")]
    pub fn buffers(&self) -> JsValue {
        let i = self.inner.borrow();
        let Some(n) = i.view.net(&i.net.info.network).map(|n| &i.view.nets[n]) else {
            return to_js(&[] as &[Buffer]);
        };
        let list: Vec<_> = n
            .buffers
            .iter()
            .map(|b| Buffer {
                target: &b.info.target,
                kind: match b.info.kind {
                    BufferKind::Channel => "channel",
                    BufferKind::Dm => "dm",
                    BufferKind::Server => "server",
                },
                topic: &b.info.topic,
                unread: b.info.unread,
                mentions: b.info.mentions,
                typing: b.typing().map(|t| n.shown(t)).collect(),
            })
            .collect();
        to_js(&list)
    }

    /// Oldest first.
    #[wasm_bindgen(unchecked_return_type = "Msg[]")]
    pub fn messages(&self, target: &str) -> JsValue {
        let i = self.inner.borrow();
        let Some(b) = i.view.buf(&i.net.info.network, target) else {
            return to_js(&[] as &[Msg]);
        };
        // shortcut: the whole buffer on every change; send only what changed
        // (`Applied::fresh`) if long scrollback gets slow.
        let list: Vec<_> = b
            .msgs
            .iter()
            .map(|m| Msg {
                key: m
                    .msgid
                    .clone()
                    .unwrap_or_else(|| format!("local:{}", m.local_id.unwrap_or_default())),
                time: m.time,
                nick: &m.nick,
                action: m.kind == MessageKind::Action,
                notice: m.kind == MessageKind::Notice,
                own: m.own,
                mention: m.mention,
                pending: m.status == SendStatus::Pending,
                failed: match &m.status {
                    SendStatus::Failed(e) => Some(e.as_str()),
                    _ => None,
                },
                redacted: m.redacted,
                edited: m.edited,
                spans: format::parse(&m.text),
            })
            .collect();
        to_js(&list)
    }

    /// A message to a channel or DM.
    pub fn send(&self, target: &str, text: &str) {
        let (target, text) = (target.to_owned(), text.to_owned());
        run(&self.inner, |i, _| {
            i.command(NetCommand::Send {
                target,
                text,
                reply_to: None,
                mentions: Vec::new(),
            });
        });
    }

    /// Join a channel, or open a DM with a nick.
    pub fn open(&self, name: &str) -> Result<(), JsError> {
        let name = name.trim().to_owned();
        run(&self.inner, |i, _| {
            if i.net.isupport.is_channel(&name) {
                i.command(NetCommand::JoinChannel { channel: name });
                Ok(())
            } else if login::valid_nick(&name) {
                i.command(NetCommand::OpenDm { nick: name });
                Ok(())
            } else {
                Err(JsError::new("That's not a channel or a nick."))
            }
        })
    }

    /// Everything in `target` has been seen.
    #[wasm_bindgen(js_name = markRead)]
    pub fn mark_read(&self, target: &str) {
        let target = target.to_owned();
        run(&self.inner, |i, _| {
            i.command(NetCommand::MarkRead { target })
        });
    }
}

impl Inner {
    /// Fetch discovery, then dial. Ends any current connection.
    fn open(&mut self, w: &W) {
        self.end_conn();
        self.net.set_state(ConnState::Connecting);
        let epoch = self.epoch;
        let origin = self.net.info.network.clone();
        let w = w.clone();
        spawn_local(async move {
            let fetched = fetch_discovery(&origin).await;
            if let Some(rc) = w.upgrade() {
                run(&rc, |i, w| {
                    if i.epoch == epoch {
                        i.dial(fetched, w);
                    }
                });
            }
        });
    }

    fn dial(&mut self, fetched: Result<Discovery, String>, w: &W) {
        let d = match fetched {
            Ok(d) => {
                self.discovery = Some(d.clone());
                d
            }
            Err(e) => match self.discovery.clone() {
                Some(d) => d,
                None => return self.lost(e, w),
            },
        };
        login::discovered(&mut self.net, &d, self.acct.guest);
        let login = match Login::start(&mut self.net, &d, self.acct.clone()) {
            Ok(l) => l,
            Err(e) => return self.fail(e),
        };
        let epoch = self.epoch;
        let ww = w.clone();
        let wake = move || later(&ww, move |i, w| i.on_socket(epoch, w));
        let sock = match Socket::connect(d.irc.websocket.as_str(), wake) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("connect: {e:?}");
                return self.lost(format!("Couldn't connect to {}.", d.name), w);
            }
        };
        self.conn = Some(Conn {
            sock,
            open: false,
            login: Some(login),
            alive: Keepalive::new(),
            tick: 0,
            timer: None,
        });
        self.arm(w);
    }

    fn on_socket(&mut self, epoch: u32, w: &W) {
        if epoch != self.epoch {
            return;
        }
        let Some(c) = &mut self.conn else { return };
        let queued: Vec<_> = c.sock.inbox.borrow_mut().drain(..).collect();
        for ev in queued {
            let end = match ev {
                Incoming::Open => {
                    if let Some(c) = &mut self.conn {
                        c.open = true;
                    }
                    None
                }
                Incoming::Line(m) => self.line(m),
                Incoming::Closed(reason) if reason.is_empty() => {
                    Some(End::Lost("Lost the connection to the server.".into()))
                }
                Incoming::Closed(reason) => Some(End::Lost(reason)),
            };
            match end {
                None => {}
                Some(End::Lost(e)) => return self.lost(e, w),
                Some(End::Fatal(e)) => return self.fail(e),
                Some(End::Again) => return self.open(w),
            }
        }
        // Anything at all restarts the wait for the next line.
        self.arm(w);
    }

    fn line(&mut self, m: Message) -> Option<End> {
        let Inner {
            conn,
            net,
            acct,
            backoff,
            ..
        } = self;
        let c = conn.as_mut()?;
        if let Some(pong) = c.alive.line(&m) {
            c.sock.send(&pong);
            return None;
        }
        let Some(login) = &mut c.login else {
            return login::live(net, m).err().map(End::Lost);
        };
        match login.handle(net, m) {
            Step::Continue | Step::LoggedIn => None,
            Step::NeedsVerification(_) => {
                acct.verifying = true;
                // The server's fine; only its registration timeout drops us.
                backoff.reset();
                None
            }
            Step::Registered => {
                for m in login.out.drain(..) {
                    c.sock.send(&m);
                }
                acct.sign_up = None;
                acct.verifying = false;
                Some(End::Again)
            }
            Step::Done(nick) => {
                c.login = None;
                net.attach(&nick);
                backoff.reset();
                None
            }
            Step::Lost(e) => Some(End::Lost(e)),
            Step::Fatal(e) => Some(End::Fatal(e)),
        }
    }

    /// (Re)starts the keepalive wait.
    fn arm(&mut self, w: &W) {
        let epoch = self.epoch;
        let Some(c) = &mut self.conn else { return };
        c.tick = c.tick.wrapping_add(1);
        let tick = c.tick;
        let w = w.clone();
        c.timer = Timer::new(c.alive.wait(), move || {
            later(&w, move |i, w| i.silence(epoch, tick, w));
        });
    }

    fn silence(&mut self, epoch: u32, tick: u32, w: &W) {
        if epoch != self.epoch {
            return;
        }
        let Some(c) = &mut self.conn else { return };
        if c.tick != tick {
            return;
        }
        if !c.open {
            return self.lost("Couldn't connect to the server.".into(), w);
        }
        match c.alive.silence() {
            Ok(ping) => {
                c.sock.send(&ping);
                self.arm(w);
            }
            Err(_) => self.lost("The server stopped responding.".into(), w),
        }
    }

    /// Dropped: say so and retry after a backoff.
    fn lost(&mut self, error: String, w: &W) {
        tracing::info!("{}: {error}", self.net.info.network);
        self.end_conn();
        self.net.set_state(ConnState::Disconnected { error });
        let epoch = self.epoch;
        let w = w.clone();
        self.retry = Timer::new(self.backoff.next_delay(), move || {
            later(&w, move |i, w| {
                if i.epoch == epoch {
                    i.open(w);
                }
            });
        });
    }

    /// Retrying won't help until the user does something.
    fn fail(&mut self, error: String) {
        self.end_conn();
        self.net.set_state(ConnState::Failed { error });
    }

    fn end_conn(&mut self) {
        if self.conn.take().is_some() {
            self.net.detach();
        }
        self.epoch = self.epoch.wrapping_add(1);
        self.retry = None;
    }

    fn command(&mut self, cmd: NetCommand) {
        let live = self.conn.as_ref().is_some_and(|c| c.login.is_none());
        self.net.command(cmd);
        // Offline, as conn.rs: the command's local effects stay, its lines go.
        if !live {
            self.net.out.clear();
        }
    }

    /// Sends what's queued and applies `net`'s events to the view. Returns
    /// the page's callback if anything changed.
    fn pump(&mut self) -> Option<js_sys::Function> {
        if let Some(c) = &mut self.conn
            && c.open
        {
            match &mut c.login {
                Some(l) => l.out.drain(..).for_each(|m| c.sock.send(&m)),
                None => self.net.out.drain(..).for_each(|m| c.sock.send(&m)),
            }
        }
        let mut changed = false;
        for ev in self.net.drain_events() {
            self.view.apply(&ev);
            changed = true;
        }
        changed.then(|| self.on_change.clone())
    }
}

/// Runs `f`, sends what it queued, then tells the page if anything changed
/// (after the borrow ends, so the page can read snapshots).
fn run<T>(rc: &Shared, f: impl FnOnce(&mut Inner, &W) -> T) -> T {
    let w = Rc::downgrade(rc);
    let (r, notify) = {
        let mut i = rc.borrow_mut();
        let r = f(&mut i, &w);
        (r, i.pump())
    };
    if let Some(cb) = notify
        && let Err(e) = cb.call0(&JsValue::NULL)
    {
        tracing::warn!("onChange threw: {e:?}");
    }
    r
}

/// [`run`] on a fresh task, never inside the socket or timer callback that
/// asked for it: `f` may drop that callback's closure, which mustn't happen
/// while it's running.
fn later(w: &W, f: impl FnOnce(&mut Inner, &W) + 'static) {
    let w = w.clone();
    spawn_local(async move {
        if let Some(rc) = w.upgrade() {
            run(&rc, f);
        }
    });
}

fn to_js<T: Serialize + ?Sized>(v: &T) -> JsValue {
    v.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .unwrap_or_else(|e| {
            tracing::warn!("can't serialize: {e}");
            JsValue::NULL
        })
}

/// `<origin>/.well-known/irchad.json`, bounded per D14.
async fn fetch_discovery(origin: &str) -> Result<Discovery, String> {
    let host = origin.split_once("://").map_or(origin, |(_, h)| h);
    let unreachable = |e: reqwest::Error| {
        tracing::warn!("discovery {origin}: {e:?}");
        format!("Couldn't reach {host}. Check the address and your connection.")
    };
    let not_irchad = || format!("{host} isn't an IrChad server.");
    let resp = reqwest::get(format!("{origin}{WELL_KNOWN_PATH}"))
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(unreachable)?;
    // shortcut: by now the browser holds the whole body; this only refuses to
    // parse a big one. Stream it (reqwest's `stream` feature) to stop sooner.
    if resp
        .content_length()
        .is_some_and(|n| n > MAX_DISCOVERY as u64)
    {
        return Err(not_irchad());
    }
    let body = resp.bytes().await.map_err(unreachable)?;
    if body.len() > MAX_DISCOVERY {
        return Err(not_irchad());
    }
    chad_core::parse_discovery(&body).ok_or_else(not_irchad)
}

/// This browser's device ID, made on first use. Without storage (a private
/// window, say) it's new every page load, which only costs ergo a device.
fn device_id() -> String {
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    if let Some(id) = storage
        .as_ref()
        .and_then(|s| s.get_item(DEVICE_KEY).ok().flatten())
        .filter(|id| chad_e2ee::valid_id(id))
    {
        return id;
    }
    let mut bytes = [0u8; 8];
    let _ = getrandom3::fill(&mut bytes);
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(s) = storage {
        let _ = s.set_item(DEVICE_KEY, &id);
    }
    id
}
