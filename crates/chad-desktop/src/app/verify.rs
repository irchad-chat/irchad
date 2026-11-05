//! Device list and emoji (SAS) verification.

use super::*;

/// One encrypted-DM device: its name, whether it's verified, and its fingerprint.
/// `removable` adds a Remove button (not on this device); returns whether it was clicked.
pub(super) fn device_row(ui: &mut egui::Ui, d: &DeviceInfo, removable: bool) -> bool {
    let remove = ui
        .horizontal(|ui| {
            let name = if d.this { "This device" } else { d.id.as_str() };
            ui.label(name);
            if d.verified {
                ui.label(icons::SHIELD_CHECK).on_hover_text("Verified");
            }
            removable
                && !d.this
                && ui
                    .small_button("Remove")
                    .on_hover_text(
                        "Stop sending encrypted DMs to this device. \
                         If it's still in use, it adds itself back.",
                    )
                    .clicked()
        })
        .inner;
    ui.label(RichText::new(&d.fingerprint).monospace().small().weak());
    remove
}

impl App {
    /// Comparing emoji with someone (SAS), one person at a time.
    pub(super) fn verify_ui(&mut self, ctx: &egui::Context) {
        let Some((network, nick, state)) = self.verify.clone() else {
            return;
        };
        let (mut step, mut close) = (None, false);
        let modal = egui::Modal::new(egui::Id::unique("verify")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(format!("Verify {nick}"));
            ui.add_space(8.0);
            match &state {
                VerifyState::Incoming => {
                    ui.label(format!(
                        "{nick} wants to make sure your messages to each other are private, \
                         by comparing emoji."
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Not now").clicked() {
                            step = Some(VerifyStep::Cancel);
                        }
                        if ui.button("Start").clicked() {
                            step = Some(VerifyStep::Accept);
                        }
                    });
                }
                VerifyState::Waiting => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Waiting for {nick}…"));
                    });
                    if ui.button("Cancel").clicked() {
                        step = Some(VerifyStep::Cancel);
                    }
                }
                VerifyState::Compare(emoji) => {
                    ui.label(format!(
                        "Check that {nick} sees the same emoji, in the same order. Compare \
                         them in person or on a call, not by message."
                    ));
                    ui.add_space(8.0);
                    // Equal-width cells wide enough that no name wraps mid-word.
                    let font = egui::TextStyle::Small.resolve(ui.style());
                    let width = emoji
                        .iter()
                        .map(|(_, n)| {
                            let g = ui.painter().layout_no_wrap(
                                n.to_string(),
                                font.clone(),
                                Color32::WHITE,
                            );
                            g.size().x
                        })
                        .fold(48.0, f32::max);
                    ui.horizontal(|ui| {
                        for (e, name) in emoji {
                            ui.allocate_ui(vec2(width, 64.0), |ui| {
                                ui.vertical_centered(|ui| {
                                    ui.label(RichText::new(*e).size(28.0));
                                    ui.add(egui::Label::new(RichText::new(*name).small()).extend());
                                });
                            });
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("They don't match").clicked() {
                            step = Some(VerifyStep::Cancel);
                        }
                        if ui.button("They match").clicked() {
                            step = Some(VerifyStep::Match);
                        }
                    });
                }
                VerifyState::Done => {
                    ui.label(format!(
                        "{} {nick}'s device is verified. Nobody is listening in.",
                        icons::SHIELD_CHECK
                    ));
                    close = ui.button("Done").clicked();
                }
                VerifyState::Failed(why) => {
                    ui.label(why);
                    close = ui.button("Close").clicked();
                }
            }
        });
        let finished = matches!(state, VerifyState::Done | VerifyState::Failed(_));
        if !finished && step.is_none() && modal.should_close() {
            step = Some(VerifyStep::Cancel);
        }
        if let Some(step) = step {
            self.send(&network, NetCommand::Verification { nick, step });
        }
        if close || (finished && modal.should_close()) {
            self.verify = None;
        }
    }
}
