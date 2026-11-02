//! The UI view model (D38): core [`Event`]s projected into networks, buffers,
//! messages and members, so each UI draws from the same state instead of
//! keeping its own copy of this logic.
//!
//! A UI hangs its own per-network and per-buffer state (drafts, scroll
//! position, textures) on `N` and `B`; those come and go with the network or
//! buffer they belong to, and follow a buffer through a rename.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use url::Url;

use crate::{
    BufferInfo, BufferKind, ChatMessage, ConnState, DeviceInfo, Event, Member, NetworkInfo,
    SendStatus, Sound, Typing, VoiceState,
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
    pub ui: N,
}

pub struct Buf<B = ()> {
    pub key: String,
    pub info: BufferInfo,
    /// Sorted by time.
    pub msgs: Vec<ChatMessage>,
    /// `None` until fetched (`NetCommand::Members`), and again after a reconnect.
    pub members: Option<Vec<Member>>,
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

impl<B> Buf<B> {
    /// Who's typing now, longest first.
    pub fn typing(&self) -> impl Iterator<Item = &str> {
        self.typing
            .iter()
            .filter(|(_, t)| t.elapsed() < TYPING_TTL)
            .map(|(n, _)| n.as_str())
    }

    pub fn index_of(&self, msgid: &str) -> Option<usize> {
        self.msgs
            .iter()
            .rposition(|m| m.msgid.as_deref() == Some(msgid))
    }
}

fn same_msg(a: &ChatMessage, b: &ChatMessage) -> bool {
    (a.msgid.is_some() && a.msgid == b.msgid) || (a.local_id.is_some() && a.local_id == b.local_id)
}

/// Merges new or updated messages, keeping `msgs` sorted by time.
fn merge(msgs: &mut Vec<ChatMessage>, incoming: &[ChatMessage], out: &mut Applied) {
    for (from, m) in incoming.iter().enumerate() {
        if let Some(i) = msgs.iter().rposition(|x| same_msg(x, m)) {
            if msgs[i].time == m.time {
                msgs[i] = m.clone();
                out.edits.push(Edit::Replaced(i));
                continue;
            }
            msgs.remove(i);
            out.edits.push(Edit::Removed(i));
        } else {
            out.fresh.push(from);
        }
        let at = msgs.partition_point(|x| x.time <= m.time);
        msgs.insert(at, m.clone());
        out.edits.push(Edit::Inserted { at, from });
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
                            msgs: Vec::new(),
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
                merge(&mut b.msgs, messages, &mut out);
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
                    b.members = Some(members.clone());
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
                    b.voice = nicks.clone();
                }
            }
            Event::Profile {
                network,
                nick,
                display_name,
                avatar,
            } => {
                if let Some(i) = self.net(network) {
                    let p = Profile {
                        display_name: display_name.clone(),
                        avatar: avatar.clone(),
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
            Event::Sounds {
                network,
                result: Ok(list),
            } => {
                if let Some(i) = self.net(network) {
                    self.nets[i].sounds = list.clone();
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
    use crate::{Crypt, Encryption, MessageKind, SendStatus};

    fn msg(id: &str, time: i64) -> ChatMessage {
        ChatMessage {
            msgid: Some(id.into()),
            local_id: None,
            time,
            nick: "a".into(),
            account: None,
            kind: MessageKind::Privmsg,
            text: "hi".into(),
            reply_to: None,
            status: SendStatus::Sent,
            redacted: false,
            edits: None,
            edited: false,
            reactions: vec![],
            mention: false,
            own: false,
            crypt: Crypt::Plain,
        }
    }

    fn net(state: ConnState) -> Event {
        Event::Network(NetworkInfo {
            network: "https://n".into(),
            name: "n".into(),
            icon: None,
            nick: "me".into(),
            state,
            guest: false,
            uploads: None,
            gifs: None,
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
                Edit::Inserted { at: 0, from: 0 },
                Edit::Inserted { at: 0, from: 1 }
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
            members: vec![],
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
}
