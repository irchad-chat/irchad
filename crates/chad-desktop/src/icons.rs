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
pub const GAME_CONTROLLER: &str = "\u{E26E}";
pub const GEAR: &str = "\u{E270}";
pub const GHOST: &str = "\u{E62A}";
pub const HEADPHONES: &str = "\u{E2A6}";
pub const LOCK: &str = "\u{E2FA}";
pub const LOCK_OPEN: &str = "\u{E306}";
pub const MICROPHONE: &str = "\u{E326}";
pub const MICROPHONE_SLASH: &str = "\u{E328}";
pub const MONITOR: &str = "\u{E32E}";
pub const MOON: &str = "\u{E330}";
pub const MUSIC_NOTES: &str = "\u{E33C}";
pub const PENCIL_SIMPLE: &str = "\u{E3B4}";
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
pub const USERS: &str = "\u{E4D6}";
pub const WARNING: &str = "\u{E4E0}";
pub const CROWN: &str = "\u{E616}";
pub const X: &str = "\u{E4F6}";

/// The font family for bold message text: egui's `strong()` only brightens.
pub const BOLD: &str = "bold";

/// Adds the icon font as a lowest-priority proportional fallback, so icons mix
/// into ordinary labels. Its glyphs live in the Private Use Area, so it never
/// shadows text or emoji. Also adds the [`BOLD`] family: Ubuntu Bold (egui's
/// text is Ubuntu Light) ahead of the same fallbacks.
pub fn install(ctx: &egui::Context) {
    use egui::{FontData, FontFamily};
    use std::sync::Arc;
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "phosphor".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/Phosphor.ttf"
        ))),
    );
    fonts.font_data.insert(
        "ubuntu-bold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/Ubuntu-Bold.ttf"
        ))),
    );
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.push("phosphor".into());
    let mut bold = proportional.clone();
    bold.insert(0, "ubuntu-bold".into());
    fonts.families.insert(FontFamily::Name(BOLD.into()), bold);
    ctx.set_fonts(fonts);
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
