//! One network's state after registration, sans-IO: feed it server lines and UI
//! commands, then drain `out` (lines to send) and `events` (for the UI).
//! `conn` does the I/O, registration and reconnects around it.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chad_irc::Message;
use chad_irc::batch::{Assembler, Batch, Item, Overflow};
use chad_irc::cap::{Negotiator, WANTED};
use chad_irc::isupport::{Casemapping, Isupport};
use chad_irc::label::Labels;

use crate::store::{Store, StoredBuffer};
use crate::{
    BufferInfo, BufferKind, ChatMessage, ConnState, Crypt, Event, Member, MessageKind, NetCommand,
    NetworkInfo, SendStatus, Typing, VoiceState, time,
};

mod e2e;
pub use e2e::{DEVICES_KEY, E2e};
use url::Url;

/// Messages per CHATHISTORY request (ergo allows 1000).
const PAGE: u32 = 100;
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
/// Metadata key for voice: a user's room (`#chan`), or `off` on a channel.
pub const VOICE_KEY: &str = "chad/voice";
/// De-facto metadata keys other clients read too.
pub const DISPLAY_NAME_KEY: &str = "display-name";
pub const AVATAR_KEY: &str = "avatar";
const MAX_DISPLAY_NAME: usize = 64;

#[derive(Debug)]
enum Pending {
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
    Who {
        channel: String,
    },
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
    /// No account, so never always-on: rejoin our channels ourselves.
    pub guest: bool,
    pub out: Vec<Message>,
    pub events: Vec<Event>,
    cm: Casemapping,
    batches: Assembler,
    labels: Labels<Pending>,
    buffers: HashMap<String, Buffer>,
    next_local: u64,
    next_batch: u64,
    typing_sent: HashMap<String, Instant>,
    connected: bool,
    synced: bool,
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
    /// Our away message, sent again after every reconnect.
    away: Option<String>,
    /// Encrypted DMs; `None` for guests, or without a keyring.
    pub e2e: Option<E2e>,
    /// Our CTCP VERSION reply.
    pub version: String,
    ctcp_at: Option<Instant>,
}

#[derive(Debug, Default, Clone)]
struct Profile {
    display_name: Option<String>,
    avatar: Option<Url>,
}

impl Net {
    pub fn new(store: Store, info: NetworkInfo) -> Self {
        Self {
            store,
            info,
            caps: Negotiator::new(),
            isupport: Isupport::new(),
            invite_channel: None,
            guest: false,
            out: Vec::new(),
            events: Vec::new(),
            cm: Casemapping::Ascii,
            batches: Assembler::new(),
            labels: Labels::new(),
            buffers: HashMap::new(),
            next_local: 0,
            next_batch: 0,
            typing_sent: HashMap::new(),
            connected: false,
            synced: false,
            persistent: true,
            joined: HashSet::new(),
            who_wanted: HashSet::new(),
            voice: None,
            voice_request: None,
            voice_published: false,
            voice_of: HashMap::new(),
            profiles: HashMap::new(),
            away: None,
            e2e: None,
            version: "IrChad".into(),
            ctcp_at: None,
        }
    }

    fn id(&self) -> String {
        self.info.network.clone()
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
        for StoredBuffer {
            key,
            name,
            kind,
            topic,
            read,
        } in self.store.buffers()
        {
            self.buffers.insert(
                key.clone(),
                Buffer {
                    name: name.clone(),
                    kind,
                    topic,
                    read,
                    members: HashMap::new(),
                    online: None,
                    away: false,
                    voice_off: false,
                },
            );
            self.emit_buffer(&key);
            let messages = self.store.recent(&key, STARTUP_SCROLLBACK);
            if !messages.is_empty() {
                self.emit_messages(&name, messages);
            }
        }
    }

    /// Registration finished (end of MOTD); `caps` and `isupport` are filled in.
    pub fn attach(&mut self, nick: &str) {
        self.info.nick = nick.to_owned();
        self.cm = self.isupport.casemapping();
        self.connected = true;
        self.synced = false;
        self.joined.clear();
        self.who_wanted.clear();
        self.set_state(ConnState::Connected);
        self.persistent = !self.guest;
        if let Some(m) = &self.away {
            self.out.push(Message::new("AWAY", [m.as_str()]));
        }
        if !self.guest && self.caps.enabled("draft/persistence") {
            self.out.push(Message::new("PERSISTENCE", ["GET"]));
        }
        // ergo sends the reattach burst (our JOINs) before reading this, so the reply
        // marks the point where the server's channel list is complete.
        self.send_labeled(Message::new("PING", ["sync"]), Pending::Sync);
    }

    /// The connection is gone. Unanswered sends fail.
    pub fn detach(&mut self) {
        self.connected = false;
        self.synced = false;
        self.batches.clear();
        self.typing_sent.clear();
        self.out.clear();
        self.e2e_detach();
        for pending in self.labels.clear() {
            if let Pending::Send {
                target, mut local, ..
            } = pending
            {
                local.status = SendStatus::Failed("Not sent: the connection dropped.".into());
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

    pub fn handle(&mut self, msg: Message) -> Result<(), Overflow> {
        if let Some(item) = self.batches.feed(msg)? {
            match self.labels.take(item) {
                Ok((token, item)) => self.labeled(token, item),
                Err(item) => self.item(item),
            }
        }
        Ok(())
    }

    // ---- commands -------------------------------------------------------

    pub fn command(&mut self, cmd: NetCommand) {
        let offline = !self.connected;
        match cmd {
            NetCommand::Send {
                target,
                text,
                reply_to,
            } => self.send_text(&target, &text, reply_to),
            NetCommand::MarkRead { target } => self.mark_read(&target),
            NetCommand::OpenDm { nick } if valid_name(&nick) && !self.is_channel(&nick) => {
                let key = self.fold(&nick);
                if !self.buffers.contains_key(&key) && self.add_buffer(&nick, BufferKind::Dm) {
                    self.monitor(&[nick.as_str()]);
                    self.fetch_new(&key);
                }
            }
            NetCommand::CloseDm { nick } => {
                let key = self.fold(&nick);
                if self
                    .buffers
                    .get(&key)
                    .is_some_and(|b| b.kind == BufferKind::Dm)
                {
                    self.remove_buffer(&key);
                    if self.connected && self.isupport.get("MONITOR").is_some() {
                        self.out.push(Message::new("MONITOR", ["-", nick.as_str()]));
                    }
                }
            }
            NetCommand::Verify { .. } | NetCommand::SignIn { .. } => {}
            // `conn` takes it while a guest is connected.
            NetCommand::Register { .. } => {
                let error = if self.guest {
                    "Wait until you're connected to sign up."
                } else {
                    "You already have an account here."
                };
                self.events.push(Event::SignUpFailed {
                    network: self.id(),
                    error: error.into(),
                });
            }
            NetCommand::LeaveVoice => self.voice_ended(None),
            NetCommand::SetAway { message } => {
                let message = message.map(|m| truncate(m.trim(), 200).to_owned());
                // AWAY with an empty reason would mean "back".
                let message = message.map(|m| if m.is_empty() { "Away".into() } else { m });
                if self.away != message && self.connected {
                    let mut m = Message::new("AWAY", [""; 0]);
                    m.params.extend(message.clone());
                    self.out.push(m);
                }
                self.away = message;
            }
            // `conn` runs these while connected; offline they fail at once.
            NetCommand::Upload { target, .. } => {
                let network = self.id();
                self.events.push(Event::Uploaded {
                    network,
                    target,
                    result: Err("You're offline right now.".into()),
                });
            }
            NetCommand::Preview { url } => {
                let network = self.id();
                self.events.push(Event::Preview {
                    network,
                    url,
                    preview: None,
                });
            }
            NetCommand::Media { url } => {
                let network = self.id();
                self.events.push(Event::Media {
                    network,
                    url,
                    result: Err("You're offline right now.".into()),
                });
            }
            NetCommand::Gifs { query, pos } => {
                let network = self.id();
                self.events.push(Event::Gifs {
                    network,
                    query,
                    pos,
                    result: Err("You're offline right now.".into()),
                });
            }
            _ if offline => self.error("You're offline right now."),
            NetCommand::SetDisplayName { name } => {
                let name: String = name
                    .trim()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(MAX_DISPLAY_NAME)
                    .collect();
                self.set_metadata(DISPLAY_NAME_KEY, &name);
            }
            NetCommand::SetAvatar { url } => {
                let ok = url.is_empty()
                    || (url.len() <= 300
                        && Url::parse(&url).is_ok_and(|u| matches!(u.scheme(), "https" | "http")));
                if ok {
                    self.set_metadata(AVATAR_KEY, &url);
                } else {
                    self.error("That isn't an image link.");
                }
            }
            NetCommand::JoinVoice { channel } => self.join_voice(&channel),
            NetCommand::React {
                target,
                msgid,
                emoji,
            } => self.out.push(
                Message::new("TAGMSG", [target])
                    .with_tag("+draft/react", truncate(&emoji, 64))
                    .with_tag("+draft/reply", msgid),
            ),
            NetCommand::Unreact {
                target,
                msgid,
                emoji,
            } => self.out.push(
                Message::new("TAGMSG", [target])
                    .with_tag("+draft/unreact", truncate(&emoji, 64))
                    .with_tag("+draft/reply", msgid),
            ),
            NetCommand::Redact { target, msgid } => {
                self.out.push(Message::new("REDACT", [target, msgid]));
            }
            NetCommand::Typing { target, state } => self.typing(target, state),
            NetCommand::LoadOlder { target } => {
                let key = self.fold(&target);
                match self.store.oldest(&key) {
                    Some((msgid, _)) => self.history(&key, Hist::Before, &format!("msgid={msgid}")),
                    None => self.history(&key, Hist::Latest, "*"),
                }
            }
            NetCommand::LoadAround { target, msgid } => {
                let key = self.fold(&target);
                self.history(&key, Hist::Around, &format!("msgid={msgid}"));
            }
            NetCommand::Members { channel } => {
                let key = self.fold(&channel);
                if self.joined.contains(&key) {
                    self.who(channel);
                } else if self.who_wanted.len() < MAX_BUFFERS {
                    self.who_wanted.insert(key);
                }
            }
            NetCommand::Devices { nick } => self.fetch_devices(&nick),
            NetCommand::Verification { nick, step } => self.verification(&nick, step),
            NetCommand::AcceptKeys { nick } => {
                let key = self.fold(&nick);
                self.store.trust(&key, None);
                self.emit_buffer(&key);
            }
            NetCommand::JoinChannel { channel } => self.out.push(Message::new("JOIN", [channel])),
            NetCommand::Part { channel } => self.out.push(Message::new("PART", [channel])),
            NetCommand::OpenDm { .. } => {}
        }
    }

    fn who(&mut self, channel: String) {
        let m = Message::new("WHO", [channel.as_str(), "%tcnfa,1"]);
        self.send_labeled(m, Pending::Who { channel });
    }

    fn send_text(&mut self, target: &str, text: &str, reply_to: Option<String>) {
        let text = text.trim_end_matches(['\n', '\r', ' ']);
        if text.trim().is_empty() {
            return;
        }
        // An encrypted message has to fit in one tag; longer ones go as several.
        if text.len() > chad_e2ee::MAX_PLAINTEXT
            && !self.is_channel(target)
            && self.may_encrypt(target)
        {
            let piece = truncate(text, chad_e2ee::MAX_PLAINTEXT);
            let cut = piece.rfind('\n').filter(|&i| i > 0).unwrap_or(piece.len());
            self.send_text(target, &text[..cut], reply_to.clone());
            return self.send_text(target, &text[cut..], reply_to);
        }
        self.next_local += 1;
        let local = ChatMessage {
            msgid: None,
            local_id: Some(self.next_local),
            time: time::now_ms(),
            nick: self.info.nick.clone(),
            account: None,
            kind: MessageKind::Privmsg,
            text: text.to_owned(),
            reply_to: reply_to.clone(),
            status: SendStatus::Pending,
            redacted: false,
            reactions: Vec::new(),
            mention: false,
            own: true,
            crypt: Crypt::Plain,
        };
        if self.connected && !self.is_channel(target) && self.e2e.is_some() {
            self.send_dm(target, local);
        } else {
            self.send_plain(target, local);
        }
    }

    fn send_plain(&mut self, target: &str, mut local: ChatMessage) {
        let (text, reply_to) = (local.text.clone(), local.reply_to.clone());
        let pieces = split(&text);
        let limits = self.caps.enabled("draft/multiline").then(|| {
            let v = self.caps.value("draft/multiline").unwrap_or_default();
            let get = |k: &str| {
                v.split(',')
                    .find_map(|kv| kv.strip_prefix(k)?.strip_prefix('=')?.parse::<usize>().ok())
            };
            (
                get("max-bytes").unwrap_or(4096),
                get("max-lines").unwrap_or(100),
            )
        });
        let fail = match limits {
            _ if !self.connected => Some("Not sent: you're offline."),
            Some((bytes, lines))
                if pieces.len() > lines
                    || pieces.iter().map(|p| p.0.len()).sum::<usize>() > bytes =>
            {
                Some("That message is too long.")
            }
            _ => None,
        };
        if let Some(why) = fail {
            local.status = SendStatus::Failed(why.into());
            self.emit_messages(target, vec![local.clone()]);
            return;
        }
        self.emit_messages(target, vec![local.clone()]);

        let token = Pending::Send {
            target: target.to_owned(),
            local: Box::new(local),
            rest: Vec::new(),
        };
        let reply = |m: Message| match &reply_to {
            Some(id) => m.with_tag("+draft/reply", id.as_str()),
            None => m,
        };
        if pieces.len() == 1 {
            let m = reply(Message::new("PRIVMSG", [target, pieces[0].0]));
            self.send_labeled(m, token);
        } else if limits.is_some() {
            self.next_batch += 1;
            let id = format!("ml{}", self.next_batch);
            let start = reply(Message::new(
                "BATCH",
                [format!("+{id}").as_str(), "draft/multiline", target],
            ));
            self.send_labeled(start, token);
            for (piece, concat) in pieces {
                let mut m = Message::new("PRIVMSG", [target, piece]).with_tag("batch", id.as_str());
                if concat {
                    m = m.with_tag("draft/multiline-concat", "");
                }
                self.out.push(m);
            }
            self.out.push(Message::new("BATCH", [format!("-{id}")]));
        } else {
            // shortcut: without draft/multiline (never on ergo) each piece is its own
            // message and only the first replaces the pending one.
            let mut pieces = pieces.into_iter();
            if let Some((first, _)) = pieces.next() {
                self.send_labeled(reply(Message::new("PRIVMSG", [target, first])), token);
            }
            for (p, _) in pieces {
                self.out.push(Message::new("PRIVMSG", [target, p]));
            }
        }
    }

    fn typing(&mut self, target: String, state: Typing) {
        let key = self.fold(&target);
        let send = match state {
            Typing::Active => self
                .typing_sent
                .get(&key)
                .is_none_or(|t| t.elapsed() >= TYPING_EVERY),
            _ => self.typing_sent.remove(&key).is_some(),
        };
        if state == Typing::Active && send {
            self.typing_sent.insert(key, Instant::now());
        }
        if send {
            self.out
                .push(Message::new("TAGMSG", [target]).with_tag("+typing", state.as_str()));
        }
    }

    fn mark_read(&mut self, target: &str) {
        let key = self.fold(target);
        let Some((_, t)) = self.store.newest(&key) else {
            return;
        };
        if self.buffers.get(&key).and_then(|b| b.read) >= Some(t) {
            return;
        }
        self.set_read(&key, Some(t));
        if self.connected && self.caps.enabled("draft/read-marker") {
            let ts = format!("timestamp={}", time::format(t));
            self.out
                .push(Message::new("MARKREAD", [target, ts.as_str()]));
        }
    }

    // ---- server input ---------------------------------------------------

    fn item(&mut self, item: Item) {
        match item {
            Item::Message(m) => self.message(m),
            Item::Batch(b) if b.kind() == "draft/multiline" => {
                if let Some((target, msg)) = self.chat_in(&Item::Batch(b)) {
                    self.live(&target, msg);
                }
            }
            // Unrequested history (no labels): keep it, but it says nothing new.
            Item::Batch(b) if b.kind() == "chathistory" => {
                if let Some(t) = b.params().first().filter(|t| valid_name(t)) {
                    let key = self.fold(t);
                    if self.buffers.contains_key(&key) {
                        self.store_history(&key, b.items);
                    }
                }
            }
            Item::Batch(b) => b.items.into_iter().for_each(|i| self.item(i)),
        }
    }

    fn labeled(&mut self, token: Pending, item: Item) {
        let items = match item {
            Item::Batch(b) if b.kind() == "labeled-response" => b.items,
            other => vec![other],
        };
        match token {
            Pending::Send {
                target,
                mut local,
                rest,
            } => {
                let echo = items.iter().find_map(|i| self.chat(i));
                if echo.is_some() || items.iter().all(|i| failure(i).is_none()) {
                    // The other devices' copies share the first one's msgid.
                    let id = echo.as_ref().and_then(|(_, m)| m.msgid.clone());
                    for m in rest {
                        self.out.push(match &id {
                            Some(id) => m.with_tag("+chad/e2e-id", id.as_str()),
                            None => m,
                        });
                    }
                }
                match (echo, items.iter().find_map(failure)) {
                    (Some((_, mut msg)), _) => {
                        msg.local_id = local.local_id;
                        if local.crypt == Crypt::Encrypted {
                            // The echo is the ciphertext for another device.
                            msg.text = local.text.clone();
                            msg.crypt = Crypt::Encrypted;
                        }
                        let key = self.fold(&target);
                        self.store.insert(&key, &msg);
                        self.emit_messages(&target, vec![msg]);
                    }
                    (None, Some(error)) => {
                        local.status = SendStatus::Failed(error);
                        self.emit_messages(&target, vec![*local]);
                    }
                    (None, None) => {
                        local.status = SendStatus::Sent;
                        self.emit_messages(&target, vec![*local]);
                    }
                }
            }
            Pending::History { target, kind } => {
                let inner = match items.into_iter().next() {
                    Some(Item::Batch(b)) if b.kind() == "chathistory" => b.items,
                    Some(Item::Message(m)) => {
                        if let Some(e) = failure(&Item::Message(m)) {
                            tracing::warn!("history for {target}: {e}");
                        }
                        Vec::new()
                    }
                    _ => Vec::new(),
                };
                let n = self.store_history(&target, inner);
                let name = self.buffers.get(&target).map(|b| b.name.clone());
                match (kind, name) {
                    (Hist::Before, Some(name)) if n < PAGE as usize => {
                        let network = self.id();
                        self.events.push(Event::HistoryStart {
                            network,
                            target: name,
                        });
                    }
                    (Hist::After { pages }, _) if n >= PAGE as usize && pages < MAX_GAP_PAGES => {
                        if let Some((msgid, _)) = self.store.newest(&target) {
                            let r = format!("msgid={msgid}");
                            self.history(&target, Hist::After { pages: pages + 1 }, &r);
                        }
                    }
                    _ => {}
                }
            }
            Pending::Targets => {
                for m in flatten(items) {
                    let Some(t) = m
                        .param(1)
                        .filter(|t| m.param(0) == Some("TARGETS") && valid_name(t))
                    else {
                        continue;
                    };
                    if self.is_channel(t) || m.command != "CHATHISTORY" {
                        continue;
                    }
                    let key = self.fold(t);
                    if !self.buffers.contains_key(&key) && self.add_buffer(t, BufferKind::Dm) {
                        self.monitor(&[t]);
                        self.fetch_new(&key);
                    }
                }
            }
            Pending::Devices { nick } => self.devices_reply(&nick, flatten(items)),
            Pending::Sync => self.synced(),
            Pending::List => {
                let channels: Vec<String> = flatten(items)
                    .filter(|m| m.command == "322")
                    .filter_map(|m| m.params.into_iter().nth(1))
                    .filter(|c| valid_name(c) && self.is_channel(c))
                    .take(MAX_BUFFERS)
                    .collect();
                self.join_all(&channels);
            }
            Pending::Who { channel } => {
                let key = self.fold(&channel);
                let prefixes: String = self.isupport.prefix().iter().map(|(_, s)| *s).collect();
                let mut members = HashMap::new();
                // 354 <me> <token> <channel> <nick> <flags> <account>
                for m in flatten(items).filter(|m| m.command == "354" && m.param(1) == Some("1")) {
                    let (Some(nick), Some(flags)) = (m.param(3), m.param(4)) else {
                        continue;
                    };
                    if members.len() >= MAX_MEMBERS || !valid_name(nick) {
                        continue;
                    }
                    let member = Member {
                        nick: nick.to_owned(),
                        account: m.param(5).filter(|a| *a != "0").map(str::to_owned),
                        away: flags.starts_with('G'),
                        // multi-prefix sends them all; PREFIX order is highest first.
                        prefix: prefixes
                            .chars()
                            .find(|p| flags.contains(*p))
                            .map(String::from)
                            .unwrap_or_default(),
                    };
                    members.insert(self.fold(nick), member);
                }
                if let Some(b) = self.buffers.get_mut(&key) {
                    b.members = members;
                    self.emit_members(&key);
                }
                self.sync_dm_away();
            }
        }
    }

    fn synced(&mut self) {
        self.synced = true;
        let channels: Vec<(String, String)> = self
            .buffers
            .iter()
            .filter(|(_, b)| b.kind == BufferKind::Channel)
            .map(|(k, b)| (k.clone(), b.name.clone()))
            .collect();
        let missing: Vec<_> = channels
            .iter()
            .filter(|(k, _)| !self.joined.contains(k))
            .collect();
        // Just signed up as a guest: the new account is in no channels yet.
        let rejoin = self.store.meta("rejoin").is_some();
        self.store.delete_meta("rejoin");
        if self.persistent && !rejoin {
            // Always-on: the server's list is the truth, so these were left elsewhere.
            for (key, _) in missing {
                self.remove_buffer(&key.clone());
            }
        } else {
            let names: Vec<String> = missing.into_iter().map(|(_, n)| n.clone()).collect();
            self.join_all(&names);
        }
        let first = self.store.meta("autojoined").is_none();
        if first && self.joined.is_empty() {
            self.send_labeled(Message::new("LIST", [""; 0]), Pending::List);
        }
        self.store.set_meta("autojoined", "1");
        if let Some(c) = self.invite_channel.take()
            && !self.joined.contains(&self.fold(&c))
        {
            self.out.push(Message::new("JOIN", [c]));
        }

        let keys: Vec<String> = self.buffers.keys().cloned().collect();
        for key in &keys {
            self.fetch_new(key);
        }
        if let Some(t) = self.store.latest_time()
            && self.caps.enabled("draft/chathistory")
        {
            let from = format!("timestamp={}", time::format(t));
            let to = format!("timestamp={}", time::format(time::now_ms() + 86_400_000));
            let m = Message::new("CHATHISTORY", ["TARGETS", &from, &to, "50"]);
            self.send_labeled(m, Pending::Targets);
        }
        let dms: Vec<String> = self
            .buffers
            .values()
            .filter(|b| b.kind == BufferKind::Dm)
            .map(|b| b.name.clone())
            .collect();
        self.monitor(&dms.iter().map(String::as_str).collect::<Vec<_>>());
        self.e2e_synced(&dms);
    }

    fn message(&mut self, m: Message) {
        let from = m.source_nick().unwrap_or_default().to_owned();
        let me = self.cm.eq(&from, &self.info.nick);
        match m.command.as_str() {
            "PRIVMSG" if !me && m.param(1) == Some("\x01VERSION\x01") => {
                self.ctcp_version(&m, &from);
            }
            "PRIVMSG" | "NOTICE" => {
                if let Some((target, msg)) = self.chat_in(&Item::Message(m)) {
                    self.live(&target, msg);
                }
            }
            "TAGMSG" => {
                let Some(target) = self.buffer_target(&m) else {
                    return;
                };
                if let Some(step) = m.tag("+chad/e2e-verify") {
                    self.verify_tag(&target, me, step);
                } else if let (Some((emoji, add)), Some(msgid)) =
                    (reaction(&m), m.tag("+draft/reply"))
                {
                    self.store.react(msgid, &from, truncate(emoji, 64), add);
                    self.refresh(msgid);
                } else if let Some(state) = m.tag("+typing").filter(|_| !me) {
                    let state = match state {
                        "active" => Typing::Active,
                        "paused" => Typing::Paused,
                        _ => Typing::Done,
                    };
                    let network = self.id();
                    self.events.push(Event::Typing {
                        network,
                        target,
                        nick: from,
                        state,
                    });
                }
            }
            "REDACT" => {
                if let Some(msgid) = m.param(1) {
                    self.store.redact(msgid);
                    self.refresh(msgid);
                }
            }
            "JOIN" => {
                let Some(chan) = m.param(0).filter(|c| valid_name(c) && self.is_channel(c)) else {
                    return;
                };
                let key = self.fold(chan);
                if me {
                    let new = !self.buffers.contains_key(&key);
                    if self.who_wanted.remove(&key) {
                        self.who(chan.to_owned());
                    }
                    if (!new || self.add_buffer(chan, BufferKind::Channel))
                        && self.joined.insert(key.clone())
                        && self.synced
                    {
                        self.fetch_new(&key);
                        // Who's in voice there; the connect burst covers the rest.
                        if self.caps.enabled("draft/metadata-2") {
                            self.out.push(Message::new("METADATA", [chan, "SYNC"]));
                        }
                    }
                } else {
                    let account = m.param(1).filter(|a| *a != "*").map(str::to_owned);
                    self.member(&key, &from, |mem| mem.account = account);
                    // Metadata isn't pushed for people who join after us.
                    let folded = self.fold(&from);
                    if self.caps.enabled("draft/metadata-2")
                        && !self.profiles.contains_key(&folded)
                        && self.profiles.len() < MAX_MEMBERS
                    {
                        self.profiles.insert(folded, Profile::default());
                        self.out.push(Message::new(
                            "METADATA",
                            [from.as_str(), "GET", DISPLAY_NAME_KEY, AVATAR_KEY],
                        ));
                    }
                }
            }
            "PART" | "KICK" => {
                let (Some(chan), Some(who)) = (
                    m.param(0),
                    if m.command == "KICK" {
                        m.param(1)
                    } else {
                        Some(from.as_str())
                    },
                ) else {
                    return;
                };
                let key = self.fold(chan);
                if self.cm.eq(who, &self.info.nick) {
                    self.joined.remove(&key);
                    self.remove_buffer(&key);
                } else {
                    let nick = self.fold(who);
                    if let Some(b) = self.buffers.get_mut(&key)
                        && b.members.remove(&nick).is_some()
                    {
                        self.emit_members(&key);
                    }
                }
            }
            "MODE" => {
                let (Some(chan), Some(modes)) = (m.param(0), m.param(1)) else {
                    return;
                };
                let key = self.fold(chan);
                // Members keep only their highest prefix, so a WHO is simpler than replaying the change.
                if self.joined.contains(&key)
                    && self
                        .buffers
                        .get(&key)
                        .is_some_and(|b| !b.members.is_empty())
                    && self
                        .isupport
                        .prefix()
                        .iter()
                        .any(|(c, _)| modes.contains(*c))
                {
                    self.who(chan.to_owned());
                }
            }
            "RENAME" => {
                let (Some(old), Some(new)) = (
                    m.param(0),
                    m.param(1).filter(|c| valid_name(c) && self.is_channel(c)),
                ) else {
                    return;
                };
                let (ok, nk) = (self.fold(old), self.fold(new));
                // ergo never renames onto an existing channel; don't merge two buffers.
                if ok != nk && self.buffers.contains_key(&nk) {
                    return;
                }
                let Some(mut b) = self.buffers.remove(&ok) else {
                    return;
                };
                let from = std::mem::replace(&mut b.name, new.to_owned());
                self.buffers.insert(nk.clone(), b);
                self.store.rename_buffer(&ok, &nk, new);
                if self.joined.remove(&ok) {
                    self.joined.insert(nk.clone());
                }
                if self.who_wanted.remove(&ok) {
                    self.who_wanted.insert(nk.clone());
                }
                self.typing_sent.remove(&ok);
                // Their calls are in the old room; they republish `chad/voice` when they rejoin.
                self.voice_of.retain(|_, (_, c)| *c != ok);
                let network = self.id();
                self.events.push(Event::BufferRenamed {
                    network,
                    from: from.clone(),
                    to: new.to_owned(),
                });
                self.emit_buffer(&nk);
                self.emit_members(&nk);
                self.emit_voice_members(&nk);
                // The LiveKit room is named after the channel.
                if self
                    .voice
                    .as_ref()
                    .is_some_and(|v| self.fold(&v.channel) == ok)
                {
                    self.voice_ended(Some(format!(
                        "{from} was renamed to {new}. Join voice again."
                    )));
                }
            }
            "QUIT" => {
                self.set_voice_of(&from, None);
                let nick = self.fold(&from);
                let keys: Vec<String> = self
                    .buffers
                    .iter_mut()
                    .filter_map(|(k, b)| b.members.remove(&nick).map(|_| k.clone()))
                    .collect();
                keys.iter().for_each(|k| self.emit_members(k));
            }
            "NICK" => {
                let Some(new) = m.param(0).filter(|n| valid_name(n)) else {
                    return;
                };
                if me {
                    self.info.nick = new.to_owned();
                    self.events.push(Event::Network(self.info.clone()));
                }
                let (old, newk) = (self.fold(&from), self.fold(new));
                if let Some(p) = self.profiles.remove(&old) {
                    self.profiles.insert(newk.clone(), p);
                    self.emit_profile(new);
                }
                if let Some((_, ch)) = self.voice_of.remove(&old) {
                    self.voice_of
                        .insert(newk.clone(), (new.to_owned(), ch.clone()));
                    self.emit_voice_members(&ch);
                }
                let keys: Vec<String> = self
                    .buffers
                    .iter_mut()
                    .filter_map(|(k, b)| {
                        let mut mem = b.members.remove(&old)?;
                        mem.nick = new.to_owned();
                        b.members.insert(newk.clone(), mem);
                        Some(k.clone())
                    })
                    .collect();
                keys.iter().for_each(|k| self.emit_members(k));
            }
            "AWAY" => {
                let away = m.param(0).is_some_and(|r| !r.is_empty());
                self.everywhere(&from, |mem| mem.away = away);
                let key = self.fold(&from);
                if let Some(b) = self.buffers.get_mut(&key)
                    && b.kind == BufferKind::Dm
                {
                    b.away = away;
                    self.emit_buffer(&key);
                }
            }
            "ACCOUNT" => {
                let account = m.param(0).filter(|a| *a != "*").map(str::to_owned);
                self.everywhere(&from, |mem| mem.account = account.clone());
            }
            "TOPIC" | "332" => {
                let skip = usize::from(m.command == "332");
                let (Some(chan), Some(topic)) = (m.param(skip), m.param(skip + 1)) else {
                    return;
                };
                let key = self.fold(chan);
                let topic = truncate(topic, 1024).to_owned();
                if let Some(b) = self.buffers.get_mut(&key) {
                    self.store.set_topic(&key, &topic);
                    b.topic = topic;
                    self.emit_buffer(&key);
                }
            }
            "MARKREAD" => {
                let (Some(target), Some(ts)) = (m.param(0), m.param(1)) else {
                    return;
                };
                let key = self.fold(target);
                let read = ts.strip_prefix("timestamp=").and_then(time::parse);
                if read.is_some() && self.buffers.contains_key(&key) {
                    self.set_read(&key, read);
                }
            }
            "730" | "731" => {
                let online = m.command == "730";
                for nick in m.param(1).unwrap_or_default().split(',') {
                    let key = self.fold(nick.split('!').next().unwrap_or(nick));
                    if let Some(b) = self.buffers.get_mut(&key)
                        && b.kind == BufferKind::Dm
                    {
                        b.online = Some(online);
                        b.away &= online;
                        self.emit_buffer(&key);
                    }
                }
                self.sync_dm_away();
            }
            // Someone's devices changed: fetch them again if we DM them (or it's us).
            "761" | "766" if m.param(2) == Some(DEVICES_KEY) => {
                if let Some(nick) = m.param(1).filter(|n| valid_name(n) && !self.is_channel(n)) {
                    let key = self.fold(nick);
                    if self.wants_devices(&key) {
                        self.devices_changed(nick);
                    }
                }
            }
            // 761 <me|*> <target> <key> <visibility> :<value>; 766 is "not set".
            "761" | "766" if m.param(2) == Some(VOICE_KEY) => {
                let value = if m.command == "761" { m.param(4) } else { None };
                if let Some(target) = m.param(1) {
                    self.voice_metadata(target, value.unwrap_or_default());
                }
            }
            "761" | "766"
                if matches!(m.param(2), Some(DISPLAY_NAME_KEY | AVATAR_KEY))
                    && m.param(1)
                        .is_some_and(|t| valid_name(t) && !self.is_channel(t)) =>
            {
                let value = if m.command == "761" { m.param(4) } else { None };
                let (Some(nick), Some(key)) = (m.param(1), m.param(2)) else {
                    return;
                };
                let folded = self.fold(nick);
                if !self.profiles.contains_key(&folded) && self.profiles.len() >= MAX_MEMBERS {
                    return;
                }
                let p = self.profiles.entry(folded).or_default();
                if key == DISPLAY_NAME_KEY {
                    p.display_name = value
                        .map(|v| {
                            v.chars()
                                .filter(|c| !c.is_control())
                                .take(MAX_DISPLAY_NAME)
                                .collect::<String>()
                        })
                        .map(|v| v.trim().to_owned())
                        .filter(|v| !v.is_empty());
                } else {
                    p.avatar = value
                        .and_then(|v| Url::parse(v).ok())
                        .filter(|u| matches!(u.scheme(), "https" | "http"));
                }
                self.emit_profile(nick);
            }
            "PERSISTENCE" if m.param(0) == Some("STATUS") => {
                self.persistent = m.param(2) != Some("OFF");
            }
            "CAP" => {
                let before: Vec<&str> = WANTED
                    .iter()
                    .copied()
                    .filter(|c| self.caps.enabled(c))
                    .collect();
                let out = self.caps.handle(&m);
                self.out.extend(out);
                let after: Vec<&str> = WANTED
                    .iter()
                    .copied()
                    .filter(|c| self.caps.enabled(c))
                    .collect();
                let diff = |a: &[&str], b: &[&str]| -> Vec<String> {
                    a.iter()
                        .filter(|c| !b.contains(c))
                        .map(|c| (*c).to_owned())
                        .collect()
                };
                let (added, removed) = (diff(&after, &before), diff(&before, &after));
                if !added.is_empty() || !removed.is_empty() {
                    let network = self.id();
                    self.events.push(Event::CapsChanged {
                        network,
                        added,
                        removed,
                    });
                }
            }
            "005" => {
                self.isupport.handle(&m);
                self.cm = self.isupport.casemapping();
            }
            _ => {
                if let Some(e) = failure(&Item::Message(m)) {
                    self.error(e);
                }
            }
        }
    }

    /// Answers a CTCP VERSION sent to us by a user (not to a channel), at most
    /// every [`CTCP_EVERY`].
    fn ctcp_version(&mut self, m: &Message, from: &str) {
        let to_us = m.param(0).is_some_and(|t| self.cm.eq(t, &self.info.nick));
        let user = m.source.as_deref().is_some_and(|s| s.contains('!'));
        if !to_us || !user || !valid_name(from) {
            return;
        }
        if self.ctcp_at.is_some_and(|t| t.elapsed() < CTCP_EVERY) {
            return;
        }
        self.ctcp_at = Some(Instant::now());
        let reply = format!("\x01VERSION {}\x01", self.version);
        self.out
            .push(Message::new("NOTICE", [from, reply.as_str()]));
    }

    /// A PRIVMSG/NOTICE or multiline batch as `(buffer name, message)`.
    fn chat(&self, item: &Item) -> Option<(String, ChatMessage)> {
        let (head, text) = match item {
            Item::Message(m) if matches!(m.command.as_str(), "PRIVMSG" | "NOTICE") => {
                (m, m.param(1)?.to_owned())
            }
            Item::Batch(Batch { start, items }) if start.param(1) == Some("draft/multiline") => {
                let mut text = String::new();
                let mut first = true;
                for i in items {
                    let Item::Message(l) = i else { continue };
                    if !first && l.tag("draft/multiline-concat").is_none() {
                        text.push('\n');
                    }
                    first = false;
                    text.push_str(l.param(1).unwrap_or_default());
                    if text.len() > MAX_TEXT {
                        break;
                    }
                }
                (start, text)
            }
            _ => return None,
        };
        let target = self.buffer_target(head)?;
        let nick = head.source_nick()?.to_owned();
        if head.source.as_deref().is_some_and(|s| !s.contains('!')) {
            return None; // server notices
        }
        let own = self.cm.eq(&nick, &self.info.nick);
        let (kind, text) = match text.strip_prefix('\x01') {
            Some(ctcp) => {
                // ACTION only; other CTCP is never shown nor answered.
                let body = ctcp
                    .strip_suffix('\x01')
                    .unwrap_or(ctcp)
                    .strip_prefix("ACTION ")?;
                (MessageKind::Action, body.to_owned())
            }
            None if head.command == "NOTICE" => (MessageKind::Notice, text),
            None => (MessageKind::Privmsg, text),
        };
        let text = truncate(&text, MAX_TEXT).to_owned();
        let dm = !self.is_channel(&target);
        Some((
            target,
            ChatMessage {
                msgid: head
                    .tag("msgid")
                    .filter(|i| i.len() <= 128)
                    .map(str::to_owned),
                local_id: None,
                time: head
                    .tag("time")
                    .and_then(time::parse)
                    .unwrap_or_else(time::now_ms),
                account: head.tag("account").map(str::to_owned),
                kind,
                reply_to: head
                    .tag("+draft/reply")
                    .filter(|i| i.len() <= 128)
                    .map(str::to_owned),
                status: SendStatus::Sent,
                redacted: false,
                reactions: Vec::new(),
                mention: !own && (dm || mentions(&text, &self.info.nick)),
                own,
                nick,
                text,
                crypt: Crypt::Plain,
            },
        ))
    }

    /// [`Net::chat`] as this device sees it: decrypted, or `None` if it's
    /// someone else's copy or waits for the sender's keys.
    fn chat_in(&mut self, item: &Item) -> Option<(String, ChatMessage)> {
        let (target, msg) = self.chat(item)?;
        let msg = self.open(&target, head(item), msg)?;
        Some((target, msg))
    }

    /// The buffer a message to `params[0]` belongs in: the channel, or the other
    /// side of a DM.
    fn buffer_target(&self, m: &Message) -> Option<String> {
        // A multiline batch start is `BATCH +id draft/multiline <target>`.
        let to = m.param(if m.command == "BATCH" { 2 } else { 0 })?;
        let statusmsg = self.isupport.get("STATUSMSG").unwrap_or_default();
        let to = to.trim_start_matches(|c| statusmsg.contains(c));
        let name = if self.is_channel(to) {
            to
        } else if self.cm.eq(to, &self.info.nick) {
            m.source_nick()?
        } else {
            to
        };
        valid_name(name).then(|| name.to_owned())
    }

    fn live(&mut self, target: &str, msg: ChatMessage) {
        let key = self.fold(target);
        if !self.buffers.contains_key(&key) {
            let kind = if self.is_channel(target) {
                BufferKind::Channel
            } else {
                BufferKind::Dm
            };
            // Messages for channels we're not in are someone else's problem.
            if kind == BufferKind::Channel || !self.add_buffer(target, kind) {
                return;
            }
            self.monitor(&[target]);
        }
        if msg.msgid.is_some() && !self.store.insert(&key, &msg) {
            return;
        }
        self.emit_messages(target, vec![msg]);
        self.emit_buffer(&key);
    }

    /// Stores a CHATHISTORY page and shows it. Returns how many messages it held.
    fn store_history(&mut self, key: &str, items: Vec<Item>) -> usize {
        let Some(name) = self.buffers.get(key).map(|b| b.name.clone()) else {
            return 0;
        };
        let mut messages = Vec::new();
        let mut touched = Vec::new();
        let mut n = 0;
        for item in items.into_iter().take(PAGE as usize * 2) {
            if let Some((target, msg)) = self.chat(&item) {
                // Copies for other devices still count towards the page.
                n += 1;
                messages.extend(self.open(&target, head(&item), msg));
            } else if let Item::Message(m) = item {
                match m.command.as_str() {
                    "TAGMSG" => {
                        if let (Some((e, add)), Some(id), Some(n)) =
                            (reaction(&m), m.tag("+draft/reply"), m.source_nick())
                        {
                            self.store.react(id, n, truncate(e, 64), add);
                            touched.push(id.to_owned());
                        }
                    }
                    "REDACT" => {
                        if let Some(id) = m.param(1) {
                            self.store.redact(id);
                            touched.push(id.to_owned());
                        }
                    }
                    _ => {}
                }
            }
        }
        self.store.batch(|| {
            for m in &messages {
                self.store.insert(key, m);
            }
        });
        messages.sort_by_key(|m| m.time);
        if !messages.is_empty() {
            self.emit_messages(&name, messages);
        }
        touched.iter().for_each(|id| self.refresh(id));
        self.emit_buffer(key);
        n
    }

    // ---- voice ------------------------------------------------------------

    fn join_voice(&mut self, channel: &str) {
        let key = self.fold(channel);
        let Some(b) = self
            .buffers
            .get(&key)
            .filter(|b| b.kind == BufferKind::Channel)
        else {
            return;
        };
        let refuse = if self.guest {
            Some("Voice needs an account. Sign up to use it.".to_owned())
        } else if b.voice_off {
            Some(format!("Voice is turned off in {}.", b.name))
        } else {
            None
        };
        if let Some(error) = refuse {
            return self.error(error);
        }
        let channel = b.name.clone();
        if self
            .voice
            .as_ref()
            .is_some_and(|v| self.cm.eq(&v.channel, &channel))
        {
            return;
        }
        self.voice = Some(Voice {
            channel: channel.clone(),
            connected: false,
        });
        self.voice_request = Some(channel);
        self.emit_voice_state();
    }

    /// `conn` joined the room: tell everyone.
    pub fn voice_connected(&mut self) {
        let Some(v) = &mut self.voice else { return };
        v.connected = true;
        let channel = v.channel.clone();
        self.set_own_voice(Some(&channel));
        self.emit_voice_state();
    }

    /// The call is over: we left, or it dropped with `error`.
    pub fn voice_ended(&mut self, error: Option<String>) {
        self.voice_request = None;
        if self.voice_published {
            self.set_own_voice(None);
        }
        if self.voice.take().is_some() {
            self.emit_voice_state();
        }
        if let Some(e) = error {
            self.error(e);
        }
    }

    /// `METADATA * SET key [value]`; an empty value deletes the key.
    fn set_metadata(&mut self, key: &str, value: &str) {
        if !self.caps.enabled("draft/metadata-2") {
            return self.error("This server can't store profiles.");
        }
        let mut m = Message::new("METADATA", ["*", "SET", key]);
        if !value.is_empty() {
            m.params.push(value.to_owned());
        }
        self.out.push(m);
    }

    fn emit_profile(&mut self, nick: &str) {
        let Some(p) = self.profiles.get(&self.fold(nick)) else {
            return;
        };
        let (display_name, avatar) = (p.display_name.clone(), p.avatar.clone());
        let network = self.id();
        self.events.push(Event::Profile {
            network,
            nick: nick.to_owned(),
            display_name,
            avatar,
        });
    }

    fn set_own_voice(&mut self, channel: Option<&str>) {
        if !self.connected || !self.caps.enabled("draft/metadata-2") {
            return;
        }
        self.voice_published = channel.is_some();
        let mut m = Message::new("METADATA", ["*", "SET", VOICE_KEY]);
        if let Some(c) = channel {
            m.params.push(c.to_owned());
        }
        self.out.push(m);
    }

    /// A `chad/voice` value: a user's room, or `off` on a channel. Empty is unset.
    fn voice_metadata(&mut self, target: &str, value: &str) {
        if !valid_name(target) {
            return;
        }
        if self.is_channel(target) {
            let key = self.fold(target);
            if let Some(b) = self.buffers.get_mut(&key) {
                b.voice_off = value == "off";
                let off = b.voice_off;
                self.emit_buffer(&key);
                // shortcut: chadd can't read channel metadata (ergo's API has no
                // call for it), so turning voice off is enforced here only.
                if off
                    && self
                        .voice
                        .as_ref()
                        .is_some_and(|v| self.fold(&v.channel) == key)
                {
                    self.voice_ended(Some(format!("Voice was turned off in {target}.")));
                }
            }
            return;
        }
        let room = (valid_name(value) && self.is_channel(value)).then_some(value);
        if self.cm.eq(target, &self.info.nick) {
            match &self.voice {
                // ergo keeps an always-on account's metadata after a crash, so a
                // value seen while reattaching is stale. Another device of ours in
                // a call puts its value back when it sees this.
                None if room.is_some() && !self.synced => self.set_own_voice(None),
                Some(v) if v.connected && room.is_none_or(|r| !self.cm.eq(r, &v.channel)) => {
                    let channel = v.channel.clone();
                    self.set_own_voice(Some(&channel));
                }
                _ => {}
            }
        }
        self.set_voice_of(target, room);
    }

    fn set_voice_of(&mut self, nick: &str, room: Option<&str>) {
        let key = self.fold(nick);
        let new = room.map(|r| self.fold(r));
        let old = match &new {
            Some(ch) if self.voice_of.len() < MAX_MEMBERS || self.voice_of.contains_key(&key) => {
                self.voice_of
                    .insert(key, (nick.to_owned(), ch.clone()))
                    .map(|(_, c)| c)
            }
            Some(_) => return,
            None => self.voice_of.remove(&key).map(|(_, c)| c),
        };
        if old != new {
            for ch in [old, new].into_iter().flatten() {
                self.emit_voice_members(&ch);
            }
        }
    }

    fn emit_voice_state(&mut self) {
        let state = match &self.voice {
            None => VoiceState::Idle,
            Some(v) if v.connected => VoiceState::Connected {
                channel: v.channel.clone(),
            },
            Some(v) => VoiceState::Connecting {
                channel: v.channel.clone(),
            },
        };
        let network = self.id();
        self.events.push(Event::Voice { network, state });
    }

    fn emit_voice_members(&mut self, key: &str) {
        let Some(b) = self.buffers.get(key) else {
            return;
        };
        let mut nicks: Vec<String> = self
            .voice_of
            .values()
            .filter(|(_, c)| c == key)
            .map(|(n, _)| n.clone())
            .collect();
        nicks.sort_by_key(|n| n.to_lowercase());
        let (network, channel) = (self.id(), b.name.clone());
        self.events.push(Event::VoiceMembers {
            network,
            channel,
            nicks,
        });
    }

    // ---- helpers ----------------------------------------------------------

    fn fold(&self, s: &str) -> String {
        self.cm.fold(s)
    }

    fn is_channel(&self, s: &str) -> bool {
        self.isupport.is_channel(s)
    }

    fn send_labeled(&mut self, m: Message, token: Pending) {
        let m = self.labels.tag(m, token);
        self.out.push(m);
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
        self.buffers.insert(
            key.clone(),
            Buffer {
                name: name.to_owned(),
                kind,
                topic: String::new(),
                read: None,
                members: HashMap::new(),
                online: None,
                away: false,
                voice_off: false,
            },
        );
        self.emit_buffer(&key);
        if kind == BufferKind::Dm {
            self.fetch_devices(name);
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
    fn refresh(&mut self, msgid: &str) {
        if let Some((key, msg)) = self.store.get(msgid)
            && let Some(name) = self.buffers.get(&key).map(|b| b.name.clone())
        {
            self.emit_messages(&name, vec![msg]);
            self.emit_buffer(&key);
        }
    }

    fn emit_messages(&mut self, target: &str, messages: Vec<ChatMessage>) {
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
            voice: b.kind == BufferKind::Channel && !b.voice_off,
            encryption: self.encryption(key),
        };
        let network = self.id();
        self.events.push(Event::Buffer { network, buffer });
    }

    fn emit_members(&mut self, key: &str) {
        let Some(b) = self.buffers.get(key) else {
            return;
        };
        let mut members: Vec<Member> = b.members.values().cloned().collect();
        members.sort_by_key(|a| a.nick.to_lowercase());
        let (network, channel) = (self.id(), b.name.clone());
        self.events.push(Event::Members {
            network,
            channel,
            members,
        });
    }
}

/// A plain-words error from a `FAIL` or error numeric, if that's what this is.
pub fn failure(item: &Item) -> Option<String> {
    let Item::Message(m) = item else { return None };
    let p = |i| m.param(i).unwrap_or_default();
    let numeric = m.command.parse::<u16>().ok();
    let text = match (m.command.as_str(), numeric) {
        ("FAIL", _) => match (p(0), p(1)) {
            ("REGISTER", "ACCOUNT_EXISTS" | "USERNAME_EXISTS") => "That username is taken.".into(),
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

/// `nick` as a whole word, ignoring ASCII case.
/// A TAGMSG's reaction emoji, and whether it adds (`react`) or removes (`unreact`).
fn reaction(m: &Message) -> Option<(&str, bool)> {
    (m.tag("+draft/react").map(|e| (e, true)))
        .or_else(|| m.tag("+draft/unreact").map(|e| (e, false)))
}

fn mentions(text: &str, nick: &str) -> bool {
    if nick.is_empty() {
        return false;
    }
    let word =
        |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || "_-[]\\`^{}|".contains(c));
    let (lower, nick) = (text.to_ascii_lowercase(), nick.to_ascii_lowercase());
    lower.match_indices(&nick).any(|(i, _)| {
        !word(lower[..i].chars().next_back()) && !word(lower[i + nick.len()..].chars().next())
    })
}

#[cfg(test)]
mod tests;
