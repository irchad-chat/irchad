//! UI-agnostic client state. Commands in, events out. Never depends on egui.
//!
//! [`start`] spawns the core on the current tokio runtime. The UI sends
//! [`Command`]s and projects the [`Event`] stream into its own view model.
//! Networks are keyed by origin (`https://chat.example.com`); buffers by name
//! as the server spells it, compared under the network's casemapping.

mod conn;
mod discovery;
pub mod invite;
mod manager;
mod net;
mod platform;
pub mod secrets;
mod store;
pub mod time;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;
use url::Url;

pub use manager::start;

pub struct Config {
    /// Per-user data directory; one SQLite cache per network lives under it.
    pub data_dir: PathBuf,
    pub secrets: Arc<dyn secrets::SecretStore>,
    /// Called after every event, e.g. `ctx.request_repaint()`.
    pub notify: Arc<dyn Fn() + Send + Sync>,
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
    /// Delete one of your own messages.
    Redact {
        target: String,
        msgid: String,
    },
    Typing {
        target: String,
        state: Typing,
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
    /// The code from the sign-up email.
    Verify {
        code: String,
    },
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
    /// Full member list of a channel after any change.
    Members {
        network: String,
        channel: String,
        members: Vec<Member>,
    },
    Typing {
        network: String,
        target: String,
        nick: String,
        state: Typing,
    },
    /// Something went wrong, in words a user understands.
    Error {
        network: String,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInfo {
    pub network: String,
    pub name: String,
    pub icon: Option<Url>,
    pub nick: String,
    pub state: ConnState,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    Channel,
    Dm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub status: SendStatus,
    pub redacted: bool,
    pub reactions: Vec<Reaction>,
    /// Highlights you (always true for someone else's DM).
    pub mention: bool,
    pub own: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Privmsg,
    Notice,
    Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendStatus {
    Pending,
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
