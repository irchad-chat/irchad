//! Voice in the UI: the glue that lets the core drive `chad-media`, the call
//! panel, who's-in-voice rows in the sidebar, and audio settings.

use std::collections::HashMap;
use std::time::Duration;

use chad_core::{MediaEvent, VoiceState};
use chad_media::{InputMode, Quality, Voice};
use egui::{Color32, RichText, Sense, vec2};
use tokio::sync::mpsc;

use crate::platform::ptt::{Ptt, PttBind, PttStatus, keymap};
use crate::settings::Settings;
use crate::{chatlog, icons};

const SPEAKING: Color32 = Color32::from_rgb(35, 165, 90);
const DANGER: Color32 = Color32::from_rgb(218, 55, 60);

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
}

/// Bottom of the channel list while in a call (on any network).
pub fn panel(
    ui: &mut egui::Ui,
    voice: &Voice,
    state: &VoiceState,
    network: &str,
    ptt: Option<&Ptt>,
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
                    Quality::Good => (Color32::from_rgb(240, 178, 50), "Good connection"),
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
        ui.label(
            RichText::new(format!("Push to talk ({}) only works here", p.bind.name()))
                .small()
                .color(ui.visuals().warn_fg_color),
        );
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
            .on_hover_text(if muted { "Unmute" } else { "Mute" });
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
            .on_hover_text(if deaf { "Undeafen" } else { "Deafen" });
        if r.clicked() {
            voice.set_deafened(!deaf);
        }
        let leave = egui::Button::new(
            RichText::new(icons::PHONE_DISCONNECT)
                .size(16.0)
                .color(Color32::WHITE),
        )
        .fill(DANGER);
        if ui.add(leave).on_hover_text("Disconnect").clicked() {
            action = Some(PanelAction::Leave);
        }
    });
    ui.add_space(4.0);
    action
}

/// Who's in a channel's voice room, under its row in the sidebar. In our own
/// call the live list (speaking, muted) replaces the metadata one. Volume
/// changes land in `volumes`, which persist with the settings.
pub fn members(
    ui: &mut egui::Ui,
    voice: &Voice,
    in_call: bool,
    nicks: &[String],
    volumes: &mut HashMap<String, f32>,
) {
    let dark = ui.visuals().dark_mode;
    let row = |ui: &mut egui::Ui, nick: &str, speaking: bool, muted: bool| {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            if speaking {
                ui.painter().circle_filled(r.center(), 4.0, SPEAKING);
            } else {
                ui.painter()
                    .circle_stroke(r.center(), 4.0, (1.0, ui.visuals().weak_text_color()));
            }
            let mut text = RichText::new(nick)
                .small()
                .color(chatlog::nick_color(nick, dark));
            if muted {
                text = RichText::new(format!("{nick} {}", icons::MICROPHONE_SLASH))
                    .small()
                    .weak();
            }
            ui.add(egui::Label::new(text).sense(Sense::click()).truncate())
        })
        .inner
    };
    match voice.call().filter(|_| in_call) {
        Some(call) => {
            for p in &call.participants {
                let r = row(ui, &p.identity, p.speaking, p.muted);
                if p.local {
                    continue;
                }
                r.context_menu(|ui| {
                    let mut v = voice.volume(&p.identity);
                    ui.label(format!("{} volume", p.identity));
                    let s = ui.add(
                        egui::Slider::new(&mut v, 0.0..=2.0)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                    );
                    if s.changed() {
                        voice.set_volume(&p.identity, v);
                        volumes.insert(p.identity.clone(), v);
                    }
                });
            }
        }
        None => {
            for n in nicks {
                row(ui, n, false, false);
            }
        }
    }
}

/// The audio part of the settings window. True when something changed.
pub fn settings(
    ui: &mut egui::Ui,
    s: &mut Settings,
    voice: &Voice,
    ptt: Option<&Ptt>,
    binding: &mut bool,
) -> bool {
    let before = (s.audio.clone(), s.ptt);
    let devices = voice.devices();
    ui.add_space(8.0);
    ui.heading("Voice");
    egui::Grid::new("audio").num_columns(2).show(ui, |ui| {
        for (label, current, list) in [
            ("Microphone", &mut s.audio.input, &devices.inputs),
            ("Speakers", &mut s.audio.output, &devices.outputs),
        ] {
            ui.label(label);
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
                });
            ui.end_row();
        }

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
                ui.label("Sensitivity");
                ui.vertical(|ui| {
                    meter(ui, voice, Some(s.audio.vad_threshold_db));
                    ui.add(
                        egui::Slider::new(&mut s.audio.vad_threshold_db, -80.0..=0.0)
                            .suffix(" dB")
                            .step_by(1.0),
                    );
                });
                ui.end_row();
            }
            InputMode::PushToTalk => {
                ui.label("Key");
                ui.horizontal(|ui| {
                    let text = if *binding {
                        "Press a key or mouse button…".to_owned()
                    } else {
                        s.ptt.name()
                    };
                    if ui.button(text).clicked() {
                        *binding = !*binding;
                    }
                    if *binding && let Some(b) = captured(ui) {
                        s.ptt = b;
                        *binding = false;
                    }
                });
                ui.end_row();
                ui.label("");
                match ptt.map(Ptt::status) {
                    Some(PttStatus::Global(how)) => ui.label(RichText::new(how).small().weak()),
                    Some(PttStatus::FocusedOnly(why)) => ui.label(
                        RichText::new(format!(
                            "{why} Push to talk works only while IrChad is focused."
                        ))
                        .small()
                        .color(ui.visuals().warn_fg_color),
                    ),
                    _ => ui.spinner(),
                };
                ui.end_row();
                ui.label("Release delay");
                ui.add(egui::Slider::new(&mut s.audio.ptt_release_ms, 0..=1000).suffix(" ms"));
                ui.end_row();
            }
        }

        ui.label("Noise");
        ui.checkbox(&mut s.audio.rnnoise, "Extra noise suppression (RNNoise)");
        ui.end_row();

        ui.label("Quality");
        ui.add(egui::Slider::new(&mut s.audio.bitrate_kbps, 16..=128).suffix(" kbps"))
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
    (s.audio.clone(), s.ptt) != before
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
