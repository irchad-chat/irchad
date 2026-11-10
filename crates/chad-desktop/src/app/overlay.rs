//! The game overlay (D74): when it shows, where, what's in it, and its
//! settings page. Drawing is `crate::overlay`'s; finding the game is
//! `platform::overlay`'s.

use std::sync::atomic::Ordering::Relaxed;

use super::*;
use crate::overlay::{self, Corner, Line, View as OverlayView, VoiceRow};
use crate::platform::overlay::{Focus, Target};

/// How long "Preview" shows it without a game.
const PREVIEW: Duration = Duration::from_secs(10);
/// The overlay window's; Windows finds it by this.
const TITLE: &str = "IrChad overlay";

/// A message the overlay can show at all: not an edit record (applied to its
/// message already), not deleted, not a send that failed.
fn overlay_shows(m: &ChatMessage) -> bool {
    m.edits.is_none() && !m.redacted && !matches!(m.status, chad_core::SendStatus::Failed(_))
}

fn overlay_line(
    profiles: &HashMap<String, chad_core::view::Profile>,
    channel: &str,
    m: &ChatMessage,
) -> Line {
    Line {
        at: Instant::now(),
        channel: channel.to_owned(),
        nick: m.nick.clone(),
        name: chatlog::shown(profiles, &m.nick).to_owned(),
        text: format::strip(&m.text),
        action: m.kind == MessageKind::Action,
    }
    .bounded()
}

impl App {
    /// Starts or stops watching for a focused game with the settings, and
    /// forgets old lines. Returns whether the overlay is up.
    pub(super) fn overlay_logic(&mut self, ctx: &egui::Context) -> bool {
        let s = &self.settings.overlay;
        let want = s.enabled && platform::overlay::support().is_ok();
        let stale = self
            .focus
            .as_ref()
            .is_some_and(|f| f.custom != s.custom_games);
        if want != self.focus.is_some() || stale {
            self.focus = want.then(|| {
                let ctx = ctx.clone();
                Focus::start(self.games.subscribe(), s.custom_games.clone(), move || {
                    ctx.request_repaint();
                })
            });
        }
        self.overlay_lines.expire(s.fade(), Instant::now());
        self.overlay_target().is_some()
    }

    fn previewing(&self) -> bool {
        self.overlay_preview.is_some_and(|t| t.elapsed() < PREVIEW)
    }

    /// The game it's over, if it's up.
    fn overlay_target(&self) -> Option<Target> {
        let s = &self.settings.overlay;
        if !s.enabled || platform::overlay::support().is_err() {
            return None;
        }
        let focused = self.focus.as_ref().and_then(|f| f.rx.borrow().clone());
        if self.previewing() {
            return Some(focused.unwrap_or(Target {
                game: "Preview".into(),
                rect: None,
                dpi: None,
            }));
        }
        focused.filter(|_| self.overlay_on.load(Relaxed))
    }

    /// Who's in our call, for the overlay.
    fn overlay_voice(&self) -> Vec<VoiceRow> {
        let Some((network, VoiceState::Connected { channel })) = &self.view.call else {
            return Vec::new();
        };
        let Some(call) = self.voice.call() else {
            return Vec::new();
        };
        let n = self.net_idx(network).map(|i| &self.view.nets[i]);
        let roster = n
            .and_then(|n| n.buffer(&key(channel)).map(|bi| &n.buffers[bi]))
            .and_then(|b| b.members.as_deref())
            .unwrap_or_default();
        call.participants
            .iter()
            .map(|p| {
                let nick = voice::nick_of(roster, &p.identity);
                let name = n.map_or(nick.as_str(), |n| chatlog::shown(&n.profiles, &nick));
                let avatar = n
                    .and_then(|n| n.profiles.get(&key(&nick)))
                    .and_then(|p| p.avatar.clone());
                VoiceRow {
                    name: chad_core::one_line(name, 64),
                    nick: nick.clone(),
                    speaking: p.speaking,
                    muted: p.muted,
                    avatar,
                }
            })
            .collect()
    }

    /// A fresh message in a channel the overlay shows, for fading lines.
    /// Ours count too; others' only while fresh (not history), like
    /// notifications.
    pub(super) fn overlay_message(&mut self, network: &str, channel: &str, m: &ChatMessage) {
        let s = &self.settings.overlay;
        if !s.enabled || !s.shows(network, &key(channel)) || !overlay_shows(m) {
            return;
        }
        if !m.own && !chad_core::view::notifiable(m) {
            return;
        }
        let line = match self.net_idx(network) {
            Some(i) => overlay_line(&self.view.nets[i].profiles, channel, m),
            None => overlay_line(&HashMap::new(), channel, m),
        };
        self.overlay_lines.push(line);
    }

    /// The newest `n` messages across the picked channels' history, oldest
    /// first: what "Keep them on screen" shows. Read from the buffers each
    /// time, so it has what was there before the overlay came up, ours, and
    /// edits and deletions.
    fn overlay_kept(&self, n: usize) -> Vec<Line> {
        let mut picked: Vec<(i64, Line)> = Vec::new();
        for (network, k) in &self.settings.overlay.channels {
            let Some(net) = self.net_idx(network).map(|i| &self.view.nets[i]) else {
                continue;
            };
            let Some(b) = net.buffer(k).map(|i| &net.buffers[i]) else {
                continue;
            };
            let newest = b.msgs.iter().rev().filter(|m| overlay_shows(m)).take(n);
            picked.extend(newest.map(|m| (m.time, overlay_line(&net.profiles, &b.info.target, m))));
        }
        picked.sort_by_key(|(t, _)| *t);
        let skip = picked.len().saturating_sub(n);
        picked.into_iter().skip(skip).map(|(_, l)| l).collect()
    }

    /// The overlay's own window, over the game, while it's up.
    pub(super) fn overlay_ui(&mut self, ctx: &egui::Context) {
        let Some(target) = self.overlay_target() else {
            return;
        };
        let s = &self.settings.overlay;
        let voice = if s.show_voice {
            self.overlay_voice()
        } else {
            Vec::new()
        };
        let sample;
        let max = s.max_lines.clamp(1, 20);
        let kept = if s.show_messages && s.keep {
            self.overlay_kept(max)
        } else {
            Vec::new()
        };
        let mut lines: Vec<&Line> = if !s.show_messages {
            Vec::new()
        } else if s.keep {
            kept.iter().collect()
        } else {
            self.overlay_lines.last(max).collect()
        };
        let preview = self.previewing();
        if preview && lines.is_empty() && s.show_messages {
            sample = Line {
                at: self.overlay_preview.unwrap_or_else(Instant::now),
                channel: "#general".into(),
                nick: "chad".into(),
                name: "chad".into(),
                text: "Messages from the channels you pick show here.".into(),
                action: false,
            };
            lines.push(&sample);
        }

        // Points on the game's monitor. Without its DPI (X11), the main
        // window's scale stands in.
        let scale = ctx.zoom_factor()
            * target.dpi.map_or_else(
                || ctx.native_pixels_per_point().unwrap_or(1.0),
                |d| d as f32 / 96.0,
            );
        let area = match target.rect {
            Some([x, y, w, h]) => egui::Rect::from_min_size(
                egui::pos2(x as f32 / scale, y as f32 / scale),
                vec2(w as f32, h as f32) / scale,
            ),
            None => {
                let monitor = ctx
                    .input(|i| i.viewport().monitor_size)
                    .unwrap_or(vec2(1280.0, 720.0));
                egui::Rect::from_min_size(egui::Pos2::ZERO, monitor)
            }
        };
        let at = s.corner.place(area, overlay::SIZE);
        // Where it goes in pixels, when we know the game's monitor (Windows):
        // placed by `finish_window`, since egui would scale by the wrong one.
        let place = target.rect.filter(|_| target.dpi.is_some()).map(|_| {
            let px = |v: f32| (v * scale).round() as i32;
            [px(at.x), px(at.y), px(overlay::SIZE.x), px(overlay::SIZE.y)]
        });
        let view = OverlayView {
            corner: s.corner,
            opacity: s.opacity,
            fade: if preview && !s.keep {
                Some(PREVIEW)
            } else {
                s.fade()
            },
            voice: &voice,
            lines,
        };
        let mut builder = egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_app_id(platform::APP_ID)
            .with_inner_size(overlay::SIZE)
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_mouse_passthrough(true)
            .with_taskbar(false)
            .with_active(false);
        if place.is_none() {
            builder = builder.with_position(at);
        }
        let now = Instant::now();
        let mut fading = false;
        // Avatars come from the call's network's images, like the sidebar's.
        let auto = self.settings.load_media;
        let net = self.view.call.as_ref().and_then(|(n, _)| self.net_idx(n));
        let mut media = net.map(|i| &mut self.view.nets[i].ui.media);
        let mut avatars =
            |ui: &egui::Ui, rect: egui::Rect, r: &VoiceRow| match (media.as_deref_mut(), &r.avatar)
            {
                (Some(m), Some(url)) => m.avatar_image(ui, rect, url, auto),
                _ => false,
            };
        // The window is made inside this call, when it's new.
        let _hook = platform::overlay::CreateHook::new(TITLE);
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("game-overlay"),
            builder,
            |ui, class| {
                // Without real windows it would land in the main one.
                if class != egui::ViewportClass::EmbeddedWindow {
                    fading = overlay::ui(ui, &view, now, &mut avatars);
                }
            },
        );
        let unsettled = platform::overlay::finish_window(TITLE, place);
        if fading || unsettled {
            ctx.request_repaint_after(Duration::from_millis(50));
        } else if (view.fade.is_some() && !view.lines.is_empty()) || preview {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    /// The "Game Overlay" settings page.
    pub(super) fn overlay_page(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let before = self.settings.overlay.clone();
        let support = platform::overlay::support();
        let focused = self.focus.as_ref().and_then(|f| f.rx.borrow().clone());
        // Without Discord's list only "Your games" are spotted; say why.
        let no_list = self.games.borrow().is_none().then(|| {
            let why = self
                .games_error
                .as_deref()
                .map_or_else(String::new, |e| format!(" ({e})"));
            format!(
                "No game list from your server yet{why}, so only the games you add below are spotted."
            )
        });
        let hidden = !self.overlay_on.load(Relaxed);
        let binding = self.binding_ptt == Some(KeyAction::Overlay);
        let mut bind_clicked = false;
        let mut preview = false;
        let s = &mut self.settings.overlay;
        egui::Grid::new("overlay")
            .num_columns(2)
            .min_col_width(settings::LABEL_W)
            .show(ui, |ui| {
                ui.label("Overlay");
                ui.vertical(|ui| {
                    ui.checkbox(&mut s.enabled, "Show an overlay over games");
                    let (text, warn) = match (&support, &focused) {
                        (Err(why), _) => ((*why).to_owned(), true),
                        _ if !s.enabled => (String::new(), false),
                        (Ok(()), Some(t)) => (format!("Over {} now.", t.game), false),
                        (Ok(()), None) if no_list.is_some() => {
                            (no_list.clone().unwrap_or_default(), true)
                        }
                        (Ok(()), None) => (
                            "It shows while a game Discord knows (or one of yours below) \
                             has focus. It doesn't show over exclusive fullscreen."
                                .into(),
                            false,
                        ),
                    };
                    if !text.is_empty() {
                        let t = RichText::new(text).small();
                        ui.label(if warn {
                            t.color(ui.visuals().warn_fg_color)
                        } else {
                            t.weak()
                        });
                    }
                    if s.enabled && hidden {
                        let text = format!(
                            "Hidden with {}. Press it again to show it.",
                            s.toggle.name()
                        );
                        ui.label(
                            RichText::new(text)
                                .small()
                                .color(ui.visuals().warn_fg_color),
                        );
                    }
                });
                ui.end_row();

                let l = ui.label("Show or hide");
                ui.horizontal(|ui| {
                    let text = if binding {
                        "Press a key combination…".to_owned()
                    } else {
                        s.toggle.name()
                    };
                    bind_clicked = ui.button(text).labelled_by(l.id).clicked();
                    if binding && let Some(c) = captured(ui) {
                        s.toggle = c;
                    }
                });
                ui.end_row();

                ui.label("Corner");
                egui::Grid::new("overlay-corner").show(ui, |ui| {
                    for (i, c) in Corner::ALL.into_iter().enumerate() {
                        ui.selectable_value(&mut s.corner, c, c.name());
                        if i % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
                ui.end_row();

                ui.label("Shows");
                ui.vertical(|ui| {
                    ui.checkbox(&mut s.show_voice, "Who's in voice, and who's speaking");
                    ui.checkbox(&mut s.show_messages, "Messages from the channels below");
                });
                ui.end_row();

                let l = ui.label("Messages");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.max_lines).range(1..=20))
                            .labelled_by(l.id);
                        if s.keep {
                            ui.label("newest, always on screen");
                        } else {
                            ui.label("at most, each for");
                            let fade =
                                ui.add(egui::DragValue::new(&mut s.fade_secs).range(2..=600));
                            let l = ui.label("seconds");
                            fade.labelled_by(l.id);
                        }
                    });
                    ui.checkbox(&mut s.keep, "Keep them on screen")
                        .on_hover_text(
                            "Messages stay until newer ones push them out, instead of fading.",
                        );
                });
                ui.end_row();

                let l = ui.label("Background");
                ui.add(
                    egui::Slider::new(&mut s.opacity, 0.0..=1.0)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                )
                .labelled_by(l.id);
                ui.end_row();

                ui.label("");
                preview = ui
                    .add_enabled(support.is_ok(), egui::Button::new("Preview"))
                    .on_hover_text("Shows it for 10 seconds, without a game.")
                    .clicked();
                ui.end_row();
            });

        ui.add_space(8.0);
        ui.strong("Channels");
        ui.weak("Right-click a channel in the sidebar to add it here too.");
        let many = self.view.nets.len() > 1;
        let mut flip = None;
        for n in &self.view.nets {
            let chans: Vec<_> = n
                .buffers
                .iter()
                .filter(|b| b.info.kind == BufferKind::Channel)
                .collect();
            if chans.is_empty() {
                continue;
            }
            if many {
                ui.label(RichText::new(&n.info.name).small().weak());
            }
            ui.horizontal_wrapped(|ui| {
                for b in chans {
                    let mut on = s.shows(&n.info.network, &b.key);
                    if ui.checkbox(&mut on, &b.info.target).changed() {
                        flip = Some((n.info.network.clone(), b.key.clone()));
                    }
                }
            });
        }
        if let Some((n, k)) = flip {
            s.toggle_channel(&n, &k);
        }

        ui.add_space(8.0);
        ui.strong("Your games");
        ui.weak("Executable names that count as games too, like MyGame.exe.");
        let mut remove = None;
        for (i, g) in s.custom_games.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(g).desired_width(240.0));
                if ui.small_button(icons::X).tip("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            s.custom_games.remove(i);
        }
        if ui.button("Add a game").clicked() {
            s.custom_games.push(String::new());
        }

        if bind_clicked {
            self.binding_ptt = (!binding).then_some(KeyAction::Overlay);
        } else if binding && self.settings.overlay.toggle != before.toggle {
            self.binding_ptt = None;
        }
        if preview {
            self.overlay_preview = Some(Instant::now());
        }
        let s = &self.settings.overlay;
        if (s.enabled, s.toggle) != (before.enabled, before.toggle) {
            self.restart_ptt(ctx);
        }
    }
}

/// The first key or side button pressed this frame that can be bound, with
/// the modifiers held. A bare key needs at least one, so typing doesn't toggle.
fn captured(ui: &egui::Ui) -> Option<platform::ptt::Combo> {
    use platform::ptt::{Combo, Mods, PttBind, keymap};
    let (bind, mods) = ui.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if keymap(*key).is_some() => Some((PttBind::Key(*key), *modifiers)),
            egui::Event::PointerButton {
                button: egui::PointerButton::Extra1,
                pressed: true,
                modifiers,
                ..
            } => Some((PttBind::Mouse4, *modifiers)),
            egui::Event::PointerButton {
                button: egui::PointerButton::Extra2,
                pressed: true,
                modifiers,
                ..
            } => Some((PttBind::Mouse5, *modifiers)),
            _ => None,
        })
    })?;
    let mods = Mods::from_egui(mods);
    let bare_key =
        matches!(bind, PttBind::Key(k) if !k.name().starts_with('F') || k.name().len() == 1);
    (mods != Mods::NONE || !bare_key).then_some(Combo { mods, bind })
}
