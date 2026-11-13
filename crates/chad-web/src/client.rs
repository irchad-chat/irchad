//! The JS-facing API, shaped like chad-ffi's: methods in, one `onChange`
//! callback out, then the page reads snapshots (`snapshot.rs`). chadd calls
//! return Promises (`api.rs`).
//!
//! This file is only the browser's driver: the socket, the timers and the
//! bridge to JS. The IRC is chad-core's [`Login`] and [`Net`], the same code
//! the desktop and mobile apps run, and chad-irc's [`Keepalive`].

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use chad_core::chadd;
use chad_core::login::{self, Account, Login, Step};
use chad_core::view::View;
use chad_core::{ConnState, Event, Net, NetCommand, NetworkInfo, Store, Typing, format};
use chad_irc::Message;
use chad_irc::keepalive::{Backoff, Keepalive};
use chad_proto::discovery::Discovery;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::irc::{Incoming, Socket};
use crate::timer::Timer;

/// localStorage key for this browser's device, for ergo's per-device login (D11).
const DEVICE_KEY: &str = "irchad.device";
/// Notices the page hasn't taken yet; more are dropped (D14: a server can
/// make `Net` say a lot).
const MAX_NOTICES: usize = 20;

pub(crate) type Shared = Rc<RefCell<Inner>>;
pub(crate) type W = Weak<RefCell<Inner>>;

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
    /// Registered; carry on.
    Ready,
}

pub(crate) struct Inner {
    pub(crate) net: Net,
    pub(crate) view: View,
    acct: Account,
    /// A guest signing up: who to go back to if it doesn't work.
    upgrading: Option<Account>,
    /// The last discovery document that worked, if the next fetch doesn't.
    pub(crate) discovery: Option<Discovery>,
    conn: Option<Conn>,
    /// chadd, once logged in; shared by every call until the next connection.
    pub(crate) chadd: Option<Rc<chadd::Client>>,
    /// The chadd session, kept across reconnects of the same account.
    session: Arc<tokio::sync::Mutex<Option<chadd::Session>>>,
    /// Bumped whenever `conn` goes, so callbacks from an old socket or timer
    /// that were already queued do nothing.
    epoch: u32,
    backoff: Backoff,
    retry: Option<Timer>,
    /// Errors for the page to show (`takeNotices`).
    notices: Vec<String>,
    /// The view changed without an event from `net` (chadd's emoji, say).
    pub(crate) dirty: bool,
    on_change: js_sys::Function,
}

#[wasm_bindgen]
pub struct Client {
    pub(crate) inner: Shared,
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
            lag_ms: None,
        };
        // No cache: history comes from chathistory on every page load.
        let store = Store::memory().map_err(|e| JsError::new(&e.to_string()))?;
        let net = Net::new(store, info);
        Ok(Client {
            inner: Rc::new(RefCell::new(Inner {
                net,
                view: View::default(),
                acct: Account::default(),
                upgrading: None,
                discovery: None,
                conn: None,
                chadd: None,
                session: Arc::default(),
                epoch: 0,
                backoff: Backoff::new(),
                retry: None,
                notices: Vec::new(),
                dirty: false,
                on_change,
            })),
        })
    }

    /// Log in, or join as a guest with an empty `password`. Replaces any
    /// current connection.
    pub fn connect(&self, nick: &str, password: &str) -> Result<(), JsError> {
        let nick = checked_nick(nick)?;
        run(&self.inner, |i, w| {
            let guest = password.is_empty();
            i.start(
                Account {
                    name: nick,
                    password: password.to_owned(),
                    guest,
                    device: device_id(),
                    ..Default::default()
                },
                w,
            );
        });
        Ok(())
    }

    /// Make an account. From the login form it's a new connection; as a
    /// connected guest, the guest carries on as the account, keeping their
    /// channels. The emailed code, if the network wants one, goes to `verify`.
    #[wasm_bindgen(js_name = signUp)]
    pub fn sign_up(
        &self,
        account: &str,
        password: &str,
        email: Option<String>,
    ) -> Result<(), JsError> {
        let account = checked_nick(account)?;
        if password.is_empty() {
            return Err(JsError::new("Pick a password."));
        }
        let email = email.map(|e| e.trim().to_owned()).filter(|e| !e.is_empty());
        run(&self.inner, |i, w| {
            let acct = Account {
                name: account,
                password: password.to_owned(),
                guest: false,
                device: device_id(),
                sign_up: Some(email),
                verifying: false,
            };
            let live_guest = i.acct.guest && i.live();
            if live_guest {
                if let Some(c) = &i.conn {
                    // Gone before the new connection asks for the nick, if it's the same.
                    c.sock.send(&Message::new("QUIT", ["signing up"]));
                }
                i.upgrading = Some(i.acct.clone());
            }
            i.start(acct, w);
        });
        Ok(())
    }

    /// The code from the sign-up email.
    pub fn verify(&self, code: &str) {
        let code = code.trim().to_owned();
        run(&self.inner, |i, _| {
            if let Some(Conn { login: Some(l), .. }) = &mut i.conn {
                l.verify(&code);
            }
        });
    }

    /// Errors since the last call, for the page to show once.
    #[wasm_bindgen(js_name = takeNotices)]
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut self.inner.borrow_mut().notices)
    }

    /// A message to a channel or DM: markdown, as the other apps. `mentions`
    /// are the nicks picked from `nicks` (D66); the core keeps the ones still
    /// in the text.
    pub fn send(
        &self,
        target: String,
        text: &str,
        #[wasm_bindgen(js_name = replyTo)] reply_to: Option<String>,
        mentions: Vec<String>,
    ) {
        let text = format::markdown(text);
        self.cmd(NetCommand::Send {
            target,
            text,
            reply_to,
            mentions,
        });
    }

    /// Replace the text of one of our own messages (D51).
    pub fn edit(&self, target: String, msgid: String, text: &str, mentions: Vec<String>) {
        let text = format::markdown(text);
        self.cmd(NetCommand::Edit {
            target,
            msgid,
            text,
            mentions,
        });
    }

    pub fn react(&self, target: String, msgid: String, emoji: String) {
        self.cmd(NetCommand::React {
            target,
            msgid,
            emoji,
        });
    }

    pub fn unreact(&self, target: String, msgid: String, emoji: String) {
        self.cmd(NetCommand::Unreact {
            target,
            msgid,
            emoji,
        });
    }

    /// Delete one of our own messages.
    pub fn redact(&self, target: String, msgid: String) {
        self.cmd(NetCommand::Redact { target, msgid });
    }

    /// Hide everything from someone, on this device only.
    pub fn block(&self, nick: String, account: Option<String>) {
        self.cmd(NetCommand::Block { nick, account });
    }

    /// The composer holds text (`true`) or was emptied or sent. The core
    /// throttles what goes out.
    pub fn typing(&self, target: String, active: bool) {
        let state = if active { Typing::Active } else { Typing::Done };
        self.cmd(NetCommand::Typing { target, state });
    }

    /// One more page of scrollback.
    #[wasm_bindgen(js_name = loadOlder)]
    pub fn load_older(&self, target: String) {
        self.cmd(NetCommand::LoadOlder { target });
    }

    /// The history around `msgid` (a reply's parent that isn't loaded).
    #[wasm_bindgen(js_name = loadAround)]
    pub fn load_around(&self, target: String, msgid: String) {
        self.cmd(NetCommand::LoadAround { target, msgid });
    }

    /// Ask for a channel's members: when it's shown, and after a reconnect.
    #[wasm_bindgen(js_name = fetchMembers)]
    pub fn fetch_members(&self, channel: String) {
        self.cmd(NetCommand::Members { channel });
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

    /// Leave a channel (on every device: always-on).
    pub fn part(&self, channel: String) {
        self.cmd(NetCommand::Part { channel });
    }

    /// Hide a DM; a new message reopens it.
    #[wasm_bindgen(js_name = closeDm)]
    pub fn close_dm(&self, nick: String) {
        self.cmd(NetCommand::CloseDm { nick });
    }

    /// `Some(reason)` to go away, `None` to come back.
    #[wasm_bindgen(js_name = setAway)]
    pub fn set_away(&self, message: Option<String>) {
        self.cmd(NetCommand::SetAway { message });
    }

    /// Everything in `target` has been seen.
    #[wasm_bindgen(js_name = markRead)]
    pub fn mark_read(&self, target: String) {
        self.cmd(NetCommand::MarkRead { target });
    }

    fn cmd(&self, cmd: NetCommand) {
        run(&self.inner, |i, _| i.command(cmd));
    }
}

impl Inner {
    /// Registered and talking.
    pub(crate) fn live(&self) -> bool {
        self.conn.as_ref().is_some_and(|c| c.login.is_none())
    }

    /// Connect as `acct`, from scratch.
    fn start(&mut self, acct: Account, w: &W) {
        if acct.name != self.acct.name || acct.guest != self.acct.guest {
            self.session = Arc::default();
        }
        if self.upgrading.is_none() {
            self.net.guest = acct.guest;
            self.net.info.guest = acct.guest;
            acct.name.clone_into(&mut self.net.info.nick);
        }
        self.acct = acct;
        self.backoff.reset();
        self.open(w);
    }

    /// Fetch discovery, then dial. Ends any current connection.
    fn open(&mut self, w: &W) {
        self.end_conn();
        if !self.acct.verifying {
            self.net.set_state(ConnState::Connecting);
        }
        let epoch = self.epoch;
        let origin = self.net.info.network.clone();
        let w = w.clone();
        spawn_local(async move {
            let fetched = chad_core::fetch_discovery(&origin).await;
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
            Err(e) => return self.fail(e, w),
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
                Some(End::Fatal(e)) => return self.fail(e, w),
                Some(End::Again) => return self.open(w),
                Some(End::Ready) => self.logged_in(w),
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
                Some(End::Ready)
            }
            Step::Lost(e) => Some(End::Lost(e)),
            Step::Fatal(e) => Some(End::Fatal(e)),
        }
    }

    /// Registered: chadd's client, and the background fetches every connect makes.
    fn logged_in(&mut self, w: &W) {
        if self.upgrading.take().is_some() {
            // The new account isn't in the guest's channels yet.
            self.net.store.set_meta("rejoin", "1");
            self.net.guest = false;
            self.net.info.guest = false;
        }
        let Some(d) = &self.discovery else { return };
        let client = Rc::new(chadd::Client::new(
            d.chadd.api.clone(),
            self.acct.name.clone(),
            (!self.acct.guest).then(|| self.acct.password.clone()),
            self.session.clone(),
        ));
        self.chadd = Some(client.clone());
        crate::api::on_connect(self, &client, w);
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
        if self.back_to_guest(&error, w) {
            return;
        }
        self.end_conn();
        // Keep the code prompt up: the code stays good across ergo's
        // registration timeout.
        if !self.acct.verifying {
            self.net.set_state(ConnState::Disconnected { error });
        }
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
    fn fail(&mut self, error: String, w: &W) {
        if self.back_to_guest(&error, w) {
            return;
        }
        self.end_conn();
        self.net.set_state(ConnState::Failed { error });
    }

    /// A guest's sign-up that didn't work: carry on as the guest.
    fn back_to_guest(&mut self, error: &str, w: &W) -> bool {
        if self.acct.verifying {
            return false;
        }
        let Some(guest) = self.upgrading.take() else {
            return false;
        };
        self.push_notice(format!("Couldn't sign up: {error}"));
        self.start(guest, w);
        true
    }

    fn end_conn(&mut self) {
        if self.conn.take().is_some() {
            self.net.detach();
        }
        self.chadd = None;
        self.epoch = self.epoch.wrapping_add(1);
        self.retry = None;
    }

    pub(crate) fn command(&mut self, cmd: NetCommand) {
        let live = self.live();
        self.net.command(cmd);
        // Offline, as conn.rs: the command's local effects stay, its lines go.
        if !live {
            self.net.out.clear();
        }
    }

    pub(crate) fn push_notice(&mut self, text: String) {
        if self.notices.len() < MAX_NOTICES {
            self.notices.push(text);
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
        let mut changed = std::mem::take(&mut self.dirty) || !self.notices.is_empty();
        for ev in self.net.drain_events() {
            self.view.apply(&ev);
            if let Event::Error { error, .. } | Event::SignUpFailed { error, .. } = ev
                && self.notices.len() < MAX_NOTICES
            {
                self.notices.push(error);
            }
            changed = true;
        }
        changed.then(|| self.on_change.clone())
    }
}

/// Runs `f`, sends what it queued, then tells the page if anything changed
/// (after the borrow ends, so the page can read snapshots).
pub(crate) fn run<T>(rc: &Shared, f: impl FnOnce(&mut Inner, &W) -> T) -> T {
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
pub(crate) fn later(w: &W, f: impl FnOnce(&mut Inner, &W) + 'static) {
    let w = w.clone();
    spawn_local(async move {
        if let Some(rc) = w.upgrade() {
            run(&rc, f);
        }
    });
}

fn checked_nick(nick: &str) -> Result<String, JsError> {
    let nick = nick.trim();
    if login::valid_nick(nick) {
        Ok(nick.to_owned())
    } else {
        Err(JsError::new(
            "Names can't have spaces or start with # or :.",
        ))
    }
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
