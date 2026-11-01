//! UniFFI bindings over chad-core for Android (Phase 6).
//!
//! Kotlin makes one [`Client`], which runs the core on its own tokio runtime
//! and projects events into a [`View`] (D38). After each event the
//! [`Listener`] hears `changed()`, and the UI reads fresh snapshots. Media
//! isn't here: Android plays calls with the LiveKit Android SDK behind
//! [`Voice`], and the core only decides when to join and leave.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chad_core::secrets::{Fut, SecretStore};
use chad_core::view::View;
use chad_core::{
    Auth, Command, ConnState, Core, Crypt, Encryption, Event, MediaEvent, MessageKind, NetCommand,
    SendStatus,
};
use tokio::sync::mpsc;

uniffi::setup_scaffolding!();

/// Told about changes; called from a core thread, never the UI thread.
#[uniffi::export(with_foreign)]
pub trait Listener: Send + Sync {
    /// The view changed: read the snapshots again.
    fn changed(&self);
    /// Something to tell the user, in words they understand.
    fn notice(&self, text: String);
    /// A join never became a network (wrong password, bad invite).
    fn join_failed(&self, invite: String, error: String);
}

/// Per-network credentials, kept by the app (Android Keystore).
#[uniffi::export(with_foreign)]
pub trait Secrets: Send + Sync {
    fn save(&self, origin: String, account: String, password: String) -> Option<String>;
    fn load(&self, origin: String) -> Option<Credentials>;
    fn delete(&self, origin: String);
}

/// Plays voice calls (LiveKit Android SDK). Joining leaves any other call.
#[uniffi::export(with_foreign)]
pub trait Voice: Send + Sync {
    /// Connect to this LiveKit room and report on `call`.
    fn join(&self, id: u64, url: String, token: String, call: Arc<Call>);
    /// The core is done with call `id`; ignore it if a newer one started.
    fn hang_up(&self, id: u64);
}

/// The way back to the core for one call.
#[derive(uniffi::Object)]
pub struct Call(mpsc::UnboundedSender<MediaEvent>);

#[uniffi::export]
impl Call {
    pub fn connected(&self) {
        let _ = self.0.send(MediaEvent::Connected);
    }

    /// It's over by itself: `reason` if something went wrong.
    pub fn ended(&self, reason: Option<String>) {
        let _ = self.0.send(MediaEvent::Ended(reason));
    }
}

struct Calls {
    voice: Arc<dyn Voice>,
    next: AtomicU64,
}

impl chad_core::Media for Calls {
    fn join(&self, url: String, token: String) -> mpsc::UnboundedReceiver<MediaEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let call = Arc::new(Call(tx.clone()));
        self.voice.join(id, url, token, call);
        let voice = self.voice.clone();
        // Runs on the core's runtime; the core dropping `rx` hangs up.
        tokio::spawn(async move {
            tx.closed().await;
            voice.hang_up(id);
        });
        rx
    }
}

#[derive(uniffi::Record)]
pub struct Credentials {
    pub account: String,
    pub password: String,
}

struct Store(Arc<dyn Secrets>);

impl SecretStore for Store {
    fn save<'a>(&'a self, origin: &'a str, account: &'a str, password: &'a str) -> Fut<'a, ()> {
        let err = self.0.save(origin.into(), account.into(), password.into());
        Box::pin(async move { err.map_or(Ok(()), Err) })
    }

    fn load<'a>(&'a self, origin: &'a str) -> Fut<'a, Option<(String, String)>> {
        let c = self.0.load(origin.into()).map(|c| (c.account, c.password));
        Box::pin(async move { Ok(c) })
    }

    fn delete<'a>(&'a self, origin: &'a str) -> Fut<'a, ()> {
        self.0.delete(origin.into());
        Box::pin(async { Ok(()) })
    }
}

/// What an invite points at: the network's origin (its id) and the channel
/// to open, or `None` if it isn't an invite.
#[derive(uniffi::Record)]
pub struct InviteInfo {
    pub origin: String,
    pub channel: Option<String>,
}

#[uniffi::export]
pub fn parse_invite(invite: String) -> Option<InviteInfo> {
    let i = chad_core::invite::parse(&invite).ok()?;
    Some(InviteInfo {
        origin: i.origin,
        channel: i.channel,
    })
}

#[derive(uniffi::Object)]
pub struct Client {
    core: Core,
    view: Arc<Mutex<View>>,
    // Dropping it stops the core.
    _rt: tokio::runtime::Runtime,
}

#[derive(uniffi::Record)]
pub struct Network {
    pub network: String,
    pub name: String,
    pub nick: String,
    pub icon: Option<String>,
    /// `None` when connected; otherwise what's going on.
    pub status: Option<String>,
    /// Won't retry by itself: sign in again (`sign_in`).
    pub failed: bool,
    /// Waiting for the code from the sign-up email (`verify`).
    pub needs_code: bool,
    pub guest: bool,
    pub unread: bool,
    pub mentions: u32,
    pub voice: Option<String>,
}

#[derive(uniffi::Enum)]
pub enum Lock {
    None,
    Off,
    On,
    Verified,
    Changed,
}

#[derive(uniffi::Record)]
pub struct Buffer {
    pub target: String,
    pub dm: bool,
    pub topic: String,
    pub unread: u32,
    pub mentions: u32,
    pub online: Option<bool>,
    pub away: bool,
    pub encryption: Lock,
    pub typing: Vec<String>,
    pub history_start: bool,
    /// Who's in the voice room.
    pub voice: Vec<String>,
    /// Channels: ops haven't turned voice off.
    pub voice_allowed: bool,
}

#[derive(uniffi::Record)]
pub struct Reaction {
    pub emoji: String,
    pub nicks: Vec<String>,
}

#[derive(uniffi::Record)]
pub struct Message {
    /// Stable for the UI's list keys: the msgid, or a local one until echoed.
    pub key: String,
    pub msgid: Option<String>,
    pub time: i64,
    pub nick: String,
    /// Display name if they set one.
    pub shown: String,
    pub avatar: Option<String>,
    pub text: String,
    pub action: bool,
    pub notice: bool,
    pub reply_to: Option<String>,
    pub pending: bool,
    pub failed: Option<String>,
    pub redacted: bool,
    pub encrypted: bool,
    /// Encrypted, but it didn't decrypt.
    pub undecryptable: bool,
    pub mention: bool,
    pub own: bool,
    pub reactions: Vec<Reaction>,
}

#[derive(uniffi::Record)]
pub struct Member {
    pub nick: String,
    pub shown: String,
    pub prefix: String,
    pub away: bool,
}

fn status(s: &ConnState) -> (Option<String>, bool, bool) {
    match s {
        ConnState::Connected => (None, false, false),
        ConnState::Connecting => (Some("Connecting…".into()), false, false),
        ConnState::NeedsVerification { message } => (Some(message.clone()), false, true),
        ConnState::Disconnected { error } => (Some(format!("{error} Reconnecting…")), false, false),
        ConnState::Failed { error } => (Some(error.clone()), true, false),
    }
}

/// rustls-native-certs finds no roots on Android, so point it at the system
/// store (the updatable APEX one first, Android 14+), plus `<data>/certs` for
/// a CA the app was given (Caddy's internal one, for the dev stack).
#[cfg(target_os = "android")]
fn trust_android_roots(data_dir: &str) {
    if std::env::var_os("SSL_CERT_DIR").is_some() {
        return;
    }
    let own = std::path::Path::new(data_dir).join("certs");
    let _ = std::fs::create_dir_all(&own);
    let dirs: Vec<PathBuf> = [
        PathBuf::from("/apex/com.android.conscrypt/cacerts"),
        PathBuf::from("/system/etc/security/cacerts"),
        own,
    ]
    .into_iter()
    .filter(|d| d.is_dir())
    .collect();
    if let Ok(v) = std::env::join_paths(dirs) {
        // SAFETY: runs once, before this library starts any thread that reads
        // the environment; the JVM's own threads don't read it through libc here.
        unsafe { std::env::set_var("SSL_CERT_DIR", v) };
    }
}

impl Client {
    fn view(&self) -> MutexGuard<'_, View> {
        self.view.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn net(&self, network: String, cmd: NetCommand) {
        self.core.send(Command::Net { network, cmd });
    }
}

#[uniffi::export]
impl Client {
    #[uniffi::constructor]
    pub fn new(
        data_dir: String,
        version: String,
        secrets: Arc<dyn Secrets>,
        voice: Arc<dyn Voice>,
        listener: Arc<dyn Listener>,
    ) -> Arc<Self> {
        #[cfg(target_os = "android")]
        trust_android_roots(&data_dir);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");
        let _g = rt.enter();
        let (core, mut events) = chad_core::start(chad_core::Config {
            data_dir: PathBuf::from(data_dir),
            secrets: Arc::new(Store(secrets)),
            notify: Arc::new(|| {}),
            media: Some(Arc::new(Calls {
                voice,
                next: AtomicU64::new(1),
            })),
            version,
        });
        let view = Arc::new(Mutex::new(View::default()));
        let v = view.clone();
        rt.spawn(async move {
            while let Some(ev) = events.recv().await {
                v.lock().unwrap_or_else(|e| e.into_inner()).apply(&ev);
                match ev {
                    Event::Error { error, .. } | Event::SignUpFailed { error, .. } => {
                        listener.notice(error)
                    }
                    Event::JoinFailed { invite, error } => listener.join_failed(invite, error),
                    Event::Uploaded { result: Err(e), .. } => {
                        listener.notice(format!("Upload failed: {e}"))
                    }
                    _ => {}
                }
                listener.changed();
            }
        });
        drop(_g);
        Arc::new(Self {
            core,
            view,
            _rt: rt,
        })
    }

    /// An invite URL, `irchad://` link or bare host, with an account.
    pub fn join(&self, invite: String, account: String, password: String) {
        let auth = Auth::Login { account, password };
        self.core.send(Command::Join { invite, auth });
    }

    pub fn register(
        &self,
        invite: String,
        account: String,
        password: String,
        email: Option<String>,
    ) {
        let auth = Auth::Register {
            account,
            password,
            email: email.filter(|e| !e.trim().is_empty()),
        };
        self.core.send(Command::Join { invite, auth });
    }

    pub fn join_guest(&self, invite: String, nick: String) {
        let auth = Auth::Guest { nick };
        self.core.send(Command::Join { invite, auth });
    }

    /// Forget a network: disconnect, delete its cache and password.
    pub fn remove(&self, network: String) {
        self.core.send(Command::Remove { network });
    }

    pub fn send(&self, network: String, target: String, text: String, reply_to: Option<String>) {
        self.net(
            network,
            NetCommand::Send {
                target,
                text,
                reply_to,
            },
        );
    }

    pub fn react(&self, network: String, target: String, msgid: String, emoji: String) {
        self.net(
            network,
            NetCommand::React {
                target,
                msgid,
                emoji,
            },
        );
    }

    pub fn unreact(&self, network: String, target: String, msgid: String, emoji: String) {
        self.net(
            network,
            NetCommand::Unreact {
                target,
                msgid,
                emoji,
            },
        );
    }

    pub fn redact(&self, network: String, target: String, msgid: String) {
        self.net(network, NetCommand::Redact { target, msgid });
    }

    pub fn mark_read(&self, network: String, target: String) {
        self.net(network, NetCommand::MarkRead { target });
    }

    pub fn load_older(&self, network: String, target: String) {
        self.net(network, NetCommand::LoadOlder { target });
    }

    /// Fetch a channel's members; call when it's shown, and after a reconnect.
    pub fn members(&self, network: String, channel: String) {
        self.net(network, NetCommand::Members { channel });
    }

    pub fn join_channel(&self, network: String, channel: String) {
        self.net(network, NetCommand::JoinChannel { channel });
    }

    pub fn part(&self, network: String, channel: String) {
        self.net(network, NetCommand::Part { channel });
    }

    pub fn open_dm(&self, network: String, nick: String) {
        self.net(network, NetCommand::OpenDm { nick });
    }

    pub fn close_dm(&self, network: String, nick: String) {
        self.net(network, NetCommand::CloseDm { nick });
    }

    /// Join a channel's voice room, leaving any other call.
    pub fn join_voice(&self, network: String, channel: String) {
        self.net(network, NetCommand::JoinVoice { channel });
    }

    pub fn leave_voice(&self, network: String) {
        self.net(network, NetCommand::LeaveVoice);
    }

    pub fn verify(&self, network: String, code: String) {
        self.net(network, NetCommand::Verify { code });
    }

    pub fn sign_in(&self, network: String, password: String) {
        self.net(network, NetCommand::SignIn { password });
    }

    pub fn networks(&self) -> Vec<Network> {
        let v = self.view();
        v.nets
            .iter()
            .map(|n| {
                let (status, failed, needs_code) = status(&n.info.state);
                let voice = v
                    .call
                    .as_ref()
                    .filter(|(net, _)| *net == n.info.network)
                    .and_then(|(_, s)| match s {
                        chad_core::VoiceState::Connected { channel }
                        | chad_core::VoiceState::Connecting { channel } => Some(channel.clone()),
                        chad_core::VoiceState::Idle => None,
                    });
                Network {
                    network: n.info.network.clone(),
                    name: n.info.name.clone(),
                    nick: n.info.nick.clone(),
                    icon: n.info.icon.as_ref().map(|u| u.to_string()),
                    status,
                    failed,
                    needs_code,
                    guest: n.info.guest,
                    unread: n.unread(),
                    mentions: n.mentions(),
                    voice,
                }
            })
            .collect()
    }

    pub fn buffers(&self, network: String) -> Vec<Buffer> {
        let v = self.view();
        let Some(n) = v.net(&network).map(|i| &v.nets[i]) else {
            return Vec::new();
        };
        n.buffers
            .iter()
            .map(|b| Buffer {
                target: b.info.target.clone(),
                dm: b.info.kind == chad_core::BufferKind::Dm,
                topic: b.info.topic.clone(),
                unread: b.info.unread,
                mentions: b.info.mentions,
                online: b.info.online,
                away: b.info.away,
                encryption: match b.info.encryption {
                    Encryption::Unknown => Lock::None,
                    Encryption::Off => Lock::Off,
                    Encryption::On => Lock::On,
                    Encryption::Verified => Lock::Verified,
                    Encryption::Changed => Lock::Changed,
                },
                typing: b.typing().map(|t| n.shown(t).to_owned()).collect(),
                history_start: b.history_start,
                voice: b.voice.clone(),
                voice_allowed: b.info.voice,
            })
            .collect()
    }

    pub fn messages(&self, network: String, target: String) -> Vec<Message> {
        let v = self.view();
        let (Some(ni), Some(b)) = (v.net(&network), v.buf(&network, &target)) else {
            return Vec::new();
        };
        let n = &v.nets[ni];
        b.msgs
            .iter()
            .map(|m| {
                let profile = n.profiles.get(&chad_core::view::key(&m.nick));
                Message {
                    key: m
                        .msgid
                        .clone()
                        .unwrap_or_else(|| format!("local:{}", m.local_id.unwrap_or_default())),
                    msgid: m.msgid.clone(),
                    time: m.time,
                    nick: m.nick.clone(),
                    shown: n.shown(&m.nick).to_owned(),
                    avatar: profile
                        .and_then(|p| p.avatar.as_ref())
                        .map(|u| u.to_string()),
                    text: strip_formatting(&m.text),
                    action: m.kind == MessageKind::Action,
                    notice: m.kind == MessageKind::Notice,
                    reply_to: m.reply_to.clone(),
                    pending: m.status == SendStatus::Pending,
                    failed: match &m.status {
                        SendStatus::Failed(e) => Some(e.clone()),
                        _ => None,
                    },
                    redacted: m.redacted,
                    encrypted: m.crypt == Crypt::Encrypted,
                    undecryptable: m.crypt == Crypt::Failed,
                    mention: m.mention,
                    own: m.own,
                    reactions: m
                        .reactions
                        .iter()
                        .map(|r| Reaction {
                            emoji: r.emoji.clone(),
                            nicks: r.nicks.clone(),
                        })
                        .collect(),
                }
            })
            .collect()
    }

    /// `None` until fetched with `members`.
    pub fn member_list(&self, network: String, channel: String) -> Option<Vec<Member>> {
        let v = self.view();
        let n = &v.nets[v.net(&network)?];
        let ms = v.buf(&network, &channel)?.members.as_ref()?;
        Some(
            ms.iter()
                .map(|m| Member {
                    nick: m.nick.clone(),
                    shown: n.shown(&m.nick).to_owned(),
                    prefix: m.prefix.clone(),
                    away: m.away,
                })
                .collect(),
        )
    }
}

/// IRC formatting codes removed.
// shortcut: Android shows no bold or colours yet; share desktop's
// `format::parse` from chad-core when Android renders spans.
fn strip_formatting(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    // Up to `n` chars matching `f`.
    fn skip(
        chars: &mut std::iter::Peekable<std::str::Chars>,
        n: usize,
        f: fn(&char) -> bool,
    ) -> usize {
        (0..n).take_while(|_| chars.next_if(f).is_some()).count()
    }
    while let Some(c) = chars.next() {
        match c {
            '\x03' => {
                if skip(&mut chars, 2, char::is_ascii_digit) > 0 {
                    let mut look = chars.clone();
                    if look.next() == Some(',') && look.peek().is_some_and(char::is_ascii_digit) {
                        chars.next();
                        skip(&mut chars, 2, char::is_ascii_digit);
                    }
                }
            }
            '\x04' => {
                let mut look = chars.clone();
                if skip(&mut look, 6, char::is_ascii_hexdigit) == 6 {
                    chars = look;
                    let mut look = chars.clone();
                    if look.next() == Some(',') && skip(&mut look, 6, char::is_ascii_hexdigit) == 6
                    {
                        chars = look;
                    }
                }
            }
            '\x02' | '\x0f' | '\x11' | '\x16' | '\x1d' | '\x1e' | '\x1f' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_formatting() {
        let s = "a\x02b\x0304red\x03 \x0308,02yb\x03,x \x04ff0000hex\x0f\x1dc\x03";
        assert_eq!(strip_formatting(s), "abred yb,x hexc");
        assert_eq!(strip_formatting("\x03123"), "3");
        assert_eq!(strip_formatting("\x04abc"), "abc");
        assert_eq!(strip_formatting("日本\x02語"), "日本語");
    }

    struct Nop;
    impl Listener for Nop {
        fn changed(&self) {}
        fn notice(&self, _: String) {}
        fn join_failed(&self, _: String, _: String) {}
    }
    impl Voice for Nop {
        fn join(&self, _: u64, _: String, _: String, _: Arc<Call>) {}
        fn hang_up(&self, _: u64) {}
    }
    impl Secrets for Nop {
        fn save(&self, _: String, _: String, _: String) -> Option<String> {
            None
        }
        fn load(&self, _: String) -> Option<Credentials> {
            None
        }
        fn delete(&self, _: String) {}
    }

    #[derive(Default)]
    struct Log(Mutex<Vec<String>>);
    impl Voice for Log {
        fn join(&self, id: u64, url: String, _: String, call: Arc<Call>) {
            call.connected();
            self.0.lock().unwrap().push(format!("join {id} {url}"));
        }
        fn hang_up(&self, id: u64) {
            self.0.lock().unwrap().push(format!("hang up {id}"));
        }
    }

    #[tokio::test]
    async fn dropping_a_call_hangs_it_up() {
        use chad_core::Media;
        let log = Arc::new(Log::default());
        let calls = Calls {
            voice: log.clone(),
            next: AtomicU64::new(1),
        };
        let mut a = calls.join("wss://a".into(), "t".into());
        assert_eq!(a.recv().await, Some(MediaEvent::Connected));
        let b = calls.join("wss://b".into(), "t".into());
        drop(a);
        drop(b);
        for _ in 0..100 {
            if log.0.lock().unwrap().len() == 4 {
                break;
            }
            tokio::task::yield_now().await;
        }
        let mut got = log.0.lock().unwrap().clone();
        got[2..].sort();
        assert_eq!(
            got,
            ["join 1 wss://a", "join 2 wss://b", "hang up 1", "hang up 2"]
        );
    }

    #[test]
    fn starts_empty_and_stops() {
        let dir = std::env::temp_dir().join(format!("chad-ffi-{}", std::process::id()));
        let c = Client::new(
            dir.display().to_string(),
            "test".into(),
            Arc::new(Nop),
            Arc::new(Nop),
            Arc::new(Nop),
        );
        assert!(c.networks().is_empty());
        assert!(c.buffers("https://nope".into()).is_empty());
        assert!(c.member_list("https://nope".into(), "#a".into()).is_none());
        drop(c);
        let _ = std::fs::remove_dir_all(dir);
    }
}
