//! UniFFI bindings over chad-core for Android (Phase 6).
//!
//! Kotlin makes one [`Client`], which runs the core on its own tokio runtime
//! and projects events into a [`View`] (D38). After each event the
//! [`Listener`] hears `changed()`, and the UI reads fresh snapshots. Media
//! isn't here: Android plays calls with the LiveKit Android SDK behind
//! [`Voice`], and the core only decides when to join and leave.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chad_core::format;
use chad_core::secrets::{Fut, SecretStore};
use chad_core::view::View;
use chad_core::{
    Auth, Command, ConnState, Core, Crypt, Encryption, Event, MediaEvent, MessageKind, NetCommand,
    SendStatus,
};
use tokio::sync::mpsc;

mod client;
mod push;
mod records;

pub use push::*;
pub use records::*;

uniffi::setup_scaffolding!();

/// Told about changes; called from a core thread, never the UI thread.
#[uniffi::export(with_foreign)]
pub trait Listener: Send + Sync {
    /// The view changed: read the snapshots again.
    fn changed(&self);
    /// Something to tell the user, in words they understand.
    fn notice(&self, text: String);
    /// `Client::upload` finished: put `url` in `target`'s draft to send.
    fn uploaded(&self, network: String, target: String, url: String);
    /// A join never became a network (wrong password, bad invite), or
    /// `Client::discover` didn't find a server.
    fn join_failed(&self, invite: String, error: String);
    /// The answer to `Client::discover`: show the sign-in step for `server`.
    fn discovered(&self, invite: String, server: Server);
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
    /// The server took a `Client::report`: say it was sent. A failure comes as `notice`.
    fn reported(&self, network: String);
    /// Open this in the browser: the Discord sign-in page (D96). Its
    /// `irchad://oauth` redirect goes to `Client::discord_callback`.
    fn open_url(&self, url: String);
    /// A first Discord sign-in: ask for the account name, prefilled with
    /// `suggestion`, and answer with `Client::discord_name`. `error` says why
    /// the last name didn't work.
    fn discord_needs_name(&self, invite: String, suggestion: String, error: Option<String>);
    /// The answer to `Client::set_password`: `None` when it worked.
    fn password_set(&self, network: String, error: Option<String>);
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

/// One of the first-launch cards.
#[derive(uniffi::Record)]
pub struct WelcomeCard {
    pub title: String,
    pub body: String,
}

/// The first-launch cards, in order: show them once, before the join screen.
#[uniffi::export]
pub fn welcome_cards() -> Vec<WelcomeCard> {
    chad_core::welcome::CARDS
        .iter()
        .map(|c| WelcomeCard {
            title: c.title.into(),
            body: c.body.into(),
        })
        .collect()
}

#[derive(uniffi::Object)]
pub struct Client {
    core: Core,
    view: Arc<Mutex<View>>,
    listener: Arc<dyn Listener>,
    /// `Listener::notify` for every message, not only DMs and mentions.
    notify_all: Arc<AtomicBool>,
    // Dropping it stops the core.
    rt: tokio::runtime::Runtime,
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

/// `(from, text, channel)` for each fresh DM or mention in `messages`
/// (every message with `all`).
fn notes(
    v: &View,
    network: &str,
    target: &str,
    messages: &[chad_core::ChatMessage],
    fresh: &[usize],
    all: bool,
) -> Vec<(String, String, bool)> {
    let (Some(ni), Some(b)) = (v.net(network), v.buf(network, target)) else {
        return Vec::new();
    };
    let channel = b.info.kind == chad_core::BufferKind::Channel;
    fresh
        .iter()
        .filter_map(|&i| messages.get(i))
        .filter(|m| (all || m.mention) && chad_core::view::notifiable(m))
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

/// A composer's text after `:shortcode:` replacement, and where its cursor
/// (in chars, Unicode scalar values) went.
#[derive(uniffi::Record)]
pub struct Shortcoded {
    pub text: String,
    pub cursor: u32,
}

/// `None` when `text` has no complete `:shortcode:` to replace. `custom` are
/// the network's own, which stay as typed.
fn replace_shortcodes(
    text: String,
    cursor: u32,
    custom: &[chad_core::CustomEmoji],
) -> Option<Shortcoded> {
    let (mut text, mut cursor) = (text, cursor as usize);
    chad_core::emoji::replace_shortcodes(&mut text, &mut cursor, custom).then(|| Shortcoded {
        text,
        cursor: u32::try_from(cursor).unwrap_or(u32::MAX),
    })
}

/// RNNoise for the mic, desktop's "Extra noise suppression".
#[derive(uniffi::Object)]
pub struct Denoiser(Mutex<Box<nnnoiseless::DenoiseState<'static>>>);

#[uniffi::export]
impl Denoiser {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(nnnoiseless::DenoiseState::new())))
    }

    /// One 10 ms frame at 48 kHz: 480 mono samples, 16-bit little-endian.
    /// Anything else comes back as it was.
    pub fn process(&self, pcm: Vec<u8>) -> Vec<u8> {
        const N: usize = nnnoiseless::DenoiseState::FRAME_SIZE;
        if pcm.len() != N * 2 {
            return pcm;
        }
        let input: Vec<f32> = pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| f32::from(i16::from_le_bytes(b)))
            .collect();
        let mut out = [0f32; N];
        chad_core::lock(&self.0).process_frame(&mut out, &input);
        out.iter()
            .flat_map(|&f| (f.clamp(-32768.0, 32767.0) as i16).to_le_bytes())
            .collect()
    }
}

/// `text` as spans, with each `@`-mention (outside links and code) its own
/// span named by `name` (D66), and each custom emoji from `emoji` its own too.
fn spans(
    text: &str,
    mentions: &[chad_core::Mention],
    emoji: &[chad_core::CustomEmoji],
    name: impl Fn(&chad_core::Mention) -> String,
) -> Vec<Span> {
    let mut out = Vec::new();
    for s in format::parse(&format::code(text)) {
        if s.link || s.mono || (mentions.is_empty() && emoji.is_empty()) {
            out.push(Span::from(s));
            continue;
        }
        let piece = |text: &str| {
            Span::from(format::Span {
                text: text.to_owned(),
                ..s.clone()
            })
        };
        for (text, m) in chad_core::mention::split(&s.text, mentions) {
            if m.is_some() {
                let mut sp = piece(text);
                sp.mention = m.map(&name);
                out.push(sp);
                continue;
            }
            for (text, e) in chad_core::emoji::split(text, emoji) {
                let mut sp = piece(text);
                sp.emoji = e.map(|e| e.url.to_string());
                out.push(sp);
            }
        }
    }
    out
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
mod tests;
