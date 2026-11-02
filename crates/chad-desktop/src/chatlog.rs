//! The virtualized, variable-height message list (design: `docs/notes/egui-chatlog.md`).
//!
//! Heights are estimated until a row is laid out at the current width;
//! `prefix[i]` is the y of row i. Only rows in the viewport are laid out, and
//! any height change above the viewport shifts the scroll offset by the same
//! amount, so what you're reading stays put.

use std::collections::HashMap;

use chad_core::view::Edit;
use chad_core::{ChatMessage, Crypt, MessageKind, SendStatus};
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
    pub media: &'a mut Media,
    /// The network's origin, to tell its own uploads apart.
    pub network: &'a str,
    /// Load images, previews and avatars without asking.
    pub auto_media: bool,
    /// A DM, where unencrypted messages are marked.
    pub dm: bool,
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
    Delete {
        msgid: String,
    },
    Jump {
        msgid: String,
    },
    OpenDm(String),
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
    heights: Vec<f32>,
    prefix: Vec<f32>,
    prefix_dirty: bool,
    measured: Vec<bool>,
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

fn index_of(msgs: &[ChatMessage], msgid: &str) -> Option<usize> {
    msgs.iter().rposition(|m| m.msgid.as_deref() == Some(msgid))
}

fn day(ms: i64) -> Option<jiff::civil::Date> {
    let ts = jiff::Timestamp::from_millisecond(ms).ok()?;
    Some(ts.to_zoned(TimeZone::system()).date())
}

pub fn clock(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|t| t.to_zoned(TimeZone::system()).strftime("%H:%M").to_string())
        .unwrap_or_default()
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
        for e in edits {
            match *e {
                Edit::Replaced(i) => {
                    if let Some(m) = self.measured.get_mut(i) {
                        *m = false;
                    }
                }
                Edit::Removed(i) if i < self.heights.len() => {
                    self.rebuild_prefix();
                    if self.prefix[i] < self.last_top && !self.at_bottom {
                        self.shift -= self.heights[i];
                    }
                    self.heights.remove(i);
                    self.measured.remove(i);
                    self.prefix_dirty = true;
                }
                Edit::Inserted { at, from } if at <= self.heights.len() => {
                    let Some(m) = incoming.get(from) else {
                        continue;
                    };
                    let h = self.estimate(m);
                    self.rebuild_prefix();
                    if self.prefix[at] < self.last_top && !self.at_bottom {
                        self.shift += h;
                    }
                    self.heights.insert(at, h);
                    self.measured.insert(at, false);
                    self.prefix_dirty = true;
                }
                _ => {}
            }
        }
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
        if !self.prefix_dirty && self.prefix.len() == self.heights.len() + 1 {
            return;
        }
        self.prefix.clear();
        let mut y = 0.0;
        self.prefix.push(0.0);
        for h in &self.heights {
            y += h;
            self.prefix.push(y);
        }
        self.prefix_dirty = false;
    }

    fn set_width(&mut self, msgs: &[ChatMessage], width: f32, line: f32) {
        // Out of step with the messages (a missed edit): start the layout over.
        let synced = self.heights.len() == msgs.len();
        if synced && (width - self.width).abs() < 0.5 && (line - self.line).abs() < 0.01 {
            return;
        }
        self.width = width;
        self.line = line;
        self.heights = msgs.iter().map(|m| self.estimate(m)).collect();
        self.measured = vec![false; msgs.len()];
        self.prefix_dirty = true;
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
        msgs: &[ChatMessage],
        own_nick: &str,
        highlights: bool,
        extras: &mut Extras,
    ) -> Output {
        let line = ui.text_style_height(&egui::TextStyle::Body) + 2.0;
        self.set_width(msgs, ui.available_width() - 16.0, line);
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
            let mut changed = false;
            let mut rows = Vec::new();
            while i < msgs.len() && y < viewport.max.y {
                let rect = Rect::from_min_size(
                    origin + vec2(GUTTER, y),
                    vec2(self.width - GUTTER, f32::INFINITY),
                );
                let prev = i.checked_sub(1).map(|p| &msgs[p]);
                let m = &msgs[i];
                let unread = m.msgid.is_some() && self.unread_from == m.msgid;
                let parent = m
                    .reply_to
                    .as_deref()
                    .and_then(|id| index_of(msgs, id))
                    .map(|p| &msgs[p]);
                let resp = ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(rect)
                        .layout(Layout::top_down(Align::Min))
                        .id_salt(("row", m.msgid.as_deref(), m.local_id)),
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
                if !self.measured[i] || (h - self.heights[i]).abs() > 0.5 {
                    let d = h - self.heights[i];
                    if self.prefix[i] < viewport.min.y && d.abs() > 0.5 {
                        compensate += d;
                    }
                    self.heights[i] = h;
                    self.measured[i] = true;
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
                Some((id, _)) => {
                    let id = id.clone();
                    rows.iter()
                        .copied()
                        .find(|(i, _)| msgs[*i].msgid.as_ref() == Some(&id))
                }
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
        if compensate != 0.0 && !self.at_bottom {
            self.pending_offset = Some(viewport.min.y + compensate);
            ui.ctx()
                .request_discard("chat log row heights changed above viewport");
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
    m: &ChatMessage,
    prev: Option<&ChatMessage>,
    parent: Option<&ChatMessage>,
    unread: bool,
    own_nick: &str,
    highlights: bool,
    dark: bool,
    extras: &mut Extras,
    actions: &mut Vec<Action>,
) {
    let new_day = prev.is_none_or(|p| day(p.time) != day(m.time));
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
                    let who = match &m.account {
                        Some(a) if name != m.nick => format!("{} · account {a}", m.nick),
                        Some(a) => format!("account {a}"),
                        None => format!("{} · not logged in", m.nick),
                    };
                    if ui
                        .add(egui::Label::new(label).sense(Sense::click()))
                        .on_hover_text(who)
                        .clicked()
                        && !m.own
                    {
                        actions.push(Action::OpenDm(m.nick.clone()));
                    }
                    ui.label(RichText::new(clock(m.time)).weak().small());
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
                    Some(p) if p.redacted => {
                        format!(
                            "{} {}: (deleted)",
                            icons::ARROW_BEND_DOWN_RIGHT,
                            shown(extras.profiles, &p.nick)
                        )
                    }
                    Some(p) => format!(
                        "{} {}: {}",
                        icons::ARROW_BEND_DOWN_RIGHT,
                        shown(extras.profiles, &p.nick),
                        snippet(&format::strip(&p.text), 80)
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
                body_ui(ui, m, name, dark);
                links_ui(ui, m, extras);
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
                        let chip = egui::Button::new(format!("{} {}", r.emoji, r.nicks.len()))
                            .small()
                            .selected(mine);
                        let who: Vec<&str> =
                            r.nicks.iter().map(|n| shown(extras.profiles, n)).collect();
                        let resp = ui.add(chip).on_hover_text(who.join(", "));
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
                        if m.own && ui.small_button(icons::TRASH).tip("Delete").clicked() {
                            actions.push(Action::Delete { msgid: id.clone() });
                        }
                    });
                });
        })
        .response
        .rect
}

/// Images inline, videos and files as cards, and a preview of the first page.
fn links_ui(ui: &mut egui::Ui, m: &ChatMessage, extras: &mut Extras) {
    if m.kind == MessageKind::Notice {
        return;
    }
    let (mut images, mut previewed) = (0, false);
    for span in format::parse(&m.text).into_iter().filter(|s| s.link) {
        match media::kind(&span.text, extras.network) {
            Some(Kind::Image) if images < MAX_IMAGES => {
                images += 1;
                extras.media.image(ui, &span.text, extras.auto_media);
            }
            Some(k @ (Kind::Video | Kind::File)) => Media::card(ui, &span.text, k),
            Some(Kind::Page) if !previewed => {
                previewed = true;
                extras.media.link_preview(ui, &span.text, extras.auto_media);
            }
            _ => {}
        }
    }
}

fn body_ui(ui: &mut egui::Ui, m: &ChatMessage, name: &str, dark: bool) {
    let weak = m.status == SendStatus::Pending || m.kind == MessageKind::Notice;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if m.kind == MessageKind::Action {
            ui.label(
                RichText::new(format!("{name} "))
                    .italics()
                    .strong()
                    .color(nick_color(&m.nick, dark)),
            );
        }
        for span in format::parse(&m.text) {
            let mut t = RichText::new(&span.text);
            if span.bold {
                t = t.strong();
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
    });
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

pub fn snippet(s: &str, max: usize) -> String {
    let one_line = s.replace('\n', " ");
    match one_line.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &one_line[..i]),
        None => one_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chad_core::{MessageKind, SendStatus};

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
            reactions: vec![],
            mention: false,
            own: false,
            crypt: Crypt::Plain,
        }
    }

    #[test]
    fn apply_keeps_heights_in_step() {
        let mut log = ChatLog::default();
        let incoming = [msg("b", 2), msg("a", 1)];
        let ins = |at, from| Edit::Inserted { at, from };
        log.apply(&[ins(0, 0), ins(0, 1)], &incoming);
        assert_eq!((log.heights.len(), log.measured.len()), (2, 2));
        log.apply(&[Edit::Removed(1), ins(1, 0)], &incoming);
        assert_eq!(log.heights.len(), 2);
        // Hostile or stale edits are skipped, not panicked on.
        log.apply(
            &[Edit::Removed(9), ins(9, 0), ins(0, 9), Edit::Replaced(9)],
            &incoming,
        );
        assert_eq!(log.heights.len(), 2);
    }

    #[test]
    fn snippets() {
        assert_eq!(snippet("a\nb", 10), "a b");
        assert_eq!(snippet("héllo", 2), "hé…");
    }
}
