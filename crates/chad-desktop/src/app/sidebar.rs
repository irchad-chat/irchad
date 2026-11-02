//! The left rail, network and channel lists, and the member list.

use super::*;

impl App {
    pub(super) fn rail(&mut self, ui: &mut egui::Ui) {
        // Settings pinned to the bottom, networks from the top.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            if ui
                .add_sized(
                    [48.0, 48.0],
                    egui::Button::new(RichText::new(icons::GEAR).size(22.0)),
                )
                .tip("Settings")
                .clicked()
            {
                self.show_settings = !self.show_settings;
            }
            ui.separator();
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                self.networks(ui);
            });
        });
    }

    pub(super) fn networks(&mut self, ui: &mut egui::Ui) {
        let mut pick = None;
        for n in &self.view.nets {
            let selected = self.current.as_deref() == Some(&n.info.network);
            let (rect, resp) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::click());
            let p = ui.painter();
            let round = if selected || resp.hovered() { 14 } else { 24 };
            match &n.ui.icon {
                Icon::Ready(tex) => {
                    egui::Image::new(tex)
                        .corner_radius(CornerRadius::same(round))
                        .paint_at(ui, rect);
                }
                _ => {
                    let fill = if selected {
                        Color32::from_rgb(88, 101, 242)
                    } else {
                        ui.visuals().widgets.inactive.bg_fill
                    };
                    p.rect_filled(rect, round, fill);
                    let initial: String = n.info.name.chars().take(1).collect();
                    p.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        initial.to_uppercase(),
                        FontId::proportional(20.0),
                        ui.visuals().strong_text_color(),
                    );
                }
            }
            if selected || n.unread() {
                let h = if selected { 36.0 } else { 8.0 };
                // In the gutter left of the icon, clear of it.
                let pill = egui::Rect::from_min_max(
                    egui::pos2(rect.left() - 9.0, rect.center().y - h / 2.0),
                    egui::pos2(rect.left() - 5.0, rect.center().y + h / 2.0),
                );
                p.rect_filled(pill, 2.0, ui.visuals().strong_text_color());
            }
            let m = n.mentions();
            if m > 0 {
                mention_badge(ui, rect.right_bottom() - vec2(6.0, 6.0), m);
            }
            if !matches!(n.info.state, ConnState::Connected) {
                p.circle_filled(rect.right_top() + vec2(-4.0, 4.0), 5.0, Color32::GRAY);
            }
            if resp.tip(&n.info.name).clicked() {
                pick = Some(n.info.network.clone());
            }
            ui.add_space(8.0);
        }
        if pick.is_some() {
            self.current = pick;
        }
        if !self.view.nets.is_empty() {
            ui.separator();
        }
        let add = ui
            .add_sized(
                [48.0, 48.0],
                egui::Button::new(RichText::new("+").size(24.0)),
            )
            .tip("Join a network");
        if add.clicked() {
            self.join = Some(JoinDialog::default());
        }
    }

    pub(super) fn channels(&mut self, ui: &mut egui::Ui, ni: usize) {
        let network = self.view.nets[ni].info.network.clone();
        if let Some((net, state)) = self.view.call.clone() {
            let name = self
                .net_idx(&net)
                .map_or(net.clone(), |i| self.view.nets[i].info.name.clone());
            let action = egui::Panel::bottom("voice-panel")
                .show(ui, |ui| {
                    voice::panel(
                        ui,
                        &self.voice,
                        &state,
                        &name,
                        self.ptt.as_ref(),
                        self.net_idx(&net)
                            .map(|i| self.view.nets[i].sounds.as_slice()),
                        &mut self.settings.audio,
                    )
                })
                .inner;
            match action {
                Some(voice::PanelAction::Leave) => self.send(&net, NetCommand::LeaveVoice),
                Some(voice::PanelAction::Share) => {
                    self.share_dialog = Some(ShareDialog::new(&self.voice, ui.ctx()));
                }
                Some(voice::PanelAction::StopShare) => self.voice.stop_share(),
                Some(voice::PanelAction::Soundboard) => self.send(&net, NetCommand::Sounds),
                Some(voice::PanelAction::Sound(file)) => {
                    match self.clips.get(&(net.clone(), file.clone())) {
                        Some(clip) => {
                            if let Err(e) = self.voice.play_sound(clip.clone()) {
                                self.toast(e.into());
                            }
                        }
                        None => self.send(&net, NetCommand::Sound { file }),
                    }
                }
                Some(voice::PanelAction::Preview) => {
                    let me = self.voice.call().and_then(|c| {
                        c.participants
                            .into_iter()
                            .find(|p| p.local)
                            .map(|p| p.identity)
                    });
                    self.voice.watch(me);
                }
                None => {}
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(icons::DOTS_THREE, |ui| {
                    if self.view.nets[ni].info.guest && ui.button("Create an account…").clicked()
                    {
                        self.signup = Some(SignUpDialog {
                            network: network.clone(),
                            account: self.view.nets[ni].info.nick.clone(),
                            password: String::new(),
                            email: String::new(),
                            busy: false,
                            error: None,
                        });
                    }
                    if ui.button("Leave network…").clicked() {
                        self.confirm_remove = Some(network.clone());
                    }
                })
                .response
                .tip("Network options");
                // Whatever's left of the menu, so long names truncate instead of overlapping it.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let name = RichText::new(&self.view.nets[ni].info.name).heading();
                    ui.add(egui::Label::new(name).truncate());
                });
            });
        });
        if let Some((text, bad)) = state_text(&self.view.nets[ni].info.state) {
            let color = if bad {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().warn_fg_color
            };
            ui.label(RichText::new(text).small().color(color));
            if bad {
                let mut go = false;
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.signin_password)
                            .password(true)
                            .hint_text("Password")
                            .desired_width(120.0),
                    );
                    go = ui.button("Sign in").clicked()
                        || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                });
                if go && !self.signin_password.is_empty() {
                    let password = std::mem::take(&mut self.signin_password);
                    self.send(&network, NetCommand::SignIn { password });
                }
            }
        }
        let own = self.view.nets[ni].info.nick.clone();
        let shown = chatlog::shown(&self.view.nets[ni].profiles, &own).to_owned();
        let (dot, color) = if self.away_sent {
            (icons::MOON, voice::AWAY)
        } else {
            ("●", voice::ONLINE)
        };
        let status = egui::Atoms::new((
            RichText::new(dot).color(color).small(),
            RichText::new(if shown == own {
                own.clone()
            } else {
                format!("{shown} ({own})")
            })
            .small(),
        ));
        ui.menu_button(status, |ui| {
            ui.radio_value(&mut self.settings.away, false, "Online");
            ui.radio_value(&mut self.settings.away, true, "Away");
            ui.separator();
            if ui
                .button(format!("{} Edit profile…", icons::USER_CIRCLE))
                .clicked()
            {
                let name = self.view.nets[ni]
                    .profiles
                    .get(&key(&own))
                    .and_then(|p| p.display_name.clone())
                    .unwrap_or_default();
                self.profile = Some(ProfileDialog {
                    network: network.clone(),
                    name,
                });
                self.send(&network, NetCommand::Devices { nick: own.clone() });
                ui.close();
            }
        })
        .response
        .tip("Your status and profile");
        ui.separator();

        let mut pick = None;
        let mut part = None;
        let mut close = None;
        let mut join_voice = None;
        let mut watch = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (kind, title) in [
                    (BufferKind::Channel, "CHANNELS"),
                    (BufferKind::Dm, "DIRECT MESSAGES"),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(title).small().weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let hint = match kind {
                                BufferKind::Channel => "Join a channel",
                                BufferKind::Dm => "Message someone",
                            };
                            let add = egui::Button::new(RichText::new("+").size(18.0))
                                .min_size(vec2(24.0, 22.0));
                            if ui.add(add).tip(hint).clicked() {
                                self.adding = Some((kind, String::new()));
                            }
                        });
                    });
                    let n = &self.view.nets[ni];
                    for b in n.buffers.iter().filter(|b| b.info.kind == kind) {
                        let selected = n.ui.selected.as_deref() == Some(&b.key);
                        let mut text = RichText::new(&b.info.target);
                        if b.info.unread > 0 || selected {
                            text = text.strong().color(ui.visuals().strong_text_color());
                        } else {
                            text = text.color(ui.visuals().weak_text_color());
                        }
                        let mut atoms = egui::Atoms::new(text);
                        let mut tip = None;
                        if kind == BufferKind::Dm {
                            let (dot, color, t) = match (b.info.online, b.info.away) {
                                (Some(true), false) => ("●", voice::ONLINE, "Online"),
                                (Some(true), true) => ("◐", voice::AWAY, "Away"),
                                _ => ("○", ui.visuals().weak_text_color(), "Offline"),
                            };
                            atoms.push_left(RichText::new(dot).color(color));
                            tip = Some(t);
                        }
                        let mut resp = ui
                            .with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                                ui.add(
                                    egui::Button::selectable(selected, atoms)
                                        .min_size(vec2(0.0, 24.0)),
                                )
                            })
                            .inner;
                        if let Some(tip) = tip {
                            resp = resp.on_hover_text(tip);
                        }
                        if b.info.mentions > 0 {
                            mention_badge(
                                ui,
                                resp.rect.right_center() - vec2(14.0, 0.0),
                                b.info.mentions,
                            );
                        }
                        let in_call = matches!(
                            &self.view.call,
                            Some((net, VoiceState::Connecting { channel } | VoiceState::Connected { channel }))
                                if *net == network && key(channel) == b.key
                        );
                        if b.info.voice && !in_call {
                            let right = if b.info.mentions > 0 { 38.0 } else { 16.0 };
                            let at = egui::Rect::from_center_size(
                                resp.rect.right_center() - vec2(right, 0.0),
                                vec2(26.0, 22.0),
                            );
                            let join =
                                egui::Button::new(RichText::new(icons::SPEAKER_HIGH).size(16.0));
                            if ui.put(at, join).tip("Join voice").clicked() {
                                join_voice = Some(b.info.target.clone());
                            }
                        }
                        resp.context_menu(|ui| match kind {
                            BufferKind::Channel => {
                                if b.info.voice && !in_call && ui.button("Join voice").clicked() {
                                    join_voice = Some(b.info.target.clone());
                                }
                                if ui.button("Leave channel").clicked() {
                                    part = Some(b.info.target.clone());
                                }
                            }
                            BufferKind::Dm => {
                                if ui.button("Close conversation").clicked() {
                                    close = Some(b.info.target.clone());
                                }
                            }
                        });
                        if resp.clicked() {
                            pick = Some(b.key.clone());
                        }
                        let live = in_call
                            && matches!(&self.view.call, Some((_, VoiceState::Connected { .. })));
                        if (live || !b.voice.is_empty())
                            && let Some(who) = voice::members(
                                ui,
                                &self.voice,
                                live,
                                &b.voice,
                                &mut self.settings.volumes,
                                &mut self.settings.screen_volumes,
                            )
                        {
                            watch = Some(who);
                        }
                    }
                    if let Some((k, text)) = &mut self.adding
                        && *k == kind
                    {
                        let hint = match kind {
                            BufferKind::Channel => "#channel",
                            BufferKind::Dm => "username",
                        };
                        let r = ui.add(
                            egui::TextEdit::singleline(text)
                                .hint_text(hint)
                                .id(adding_id()),
                        );
                        // Focus it while open: the "+" click itself would steal focus
                        // on the first frame, leaving a field that never closes.
                        if !r.has_focus() && !r.lost_focus() {
                            r.request_focus();
                        }
                        // Escape or clicking away cancels; Enter submits.
                        if r.lost_focus() {
                            let enter = ui.input(|i| i.key_pressed(Key::Enter));
                            let name = text.trim().to_owned();
                            let kind = *k;
                            self.adding = None;
                            if enter && !name.is_empty() {
                                let cmd = match kind {
                                    BufferKind::Channel => {
                                        let ch = if name.starts_with('#') {
                                            name
                                        } else {
                                            format!("#{name}")
                                        };
                                        pick = Some(key(&ch));
                                        NetCommand::JoinChannel { channel: ch }
                                    }
                                    BufferKind::Dm => {
                                        pick = Some(key(&name));
                                        NetCommand::OpenDm { nick: name }
                                    }
                                };
                                self.send(&network, cmd);
                            }
                        }
                    }
                    ui.add_space(8.0);
                }
            });
        if let Some(k) = pick {
            select(&mut self.view.nets[ni], &k);
            // Back to the chat; a share that isn't in its place keeps playing.
            if self.settings.stage == Layout::Fill {
                self.voice.watch(None);
            }
        }
        if let Some(who) = watch {
            self.voice.watch(Some(who));
        }
        if let Some(channel) = part {
            self.send(&network, NetCommand::Part { channel });
        }
        if let Some(nick) = close {
            self.send(&network, NetCommand::CloseDm { nick });
        }
        if let Some(channel) = join_voice {
            self.send(&network, NetCommand::JoinVoice { channel });
        }
    }

    /// The member list's narrowest width: its widest row, plus the panel's
    /// margins, the scroll bar and a little air. Capped, since display names
    /// can be long; past that they truncate.
    pub(super) fn members_width(&self, ui: &egui::Ui, ni: usize, bi: usize) -> f32 {
        let profiles = &self.view.nets[ni].profiles;
        let Some(members) = &self.view.nets[ni].buffers[bi].members else {
            return 100.0;
        };
        let body = egui::TextStyle::Body.resolve(ui.style());
        let small = egui::TextStyle::Small.resolve(ui.style());
        let width = |text: String, font: &FontId| {
            ui.painter()
                .layout_no_wrap(text, font.clone(), Color32::WHITE)
                .size()
                .x
        };
        let away = members.iter().filter(|m| m.away).count();
        let widest = members
            .iter()
            .map(|m| {
                let name = chatlog::shown(profiles, &m.nick);
                let icon = if m.oper {
                    format!("{} ", icons::WRENCH)
                } else {
                    String::new()
                };
                width(format!("{icon}{}{name}", m.prefix), &body)
            })
            .chain([
                width(format!("ONLINE — {}", members.len() - away), &small),
                width(format!("AWAY — {away}"), &small),
            ])
            .fold(0.0, f32::max);
        let s = &ui.style().spacing.scroll;
        let bar = s.bar_width + s.bar_inner_margin + s.bar_outer_margin;
        (widest + 16.0 + bar + 12.0).clamp(100.0, 360.0)
    }

    pub(super) fn members(&mut self, ui: &mut egui::Ui, ni: usize, bi: usize) {
        let dark = ui.visuals().dark_mode;
        let network = self.view.nets[ni].info.network.clone();
        let own = self.view.nets[ni].info.nick.clone();
        let profiles = &self.view.nets[ni].profiles;
        let Some(members) = &self.view.nets[ni].buffers[bi].members else {
            ui.add_space(8.0);
            ui.spinner();
            return;
        };
        let mut dm = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (away, title) in [(false, "ONLINE"), (true, "AWAY")] {
                    let list: Vec<&Member> = members.iter().filter(|m| m.away == away).collect();
                    if list.is_empty() {
                        continue;
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("{title} — {}", list.len()))
                            .small()
                            .weak(),
                    );
                    for m in list {
                        let name = chatlog::shown(profiles, &m.nick);
                        let mut t = RichText::new(format!("{}{name}", m.prefix))
                            .color(chatlog::nick_color(&m.nick, dark));
                        if away {
                            t = t.weak();
                        }
                        let r = ui
                            .horizontal(|ui| {
                                if m.oper {
                                    ui.label(icons::WRENCH).on_hover_text("IRC operator");
                                }
                                if m.account.is_none() {
                                    ui.label(RichText::new(icons::GHOST).weak())
                                        .on_hover_text("Guest (not logged in)");
                                }
                                ui.add(egui::Label::new(t).sense(Sense::click()).truncate())
                                    .on_hover_text(match &m.account {
                                        Some(a) => format!("{} · account {a}", m.nick),
                                        None => format!("{} · not logged in", m.nick),
                                    })
                            })
                            .inner;
                        if r.clicked() && m.nick != own {
                            dm = Some(m.nick.clone());
                        }
                    }
                }
            });
        if let Some(nick) = dm {
            self.send(&network, NetCommand::OpenDm { nick: nick.clone() });
            select(&mut self.view.nets[ni], &key(&nick));
        }
    }
}
