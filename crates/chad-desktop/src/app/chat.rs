//! A buffer's chat pane: header, log and composer.
//!
//! The panels are free functions over the parts of the network they borrow,
//! so they can hold `&mut` buffer and media at once. What they want done
//! with the rest of `App` comes back in their `*Out` and `chat` applies it.

use super::verify::device_row;
use super::*;
use crate::platform::ptt::PttBind;

type Buffer = chad_core::view::Buf<BufUi>;

/// What every panel needs besides the buffer.
struct Pane<'a> {
    network: &'a str,
    own: &'a str,
    target: &'a str,
    connected: bool,
    /// The window has focus.
    focused: bool,
    send: &'a dyn Fn(NetCommand),
}

impl Pane<'_> {
    fn send(&self, cmd: NetCommand) {
        (self.send)(cmd)
    }
}

struct ComposerOpts {
    /// The focused pane, which takes typing that lands nowhere.
    active: bool,
    /// Put the cursor in the composer this frame.
    focus: bool,
    uploads: bool,
    gifs: bool,
    /// Keys that aren't typing: push-to-talk's, and any being bound.
    ptt_key: Option<PttBind>,
    binding_ptt: bool,
}

#[derive(Default)]
struct ComposerOut {
    /// Open the file dialog.
    attach: bool,
    /// Where the picker opens, and whether on the GIF tab (searching
    /// `gif_query`, from `/gif`).
    emoji_at: Option<(egui::Pos2, bool)>,
    gif_query: String,
    no_gifs: bool,
    pasted: Vec<Pasted>,
}

#[derive(Default)]
struct LogOut {
    dm: Option<String>,
    pick_reaction: Option<(String, egui::Pos2)>,
    refocus: bool,
}

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
        let core = self.core.clone();
        let send = |cmd| {
            core.send(Command::Net {
                network: network.clone(),
                cmd,
            })
        };
        let auto_media = self.settings.load_media;
        let guest_media = self.settings.guest_media;
        let ptt_key = self.ptt.as_ref().map(|p| p.bind);
        let binding_ptt = self.binding_ptt;
        let n = &mut self.view.nets[ni];
        let opts = ComposerOpts {
            active,
            focus: active && std::mem::take(&mut n.ui.focus),
            uploads: n.info.uploads.is_some(),
            gifs: n.info.gifs.is_some(),
            ptt_key,
            binding_ptt,
        };
        let connected = n.info.state == ConnState::Connected;
        let (b, profiles, media, devices) =
            (&mut n.buffers[bi], &n.profiles, &mut n.ui.media, &n.devices);
        let target = b.info.target.clone();
        let cx = Pane {
            network: &network,
            own: &own,
            target: &target,
            connected,
            focused: ui.input(|i| i.focused),
            send: &send,
        };

        if connected && b.info.kind == BufferKind::Channel && !b.ui.members_asked {
            b.ui.members_asked = true;
            cx.send(NetCommand::Members {
                channel: target.clone(),
            });
        }

        let members = (b.info.kind == BufferKind::Channel).then_some(!b.ui.hide_members);
        let (close, toggle) = egui::Panel::top("topic")
            .show(ui, |ui| {
                header(ui, &cx, b, profiles, devices, tile, closable, members)
            })
            .inner;
        b.ui.hide_members ^= toggle;
        let c = egui::Panel::bottom("composer")
            .show(ui, |ui| composer(ui, &cx, b, profiles, media, &opts))
            .inner;
        let extras = Extras {
            profiles,
            members: b.members.clone(),
            media,
            network: &network,
            auto_media,
            guest_media,
            dm: b.info.kind == BufferKind::Dm,
        };
        let l = egui::CentralPanel::default()
            .show(ui, |ui| messages(ui, &cx, b, extras))
            .inner;

        if let Some(nick) = l.dm {
            send(NetCommand::OpenDm { nick: nick.clone() });
            select(&mut self.view.nets[ni], &key(&nick));
        }
        if l.refocus {
            self.view.nets[ni].ui.focus = true;
        }
        if let Some((msgid, at)) = l.pick_reaction {
            self.picker = Some(Picker {
                react: Some((network.clone(), target.clone(), msgid)),
                at,
                query: String::new(),
                shown: false,
                gif_tab: false,
                gifs: Default::default(),
            });
        }
        if let Some((at, gif_tab)) = c.emoji_at {
            self.picker = Some(Picker {
                react: None,
                at,
                query: String::new(),
                shown: false,
                gif_tab,
                gifs: gifs::Gifs::searching(c.gif_query),
            });
        }
        if c.no_gifs {
            self.toast("This network doesn't have GIFs.".into());
        }
        if c.attach {
            let (tx, ctx) = (self.files.0.clone(), ui.ctx().clone());
            let (network, target) = (network.clone(), target.clone());
            let max = self.upload_limit(&network);
            tokio::spawn(async move {
                if let Some(f) = rfd::AsyncFileDialog::new().pick_file().await {
                    read_picked(tx, ctx, network, Some(target), f.path().to_owned(), max);
                }
            });
        }
        for p in c.pasted {
            match p {
                Pasted::File(path) => {
                    let ctx = ui.ctx().clone();
                    self.upload_file(&ctx, &network, Some(target.clone()), path);
                }
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

/// Title, DM status and encryption, topic. `members`: a channel's member
/// list is shown. Returns whether close, and the member list toggle, were clicked.
#[allow(clippy::too_many_arguments)]
fn header(
    ui: &mut egui::Ui,
    cx: &Pane,
    b: &Buffer,
    profiles: &HashMap<String, chatlog::Profile>,
    devices: &HashMap<String, Vec<DeviceInfo>>,
    tile: Option<usize>,
    closable: bool,
    members: Option<bool>,
) -> (bool, bool) {
    let target = cx.target;
    let dm = b.info.kind == BufferKind::Dm;
    let title = if dm {
        chatlog::shown(profiles, target)
    } else {
        target
    };
    // Display names aren't unique, so a DM's title names who it really is.
    let who = dm.then(|| {
        let account = b
            .msgs
            .iter()
            .rev()
            .filter(|m| m.nick.eq_ignore_ascii_case(target))
            .find_map(|m| m.account.as_deref());
        match account {
            Some(a) => format!("Account: {a}"),
            None => format!("Nick: {target}"),
        }
    });
    let (mut close, mut toggle) = (false, false);
    let buttons = usize::from(closable) + usize::from(members.is_some());
    let reserve = 28.0 * buttons as f32;
    if buttons > 0 {
        // A child, so it doesn't push the topic row down.
        let at = egui::Rect::from_min_size(
            ui.max_rect().right_top() - vec2(reserve, 0.0),
            vec2(reserve, 24.0),
        );
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(at)
                .layout(egui::Layout::right_to_left(egui::Align::Min)),
        );
        if closable {
            close = row.button(icons::X).tip("Close pane").clicked();
        }
        if let Some(show) = members {
            let tip = if show {
                "Hide member list"
            } else {
                "Show member list"
            };
            toggle = row.selectable_label(show, icons::USERS).tip(tip).clicked();
        }
    }
    ui.horizontal(|ui| {
        if buttons > 0 {
            ui.set_max_width(ui.available_width() - reserve);
            // Title, status and Verify don't shrink; cut them off before the buttons.
            let mut clip = ui.clip_rect();
            clip.max.x = clip.max.x.min(ui.max_rect().right());
            ui.set_clip_rect(clip);
        }
        match tile {
            Some(i) => {
                let id = egui::Id::unique(("pane-drag", cx.network, &b.key));
                let r = ui
                    .dnd_drag_source(id, PaneDrag(i), |ui| ui.heading(title))
                    .response;
                let drag = "Drag onto another pane to swap them";
                match &who {
                    Some(who) => r.on_hover_text(format!("{who}\n{drag}")),
                    None => r.on_hover_text(drag),
                };
            }
            None => {
                let r = ui.heading(title);
                if let Some(who) = &who {
                    r.on_hover_text(who);
                }
            }
        }
        if dm {
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
                Encryption::Verified => Some((icons::SHIELD_CHECK, "Verified", VERIFIED_TIP)),
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
            if cx.connected
                && matches!(b.info.encryption, Encryption::On | Encryption::Changed)
                && ui.small_button("Verify…").clicked()
            {
                cx.send(NetCommand::Verification {
                    nick: target.to_owned(),
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
                cx.send(NetCommand::AcceptKeys {
                    nick: target.to_owned(),
                });
            }
        });
    }
    (close, toggle)
}

/// Typing line, uploads, attachments, reply/edit bars and the message field.
fn composer(
    ui: &mut egui::Ui,
    cx: &Pane,
    b: &mut Buffer,
    profiles: &HashMap<String, chatlog::Profile>,
    media: &mut Media,
    opts: &ComposerOpts,
) -> ComposerOut {
    let target = cx.target;
    let mut out = ComposerOut::default();
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
    attachments(ui, b, media);
    let id = egui::Id::unique(("composer", cx.network, &b.key));
    // The `@` list was open last frame, so Tab, Enter and Escape are its.
    let open_id = id.with("completing");
    let completing = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
    // Typing or pasting while nothing has focus goes to the composer,
    // this frame, so the first character isn't lost.
    if opts.active
        && cx.focused
        && !opts.binding_ptt
        && ui.memory(|m| m.focused().is_none())
        && ui.input(|i| {
            !opts.ptt_key.is_some_and(|k| k.down(i))
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
        // Pasted images and files become uploads instead of text.
        ui.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::PasteImage(img) => {
                    out.pasted.push(Pasted::Image(img.clone()));
                    false
                }
                egui::Event::Paste(text) => match pasted_files(text) {
                    Some(paths) => {
                        out.pasted.extend(paths.into_iter().map(Pasted::File));
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
    let field = ui
        .horizontal(|ui| {
            if opts.uploads
                && ui
                    .add(
                        egui::Button::new(RichText::new(icons::PAPERCLIP).size(18.0))
                            .min_size(side),
                    )
                    .tip("Upload a file")
                    .clicked()
            {
                out.attach = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let e = ui
                    .add(egui::Button::new(RichText::new(icons::SMILEY).size(18.0)).min_size(side))
                    .tip("Emoji");
                // GIFs are a tab inside the picker.
                if e.clicked() {
                    out.emoji_at = Some((e.rect.left_top(), false));
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
                    let nicks =
                        complete::completable(members.as_deref(), msgs, info, cx.own, profiles);
                    let list = complete::suggest(&nicks, q.word).unwrap_or_default();
                    list.into_iter()
                        .map(|c| {
                            let label = match &c.display_name {
                                Some(d) => format!("{d}  ({})", c.nick),
                                None => c.nick.clone(),
                            };
                            egui::Suggestion::new(complete::insert(&c.nick, q.is_at_start()))
                                .content(label)
                        })
                        .collect()
                })
            })
            .inner
        })
        .inner;
    ui.data_mut(|d| d.insert_temp(open_id, field.is_open));
    let resp = field.text_edit.response.response;
    if opts.focus {
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
    if let Some(s) = &field.accepted {
        b.ui.pick(s.insert.trim_end().trim_end_matches(':'));
    }
    let mut changed = resp.changed() || field.accepted.is_some();
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
            let nicks =
                complete::completable(b.members.as_deref(), &b.msgs, &b.info, cx.own, profiles);
            if let Some(c) = complete::tab(&mut b.ui.draft, cursor, &mut b.ui.tab, &nicks, back) {
                (cursor, moved, changed) = (c, true, true);
                if let Some(nick) = b.ui.tab.as_ref().map(|t| t.nick().to_owned()) {
                    b.ui.pick(&nick);
                }
            }
        }
        if changed && chad_core::emoji::replace_shortcodes(&mut b.ui.draft, &mut cursor) {
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
            b.ui.picked.clear();
            Typing::Done
        } else {
            Typing::Active
        };
        cx.send(NetCommand::Typing {
            target: target.to_owned(),
            state,
        });
    }
    let enter = ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
    if resp.has_focus() && enter {
        submit(cx, b, media, opts.gifs, resp.rect.right_top(), &mut out);
    }
    ui.add_space(12.0);
    out
}

/// Files waiting in the composer, each with a remove button.
fn attachments(ui: &mut egui::Ui, b: &mut Buffer, media: &mut Media) {
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
}

/// Enter in the composer: send, finish an edit, or `/gif`, whose picker
/// opens at `gif_at`.
fn submit(
    cx: &Pane,
    b: &mut Buffer,
    media: &mut Media,
    gifs: bool,
    gif_at: egui::Pos2,
    out: &mut ComposerOut,
) {
    let target = cx.target;
    let text = b.ui.draft.trim_end().to_owned();
    if let Some(q) = gifs::command(&text) {
        // `/gif [search]` opens the GIF tab instead of sending.
        if gifs {
            out.emoji_at = Some((gif_at, true));
            q.clone_into(&mut out.gif_query);
            b.ui.draft.clear();
        } else {
            out.no_gifs = true;
        }
        return;
    }
    if text.trim().is_empty() && b.ui.attached.is_empty() {
        return;
    }
    let mentions = std::mem::take(&mut b.ui.picked);
    if let Some(msgid) = b.ui.editing.take() {
        if !text.trim().is_empty() {
            cx.send(NetCommand::Edit {
                target: target.to_owned(),
                msgid,
                text: format::markdown(&text),
                mentions,
            });
        }
    } else if !text.trim().is_empty() {
        let reply_to = b.ui.reply.take().map(|(id, _)| id);
        cx.send(NetCommand::Send {
            target: target.to_owned(),
            text: format::markdown(&text),
            reply_to,
            mentions,
        });
    }
    // Each attachment follows as its own message once it's up.
    for a in b.ui.attached.drain(..) {
        media.forget(&a.key);
        b.ui.uploads.push(a.name.clone());
        cx.send(NetCommand::Upload {
            target: Some(target.to_owned()),
            name: a.name,
            bytes: a.bytes,
        });
    }
    b.ui.draft.clear();
    b.ui.log.scroll_to_bottom();
}

/// The message log, its jump button, history paging and read markers.
fn messages(ui: &mut egui::Ui, cx: &Pane, b: &mut Buffer, mut extras: Extras) -> LogOut {
    let target = cx.target;
    if b.msgs.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new(format!("No messages in {target} yet. Say hi!")).weak());
        });
    }
    let channel = b.info.kind == BufferKind::Channel;
    let log = b.ui.log.ui(ui, &b.msgs, cx.own, channel, &mut extras);
    if !log.at_bottom {
        let pos = ui.max_rect().right_bottom() - vec2(16.0, 16.0);
        egui::Area::new(egui::Id::unique(("jump-latest", cx.network, &b.key)))
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
    if log.near_top
        && cx.connected
        && !b.history_start
        && !b.msgs.is_empty()
        && b.ui
            .older_asked
            .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
    {
        b.ui.older_asked = Some(Instant::now());
        cx.send(NetCommand::LoadOlder {
            target: target.to_owned(),
        });
    }
    if log.at_bottom && cx.focused && b.info.unread > 0 && !b.ui.mark_sent {
        b.ui.mark_sent = true;
        cx.send(NetCommand::MarkRead {
            target: target.to_owned(),
        });
    }
    let mut out = LogOut::default();
    for a in log.actions {
        match a {
            Action::Reply(i) => {
                if let Some(id) = b.msgs[i].msgid.clone() {
                    b.ui.reply = Some((id, b.msgs[i].nick.clone()));
                }
            }
            Action::React { msgid, emoji } => {
                cx.send(NetCommand::React {
                    target: target.to_owned(),
                    msgid,
                    emoji,
                });
            }
            Action::Unreact { msgid, emoji } => {
                cx.send(NetCommand::Unreact {
                    target: target.to_owned(),
                    msgid,
                    emoji,
                });
            }
            Action::Edit(i) => {
                b.ui.start_edit(&b.msgs[i]);
                out.refocus = true;
            }
            Action::Delete { msgid } => {
                cx.send(NetCommand::Redact {
                    target: target.to_owned(),
                    msgid,
                });
            }
            Action::Jump { msgid } => match b.index_of(&msgid) {
                Some(i) => b.ui.log.scroll_to_index(i, 60.0),
                None => {
                    b.ui.jump = Some(msgid.clone());
                    cx.send(NetCommand::LoadAround {
                        target: target.to_owned(),
                        msgid,
                    });
                }
            },
            Action::OpenDm(nick) => out.dm = Some(nick),
            Action::Block(nick, account) => cx.send(NetCommand::Block { nick, account }),
            Action::PickReaction { msgid, at } => out.pick_reaction = Some((msgid, at)),
        }
    }
    out
}
