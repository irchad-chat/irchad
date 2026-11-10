//! Modal dialogs and popups.

use super::verify::device_row;
use super::*;

impl App {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        self.share_dialog(ctx);
        self.join_dialog(ctx);
        self.signup_dialog(ctx);
        self.browse_dialog(ctx);
        self.email_code_dialog(ctx);
        self.remove_dialog(ctx);
        self.settings_window(ctx);
        self.update_banner(ctx);
        self.toasts(ctx);
    }

    /// The screen-share picker.
    fn share_dialog(&mut self, ctx: &egui::Context) {
        if let Some(d) = &mut self.share_dialog {
            let s = &mut self.settings;
            match d.show(ctx, &self.voice, &mut s.screen, &mut s.restore_token) {
                DialogOutcome::Open => {}
                DialogOutcome::Closed => self.share_dialog = None,
                DialogOutcome::Shared(token) => {
                    if token.is_some() {
                        s.restore_token = token;
                    }
                    self.share_dialog = None;
                }
            }
        }
    }

    /// Joining a network from an invite.
    fn join_dialog(&mut self, ctx: &egui::Context) {
        if let Some(j) = &mut self.join {
            let mut submit = false;
            let mut cancel = false;
            let busy = j.busy.is_some();
            let modal = egui::Modal::new(egui::Id::unique("join")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading("Join a network");
                ui.add_space(8.0);
                let was_classic = j.classic;
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        ui.selectable_value(&mut j.classic, false, "IrChad server");
                        ui.selectable_value(&mut j.classic, true, "Classic IRC");
                    });
                });
                // An invite URL isn't a host, nor the other way round.
                if j.classic != was_classic {
                    j.invite.clear();
                }
                ui.add_space(4.0);
                if j.classic {
                    if j.mode == JoinMode::Register {
                        j.mode = JoinMode::Login;
                    }
                    ui.horizontal(|ui| {
                        let l = ui.label("Server");
                        ui.add_enabled(
                            !busy,
                            egui::TextEdit::singleline(&mut j.invite)
                                .hint_text("irc.libera.chat")
                                .desired_width(200.0),
                        )
                        .labelled_by(l.id);
                        let l = ui.label("Port");
                        let default = if j.plaintext { "6667" } else { "6697" };
                        ui.add_enabled(
                            !busy,
                            egui::TextEdit::singleline(&mut j.port)
                                .hint_text(default)
                                .desired_width(f32::INFINITY),
                        )
                        .labelled_by(l.id);
                    });
                    let mut tls = !j.plaintext;
                    ui.add_enabled(!busy, egui::Checkbox::new(&mut tls, "Encrypted (TLS)"));
                    j.plaintext = !tls;
                    if j.plaintext {
                        ui.label(
                            RichText::new(
                                "Without TLS, your messages and password \
                                 cross the network unencrypted.",
                            )
                            .color(ui.visuals().warn_fg_color),
                        );
                    }
                } else {
                    let l = ui.label("Server URL");
                    ui.add_enabled(
                        !busy,
                        egui::TextEdit::singleline(&mut j.invite)
                            .hint_text("chat.example.com")
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(l.id);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        ui.selectable_value(&mut j.mode, JoinMode::Login, "I have an account");
                        if !j.classic {
                            ui.selectable_value(
                                &mut j.mode,
                                JoinMode::Register,
                                "Create an account",
                            );
                        }
                        let guest = if j.classic {
                            "Just a nickname"
                        } else {
                            "Guest"
                        };
                        ui.selectable_value(&mut j.mode, JoinMode::Guest, guest);
                    });
                });
                ui.add_space(4.0);
                egui::Grid::new("join-fields")
                    .num_columns(2)
                    .show(ui, |ui| {
                        let guest = j.mode == JoinMode::Guest;
                        let l = ui.label(if guest { "Nickname" } else { "Username" });
                        let field =
                            |text| egui::TextEdit::singleline(text).desired_width(f32::INFINITY);
                        ui.add_enabled(!busy, field(&mut j.account))
                            .labelled_by(l.id);
                        ui.end_row();
                        if !guest {
                            let l = ui.label("Password");
                            ui.add_enabled(!busy, field(&mut j.password).password(true))
                                .labelled_by(l.id);
                            ui.end_row();
                        }
                        if j.mode == JoinMode::Register {
                            let l = ui.label("Email");
                            ui.add_enabled(
                                !busy,
                                field(&mut j.email).hint_text("if the network asks for one"),
                            )
                            .labelled_by(l.id);
                            ui.end_row();
                        }
                    });
                if j.mode == JoinMode::Guest && !j.classic {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Guests miss what's said while they're away, \
                             and anyone can take the nickname.",
                        )
                        .weak(),
                    );
                }
                if let Some(e) = &j.error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
                }
                ui.add_space(8.0);
                let ready = !j.invite.trim().is_empty()
                    && !j.account.trim().is_empty()
                    && (j.mode == JoinMode::Guest || !j.password.is_empty());
                let label = match j.mode {
                    JoinMode::Login => "Log in",
                    JoinMode::Register => "Sign up",
                    JoinMode::Guest => "Join",
                };
                let (go, no) = footer(ui, label, ready && !busy, busy);
                submit = go || (ready && !busy && ui.input(|i| i.key_pressed(Key::Enter)));
                cancel = no;
            });
            if submit {
                let url = if j.classic {
                    let scheme = if j.plaintext { "irc" } else { "ircs" };
                    let port = j.port.trim();
                    let port = if port.is_empty() {
                        String::new()
                    } else {
                        format!(":{port}")
                    };
                    format!("{scheme}://{}{port}", j.invite.trim())
                } else {
                    j.invite.trim().to_owned()
                };
                match invite::parse(&url) {
                    Err(e) => j.error = Some(e),
                    Ok(_) => {
                        let (account, password) = (j.account.trim().to_owned(), j.password.clone());
                        let auth = match j.mode {
                            JoinMode::Register => {
                                let email =
                                    Some(j.email.trim().to_owned()).filter(|e| !e.is_empty());
                                Auth::Register {
                                    account,
                                    password,
                                    email,
                                }
                            }
                            JoinMode::Login => Auth::Login { account, password },
                            JoinMode::Guest => Auth::Guest { nick: account },
                        };
                        j.error = None;
                        j.busy = Some(url.clone());
                        self.core.send(Command::Join { invite: url, auth });
                    }
                }
            }
            // A join in flight can't be cancelled; it ends on its own.
            if (cancel || modal.should_close()) && !busy {
                self.join = None;
            }
        }
    }

    /// "Browse channels…": filter the LIST and join from it.
    fn browse_dialog(&mut self, ctx: &egui::Context) {
        let Some(b) = &mut self.browse else { return };
        let mut join = None;
        let modal = egui::Modal::new(egui::Id::unique("browse")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading("Browse channels");
            ui.add_space(8.0);
            let l = ui.label("Search");
            ui.add(
                egui::TextEdit::singleline(&mut b.filter)
                    .hint_text("name or topic")
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(l.id);
            ui.add_space(8.0);
            let Some(channels) = &b.channels else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Asking the server…");
                });
                return;
            };
            let filter = b.filter.trim().to_lowercase();
            let shown: Vec<_> = channels
                .iter()
                .filter(|c| {
                    filter.is_empty()
                        || c.name.to_lowercase().contains(&filter)
                        || c.topic.to_lowercase().contains(&filter)
                })
                .collect();
            if shown.is_empty() {
                ui.label(RichText::new("No channels match.").weak());
            }
            let row = ui.text_style_height(&egui::TextStyle::Body) * 2.4;
            egui::ScrollArea::vertical()
                .max_height(400.0)
                .auto_shrink([false, true])
                .show_rows(ui, row, shown.len(), |ui, range| {
                    for c in &shown[range] {
                        ui.horizontal(|ui| {
                            if ui.button("Join").clicked() {
                                join = Some(c.name.clone());
                            }
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(format!("{} · {}", c.name, c.users)).strong(),
                                );
                                let topic = format::strip(&c.topic);
                                ui.add(egui::Label::new(RichText::new(topic).weak()).truncate());
                            });
                        });
                    }
                });
        });
        if let Some(channel) = join {
            let network = b.network.clone();
            let k = key(&channel);
            self.send(&network, NetCommand::JoinChannel { channel });
            if let Some(ni) = self.net_idx(&network) {
                select(&mut self.view.nets[ni], &k);
            }
            self.browse = None;
        } else if modal.should_close() {
            self.browse = None;
        }
    }

    /// Signing up a guest.
    fn signup_dialog(&mut self, ctx: &egui::Context) {
        if let Some(name) = self.signup.as_ref().map(|s| self.net_name(&s.network))
            && let Some(s) = &mut self.signup
        {
            let mut submit = false;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::unique("signup")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading(format!("Create an account on {name}"));
                ui.label(
                    RichText::new(
                        "Keep your name, catch up on what's said while you're away, \
                         and use voice and encrypted DMs.",
                    )
                    .weak(),
                );
                ui.add_space(8.0);
                egui::Grid::new("signup-fields")
                    .num_columns(2)
                    .show(ui, |ui| {
                        let field =
                            |text| egui::TextEdit::singleline(text).desired_width(f32::INFINITY);
                        let l = ui.label("Username");
                        ui.add_enabled(!s.busy, field(&mut s.account))
                            .labelled_by(l.id);
                        ui.end_row();
                        let l = ui.label("Password");
                        ui.add_enabled(!s.busy, field(&mut s.password).password(true))
                            .labelled_by(l.id);
                        ui.end_row();
                        let l = ui.label("Email");
                        ui.add_enabled(
                            !s.busy,
                            field(&mut s.email).hint_text("if the network asks for one"),
                        )
                        .labelled_by(l.id);
                        ui.end_row();
                    });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("You'll reconnect for a moment; your channels come with you.")
                        .weak(),
                );
                if let Some(e) = &s.error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
                }
                ui.add_space(8.0);
                let ready = !s.account.trim().is_empty() && !s.password.is_empty() && !s.busy;
                let (go, no) = footer(ui, "Sign up", ready, s.busy);
                submit = go || (ready && ui.input(|i| i.key_pressed(Key::Enter)));
                cancel = no;
            });
            if submit {
                s.busy = true;
                s.error = None;
                self.verify_dismissed = None;
                self.core.send(Command::Net {
                    network: s.network.clone(),
                    cmd: NetCommand::Register {
                        account: s.account.trim().to_owned(),
                        password: s.password.clone(),
                        email: Some(s.email.trim().to_owned()).filter(|e| !e.is_empty()),
                    },
                });
            }
            // A sign-up in flight can't be cancelled; it ends on its own.
            if (cancel || modal.should_close()) && !s.busy {
                self.signup = None;
            }
        }
    }

    /// The code from the sign-up email.
    fn email_code_dialog(&mut self, ctx: &egui::Context) {
        if let Some(ni) = self.current.as_deref().and_then(|c| self.net_idx(c))
            && let ConnState::NeedsVerification { message } = &self.view.nets[ni].info.state
            && self.verify_dismissed.as_deref() != Some(&self.view.nets[ni].info.network)
        {
            let network = self.view.nets[ni].info.network.clone();
            let mut done = false;
            let modal = egui::Modal::new(egui::Id::unique("verify")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.heading("Check your email");
                ui.label(message);
                ui.add(egui::TextEdit::singleline(&mut self.verify_code).hint_text("Code"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Not now").clicked() {
                        done = true;
                    }
                    if ui.button("Verify").clicked() && !self.verify_code.trim().is_empty() {
                        let code = std::mem::take(&mut self.verify_code).trim().to_owned();
                        self.core.send(Command::Net {
                            network: network.clone(),
                            cmd: NetCommand::Verify { code },
                        });
                    }
                });
            });
            if done || modal.should_close() {
                self.verify_dismissed = Some(network);
            }
        }
    }

    /// Confirming a network's removal.
    fn remove_dialog(&mut self, ctx: &egui::Context) {
        if let Some(network) = self.confirm_remove.clone() {
            let name = self.net_name(&network);
            let guest = self
                .net_idx(&network)
                .is_some_and(|i| self.view.nets[i].info.guest);
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::unique("remove")).show(ctx, |ui| {
                ui.heading(format!("Leave {name}?"));
                ui.label("IrChad forgets this network and your saved password for it.");
                ui.label(
                    "This device's encryption keys go too, so it can't read encrypted DMs again.",
                );
                if self.delete_account {
                    ui.label("Your account is deleted from the server, and its channels with it. The name stays reserved.");
                } else {
                    ui.label("Your account stays, so you can join again later.");
                }
                if !guest {
                    ui.checkbox(&mut self.delete_account, "Delete account");
                }
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.delete_account { "Delete and leave" } else { "Leave" };
                    let leave = egui::Button::new(RichText::new(label).color(Color32::WHITE))
                        .fill(Color32::from_rgb(218, 55, 60));
                    let leave = ui.add(leave).clicked();
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if leave {
                        if self.delete_account {
                            self.send(&network, NetCommand::DeleteAccount);
                        } else {
                            self.core.send(Command::Remove {
                                network: network.clone(),
                            });
                        }
                        close = true;
                    }
                });
            });
            if close || modal.should_close() {
                self.confirm_remove = None;
                self.delete_account = false;
            }
        }
    }

    /// The settings window.
    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        egui::Window::new("Settings")
            .open(&mut open)
            .pivot(Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(110.0);
                        for &s in settings::Section::ALL {
                            let r = ui.add_sized(
                                [110.0, 24.0],
                                egui::Button::selectable(self.settings_section == s, s.name()),
                            );
                            if r.clicked() {
                                self.settings_section = s;
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        // Fixed, so switching pages doesn't resize the window.
                        ui.set_min_size(egui::vec2(560.0, 420.0));
                        ui.set_max_width(560.0);
                        let section = self.settings_section;
                        ui.heading(section.name());
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .max_height(420.0)
                            .show(ui, |ui| self.settings_page(ctx, ui, section));
                    });
                });
            });
        // "Hear yourself" and a key bind only last while their page is up.
        let bind_page = match self.binding_ptt {
            Some(KeyAction::Overlay) => settings::Section::Overlay,
            _ => settings::Section::Voice,
        };
        if !open || self.settings_section != bind_page {
            self.binding_ptt = None;
        }
        if (!open || self.settings_section != settings::Section::Voice) && self.voice.testing() {
            self.voice.set_testing(false);
        }
        self.show_settings = open;

        self.picker_ui(ctx);
        self.switcher_ui(ctx);
        self.profile_ui(ctx);
        self.verify_ui(ctx);
    }

    /// A newer release is out.
    fn update_banner(&mut self, ctx: &egui::Context) {
        if let Some((version, url)) = self.new_version.clone() {
            let mut close = false;
            egui::Area::new(egui::Id::unique("update"))
                .anchor(Align2::RIGHT_TOP, vec2(-16.0, 16.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("IrChad {version} is out."));
                            ui.hyperlink_to("Get it", url);
                            close = ui.small_button(icons::X).tip("Dismiss").clicked();
                        });
                    });
                });
            if close {
                self.new_version = None;
            }
        }

        self.toasts.retain(|(_, t)| t.elapsed() < TOAST_TTL);
    }

    /// Errors and notices, bottom right.
    fn toasts(&mut self, ctx: &egui::Context) {
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(Duration::from_secs(1));
            egui::Area::new(egui::Id::unique("toasts"))
                .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -72.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let mut dismiss = HashSet::new();
                    for (i, (text, _)) in self.toasts.iter().enumerate() {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_max_width(320.0);
                            ui.horizontal(|ui| {
                                ui.label(text);
                                if ui.small_button(icons::X).tip("Dismiss").clicked() {
                                    dismiss.insert(i);
                                }
                            });
                        });
                    }
                    let mut i = 0;
                    self.toasts.retain(|_| {
                        i += 1;
                        !dismiss.contains(&(i - 1))
                    });
                });
        }
    }
}

impl App {
    fn settings_page(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        section: settings::Section,
    ) {
        if section == settings::Section::Blocked {
            self.blocked_page(ui);
            return;
        }
        if section == settings::Section::Overlay {
            self.overlay_page(ctx, ui);
            return;
        }
        if section != settings::Section::Voice {
            #[cfg(windows)]
            let autostart = self.settings.autostart;
            let players = self
                .poll
                .as_ref()
                .map(|p| p.rx.borrow().players.clone())
                .unwrap_or_default();
            if self.settings.ui(ui, section, &players) {
                self.settings.apply(ctx);
                #[cfg(windows)]
                if self.settings.autostart != autostart {
                    platform::autostart(self.settings.autostart);
                }
            }
            return;
        }
        let ptt = (
            self.settings.audio.mode,
            self.settings.ptt,
            self.settings.whisper,
        );
        if voice::settings(
            ui,
            &mut self.settings,
            &self.voice,
            self.ptt.as_ref(),
            &mut self.binding_ptt,
        ) {
            self.voice.set_settings(self.settings.audio.clone());
            if (
                self.settings.audio.mode,
                self.settings.ptt,
                self.settings.whisper,
            ) != ptt
            {
                self.restart_ptt(ctx);
            }
        }
    }

    /// Everyone blocked, per network, each with an Unblock button.
    fn blocked_page(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(
                "You won't see anything they send. Only on this device; they aren't told.",
            )
            .weak(),
        );
        ui.add_space(8.0);
        let mut unblock = None;
        let many = self.view.nets.len() > 1;
        if self.view.nets.iter().all(|n| n.blocks.is_empty()) {
            ui.label("Nobody blocked. Right-click someone's name to block them.");
        }
        for n in self.view.nets.iter().filter(|n| !n.blocks.is_empty()) {
            if many {
                ui.strong(&n.info.name);
            }
            for b in &n.blocks {
                ui.horizontal(|ui| {
                    ui.label(&b.nick);
                    match b.mask.strip_prefix("$a:") {
                        Some(account) => {
                            ui.label(RichText::new(format!("account {account}")).weak())
                        }
                        None => ui.label(RichText::new(&b.mask).weak()),
                    };
                    let right = egui::Layout::right_to_left(egui::Align::Center);
                    ui.with_layout(right, |ui| {
                        if ui.button("Unblock").clicked() {
                            unblock = Some((n.info.network.clone(), b.mask.clone()));
                        }
                    });
                });
            }
            ui.add_space(8.0);
        }
        if let Some((network, mask)) = unblock {
            self.send(&network, NetCommand::Unblock { mask });
        }
    }

    pub(super) fn picker_ui(&mut self, ctx: &egui::Context) {
        let here = self.on_screen();
        // GIFs only from the composer, on a network that searches them.
        let gif_net = here
            .as_ref()
            .and_then(|(n, _)| self.net_idx(n))
            .and_then(|i| Some((i, self.view.nets[i].info.gifs.clone()?)));
        // Custom emoji are those of the network the message or composer is on.
        let emoji_net = self
            .picker
            .as_ref()
            .and_then(|p| p.react.as_ref().map(|r| r.0.clone()))
            .or_else(|| here.as_ref().map(|(n, _)| n.clone()))
            .and_then(|n| self.net_idx(&n));
        let Some(p) = &mut self.picker else { return };
        let gif_net = gif_net.filter(|_| p.react.is_none());
        let nets = &mut self.view.nets;
        let (mut picked, mut gif) = (None, None);
        let area = egui::Area::new(egui::Id::unique("emoji-picker"))
            .fixed_pos(p.at)
            .pivot(Align2::RIGHT_BOTTOM)
            .constrain(true)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(emoji::PICKER_WIDTH);
                    match &gif_net {
                        Some((ni, provider)) => {
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut p.gif_tab, false, "Emoji");
                                ui.selectable_value(&mut p.gif_tab, true, "GIFs");
                            });
                            if p.gif_tab {
                                gif = p.gifs.ui(ui, &mut nets[*ni].ui.media, provider);
                            } else {
                                picked = emoji_picker(ui, &mut p.query, nets, emoji_net);
                            }
                        }
                        None => picked = emoji_picker(ui, &mut p.query, nets, emoji_net),
                    }
                });
            });
        let close = (p.shown && area.response.clicked_elsewhere())
            || ctx.input(|i| i.key_pressed(Key::Escape));
        p.shown = true;
        let react = p.react.clone();
        match (gif, here) {
            (Some(gifs::Action::Fetch { query, pos }), Some((network, _))) => {
                self.send(&network, NetCommand::Gifs { query, pos });
            }
            // Posted as a plain link: every client can open it, and IrChad
            // shows it inline.
            (Some(gifs::Action::Pick(g)), Some((network, target))) => {
                if let Some(b) = self.view.buf_mut(&network, &target) {
                    let reply_to = b.ui.reply.take().map(|(id, _)| id);
                    b.ui.log.scroll_to_bottom();
                    self.send(
                        &network,
                        NetCommand::Send {
                            target,
                            text: g.url.into(),
                            reply_to,
                            mentions: Vec::new(),
                        },
                    );
                }
                self.picker = None;
                self.refocus_composer();
                return;
            }
            _ => {}
        }
        if let Some(e) = picked {
            match react {
                Some((network, target, msgid)) => {
                    self.send(
                        &network,
                        NetCommand::React {
                            target,
                            msgid,
                            emoji: e,
                        },
                    );
                }
                None => {
                    if let Some((network, target)) = self.on_screen()
                        && let Some(b) = self.view.buf_mut(&network, &target)
                    {
                        // At the caret, which then sits after the emoji.
                        let id = egui::Id::unique(("composer", &network, &b.key));
                        let state = egui::TextEdit::load_state(ctx, id);
                        let end = b.ui.draft.chars().count();
                        let at = state
                            .as_ref()
                            .and_then(|s| s.cursor.char_range())
                            .map_or(end, |r| usize::from(r.primary.index).min(end));
                        let byte =
                            b.ui.draft
                                .char_indices()
                                .nth(at)
                                .map_or(b.ui.draft.len(), |(i, _)| i);
                        b.ui.draft.insert_str(byte, &e);
                        if let Some(mut s) = state {
                            let c = egui::text::CCursor::new(at + e.chars().count());
                            s.cursor
                                .set_char_range(Some(egui::text::CCursorRange::one(c)));
                            s.store(ctx, id);
                        }
                    }
                }
            }
            self.picker = None;
            self.refocus_composer();
        } else if close {
            self.picker = None;
        }
    }

    /// Back to typing in the buffer on screen, e.g. after the picker.
    pub(super) fn refocus_composer(&mut self) {
        if let Some(ni) = self.current.as_deref().and_then(|c| self.net_idx(c)) {
            self.view.nets[ni].ui.focus = true;
        }
    }

    pub(super) fn switcher_ui(&mut self, ctx: &egui::Context) {
        let Some(sw) = &mut self.switcher else { return };
        let q = sw.query.trim().to_lowercase();
        // (network, key, label), names starting with the query first.
        let mut hits: Vec<(bool, String, String, String)> = self
            .view
            .nets
            .iter()
            .flat_map(|n| {
                n.buffers.iter().filter_map(|b| {
                    let name = b.info.target.to_lowercase();
                    let pos = name.trim_start_matches('#').find(&q)?;
                    let label = format!("{}  ·  {}", b.info.target, n.info.name);
                    Some((pos != 0, n.info.network.clone(), b.key.clone(), label))
                })
            })
            .collect();
        hits.sort_by(|a, b| (a.0, &a.3).cmp(&(b.0, &b.3)));
        hits.truncate(10);
        let mut go = None;
        let (up, down, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.key_pressed(Key::Enter),
            )
        });
        if !hits.is_empty() {
            if up {
                sw.sel = sw.sel.saturating_sub(1);
            }
            if down {
                sw.sel = (sw.sel + 1).min(hits.len() - 1);
            }
            sw.sel = sw.sel.min(hits.len() - 1);
        }
        let modal = egui::Modal::new(egui::Id::unique("switcher")).show(ctx, |ui| {
            ui.set_width(360.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut sw.query)
                    .hint_text("Where to? Channel or person")
                    .desired_width(f32::INFINITY),
            );
            ui.ctx()
                .accesskit_node_builder(r.id, |n| n.set_label("Where to? Channel or person"));
            if !r.has_focus() && !r.lost_focus() {
                r.request_focus();
            }
            if r.changed() {
                sw.sel = 0;
            }
            for (i, (_, net, k, label)) in hits.iter().enumerate() {
                let b = ui.add(egui::Button::selectable(i == sw.sel, label.as_str()));
                if b.clicked() || (enter && i == sw.sel) {
                    go = Some((net.clone(), k.clone()));
                }
            }
            if hits.is_empty() {
                ui.label(RichText::new("Nothing by that name.").weak());
            }
        });
        if let Some((net, k)) = go {
            if let Some(i) = self.net_idx(&net) {
                self.current = Some(net);
                select(&mut self.view.nets[i], &k);
            }
            self.switcher = None;
        } else if modal.should_close() {
            self.switcher = None;
        }
    }

    pub(super) fn profile_ui(&mut self, ctx: &egui::Context) {
        let Some(network) = self.profile.as_ref().map(|d| d.network.clone()) else {
            return;
        };
        let Some(ni) = self.net_idx(&network) else {
            self.profile = None;
            return;
        };
        let Some(d) = &mut self.profile else { return };
        let n = &mut self.view.nets[ni];
        let own = n.info.nick.clone();
        let avatar = n.profiles.get(&key(&own)).and_then(|p| p.avatar.clone());
        let (mut save, mut close, mut choose, mut remove) = (false, false, false, false);
        let mut drop_device = None;
        let modal = egui::Modal::new(egui::Id::unique("profile")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.heading(format!("Your profile on {}", n.info.name));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
                n.ui.media.avatar(ui, rect, &own, avatar.as_ref(), true);
                ui.vertical(|ui| {
                    if n.info.uploads.is_some() {
                        choose = ui.button("Choose a picture…").clicked();
                    }
                    if avatar.is_some() {
                        remove = ui.button("Remove picture").clicked();
                    }
                });
            });
            ui.add_space(8.0);
            let l = ui.label("Display name");
            ui.add(
                egui::TextEdit::singleline(&mut d.name)
                    .hint_text(&own)
                    .char_limit(64)
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(l.id);
            ui.label(
                RichText::new(format!("People still see that you're {own}."))
                    .small()
                    .weak(),
            );
            if !n.info.guest {
                ui.add_space(8.0);
                ui.checkbox(&mut d.receipts, "Read receipts in DMs");
                ui.label(
                    RichText::new(
                        "Show when you've read a DM, and see when they have. \
                         Only between people who both turn this on.",
                    )
                    .small()
                    .weak(),
                );
            }
            if let Some(devices) = n.devices.get(&key(&own)).filter(|d| !d.is_empty()) {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("{} Devices with encrypted DMs", icons::LOCK)).strong(),
                );
                for d in devices {
                    if device_row(ui, d, true) {
                        drop_device = Some(d.id.clone());
                    }
                }
            }
            ui.add_space(8.0);
            (save, close) = footer(ui, "Save", true, false);
        });
        if save {
            let name = d.name.trim().to_owned();
            let receipts = (d.receipts != n.info.receipts).then_some(d.receipts);
            self.send(&network, NetCommand::SetDisplayName { name });
            if let Some(on) = receipts {
                self.send(&network, NetCommand::SetReceipts { on });
            }
        }
        if remove {
            self.send(&network, NetCommand::SetAvatar { url: String::new() });
        }
        if let Some(id) = drop_device {
            self.send(&network, NetCommand::RemoveDevice { id });
        }
        if choose {
            let (tx, ctx) = (self.files.0.clone(), ctx.clone());
            let (network, max) = (network.clone(), self.upload_limit(&network));
            tokio::spawn(async move {
                let pick = rfd::AsyncFileDialog::new()
                    .add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp"])
                    .pick_file()
                    .await;
                if let Some(f) = pick {
                    read_picked(tx, ctx, network, None, f.path().to_owned(), max);
                }
            });
        }
        if save || close || modal.should_close() {
            self.profile = None;
        }
    }
}

/// `emoji::picker` with the custom emoji of network `net`.
fn emoji_picker(
    ui: &mut egui::Ui,
    query: &mut String,
    nets: &mut [Network],
    net: Option<usize>,
) -> Option<String> {
    match net.and_then(|i| nets.get_mut(i)) {
        Some(n) => emoji::picker(ui, query, &n.emoji, &mut n.ui.media),
        None => emoji::picker(ui, query, &[], &mut crate::media::Media::default()),
    }
}
