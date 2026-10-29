//! Phosphor icons (regular) for UI chrome. Reactions stay system emoji.
//! Codepoints from <https://phosphoricons.com>; add more as needed.

pub const ARROW_BEND_DOWN_RIGHT: &str = "\u{E01A}";
pub const ARROW_BEND_UP_LEFT: &str = "\u{E024}";
pub const ARROW_DOWN: &str = "\u{E03E}";
pub const DOTS_THREE: &str = "\u{E1FE}";
pub const GEAR: &str = "\u{E270}";
pub const HEADPHONES: &str = "\u{E2A6}";
pub const MICROPHONE: &str = "\u{E326}";
pub const MICROPHONE_SLASH: &str = "\u{E328}";
pub const PHONE_DISCONNECT: &str = "\u{E3BC}";
pub const SPEAKER_HIGH: &str = "\u{E44A}";
pub const TRASH: &str = "\u{E4A6}";
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
