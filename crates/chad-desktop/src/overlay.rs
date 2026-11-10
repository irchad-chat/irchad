//! The game overlay (D74): who's in our voice call, speaking or muted, and the
//! last messages from picked channels, drawn over the focused game in a corner
//! of its window. Display only: clicks go through to the game.
//!
//! This module only draws; `app/overlay.rs` decides when and where, and
//! `platform::overlay` finds the game.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Color32, Layout, Sense, Vec2, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::chatlog::nick_color;
use crate::icons;
use crate::platform::ptt::Combo;
use crate::voice::SPEAKING;

/// The overlay window, in points; it sits in a corner of the game's.
pub const SIZE: Vec2 = vec2(380.0, 520.0);
/// Gap between the game window's edge and the overlay's.
pub const MARGIN: f32 = 16.0;
/// Lines kept; a busy channel can't grow it.
pub const MAX_LINES: usize = 50;
/// Characters of a message shown; the rest is cut.
pub const MAX_TEXT: usize = 200;
/// A line fades out over its last this long.
const FADE_OUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    /// In reading order, for a 2×2 picker.
    pub const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Corner::TopLeft => "Top left",
            Corner::TopRight => "Top right",
            Corner::BottomLeft => "Bottom left",
            Corner::BottomRight => "Bottom right",
        }
    }

    pub(crate) fn right(self) -> bool {
        matches!(self, Corner::TopRight | Corner::BottomRight)
    }

    pub(crate) fn bottom(self) -> bool {
        matches!(self, Corner::BottomLeft | Corner::BottomRight)
    }

    /// Where a `size` box goes in `area`, `MARGIN` in from this corner, kept
    /// inside `area`'s top left when it doesn't fit.
    pub fn place(self, area: egui::Rect, size: Vec2) -> egui::Pos2 {
        let x = if self.right() {
            area.right() - size.x - MARGIN
        } else {
            area.left() + MARGIN
        };
        let y = if self.bottom() {
            area.bottom() - size.y - MARGIN
        } else {
            area.top() + MARGIN
        };
        pos2(x.max(area.left()), y.max(area.top()))
    }

    fn align2(self) -> egui::Align2 {
        match self {
            Corner::TopLeft => egui::Align2::LEFT_TOP,
            Corner::TopRight => egui::Align2::RIGHT_TOP,
            Corner::BottomLeft => egui::Align2::LEFT_BOTTOM,
            Corner::BottomRight => egui::Align2::RIGHT_BOTTOM,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlaySettings {
    pub enabled: bool,
    pub corner: Corner,
    /// Shows and hides it, from any app.
    pub toggle: Combo,
    pub show_voice: bool,
    pub show_messages: bool,
    /// Channels whose messages show, as (network origin, channel key).
    pub channels: Vec<(String, String)>,
    /// Lines on screen at most.
    pub max_lines: usize,
    /// How long a line stays, in seconds.
    pub fade_secs: u32,
    /// Lines stay until newer ones push them out, instead of fading.
    pub keep: bool,
    /// The backgrounds' opacity, 0.0..=1.0.
    pub opacity: f32,
    /// Executable file names that count as games too, for networks without
    /// Discord's list or games it doesn't know.
    pub custom_games: Vec<String>,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            corner: Corner::default(),
            toggle: Combo::default(),
            show_voice: true,
            show_messages: true,
            channels: Vec::new(),
            max_lines: 5,
            fade_secs: 10,
            keep: false,
            opacity: 0.55,
            custom_games: Vec::new(),
        }
    }
}

impl OverlaySettings {
    pub fn shows(&self, network: &str, key: &str) -> bool {
        self.channels.iter().any(|(n, k)| n == network && k == key)
    }

    pub fn toggle_channel(&mut self, network: &str, key: &str) {
        if self.shows(network, key) {
            self.channels.retain(|(n, k)| !(n == network && k == key));
        } else {
            self.channels.push((network.to_owned(), key.to_owned()));
        }
    }

    /// How long a line stays; `None` when lines are kept.
    pub fn fade(&self) -> Option<Duration> {
        (!self.keep).then(|| Duration::from_secs(u64::from(self.fade_secs.clamp(2, 600))))
    }
}

/// Someone in the call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceRow {
    /// For their color.
    pub nick: String,
    /// Their display name.
    pub name: String,
    pub speaking: bool,
    pub muted: bool,
    /// Their profile picture.
    pub avatar: Option<url::Url>,
}

/// Draws `row`'s avatar image in the rect, if it has one and it's loaded;
/// returns whether it did. The overlay draws the initial otherwise.
pub type Avatars<'a> = dyn FnMut(&egui::Ui, egui::Rect, &VoiceRow) -> bool + 'a;

/// A message, already made one bounded line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub at: Instant,
    pub channel: String,
    pub nick: String,
    pub name: String,
    pub text: String,
    /// A `/me`.
    pub action: bool,
}

/// The newest lines, oldest first.
#[derive(Debug, Default)]
pub struct Lines(VecDeque<Line>);

impl Line {
    /// Made one line and cut to `MAX_TEXT`: what the server sent may be
    /// anything (D14).
    pub fn bounded(mut self) -> Self {
        // Newlines become spaces first; then nothing that moves the cursor is left.
        let text = crate::chatlog::snippet(&self.text, MAX_TEXT);
        self.text = chad_core::one_line(&text, MAX_TEXT + 1);
        self.channel = chad_core::one_line(&self.channel, 64);
        self.name = chad_core::one_line(&self.name, 64);
        self
    }
}

impl Lines {
    pub fn push(&mut self, line: Line) {
        if self.0.len() >= MAX_LINES {
            self.0.pop_front();
        }
        self.0.push_back(line.bounded());
    }

    /// Forgets lines older than `fade` (none, without one).
    pub fn expire(&mut self, fade: Option<Duration>, now: Instant) {
        if let Some(fade) = fade {
            self.0
                .retain(|l| now.saturating_duration_since(l.at) < fade);
        }
    }

    pub fn last(&self, n: usize) -> impl DoubleEndedIterator<Item = &Line> {
        self.0.iter().skip(self.0.len().saturating_sub(n))
    }
}

/// What to draw.
pub struct View<'a> {
    pub corner: Corner,
    pub opacity: f32,
    /// How long lines stay; `None` keeps them.
    pub fade: Option<Duration>,
    pub voice: &'a [VoiceRow],
    /// Oldest first.
    pub lines: Vec<&'a Line>,
}

/// How opaque a line `age` old is: whole until its last `FADE_OUT`, then less.
/// Always whole when lines are kept.
fn alpha(age: Duration, fade: Option<Duration>) -> f32 {
    let Some(fade) = fade else {
        return 1.0;
    };
    let left = fade.saturating_sub(age);
    if left.is_zero() {
        0.0
    } else {
        (left.as_secs_f32() / FADE_OUT.as_secs_f32()).min(1.0)
    }
}

/// Draws the overlay in the corner of `ui`'s window, which should be the
/// overlay's own. The voice block sits nearest the corner, messages beyond it,
/// oldest first. Returns whether anything is fading, so the caller keeps
/// repainting.
pub fn ui(ui: &mut egui::Ui, view: &View, now: Instant, avatars: &mut Avatars) -> bool {
    let bg = Color32::from_black_alpha((view.opacity.clamp(0.0, 1.0) * 255.0) as u8);
    let width = SIZE.x - 2.0 * MARGIN;
    let c = view.corner;
    let mut fading = false;
    let mut voice = |ui: &mut egui::Ui| {
        if !view.voice.is_empty() {
            block(ui, bg, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for r in view.voice {
                    voice_row(ui, r, avatars);
                }
            });
        }
    };
    egui::Area::new(egui::Id::unique("game-overlay"))
        .anchor(c.align2(), Vec2::ZERO)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            ui.set_max_width(width);
            // Left-aligned in every corner: right-aligned, egui mirrors the
            // rows inside the boxes too.
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                if !c.bottom() {
                    voice(ui);
                }
                for l in &view.lines {
                    let a = alpha(now.saturating_duration_since(l.at), view.fade);
                    if a <= 0.0 {
                        continue;
                    }
                    fading |= a < 1.0;
                    ui.scope(|ui| {
                        ui.multiply_opacity(a);
                        block(ui, bg, |ui| line(ui, l, width));
                    });
                }
                if c.bottom() {
                    voice(ui);
                }
            });
        });
    fading
}

fn block(ui: &mut egui::Ui, bg: Color32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, add);
}

fn voice_row(ui: &mut egui::Ui, r: &VoiceRow, avatars: &mut Avatars) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        let face = egui::Rect::from_center_size(rect.center(), vec2(16.0, 16.0));
        if !avatars(ui, face, r) {
            let p = ui.painter();
            p.circle_filled(
                rect.center(),
                8.0,
                nick_color(&r.nick, true).gamma_multiply(0.8),
            );
            let initial = r
                .name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                initial,
                egui::FontId::proportional(10.0),
                Color32::BLACK,
            );
        }
        if r.speaking {
            ui.painter()
                .circle_stroke(rect.center(), 9.5, (2.0, SPEAKING));
        }
        let (color, strong) = if r.speaking {
            (Color32::from_gray(250), true)
        } else {
            (Color32::from_gray(200), false)
        };
        let mut text = egui::RichText::new(&r.name).color(color);
        if strong {
            text = text.strong();
        }
        ui.add(egui::Label::new(text).truncate());
        if r.muted {
            ui.label(egui::RichText::new(icons::MICROPHONE_SLASH).color(Color32::from_gray(160)));
        }
    });
}

fn line(ui: &mut egui::Ui, l: &Line, width: f32) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let plain = |c| TextFormat::simple(font.clone(), c);
    let mut job = LayoutJob::default();
    job.append(
        &format!("{} ", l.channel),
        0.0,
        plain(Color32::from_gray(150)),
    );
    let name = if l.action {
        format!("* {} ", l.name)
    } else {
        format!("{}: ", l.name)
    };
    job.append(&name, 0.0, plain(nick_color(&l.nick, true)));
    job.append(&l.text, 0.0, plain(Color32::from_gray(235)));
    job.wrap.max_width = width - 16.0;
    job.wrap.max_rows = 4;
    ui.add(egui::Label::new(job).selectable(false));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_place_inside() {
        let area = egui::Rect::from_min_size(pos2(100.0, 50.0), vec2(1000.0, 800.0));
        let s = vec2(300.0, 400.0);
        assert_eq!(Corner::TopLeft.place(area, s), pos2(116.0, 66.0));
        assert_eq!(Corner::TopRight.place(area, s), pos2(784.0, 66.0));
        assert_eq!(Corner::BottomLeft.place(area, s), pos2(116.0, 434.0));
        assert_eq!(Corner::BottomRight.place(area, s), pos2(784.0, 434.0));
        // Too small: pinned to the top left.
        let tiny = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
        assert_eq!(Corner::BottomRight.place(tiny, s), pos2(0.0, 0.0));
    }

    #[test]
    fn lines_are_bounded_and_expire() {
        let t0 = Instant::now();
        let mut lines = Lines::default();
        for i in 0..MAX_LINES + 10 {
            lines.push(Line {
                at: t0 + Duration::from_secs(i as u64),
                channel: "#c".into(),
                nick: "n".into(),
                name: "n".into(),
                text: "x\n".repeat(500),
                action: false,
            });
        }
        assert_eq!(lines.0.len(), MAX_LINES);
        assert!(
            lines
                .0
                .iter()
                .all(|l| l.text.chars().count() <= MAX_TEXT + 1)
        );
        assert!(lines.0.iter().all(|l| !l.text.contains('\n')));
        assert_eq!(lines.last(3).count(), 3);
        // Kept: nothing goes but what MAX_LINES pushed out.
        lines.expire(None, t0 + Duration::from_secs(3600));
        assert_eq!(lines.0.len(), MAX_LINES);
        lines.expire(Some(Duration::from_secs(5)), t0 + Duration::from_secs(60));
        assert_eq!(lines.0.len(), 4);
    }

    #[test]
    fn lines_fade_at_the_end() {
        let fade = Some(Duration::from_secs(10));
        assert!((alpha(Duration::ZERO, fade) - 1.0).abs() < f32::EPSILON);
        assert!((alpha(Duration::from_secs(9), fade) - 0.5).abs() < 0.01);
        assert!(alpha(Duration::from_secs(10), fade) <= 0.0);
        assert!((alpha(Duration::from_secs(3600), None) - 1.0).abs() < f32::EPSILON);
        let mut s = OverlaySettings::default();
        assert_eq!(s.fade(), Some(Duration::from_secs(10)));
        s.keep = true;
        assert_eq!(s.fade(), None);
    }

    #[test]
    fn old_settings_still_load() {
        let s: OverlaySettings = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(s.enabled && s.show_voice);
        assert_eq!(s.toggle, Combo::default());
        let mut s = s;
        s.toggle_channel("net", "#a");
        assert!(s.shows("net", "#a"));
        s.toggle_channel("net", "#a");
        assert!(!s.shows("net", "#a"));
    }
}
