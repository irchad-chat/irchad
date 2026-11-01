//! Catppuccin flavors as egui visuals.
//!
//! Adapted from <https://github.com/catppuccin/egui> (MIT, © 2023 Catppuccin),
//! which only supports egui up to 0.33. Only the colors the visuals use are kept.

use egui::{Color32, Stroke, Visuals, style::WidgetVisuals};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub rosewater: Color32,
    pub maroon: Color32,
    pub peach: Color32,
    pub blue: Color32,
    pub text: Color32,
    pub overlay1: Color32,
    pub surface2: Color32,
    pub surface1: Color32,
    pub surface0: Color32,
    pub base: Color32,
    pub mantle: Color32,
    pub crust: Color32,
    pub dark: bool,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub const LATTE: Palette = Palette {
    rosewater: rgb(220, 138, 120),
    maroon: rgb(230, 69, 83),
    peach: rgb(254, 100, 11),
    blue: rgb(30, 102, 245),
    text: rgb(76, 79, 105),
    overlay1: rgb(140, 143, 161),
    surface2: rgb(172, 176, 190),
    surface1: rgb(188, 192, 204),
    surface0: rgb(204, 208, 218),
    base: rgb(239, 241, 245),
    mantle: rgb(230, 233, 239),
    crust: rgb(220, 224, 232),
    dark: false,
};

pub const FRAPPE: Palette = Palette {
    rosewater: rgb(242, 213, 207),
    maroon: rgb(234, 153, 156),
    peach: rgb(239, 159, 118),
    blue: rgb(140, 170, 238),
    text: rgb(198, 208, 245),
    overlay1: rgb(131, 139, 167),
    surface2: rgb(98, 104, 128),
    surface1: rgb(81, 87, 109),
    surface0: rgb(65, 69, 89),
    base: rgb(48, 52, 70),
    mantle: rgb(41, 44, 60),
    crust: rgb(35, 38, 52),
    dark: true,
};

pub const MACCHIATO: Palette = Palette {
    rosewater: rgb(244, 219, 214),
    maroon: rgb(238, 153, 160),
    peach: rgb(245, 169, 127),
    blue: rgb(138, 173, 244),
    text: rgb(202, 211, 245),
    overlay1: rgb(128, 135, 162),
    surface2: rgb(91, 96, 120),
    surface1: rgb(73, 77, 100),
    surface0: rgb(54, 58, 79),
    base: rgb(36, 39, 58),
    mantle: rgb(30, 32, 48),
    crust: rgb(24, 25, 38),
    dark: true,
};

pub const MOCHA: Palette = Palette {
    rosewater: rgb(245, 224, 220),
    maroon: rgb(235, 160, 172),
    peach: rgb(250, 179, 135),
    blue: rgb(137, 180, 250),
    text: rgb(205, 214, 244),
    overlay1: rgb(127, 132, 156),
    surface2: rgb(88, 91, 112),
    surface1: rgb(69, 71, 90),
    surface0: rgb(49, 50, 68),
    base: rgb(30, 30, 46),
    mantle: rgb(24, 24, 37),
    crust: rgb(17, 17, 27),
    dark: true,
};

impl Palette {
    pub fn visuals(&self) -> Visuals {
        let old = if self.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        let shadow = Color32::from_black_alpha(if self.dark { 96 } else { 25 });
        let widget = |old: WidgetVisuals, bg_fill| WidgetVisuals {
            bg_fill,
            weak_bg_fill: bg_fill,
            bg_stroke: Stroke {
                color: self.overlay1,
                ..old.bg_stroke
            },
            fg_stroke: Stroke {
                color: self.text,
                ..old.fg_stroke
            },
            ..old
        };
        let mut v = Visuals {
            hyperlink_color: self.rosewater,
            faint_bg_color: self.surface0,
            extreme_bg_color: self.crust,
            code_bg_color: self.mantle,
            warn_fg_color: self.peach,
            error_fg_color: self.maroon,
            window_fill: self.base,
            panel_fill: self.base,
            window_stroke: Stroke {
                color: self.overlay1,
                ..old.window_stroke
            },
            ..old.clone()
        };
        v.widgets.noninteractive = widget(old.widgets.noninteractive, self.base);
        v.widgets.inactive = widget(old.widgets.inactive, self.surface0);
        v.widgets.hovered = widget(old.widgets.hovered, self.surface2);
        v.widgets.active = widget(old.widgets.active, self.surface1);
        v.widgets.open = widget(old.widgets.open, self.surface0);
        v.selection.bg_fill = self.blue.linear_multiply(if self.dark { 0.2 } else { 0.4 });
        v.selection.stroke.color = self.text;
        v.window_shadow.color = shadow;
        v.popup_shadow.color = shadow;
        v
    }
}
