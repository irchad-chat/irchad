//! The virtualized, variable-height message list (design: `docs/notes/egui-chatlog.md`).
//!
//! Heights are estimated until a row is laid out at the current width;
//! `prefix[i]` is the y of row i. Only rows in the viewport are laid out, and
//! any height change above the viewport shifts the scroll offset by the same
//! amount, so what you're reading stays put.

use std::collections::HashMap;

use chad_core::view::{Edit, Msgs, mention_name};
use chad_core::{ChatMessage, Crypt, Mention, MessageKind, SendStatus, mention};
use egui::{Align, Color32, Layout, Rect, RichText, ScrollArea, Sense, Stroke, UiBuilder, vec2};
use jiff::tz::TimeZone;

use crate::icons::{self, Tip};
use crate::media::{self, Kind, Media};
use chad_core::format;

const ROW_GAP: f32 = 2.0;
/// Room left of each row for the mention bar.
const GUTTER: f32 = 8.0;
/// A new header after this much quiet, even from the same person.
const GROUP_GAP_MS: i64 = 5 * 60 * 1000;
pub const QUICK_REACTIONS: [&str; 4] = ["👍", "❤", "😂", "😮"];
const AVATAR: f32 = 32.0;
/// Inline images per message.
const MAX_IMAGES: usize = 3;

pub use chad_core::view::{Profile, shown};

/// What rows need besides the messages.
pub struct Extras<'a> {
    pub profiles: &'a HashMap<String, Profile>,
    /// The buffer's members, to name mentions by account (D66).
    pub members: Option<std::sync::Arc<[chad_core::Member]>>,
    pub media: &'a mut Media,
    /// The network's custom emoji.
    pub emoji: &'a [chad_core::CustomEmoji],
    /// The network's origin, to tell its own uploads apart.
    pub network: &'a str,
    /// Load images, previews and avatars without asking.
    pub auto_media: bool,
    /// Show images and previews in guests' messages at all.
    pub guest_media: bool,
    /// A DM, where unencrypted messages are marked.
    pub dm: bool,
    /// The message to put "Seen" under, and when they read it (D68).
    pub seen: Option<(String, i64)>,
}

pub enum Action {
    Reply(usize),
    React {
        msgid: String,
        emoji: String,
    },
    Unreact {
        msgid: String,
        emoji: String,
    },
    Edit(usize),
    Delete {
        msgid: String,
    },
    Jump {
        msgid: String,
    },
    OpenDm(String),
    /// `(nick, account)`.
    Block(String, Option<String>),
    /// Open the emoji picker to react to `msgid`, near `at`.
    PickReaction {
        msgid: String,
        at: egui::Pos2,
    },
}

#[derive(Default)]
pub struct Output {
    pub actions: Vec<Action>,
    pub near_top: bool,
    pub at_bottom: bool,
}

/// Layout of one buffer's messages, which live in the view (`Buf::msgs`).
#[derive(Default)]
pub struct ChatLog {
    /// One per message, in step with them.
    rows: Vec<Row>,
    prefix: Vec<f32>,
    prefix_dirty: bool,
    width: f32,
    /// Applied on the next pass (often a discarded re-run of this frame).
    pending_offset: Option<f32>,
    /// Height added (or removed) above the viewport since the last pass.
    shift: f32,
    /// The "new messages" line sits above this message.
    pub unread_from: Option<String>,
    last_top: f32,
    view_height: f32,
    at_bottom: bool,
    line: f32,
    to_bottom: bool,
    /// The row showing the toolbar, and where the toolbar is.
    hover: Option<(String, Rect)>,
}

struct Row {
    height: f32,
    measured: bool,
    /// Made on first draw, dropped when the message changes.
    parsed: Option<Parsed>,
}

impl Row {
    fn new(height: f32) -> Self {
        Self {
            height,
            measured: false,
            parsed: None,
        }
    }
}

/// What drawing a message needs that's too costly to work out every frame.
struct Parsed {
    /// Spans per line: each line is its own PRIVMSG to plain clients.
    lines: Vec<Vec<format::Span>>,
    /// Without formatting, for reply snippets.
    plain: String,
    /// Inline media and previews, in order.
    links: Vec<(String, Kind)>,
    /// shortcut: kept across a change of the system time zone; restart to pick it up.
    day: Option<jiff::civil::Date>,
    clock: String,
    /// Where the message this replies to was last seen.
    parent: Option<usize>,
}

impl Parsed {
    fn new(m: &ChatMessage, network: &str) -> Self {
        let lines: Vec<Vec<format::Span>> = m.text.split('\n').map(format::parse).collect();
        let links = lines
            .iter()
            .flatten()
            .filter(|s| s.link)
            .filter_map(|s| Some((s.text.clone(), media::kind(&s.text, network)?)))
            .collect();
        let ts = jiff::Timestamp::from_millisecond(m.time)
            .ok()
            .map(|t| t.to_zoned(TimeZone::system()));
        Self {
            plain: lines
                .iter()
                .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n"),
            lines,
            links,
            day: ts.as_ref().map(jiff::Zoned::date),
            clock: ts
                .map(|t| t.strftime("%H:%M").to_string())
                .unwrap_or_default(),
            parent: None,
        }
    }
}

/// Stable per nick, readable on both themes.
pub fn nick_color(nick: &str, dark: bool) -> Color32 {
    let h = nick
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let hue = (h % 360) as f32 / 360.0;
    egui::ecolor::Hsva::new(hue, 0.55, if dark { 0.9 } else { 0.55 }, 1.0).into()
}

impl ChatLog {
    /// Follows the view's merge of `incoming` into the buffer's messages.
    pub fn apply(&mut self, edits: &[Edit], incoming: &[ChatMessage]) {
        let mut rest = edits;
        while let Some((e, tail)) = rest.split_first() {
            match *e {
                Edit::Replaced(i) => {
                    if let Some(r) = self.rows.get_mut(i) {
                        (r.measured, r.parsed) = (false, None);
                    }
                }
                Edit::Removed(i) if i < self.rows.len() => {
                    let y: f32 = self.rows[..i].iter().map(|r| r.height).sum();
                    let r = self.rows.remove(i);
                    if y < self.last_top && !self.at_bottom {
                        self.shift -= r.height;
                    }
                    self.prefix_dirty = true;
                }
                Edit::Inserted { .. } => {
                    let n = rest
                        .iter()
                        .take_while(|e| matches!(e, Edit::Inserted { .. }))
                        .count();
                    self.insert_run(&rest[..n], incoming);
                    rest = &rest[n..];
                    continue;
                }
                Edit::Removed(_) => {}
            }
            rest = tail;
        }
    }

    /// Consecutive inserts in one pass over the rows (a history page is a
    /// hundred of them). Each `at` counts the run's earlier inserts.
    fn insert_run(&mut self, run: &[Edit], incoming: &[ChatMessage]) {
        let mut old = std::mem::take(&mut self.rows).into_iter();
        let mut rows: Vec<Row> = Vec::with_capacity(old.len() + run.len());
        // Height of `rows` so far.
        let mut total = 0.0;
        for e in run {
            let Edit::Inserted { at, from } = *e else {
                continue;
            };
            let Some(m) = incoming.get(from) else {
                continue;
            };
            while rows.len() < at
                && let Some(r) = old.next()
            {
                total += r.height;
                rows.push(r);
            }
            // Past the end: stale or hostile.
            if rows.len() < at {
                continue;
            }
            let y = if at == rows.len() {
                total
            } else {
                // Out of order, which `View` never sends: correct, if slower.
                rows[..at].iter().map(|r| r.height).sum()
            };
            let h = self.estimate(m);
            if y < self.last_top && !self.at_bottom {
                self.shift += h;
            }
            total += h;
            rows.insert(at, Row::new(h));
        }
        rows.extend(old);
        self.rows = rows;
        self.prefix_dirty = true;
    }

    fn estimate(&self, m: &ChatMessage) -> f32 {
        let line = if self.line > 0.0 { self.line } else { 18.0 };
        let per_line = (self.width / (line * 0.45)).max(10.0);
        let lines = m
            .text
            .split('\n')
            .map(|l| (l.chars().count() as f32 / per_line).ceil().max(1.0))
            .sum::<f32>();
        let extra = if m.reactions.is_empty() {
            0.0
        } else {
            line + 6.0
        };
        lines * line + extra + ROW_GAP
    }

    fn rebuild_prefix(&mut self) {
        if !self.prefix_dirty && self.prefix.len() == self.rows.len() + 1 {
            return;
        }
        self.prefix.clear();
        let mut y = 0.0;
        self.prefix.push(0.0);
        for r in &self.rows {
            y += r.height;
            self.prefix.push(y);
        }
        self.prefix_dirty = false;
    }

    /// True when the layout width or line height changed.
    fn set_width(&mut self, msgs: &[ChatMessage], width: f32, line: f32) -> bool {
        // Out of step with the messages (a missed edit): start the layout over.
        let synced = self.rows.len() == msgs.len();
        let resized = (width - self.width).abs() >= 0.5 || (line - self.line).abs() >= 0.01;
        if synced && !resized {
            return false;
        }
        let same_line = (line - self.line).abs() < 0.01;
        self.width = width;
        self.line = line;
        // A resize keeps the old heights as guesses: close enough that a
        // panel drag doesn't throw the scroll position around every frame.
        if synced && same_line {
            self.rows.iter_mut().for_each(|r| r.measured = false);
        } else {
            self.rows = msgs.iter().map(|m| Row::new(self.estimate(m))).collect();
            self.prefix_dirty = true;
        }
        resized
    }

    fn parse(&mut self, i: usize, msgs: &[ChatMessage], network: &str) {
        if self.rows[i].parsed.is_none() {
            self.rows[i].parsed = Some(Parsed::new(&msgs[i], network));
        }
    }

    /// The index of the message row `i` replies to, if it's loaded.
    fn parent(&mut self, i: usize, msgs: &Msgs) -> Option<usize> {
        let id = msgs[i].reply_to.as_deref()?;
        let cached = self.rows[i].parsed.as_ref().and_then(|p| p.parent);
        if let Some(p) =
            cached.filter(|&p| msgs.get(p).is_some_and(|m| m.msgid.as_deref() == Some(id)))
        {
            return Some(p);
        }
        // Only scans when it moved; an unloaded parent is an index miss.
        let p = msgs.position(id)?;
        if let Some(parsed) = &mut self.rows[i].parsed {
            parsed.parent = Some(p);
        }
        Some(p)
    }

    /// A real offset, not f32::MAX: egui uses an out-of-range offset as-is for
    /// one pass, which paints the whole log off-screen.
    /// Resolved in the next pass, once rows and viewport height are known.
    pub fn scroll_to_bottom(&mut self) {
        self.to_bottom = true;
    }

    pub fn scroll_to_index(&mut self, i: usize, margin: f32) {
        self.rebuild_prefix();
        if let Some(y) = self.prefix.get(i) {
            self.pending_offset = Some((y - margin).max(0.0));
        }
    }

    /// `highlights` is off in DMs, where every message is for you anyway.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        msgs: &Msgs,
        own_nick: &str,
        highlights: bool,
        extras: &mut Extras,
    ) -> Output {
        let line = ui.text_style_height(&egui::TextStyle::Body) + 2.0;
        let resized = self.set_width(msgs, ui.available_width() - 16.0, line);
        self.rebuild_prefix();
        let mut out = Output::default();
        if msgs.is_empty() {
            out.near_top = true;
            out.at_bottom = true;
            self.at_bottom = true;
            return out;
        }

        if std::mem::take(&mut self.to_bottom) {
            let total = self.prefix.last().copied().unwrap_or(0.0);
            self.pending_offset = Some((total - ui.available_height()).max(0.0));
            self.shift = 0.0;
        }
        let mut area = ScrollArea::vertical()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .animated(false);
        if let Some(off) = self.pending_offset.take() {
            area = area.vertical_scroll_offset(off + std::mem::take(&mut self.shift));
        } else if self.shift != 0.0 {
            area = area.vertical_scroll_offset(self.last_top + std::mem::take(&mut self.shift));
        }

        let mut compensate = 0.0;
        let mut changed = false;
        let was_bottom = self.at_bottom;
        let dark = ui.visuals().dark_mode;
        let res = area.show_viewport(ui, |ui, viewport| {
            let total = *self.prefix.last().unwrap_or(&0.0);
            ui.set_height(total);
            let origin = ui.min_rect().min;
            let first = self
                .prefix
                .partition_point(|&y| y <= viewport.min.y)
                .saturating_sub(1)
                .min(msgs.len() - 1);
            let mut y = self.prefix[first];
            let mut i = first;
            let mut rows = Vec::new();
            while i < msgs.len() && y < viewport.max.y {
                let rect = Rect::from_min_size(
                    origin + vec2(GUTTER, y),
                    vec2(self.width - GUTTER, f32::INFINITY),
                );
                let parent = self.parent(i, msgs);
                for j in [Some(i), i.checked_sub(1), parent].into_iter().flatten() {
                    self.parse(j, msgs, extras.network);
                }
                let parsed = |j: usize| self.rows[j].parsed.as_ref().expect("parsed above");
                let prev = i.checked_sub(1).map(|p| (&msgs[p], parsed(p)));
                let parent = parent.map(|p| (&msgs[p], parsed(p)));
                let m = (&msgs[i], parsed(i));
                let unread = m.0.msgid.is_some() && self.unread_from == m.0.msgid;
                let resp = ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(rect)
                        .layout(Layout::top_down(Align::Min))
                        .id_salt(("row", m.0.msgid.as_deref(), m.0.local_id)),
                    |ui| {
                        row_ui(
                            ui,
                            m,
                            prev,
                            parent,
                            unread,
                            own_nick,
                            highlights,
                            dark,
                            extras,
                            &mut out.actions,
                        )
                    },
                );
                // Full width, so the toolbar sits at the right edge, not over short text.
                let row = Rect::from_x_y_ranges(rect.x_range(), resp.response.rect.y_range());
                rows.push((i, row));
                let h = resp.response.rect.height() + ROW_GAP;
                let r = &mut self.rows[i];
                if !r.measured || (h - r.height).abs() > 0.5 {
                    let d = h - r.height;
                    if self.prefix[i] < viewport.min.y && d.abs() > 0.5 {
                        compensate += d;
                    }
                    (r.height, r.measured) = (h, true);
                    changed = true;
                }
                y += h;
                i += 1;
            }
            if changed {
                self.prefix_dirty = true;
            }
            // The toolbar stays while the pointer is on it, even past its row.
            // A window over the log hides it.
            let (log, bar) = (ui.layer_id(), toolbar_layer(ui.layer_id()));
            let pointer = ui.ctx().pointer_hover_pos().filter(|p| {
                ui.clip_rect().contains(*p)
                    && ui
                        .ctx()
                        .layer_id_at(*p)
                        .is_none_or(|l| l == log || l == bar)
            });
            let keep = self
                .hover
                .as_ref()
                .filter(|(_, bar)| pointer.is_some_and(|p| bar.contains(p)));
            let hovered = match keep {
                Some((id, _)) => rows
                    .iter()
                    .copied()
                    .find(|(i, _)| msgs[*i].msgid.as_ref() == Some(id)),
                None => rows
                    .iter()
                    .copied()
                    .find(|(_, r)| pointer.is_some_and(|p| r.contains(p))),
            };
            self.hover = hovered.and_then(|(i, r)| {
                let m = &msgs[i];
                if m.redacted {
                    return None;
                }
                let bar = toolbar(ui.ctx(), ui.layer_id(), i, m, r, own_nick, &mut out.actions);
                m.msgid.clone().map(|id| (id, bar))
            });
            viewport
        });

        let viewport = res.inner;
        self.last_top = viewport.min.y;
        self.view_height = viewport.height();
        self.at_bottom = viewport.max.y >= res.content_size.y - 2.0;
        // While a panel is dragged every frame relayouts; a frame of lag beats
        // discarding every pass, so fix it up next frame instead.
        let redo = |why: &'static str| match resized {
            true => ui.ctx().request_repaint(),
            false => ui.ctx().request_discard(why),
        };
        if compensate != 0.0 && !self.at_bottom {
            self.pending_offset = Some(viewport.min.y + compensate);
            redo("chat log row heights changed above viewport");
        }
        // Heights at the bottom changed (resize, new message): stick_to_bottom
        // would lag a frame behind the new total, so redo the pass pinned.
        if changed && was_bottom {
            self.to_bottom = true;
            redo("chat log height changed at bottom");
        }
        if self.shift != 0.0 {
            ui.ctx().request_repaint();
        }
        out.near_top = viewport.min.y < 400.0;
        out.at_bottom = self.at_bottom;
        out
    }
}

#[allow(clippy::too_many_arguments)]
fn row_ui(
    ui: &mut egui::Ui,
    (m, parsed): (&ChatMessage, &Parsed),
    prev: Option<(&ChatMessage, &Parsed)>,
    parent: Option<(&ChatMessage, &Parsed)>,
    unread: bool,
    own_nick: &str,
    highlights: bool,
    dark: bool,
    extras: &mut Extras,
    actions: &mut Vec<Action>,
) {
    let new_day = prev.is_none_or(|(_, p)| p.day != parsed.day);
    let prev = prev.map(|(p, _)| p);
    if new_day && let Some(ts) = jiff::Timestamp::from_millisecond(m.time).ok() {
        let label = ts
            .to_zoned(TimeZone::system())
            .strftime("%A, %-d %B %Y")
            .to_string();
        separator(ui, &label, ui.visuals().weak_text_color());
    }
    if unread {
        separator(ui, "New messages", Color32::from_rgb(237, 66, 69));
    }
    let header = new_day
        || unread
        || m.reply_to.is_some()
        || prev.is_none_or(|p| {
            p.nick != m.nick
                || m.time - p.time > GROUP_GAP_MS
                || p.kind != m.kind
                || p.crypt != m.crypt
        });

    let bg = ui.painter().add(egui::Shape::Noop);
    let top = ui.cursor().min;
    let name = shown(extras.profiles, &m.nick);
    ui.horizontal_top(|ui| {
        let (avatar, _) = ui.allocate_exact_size(vec2(AVATAR, 1.0), Sense::hover());
        ui.vertical(|ui| {
            if header && m.kind != MessageKind::Action {
                ui.add_space(4.0);
                let at = Rect::from_min_size(
                    egui::pos2(avatar.left(), ui.cursor().top()),
                    vec2(AVATAR, AVATAR),
                );
                let profile = extras.profiles.get(&m.nick.to_ascii_lowercase());
                extras.media.avatar(
                    ui,
                    at,
                    name,
                    profile.and_then(|p| p.avatar.as_ref()),
                    extras.auto_media,
                );
                ui.horizontal(|ui| {
                    let label = RichText::new(name)
                        .strong()
                        .color(nick_color(&m.nick, dark));
                    let r = ui
                        .add(egui::Label::new(label).sense(Sense::click()))
                        .on_hover_ui(|ui| {
                            ui.label(account_tip(&m.nick, name, m.account.as_deref()));
                        });
                    if !m.own {
                        if r.clicked() {
                            actions.push(Action::OpenDm(m.nick.clone()));
                        }
                        r.context_menu(|ui| {
                            if ui.button("Block").clicked() {
                                actions.push(Action::Block(m.nick.clone(), m.account.clone()));
                            }
                        });
                    }
                    // Own pending lines have no account until the echo; skip them.
                    if m.account.is_none() && !m.own {
                        guest_badge(ui);
                    }
                    ui.label(RichText::new(&parsed.clock).weak().small());
                    if extras.dm && m.crypt == Crypt::Plain {
                        ui.label(
                            RichText::new(format!("{} not encrypted", icons::LOCK_OPEN))
                                .weak()
                                .small(),
                        );
                    }
                });
            }
            if m.reply_to.is_some() {
                let text = match parent {
                    Some((p, _)) if p.redacted => {
                        format!(
                            "{} {}: (deleted)",
                            icons::ARROW_BEND_DOWN_RIGHT,
                            shown(extras.profiles, &p.nick)
                        )
                    }
                    Some((p, pp)) => format!(
                        "{} {}: {}",
                        icons::ARROW_BEND_DOWN_RIGHT,
                        shown(extras.profiles, &p.nick),
                        snippet(&pp.plain, 80)
                    ),
                    None => format!("{} Earlier message", icons::ARROW_BEND_DOWN_RIGHT),
                };
                let r = ui.add(
                    egui::Label::new(RichText::new(text).weak().small())
                        .sense(Sense::click())
                        .truncate(),
                );
                if r.on_hover_text("Jump to the message").clicked()
                    && let Some(id) = &m.reply_to
                {
                    actions.push(Action::Jump { msgid: id.clone() });
                }
            }

            if m.redacted {
                ui.label(RichText::new("This message was deleted.").italics().weak());
            } else if m.crypt == Crypt::Failed {
                let text = format!(
                    "{} This message couldn't be decrypted. It may have been tampered with.",
                    icons::WARNING
                );
                ui.label(
                    RichText::new(text)
                        .italics()
                        .color(ui.visuals().error_fg_color),
                );
            } else {
                body_ui(ui, m, &parsed.lines, name, dark, extras);
                if m.edited {
                    ui.label(RichText::new("(edited)").weak().small());
                }
                links_ui(ui, m, &parsed.links, extras);
            }
            if let SendStatus::Failed(why) = &m.status {
                ui.label(
                    RichText::new(format!("Not sent: {why}"))
                        .small()
                        .color(ui.visuals().error_fg_color),
                );
            }
            if !m.reactions.is_empty() && !m.redacted {
                ui.horizontal_wrapped(|ui| {
                    for r in &m.reactions {
                        let mine = r.nicks.iter().any(|n| n == own_nick);
                        let label = format!("{} {}", r.emoji, r.nicks.len());
                        let height = ui.text_style_height(&egui::TextStyle::Body);
                        let chip = match chad_core::emoji::lookup(&r.emoji, extras.emoji)
                            .and_then(|e| extras.media.emoji_image(ui, e, height))
                        {
                            Some(img) => {
                                egui::Button::image_and_text(img, r.nicks.len().to_string())
                            }
                            None => egui::Button::new(&label),
                        };
                        let resp = ui.add(chip.small().selected(mine));
                        resp.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Button, true, &label)
                        });
                        let resp = resp.on_hover_ui(|ui| {
                            let who: Vec<&str> =
                                r.nicks.iter().map(|n| shown(extras.profiles, n)).collect();
                            ui.label(who.join(", "));
                        });
                        if resp.clicked()
                            && let Some(id) = &m.msgid
                        {
                            let (msgid, emoji) = (id.clone(), r.emoji.clone());
                            actions.push(if mine {
                                Action::Unreact { msgid, emoji }
                            } else {
                                Action::React { msgid, emoji }
                            });
                        }
                    }
                });
            }
            if let Some((_, at)) = extras
                .seen
                .as_ref()
                .filter(|(id, _)| m.msgid.as_ref() == Some(id))
            {
                let when = jiff::Timestamp::from_millisecond(*at)
                    .map(|t| {
                        t.to_zoned(TimeZone::system())
                            .strftime("%-d %B, %H:%M")
                            .to_string()
                    })
                    .unwrap_or_default();
                ui.label(RichText::new(format!("{} Seen", icons::EYE)).small().weak())
                    .on_hover_text(format!("Read {when}"));
            }
        });
    });

    let rect = Rect::from_min_max(
        top - vec2(GUTTER, 0.0),
        egui::pos2(ui.max_rect().right(), ui.min_rect().bottom()),
    );
    let hovered = ui.rect_contains_pointer(rect);
    let highlight = highlights && m.mention && !m.own;
    let fill = if highlight {
        Color32::from_rgba_unmultiplied(250, 166, 26, if hovered { 40 } else { 25 })
    } else if hovered {
        ui.visuals().faint_bg_color
    } else {
        Color32::TRANSPARENT
    };
    ui.painter()
        .set(bg, egui::Shape::rect_filled(rect, 2.0, fill));
    if highlight {
        let bar = Rect::from_min_max(rect.min, egui::pos2(rect.min.x + 2.0, rect.max.y));
        ui.painter()
            .rect_filled(bar, 0.0, Color32::from_rgb(250, 166, 26));
    }
}

fn toolbar_layer(log: egui::LayerId) -> egui::LayerId {
    egui::LayerId::new(log.order, egui::Id::unique("row-toolbar"))
}

/// Replies, quick reactions, delete; floats over the row's top-right corner
/// so it never changes a row's height. Returns its rect.
fn toolbar(
    ctx: &egui::Context,
    log: egui::LayerId,
    index: usize,
    m: &ChatMessage,
    row: Rect,
    own_nick: &str,
    actions: &mut Vec<Action>,
) -> Rect {
    let Some(id) = &m.msgid else {
        return Rect::NOTHING;
    };
    // Stacked just above the log's own layer, so floating windows cover it.
    let layer = toolbar_layer(log);
    ctx.set_sublayer(log, layer);
    egui::Area::new(layer.id)
        .order(layer.order)
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(row.right_top() - vec2(4.0, 12.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(2.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for e in QUICK_REACTIONS {
                            let mine = m
                                .reactions
                                .iter()
                                .any(|r| r.emoji == e && r.nicks.iter().any(|n| n == own_nick));
                            let b = egui::Button::new(e).small().selected(mine);
                            let tip = if mine { "Remove reaction" } else { "React" };
                            if ui.add(b).on_hover_text(tip).clicked() {
                                let (msgid, emoji) = (id.clone(), e.into());
                                actions.push(if mine {
                                    Action::Unreact { msgid, emoji }
                                } else {
                                    Action::React { msgid, emoji }
                                });
                            }
                        }
                        let more = ui.small_button(icons::SMILEY).tip("Add a reaction");
                        if more.clicked() {
                            actions.push(Action::PickReaction {
                                msgid: id.clone(),
                                at: more.rect.left_bottom(),
                            });
                        }
                        if ui
                            .small_button(icons::ARROW_BEND_UP_LEFT)
                            .tip("Reply")
                            .clicked()
                        {
                            actions.push(Action::Reply(index));
                        }
                        if m.own
                            && m.crypt != Crypt::Failed
                            && ui.small_button(icons::PENCIL_SIMPLE).tip("Edit").clicked()
                        {
                            actions.push(Action::Edit(index));
                        }
                        if m.own && ui.small_button(icons::TRASH).tip("Delete").clicked() {
                            actions.push(Action::Delete { msgid: id.clone() });
                        }
                    });
                });
        })
        .response
        .rect
}

/// "nick · account x", for hovering someone's name.
pub fn account_tip(nick: &str, name: &str, account: Option<&str>) -> String {
    match account {
        Some(a) if name != nick => format!("{nick} · account {a}"),
        Some(a) => format!("account {a}"),
        None => format!("{nick} · not logged in"),
    }
}

pub fn guest_badge(ui: &mut egui::Ui) {
    ui.label(RichText::new(icons::GHOST).weak())
        .on_hover_text("Guest (not logged in)");
}

/// Images inline, videos and files as cards, and a preview of the first page.
fn links_ui(ui: &mut egui::Ui, m: &ChatMessage, links: &[(String, Kind)], extras: &mut Extras) {
    if m.kind == MessageKind::Notice || (!extras.guest_media && m.account.is_none() && !m.own) {
        return;
    }
    let (mut images, mut previewed) = (0, false);
    for (url, kind) in links {
        match *kind {
            Kind::Image if images < MAX_IMAGES => {
                images += 1;
                extras.media.image(ui, url, extras.auto_media);
            }
            k @ (Kind::Video | Kind::File) => Media::card(ui, url, k),
            Kind::Page if !previewed => {
                previewed = true;
                extras.media.link_preview(ui, url, extras.auto_media);
            }
            _ => {}
        }
    }
}

fn body_ui(
    ui: &mut egui::Ui,
    m: &ChatMessage,
    lines: &[Vec<format::Span>],
    name: &str,
    dark: bool,
    extras: &mut Extras,
) {
    let weak = m.status == SendStatus::Pending || m.kind == MessageKind::Notice;
    let mut first = true;
    for group in lines.chunk_by(|a, b| format::code_line(a) == format::code_line(b)) {
        if format::code_line(&group[0]) {
            egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .corner_radius(4)
                .inner_margin(6)
                .show(ui, |ui| {
                    let text: Vec<String> = group
                        .iter()
                        .map(|l| l.iter().map(|s| s.text.as_str()).collect())
                        .collect();
                    ui.label(RichText::new(text.join("\n")).monospace());
                });
            first = false;
            continue;
        }
        for spans in group {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if std::mem::take(&mut first) && m.kind == MessageKind::Action {
                    ui.label(
                        RichText::new(format!("{name} "))
                            .italics()
                            .strong()
                            .color(nick_color(&m.nick, dark)),
                    );
                }
                if spans.is_empty() {
                    ui.label(" ");
                }
                for span in spans {
                    if span.link || span.mono {
                        span_ui(ui, m, span, &span.text, weak);
                        continue;
                    }
                    let pieces = if m.mentions.is_empty() {
                        vec![(span.text.as_str(), None)]
                    } else {
                        mention::split(&span.text, &m.mentions)
                    };
                    for (text, x) in pieces {
                        if let Some(x) = x {
                            let who = mention_name(extras.profiles, extras.members.as_deref(), x);
                            mention_ui(ui, x, who, dark);
                            continue;
                        }
                        for (text, e) in chad_core::emoji::split(text, extras.emoji) {
                            match e {
                                Some(e) => extras.media.emoji(ui, e),
                                None => span_ui(ui, m, span, text, weak),
                            }
                        }
                    }
                }
            });
        }
    }
}

/// An `@`-mention as a pill with the person's current name (D66); hovering
/// shows the nick it was written as and the account.
fn mention_ui(ui: &mut egui::Ui, x: &Mention, name: &str, dark: bool) {
    let color = nick_color(&x.nick, dark);
    let t = RichText::new(format!("@{name}"))
        .color(color)
        .background_color(color.gamma_multiply(0.18));
    ui.label(t)
        .on_hover_text(format!("{} (account {})", x.nick, x.account));
}

/// `text`, all or a piece of `span`'s, in its style.
fn span_ui(ui: &mut egui::Ui, m: &ChatMessage, span: &format::Span, text: &str, weak: bool) {
    let mut t = RichText::new(text);
    if span.bold {
        t = t.strong();
    }
    // A family overrides `monospace()`, so bold code stays monospace.
    if span.bold && !span.mono {
        t = t.family(egui::FontFamily::Name(icons::BOLD.into()));
    }
    if span.italic || m.kind == MessageKind::Action {
        t = t.italics();
    }
    if span.underline {
        t = t.underline();
    }
    if span.strike {
        t = t.strikethrough();
    }
    if span.mono {
        t = t.monospace();
    }
    if let Some([r, g, b]) = span.fg {
        t = t.color(Color32::from_rgb(r, g, b));
    } else if weak {
        t = t.weak();
    }
    if let Some([r, g, b]) = span.bg {
        t = t.background_color(Color32::from_rgb(r, g, b));
    }
    if span.link {
        ui.hyperlink_to(t, &span.text);
    } else {
        ui.label(t);
    }
}

fn separator(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
    let font = egui::FontId::proportional(11.0);
    let galley = ui.painter().layout_no_wrap(text.into(), font, color);
    let mid = rect.center();
    let half = galley.size().x / 2.0 + 8.0;
    let stroke = Stroke::new(1.0, color.gamma_multiply(0.5));
    ui.painter()
        .hline(rect.left()..=mid.x - half, mid.y, stroke);
    ui.painter()
        .hline(mid.x + half..=rect.right(), mid.y, stroke);
    ui.painter()
        .galley(mid - galley.size() / 2.0, galley, color);
    ui.add_space(2.0);
}

/// The first `max` characters on one line; only looks that far into `s`.
pub fn snippet(s: &str, max: usize) -> String {
    let mut chars = s.chars().map(|c| if c == '\n' { ' ' } else { c });
    let mut out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, time: i64) -> ChatMessage {
        ChatMessage {
            msgid: Some(id.into()),
            time,
            nick: "a".into(),
            text: "hi".into(),
            ..Default::default()
        }
    }

    #[test]
    fn apply_keeps_heights_in_step() {
        let mut log = ChatLog::default();
        let incoming = [msg("b", 2), msg("a", 1)];
        let ins = |at, from| Edit::Inserted { at, from };
        log.apply(&[ins(0, 0), ins(0, 1)], &incoming);
        assert_eq!(log.rows.len(), 2);
        log.apply(&[Edit::Removed(1), ins(1, 0)], &incoming);
        assert_eq!(log.rows.len(), 2);
        // Hostile or stale edits are skipped, not panicked on.
        log.apply(
            &[Edit::Removed(9), ins(9, 0), ins(0, 9), Edit::Replaced(9)],
            &incoming,
        );
        assert_eq!(log.rows.len(), 2);
        // A run in View's order (ascending) lands in one pass, in place.
        log.rows[0].height = 100.0;
        log.apply(&[ins(0, 0), ins(2, 1), ins(4, 0)], &incoming);
        let h: Vec<f32> = log.rows.iter().map(|r| r.height).collect();
        assert_eq!(h.len(), 5);
        assert_eq!(h[1], 100.0);
    }

    #[test]
    fn snippets() {
        assert_eq!(snippet("a\nb", 10), "a b");
        assert_eq!(snippet("héllo", 2), "hé…");
        assert_eq!(snippet("ab", 2), "ab");
    }
}
