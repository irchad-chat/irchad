//! Phosphor icons (regular) for UI chrome. Reactions stay system emoji.
//! Codepoints from <https://phosphoricons.com>; add more as needed.

pub const APP_WINDOW: &str = "\u{E5DA}";
pub const ARROW_BEND_DOWN_RIGHT: &str = "\u{E01A}";
pub const ARROW_BEND_UP_LEFT: &str = "\u{E024}";
pub const ARROW_DOWN: &str = "\u{E03E}";
pub const ARROW_SQUARE_OUT: &str = "\u{E5DE}";
pub const CORNERS_IN: &str = "\u{E1CE}";
pub const CORNERS_OUT: &str = "\u{E1D0}";
pub const DOTS_THREE: &str = "\u{E1FE}";
pub const EYE: &str = "\u{E220}";
pub const FILE: &str = "\u{E230}";
pub const GEAR: &str = "\u{E270}";
pub const HEADPHONES: &str = "\u{E2A6}";
pub const LOCK: &str = "\u{E2FA}";
pub const LOCK_OPEN: &str = "\u{E306}";
pub const MICROPHONE: &str = "\u{E326}";
pub const MICROPHONE_SLASH: &str = "\u{E328}";
pub const MONITOR: &str = "\u{E32E}";
pub const MOON: &str = "\u{E330}";
pub const PAPERCLIP: &str = "\u{E39A}";
pub const PHONE_DISCONNECT: &str = "\u{E3BC}";
pub const PICTURE_IN_PICTURE: &str = "\u{E64C}";
pub const PLAY: &str = "\u{E3D0}";
pub const SCREENCAST: &str = "\u{E404}";
pub const SHIELD_CHECK: &str = "\u{E40C}";
pub const SMILEY: &str = "\u{E436}";
pub const SPEAKER_HIGH: &str = "\u{E44A}";
pub const SQUARES_FOUR: &str = "\u{E464}";
pub const TRASH: &str = "\u{E4A6}";
pub const USER_CIRCLE: &str = "\u{E4C4}";
pub const WARNING: &str = "\u{E4E0}";
pub const X: &str = "\u{E4F6}";

/// Adds the icon font as a lowest-priority proportional fallback, so icons mix
/// into ordinary labels. Its glyphs live in the Private Use Area, so it never
/// shadows text or emoji.
pub fn install(ctx: &egui::Context) {
    ctx.add_font(egui::epaint::text::FontInsert::new(
        "phosphor",
        egui::FontData::from_static(include_bytes!("../assets/Phosphor.ttf")),
        vec![egui::epaint::text::InsertFontFamily {
            family: egui::FontFamily::Proportional,
            priority: egui::epaint::text::FontPriority::Lowest,
        }],
    ));
}

pub trait Tip {
    /// Hover text that is also the accessible name, for icon-only widgets.
    fn tip(self, text: &str) -> Self;
}

impl Tip for egui::Response {
    fn tip(self, text: &str) -> Self {
        self.ctx
            .accesskit_node_builder(self.id, |n| n.set_label(text));
        self.on_hover_text(text)
    }
}
