//! Snapshot records the UI reads from a `Client`.

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
    /// None for guests (no account).
    pub account: Option<String>,
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
    /// IRC operator.
    pub oper: bool,
    /// Services account; `None` for guests (not logged in).
    pub account: Option<String>,
}

#[derive(uniffi::Record)]
pub struct Profile {
    pub display_name: Option<String>,
    pub avatar: Option<String>,
}

/// One of someone's devices with encrypted DMs on.
#[derive(uniffi::Record)]
pub struct Device {
    pub id: String,
    /// Its signing key, for comparing by eye.
    pub fingerprint: String,
    pub verified: bool,
    /// This install.
    pub this: bool,
}

/// Someone blocked (`Client::blocks`).
#[derive(uniffi::Record)]
pub struct Blocked {
    /// For `Client::unblock`.
    pub mask: String,
    /// Who they were when blocked.
    pub nick: String,
    /// `None` when blocked by host (a guest).
    pub account: Option<String>,
}
