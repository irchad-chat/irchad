//! What the page reads after `onChange`: snapshots of chad-core's `View`, in
//! the shapes declared to TypeScript below. Like chad-ffi's records.

use chad_core::view::{self, View};
use chad_core::{BufferKind, ConnState, MessageKind, SendStatus, format};
use serde::Serialize;
use wasm_bindgen::prelude::*;

use crate::client::Client;

#[wasm_bindgen(typescript_custom_section)]
const TS: &str = r#"
export type Rgb = [number, number, number];
export type Span = {
  text: string; bold: boolean; italic: boolean; underline: boolean; strike: boolean;
  mono: boolean; fg: Rgb | null; bg: Rgb | null; link: boolean;
  /** An @-mention: the name to show for it (`text` is the nick as written). */
  mention: string | null;
  /** A custom emoji's image (`text` is its `:name:`). */
  emoji: string | null;
};
/** What the network offers on the sign-in screen. */
export type Server = { name: string; signUp: boolean; guests: boolean; discord: boolean };
export type NetState = {
  state: "connecting" | "verifying" | "connected" | "disconnected" | "failed";
  error: string | null; nick: string; name: string; guest: boolean;
  /** Signs in with Discord (D96), so `setPassword` is for other IRC clients. */
  discord: boolean;
  /** Ours, as others see them. */
  displayName: string | null; avatar: string | null;
  /** Largest upload in bytes, if the network takes them. */
  uploads: number | null;
  /** Who GIF search comes from, if the network has it. */
  gifs: string | null;
  /** We can start meetings guests join from a link (D86). */
  meetings: boolean;
  /** Our voice call: joining, then in. */
  call: { channel: string; connected: boolean } | null;
};
export type Buffer = {
  target: string; kind: "channel" | "dm" | "server"; topic: string; unread: number; mentions: number;
  typing: string[];
  /** DMs: whether they're on (MONITOR), if known. */
  online: boolean | null; away: boolean;
  /** Scrollback reaches the start of the history. */
  historyStart: boolean;
  /** DMs: the msgid of ours to show "Seen" under (D68). */
  seen: string | null;
  /** Channels: voice is on here. */
  voice: boolean;
  /** Channels: nicks in its voice room, per `chad/voice`. */
  inVoice: string[];
};
/** A mention or DM to notify about (`takeAlerts`). */
export type Alert = { target: string; title: string; body: string };
export type VoiceJoin = { url: string; token: string };
export type Reaction = { emoji: string; image: string | null; nicks: string[]; own: boolean };
export type Link = { url: string; kind: "image" | "video" | "file" | "page" };
export type Msg = {
  /** Stable list key: the msgid, or a local one until echoed. */
  key: string; msgid: string | null; time: number; nick: string; shown: string;
  account: string | null; avatar: string | null;
  /** Without formatting, for copying and editing. */
  text: string; spans: Span[];
  /** The message is nothing but a sticker (D103): its image, to draw big in
      place of the text once it's there. */
  sticker: string | null;
  action: boolean; notice: boolean; replyTo: string | null; own: boolean; mention: boolean;
  pending: boolean; failed: string | null; redacted: boolean; edited: boolean;
  reactions: Reaction[]; links: Link[];
  /** The nicks it @-mentions (D66), to pass back when editing it. */
  mentions: string[];
};
export type Member = {
  nick: string; shown: string; prefix: string; away: boolean; oper: boolean; account: string | null;
  avatar: string | null;
  /** Their `chad/activity` ("Playing Celeste"). */
  activity: string | null;
  /** `chad/music`: the track behind that activity (unix second it began, its
      seconds or 0, an https link to it). */
  music: { start: number; length: number; url: string | null } | null;
};
export type Nick = { nick: string; shown: string };
/** `sticker` (D103): in its own picker tab, posted as a message of its own. */
export type CustomEmoji = { name: string; url: string; sticker: boolean };
export type Emoji = { emoji: string; name: string };
export type Shortcoded = { text: string; cursor: number };
export type LinkPreview = { title: string | null; description: string | null; site: string | null; image: string | null };
export type Gif = { id: string; title: string; preview: string; url: string; width: number; height: number };
export type GifPage = { gifs: Gif[]; next: string | null };
"#;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct State<'a> {
    state: &'static str,
    error: Option<&'a str>,
    nick: &'a str,
    name: &'a str,
    guest: bool,
    discord: bool,
    display_name: Option<&'a str>,
    avatar: Option<String>,
    uploads: Option<u64>,
    gifs: Option<&'a str>,
    meetings: bool,
    call: Option<Call<'a>>,
}

#[derive(Serialize)]
struct Call<'a> {
    channel: &'a str,
    connected: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Buffer<'a> {
    target: &'a str,
    kind: &'static str,
    topic: &'a str,
    unread: u32,
    mentions: u32,
    typing: Vec<&'a str>,
    online: Option<bool>,
    away: bool,
    history_start: bool,
    seen: Option<&'a str>,
    voice: bool,
    in_voice: &'a [String],
}

#[derive(Serialize, Debug, PartialEq, Eq)]
struct Span {
    text: String,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    mono: bool,
    fg: Option<[u8; 3]>,
    bg: Option<[u8; 3]>,
    link: bool,
    mention: Option<String>,
    emoji: Option<String>,
}

impl From<format::Span> for Span {
    fn from(s: format::Span) -> Self {
        Span {
            text: s.text,
            bold: s.bold,
            italic: s.italic,
            underline: s.underline,
            strike: s.strike,
            mono: s.mono,
            fg: s.fg,
            bg: s.bg,
            link: s.link,
            mention: None,
            emoji: None,
        }
    }
}

#[derive(Serialize)]
struct Reaction<'a> {
    emoji: &'a str,
    image: Option<String>,
    nicks: &'a [String],
    own: bool,
}

#[derive(Serialize)]
struct Link {
    url: String,
    kind: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Msg<'a> {
    key: String,
    msgid: Option<&'a str>,
    time: i64,
    nick: &'a str,
    shown: &'a str,
    account: Option<&'a str>,
    avatar: Option<String>,
    text: String,
    spans: Vec<Span>,
    sticker: Option<&'a str>,
    action: bool,
    notice: bool,
    reply_to: Option<&'a str>,
    own: bool,
    mention: bool,
    pending: bool,
    failed: Option<&'a str>,
    redacted: bool,
    edited: bool,
    reactions: Vec<Reaction<'a>>,
    links: Vec<Link>,
    mentions: Vec<&'a str>,
}

#[derive(Serialize)]
struct Member<'a> {
    nick: &'a str,
    shown: &'a str,
    prefix: &'a str,
    away: bool,
    oper: bool,
    account: Option<&'a str>,
    avatar: Option<String>,
    activity: Option<&'a str>,
    music: Option<Music<'a>>,
}

#[derive(Serialize)]
struct Music<'a> {
    start: u64,
    length: u32,
    url: Option<&'a str>,
}

#[derive(Serialize)]
struct Named<'a> {
    nick: &'a str,
    shown: &'a str,
}

#[derive(Serialize)]
struct Custom<'a> {
    name: &'a str,
    url: &'a str,
    sticker: bool,
}

#[derive(Serialize)]
struct Emoji {
    emoji: &'static str,
    name: &'static str,
}

#[derive(Serialize)]
struct Shortcoded {
    text: String,
    cursor: usize,
}

#[wasm_bindgen]
impl Client {
    #[wasm_bindgen(unchecked_return_type = "NetState")]
    pub fn state(&self) -> JsValue {
        let i = self.inner.borrow();
        let info = &i.net.info;
        let (state, error) = match &info.state {
            ConnState::Connecting => ("connecting", None),
            ConnState::NeedsVerification { message } => ("verifying", Some(message.as_str())),
            ConnState::Connected => ("connected", None),
            ConnState::Disconnected { error } => ("disconnected", Some(error.as_str())),
            ConnState::Failed { error } => ("failed", Some(error.as_str())),
        };
        let own = i.view.net(&info.network).and_then(|n| {
            i.view.nets[n]
                .profiles
                .get(&chad_core::view::key(&info.nick))
        });
        to_js(&State {
            state,
            error: error.filter(|e| !e.is_empty()),
            nick: &info.nick,
            name: &info.name,
            guest: info.guest,
            discord: info.discord,
            display_name: own.and_then(|p| p.display_name.as_deref()),
            avatar: own.and_then(|p| p.avatar.as_ref()).map(ToString::to_string),
            uploads: info.uploads,
            gifs: info.gifs.as_deref(),
            meetings: info.meetings,
            call: i.net.voice.as_ref().map(|v| Call {
                channel: &v.channel,
                connected: v.connected,
            }),
        })
    }

    #[wasm_bindgen(unchecked_return_type = "Buffer[]")]
    pub fn buffers(&self) -> JsValue {
        let i = self.inner.borrow();
        let Some(n) = i.view.net(&i.net.info.network).map(|n| &i.view.nets[n]) else {
            return to_js(&[] as &[Buffer]);
        };
        let list: Vec<_> = n
            .buffers
            .iter()
            .map(|b| Buffer {
                target: &b.info.target,
                kind: match b.info.kind {
                    BufferKind::Channel => "channel",
                    BufferKind::Dm => "dm",
                    BufferKind::Server => "server",
                },
                topic: &b.info.topic,
                unread: b.info.unread,
                mentions: b.info.mentions,
                typing: b.typing().map(|t| n.shown(t)).collect(),
                online: b.info.online,
                away: b.info.away,
                history_start: b.history_start,
                seen: b.seen().and_then(|s| b.msgs[s].msgid.as_deref()),
                voice: b.info.voice,
                in_voice: &b.voice,
            })
            .collect();
        to_js(&list)
    }

    /// Oldest first.
    #[wasm_bindgen(unchecked_return_type = "Msg[]")]
    pub fn messages(&self, target: &str) -> JsValue {
        let i = self.inner.borrow();
        let network = &i.net.info.network;
        let (Some(ni), Some(b)) = (i.view.net(network), i.view.buf(network, target)) else {
            return to_js(&[] as &[Msg]);
        };
        let n = &i.view.nets[ni];
        let me = view::key(&n.info.nick);
        // shortcut: the whole buffer on every change; send only what changed
        // (`Applied::fresh`) if long scrollback gets slow.
        let list: Vec<_> = b
            .msgs
            .iter()
            .map(|m| {
                let spans = spans(&m.text, &m.mentions, &n.emoji, |x| {
                    view::mention_name(&n.profiles, b.members.as_deref(), x).to_owned()
                });
                let text = format::strip(&m.text);
                Msg {
                    sticker: chad_core::emoji::sticker(&text, &n.emoji)
                        .filter(|_| m.kind != MessageKind::Action)
                        .map(|e| e.url.as_str()),
                    key: m
                        .msgid
                        .clone()
                        .unwrap_or_else(|| format!("local:{}", m.local_id.unwrap_or_default())),
                    msgid: m.msgid.as_deref(),
                    time: m.time,
                    nick: &m.nick,
                    shown: n.shown(&m.nick),
                    account: m.account.as_deref(),
                    avatar: n
                        .profiles
                        .get(&view::key(&m.nick))
                        .and_then(|p| p.avatar.as_ref())
                        .map(ToString::to_string),
                    links: links(&spans, network),
                    text,
                    spans,
                    action: m.kind == MessageKind::Action,
                    notice: m.kind == MessageKind::Notice,
                    reply_to: m.reply_to.as_deref(),
                    own: m.own,
                    mention: m.mention,
                    pending: m.status == SendStatus::Pending,
                    failed: match &m.status {
                        SendStatus::Failed(e) => Some(e.as_str()),
                        _ => None,
                    },
                    redacted: m.redacted,
                    edited: m.edited,
                    reactions: m
                        .reactions
                        .iter()
                        .map(|r| Reaction {
                            emoji: &r.emoji,
                            image: chad_core::emoji::lookup(&r.emoji, &n.emoji)
                                .map(|e| e.url.to_string()),
                            nicks: &r.nicks,
                            own: r.nicks.iter().any(|x| view::key(x) == me),
                        })
                        .collect(),
                    mentions: m.mentions.iter().map(|x| x.nick.as_str()).collect(),
                }
            })
            .collect();
        to_js(&list)
    }

    /// `null` until fetched with `fetchMembers`.
    #[wasm_bindgen(unchecked_return_type = "Member[] | null")]
    pub fn members(&self, channel: &str) -> JsValue {
        let i = self.inner.borrow();
        let v = &i.view;
        let network = &i.net.info.network;
        let (Some(ni), Some(ms)) = (
            v.net(network),
            v.buf(network, channel).and_then(|b| b.members.as_ref()),
        ) else {
            return JsValue::NULL;
        };
        let n = &v.nets[ni];
        let list: Vec<_> = ms
            .iter()
            .map(|m| {
                let p = n.profiles.get(&view::key(&m.nick));
                Member {
                    nick: &m.nick,
                    shown: n.shown(&m.nick),
                    prefix: &m.prefix,
                    away: m.away,
                    oper: m.oper,
                    account: m.account.as_deref(),
                    avatar: p.and_then(|p| p.avatar.as_ref()).map(ToString::to_string),
                    activity: p.and_then(|p| p.activity.as_deref()),
                    music: p.and_then(|p| p.music.as_ref()).map(|m| Music {
                        start: m.start,
                        length: m.length,
                        url: m.url.as_ref().map(url::Url::as_str),
                    }),
                }
            })
            .collect();
        to_js(&list)
    }

    /// The `@` list in `target` for `query` (what follows the `@`): nick or
    /// display name starts with it, then contains it, recent speakers first.
    #[wasm_bindgen(unchecked_return_type = "Nick[]")]
    pub fn nicks(&self, target: &str, query: &str) -> JsValue {
        let i = self.inner.borrow();
        let list = nicks(&i.view, &i.net.info.network, target, query);
        let named: Vec<_> = list
            .iter()
            .map(|(nick, shown)| Named { nick, shown })
            .collect();
        to_js(&named)
    }

    /// The network's custom emoji, sorted by name.
    #[wasm_bindgen(js_name = customEmoji, unchecked_return_type = "CustomEmoji[]")]
    pub fn custom_emoji(&self) -> JsValue {
        let i = self.inner.borrow();
        let set = i
            .view
            .net(&i.net.info.network)
            .map_or(&[][..], |n| &i.view.nets[n].emoji);
        let list: Vec<_> = set
            .iter()
            .map(|e| Custom {
                name: &e.name,
                url: e.url.as_str(),
                sticker: e.sticker,
            })
            .collect();
        to_js(&list)
    }

    /// The composer's text with each `:shortcode:` before `cursor` replaced
    /// by its emoji (custom ones stay as typed), or `null` when there's none.
    /// `cursor` is a JS string index (UTF-16), in and out.
    #[wasm_bindgen(js_name = replaceShortcodes, unchecked_return_type = "Shortcoded | null")]
    pub fn replace_shortcodes(&self, mut text: String, mut cursor: usize) -> JsValue {
        let i = self.inner.borrow();
        let custom = i
            .view
            .net(&i.net.info.network)
            .map_or(&[][..], |n| &i.view.nets[n].emoji);
        if chad_core::emoji::replace_shortcodes_utf16(&mut text, &mut cursor, custom) {
            to_js(&Shortcoded { text, cursor })
        } else {
            JsValue::NULL
        }
    }
}

/// Standard emoji whose name or shortcode contains `query`; all of them for
/// an empty one.
#[wasm_bindgen(js_name = emojis, unchecked_return_type = "Emoji[]")]
pub fn emoji_search(query: &str) -> JsValue {
    let q = query.trim().to_lowercase();
    let list: Vec<_> = emojis::iter()
        .filter(|e| q.is_empty() || e.name().contains(&q) || e.shortcodes().any(|s| s.contains(&q)))
        .map(|e| Emoji {
            emoji: e.as_str(),
            name: e.name(),
        })
        .collect();
    to_js(&list)
}

/// `(nick, shown)` for [`Client::nicks`].
fn nicks(v: &View, network: &str, target: &str, query: &str) -> Vec<(String, String)> {
    let (Some(ni), Some(b)) = (v.net(network), v.buf(network, target)) else {
        return Vec::new();
    };
    let n = &v.nets[ni];
    let cands = chad_core::complete::completable(
        b.members.as_deref(),
        &b.msgs,
        &b.info,
        &n.info.nick,
        &n.profiles,
    );
    chad_core::complete::suggest(&cands, &format!("@{query}"))
        .unwrap_or_default()
        .into_iter()
        .map(|c| (c.nick.clone(), c.shown().to_owned()))
        .collect()
}

/// `text` as spans: formatting, links, @-mentions and custom emoji. As chad-ffi's.
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

fn links(spans: &[Span], network: &str) -> Vec<Link> {
    use chad_core::format::Kind;
    spans
        .iter()
        .filter(|s| s.link)
        .filter_map(|s| {
            let kind = match format::kind(&s.text, network)? {
                Kind::Image => "image",
                Kind::Video => "video",
                Kind::File => "file",
                Kind::Page => "page",
            };
            Some(Link {
                url: s.text.clone(),
                kind,
            })
        })
        .collect()
}

pub(crate) fn to_js<T: Serialize + ?Sized>(v: &T) -> JsValue {
    v.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .unwrap_or_else(|e| {
            tracing::warn!("can't serialize: {e}");
            JsValue::NULL
        })
}
