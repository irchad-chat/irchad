//! One network's state after registration, sans-IO: feed it server lines and UI
//! commands, then drain `out` (lines to send) and `events` (for the UI).
//! `conn` does the I/O, registration and reconnects around it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use chad_irc::Message;
use chad_irc::batch::{Assembler, Batch, Item, Overflow};
use chad_irc::cap::{Negotiator, WANTED};
use chad_irc::isupport::{Casemapping, Isupport};
use chad_irc::label::Labels;
use web_time::Instant;

use crate::store::{Store, StoredBuffer};
use crate::view::Profile;
use crate::{
    Block, BufferInfo, BufferKind, ChatMessage, ConnState, Crypt, Event, ListedChannel, Member,
    Mention, MessageKind, NetCommand, NetworkInfo, PushSubscription, Seen, SendStatus, Typing,
    VoiceState, mention, one_line, time,
};

mod block;
mod command;
mod e2e;
mod inbound;
mod members;
mod metadata;
mod push;
mod receipts;
mod report;
pub use e2e::{DEVICES_KEY, E2e};
use url::Url;

/// Messages per CHATHISTORY request (ergo allows 1000).
const PAGE: u32 = 100;
/// Store meta: where we are in chadd's `redacted` log (D69).
// shortcut: the web has no chadd client yet, so no D69 tombstones; drop once it has.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) const REDACTED_CURSOR: &str = "redacted_cursor";
/// `CHATHISTORY AFTER` pages per buffer when filling a gap on reconnect.
const MAX_GAP_PAGES: u8 = 10;
/// Shown from the cache at startup.
const STARTUP_SCROLLBACK: u32 = 200;
/// D14 bounds on what the server can make us hold.
const MAX_BUFFERS: usize = 1000;
const MAX_MEMBERS: usize = 100_000;
const MAX_TEXT: usize = 16 << 10;
const MAX_NAME: usize = 200;
/// Bytes of text per PRIVMSG line we send: 512 minus the relayed prefix, target and slack.
const MAX_PIECE: usize = 350;
const TYPING_EVERY: Duration = Duration::from_secs(3);
/// At most one CTCP reply this often, so nobody can make us flood ourselves off.
const CTCP_EVERY: Duration = Duration::from_secs(2);
/// REGISTER retries after ergo's test push fails (5s, 10s, 20s, 40s).
const PUSH_TRIES: u32 = 4;
/// Metadata key for voice: a user's room (`#chan`), or `off` on a channel.
pub const VOICE_KEY: &str = "chad/voice";
/// Metadata key on the home channel: the official channels, space-separated.
pub const CHANNELS_KEY: &str = "chad/channels";
/// Metadata key on the home channel: the account that takes reports (a moderator or bot).
pub const REPORT_KEY: &str = "chad/report";
/// Store meta: the last `chad/channels` we acted on, folded.
const OFFICIAL_META: &str = "official";
/// ergo's away reason when an always-on user has no connected devices.
const ERGO_AUTO_AWAY: &str = "User is currently disconnected";
/// De-facto metadata keys other clients read too.
pub const DISPLAY_NAME_KEY: &str = "display-name";
pub const AVATAR_KEY: &str = "avatar";
const MAX_DISPLAY_NAME: usize = 64;
/// Metadata key: what someone's doing, as plain text ("Playing Celeste").
pub const ACTIVITY_KEY: &str = "chad/activity";
const MAX_ACTIVITY: usize = 128;
/// Metadata key: the track behind the activity ([`crate::Music`]).
pub const MUSIC_KEY: &str = "chad/music";
/// Metadata key: `1` if the account sends read receipts in DMs (D68).
pub const RECEIPTS_KEY: &str = "chad/receipts";
/// The client tag on a read receipt: the msgid read up to.
const READ_TAG: &str = "+chad/read";
/// At most one read position per buffer this often: ergo's fakelag allows ~2
/// commands/s, and a busy channel read live would otherwise queue everything
/// we send (lag pings, messages) behind a growing pile of MARKREADs.
const READ_EVERY: Duration = Duration::from_secs(3);
/// Store meta: our own `chad/receipts`, as last seen, for offline startup.
const RECEIPTS_META: &str = "receipts";
/// The server/status buffer's key and name on classic networks (D72): not a
/// valid nick or channel, so it can't collide with either.
pub const SERVER_BUFFER: &str = "*";
/// D14: `322`s kept from one unlabeled LIST, and channels shown from any LIST.
const MAX_LISTED: usize = 20_000;
const MAX_LIST_SHOWN: usize = 5_000;

#[derive(Debug)]
enum Pending {
    /// Our lag `PING`, sent then.
    Lag(Instant),
    /// `rest`: the other devices' copies of an encrypted DM, sent once the first
    /// one's echo gives them a msgid to share.
    Send {
        target: String,
        local: Box<ChatMessage>,
        rest: Vec<Message>,
    },
    /// Someone's metadata, for their E2EE devices.
    Devices {
        nick: String,
    },
    History {
        target: String,
        kind: Hist,
    },
    Targets,
    Sync,
    List,
    /// The channel browser's LIST.
    Browse,
    Who {
        channel: String,
    },
    /// WHOX for someone to block, whose account we didn't know.
    Block {
        nick: String,
    },
    Report,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hist {
    Latest,
    Before,
    After { pages: u8 },
    Around,
}

#[derive(Debug)]
struct Buffer {
    name: String,
    kind: BufferKind,
    topic: String,
    read: Option<i64>,
    /// Keyed by folded nick.
    members: HashMap<String, Member>,
    online: Option<bool>,
    away: bool,
    /// `chad/voice=off` on the channel.
    voice_off: bool,
    /// DMs: their latest read receipt (D68).
    seen: Option<Seen>,
}

impl Buffer {
    fn new(name: &str, kind: BufferKind) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            topic: String::new(),
            read: None,
            members: HashMap::new(),
            online: None,
            away: false,
            voice_off: false,
            seen: None,
        }
    }
}

/// Our voice call on this network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    pub channel: String,
    pub connected: bool,
}

pub struct Net {
    pub store: Store,
    pub info: NetworkInfo,
    pub caps: Negotiator,
    pub isupport: Isupport,
    /// `?channel=` from the invite, joined once synced.
    pub invite_channel: Option<String>,
    /// Discovery's `home`: the only channel whose `chad/channels` we trust.
    pub home: Option<String>,
    /// No account, so never always-on: rejoin our channels ourselves.
    pub guest: bool,
    /// The account we're logged in to (`900`), for `+chad/mentions` (D66).
    pub account: Option<String>,
    pub out: Vec<Message>,
    events: Vec<Event>,
    cm: Casemapping,
    batches: Assembler,
    labels: Labels<Pending>,
    /// Without `labeled-response` (classic networks): sends waiting for their
    /// `echo-message`, oldest first.
    echoes: VecDeque<Pending>,
    /// Without `labeled-response`: a LIST in progress and its `322`s so far.
    listing: Option<(Pending, Vec<Item>)>,
    /// `353`s per folded channel until their `366`.
    names: HashMap<String, HashMap<String, Member>>,
    buffers: HashMap<String, Buffer>,
    next_local: u64,
    next_batch: u64,
    typing_sent: HashMap<String, Instant>,
    /// Per folded buffer: when we last sent our read position, and the newest
    /// one held back since by [`READ_EVERY`] (target, msgid, time).
    read_sent: HashMap<String, Instant>,
    read_due: HashMap<String, (String, String, i64)>,
    connected: bool,
    synced: bool,
    /// The home channel's `chad/report`.
    report_to: Option<String>,
    /// Always-on, per `PERSISTENCE STATUS`. ergo's default (D11).
    persistent: bool,
    joined: HashSet<String>,
    /// `Members` asked before our JOIN landed; WHO then would miss +i users and us.
    who_wanted: HashSet<String>,
    pub voice: Option<Voice>,
    /// A `JoinVoice` was accepted: `conn` takes this and fetches a token.
    pub voice_request: Option<String>,
    /// Our `chad/voice` is set (by this client).
    voice_published: bool,
    /// Folded nick → (nick, folded channel) from their `chad/voice`.
    voice_of: HashMap<String, (String, String)>,
    /// Folded nick → display name and avatar, from metadata.
    profiles: HashMap<String, Profile>,
    /// Folded nicks whose `chad/receipts` is on: we send them receipts.
    receipts_of: HashSet<String>,
    /// Our away message, sent again after every reconnect.
    away: Option<String>,
    /// Our `chad/activity`, set again after every reconnect (empty clears it).
    /// `None` while this client doesn't manage it, so it never touches a value
    /// another device of ours set.
    activity: Option<String>,
    /// Our `chad/music`, alongside `activity`.
    music: Option<crate::Music>,
    /// Encrypted DMs; `None` for guests, or without a keyring.
    pub e2e: Option<E2e>,
    /// Our CTCP VERSION reply.
    pub version: String,
    /// Channels whose member list changed since the last `drain_events`.
    members_dirty: Vec<String>,
    ctcp_at: Option<Instant>,
    /// A lag `PING` is unanswered; D14: never more than one.
    lag_out: bool,
    /// Where ergo pushes to, and whether this connection has registered it.
    push: Option<PushSubscription>,
    push_registered: bool,
    /// Failed REGISTERs on this connection, and when to try again.
    push_tries: u32,
    push_retry: Option<Instant>,
    /// Nothing from these is shown; kept in the store.
    blocks: Vec<Block>,
}

impl Net {
    pub fn new(store: Store, mut info: NetworkInfo) -> Self {
        info.receipts = store.meta(RECEIPTS_META).is_some();
        Self {
            info,
            caps: Negotiator::new(),
            isupport: Isupport::new(),
            invite_channel: None,
            home: None,
            guest: false,
            account: None,
            out: Vec::new(),
            events: Vec::new(),
            cm: Casemapping::Ascii,
            batches: Assembler::new(),
            labels: Labels::new(),
            echoes: VecDeque::new(),
            listing: None,
            names: HashMap::new(),
            buffers: HashMap::new(),
            next_local: 0,
            next_batch: 0,
            typing_sent: HashMap::new(),
            read_sent: HashMap::new(),
            read_due: HashMap::new(),
            connected: false,
            synced: false,
            report_to: None,
            persistent: true,
            joined: HashSet::new(),
            who_wanted: HashSet::new(),
            voice: None,
            voice_request: None,
            voice_published: false,
            voice_of: HashMap::new(),
            profiles: HashMap::new(),
            receipts_of: HashSet::new(),
            away: None,
            activity: None,
            music: None,
            e2e: None,
            version: "IrChad".into(),
            members_dirty: Vec::new(),
            ctcp_at: None,
            lag_out: false,
            push: None,
            push_registered: false,
            push_tries: 0,
            push_retry: None,
            blocks: store.blocks(),
            store,
        }
    }

    fn id(&self) -> String {
        self.info.network.clone()
    }

    /// A plain IRC network (`ircs://` / `irc://`) rather than an IrChad server
    /// (D72): no chadd, no home channel, no auto-join.
    pub fn classic(&self) -> bool {
        crate::invite::classic(&self.info.network)
    }

    pub fn set_state(&mut self, state: ConnState) {
        self.info.state = state;
        self.events.push(Event::Network(self.info.clone()));
    }

    pub fn error(&mut self, error: impl Into<String>) {
        let network = self.id();
        self.events.push(Event::Error {
            network,
            error: error.into(),
        });
    }

    /// Shows the cached buffers and scrollback before any connection.
    pub fn load_cache(&mut self) {
        // First, so the view hides cached messages from blocked people.
        self.emit_blocks();
        for StoredBuffer {
            key,
            name,
            kind,
            topic,
            read,
            seen,
        } in self.store.buffers()
        {
            let b = Buffer {
                topic,
                read,
                seen,
                ..Buffer::new(&name, kind)
            };
            self.buffers.insert(key.clone(), b);
            self.emit_buffer(&key);
            let messages = self.store.recent(&key, STARTUP_SCROLLBACK);
            if !messages.is_empty() {
                self.emit_messages(&name, messages);
            }
        }
    }

    /// Registration finished (end of MOTD); `caps` and `isupport` are filled in.
    pub fn attach(&mut self, nick: &str) {
        nick.clone_into(&mut self.info.nick);
        self.cm = self.isupport.casemapping();
        self.connected = true;
        self.synced = false;
        self.joined.clear();
        self.who_wanted.clear();
        self.set_state(ConnState::Connected);
        self.persistent = !self.guest && self.caps.enabled("draft/persistence");
        if let Some(m) = &self.away {
            self.out.push(Message::new("AWAY", [m.as_str()]));
        }
        // Empty also clears one a crash left behind (always-on keeps metadata).
        if let Some(a) = self.activity.clone() {
            self.publish_activity(&a);
            self.publish_music();
        }
        if !self.guest && self.caps.enabled("draft/persistence") {
            self.out.push(Message::new("PERSISTENCE", ["GET"]));
        }
        self.push_registered = false;
        (self.push_tries, self.push_retry) = (0, None);
        let vapid = self.isupport.get("VAPID").filter(|v| is_base64url(v, 200));
        if let Some(vapid) = vapid.filter(|_| !self.guest && self.caps.enabled("draft/webpush")) {
            let (network, vapid) = (self.id(), vapid.to_owned());
            self.events.push(Event::PushAvailable { network, vapid });
        }
        // ergo sends the reattach burst (our JOINs) before reading this, so the reply
        // marks the point where the server's channel list is complete.
        self.send_labeled(Message::new("PING", ["sync"]), Pending::Sync);
    }

    /// The connection is gone. Unanswered sends fail.
    pub fn detach(&mut self) {
        self.connected = false;
        self.info.lag_ms = None;
        self.lag_out = false;
        self.synced = false;
        self.batches.clear();
        self.typing_sent.clear();
        self.read_sent.clear();
        self.out.clear();
        self.e2e_detach();
        self.listing = None;
        self.names.clear();
        let unlabeled: Vec<Pending> = self.echoes.drain(..).collect();
        for pending in self.labels.clear().into_iter().chain(unlabeled) {
            if let Pending::Send {
                target, mut local, ..
            } = pending
            {
                local.status = SendStatus::Failed("The connection dropped.".into());
                self.emit_messages(&target, vec![*local]);
            }
        }
        for b in self.buffers.values_mut() {
            b.members.clear();
            b.online = None;
        }
        // The call can't outlive the connection; `conn` hangs up.
        self.voice_request = None;
        self.voice_published = false;
        if self.voice.take().is_some() {
            self.emit_voice_state();
        }
        let channels: HashSet<String> = self.voice_of.drain().map(|(_, (_, c))| c).collect();
        channels.iter().for_each(|c| self.emit_voice_members(c));
    }

    /// Measures the round trip to the server. Needs labels to match the reply.
    pub fn ping_lag(&mut self) {
        if self.synced && self.caps.enabled("labeled-response") && !self.lag_out {
            self.lag_out = true;
            self.send_labeled(Message::new("PING", ["lag"]), Pending::Lag(Instant::now()));
        }
    }

    /// Back in front with the socket still up: it may have died silently while
    /// frozen, so catch up now; the sends also make the keepalive check in
    /// seconds rather than after its idle wait.
    pub fn resumed(&mut self) {
        if !self.synced {
            return;
        }
        let keys: Vec<String> = self.buffers.keys().cloned().collect();
        for key in &keys {
            self.fetch_new(key);
        }
    }

    /// What happened since last time, for the UI. A member list that changed
    /// many times (a netsplit, a big WHO) goes out once.
    pub fn drain_events(&mut self) -> std::vec::Drain<'_, Event> {
        for key in std::mem::take(&mut self.members_dirty) {
            self.send_members(&key);
        }
        self.events.drain(..)
    }

    pub fn handle(&mut self, msg: Message) -> Result<(), Overflow> {
        if let Some(item) = self.batches.feed(msg)? {
            match self.labels.take(item) {
                Ok((token, item)) => self.labeled(token, item),
                Err(item) => {
                    if let Some(item) = self.unlabeled(item) {
                        self.item(item);
                    }
                }
            }
        }
        Ok(())
    }

    // ---- helpers ----------------------------------------------------------

    fn fold(&self, s: &str) -> String {
        self.cm.fold(s)
    }

    fn is_channel(&self, s: &str) -> bool {
        self.isupport.is_channel(s)
    }

    fn send_labeled(&mut self, m: Message, token: Pending) {
        if self.caps.enabled("labeled-response") {
            let m = self.labels.tag(m, token);
            self.out.push(m);
            return;
        }
        // Classic networks (D72): match the reply by what it looks like instead.
        self.out.push(m);
        match token {
            Pending::Sync => self.reply(token, Vec::new()),
            Pending::Send { .. } if self.caps.enabled("echo-message") => {
                self.echoes.push_back(token);
            }
            Pending::Send { .. } => self.reply(token, Vec::new()),
            Pending::List | Pending::Browse => self.listing = Some((token, Vec::new())),
            // Callers gate these on caps no labels-less server has; WHO falls back to NAMES.
            Pending::Who { .. }
            | Pending::Lag(_)
            | Pending::History { .. }
            | Pending::Targets
            | Pending::Devices { .. }
            | Pending::Block { .. }
            | Pending::Report => {}
        }
    }

    /// Without `labeled-response`: the reply to an [`Net::send_labeled`], else
    /// `item` back for the usual handling.
    fn unlabeled(&mut self, item: Item) -> Option<Item> {
        let Item::Message(m) = &item else {
            return Some(item);
        };
        match m.command.as_str() {
            "321" | "322" if self.listing.is_some() => {
                let (_, items) = self.listing.as_mut().expect("checked");
                if items.len() < MAX_LISTED {
                    items.push(item);
                }
                None
            }
            "323" if self.listing.is_some() => {
                let (token, items) = self.listing.take().expect("checked");
                self.reply(token, items);
                None
            }
            "PRIVMSG" | "NOTICE"
                if m.source_nick()
                    .is_some_and(|n| self.cm.eq(n, &self.info.nick)) =>
            {
                let target = m.param(0).unwrap_or_default();
                let ours = matches!(
                    self.echoes.front(),
                    Some(Pending::Send { target: t, .. }) if self.cm.eq(t, target)
                );
                if !ours {
                    return Some(item);
                }
                let token = self.echoes.pop_front().expect("checked");
                self.reply(token, vec![item]);
                None
            }
            _ => Some(item),
        }
    }

    fn history(&mut self, key: &str, kind: Hist, reference: &str) {
        if !self.caps.enabled("draft/chathistory") {
            return;
        }
        let Some(name) = self.buffers.get(key).map(|b| b.name.clone()) else {
            return;
        };
        let page = PAGE
            .min(self.isupport.chathistory_limit().unwrap_or(PAGE).max(1))
            .to_string();
        let sub = match kind {
            Hist::Latest => "LATEST",
            Hist::Before => "BEFORE",
            Hist::After { .. } => "AFTER",
            Hist::Around => "AROUND",
        };
        let m = Message::new("CHATHISTORY", [sub, &name, reference, &page]);
        self.send_labeled(
            m,
            Pending::History {
                target: key.to_owned(),
                kind,
            },
        );
    }

    /// Whatever arrived since the newest cached message.
    fn fetch_new(&mut self, key: &str) {
        match self.store.newest(key) {
            Some((msgid, _)) => {
                self.history(key, Hist::After { pages: 1 }, &format!("msgid={msgid}"))
            }
            None => self.history(key, Hist::Latest, "*"),
        }
    }

    fn join_all(&mut self, channels: &[String]) {
        for chunk in pack(channels.iter().map(String::as_str), 400) {
            self.out.push(Message::new("JOIN", [chunk]));
        }
    }

    fn monitor(&mut self, nicks: &[&str]) {
        if !self.connected || self.isupport.get("MONITOR").is_none() {
            return;
        }
        for chunk in pack(nicks.iter().copied(), 400) {
            self.out
                .push(Message::new("MONITOR", ["+", chunk.as_str()]));
        }
    }

    fn add_buffer(&mut self, name: &str, kind: BufferKind) -> bool {
        if self.buffers.len() >= MAX_BUFFERS {
            return false;
        }
        let key = self.fold(name);
        self.store.put_buffer(&key, name, kind);
        self.buffers.insert(key.clone(), Buffer::new(name, kind));
        self.emit_buffer(&key);
        if kind == BufferKind::Dm {
            self.fetch_devices(name);
            self.fetch_receipts(&[name.to_owned()]);
        }
        true
    }

    fn remove_buffer(&mut self, key: &str) {
        if let Some(b) = self.buffers.remove(key) {
            self.store.remove_buffer(key);
            let network = self.id();
            self.events.push(Event::BufferRemoved {
                network,
                target: b.name,
            });
        }
    }

    fn set_read(&mut self, key: &str, read: Option<i64>) {
        if let Some(b) = self.buffers.get_mut(key) {
            b.read = read.max(b.read);
            if let Some(t) = b.read {
                self.store.set_read(key, t);
            }
            self.emit_buffer(key);
        }
    }

    /// Upserts a member of a channel we hold.
    fn member(&mut self, key: &str, nick: &str, f: impl FnOnce(&mut Member)) {
        let folded = self.fold(nick);
        let Some(b) = self.buffers.get_mut(key) else {
            return;
        };
        if b.members.len() >= MAX_MEMBERS && !b.members.contains_key(&folded) {
            return;
        }
        let m = b.members.entry(folded).or_insert_with(|| Member {
            nick: nick.to_owned(),
            account: None,
            away: false,
            oper: false,
            prefix: String::new(),
        });
        f(m);
        self.emit_members(key);
    }

    /// MONITOR only says online, so DMs take away from a shared channel's WHO.
    fn sync_dm_away(&mut self) {
        let changed: Vec<String> = self
            .buffers
            .iter()
            .filter(|(_, b)| b.kind == BufferKind::Dm && b.online == Some(true))
            .filter(|(k, b)| {
                self.buffers
                    .values()
                    .find_map(|c| c.members.get(*k))
                    .is_some_and(|m| m.away != b.away)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in changed {
            if let Some(b) = self.buffers.get_mut(&k) {
                b.away = !b.away;
            }
            self.emit_buffer(&k);
        }
    }

    fn everywhere(&mut self, nick: &str, f: impl Fn(&mut Member)) {
        let folded = self.fold(nick);
        let keys: Vec<String> = self
            .buffers
            .iter_mut()
            .filter_map(|(k, b)| b.members.get_mut(&folded).map(&f).map(|()| k.clone()))
            .collect();
        keys.iter().for_each(|k| self.emit_members(k));
    }

    /// Re-sends a stored message after a reaction or redaction.
    /// If `msg` is an edit (D51), applies it to the stored message it names when
    /// it has the same author, and returns true.
    fn edit(&mut self, key: &str, msg: &ChatMessage) -> bool {
        let Some(id) = &msg.edits else {
            return false;
        };
        let Some((buffer, orig)) = self.store.get(id) else {
            // shortcut: an edit of a message we haven't stored is dropped, so a page
            // holding only the edit misses it; store pending edits if that bites.
            return true;
        };
        let author = match (&orig.account, &msg.account) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.cm.eq(&orig.nick, &msg.nick),
            _ => false,
        };
        // The server can forge plain edits, so only an encrypted one changes an encrypted message.
        let crypt = match orig.crypt {
            Crypt::Encrypted => msg.crypt == Crypt::Encrypted,
            _ => msg.crypt == Crypt::Plain,
        };
        let text = msg.text.strip_prefix("* ").unwrap_or(&msg.text);
        if buffer == key
            && author
            && crypt
            && msg.status == SendStatus::Sent
            && orig.kind != MessageKind::Notice
            && self.store.edit(id, text, &msg.mentions, msg.time)
        {
            self.refresh(id);
        }
        true
    }

    fn refresh(&mut self, msgid: &str) {
        if let Some((key, msg)) = self.store.get(msgid)
            && let Some(name) = self.buffers.get(&key).map(|b| b.name.clone())
        {
            self.emit_messages(&name, vec![msg]);
            self.emit_buffer(&key);
        }
    }

    fn emit_messages(&mut self, target: &str, mut messages: Vec<ChatMessage>) {
        // Our own edits on their way out aren't messages; only their failures show.
        let mut failed = Vec::new();
        messages.retain(|m| match (&m.edits, &m.status) {
            (None, _) => true,
            (Some(_), SendStatus::Failed(why)) => {
                failed.push(format!("Your edit failed. {why}"));
                false
            }
            _ => false,
        });
        failed.into_iter().for_each(|e| self.error(e));
        // Cached before the block, or refreshed by someone else's reaction.
        messages.retain_mut(|m| crate::unblocked(&self.blocks, m));
        if messages.is_empty() {
            return;
        }
        let network = self.id();
        self.events.push(Event::Messages {
            network,
            target: target.to_owned(),
            messages,
        });
    }

    fn emit_buffer(&mut self, key: &str) {
        let Some(b) = self.buffers.get(key) else {
            return;
        };
        let (unread, mentions) = self.store.counts(key, b.read);
        let buffer = BufferInfo {
            target: b.name.clone(),
            kind: b.kind,
            topic: b.topic.clone(),
            unread,
            mentions,
            online: b.online,
            away: b.away,
            voice: b.kind == BufferKind::Channel && !b.voice_off && !self.classic(),
            encryption: self.encryption(key),
            seen: b.seen.filter(|_| self.info.receipts),
        };
        let network = self.id();
        self.events.push(Event::Buffer { network, buffer });
    }

    fn emit_members(&mut self, key: &str) {
        if self.buffers.contains_key(key) && !self.members_dirty.iter().any(|k| k == key) {
            self.members_dirty.push(key.to_owned());
        }
    }

    fn send_members(&mut self, key: &str) {
        let Some(b) = self.buffers.get(key) else {
            return;
        };
        let mut members: Vec<Member> = b.members.values().cloned().collect();
        // Highest PREFIX first (ISUPPORT order), unprefixed last, then by nick.
        let order = self.isupport.prefix();
        let rank = |m: &Member| {
            let p = m.prefix.chars().next();
            order
                .iter()
                .position(|(_, s)| Some(*s) == p)
                .unwrap_or(order.len())
        };
        members.sort_by_cached_key(|m| (rank(m), m.nick.to_lowercase()));
        let (network, channel) = (self.id(), b.name.clone());
        self.events.push(Event::Members {
            network,
            channel,
            members: members.into(),
        });
    }
}

fn is_base64url(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'=')
}

/// A plain-words error from a `FAIL` or error numeric, if that's what this is.
pub fn failure(item: &Item) -> Option<String> {
    let Item::Message(m) = item else { return None };
    let p = |i| m.param(i).unwrap_or_default();
    let numeric = m.command.parse::<u16>().ok();
    let text = match (m.command.as_str(), numeric) {
        ("FAIL", _) => match (p(0), p(1)) {
            ("REGISTER", "ACCOUNT_EXISTS" | "USERNAME_EXISTS") => "That username is taken.".into(),
            ("WEBPUSH", _) => format!(
                "Push notifications didn't turn on: {}",
                m.params.last().map_or("", |s| s.as_str())
            ),
            (_, "RATE_LIMITED") => "Slow down a little and try again.".into(),
            _ => m.params.last().cloned().unwrap_or_default(),
        },
        (_, Some(401)) => format!("Nobody called {} is around.", p(1)),
        (_, Some(403)) => format!("{} doesn't exist.", p(1)),
        (_, Some(404)) => format!("You can't send messages to {} right now.", p(1)),
        (_, Some(417)) => "That message is too long.".into(),
        (_, Some(433)) => "That username is taken.".into(),
        (_, Some(471)) => format!("{} is full.", p(1)),
        (_, Some(473)) => format!("{} is invite-only.", p(1)),
        (_, Some(474)) => format!("You're banned from {}.", p(1)),
        (_, Some(475)) => format!("{} needs a password.", p(1)),
        (_, Some(482)) => "Only channel operators can do that.".into(),
        // 9xx is SASL, handled during registration.
        (_, Some(400..=599)) => m.params.last().cloned().unwrap_or_default(),
        _ => return None,
    };
    Some(truncate(&text, 400).to_owned())
}

/// The message whose tags speak for an item: a multiline batch's start.
fn head(item: &Item) -> &Message {
    match item {
        Item::Message(m) => m,
        Item::Batch(b) => &b.start,
    }
}

fn flatten(items: Vec<Item>) -> impl Iterator<Item = Message> {
    items.into_iter().flat_map(|i| match i {
        Item::Message(m) => vec![m],
        Item::Batch(b) => flatten(b.items).collect(),
    })
}

/// Nicks and channel names we're willing to hold and send back.
fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME
        && !s.starts_with(':')
        && !s.contains(|c: char| c == ' ' || c == ',' || c.is_control())
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Lines of `text` cut to fit one PRIVMSG each; `true` marks a piece that
/// continues the previous one (`draft/multiline-concat`).
fn split(text: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    for line in text.split('\n').map(|l| l.trim_end_matches('\r')) {
        let mut rest = line;
        let mut concat = false;
        loop {
            let piece = truncate(rest, MAX_PIECE);
            out.push((piece, concat));
            rest = &rest[piece.len()..];
            concat = true;
            if rest.is_empty() {
                break;
            }
        }
    }
    out
}

/// Comma-joins names into lists no longer than `max` bytes.
fn pack<'a>(names: impl Iterator<Item = &'a str>, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        match out.last_mut() {
            Some(l) if l.len() + 1 + n.len() <= max => {
                l.push(',');
                l.push_str(n);
            }
            _ => out.push(n.to_owned()),
        }
    }
    out
}

/// A TAGMSG's reaction emoji, and whether it adds (`react`) or removes (`unreact`).
fn reaction(m: &Message) -> Option<(&str, bool)> {
    (m.tag("+draft/react").map(|e| (e, true)))
        .or_else(|| m.tag("+draft/unreact").map(|e| (e, false)))
}

impl Net {
    /// `text` names us: our account in its `+chad/mentions`, or our nick or
    /// display name as a whole word.
    fn mentioned(&self, text: &str, tagged: &[Mention]) -> bool {
        let display_name = (self.profiles.get(&self.fold(&self.info.nick)))
            .and_then(|p| p.display_name.as_deref());
        (self.account.as_deref()).is_some_and(|a| tagged.iter().any(|m| self.cm.eq(&m.account, a)))
            || mentions(text, &self.info.nick)
            || display_name.is_some_and(|d| mentions(text, d))
    }
}

/// `nick` (or a display name) as a whole word, ignoring ASCII case.
fn mentions(text: &str, nick: &str) -> bool {
    mention::find(text, nick).next().is_some()
}

#[cfg(test)]
mod tests;
