//! A buffer's chat pane: header, log and composer.

use super::verify::device_row;
use super::*;

impl App {
    /// One buffer's pane. `active`: the focused pane, which takes the
    /// composer focus. `tile`: its cell when it's one of several, which makes
    /// its title a drag handle. `closable`: it isn't the only chat pane.
    /// Returns whether its close button was clicked.
    pub(super) fn chat(
        &mut self,
        ui: &mut egui::Ui,
        ni: usize,
        bi: usize,
        active: bool,
        tile: Option<usize>,
        closable: bool,
    ) -> bool {
        let network = self.view.nets[ni].info.network.clone();
        let own = self.view.nets[ni].info.nick.clone();
        let connected = self.view.nets[ni].info.state == ConnState::Connected;
        let focused = ui.input(|i| i.focused);
        let core = self.core.clone();
        let send = |cmd| {
            core.send(Command::Net {
                network: network.clone(),
                cmd,
            })
        };
        let uploads = self.view.nets[ni].info.uploads.is_some();
        let gifs = self.view.nets[ni].info.gifs.is_some();
        let auto_media = self.settings.load_media;
        let guest_media = self.settings.guest_media;
        // Keys that aren't typing: the one being bound, and push-to-talk's.
        let ptt_key = self.ptt.as_ref().map(|p| p.bind);
        let binding_ptt = self.binding_ptt;
        let n = &mut self.view.nets[ni];
        let (b, profiles, media, devices) =
            (&mut n.buffers[bi], &n.profiles, &mut n.ui.media, &n.devices);
        let focus = active && std::mem::take(&mut n.ui.focus);
        let target = b.info.target.clone();
        let mut close = false;
        // Set inside the panels, acted on once `b` is free again.
        let mut attach = false;
        // Where the picker opens, and whether on the GIF tab (searching
        // `gif_query`, from `/gif`).
        let mut emoji_at = None;
        let (mut gif_query, mut no_gifs) = (String::new(), false);
        let mut pick_reaction = None;
        let mut refocus = false;
        let mut pasted = Vec::new();

        if connected && b.info.kind == BufferKind::Channel && !b.ui.members_asked {
            b.ui.members_asked = true;
            send(NetCommand::Members {
                channel: target.clone(),
            });
        }

        egui::Panel::top("topic").show(ui, |ui| {
            if closable {
                // A child, so it doesn't push the topic row down.
                let at = egui::Rect::from_min_size(
                    ui.max_rect().right_top() - vec2(24.0, 0.0),
                    vec2(24.0, 24.0),
                );
                close = ui
                    .new_child(egui::UiBuilder::new().max_rect(at))
                    .button(icons::X)
                    .tip("Close pane")
                    .clicked();
            }
            ui.horizontal(|ui| {
                if closable {
                    ui.set_max_width(ui.available_width() - 28.0);
                }
                match tile {
                    Some(i) => {
                        let id = egui::Id::unique(("pane-drag", &network, &b.key));
                        ui.dnd_drag_source(id, PaneDrag(i), |ui| ui.heading(&target))
                            .response
                            .on_hover_text("Drag onto another pane to swap them");
                    }
                    None => {
                        ui.heading(&target);
                    }
                }
                if b.info.kind == BufferKind::Dm {
                    let status = match (b.info.online, b.info.away) {
                        (Some(true), false) => "online",
                        (Some(true), true) => "away",
                        (Some(false), _) => "offline",
                        (None, _) => "",
                    };
                    ui.label(RichText::new(status).weak());
                    let lock = match b.info.encryption {
                        Encryption::Unknown => None,
                        Encryption::Off => Some((icons::LOCK_OPEN, "Not encrypted", OFF_TIP)),
                        Encryption::On => Some((icons::LOCK, "Encrypted", ON_TIP)),
                        Encryption::Verified => {
                            Some((icons::SHIELD_CHECK, "Verified", VERIFIED_TIP))
                        }
                        Encryption::Changed => Some((icons::WARNING, "Keys changed", CHANGED_TIP)),
                    };
                    if let Some((icon, text, tip)) = lock {
                        ui.separator();
                        ui.label(RichText::new(format!("{icon} {text}")).weak())
                            .on_hover_ui(|ui| {
                                ui.label(tip);
                                for d in devices.get(&b.key).into_iter().flatten() {
                                    device_row(ui, d, false);
                                }
                            });
                    }
                    if connected
                        && matches!(b.info.encryption, Encryption::On | Encryption::Changed)
                        && ui.small_button("Verify…").clicked()
                    {
                        send(NetCommand::Verification {
                            nick: target.clone(),
                            step: VerifyStep::Start,
                        });
                    }
                }
                let topic = format::strip(&b.info.topic);
                if !topic.is_empty() {
                    ui.separator();
                    ui.add(egui::Label::new(RichText::new(&topic).weak()).truncate())
                        .on_hover_text(&topic);
                }
            });
            if b.info.encryption == Encryption::Changed {
                ui.horizontal_wrapped(|ui| {
                    let warn = format!(
                        "{} {target}'s encryption keys changed. That's normal after they \
                         reinstall, but it can also mean someone is listening in. Verify \
                         to be sure.",
                        icons::WARNING
                    );
                    ui.label(RichText::new(warn).color(ui.visuals().warn_fg_color));
                    if ui.button("Accept the new keys").clicked() {
                        send(NetCommand::AcceptKeys {
                            nick: target.clone(),
                        });
                    }
                });
            }
        });

        egui::Panel::bottom("composer").show(ui, |ui| {
            ui.add_space(4.0);
            let typing: Vec<_> = b.typing().collect();
            let typing_text = match typing.as_slice() {
                [] => String::new(),
                [a] => format!("{a} is typing…"),
                [a, c] => format!("{a} and {c} are typing…"),
                _ => "Several people are typing…".into(),
            };
            if !typing.is_empty() {
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            }
            ui.label(RichText::new(typing_text).small().weak());
            for name in &b.ui.uploads {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(format!("Uploading {name}…")).small());
                });
            }
            let mut unattach = None;
            ui.horizontal_wrapped(|ui| {
                for (i, a) in b.ui.attached.iter().enumerate() {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.vertical(|ui| {
                            match media::media_kind(&a.name) {
                                Some(media::Kind::Image) => {
                                    media.local_preview(ui, &a.key, vec2(160.0, 120.0))
                                }
                                Some(media::Kind::Video) => {
                                    ui.label(RichText::new(icons::PLAY).size(32.0));
                                }
                                _ => {
                                    ui.label(RichText::new(icons::FILE).size(32.0));
                                }
                            }
                            ui.horizontal(|ui| {
                                ui.add(egui::Label::new(RichText::new(&a.name).small()).truncate());
                                if ui
                                    .small_button(icons::X)
                                    .tip(&format!("Remove {}", a.name))
                                    .clicked()
                                {
                                    unattach = Some(i);
                                }
                            });
                        });
                    });
                }
            });
            if let Some(i) = unattach {
                media.forget(&b.ui.attached.remove(i).key);
            }
            let id = egui::Id::unique(("composer", &network, &b.key));
            // The `@` list was open last frame, so Tab, Enter and Escape are its.
            let open_id = id.with("completing");
            let completing = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
            // Typing or pasting while nothing has focus goes to the composer,
            // this frame, so the first character isn't lost.
            if active
                && focused
                && !binding_ptt
                && ui.memory(|m| m.focused().is_none())
                && ui.input(|i| {
                    !ptt_key.is_some_and(|k| k.down(i))
                        && i.events.iter().any(|e| match e {
                            egui::Event::Text(t) => !t.is_empty(),
                            egui::Event::Paste(_) | egui::Event::PasteImage(_) => true,
                            _ => false,
                        })
                })
            {
                ui.memory_mut(|m| m.request_focus(id));
            }
            if b.ui.editing.is_some() {
                let mut cancel = false;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Editing your message").small());
                    cancel = ui.small_button(icons::X).tip("Cancel edit").clicked();
                });
                if cancel || (!completing && ui.input(|i| i.key_pressed(Key::Escape))) {
                    b.ui.editing = None;
                    b.ui.draft.clear();
                }
            }
            if let Some((_, nick)) = &b.ui.reply {
                let mut cancel = false;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Replying to {nick}")).small());
                    cancel = ui.small_button(icons::X).tip("Cancel reply").clicked();
                });
                if cancel || (!completing && ui.input(|i| i.key_pressed(Key::Escape))) {
                    b.ui.reply = None;
                }
            }
            // Tab completes nicks rather than moving focus (Shift+Tab goes back).
            let mut tab = None;
            if ui.memory(|m| m.has_focus(id)) {
                ui.input_mut(|i| {
                    // Plain Tab is left to the open list, which accepts with it.
                    if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                        tab = (!completing).then_some(true);
                    } else if !completing && i.consume_key(Modifiers::NONE, Key::Tab) {
                        tab = Some(false);
                    }
                });
            }
            // Pasted images and files become uploads instead of text.
            if ui.memory(|m| m.has_focus(id)) {
                ui.input_mut(|i| {
                    i.events.retain(|e| match e {
                        egui::Event::PasteImage(img) => {
                            pasted.push(Pasted::Image(img.clone()));
                            false
                        }
                        egui::Event::Paste(text) => match pasted_files(text) {
                            Some(paths) => {
                                pasted.extend(paths.into_iter().map(Pasted::File));
                                false
                            }
                            None => true,
                        },
                        _ => true,
                    })
                });
            }
            // Square buttons as tall as the one-line field beside them.
            let side = vec2(1.0, 1.0) * (ui.text_style_height(&egui::TextStyle::Body) + 24.0);
            let out = ui
                .horizontal(|ui| {
                    if uploads
                        && ui
                            .add(
                                egui::Button::new(RichText::new(icons::PAPERCLIP).size(18.0))
                                    .min_size(side),
                            )
                            .tip("Upload a file")
                            .clicked()
                    {
                        attach = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let e = ui
                            .add(
                                egui::Button::new(RichText::new(icons::SMILEY).size(18.0))
                                    .min_size(side),
                            )
                            .tip("Emoji");
                        // GIFs are a tab inside the picker.
                        if e.clicked() {
                            emoji_at = Some((e.rect.left_top(), false));
                        }
                        let edit = egui::TextEdit::multiline(&mut b.ui.draft)
                            .hint_text(format!("Message {target}"))
                            .desired_rows(1)
                            .margin(egui::Margin::symmetric(14, 12))
                            .desired_width(f32::INFINITY)
                            .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                            .lock_focus(true);
                        let (members, msgs, info) = (&b.members, &b.msgs, &b.info);
                        egui::CompletionPopup::new(id).show(ui, edit, |q| {
                            if !q.word.starts_with('@') {
                                return Vec::new();
                            }
                            let nicks = completable(members, msgs, info, &own);
                            let list = complete::suggest(&nicks, q.word).unwrap_or_default();
                            list.into_iter()
                                .map(|n| {
                                    egui::Suggestion::new(complete::insert(n, q.is_at_start()))
                                        .content(n.to_owned())
                                })
                                .collect()
                        })
                    })
                    .inner
                })
                .inner;
            ui.data_mut(|d| d.insert_temp(open_id, out.is_open));
            let resp = out.text_edit.response.response;
            if focus {
                resp.request_focus();
            }
            // Screen readers get the hint as the field's name.
            ui.ctx()
                .accesskit_node_builder(resp.id, |n| n.set_label(format!("Message {target}")));
            // Up in an empty composer edits our last message.
            if resp.has_focus()
                && b.ui.draft.is_empty()
                && ui.input(|i| i.key_pressed(Key::ArrowUp))
                && let Some(m) = b.msgs.iter().rev().find(|m| {
                    m.own
                        && m.msgid.is_some()
                        && !m.redacted
                        && m.crypt != Crypt::Failed
                        && m.kind != MessageKind::Notice
                })
            {
                b.ui.start_edit(m);
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                    let end = egui::text::CCursor::new(b.ui.draft.chars().count());
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(end)));
                    state.store(ui.ctx(), id);
                }
            }
            let mut changed = resp.changed() || out.accepted.is_some();
            if (changed || tab.is_some())
                && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id)
            {
                let end = b.ui.draft.chars().count();
                let mut cursor = state
                    .cursor
                    .char_range()
                    .map_or(end, |r| r.primary.index.into());
                let mut moved = false;
                if let Some(back) = tab {
                    let nicks = completable(&b.members, &b.msgs, &b.info, &own);
                    if let Some(c) =
                        complete::tab(&mut b.ui.draft, cursor, &mut b.ui.tab, &nicks, back)
                    {
                        (cursor, moved, changed) = (c, true, true);
                    }
                }
                if changed && emoji::replace_shortcodes(&mut b.ui.draft, &mut cursor) {
                    moved = true;
                }
                if moved {
                    let c = egui::text::CCursor::new(cursor);
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(c)));
                    state.store(ui.ctx(), id);
                }
            }
            if changed {
                let state = if b.ui.draft.trim().is_empty() {
                    Typing::Done
                } else {
                    Typing::Active
                };
                send(NetCommand::Typing {
                    target: target.clone(),
                    state,
                });
            }
            let enter = ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
            if resp.has_focus() && enter {
                let text = b.ui.draft.trim_end().to_owned();
                if let Some(q) = gifs::command(&text) {
                    // `/gif [search]` opens the GIF tab instead of sending.
                    if gifs {
                        emoji_at = Some((resp.rect.right_top(), true));
                        gif_query = q.to_owned();
                        b.ui.draft.clear();
                    } else {
                        no_gifs = true;
                    }
                } else if !text.trim().is_empty() || !b.ui.attached.is_empty() {
                    if let Some(msgid) = b.ui.editing.take() {
                        if !text.trim().is_empty() {
                            send(NetCommand::Edit {
                                target: target.clone(),
                                msgid,
                                text: format::markdown(&text),
                            });
                        }
                    } else if !text.trim().is_empty() {
                        let reply_to = b.ui.reply.take().map(|(id, _)| id);
                        send(NetCommand::Send {
                            target: target.clone(),
                            text: format::markdown(&text),
                            reply_to,
                        });
                    }
                    // Each attachment follows as its own message once it's up.
                    for a in b.ui.attached.drain(..) {
                        media.forget(&a.key);
                        b.ui.uploads.push(a.name.clone());
                        send(NetCommand::Upload {
                            target: Some(target.clone()),
                            name: a.name,
                            bytes: a.bytes,
                        });
                    }
                    b.ui.draft.clear();
                    b.ui.log.scroll_to_bottom();
                }
            }
            ui.add_space(12.0);
        });

        let dm = egui::CentralPanel::default().show(ui, |ui| {
            if b.msgs.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new(format!("No messages in {target} yet. Say hi!")).weak());
                });
            }
            let mut extras = Extras {
                profiles,
                media,
                network: &network,
                auto_media,
                guest_media,
                dm: b.info.kind == BufferKind::Dm,
            };
            let channel = b.info.kind == BufferKind::Channel;
            let out = b.ui.log.ui(ui, &b.msgs, &own, channel, &mut extras);
            if !out.at_bottom {
                let pos = ui.max_rect().right_bottom() - vec2(16.0, 16.0);
                egui::Area::new(egui::Id::unique(("jump-latest", &network, &b.key)))
                    .fixed_pos(pos)
                    .pivot(Align2::RIGHT_BOTTOM)
                    .show(ui.ctx(), |ui| {
                        if ui
                            .button(format!("Jump to latest {}", icons::ARROW_DOWN))
                            .clicked()
                        {
                            b.ui.log.scroll_to_bottom();
                        }
                    });
            }
            if out.near_top
                && connected
                && !b.history_start
                && !b.msgs.is_empty()
                && b.ui
                    .older_asked
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
            {
                b.ui.older_asked = Some(Instant::now());
                send(NetCommand::LoadOlder {
                    target: target.clone(),
                });
            }
            if out.at_bottom && focused && b.info.unread > 0 && !b.ui.mark_sent {
                b.ui.mark_sent = true;
                send(NetCommand::MarkRead {
                    target: target.clone(),
                });
            }
            let mut dm = None;
            for a in out.actions {
                match a {
                    Action::Reply(i) => {
                        if let Some(id) = b.msgs[i].msgid.clone() {
                            b.ui.reply = Some((id, b.msgs[i].nick.clone()));
                        }
                    }
                    Action::React { msgid, emoji } => {
                        send(NetCommand::React {
                            target: target.clone(),
                            msgid,
                            emoji,
                        });
                    }
                    Action::Unreact { msgid, emoji } => {
                        send(NetCommand::Unreact {
                            target: target.clone(),
                            msgid,
                            emoji,
                        });
                    }
                    Action::Edit(i) => {
                        b.ui.start_edit(&b.msgs[i]);
                        refocus = true;
                    }
                    Action::Delete { msgid } => {
                        send(NetCommand::Redact {
                            target: target.clone(),
                            msgid,
                        });
                    }
                    Action::Jump { msgid } => match b.index_of(&msgid) {
                        Some(i) => b.ui.log.scroll_to_index(i, 60.0),
                        None => {
                            b.ui.jump = Some(msgid.clone());
                            send(NetCommand::LoadAround {
                                target: target.clone(),
                                msgid,
                            });
                        }
                    },
                    Action::OpenDm(nick) => dm = Some(nick),
                    Action::PickReaction { msgid, at } => pick_reaction = Some((msgid, at)),
                }
            }
            dm
        });
        if let Some(nick) = dm.inner {
            send(NetCommand::OpenDm { nick: nick.clone() });
            select(&mut self.view.nets[ni], &key(&nick));
        }
        if refocus {
            self.view.nets[ni].ui.focus = true;
        }
        if let Some((msgid, at)) = pick_reaction {
            self.picker = Some(Picker {
                react: Some((network.clone(), target.clone(), msgid)),
                at,
                query: String::new(),
                shown: false,
                gif_tab: false,
                gifs: Default::default(),
            });
        }
        if let Some((at, gif_tab)) = emoji_at {
            self.picker = Some(Picker {
                react: None,
                at,
                query: String::new(),
                shown: false,
                gif_tab,
                gifs: gifs::Gifs::searching(gif_query),
            });
        }
        if no_gifs {
            self.toast("This network doesn't have GIFs.".into());
        }
        if attach {
            let (tx, ctx) = (self.files.0.clone(), ui.ctx().clone());
            let (network, target) = (network.clone(), target.clone());
            tokio::spawn(async move {
                if let Some(f) = rfd::AsyncFileDialog::new().pick_file().await {
                    let _ = tx.send((network, Some(target), f.path().to_owned()));
                    ctx.request_repaint();
                }
            });
        }
        for p in pasted {
            match p {
                Pasted::File(path) => self.upload_file(&network, Some(target.clone()), path),
                Pasted::Image(img) => match png(&img) {
                    Some(bytes) => {
                        let ctx = ui.ctx().clone();
                        self.upload(
                            &ctx,
                            &network,
                            Some(target.clone()),
                            "pasted.png".into(),
                            bytes,
                        )
                    }
                    None => self.toast("Couldn't paste that image.".into()),
                },
            }
        }
        close
    }
}
