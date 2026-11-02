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

use chad_core::format;
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
    /// The answer to `Client::preview`: `None` if the page has none.
    fn preview(&self, network: String, url: String, preview: Option<LinkPreview>);
    /// The answer to `Client::media`: the image's bytes (≤ 8 MB, not yet
    /// checked to be an image), or `None` if it couldn't be fetched.
    fn media(&self, network: String, url: String, bytes: Option<Vec<u8>>);
    /// The answer to `Client::gifs`.
    fn gifs(&self, network: String, query: String, pos: Option<String>, page: GifPage);
    /// The network can push (D39): register with the UnifiedPush distributor
    /// using this VAPID key and hand the endpoint to `Client::push`.
    /// Comes again on every connection.
    fn push_available(&self, network: String, vapid: String);
    /// A DM or mention from `from` (as shown) just arrived on the live
    /// connection; `target` is the buffer. The app decides whether to show it.
    fn notify(&self, network: String, target: String, from: String, text: String, channel: bool);
    /// A verification (SAS emoji comparison) with `nick` moved on; answer with
    /// `Client::verification`.
    fn verification(&self, network: String, nick: String, state: Sas);
    /// The answer to `Client::sounds`: the network's soundboard, or why not.
    fn sounds(&self, network: String, clips: Vec<Clip>, error: Option<String>);
    /// The answer to `Client::sound`: the clip's encoded bytes (mp3, ogg, wav
    /// or flac, unchecked), or `None` if it couldn't be fetched.
    fn sound(&self, network: String, file: String, bytes: Option<Vec<u8>>);
}

/// A soundboard button (D37).
#[derive(uniffi::Record)]
pub struct Clip {
    pub name: String,
    /// For `Client::sound`.
    pub file: String,
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
    /// Who GIF search comes from, if the network has it.
    pub gifs: Option<String>,
    /// Largest upload in bytes, if the network takes them.
    pub uploads: Option<u64>,
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
pub struct LinkPreview {
    pub title: Option<String>,
    pub description: Option<String>,
    pub site: Option<String>,
    /// Load with `Client::media`, never directly (D25).
    pub image: Option<String>,
}

#[derive(uniffi::Record)]
pub struct Gif {
    /// May be empty.
    pub title: String,
    /// A small rendition for the picker.
    pub preview: String,
    /// What goes in the message.
    pub url: String,
    /// Of `preview`, in pixels.
    pub width: u32,
    pub height: u32,
}

#[derive(uniffi::Record)]
pub struct GifPage {
    pub gifs: Vec<Gif>,
    /// Pass as `pos` for the next page; `None` at the end.
    pub next: Option<String>,
    /// Why there are no GIFs, in words for the user.
    pub error: Option<String>,
}

/// Where a verification with someone is (`chad_core::VerifyState`).
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum Sas {
    /// They asked: `SasStep::Accept` or `Cancel`.
    Incoming,
    Waiting,
    /// Compare with what they see: `SasStep::Match` or `Cancel`.
    Compare {
        emoji: Vec<SasEmoji>,
    },
    Done,
    Failed {
        reason: String,
    },
}

#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct SasEmoji {
    pub emoji: String,
    pub name: String,
}

#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SasStep {
    Start,
    Accept,
    Match,
    Cancel,
}

impl From<chad_core::VerifyState> for Sas {
    fn from(s: chad_core::VerifyState) -> Self {
        use chad_core::VerifyState as V;
        match s {
            V::Incoming => Sas::Incoming,
            V::Waiting => Sas::Waiting,
            V::Compare(e) => Sas::Compare {
                emoji: e
                    .into_iter()
                    .map(|(emoji, name)| SasEmoji {
                        emoji: emoji.into(),
                        name: name.into(),
                    })
                    .collect(),
            },
            V::Done => Sas::Done,
            V::Failed(reason) => Sas::Failed { reason },
        }
    }
}

#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Image,
    Video,
    /// One of this network's uploads that isn't media.
    File,
    Page,
}

#[derive(uniffi::Record, Debug, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub kind: LinkKind,
}

/// A run of message text in one style (`chad_core::format`).
#[derive(uniffi::Record, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub mono: bool,
    /// 0xRRGGBB.
    pub fg: Option<u32>,
    pub bg: Option<u32>,
    /// An `http(s)` link; `text` is the URL.
    pub link: bool,
}

impl From<chad_core::format::Span> for Span {
    fn from(s: chad_core::format::Span) -> Self {
        let rgb = |c: [u8; 3]| u32::from_be_bytes([0, c[0], c[1], c[2]]);
        Span {
            text: s.text,
            bold: s.bold,
            italic: s.italic,
            underline: s.underline,
            strike: s.strike,
            mono: s.mono,
            fg: s.fg.map(rgb),
            bg: s.bg.map(rgb),
            link: s.link,
        }
    }
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
    /// Without formatting, for copying and quoting.
    pub text: String,
    /// `text` with its formatting and links.
    pub spans: Vec<Span>,
    pub action: bool,
    pub notice: bool,
    pub reply_to: Option<String>,
    pub pending: bool,
    pub failed: Option<String>,
    pub redacted: bool,
    /// Changed by an edit since it was sent.
    pub edited: bool,
    pub encrypted: bool,
    /// Encrypted, but it didn't decrypt.
    pub undecryptable: bool,
    pub mention: bool,
    pub own: bool,
    pub reactions: Vec<Reaction>,
    /// The http(s) links in `text`, in order.
    pub links: Vec<Link>,
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
        let c = core.clone();
        rt.spawn(async move {
            while let Some(ev) = events.recv().await {
                let applied = v.lock().unwrap_or_else(|e| e.into_inner()).apply(&ev);
                match ev {
                    Event::Messages {
                        network,
                        target,
                        messages,
                    } => {
                        let notes = notes(
                            &v.lock().unwrap_or_else(|e| e.into_inner()),
                            &network,
                            &target,
                            &messages,
                            &applied.fresh,
                        );
                        for (from, text, channel) in notes {
                            listener.notify(network.clone(), target.clone(), from, text, channel);
                        }
                    }
                    Event::PushAvailable { network, vapid } => {
                        listener.push_available(network, vapid)
                    }
                    Event::Error { error, .. } | Event::SignUpFailed { error, .. } => {
                        listener.notice(error)
                    }
                    Event::JoinFailed { invite, error } => listener.join_failed(invite, error),
                    Event::Verification {
                        network,
                        nick,
                        state,
                    } => listener.verification(network, nick, state.into()),
                    Event::Preview {
                        network,
                        url,
                        preview,
                    } => listener.preview(
                        network,
                        url,
                        preview.map(|p| LinkPreview {
                            title: p.title,
                            description: p.description,
                            site: p.site,
                            image: p.image.map(String::from),
                        }),
                    ),
                    Event::Media {
                        network,
                        url,
                        result,
                    } => listener.media(network, url, result.ok().map(|b| b.0.to_vec())),
                    Event::Gifs {
                        network,
                        query,
                        pos,
                        result,
                    } => listener.gifs(
                        network,
                        query,
                        pos,
                        result.map_or_else(
                            |e| GifPage {
                                gifs: Vec::new(),
                                next: None,
                                error: Some(e),
                            },
                            |p| GifPage {
                                gifs: p
                                    .gifs
                                    .into_iter()
                                    .map(|g| Gif {
                                        title: g.title,
                                        preview: g.preview.into(),
                                        url: g.url.into(),
                                        width: g.width,
                                        height: g.height,
                                    })
                                    .collect(),
                                next: p.next,
                                error: None,
                            },
                        ),
                    ),
                    Event::Uploaded { result: Err(e), .. } => {
                        listener.notice(format!("Upload failed: {e}"))
                    }
                    // Posted as a plain link, like desktop: every client can open it.
                    Event::Uploaded {
                        network,
                        target: Some(target),
                        result: Ok(url),
                    } => c.send(Command::Net {
                        network,
                        cmd: NetCommand::Send {
                            target,
                            text: url.into(),
                            reply_to: None,
                        },
                    }),
                    Event::Sounds { network, result } => match result {
                        Ok(clips) => listener.sounds(
                            network,
                            clips
                                .into_iter()
                                .map(|c| Clip {
                                    name: c.name,
                                    file: c.file,
                                })
                                .collect(),
                            None,
                        ),
                        Err(e) => listener.sounds(network, Vec::new(), Some(e)),
                    },
                    Event::Sound {
                        network,
                        file,
                        result,
                    } => listener.sound(network, file, result.ok().map(|b| b.0.to_vec())),
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

    /// The app went to the background, or came back (D39): connections
    /// pause meanwhile (not one in voice) and catch up when back.
    pub fn background(&self, on: bool) {
        self.core.send(Command::Background { on });
    }

    /// Upload a file through chadd, then post its link to `target`.
    /// Failures come as `Listener::notice`.
    pub fn upload(&self, network: String, target: String, name: String, bytes: Vec<u8>) {
        let bytes = chad_core::Blob(bytes.into());
        self.net(
            network,
            NetCommand::Upload {
                target: Some(target),
                name,
                bytes,
            },
        );
    }

    /// The network's soundboard: `Listener::sounds`.
    pub fn sounds(&self, network: String) {
        self.net(network, NetCommand::Sounds);
    }

    /// One clip's bytes: `Listener::sound`.
    pub fn sound(&self, network: String, file: String) {
        self.net(network, NetCommand::Sound { file });
    }

    pub fn send(&self, network: String, target: String, text: String, reply_to: Option<String>) {
        self.net(
            network,
            NetCommand::Send {
                target,
                text: format::markdown(&text),
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

    /// Start, answer or end a verification with `nick` (see `Listener::verification`).
    pub fn verification(&self, network: String, nick: String, step: SasStep) {
        use chad_core::VerifyStep as V;
        let step = match step {
            SasStep::Start => V::Start,
            SasStep::Accept => V::Accept,
            SasStep::Match => V::Match,
            SasStep::Cancel => V::Cancel,
        };
        self.net(network, NetCommand::Verification { nick, step });
    }

    /// Keep using `nick`'s devices after their keys changed, without verifying.
    pub fn accept_keys(&self, network: String, nick: String) {
        self.net(network, NetCommand::AcceptKeys { nick });
    }

    pub fn sign_in(&self, network: String, password: String) {
        self.net(network, NetCommand::SignIn { password });
    }

    /// What a link says about itself, through chadd: `Listener::preview`.
    pub fn preview(&self, network: String, url: String) {
        self.net(network, NetCommand::Preview { url });
    }

    /// An image's bytes (own uploads directly, the rest through chadd):
    /// `Listener::media`.
    pub fn media(&self, network: String, url: String) {
        self.net(network, NetCommand::Media { url });
    }

    /// A page of GIFs; empty `query` for trending: `Listener::gifs`.
    pub fn gifs(&self, network: String, query: String, pos: Option<String>) {
        self.net(network, NetCommand::Gifs { query, pos });
    }

    /// Pushes for this network go to this UnifiedPush endpoint; `None` stops them.
    pub fn push(&self, network: String, subscription: Option<PushSubscription>) {
        let subscription = subscription.map(|s| chad_core::PushSubscription {
            endpoint: s.endpoint,
            p256dh: s.p256dh,
            auth: s.auth,
        });
        self.net(network, NetCommand::Push { subscription });
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
                    gifs: n.info.gifs.clone(),
                    uploads: n.info.uploads,
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
                let spans: Vec<Span> = format::parse(&m.text).into_iter().map(Span::from).collect();
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
                    links: links(&spans, &network),
                    text: spans.iter().map(|s| s.text.as_str()).collect(),
                    spans,
                    action: m.kind == MessageKind::Action,
                    notice: m.kind == MessageKind::Notice,
                    reply_to: m.reply_to.clone(),
                    pending: m.status == SendStatus::Pending,
                    failed: match &m.status {
                        SendStatus::Failed(e) => Some(e.clone()),
                        _ => None,
                    },
                    redacted: m.redacted,
                    edited: m.edited,
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

/// A UnifiedPush endpoint and the keys the distributor made for it (base64url).
#[derive(uniffi::Record)]
pub struct PushSubscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

/// What a push from ergo (`draft/webpush`) says.
#[derive(uniffi::Enum, Debug, PartialEq, Eq)]
pub enum Push {
    /// A DM or a mention; `target` is the buffer (their nick for a DM).
    Message {
        target: String,
        from: String,
        text: String,
        channel: bool,
    },
    /// Read on another device: clear `target`'s notification.
    Read { target: String },
}

/// A decrypted push payload, which is one IRC line. ergo keeps only the
/// `time`, `msgid` and `account` tags, so an encrypted DM arrives as its
/// readable fallback (`+chad/e2e` is gone). `None` for anything else, such as
/// the `PING` that tests a new subscription. D14: the line is the server's.
#[uniffi::export]
pub fn read_push(line: String) -> Option<Push> {
    if line.len() > 8192 {
        return None;
    }
    let m = chad_irc::Message::parse(&line).ok()?;
    let p = |i| m.param(i).filter(|s| !s.is_empty());
    match m.command.as_str() {
        "PRIVMSG" | "NOTICE" => {
            let (to, text) = (p(0)?, p(1)?);
            let nick = snippet(m.source_nick().filter(|n| !n.is_empty())?, 64);
            let action = text
                .strip_prefix("\x01ACTION ")
                .map(|t| t.trim_end_matches('\x01'));
            if action.is_none() && text.starts_with('\x01') {
                return None;
            }
            let channel = to.starts_with('#');
            let text = match action {
                Some(a) => format!("* {nick} {}", format::strip(a)),
                None => format::strip(text),
            };
            Some(Push::Message {
                target: if channel {
                    snippet(to, 64)
                } else {
                    nick.clone()
                },
                from: nick,
                text: snippet(&text, 200),
                channel,
            })
        }
        "MARKREAD" => Some(Push::Read {
            target: snippet(p(0)?, 64),
        }),
        _ => None,
    }
}

/// `(from, text, channel)` for each fresh DM or mention in `messages`.
fn notes(
    v: &View,
    network: &str,
    target: &str,
    messages: &[chad_core::ChatMessage],
    fresh: &[usize],
) -> Vec<(String, String, bool)> {
    let (Some(ni), Some(b)) = (v.net(network), v.buf(network, target)) else {
        return Vec::new();
    };
    let channel = b.info.kind == chad_core::BufferKind::Channel;
    fresh
        .iter()
        .filter_map(|&i| messages.get(i))
        .filter(|m| m.mention && chad_core::view::notifiable(m))
        .map(|m| {
            let from = snippet(v.nets[ni].shown(&m.nick), 64);
            (from, snippet(&format::strip(&m.text), 200), channel)
        })
        .collect()
}

/// At most `max` chars of `s`, with an ellipsis if cut.
fn snippet(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

/// Emoji whose name or GitHub shortcode contains `query`, all of them for an
/// empty one, in Unicode's order.
#[uniffi::export]
pub fn emojis(query: String) -> Vec<String> {
    let q = query.trim().to_lowercase();
    emojis::iter()
        .filter(|e| q.is_empty() || e.name().contains(&q) || e.shortcodes().any(|s| s.contains(&q)))
        .map(|e| e.as_str().to_owned())
        .collect()
}

/// The links among `spans`, in order, the same way desktop sorts them.
fn links(spans: &[Span], network: &str) -> Vec<Link> {
    use chad_core::format::Kind;
    spans
        .iter()
        .filter(|s| s.link)
        .filter_map(|s| {
            let kind = match format::kind(&s.text, network)? {
                Kind::Image => LinkKind::Image,
                Kind::Video => LinkKind::Video,
                Kind::File => LinkKind::File,
                Kind::Page => LinkKind::Page,
            };
            Some(Link {
                url: s.text.clone(),
                kind,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links() {
        let net = "https://chat.example";
        let spans: Vec<Span> = format::parse(
            "see (https://a.example/x.PNG), https://chat.example/uploads/1/a.zip and \
             <https://b.example/page>. \x02https://c.example/v.mp4\x02 http:// ftp://x",
        )
        .into_iter()
        .map(Span::from)
        .collect();
        assert_eq!(
            Span::from(format::parse("\x0304,12x\x04ff8800y").remove(1)).fg,
            Some(0xff8800)
        );
        assert_eq!(
            Span::from(format::parse("\x0304,12x").remove(0)).bg,
            Some(0x0000fc)
        );
        let got = links(&spans, net);
        let k: Vec<_> = got.iter().map(|l| (l.url.as_str(), l.kind)).collect();
        assert_eq!(
            k,
            [
                ("https://a.example/x.PNG", LinkKind::Image),
                ("https://chat.example/uploads/1/a.zip", LinkKind::File),
                ("https://b.example/page", LinkKind::Page),
                ("https://c.example/v.mp4", LinkKind::Video),
            ]
        );
    }

    #[test]
    fn reads_pushes() {
        let dm = "@time=2026-11-01T10:00:00.000Z;msgid=x;account=al :alice!a@h PRIVMSG bob :hi \x02there\x02";
        assert_eq!(
            read_push(dm.into()),
            Some(Push::Message {
                target: "alice".into(),
                from: "alice".into(),
                text: "hi there".into(),
                channel: false,
            })
        );
        let me = ":alice!a@h PRIVMSG #dev :\x01ACTION waves at bob\x01";
        assert_eq!(
            read_push(me.into()),
            Some(Push::Message {
                target: "#dev".into(),
                from: "alice".into(),
                text: "* alice waves at bob".into(),
                channel: true,
            })
        );
        let long = format!(":alice!a@h PRIVMSG bob :{}", "é".repeat(300));
        let Some(Push::Message { text, .. }) = read_push(long) else {
            panic!()
        };
        assert_eq!(text.chars().count(), 201);
        assert_eq!(
            read_push(":srv MARKREAD #dev timestamp=2026-11-01T10:00:00.000Z".into()),
            Some(Push::Read {
                target: "#dev".into()
            })
        );
        for junk in [
            "PING webpush",
            ":alice!a@h PRIVMSG bob :\x01VERSION\x01",
            ":alice!a@h PRIVMSG bob",
            ":srv 001 bob :hi",
            "",
        ] {
            assert_eq!(read_push(junk.into()), None, "{junk:?}");
        }
        assert_eq!(read_push("x".repeat(9000)), None);
    }

    #[test]
    fn finds_emoji() {
        assert!(emojis("thumbsup".into()).contains(&"👍".to_owned()));
        assert!(emojis("".into()).len() > 1000);
        assert!(emojis("zzzzqqq".into()).is_empty());
    }

    struct Nop;
    impl Listener for Nop {
        fn changed(&self) {}
        fn notice(&self, _: String) {}
        fn join_failed(&self, _: String, _: String) {}
        fn preview(&self, _: String, _: String, _: Option<LinkPreview>) {}
        fn media(&self, _: String, _: String, _: Option<Vec<u8>>) {}
        fn gifs(&self, _: String, _: String, _: Option<String>, _: GifPage) {}
        fn push_available(&self, _: String, _: String) {}
        fn notify(&self, _: String, _: String, _: String, _: String, _: bool) {}
        fn verification(&self, _: String, _: String, _: Sas) {}
        fn sounds(&self, _: String, _: Vec<Clip>, _: Option<String>) {}
        fn sound(&self, _: String, _: String, _: Option<Vec<u8>>) {}
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
