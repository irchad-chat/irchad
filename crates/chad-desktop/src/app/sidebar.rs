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
        self.call_panel(ui);
        self.network_header(ui, ni);
        self.status_menu(ui, ni);
        self.buffer_list(ui, ni);
    }

    /// Our call, pinned to the bottom of the sidebar.
    fn call_panel(&mut self, ui: &mut egui::Ui) {
        if let Some((net, state)) = self.view.call.clone() {
            let name = self.net_name(&net);
            let streaming = self.voice.streaming();
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
                        streaming,
                    )
                })
                .inner;
            match action {
                Some(voice::PanelAction::Leave) => self.send(&net, NetCommand::LeaveVoice),
                Some(voice::PanelAction::Share) => {
                    let streams = self
                        .net_idx(&net)
                        .is_some_and(|i| self.view.nets[i].info.meetings);
                    self.share_dialog = Some(ShareDialog::new(&self.voice, ui.ctx(), streams));
                }
                Some(voice::PanelAction::StopShare) => self.voice.stop_share(),
                Some(voice::PanelAction::CopyLink) => {
                    if let Some(link) = &self.stream_link {
                        ui.ctx().copy_text(link.to_string());
                        self.toast("Stream link copied.".into());
                    }
                }
                Some(voice::PanelAction::Camera) => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let (v, ctx) = (self.voice.clone(), ui.ctx().clone());
                    tokio::spawn(async move {
                        if let Err(e) = v.start_camera().await {
                            let _ = tx.send(e);
                            ctx.request_repaint();
                        }
                    });
                    self.camera_error = Some(rx);
                }
                Some(voice::PanelAction::StopCamera) => self.voice.stop_camera(),
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
    }

    /// The network's name and menu, and its connection state.
    fn network_header(&mut self, ui: &mut egui::Ui, ni: usize) {
        let network = self.view.nets[ni].info.network.clone();
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
                    if self.view.nets[ni].info.meetings && ui.button("New meeting").clicked() {
                        self.send(&network, NetCommand::CreateMeeting);
                    }
                    if ui.button("Browse channels…").clicked() {
                        self.browse = Some(ChannelBrowser::new(network.clone()));
                        self.send(&network, NetCommand::ListChannels);
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
        let info = &self.view.nets[ni].info;
        // Only worth mentioning when it's slow.
        if let (ConnState::Connected, Some(ms @ 300..)) = (&info.state, info.lag_ms) {
            let color = if ms < 1500 {
                ui.visuals().warn_fg_color
            } else {
                ui.visuals().error_fg_color
            };
            ui.label(
                RichText::new(format!("Connected · {ms} ms"))
                    .small()
                    .color(color),
            )
            .tip("Round trip to the server");
        }
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
    }

    /// Our name, away state and profile.
    fn status_menu(&mut self, ui: &mut egui::Ui, ni: usize) {
        let network = self.view.nets[ni].info.network.clone();
        let info = self.view.nets[ni].info.clone();
        if info.oper {
            ui.label(RichText::new(icons::CROWN).small());
        }
        let own = info.nick;
        let shown = chatlog::shown(&self.view.nets[ni].profiles, &own).to_owned();
        let (dot, color) = if self.away_sent {
            (icons::MOON, voice::AWAY)
        } else {
            ("●", voice::ONLINE)
        };
        let name = RichText::new(if shown == own {
            own.clone()
        } else {
            format!("{shown} ({own})")
        })
        .small();
        let status = egui::Atoms::new((
            RichText::new(dot).color(color).small(),
            egui::AtomExt::atom_shrink(name, true),
        ));
        // Truncate the name rather than hold the channel list open.
        ui.scope(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
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
                        receipts: self.view.nets[ni].info.receipts,
                    });
                    self.send(&network, NetCommand::Devices { nick: own.clone() });
                    ui.close();
                }
            })
            .response
            .tip("Your status and profile");
        });
        // What everyone sees as our activity (settings → General → Activity).
        if let Some(a) = self.activity_sent.0.as_deref().filter(|a| !a.is_empty()) {
            let line = format!("{} {a}", crate::presence::icon(a));
            ui.add(egui::Label::new(RichText::new(line).small().weak()).truncate())
                .on_hover_text(a);
        }
        ui.separator();
    }

    /// Channels, then DMs.
    fn buffer_list(&mut self, ui: &mut egui::Ui, ni: usize) {
        let network = self.view.nets[ni].info.network.clone();
        let mut pick = None;
        let mut part = None;
        let mut rebuild = None;
        let mut close = None;
        let mut block: Option<(String, Option<String>)> = None;
        let mut join_voice = None;
        let mut overlay_pick = None;
        let mut watch = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                // A classic network's server buffer (D72), above everything.
                let n = &self.view.nets[ni];
                if let Some(b) = n.buffers.iter().find(|b| b.info.kind == BufferKind::Server) {
                    let selected = n.ui.selected.as_deref() == Some(&b.key);
                    let mut text = RichText::new("Server");
                    if b.info.unread > 0 || selected {
                        text = text.strong().color(ui.visuals().strong_text_color());
                    } else {
                        text = text.color(ui.visuals().weak_text_color());
                    }
                    let resp = ui
                        .with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                            ui.add(egui::Button::selectable(selected, text).min_size(vec2(0.0, 24.0)))
                        })
                        .inner
                        .on_hover_text("Server messages. Type raw IRC commands here.");
                    if resp.clicked() {
                        pick = Some(b.key.clone());
                    }
                    ui.add_space(8.0);
                }
                for (kind, title) in [
                    (BufferKind::Channel, "CHANNELS"),
                    (BufferKind::Dm, "DIRECT MESSAGES"),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(title).small().weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let hint = match kind {
                                BufferKind::Channel => "Join a channel",
                                BufferKind::Dm | BufferKind::Server => "Message someone",
                            };
                            let add = egui::Button::new(RichText::new("+").size(18.0))
                                .min_size(vec2(24.0, 22.0));
                            if ui.add(add).tip(hint).clicked() {
                                self.adding = Some((kind, String::new()));
                            }

                        });
                    });
                    let n = &mut self.view.nets[ni];
                    for b in n.buffers.iter().filter(|b| b.info.kind == kind) {
                        let selected = n.ui.selected.as_deref() == Some(&b.key);
                        let meeting = chad_core::meeting::code_of(&b.info.target);
                        let name = match kind {
                            BufferKind::Dm => chatlog::shown(&n.profiles, &b.info.target),
                            _ if meeting.is_some() => "Meeting",
                            BufferKind::Channel | BufferKind::Server => &b.info.target,
                        };
                        let mut text = RichText::new(name);
                        if b.info.unread > 0 || selected {
                            text = text.strong().color(ui.visuals().strong_text_color());
                        } else {
                            text = text.color(ui.visuals().weak_text_color());
                        }
                        // The name truncates, not the DM dot pushed in front of it.
                        let mut atoms = egui::Atoms::new(egui::AtomExt::atom_shrink(text, true));
                        let mut tip = None;
                        if meeting.is_some() {
                            atoms.push_left(RichText::new(icons::VIDEO_CAMERA));
                            tip = Some(b.info.target.clone());
                        }
                        if kind == BufferKind::Dm {
                            let (dot, color, t) = match (b.info.online, b.info.away) {
                                (Some(true), false) => ("●", voice::ONLINE, "Online"),
                                (Some(true), true) => ("◐", voice::AWAY, "Away"),
                                _ => ("○", ui.visuals().weak_text_color(), "Offline"),
                            };
                            atoms.push_left(RichText::new(dot).color(color));
                            // Display names aren't unique; the nick is.
                            tip = Some(if name == b.info.target {
                                t.to_owned()
                            } else {
                                format!("{t} · {}", b.info.target)
                            });
                        }
                        let mut resp = ui
                            .with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                                ui.add(
                                    egui::Button::selectable(selected, atoms)
                                        .truncate()
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
                            let mut icon = RichText::new(icons::SPEAKER_HIGH).size(16.0);
                            if !b.voice.is_empty() {
                                icon = icon.color(voice::ONLINE);
                            }
                            let join = egui::Button::new(icon);
                            if ui.put(at, join).tip("Join voice").clicked() {
                                join_voice = Some(b.info.target.clone());
                            }
                        }
                        resp.context_menu(|ui| match kind {
                            BufferKind::Channel => {
                                if b.info.voice && !in_call && ui.button("Join voice").clicked() {
                                    join_voice = Some(b.info.target.clone());
                                }
                                if let Some(code) = &meeting
                                    && ui.button("Copy meeting link").clicked()
                                {
                                    ui.ctx().copy_text(format!("{network}/meet/{code}"));
                                }
                                let mut shown = self.settings.overlay.shows(&network, &b.key);
                                if ui
                                    .checkbox(&mut shown, "Show in game overlay")
                                    .clicked()
                                {
                                    overlay_pick = Some(b.key.clone());
                                }
                                if ui
                                    .button("Rebuild local cache")
                                    .on_hover_text("Forget this channel's saved messages and download them again")
                                    .clicked()
                                {
                                    rebuild = Some(b.info.target.clone());
                                }
                                if ui.button("Leave channel").clicked() {
                                    part = Some(b.info.target.clone());
                                }
                            }
                            BufferKind::Server => {}
                            BufferKind::Dm => {
                                if ui.button("Close conversation").clicked() {
                                    close = Some(b.info.target.clone());
                                }
                                if ui.button("Block").clicked() {
                                    block = Some((b.info.target.clone(), None));
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
                                b.members.as_deref().unwrap_or_default(),
                                &n.profiles,
                                &mut n.ui.media,
                                self.settings.load_media,
                                &mut self.settings.volumes,
                                &mut self.settings.screen_volumes,
                                &mut self.settings.whisper_to,
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
                            BufferKind::Dm | BufferKind::Server => "username",
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
                                    BufferKind::Dm | BufferKind::Server => {
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
        match watch {
            Some((who, true)) => self.voice.watch_camera(who),
            Some((who, false)) => self.voice.watch(Some(who)),
            None => {}
        }
        if let Some(channel) = part {
            self.send(&network, NetCommand::Part { channel });
        }
        if let Some(channel) = rebuild {
            let n = &mut self.view.nets[ni];
            if let Some(bi) = n.buffer(&key(&channel)) {
                n.buffers[bi].msgs = Default::default();
                n.buffers[bi].history_start = false;
            }
            self.send(&network, NetCommand::RebuildCache { channel });
        }
        if let Some(nick) = close {
            self.send(&network, NetCommand::CloseDm { nick });
        }
        // Blocking from the DM list closes the conversation too.
        if let Some((nick, account)) = block {
            self.send(
                &network,
                NetCommand::Block {
                    nick: nick.clone(),
                    account,
                },
            );
            self.send(&network, NetCommand::CloseDm { nick });
        }
        if let Some(channel) = join_voice {
            self.send(&network, NetCommand::JoinVoice { channel });
        }
        if let Some(k) = overlay_pick {
            self.settings.overlay.toggle_channel(&network, &k);
        }
    }

    /// The member list's starting width: its widest name beside an avatar,
    /// plus the panel's margins, the scroll bar and a little air. Capped,
    /// since display names can be long; past that they (and activities) truncate.
    /// Dragging its edge overrides it.
    pub(super) fn members_width(&self, ui: &egui::Ui, ni: usize, bi: usize) -> f32 {
        let profiles = &self.view.nets[ni].profiles;
        let b = &self.view.nets[ni].buffers[bi];
        let Some(members) = &b.members else {
            return 100.0;
        };
        // shortcut: a changed display name shows at the old width until the
        // member list or profile count next changes; key on a profile revision if that bites.
        let key = (members.as_ptr() as usize, profiles.len());
        if let Some((m, p, w)) = b.ui.members_width.get()
            && (m, p) == key
        {
            return w;
        }
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
                ROW_TEXT_X + BADGE_GAP + width(format!("{}{name}", badges(m)), &body)
            })
            .chain([
                width(format!("ONLINE — {}", members.len() - away), &small),
                width(format!("AWAY — {away}"), &small),
            ])
            .fold(0.0, f32::max);
        let s = &ui.style().spacing.scroll;
        let bar = s.bar_width + s.bar_inner_margin + s.bar_outer_margin;
        // Never so narrow that an activity line is all ellipsis.
        let w = (widest + 16.0 + bar + 12.0).clamp(180.0, 360.0);
        b.ui.members_width.set(Some((key.0, key.1, w)));
        w
    }

    pub(super) fn members(&mut self, ui: &mut egui::Ui, ni: usize, bi: usize) {
        let auto = self.settings.load_media;
        let n = &mut self.view.nets[ni];
        let network = n.info.network.clone();
        let own = n.info.nick.clone();
        let (profiles, media) = (&n.profiles, &mut n.ui.media);
        let Some(members) = &n.buffers[bi].members else {
            ui.add_space(8.0);
            ui.spinner();
            return;
        };
        enum Line<'a> {
            Header(&'static str, usize),
            Member(&'a Member),
        }
        let mut lines = Vec::with_capacity(members.len() + 4);
        for (away, title) in [(false, "ONLINE"), (true, "AWAY")] {
            let start = lines.len();
            lines.push(Line::Header(title, 0));
            lines.extend(members.iter().filter(|m| m.away == away).map(Line::Member));
            match lines.len() - start - 1 {
                0 => lines.truncate(start),
                n => lines[start] = Line::Header(title, n),
            }
        }
        let mut dm = None;
        let mut block = None;
        // Each line's top; a header after a section leaves a gap above it.
        let height = |i: usize, line: &Line| match line {
            Line::Header(..) if i == 0 => HEAD_H,
            Line::Header(..) => HEAD_H + SECTION_GAP,
            Line::Member(_) => ROW_H,
        };
        let mut tops = Vec::with_capacity(lines.len() + 1);
        tops.push(0.0);
        for (i, line) in lines.iter().enumerate() {
            tops.push(tops[i] + height(i, line));
        }
        // Only the rows in view are laid out: a channel can have thousands.
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show_viewport(ui, |ui, view| {
                ui.set_height(tops[lines.len()]);
                let origin = ui.min_rect().min;
                let first = tops[1..].partition_point(|&bottom| bottom <= view.min.y);
                let last = tops
                    .partition_point(|&top| top < view.max.y)
                    .min(lines.len());
                for i in first..last {
                    let rect = egui::Rect::from_min_max(
                        origin + vec2(0.0, tops[i]),
                        egui::pos2(ui.max_rect().right(), origin.y + tops[i + 1]),
                    );
                    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                    let ui = &mut ui;
                    let m = match &lines[i] {
                        Line::Header(title, n) => {
                            let row = egui::Layout::left_to_right(egui::Align::Max);
                            ui.with_layout(row, |ui| {
                                ui.label(RichText::new(format!("{title} — {n}")).small().weak());
                            });
                            continue;
                        }
                        Line::Member(m) => m,
                    };
                    let profile = profiles.get(&key(&m.nick));
                    let r = member_row(
                        ui,
                        media,
                        auto,
                        m,
                        profile,
                        chatlog::shown(profiles, &m.nick),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                    let card = egui::Popup::from_toggle_button_response(&r)
                        // Not the default id: `context_menu` on `r` closes that one on left click.
                        .id(r.id.with("card"))
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .align(egui::RectAlign::LEFT_START)
                        .show(|ui| {
                            chatlog::profile_card(
                                ui,
                                media,
                                auto,
                                m,
                                profile,
                                chatlog::shown(profiles, &m.nick),
                                m.nick != own,
                                true,
                            )
                        });
                    if card.is_some_and(|c| c.inner) {
                        dm = Some(m.nick.clone());
                    }
                    if m.nick != own {
                        r.context_menu(|ui| {
                            if ui.button("Block").clicked() {
                                block = Some((m.nick.clone(), m.account.clone()));
                            }
                        });
                    }
                }
            });
        if let Some((nick, account)) = block {
            self.send(&network, NetCommand::Block { nick, account });
        }
        if let Some(nick) = dm {
            self.send(&network, NetCommand::OpenDm { nick: nick.clone() });
            select(&mut self.view.nets[ni], &key(&nick));
        }
    }
}

const ROW_H: f32 = 40.0;
/// A section header ("ONLINE — 4"): its small text, bottom-aligned.
const HEAD_H: f32 = 20.0;
/// Above every header but the first.
const SECTION_GAP: f32 = 12.0;
/// Between a member's name and its crown or ghost.
const BADGE_GAP: f32 = 4.0;
const ROW_AVATAR: f32 = 32.0;
/// Where a member row's text starts: past the padding, avatar and a gap.
const ROW_TEXT_X: f32 = 6.0 + ROW_AVATAR + 10.0;

/// Icons after a member's name: IRC operator, guest.
fn badges(m: &Member) -> String {
    let mut s = String::new();
    if m.oper {
        s.push_str(icons::CROWN);
        s.push(' ');
    }
    if m.account.is_none() {
        s.push_str(icons::GHOST);
        s.push(' ');
    }
    s
}

/// Channel roles from mode prefixes (`~&@%+`), highest first.
/// One member, Discord style: avatar, display name, and their activity under it.
fn member_row(
    ui: &mut egui::Ui,
    media: &mut Media,
    auto: bool,
    m: &Member,
    profile: Option<&chatlog::Profile>,
    name: &str,
) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    if !ui.is_rect_visible(rect) {
        return r;
    }
    let v = ui.visuals();
    if r.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(4), v.widgets.hovered.weak_bg_fill);
    }
    let at = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 6.0 + ROW_AVATAR / 2.0, rect.center().y),
        vec2(ROW_AVATAR, ROW_AVATAR),
    );
    chatlog::avatar_with_status(ui, media, auto, at, name, profile, m.away);

    let x = rect.left() + ROW_TEXT_X;
    let room = (rect.right() - 4.0 - x).max(0.0);
    let line = |text: RichText, style: egui::TextStyle, room: f32| {
        egui::WidgetText::from(text).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            room,
            style,
        )
    };
    let mut color = chatlog::nick_color(&m.nick, v.dark_mode);
    if m.away {
        color = color.gamma_multiply(0.55);
    }
    let marks = line(RichText::new(badges(m)).weak(), egui::TextStyle::Body, room);
    let title = line(
        RichText::new(name).color(color),
        egui::TextStyle::Body,
        (room - marks.size().x - BADGE_GAP).max(0.0),
    );
    let sub = profile.and_then(|p| p.activity.as_deref()).map(|a| {
        let t = format!("{} {a}", crate::presence::icon(a));
        line(
            RichText::new(t).small().weak(),
            egui::TextStyle::Small,
            room,
        )
    });
    let high = title.size().y + sub.as_ref().map_or(0.0, |s| s.size().y + 1.0);
    let top = rect.center().y - high / 2.0;
    let p = ui.painter();
    let fg = v.text_color();
    p.galley(egui::pos2(x + title.size().x + BADGE_GAP, top), marks, fg);
    let below = top + title.size().y + 1.0;
    p.galley(egui::pos2(x, top), title, fg);
    if let Some(sub) = sub {
        p.galley(egui::pos2(x, below), sub, fg);
    }
    r
}
