//! The UI view model (D38): core [`Event`]s projected into networks, buffers,
//! messages and members, so each UI draws from the same state instead of
//! keeping its own copy of this logic.
//!
//! A UI hangs its own per-network and per-buffer state (drafts, scroll
//! position, textures) on `N` and `B`; those come and go with the network or
//! buffer they belong to, and follow a buffer through a rename.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use url::Url;
use web_time::Instant;

use crate::{
    Block, BufferInfo, BufferKind, ChatMessage, ConnState, CustomEmoji, DeviceInfo, Event, Member,
    NetworkInfo, SendStatus, Sound, Typing, VoiceState,
};

/// Only messages this fresh notify; gap-fill after a restart stays quiet.
const NOTIFY_WINDOW_MS: i64 = 60_000;

/// Someone else's message, just now: notify for it if the user's rule says so.
pub fn notifiable(m: &ChatMessage) -> bool {
    let fresh = crate::time::now_ms() - m.time < NOTIFY_WINDOW_MS;
    fresh && !m.own && m.status == SendStatus::Sent && !m.redacted
}

/// How long a typing notice holds without another one.
pub const TYPING_TTL: Duration = Duration::from_secs(6);

/// Buffer names and nicks compare under ergo's `ascii` casemapping.
pub fn key(name: &str) -> String {
    name.to_ascii_lowercase()
}

pub struct View<N = (), B = ()> {
    pub nets: Vec<Net<N, B>>,
    /// Our call: (network, state). One at a time across networks.
    pub call: Option<(String, VoiceState)>,
}

impl<N, B> Default for View<N, B> {
    fn default() -> Self {
        Self {
            nets: Vec::new(),
            call: None,
        }
    }
}

pub struct Net<N = (), B = ()> {
    pub info: NetworkInfo,
    /// Channels, then DMs, each by key.
    pub buffers: Vec<Buf<B>>,
    /// By `key(nick)`.
    pub profiles: HashMap<String, Profile>,
    /// Encrypted-DM devices, by `key(nick)`.
    pub devices: HashMap<String, Vec<DeviceInfo>>,
    /// The soundboard, as chadd last listed it.
    pub sounds: Vec<Sound>,
    /// Custom emoji, sorted by name, as chadd last listed them.
    pub emoji: Vec<CustomEmoji>,
    /// Blocked people, oldest first: their messages and reactions are hidden.
    pub blocks: Vec<Block>,
    /// Who takes reports (`Event::ReportTo`).
    pub report_to: Option<String>,
    pub ui: N,
}

pub struct Buf<B = ()> {
    pub key: String,
    pub info: BufferInfo,
    /// Sorted by time.
    pub msgs: Msgs,
    /// `None` until fetched (`NetCommand::Members`), and again after a reconnect.
    pub members: Option<std::sync::Arc<[Member]>>,
    typing: Vec<(String, Instant)>,
    /// Scrollback reaches the start of the history.
    pub history_start: bool,
    /// Who's in the channel's voice room (metadata).
    pub voice: Vec<String>,
    pub ui: B,
}

/// Someone's metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    pub display_name: Option<String>,
    pub avatar: Option<Url>,
    /// Their `chad/activity` ("Playing Celeste").
    pub activity: Option<String>,
    /// Their `chad/music`: progress and a link for that activity.
    pub music: Option<crate::Music>,
}

/// One step in how a buffer's `msgs` changed, applied in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// Same place, new content.
    Replaced(usize),
    Removed(usize),
    /// `incoming[from]` of the event went in at `at`.
    Inserted {
        at: usize,
        from: usize,
    },
}

/// What [`View::apply`] did, for the UI to follow up on.
#[derive(Debug, Default)]
pub struct Applied {
    /// `Network` or `Buffer`: seen for the first time.
    pub new: bool,
    /// `Network`: connected again, so member lists were dropped.
    pub reconnected: bool,
    /// `Messages`: how the buffer changed.
    pub edits: Vec<Edit>,
    /// `Messages`: indices into the event's messages not held before.
    pub fresh: Vec<usize>,
}

impl<N, B> Net<N, B> {
    pub fn buffer(&self, k: &str) -> Option<usize> {
        self.buffers.iter().position(|b| b.key == k)
    }

    pub fn unread(&self) -> bool {
        self.buffers.iter().any(|b| b.info.unread > 0)
    }

    pub fn mentions(&self) -> u32 {
        self.buffers.iter().map(|b| b.info.mentions).sum()
    }

    /// The name to show for `nick`: their display name if they set one.
    pub fn shown<'a>(&'a self, nick: &'a str) -> &'a str {
        shown(&self.profiles, nick)
    }

    fn sort(&mut self) {
        self.buffers.sort_by(|a, b| {
            (a.info.kind == BufferKind::Dm, &a.key).cmp(&(b.info.kind == BufferKind::Dm, &b.key))
        });
    }
}

/// The name to show for `nick`: their display name if they set one.
pub fn shown<'a>(profiles: &'a HashMap<String, Profile>, nick: &'a str) -> &'a str {
    profiles
        .get(&key(nick))
        .and_then(|p| p.display_name.as_deref())
        .unwrap_or(nick)
}

/// The name to show for a mention (D66): the display name, else the nick, of
/// whoever is in `members` with its account now, else of the nick it was
/// written as.
pub fn mention_name<'a>(
    profiles: &'a HashMap<String, Profile>,
    members: Option<&'a [crate::Member]>,
    m: &'a crate::Mention,
) -> &'a str {
    let nick = members
        .into_iter()
        .flatten()
        .find(|x| (x.account.as_deref()).is_some_and(|a| a.eq_ignore_ascii_case(&m.account)))
        .map_or(m.nick.as_str(), |x| x.nick.as_str());
    shown(profiles, nick)
}

impl<B> Buf<B> {
    /// Who's typing now, longest first.
    pub fn typing(&self) -> impl Iterator<Item = &str> {
        self.typing
            .iter()
            .filter(|(_, t)| t.elapsed() < TYPING_TTL)
            .map(|(n, _)| n.as_str())
    }

    pub fn index_of(&self, msgid: &str) -> Option<usize> {
        self.msgs.position(msgid)
    }

    /// Where "Seen" goes (D68): our newest message they've read, unless they've
    /// written since, which says as much.
    pub fn seen(&self) -> Option<usize> {
        let upto = self.info.seen?.upto;
        let end = self.msgs.partition_point(|m| m.time <= upto);
        let i = self.msgs[..end]
            .iter()
            .rposition(|m| m.own && m.msgid.is_some())?;
        (!self.msgs[i + 1..].iter().any(|m| !m.own)).then_some(i)
    }
}

fn same_msg(a: &ChatMessage, b: &ChatMessage) -> bool {
    (a.msgid.is_some() && a.msgid == b.msgid) || (a.local_id.is_some() && a.local_id == b.local_id)
}

/// A buffer's messages, sorted by time, with their ids indexed so telling a
/// new message from an update doesn't scan them all.
#[derive(Debug, Default)]
pub struct Msgs {
    list: Vec<ChatMessage>,
    msgids: HashSet<String>,
    locals: HashSet<u64>,
}

impl std::ops::Deref for Msgs {
    type Target = [ChatMessage];
    fn deref(&self) -> &[ChatMessage] {
        &self.list
    }
}

impl Msgs {
    /// Where the message with this id is; a miss costs no scan.
    pub fn position(&self, msgid: &str) -> Option<usize> {
        if !self.msgids.contains(msgid) {
            return None;
        }
        self.list
            .iter()
            .rposition(|m| m.msgid.as_deref() == Some(msgid))
    }

    /// Drops the message at `i`, like an unsent one the user gave up on.
    pub fn remove(&mut self, i: usize) -> Option<ChatMessage> {
        let m = (i < self.list.len()).then(|| self.list.remove(i))?;
        self.unindex(&m);
        Some(m)
    }

    fn holds(&self, m: &ChatMessage) -> bool {
        m.msgid.as_ref().is_some_and(|i| self.msgids.contains(i))
            || m.local_id.is_some_and(|i| self.locals.contains(&i))
    }

    fn index(&mut self, m: &ChatMessage) {
        self.msgids.extend(m.msgid.clone());
        self.locals.extend(m.local_id);
    }

    fn unindex(&mut self, m: &ChatMessage) {
        if let Some(i) = &m.msgid {
            self.msgids.remove(i);
        }
        if let Some(i) = m.local_id {
            self.locals.remove(&i);
        }
    }

    /// Where `m`'s earlier copy is. Looks among the same timestamp first, since
    /// most updates (reactions, redactions, edits) keep it.
    fn find(&self, m: &ChatMessage) -> Option<usize> {
        let from = self.list.partition_point(|x| x.time < m.time);
        let to = self.list.partition_point(|x| x.time <= m.time);
        (from..to)
            .find(|&i| same_msg(&self.list[i], m))
            .or_else(|| self.list.iter().rposition(|x| same_msg(x, m)))
    }

    /// Merges new or updated messages, keeping them sorted by time. Inserts
    /// are reported in ascending order of where they ended up.
    fn merge(&mut self, incoming: &[ChatMessage], out: &mut Applied) {
        // A later copy of a message in the same batch wins.
        let (mut ids, mut locals) = (HashSet::new(), HashSet::new());
        let mut keep = vec![false; incoming.len()];
        for (i, m) in incoming.iter().enumerate().rev() {
            // `|`, not `||`: both ids are recorded.
            let dup = m.msgid.as_deref().is_some_and(|id| !ids.insert(id))
                | m.local_id.is_some_and(|l| !locals.insert(l));
            keep[i] = !dup;
        }
        let mut new: Vec<(usize, &ChatMessage)> = Vec::new();
        for (from, m) in incoming.iter().enumerate().filter(|(i, _)| keep[*i]) {
            match self.holds(m).then(|| self.find(m)).flatten() {
                Some(i) if self.list[i].time == m.time => {
                    let old = std::mem::replace(&mut self.list[i], m.clone());
                    self.unindex(&old);
                    self.index(m);
                    out.edits.push(Edit::Replaced(i));
                }
                Some(i) => {
                    let old = self.list.remove(i);
                    self.unindex(&old);
                    out.edits.push(Edit::Removed(i));
                    new.push((from, m));
                }
                None => {
                    out.fresh.push(from);
                    new.push((from, m));
                }
            }
        }
        if new.is_empty() {
            return;
        }
        // Stable, so equal times keep their order, after the ones already held.
        new.sort_by_key(|(_, m)| m.time);
        let start = self.list.partition_point(|x| x.time <= new[0].1.time);
        let mut tail = self.list.split_off(start).into_iter().peekable();
        self.list.reserve(new.len() + tail.len());
        for (from, m) in new {
            self.list
                .extend(std::iter::from_fn(|| tail.next_if(|x| x.time <= m.time)));
            out.edits.push(Edit::Inserted {
                at: self.list.len(),
                from,
            });
            self.index(m);
            self.list.push(m.clone());
        }
        self.list.extend(tail);
    }
}

impl<N: Default, B: Default> View<N, B> {
    pub fn net(&self, origin: &str) -> Option<usize> {
        self.nets.iter().position(|n| n.info.network == origin)
    }

    pub fn buf(&self, network: &str, target: &str) -> Option<&Buf<B>> {
        let n = &self.nets[self.net(network)?];
        n.buffers.get(n.buffer(&key(target))?)
    }

    pub fn buf_mut(&mut self, network: &str, target: &str) -> Option<&mut Buf<B>> {
        let ni = self.net(network)?;
        let n = &mut self.nets[ni];
        let bi = n.buffer(&key(target))?;
        Some(&mut n.buffers[bi])
    }

    /// Projects one event. Events for networks or buffers not (or no longer)
    /// held are dropped.
    pub fn apply(&mut self, ev: &Event) -> Applied {
        let mut out = Applied::default();
        match ev {
            Event::Network(info) => match self.net(&info.network) {
                Some(i) => {
                    let n = &mut self.nets[i];
                    if info.state == ConnState::Connected && n.info.state != ConnState::Connected {
                        out.reconnected = true;
                        for b in &mut n.buffers {
                            b.members = None;
                        }
                    }
                    n.info = info.clone();
                }
                None => {
                    out.new = true;
                    self.nets.push(Net {
                        info: info.clone(),
                        buffers: Vec::new(),
                        profiles: HashMap::new(),
                        devices: HashMap::new(),
                        sounds: Vec::new(),
                        emoji: Vec::new(),
                        blocks: Vec::new(),
                        report_to: None,
                        ui: N::default(),
                    });
                }
            },
            Event::NetworkRemoved { network } => {
                self.nets.retain(|n| n.info.network != *network);
                if self.call.as_ref().is_some_and(|(n, _)| n == network) {
                    self.call = None;
                }
            }
            Event::Buffer { network, buffer } => {
                let Some(ni) = self.net(network) else {
                    return out;
                };
                let n = &mut self.nets[ni];
                let k = key(&buffer.target);
                match n.buffer(&k) {
                    Some(bi) => n.buffers[bi].info = buffer.clone(),
                    None => {
                        out.new = true;
                        n.buffers.push(Buf {
                            key: k,
                            info: buffer.clone(),
                            msgs: Msgs::default(),
                            members: None,
                            typing: Vec::new(),
                            history_start: false,
                            voice: Vec::new(),
                            ui: B::default(),
                        });
                        n.sort();
                    }
                }
            }
            Event::BufferRemoved { network, target } => {
                if let Some(ni) = self.net(network) {
                    let k = key(target);
                    self.nets[ni].buffers.retain(|b| b.key != k);
                }
            }
            Event::BufferRenamed { network, from, to } => {
                let Some(ni) = self.net(network) else {
                    return out;
                };
                let n = &mut self.nets[ni];
                let (old, new) = (key(from), key(to));
                // Renamed onto a buffer we already hold: keep that one.
                if n.buffer(&new).is_some() {
                    return out;
                }
                let Some(bi) = n.buffer(&old) else {
                    return out;
                };
                n.buffers[bi].key = new;
                n.sort();
            }
            Event::Messages {
                network,
                target,
                messages,
            } => {
                let Some(b) = self.buf_mut(network, target) else {
                    return out;
                };
                b.msgs.merge(messages, &mut out);
                for &i in &out.fresh {
                    b.typing.retain(|(nick, _)| *nick != messages[i].nick);
                }
            }
            Event::HistoryStart { network, target } => {
                if let Some(b) = self.buf_mut(network, target) {
                    b.history_start = true;
                }
            }
            Event::Members {
                network,
                channel,
                members,
            } => {
                if let Some(b) = self.buf_mut(network, channel) {
                    b.members = Some(std::sync::Arc::clone(members));
                }
            }
            Event::Typing {
                network,
                target,
                nick,
                state,
            } => {
                if let Some(b) = self.buf_mut(network, target) {
                    b.typing.retain(|(n, _)| n != nick);
                    if *state == Typing::Active {
                        b.typing.push((nick.clone(), Instant::now()));
                    }
                }
            }
            Event::Voice { network, state } => {
                if *state != VoiceState::Idle {
                    self.call = Some((network.clone(), state.clone()));
                } else if self.call.as_ref().is_some_and(|(n, _)| n == network) {
                    self.call = None;
                }
            }
            Event::VoiceMembers {
                network,
                channel,
                nicks,
            } => {
                if let Some(b) = self.buf_mut(network, channel) {
                    b.voice.clone_from(nicks);
                }
            }
            Event::Profile {
                network,
                nick,
                display_name,
                avatar,
                activity,
                music,
            } => {
                if let Some(i) = self.net(network) {
                    let p = Profile {
                        display_name: display_name.clone(),
                        avatar: avatar.clone(),
                        activity: activity.clone(),
                        music: music.clone(),
                    };
                    self.nets[i].profiles.insert(key(nick), p);
                }
            }
            Event::Devices {
                network,
                nick,
                devices,
            } => {
                if let Some(i) = self.net(network) {
                    self.nets[i].devices.insert(key(nick), devices.clone());
                }
            }
            Event::ReportTo { network, account } => {
                if let Some(i) = self.net(network) {
                    self.nets[i].report_to.clone_from(account);
                }
            }
            Event::Blocks { network, blocks } => {
                if let Some(i) = self.net(network) {
                    let n = &mut self.nets[i];
                    n.blocks.clone_from(blocks);
                    // Unblocking brings nothing back: their messages come with the next restart.
                    for b in &mut n.buffers {
                        let mut list = b.msgs.to_vec();
                        list.retain_mut(|m| crate::unblocked(blocks, m));
                        if list[..] != b.msgs[..] {
                            b.msgs = Msgs::default();
                            b.msgs.merge(&list, &mut Applied::default());
                        }
                    }
                }
            }
            Event::Emoji { network, emoji } => {
                if let Some(i) = self.net(network) {
                    self.nets[i].emoji.clone_from(emoji);
                }
            }
            Event::Sounds {
                network,
                result: Ok(list),
            } => {
                if let Some(i) = self.net(network) {
                    self.nets[i].sounds.clone_from(list);
                }
            }
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Encryption, SendStatus};

    fn msg(id: &str, time: i64) -> ChatMessage {
        crate::store::msg(id, time, "a", "hi")
    }

    fn net(state: ConnState) -> Event {
        Event::Network(NetworkInfo {
            network: "https://n".into(),
            name: "n".into(),
            icon: None,
            nick: "me".into(),
            state,
            guest: false,
            discord: false,
            uploads: None,
            gifs: None,
            games: false,
            meetings: false,
            emoji: false,
            receipts: false,
            lag_ms: None,
            oper: false,
        })
    }

    fn buffer(target: &str, kind: BufferKind) -> Event {
        Event::Buffer {
            network: "https://n".into(),
            buffer: BufferInfo {
                target: target.into(),
                kind,
                topic: String::new(),
                unread: 0,
                mentions: 0,
                online: None,
                away: false,
                voice: true,
                encryption: Encryption::Unknown,
                seen: None,
            },
        }
    }

    fn messages(target: &str, messages: Vec<ChatMessage>) -> Event {
        Event::Messages {
            network: "https://n".into(),
            target: target.into(),
            messages,
        }
    }

    #[test]
    fn merge_sorts_dedupes_and_replaces_pending() {
        let mut v = View::<(), ()>::default();
        assert!(v.apply(&net(ConnState::Connecting)).new);
        assert!(v.apply(&buffer("#A", BufferKind::Channel)).new);
        let a = v.apply(&messages("#a", vec![msg("b", 2), msg("a", 1)]));
        assert_eq!(a.fresh, [0, 1]);
        assert_eq!(
            a.edits,
            [
                Edit::Inserted { at: 0, from: 1 },
                Edit::Inserted { at: 1, from: 0 }
            ]
        );
        let a = v.apply(&messages("#a", vec![msg("a", 1)]));
        assert!(a.fresh.is_empty());
        assert_eq!(a.edits, [Edit::Replaced(0)]);
        let mut pending = msg("x", 5);
        pending.msgid = None;
        pending.local_id = Some(7);
        pending.status = SendStatus::Pending;
        assert_eq!(v.apply(&messages("#a", vec![pending])).fresh, [0]);
        // The echo carries the msgid and server time, and replaces it.
        let mut echo = msg("c", 4);
        echo.local_id = Some(7);
        let a = v.apply(&messages("#a", vec![echo]));
        assert!(a.fresh.is_empty());
        assert_eq!(
            a.edits,
            [Edit::Removed(2), Edit::Inserted { at: 2, from: 0 }]
        );
        let b = v.buf("https://n", "#a").unwrap();
        let ids: Vec<_> = b.msgs.iter().map(|m| m.msgid.clone().unwrap()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn merge_interleaves_a_page_and_dedupes_within_it() {
        let mut v = View::<(), ()>::default();
        v.apply(&net(ConnState::Connected));
        v.apply(&buffer("#a", BufferKind::Channel));
        v.apply(&messages("#a", vec![msg("b", 20), msg("d", 40)]));
        let mut later = msg("c", 30);
        later.text = "second".into();
        let a = v.apply(&messages(
            "#a",
            vec![
                msg("e", 50),
                msg("c", 30),
                msg("a", 10),
                later,
                msg("b", 20),
            ],
        ));
        assert_eq!(
            a.edits,
            [
                Edit::Replaced(0),
                Edit::Inserted { at: 0, from: 2 },
                Edit::Inserted { at: 2, from: 3 },
                Edit::Inserted { at: 4, from: 0 },
            ]
        );
        assert_eq!(a.fresh, [0, 2, 3]);
        let b = v.buf("https://n", "#a").unwrap();
        let ids: Vec<_> = b.msgs.iter().map(|m| m.msgid.clone().unwrap()).collect();
        assert_eq!(ids, ["a", "b", "c", "d", "e"]);
        assert_eq!(b.msgs[2].text, "second");
        assert_eq!(b.index_of("c"), Some(2));
        assert_eq!(b.index_of("zz"), None);
    }

    #[test]
    fn buffers_sort_rename_and_reset_members() {
        let mut v = View::<(), u32>::default();
        v.apply(&net(ConnState::Connected));
        v.apply(&buffer("bob", BufferKind::Dm));
        v.apply(&buffer("#z", BufferKind::Channel));
        v.apply(&buffer("#b", BufferKind::Channel));
        let keys = |v: &View<(), u32>| {
            v.nets[0]
                .buffers
                .iter()
                .map(|b| b.key.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&v), ["#b", "#z", "bob"]);
        v.buf_mut("https://n", "#z").unwrap().ui = 7;
        v.apply(&Event::BufferRenamed {
            network: "https://n".into(),
            from: "#z".into(),
            to: "#a".into(),
        });
        assert_eq!(keys(&v), ["#a", "#b", "bob"]);
        assert_eq!(v.buf("https://n", "#A").unwrap().ui, 7);

        v.apply(&Event::Members {
            network: "https://n".into(),
            channel: "#a".into(),
            members: Vec::new().into(),
        });
        assert!(v.buf("https://n", "#a").unwrap().members.is_some());
        v.apply(&net(ConnState::Disconnected { error: "x".into() }));
        assert!(v.apply(&net(ConnState::Connected)).reconnected);
        assert!(v.buf("https://n", "#a").unwrap().members.is_none());
    }

    #[test]
    fn typing_clears_on_message() {
        let mut v = View::<(), ()>::default();
        v.apply(&net(ConnState::Connected));
        v.apply(&buffer("#a", BufferKind::Channel));
        v.apply(&Event::Typing {
            network: "https://n".into(),
            target: "#a".into(),
            nick: "a".into(),
            state: Typing::Active,
        });
        assert_eq!(
            v.buf("https://n", "#a")
                .unwrap()
                .typing()
                .collect::<Vec<_>>(),
            ["a"]
        );
        v.apply(&messages("#a", vec![msg("m", 1)]));
        assert_eq!(v.buf("https://n", "#a").unwrap().typing().count(), 0);
    }

    #[test]
    fn seen_goes_under_our_newest_read_message() {
        let mut v = View::<(), ()>::default();
        v.apply(&net(ConnState::Connected));
        let seen = |v: &mut View, upto: Option<i64>| {
            let Event::Buffer {
                network,
                mut buffer,
            } = buffer("a", BufferKind::Dm)
            else {
                unreachable!()
            };
            buffer.seen = upto.map(|upto| crate::Seen { upto, at: upto });
            v.apply(&Event::Buffer { network, buffer });
            v.buf("https://n", "a").unwrap().seen()
        };
        let own = |id, t| ChatMessage {
            own: true,
            ..msg(id, t)
        };
        assert_eq!(seen(&mut v, Some(10)), None);
        v.apply(&messages("a", vec![own("1", 1), own("2", 2), own("3", 3)]));
        assert_eq!(seen(&mut v, None), None);
        assert_eq!(seen(&mut v, Some(2)), Some(1));
        assert_eq!(seen(&mut v, Some(9)), Some(2));
        // Their reply says it already.
        v.apply(&messages("a", vec![msg("4", 4)]));
        assert_eq!(seen(&mut v, Some(9)), None);
        assert_eq!(seen(&mut v, Some(3)), None);
    }
}
