//! UI-agnostic client state. Commands in, events out. Never depends on egui.
//!
//! [`start`] spawns the core on the current tokio runtime. The UI sends
//! [`Command`]s and projects the [`Event`] stream with [`view::View`].
//! Networks are keyed by origin (`https://chat.example.com`); buffers by name
//! as the server spells it, compared under the network's casemapping.

mod chadd;
pub mod complete;
mod conn;
mod discovery;
pub mod emoji;
pub mod format;
pub mod invite;
mod manager;
mod net;
mod platform;
pub mod secrets;
mod store;
pub mod time;
pub mod view;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;
use url::Url;

pub use chad_proto::chadd::{Gif, GifPage, LinkPreview, Sound};
pub use discovery::fetch_icon;
pub use manager::start;

/// File or image bytes; `Debug` shows only the length, so logging a command
/// never dumps a file.
#[derive(Clone, PartialEq, Eq)]
pub struct Blob(pub Arc<[u8]>);

impl std::fmt::Debug for Blob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Blob({} bytes)", self.0.len())
    }
}

pub struct Config {
    /// Per-user data directory; one SQLite cache per network lives under it.
    pub data_dir: PathBuf,
    pub secrets: Arc<dyn secrets::SecretStore>,
    /// Called after every event, e.g. `ctx.request_repaint()`.
    pub notify: Arc<dyn Fn() + Send + Sync>,
    /// Voice calls; `None` turns voice off.
    pub media: Option<Arc<dyn Media>>,
    /// What the app calls itself, e.g. `IrChad 0.1.1`: the CTCP VERSION reply.
    pub version: String,
}

/// Whatever plays voice calls (`chad-media` on desktop). The core decides when
/// to join and leave; this only connects.
pub trait Media: Send + Sync {
    /// Joins a LiveKit room, leaving any other. Report on the returned channel;
    /// when the core drops its end, hang up.
    fn join(&self, url: String, token: String) -> mpsc::UnboundedReceiver<MediaEvent>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaEvent {
    Connected,
    /// The call is over by itself: `Some(reason)` if something went wrong.
    Ended(Option<String>),
}

/// Handle to a running core. Dropping it shuts the core down.
#[derive(Clone)]
pub struct Core {
    tx: mpsc::UnboundedSender<Command>,
}

impl Core {
    pub fn send(&self, cmd: Command) {
        // The core only stops when every handle is gone.
        let _ = self.tx.send(cmd);
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    /// Whatever was pasted or opened: invite URL, `irchad://` link or bare host.
    Join {
        invite: String,
        auth: Auth,
    },
    /// Forget a network: disconnect, delete its cache and password.
    Remove {
        network: String,
    },
    /// The app went to the background (`true`) or came back (D39). Every
    /// network drops its socket meanwhile, unless it's in voice, and catches
    /// up through chathistory when back. Desktop never sends it.
    Background {
        on: bool,
    },
    Net {
        network: String,
        cmd: NetCommand,
    },
}

#[derive(Debug, Clone)]
pub enum Auth {
    Login {
        account: String,
        password: String,
    },
    Register {
        account: String,
        password: String,
        email: Option<String>,
    },
    /// No account: no SASL, nothing in the secret store, and no always-on, so
    /// channels are rejoined on reconnect and anything said while away is missed.
    Guest {
        nick: String,
    },
}

#[derive(Debug, Clone)]
pub enum NetCommand {
    /// `text` may hold newlines; it goes out as one `draft/multiline` message.
    Send {
        target: String,
        text: String,
        reply_to: Option<String>,
    },
    React {
        target: String,
        msgid: String,
        emoji: String,
    },
    /// Take back your own reaction.
    Unreact {
        target: String,
        msgid: String,
        emoji: String,
    },
    /// Replace the text of one of your own messages (`+chad/edit`, D51).
    Edit {
        target: String,
        msgid: String,
        text: String,
    },
    /// Delete one of your own messages.
    Redact {
        target: String,
        msgid: String,
    },
    Typing {
        target: String,
        state: Typing,
    },
    /// Web push (D39) to this subscription, renewed on every connection
    /// after `Event::PushAvailable`; `None` stops it.
    Push {
        subscription: Option<PushSubscription>,
    },
    /// Everything in the buffer has been seen.
    MarkRead {
        target: String,
    },
    /// One more page of scrollback before the oldest message held.
    LoadOlder {
        target: String,
    },
    /// Jump to a message (a reply's parent, say) that isn't loaded.
    LoadAround {
        target: String,
        msgid: String,
    },
    /// Fetch the member list; call when a channel becomes visible, and again
    /// after a reconnect (member lists are dropped with the connection).
    Members {
        channel: String,
    },
    JoinChannel {
        channel: String,
    },
    /// Leaving is explicit and persists across devices (always-on).
    Part {
        channel: String,
    },
    OpenDm {
        nick: String,
    },
    /// Unregister our account on the server (ergo's two-step `NS UNREGISTER`),
    /// then forget the network as [`Command::Remove`] does. Only while connected.
    DeleteAccount,
    /// Hide a DM and forget its local history; a new message reopens it.
    CloseDm {
        nick: String,
    },
    /// Join a channel's voice room, leaving any other (on any network).
    JoinVoice {
        channel: String,
    },
    LeaveVoice,
    /// The code from the sign-up email.
    Verify {
        code: String,
    },
    /// Try again with this password after `ConnState::Failed` (a missing or
    /// changed password, say). It's saved once a login works.
    SignIn {
        password: String,
    },
    /// A guest makes an account and carries on as it, keeping their channels
    /// and DMs. The network reconnects to sign up (`NeedsVerification` may
    /// follow); once logged in its `NetworkInfo::guest` turns false. On failure,
    /// `Event::SignUpFailed`, and it goes back to being a guest.
    Register {
        account: String,
        password: String,
        email: Option<String>,
    },
    /// Upload through chadd; answered by `Event::Uploaded`. `target` is the
    /// buffer it's for, `None` for an avatar.
    Upload {
        target: Option<String>,
        name: String,
        bytes: Blob,
    },
    /// Empty clears it. Other clients see it as metadata `display-name`.
    SetDisplayName {
        name: String,
    },
    /// An image URL (upload it first); empty clears it. Metadata `avatar`.
    SetAvatar {
        url: String,
    },
    /// `Some(reason)` to go away, `None` to come back. Kept across reconnects.
    SetAway {
        message: Option<String>,
    },
    /// What a link says about itself, fetched by chadd: `Event::Preview`.
    Preview {
        url: String,
    },
    /// An image's bytes: from this network's own uploads directly, anything
    /// else through chadd (D25). Answered by `Event::Media`.
    Media {
        url: String,
    },
    /// A page of GIFs through chadd (D30); empty `query` for trending.
    /// Answered by `Event::Gifs`.
    Gifs {
        query: String,
        pos: Option<String>,
    },
    /// The network's soundboard: `Event::Sounds`.
    Sounds,
    /// One soundboard clip's bytes: `Event::Sound`.
    Sound {
        file: String,
    },
    /// Someone's devices with encrypted DMs on, fetched again: `Event::Devices`.
    Devices {
        nick: String,
    },
    /// Compare emoji with someone to check their device's keys (SAS).
    Verification {
        nick: String,
        step: VerifyStep,
    },
    /// Keep using someone's devices after a key change, without verifying them.
    AcceptKeys {
        nick: String,
    },
    /// Take one of our other devices' encrypted-DM records off the server (a
    /// stale install). A device that's still running publishes itself again.
    RemoveDevice {
        id: String,
    },
    /// Hide everything from someone, on this device only: by account, else by
    /// host. `account` is what the UI knows of them; without it we ask (WHOX).
    Block {
        nick: String,
        account: Option<String>,
    },
    /// `Block::mask` from `Event::Blocks`.
    Unblock {
        mask: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyStep {
    Start,
    /// Answer their request.
    Accept,
    /// The emoji are the same on both screens.
    Match,
    Cancel,
}

#[derive(Debug, Clone)]
pub enum Event {
    /// A network appeared or changed.
    Network(NetworkInfo),
    NetworkRemoved {
        network: String,
    },
    /// A `Join` never got as far as being a network.
    JoinFailed {
        invite: String,
        error: String,
    },
    /// A guest's `NetCommand::Register` didn't work; they're a guest again.
    SignUpFailed {
        network: String,
        error: String,
    },
    /// Capabilities enabled or dropped at runtime (`cap-notify`). E.g. without
    /// `away-notify` the member list's away state goes stale.
    CapsChanged {
        network: String,
        added: Vec<String>,
        removed: Vec<String>,
    },
    Buffer {
        network: String,
        buffer: BufferInfo,
    },
    BufferRemoved {
        network: String,
        target: String,
    },
    /// An op renamed a channel. A `Buffer` with the new name follows.
    BufferRenamed {
        network: String,
        from: String,
        to: String,
    },
    /// New or updated messages, oldest first. Merge by `msgid` (or `local_id`
    /// for your own not yet echoed) and keep sorted by `time`.
    Messages {
        network: String,
        target: String,
        messages: Vec<ChatMessage>,
    },
    /// `LoadOlder` reached the start of the history.
    HistoryStart {
        network: String,
        target: String,
    },
    /// Full member list of a channel, at most once per batch of changes.
    /// Shared, so the view keeps it without a copy.
    Members {
        network: String,
        channel: String,
        members: std::sync::Arc<[Member]>,
    },
    Typing {
        network: String,
        target: String,
        nick: String,
        state: Typing,
    },
    /// Our own voice call on this network.
    Voice {
        network: String,
        state: VoiceState,
    },
    /// Who's in a channel's voice room, per `chad/voice` metadata. Inside the
    /// room, the call's own participant list is the better source.
    VoiceMembers {
        network: String,
        channel: String,
        nicks: Vec<String>,
    },
    /// Something went wrong, in words a user understands.
    Error {
        network: String,
        error: String,
    },
    Uploaded {
        network: String,
        target: Option<String>,
        result: Result<Url, String>,
    },
    /// Someone's display name or avatar (metadata), as far as we know it.
    Profile {
        network: String,
        nick: String,
        display_name: Option<String>,
        avatar: Option<Url>,
    },
    Preview {
        network: String,
        url: String,
        preview: Option<LinkPreview>,
    },
    Media {
        network: String,
        url: String,
        result: Result<Blob, String>,
    },
    /// For `NetCommand::Gifs` with this `query` and `pos`.
    Gifs {
        network: String,
        query: String,
        pos: Option<String>,
        result: Result<GifPage, String>,
    },
    Sounds {
        network: String,
        result: Result<Vec<Sound>, String>,
    },
    Sound {
        network: String,
        file: String,
        result: Result<Blob, String>,
    },
    /// Someone's devices with encrypted DMs on (ours, for our own nick).
    Devices {
        network: String,
        nick: String,
        devices: Vec<DeviceInfo>,
    },
    /// Connected, and the network can push (`draft/webpush`): subscribe with
    /// its VAPID public key (base64url) and answer with `NetCommand::Push`.
    PushAvailable {
        network: String,
        vapid: String,
    },
    /// A verification with `nick` moved on.
    Verification {
        network: String,
        nick: String,
        state: VerifyState,
    },
    /// The network's whole block list, at startup and after every change.
    Blocks {
        network: String,
        blocks: Vec<Block>,
    },
}

/// Someone blocked: nothing from them is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// `$a:<account>` (ASCII lowercase), or `*!*@<host>` for someone without one.
    pub mask: String,
    /// Who they were when blocked, to show in the list.
    pub nick: String,
}

impl Block {
    /// `m` is theirs. Messages carry no host, so a host block goes by the
    /// nick they had when blocked.
    pub fn hides(&self, m: &ChatMessage) -> bool {
        !m.own
            && match (self.mask.strip_prefix("$a:"), &m.account) {
                (Some(a), Some(ma)) => ma.eq_ignore_ascii_case(a),
                (None, None) => m.nick.eq_ignore_ascii_case(&self.nick),
                _ => false,
            }
    }
}

/// False if `m` is from someone in `blocks`; otherwise drops their reactions.
pub fn unblocked(blocks: &[Block], m: &mut ChatMessage) -> bool {
    if blocks.is_empty() {
        return true;
    }
    if blocks.iter().any(|b| b.hides(m)) {
        return false;
    }
    // shortcut: reactions carry only a nick, so this goes by the nick they had when blocked.
    for r in &mut m.reactions {
        r.nicks
            .retain(|n| !blocks.iter().any(|b| b.nick.eq_ignore_ascii_case(n)));
    }
    m.reactions.retain(|r| !r.nicks.is_empty());
    true
}

/// Where ergo sends our pushes (`draft/webpush`): the push service's URL and
/// the receiving device's keys (RFC 8291), base64url.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushSubscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    /// The device's signing key, for comparing by eye.
    pub fingerprint: String,
    pub verified: bool,
    /// This install.
    pub this: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyState {
    /// They asked: `VerifyStep::Accept` or `Cancel`.
    Incoming,
    Waiting,
    /// `(emoji, name)` to compare with what they see: `Match` or `Cancel`.
    Compare(Vec<(&'static str, &'static str)>),
    Done,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInfo {
    pub network: String,
    pub name: String,
    pub icon: Option<Url>,
    pub nick: String,
    pub state: ConnState,
    /// No account; `NetCommand::Register` makes one.
    pub guest: bool,
    // Whether or not we're an oper.
    pub oper: bool,
    /// Largest upload chadd takes, if it takes them (and we have an account).
    pub uploads: Option<u64>,
    /// Who chadd's GIF search comes from, if it has one (and we have an account).
    pub gifs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    /// Sign-up needs the code from an email; answer with `NetCommand::Verify`.
    NeedsVerification {
        message: String,
    },
    Connected,
    /// Retrying by itself.
    Disconnected {
        error: String,
    },
    /// Won't retry until the user does something (wrong password, say).
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferInfo {
    pub target: String,
    pub kind: BufferKind,
    pub topic: String,
    pub unread: u32,
    pub mentions: u32,
    /// DMs: whether the other side is connected (MONITOR), and away.
    pub online: Option<bool>,
    pub away: bool,
    /// Channels: ops can turn voice off with metadata `chad/voice=off`.
    pub voice: bool,
    /// DMs: whether they're end-to-end encrypted.
    pub encryption: Encryption,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encryption {
    /// A channel, or their keys aren't known yet.
    Unknown,
    /// They publish no keys (not IrChad, or a guest): DMs go in the clear.
    Off,
    On,
    /// Every device of theirs checked by comparing emoji.
    Verified,
    /// A device key changed since we last saw it: a new install, or someone
    /// in the middle. Until verified or `AcceptKeys`.
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceState {
    Idle,
    Connecting { channel: String },
    Connected { channel: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    Channel,
    Dm,
}

/// The default is a plain, sent PRIVMSG with everything else empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatMessage {
    pub msgid: Option<String>,
    /// Set on your own messages from this session, so the echo can replace them.
    pub local_id: Option<u64>,
    /// Unix milliseconds, server time.
    pub time: i64,
    pub nick: String,
    pub account: Option<String>,
    pub kind: MessageKind,
    pub text: String,
    pub reply_to: Option<String>,
    /// This message is an edit of that msgid; it's applied there, never shown.
    pub edits: Option<String>,
    /// The text was changed by an edit.
    pub edited: bool,
    pub status: SendStatus,
    pub redacted: bool,
    pub reactions: Vec<Reaction>,
    /// Highlights you (always true for someone else's DM).
    pub mention: bool,
    pub own: bool,
    pub crypt: Crypt,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Crypt {
    #[default]
    Plain,
    /// End-to-end encrypted; `text` is what it decrypted to.
    Encrypted,
    /// Encrypted, but it didn't decrypt: tampered with, replayed, or sent to
    /// keys this device no longer has. `text` is empty.
    Failed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MessageKind {
    #[default]
    Privmsg,
    Notice,
    Action,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SendStatus {
    Pending,
    #[default]
    Sent,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    pub emoji: String,
    pub nicks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub nick: String,
    pub account: Option<String>,
    pub away: bool,
    /// IRC operator (`*` in WHO flags); refreshed only by the next WHO.
    pub oper: bool,
    /// Mode prefixes, highest first (`~@`).
    pub prefix: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typing {
    Active,
    Paused,
    Done,
}

impl Typing {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Done => "done",
        }
    }
}

/// Locks `m`, recovering from poisoning: a holder that panicked mustn't
/// take every later user down with it.
pub fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
