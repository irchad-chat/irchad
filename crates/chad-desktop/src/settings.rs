//! User preferences, persisted by eframe's storage (per user).

use std::collections::HashMap;

use chad_media::{AudioSettings, ScreenSettings};
use egui::{FontId, ThemePreference, Visuals};
use serde::{Deserialize, Serialize};

use crate::catppuccin;
use crate::platform::ptt::PttBind;
use crate::screen::Layout;

pub const KEY: &str = "settings";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    pub font_size: f32,
    pub notify: Notify,
    pub audio: AudioSettings,
    pub ptt: PttBind,
    /// Voice volume per account, 0.0..=2.0.
    pub volumes: HashMap<String, f32>,
    /// Screen share audio volume per account, 0.0..=2.0.
    pub screen_volumes: HashMap<String, f32>,
    pub screen: ScreenSettings,
    /// The ScreenCast portal's token for the last thing shared, so it doesn't
    /// ask again.
    pub restore_token: Option<String>,
    /// Images, previews and avatars load by themselves; off, each waits for a click.
    pub load_media: bool,
    /// Images and previews in messages from people without an account; off,
    /// their links stay plain links.
    pub guest_media: bool,
    /// Away after this many idle minutes; 0 never.
    pub away_after: u32,
    /// Away by choice, on every network.
    pub away: bool,
    pub check_updates: bool,
    /// Opening a channel splits the window instead of replacing the one on screen.
    pub panes: bool,
    /// Where a watched screen share goes.
    pub stage: Layout,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            font_size: 14.0,
            notify: Notify::MentionsAndDms,
            audio: AudioSettings::default(),
            ptt: PttBind::default(),
            volumes: HashMap::new(),
            screen_volumes: HashMap::new(),
            screen: ScreenSettings::default(),
            restore_token: None,
            load_media: true,
            guest_media: true,
            away_after: 10,
            away: false,
            check_updates: true,
            panes: true,
            stage: Layout::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Notify {
    All,
    MentionsAndDms,
    Never,
}

/// egui's own looks plus the Catppuccin flavors. The first three keep the names
/// `ThemePreference` serializes to, so older settings still load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Dark,
    Light,
    System,
    Latte,
    Frappe,
    Macchiato,
    Mocha,
}

impl Theme {
    fn palette(self) -> Option<catppuccin::Palette> {
        match self {
            Theme::Dark | Theme::Light | Theme::System => None,
            Theme::Latte => Some(catppuccin::LATTE),
            Theme::Frappe => Some(catppuccin::FRAPPE),
            Theme::Macchiato => Some(catppuccin::MACCHIATO),
            Theme::Mocha => Some(catppuccin::MOCHA),
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        // Back to stock first, so leaving a flavor leaves none of its colors behind.
        ctx.set_visuals_of(egui::Theme::Dark, Visuals::dark());
        ctx.set_visuals_of(egui::Theme::Light, Visuals::light());
        match self.palette() {
            None => ctx.set_theme(match self {
                Theme::Light => ThemePreference::Light,
                Theme::System => ThemePreference::System,
                _ => ThemePreference::Dark,
            }),
            Some(p) => {
                let theme = if p.dark {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                };
                ctx.set_visuals_of(theme, p.visuals());
                ctx.set_theme(theme);
            }
        }
    }
}

/// Label column width, shared by every settings grid so they line up.
pub const LABEL_W: f32 = 140.0;

impl Settings {
    pub fn apply(&self, ctx: &egui::Context) {
        self.theme.apply(ctx);
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

    /// One section's rows, all but [`Section::Voice`] (see `voice::settings`).
    /// Returns true when something changed.
    pub fn ui(&mut self, ui: &mut egui::Ui, section: Section) -> bool {
        let before = self.clone();
        let (mut dragging, mut released) = (false, false);
        egui::Grid::new("settings")
            .num_columns(2)
            .min_col_width(LABEL_W)
            .show(ui, |ui| match section {
                Section::Appearance => {
                    ui.label("Theme");
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut self.theme, Theme::Dark, "Dark");
                            ui.selectable_value(&mut self.theme, Theme::Light, "Light");
                            ui.selectable_value(&mut self.theme, Theme::System, "Match system");
                        });
                        ui.horizontal(|ui| {
                            ui.weak("Catppuccin");
                            ui.selectable_value(&mut self.theme, Theme::Latte, "Latte");
                            ui.selectable_value(&mut self.theme, Theme::Frappe, "Frappé");
                            ui.selectable_value(&mut self.theme, Theme::Macchiato, "Macchiato");
                            ui.selectable_value(&mut self.theme, Theme::Mocha, "Mocha");
                        });
                    });
                    ui.end_row();

                    let l = ui.label("Text size");
                    let r = ui
                        .add(
                            egui::Slider::new(&mut self.font_size, 10.0..=24.0)
                                .step_by(1.0)
                                .fixed_decimals(0),
                        )
                        .labelled_by(l.id);
                    // Resizing mid-drag moves the slider under the pointer and feeds back into it.
                    dragging = r.dragged();
                    released = r.drag_stopped();
                    ui.end_row();

                    ui.label("Layout");
                    ui.checkbox(
                        &mut self.panes,
                        "Open channels side by side (Ctrl+arrows move between them)",
                    );
                    ui.end_row();
                }
                Section::Chat => {
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

                    ui.label("Images");
                    ui.checkbox(
                        &mut self.load_media,
                        "Show images, link previews and avatars without a click",
                    );
                    ui.end_row();

                    ui.label("");
                    ui.checkbox(
                        &mut self.guest_media,
                        "Show images and link previews from guests (no account)",
                    );
                    ui.end_row();
                }
                Section::General => {
                    let l = ui.label("Away");
                    ui.horizontal(|ui| {
                        ui.label("after");
                        ui.add(egui::DragValue::new(&mut self.away_after).range(0..=240))
                            .labelled_by(l.id);
                        ui.label("idle minutes (0: never)");
                    });
                    ui.end_row();

                    ui.label("Updates");
                    ui.checkbox(&mut self.check_updates, "Tell me when a new version is out");
                    ui.end_row();

                    ui.label("Version");
                    ui.label(format!("IrChad {}", crate::update::version()));
                    ui.end_row();
                }
                Section::Voice => {}
            });
        (*self != before && !dragging) || released
    }
}

/// The settings window's pages, in its side nav.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Appearance,
    Chat,
    Voice,
    General,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Appearance,
        Section::Chat,
        Section::Voice,
        Section::General,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Chat => "Chat",
            Section::Voice => "Voice",
            Section::General => "General",
        }
    }
}
