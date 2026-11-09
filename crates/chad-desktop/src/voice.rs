//! Voice in the UI: the glue that lets the core drive `chad-media`, the call
//! panel, who's-in-voice rows in the sidebar, and audio settings.

use std::collections::HashMap;
use std::time::Duration;

use chad_core::view::key;
use chad_core::{MediaEvent, Member, Sound, VoiceState};
use chad_media::{AudioSettings, InputMode, Quality, Voice};
use egui::{Color32, RichText, Sense, vec2};
use tokio::sync::mpsc;

use crate::chatlog::{self, Profile, shown};
use crate::icons::{self, Tip};
use crate::media::Media;
use crate::platform::ptt::{Action, Ptt, PttBind, PttStatus, keymap};
use crate::settings::{self, Settings};

const SPEAKING: Color32 = Color32::from_rgb(35, 165, 90);
pub const ONLINE: Color32 = SPEAKING;
pub const AWAY: Color32 = Color32::from_rgb(240, 178, 50);
/// A voice member's avatar, and the speaking ring drawn inside its edge.
const AVATAR: f32 = 24.0;
const RING: f32 = 2.0;
const DANGER: Color32 = Color32::from_rgb(218, 55, 60);
/// Whispers, to tell them from talking to everyone.
const WHISPER: Color32 = Color32::from_rgb(160, 130, 235);

/// The core decides when to join and leave; this connects.
pub struct Glue(pub Voice);

impl chad_core::Media for Glue {
    fn join(&self, url: String, token: String) -> mpsc::UnboundedReceiver<MediaEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let voice = self.0.clone();
        tokio::spawn(async move {
            let mut call = match voice.join(&url, &token).await {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(MediaEvent::Ended(Some(e)));
                    return;
                }
            };
            let _ = tx.send(MediaEvent::Connected);
            // Dropping `call` hangs up: when the core lets go, or it ended.
            tokio::select! {
                () = tx.closed() => {}
                r = call.ended() => {
                    let _ = tx.send(MediaEvent::Ended(r));
                }
            }
        });
        rx
    }
}

pub enum PanelAction {
    Leave,
    Share,
    StopShare,
    Camera,
    StopCamera,
    /// Watch our own share.
    Preview,
    /// The soundboard opened: list the network's sounds again.
    Soundboard,
    /// Play this soundboard file.
    Sound(String),
}

/// Bottom of the channel list while in a call (on any network).
pub fn panel(
    ui: &mut egui::Ui,
    voice: &Voice,
    state: &VoiceState,
    network: &str,
    ptt: Option<&Ptt>,
    sounds: Option<&[Sound]>,
    audio: &mut AudioSettings,
) -> Option<PanelAction> {
    let call = voice.call();
    let (text, color, channel) = match state {
        VoiceState::Connected { channel } if call.as_ref().is_some_and(|c| c.reconnecting) => {
            ("Reconnecting…", ui.visuals().warn_fg_color, channel)
        }
        VoiceState::Connected { channel } => ("Voice connected", SPEAKING, channel),
        VoiceState::Connecting { channel } => ("Connecting…", ui.visuals().warn_fg_color, channel),
        VoiceState::Idle => return None,
    };
    let mut action = None;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(text).strong().color(color));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(c) = &call {
                let (dot, tip) = match c.quality {
                    Quality::Excellent => (SPEAKING, "Excellent connection"),
                    Quality::Good => (AWAY, "Good connection"),
                    Quality::Poor | Quality::Lost => (DANGER, "Poor connection"),
                    Quality::Unknown => (Color32::GRAY, "Measuring…"),
                };
                if let Some(rtt) = c.rtt_ms {
                    ui.label(RichText::new(format!("{rtt} ms")).small().weak());
                }
                let (r, resp) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, dot);
                resp.on_hover_text(tip);
            }
        });
    });
    ui.label(
        RichText::new(format!("{channel} · {network}"))
            .small()
            .weak(),
    );
    if let Some(p) = ptt.filter(|p| p.focused_only()) {
        for (action, bind) in &p.binds {
            ui.label(
                RichText::new(format!(
                    "{} ({}) only works here",
                    action.label(),
                    bind.name()
                ))
                .small()
                .color(ui.visuals().warn_fg_color),
            );
        }
    }
    if voice.sharing() {
        let watchers = call
            .as_ref()
            .and_then(|c| c.participants.iter().find(|p| p.local))
            .map_or(0, |p| p.watchers);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "{} Sharing your screen · {} {watchers} watching",
                    icons::SCREENCAST,
                    icons::EYE
                ))
                .small()
                .color(SPEAKING),
            );
            if ui
                .small_button("Preview")
                .on_hover_text("See what you're sending")
                .clicked()
            {
                action = Some(PanelAction::Preview);
            }
        });
    }
    ui.horizontal(|ui| {
        let (muted, deaf) = (voice.muted(), voice.deafened());
        let mic = if muted {
            icons::MICROPHONE_SLASH
        } else {
            icons::MICROPHONE
        };
        let r = ui
            .add(egui::Button::selectable(
                muted,
                RichText::new(mic).size(16.0),
            ))
            .tip(if muted { "Unmute" } else { "Mute" });
        if r.clicked() {
            // Unmuting while deafened undeafens too, like Discord.
            if deaf {
                voice.set_deafened(false);
                voice.set_muted(false);
            } else {
                voice.set_muted(!muted);
            }
        }
        let r = ui
            .add(egui::Button::selectable(
                deaf,
                RichText::new(icons::HEADPHONES).size(16.0),
            ))
            .tip(if deaf { "Undeafen" } else { "Deafen" });
        if r.clicked() {
            voice.set_deafened(!deaf);
        }
        let sharing = voice.sharing();
        let connected = matches!(state, VoiceState::Connected { .. });
        let r = ui
            .add_enabled(
                connected,
                egui::Button::selectable(sharing, RichText::new(icons::SCREENCAST).size(16.0)),
            )
            .tip(if sharing {
                "Stop sharing"
            } else {
                "Share your screen"
            });
        if r.clicked() {
            action = Some(if sharing {
                PanelAction::StopShare
            } else {
                PanelAction::Share
            });
        }
        let camera = voice.camera_on();
        let starting = voice.camera_starting();
        let icon = if camera || starting {
            icons::VIDEO_CAMERA
        } else {
            icons::VIDEO_CAMERA_SLASH
        };
        let r = ui
            .add_enabled(
                connected && !starting,
                egui::Button::selectable(camera, RichText::new(icon).size(16.0)),
            )
            .tip(if starting {
                "Turning on camera…"
            } else if camera {
                "Turn off camera"
            } else {
                "Turn on camera"
            });
        if r.clicked() {
            action = Some(if camera {
                PanelAction::StopCamera
            } else {
                PanelAction::Camera
            });
        }
        let board = ui.add_enabled_ui(connected, |ui| {
            ui.menu_button(RichText::new(icons::MUSIC_NOTES).size(16.0), |ui| {
                soundboard(ui, voice, sounds, audio)
            })
        });
        let board = board.inner;
        if board.response.clicked() {
            action = Some(PanelAction::Soundboard);
        }
        board.response.tip("Soundboard");
        if let Some(Some(file)) = board.inner {
            action = Some(PanelAction::Sound(file));
        }
        let leave = egui::Button::new(
            RichText::new(icons::PHONE_DISCONNECT)
                .size(16.0)
                .color(Color32::WHITE),
        )
        .fill(DANGER);
        if ui.add(leave).tip("Disconnect").clicked() {
            action = Some(PanelAction::Leave);
        }
    });
    ui.add_space(4.0);
    action
}

/// The soundboard menu: the network's sounds, and their volume for us.
/// Returns the file clicked.
fn soundboard(
    ui: &mut egui::Ui,
    voice: &Voice,
    sounds: Option<&[Sound]>,
    audio: &mut AudioSettings,
) -> Option<String> {
    let mut play = None;
    ui.set_max_width(280.0);
    match sounds {
        None => {
            ui.spinner();
        }
        Some([]) => {
            ui.label(RichText::new("This server has no sounds yet.").weak());
        }
        Some(sounds) => {
            egui::ScrollArea::vertical()
                .max_height(240.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for s in sounds {
                            if ui.button(&s.name).clicked() {
                                play = Some(s.file.clone());
                            }
                        }
                    });
                });
        }
    }
    if voice.playing_sound() && ui.button(format!("{} Stop", icons::X)).clicked() {
        voice.stop_sound();
    }
    ui.separator();
    let l = ui.label("Soundboard volume");
    if volume_slider(ui, &mut audio.soundboard_volume)
        .labelled_by(l.id)
        .on_hover_text("How loud everyone's sounds are for you. Voices stay as they are.")
        .changed()
    {
        voice.set_settings(audio.clone());
    }
    play
}

/// Who's in a channel's voice room, under its row in the sidebar. In our own
/// call the live list (speaking, muted, sharing) replaces the metadata one.
/// Volume changes land in `volumes` and `screen_volumes`, and whisper list
/// changes in `whisper_to`, which persist with the settings. Returns whose screen share (or camera, `true`) was clicked,
/// to watch.
#[allow(clippy::too_many_arguments)]
pub fn members(
    ui: &mut egui::Ui,
    voice: &Voice,
    in_call: bool,
    nicks: &[String],
    roster: &[Member],
    profiles: &HashMap<String, Profile>,
    media: &mut Media,
    auto: bool,
    volumes: &mut HashMap<String, f32>,
    screen_volumes: &mut HashMap<String, f32>,
    whisper_to: &mut Vec<String>,
) -> Option<(String, bool)> {
    let mut watch = None;
    let dark = ui.visuals().dark_mode;
    // LiveKit identities are accounts; profiles are keyed by nick.
    // shortcut: ASCII-only account match, falls back to the account itself.
    let nick_of = |id: &'_ str| -> String {
        roster
            .iter()
            .find(|m| {
                m.account
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(id))
            })
            .map_or_else(|| id.to_owned(), |m| m.nick.clone())
    };
    // `note`: whispering, or on the whisper list. `reserve`: width kept free on
    // the right for the LIVE/camera badges, so the name truncates before them.
    let mut row = |ui: &mut egui::Ui,
                   nick: &str,
                   speaking: bool,
                   muted: bool,
                   note: Option<RichText>,
                   reserve: f32| {
        let name = shown(profiles, nick).to_owned();
        let avatar = profiles.get(&key(nick)).and_then(|p| p.avatar.clone());
        let r = ui
            .horizontal(|ui| {
                ui.add_space(16.0);
                let (r, _) = ui.allocate_exact_size(vec2(AVATAR, AVATAR), Sense::hover());
                media.avatar(ui, r.shrink(RING), &name, avatar.as_ref(), auto);
                if speaking {
                    let radius = (AVATAR - RING) / 2.0;
                    ui.painter()
                        .circle_stroke(r.center(), radius, (RING, SPEAKING));
                }
                let color = chatlog::nick_color(nick, dark);
                let text = if muted {
                    RichText::new(format!("{name} {}", icons::MICROPHONE_SLASH)).weak()
                } else if speaking {
                    RichText::new(&name).color(color).strong()
                } else {
                    RichText::new(&name).color(color.gamma_multiply(0.7))
                };
                ui.scope(|ui| {
                    ui.set_max_width((ui.available_width() - reserve).max(0.0));
                    let r = ui.add(egui::Label::new(text).sense(Sense::click()).truncate());
                    if let Some(note) = note {
                        ui.add(egui::Label::new(note.small()).truncate());
                    }
                    r
                })
                .inner
            })
            .inner;
        (r, name)
    };
    match voice.call().filter(|_| in_call) {
        Some(call) => {
            let targets = call
                .participants
                .iter()
                .filter(|p| !p.local && whisper_to.contains(&p.identity))
                .count();
            for p in &call.participants {
                let listed = whisper_to.contains(&p.identity);
                let note = match (p.whispering, p.local) {
                    (true, true) => {
                        Some(RichText::new(format!("whispering to {targets}")).color(WHISPER))
                    }
                    (true, false) => Some(RichText::new("whispering to you").color(WHISPER)),
                    (false, false) if listed => Some(RichText::new("whisper").weak()),
                    _ => None,
                };
                // Room for the badges' natural widths (egui's button min width is 40).
                let reserve =
                    if p.sharing { 48.0 } else { 0.0 } + if p.camera { 44.0 } else { 0.0 };
                let (r, name) = row(
                    ui,
                    &nick_of(&p.identity),
                    p.speaking,
                    p.muted,
                    note,
                    reserve,
                );
                // A child, not `put`: an overflowing badge must not widen the
                // panel, or the panel creeps wider every frame.
                let mut badges = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(egui::Rect::from_x_y_ranges(
                            ui.max_rect().x_range(),
                            r.rect.y_range(),
                        ))
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                if p.sharing {
                    let live = egui::Button::new(
                        RichText::new("LIVE").small().strong().color(Color32::WHITE),
                    )
                    .fill(DANGER)
                    .small();
                    let tip = if p.local {
                        "Preview your share"
                    } else {
                        "Watch"
                    };
                    if badges.add(live).on_hover_text(tip).clicked() {
                        watch = Some((p.identity.clone(), false));
                    }
                }
                if p.camera {
                    let cam = egui::Button::new(RichText::new(icons::VIDEO_CAMERA).small()).small();
                    let tip = if p.local {
                        "Preview your camera"
                    } else {
                        "Watch their camera"
                    };
                    if badges.add(cam).on_hover_text(tip).clicked() {
                        watch = Some((p.identity.clone(), true));
                    }
                }
                if p.local {
                    continue;
                }
                r.context_menu(|ui| {
                    let mut on = listed;
                    let room = on || whisper_to.len() < chad_media::MAX_WHISPER_TARGETS;
                    if ui
                        .add_enabled(
                            room,
                            egui::Checkbox::new(&mut on, format!("Whisper to {name}")),
                        )
                        .on_hover_text("Your whisper key talks only to the people ticked here.")
                        .changed()
                    {
                        if on {
                            whisper_to.push(p.identity.clone());
                        } else {
                            whisper_to.retain(|a| *a != p.identity);
                        }
                        voice.set_whisper_targets(whisper_to.clone());
                    }
                    let mut v = voice.volume(&p.identity);
                    let l = ui.label(format!("{name} volume"));
                    if volume_slider(ui, &mut v).labelled_by(l.id).changed() {
                        voice.set_volume(&p.identity, v);
                        volumes.insert(p.identity.clone(), v);
                    }
                    if p.sharing {
                        let mut v = voice.screen_volume(&p.identity);
                        let l = ui.label("Screen share volume");
                        if volume_slider(ui, &mut v).labelled_by(l.id).changed() {
                            voice.set_screen_volume(&p.identity, v);
                            screen_volumes.insert(p.identity.clone(), v);
                        }
                    }
                });
            }
        }
        None => {
            for n in nicks {
                row(ui, n, false, false, None, 0.0);
            }
        }
    }
    watch
}

/// The audio part of the settings window. True when something changed.
pub fn settings(
    ui: &mut egui::Ui,
    s: &mut Settings,
    voice: &Voice,
    ptt: Option<&Ptt>,
    binding: &mut Option<Action>,
) -> bool {
    let before = (s.audio.clone(), s.ptt, s.whisper);
    let devices = voice.devices();
    egui::Grid::new("audio")
        .num_columns(2)
        .min_col_width(settings::LABEL_W)
        .show(ui, |ui| {
            for (label, current, list) in [
                ("Microphone", &mut s.audio.input, &devices.inputs),
                ("Speakers", &mut s.audio.output, &devices.outputs),
                ("Camera", &mut s.audio.camera, &devices.cameras),
            ] {
                let l = ui.label(label);
                let shown = current
                    .as_ref()
                    .map(|id| {
                        list.iter()
                            .find(|d| &d.id == id)
                            .map_or_else(|| format!("{id} (unplugged)"), |d| d.name.clone())
                    })
                    .unwrap_or_else(|| "System default".into());
                egui::ComboBox::from_id_salt(label)
                    .selected_text(shown)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(current, None, "System default");
                        for d in list {
                            ui.selectable_value(current, Some(d.id.clone()), &d.name);
                        }
                    })
                    .response
                    .labelled_by(l.id);
                ui.end_row();
            }

            let l = ui.label("Input volume");
            volume_slider(ui, &mut s.audio.input_gain)
                .labelled_by(l.id)
                .on_hover_text("How loud others hear you.");
            ui.end_row();

            let l = ui.label("Soundboard volume");
            volume_slider(ui, &mut s.audio.soundboard_volume)
                .labelled_by(l.id)
                .on_hover_text("How loud sounds played in voice are for you.");
            ui.end_row();

            ui.label("Input");
            ui.vertical(|ui| {
                ui.radio_value(
                    &mut s.audio.mode,
                    InputMode::VoiceActivity,
                    "Voice activity",
                );
                ui.radio_value(&mut s.audio.mode, InputMode::PushToTalk, "Push to talk");
            });
            ui.end_row();

            match s.audio.mode {
                InputMode::VoiceActivity => {
                    let l = ui.label("Sensitivity");
                    ui.vertical(|ui| {
                        meter(ui, voice, Some(s.audio.vad_threshold_db));
                        ui.add(
                            egui::Slider::new(&mut s.audio.vad_threshold_db, -80.0..=0.0)
                                .suffix(" dB")
                                .step_by(1.0)
                                .fixed_decimals(0),
                        )
                        .labelled_by(l.id);
                    });
                    ui.end_row();
                }
                InputMode::PushToTalk => {
                    ui.label("Key");
                    let mut b = Some(s.ptt);
                    bind_button(ui, Action::Talk, &mut b, binding, false);
                    s.ptt = b.unwrap_or(s.ptt);
                    ui.end_row();
                    let l = ui.label("Release delay");
                    ui.add(egui::Slider::new(&mut s.audio.ptt_release_ms, 0..=1000).suffix(" ms"))
                        .labelled_by(l.id);
                    ui.end_row();
                }
            }

            ui.label("Whisper key");
            ui.vertical(|ui| {
                bind_button(ui, Action::Whisper, &mut s.whisper, binding, true);
                ui.label(
                    RichText::new(
                        "Hold it to talk only to the people you whisper to \
                         (right-click someone in voice).",
                    )
                    .small()
                    .weak(),
                );
            });
            ui.end_row();

            if s.audio.mode == InputMode::PushToTalk || s.whisper.is_some() {
                ui.label("");
                let what = if s.whisper.is_some() {
                    "Push to talk and whisper work"
                } else {
                    "Push to talk works"
                };
                match ptt.map(Ptt::status) {
                    Some(PttStatus::Global(how)) => ui.label(RichText::new(how).small().weak()),
                    Some(PttStatus::FocusedOnly(why)) => ui.label(
                        RichText::new(format!("{why} {what} only while IrChad is focused."))
                            .small()
                            .color(ui.visuals().warn_fg_color),
                    ),
                    _ => ui.spinner(),
                };
                ui.end_row();
            }

            ui.label("Echo");
            ui.checkbox(&mut s.audio.echo_cancellation, "Echo cancellation")
                .on_hover_text("Stops others hearing themselves through your speakers.");
            ui.end_row();

            ui.label("Noise");
            ui.vertical(|ui| {
                ui.checkbox(&mut s.audio.noise_suppression, "Noise suppression");
                ui.checkbox(&mut s.audio.rnnoise, "Extra noise suppression (RNNoise)");
            });
            ui.end_row();

            ui.label("Gain");
            ui.checkbox(&mut s.audio.auto_gain, "Automatic gain control")
                .on_hover_text("Evens out your volume, but can make it pump between words.");
            ui.end_row();

            let l = ui.label("Quality");
            ui.add(egui::Slider::new(&mut s.audio.bitrate_kbps, 16..=128).suffix(" kbps"))
                .labelled_by(l.id)
                .on_hover_text("Takes effect the next time you join voice.");
            ui.end_row();

            ui.label("Test");
            ui.vertical(|ui| {
                let mut testing = voice.testing();
                if ui
                    .checkbox(&mut testing, "Hear yourself")
                    .on_hover_text("Plays your microphone back, processed the way others hear it.")
                    .changed()
                {
                    voice.set_testing(testing);
                }
                if s.audio.mode == InputMode::PushToTalk {
                    meter(ui, voice, None);
                }
            });
            ui.end_row();
        });
    // The meter moves.
    ui.ctx().request_repaint_after(Duration::from_millis(50));
    (s.audio.clone(), s.ptt, s.whisper) != before
}

/// A key bind's button: click, then press the key. `clear` offers to unbind.
fn bind_button(
    ui: &mut egui::Ui,
    action: Action,
    bind: &mut Option<PttBind>,
    binding: &mut Option<Action>,
    clear: bool,
) {
    ui.horizontal(|ui| {
        let text = if *binding == Some(action) {
            "Press a key or mouse button…".to_owned()
        } else {
            bind.map_or_else(|| "Not set".to_owned(), PttBind::name)
        };
        if ui.button(text).clicked() {
            *binding = (*binding != Some(action)).then_some(action);
        }
        if *binding == Some(action)
            && let Some(b) = captured(ui)
        {
            *bind = Some(b);
            *binding = None;
        }
        if clear && bind.is_some() && ui.small_button("Clear").clicked() {
            *bind = None;
            *binding = None;
        }
    });
}

/// A 0–200% volume.
pub fn volume_slider(ui: &mut egui::Ui, v: &mut f32) -> egui::Response {
    ui.add(egui::Slider::new(v, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)))
}

/// Input level, with the voice activity threshold marked.
fn meter(ui: &mut egui::Ui, voice: &Voice, threshold: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(260.0, 8.0), Sense::hover());
    let x = |db: f32| rect.left() + rect.width() * ((db + 80.0) / 80.0).clamp(0.0, 1.0);
    let p = ui.painter();
    p.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let level = voice.input_level();
    let fill = if voice.transmitting() {
        SPEAKING
    } else {
        ui.visuals().weak_text_color()
    };
    let lit = egui::Rect::from_min_max(rect.min, egui::pos2(x(level), rect.max.y));
    p.rect_filled(lit, 3.0, fill);
    if let Some(t) = threshold {
        let tx = x(t);
        p.line_segment(
            [
                egui::pos2(tx, rect.top() - 2.0),
                egui::pos2(tx, rect.bottom() + 2.0),
            ],
            (2.0, ui.visuals().strong_text_color()),
        );
    }
}

/// The first key or side button pressed this frame that can be a PTT bind.
fn captured(ui: &egui::Ui) -> Option<PttBind> {
    ui.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key {
                key, pressed: true, ..
            } if keymap(*key).is_some() => Some(PttBind::Key(*key)),
            egui::Event::PointerButton {
                button: egui::PointerButton::Extra1,
                pressed: true,
                ..
            } => Some(PttBind::Mouse4),
            egui::Event::PointerButton {
                button: egui::PointerButton::Extra2,
                pressed: true,
                ..
            } => Some(PttBind::Mouse5),
            _ => None,
        })
    })
}
