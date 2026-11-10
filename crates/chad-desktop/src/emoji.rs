//! The emoji picker, used for both the composer and reactions.
//! `:shortcode:` replacement is `chad_core::emoji`.

use egui::{RichText, vec2};

use crate::media::Media;

const PER_ROW: usize = 9;
/// The picker's width, shared by the emoji and GIF tabs.
pub const PICKER_WIDTH: f32 = PER_ROW as f32 * 36.0;

/// The picker's grid with a search box. Returns the emoji clicked: the
/// character, or `:name:` for one of the network's `custom` ones.
pub fn picker(
    ui: &mut egui::Ui,
    query: &mut String,
    custom: &[chad_core::CustomEmoji],
    media: &mut Media,
) -> Option<String> {
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
    let custom: Vec<&chad_core::CustomEmoji> =
        custom.iter().filter(|e| e.name.contains(&q)).collect();
    let mut picked = None;
    if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        // A search that names a custom one means it.
        picked = match custom.first().filter(|_| !q.is_empty()) {
            Some(e) => Some(format!(":{}:", e.name)),
            None => list.first().map(|e| e.as_str().to_owned()),
        };
    }
    let rows = list.len().div_ceil(PER_ROW);
    let size = ui.text_style_height(&egui::TextStyle::Body) * 1.6;
    let any_custom = !custom.is_empty();
    if any_custom {
        egui::ScrollArea::vertical()
            .id_salt("custom")
            .max_height(size * 3.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    let cell = ui.available_width() / PER_ROW as f32;
                    for e in custom {
                        let code = format!(":{}:", e.name);
                        let b = match media.emoji_image(ui, e, size * 0.75) {
                            Some(img) => egui::Button::image(img),
                            None => egui::Button::new(RichText::new(&e.name).small()).truncate(),
                        };
                        let r = ui
                            .add_sized([cell, size], b.frame(false))
                            .on_hover_text(&code);
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Button, true, &code)
                        });
                        if r.clicked() {
                            picked = Some(code);
                        }
                    }
                });
            });
        ui.separator();
    }
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
    if list.is_empty() && !any_custom {
        ui.label(RichText::new("No emoji found.").weak());
    }
    picked
}
