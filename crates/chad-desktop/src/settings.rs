//! User preferences, persisted by eframe's storage (per user).

use std::collections::HashMap;

use chad_media::AudioSettings;
use egui::{FontId, ThemePreference};
use serde::{Deserialize, Serialize};

use crate::platform::ptt::PttBind;

pub const KEY: &str = "settings";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemePreference,
    pub font_size: f32,
    pub notify: Notify,
    pub audio: AudioSettings,
    pub ptt: PttBind,
    /// Voice volume per account, 0.0..=2.0.
    pub volumes: HashMap<String, f32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::Dark,
            font_size: 14.0,
            notify: Notify::MentionsAndDms,
            audio: AudioSettings::default(),
            ptt: PttBind::default(),
            volumes: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Notify {
    All,
    MentionsAndDms,
    Never,
}

impl Settings {
    pub fn apply(&self, ctx: &egui::Context) {
        ctx.set_theme(self.theme);
        let k = self.font_size.clamp(10.0, 24.0) / 12.5;
        let defaults = egui::Style::default().text_styles;
        ctx.all_styles_mut(|style| {
            for (ts, font) in &defaults {
                let size = font.size * k;
                style
                    .text_styles
                    .insert(ts.clone(), FontId::new(size, font.family.clone()));
            }
        });
    }

    /// Returns true when something changed.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        let before = self.clone();
        let (mut dragging, mut released) = (false, false);
        egui::Grid::new("settings").num_columns(2).show(ui, |ui| {
            ui.label("Theme");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.theme, ThemePreference::Dark, "Dark");
                ui.selectable_value(&mut self.theme, ThemePreference::Light, "Light");
                ui.selectable_value(&mut self.theme, ThemePreference::System, "Match system");
            });
            ui.end_row();

            ui.label("Text size");
            let r = ui.add(egui::Slider::new(&mut self.font_size, 10.0..=24.0).step_by(1.0));
            // Resizing mid-drag moves the slider under the pointer and feeds back into it.
            dragging = r.dragged();
            released = r.drag_stopped();
            ui.end_row();

            ui.label("Notify me about");
            ui.vertical(|ui| {
                ui.radio_value(&mut self.notify, Notify::All, "Every message");
                ui.radio_value(
                    &mut self.notify,
                    Notify::MentionsAndDms,
                    "Mentions and direct messages",
                );
                ui.radio_value(&mut self.notify, Notify::Never, "Nothing");
            });
            ui.end_row();
        });
        (*self != before && !dragging) || released
    }
}
