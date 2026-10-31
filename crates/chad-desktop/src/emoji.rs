//! Emoji: `:shortcode:` (GitHub's names) in the composer, and the picker
//! used for both the composer and reactions.

use egui::{RichText, vec2};

const PER_ROW: usize = 9;
/// The picker's width, shared by the emoji and GIF tabs.
pub const PICKER_WIDTH: f32 = PER_ROW as f32 * 36.0;

/// Replaces each complete `:shortcode:` in `text`. `cursor` is a char index
/// that is moved to stay after the same text.
pub fn replace_shortcodes(text: &mut String, cursor: &mut usize) -> bool {
    let mut out = String::with_capacity(text.len());
    let (mut pos, mut changed, mut shrunk) = (0, false, 0usize);
    while let Some(a) = text[pos..].find(':').map(|i| pos + i) {
        let Some(b) = text[a + 1..].find(':').map(|i| a + 1 + i) else {
            break;
        };
        let name = &text[a + 1..b];
        let valid = (1..=40).contains(&name.len())
            && name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_+-".contains(&c));
        match emojis::get_by_shortcode(name).filter(|_| valid) {
            Some(e) => {
                out.push_str(&text[pos..a]);
                out.push_str(e.as_str());
                if text[..=b].chars().count() <= *cursor {
                    shrunk += (name.len() + 2).saturating_sub(e.as_str().chars().count());
                }
                pos = b + 1;
                changed = true;
            }
            // The second colon may open the next code.
            None => {
                out.push_str(&text[pos..b]);
                pos = b;
            }
        }
    }
    if changed {
        out.push_str(&text[pos..]);
        *text = out;
        *cursor = cursor.saturating_sub(shrunk);
    }
    changed
}

/// The picker's grid with a search box. Returns the emoji clicked.
pub fn picker(ui: &mut egui::Ui, query: &mut String) -> Option<String> {
    let search = ui.add(
        egui::TextEdit::singleline(query)
            .hint_text("Find an emoji")
            .desired_width(f32::INFINITY),
    );
    ui.ctx()
        .accesskit_node_builder(search.id, |n| n.set_label("Find an emoji"));
    if !search.has_focus() && !search.lost_focus() {
        search.request_focus();
    }
    let q = query.trim().to_lowercase();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let list: Vec<&emojis::Emoji> = ui.ctx().fonts_mut(|f| {
        emojis::iter()
            .filter(|e| {
                q.is_empty() || e.name().contains(&q) || e.shortcodes().any(|s| s.contains(&q))
            })
            // Only what our fonts can draw (joiners and variation selectors aside).
            .filter(|e| {
                let base: String = e
                    .as_str()
                    .chars()
                    .filter(|c| !matches!(c, '\u{200d}' | '\u{fe0f}'))
                    .collect();
                f.has_glyphs(&font, &base)
            })
            .collect()
    });
    let mut picked = None;
    if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        picked = list.first().map(|e| e.as_str().to_owned());
    }
    let rows = list.len().div_ceil(PER_ROW);
    let size = ui.text_style_height(&egui::TextStyle::Body) * 1.6;
    egui::ScrollArea::vertical()
        .max_height(260.0)
        .auto_shrink([false, true])
        .show_rows(ui, size + ui.spacing().item_spacing.y, rows, |ui, range| {
            // Cells split the whole width, so the grid fills the panel.
            let cell = ui.available_width() / PER_ROW as f32;
            for row in range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for e in list.iter().skip(row * PER_ROW).take(PER_ROW) {
                        let b = egui::Button::new(RichText::new(e.as_str()).size(size * 0.75))
                            .frame(false)
                            .min_size(vec2(cell, size));
                        let tip = match e.shortcode() {
                            Some(s) => format!(":{s}:"),
                            None => e.name().to_owned(),
                        };
                        if ui.add(b).on_hover_text(tip).clicked() {
                            picked = Some(e.as_str().to_owned());
                        }
                    }
                });
            }
        });
    if list.is_empty() {
        ui.label(RichText::new("No emoji found.").weak());
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcodes() {
        let mut t = "hi :smile: at 12:30:45 :nope: :+1: x".to_owned();
        let mut cursor = t.chars().count();
        assert!(replace_shortcodes(&mut t, &mut cursor));
        assert_eq!(t, "hi 😄 at 12:30:45 :nope: 👍 x");
        assert_eq!(cursor, t.chars().count());

        // Cursor before the code stays put.
        let mut t = "ab :heart:".to_owned();
        let mut cursor = 1;
        replace_shortcodes(&mut t, &mut cursor);
        assert_eq!((t.as_str(), cursor), ("ab ❤️", 1));

        let mut t = "no codes: here".to_owned();
        assert!(!replace_shortcodes(&mut t, &mut 0));
        let mut t = "é:é:".to_owned();
        assert!(!replace_shortcodes(&mut t, &mut 0));
    }
}
