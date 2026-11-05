//! Core events and the per-frame poll of channels and tasks.

use super::*;
use chad_core::NetworkInfo;
use chad_core::view::Applied;

impl App {
    pub(super) fn handle(&mut self, ev: Event, ctx: &egui::Context) {
        // A removed buffer's key, and whether an icon changed, before the view forgets.
        let removed = match &ev {
            Event::BufferRemoved { network, target } => Some((network.clone(), key(target))),
            _ => None,
        };
        let icon_changed = match &ev {
            Event::Network(info) => self
                .net_idx(&info.network)
                .is_some_and(|i| self.view.nets[i].info.icon != info.icon),
            _ => false,
        };
        let unread_was = match &ev {
            Event::Buffer { network, buffer } => self
                .view
                .buf(network, &buffer.target)
                .map(|b| b.info.unread),
            _ => None,
        };
        let applied = self.view.apply(&ev);
        match ev {
            Event::Network(info) => self.network_changed(&info, &applied, icon_changed),
            Event::NetworkRemoved { network } => {
                if self.current.as_deref() == Some(&network) {
                    self.current = self.view.nets.first().map(|n| n.info.network.clone());
                }
            }
            Event::JoinFailed { invite, error } => match &mut self.join {
                Some(j) if j.busy.as_deref() == Some(&invite) => {
                    j.busy = None;
                    j.error = Some(error);
                }
                _ => self.toast(error),
            },
            Event::SignUpFailed { network, error } => match &mut self.signup {
                Some(s) if s.network == network => {
                    s.busy = false;
                    s.error = Some(error);
                }
                _ => self.toast(error),
            },
            Event::CapsChanged {
                network,
                added,
                removed,
            } => {
                tracing::info!("{network}: caps +{added:?} -{removed:?}");
            }
            Event::Buffer { network, buffer } => {
                let Some(ni) = self.net_idx(&network) else {
                    return;
                };
                let n = &mut self.view.nets[ni];
                let k = key(&buffer.target);
                if applied.new {
                    if n.ui.selected.is_none() || n.ui.selected.as_deref() == Some(&k) {
                        select(n, &k);
                    }
                } else if let Some(bi) = n.buffer(&k)
                    && unread_was != Some(buffer.unread)
                {
                    // Unread changed since we said it was read: say so again.
                    n.buffers[bi].ui.mark_sent = false;
                }
            }
            Event::BufferRemoved { .. } => {
                let Some((network, k)) = removed else { return };
                let Some(ni) = self.net_idx(&network) else {
                    return;
                };
                let n = &mut self.view.nets[ni];
                if n.ui.selected.as_deref() == Some(&k) {
                    n.ui.selected = None;
                    if let Some(first) = n.buffers.first().map(|b| b.key.clone()) {
                        select(n, &first);
                    }
                }
            }
            Event::BufferRenamed { network, from, to } => {
                let Some(ni) = self.net_idx(&network) else {
                    return;
                };
                let n = &mut self.view.nets[ni];
                let (old, new) = (key(&from), key(&to));
                if n.buffer(&new).is_none() {
                    return;
                }
                if n.ui.selected.as_deref() == Some(&old) {
                    n.ui.selected = Some(new.clone());
                }
                for p in &mut self.panes {
                    if p.0 == network && p.1 == old {
                        p.1.clone_from(&new);
                    }
                }
            }
            Event::Messages {
                network,
                target,
                messages,
            } => self.messages_arrived(ctx, &network, &target, &messages, &applied),
            Event::Error { network, error } => {
                let name = self.net_name(&network);
                self.toast(format!("{name}: {error}"));
            }
            Event::Uploaded {
                network,
                target,
                result,
            } => {
                match (&target, result) {
                    (Some(t), Ok(url)) => self.send(
                        &network,
                        NetCommand::Send {
                            target: t.clone(),
                            text: url.to_string(),
                            reply_to: None,
                        },
                    ),
                    (None, Ok(url)) => self.send(
                        &network,
                        NetCommand::SetAvatar {
                            url: url.to_string(),
                        },
                    ),
                    (_, Err(e)) => self.toast(format!("Upload failed: {e}")),
                }
                if let Some(b) = target.and_then(|t| self.view.buf_mut(&network, &t))
                    && !b.ui.uploads.is_empty()
                {
                    b.ui.uploads.remove(0);
                }
            }
            Event::Verification {
                network,
                nick,
                state,
            } => {
                // A request from someone else waits behind the one on screen.
                let busy = self.verify.as_ref().is_some_and(|(n, k, s)| {
                    (*n != network || key(k) != key(&nick))
                        && !matches!(s, VerifyState::Done | VerifyState::Failed(_))
                });
                if !busy {
                    self.verify = Some((network, nick, state));
                } else if state == VerifyState::Incoming {
                    self.send(
                        &network,
                        NetCommand::Verification {
                            nick,
                            step: VerifyStep::Cancel,
                        },
                    );
                }
            }
            Event::Preview {
                network,
                url,
                preview,
            } => {
                if let Some(i) = self.net_idx(&network) {
                    self.view.nets[i].ui.media.preview(url, preview);
                }
            }
            Event::Media {
                network,
                url,
                result,
            } => {
                if let Some(i) = self.net_idx(&network) {
                    self.view.nets[i].ui.media.bytes(url, result, ctx);
                }
            }
            Event::Sounds { result: Err(e), .. } => tracing::info!("soundboard: {e}"),
            Event::Sound {
                network,
                file,
                result,
            } => match result {
                Ok(bytes) => {
                    let (tx, ctx) = (self.decoded.0.clone(), ctx.clone());
                    tokio::task::spawn_blocking(move || {
                        // A hostile file that panics the decoder only loses the sound (D14).
                        let clip = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            chad_media::load_sound(bytes.0, &file)
                        }))
                        .unwrap_or_else(|_| Err("It's damaged.".into()));
                        let _ = tx.send((network, file, clip));
                        ctx.request_repaint();
                    });
                }
                Err(e) => self.toast(e),
            },
            Event::Gifs {
                network,
                query,
                pos,
                result,
            } => {
                let here = self.on_screen().is_some_and(|(n, _)| n == network);
                if let Some(p) = &mut self.picker
                    && here
                {
                    p.gifs.page(&query, pos.as_deref(), result);
                }
            }
            // Held in the view, nothing more to do.
            // The view dropped their messages: lay the logs out again.
            Event::Blocks { network, .. } => {
                if let Some(ni) = self.net_idx(&network) {
                    for b in &mut self.view.nets[ni].buffers {
                        b.ui.log = Default::default();
                    }
                }
            }
            Event::HistoryStart { .. }
            | Event::Members { .. }
            | Event::Typing { .. }
            | Event::Voice { .. }
            | Event::VoiceMembers { .. }
            | Event::Profile { .. }
            | Event::Devices { .. }
            | Event::Sounds { .. }
            | Event::PushAvailable { .. } => {}
        }
    }

    /// A network connected, changed name or icon, or finished signing up.
    fn network_changed(&mut self, info: &NetworkInfo, applied: &Applied, icon_changed: bool) {
        let origin = info.network.clone();
        let i = self.net_idx(&origin).unwrap_or_default();
        if applied.reconnected {
            let n = &mut self.view.nets[i];
            for b in &mut n.buffers {
                b.ui.members_asked = false;
            }
            n.ui.media.retry_failed();
        }
        if applied.new && self.away_sent {
            let message = Some(AWAY.to_owned());
            self.send(&origin, NetCommand::SetAway { message });
        }
        if icon_changed {
            self.view.nets[i].ui.icon = Icon::None;
        }
        // Signed up, or waiting on the email's code: the dialog's done.
        if let Some(s) = &self.signup
            && s.busy
            && s.network == origin
            && (!info.guest || matches!(info.state, ConnState::NeedsVerification { .. }))
        {
            self.signup = None;
        }
        let waiting = matches!(
            info.state,
            ConnState::Connected | ConnState::NeedsVerification { .. }
        );
        if waiting
            && let Some(j) = &self.join
            && let Some(inv) = j.busy.as_deref().and_then(|b| invite::parse(b).ok())
            && inv.origin == origin
        {
            self.join = None;
            self.current = Some(origin.clone());
            // Open the invite's channel, not whichever autojoin lands first.
            if let Some(ch) = inv.channel {
                self.view.nets[i].ui.selected = Some(key(&ch));
            }
        }
        if self.current.is_none() {
            self.current = Some(origin);
        }
    }

    /// New or changed messages: follow them in the log, and notify.
    fn messages_arrived(
        &mut self,
        ctx: &egui::Context,
        network: &str,
        target: &str,
        messages: &[ChatMessage],
        applied: &Applied,
    ) {
        // On screen counts as seen, even when focus is elsewhere.
        let seen = ctx.input(|i| {
            let v = i.viewport();
            i.focused || (v.minimized != Some(true) && v.occluded != Some(true))
        });
        let k = key(target);
        let viewing = seen && self.panes.iter().any(|p| p.0 == network && p.1 == k);
        let Some(ni) = self.net_idx(network) else {
            return;
        };
        let n = &mut self.view.nets[ni];
        let Some(bi) = n.buffer(&k) else { return };
        let b = &mut n.buffers[bi];
        b.ui.older_asked = None;
        b.ui.log.apply(&applied.edits, messages);
        for &i in &applied.fresh {
            let m = &messages[i];
            if !viewing && should_notify(self.settings.notify, m) {
                let who = chatlog::shown(&n.profiles, &m.nick);
                let summary = match b.info.kind {
                    BufferKind::Dm => who.to_owned(),
                    BufferKind::Channel => format!("{who} in {}", b.info.target),
                };
                platform::notify(summary, chatlog::snippet(&format::strip(&m.text), 200));
            }
        }
        if let Some(id) = &b.ui.jump
            && let Some(i) = b.index_of(id)
        {
            b.ui.log.scroll_to_index(i, 60.0);
            b.ui.jump = None;
        }
    }

    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.events.try_recv() {
            self.handle(ev, ctx);
        }
        while let Ok(msg) = self.os.try_recv() {
            match msg {
                Os::Open(url) => self.open_invite(&url),
                Os::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Os::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        // No global shortcut: push-to-talk follows the key while we're focused.
        if let Some(p) = self.ptt.as_ref().filter(|p| p.focused_only()) {
            let bind = p.bind;
            self.voice
                .set_ptt(ctx.input(|i| i.focused && bind.down(i)) && !self.binding_ptt);
        }
        for n in &mut self.view.nets {
            n.ui.media.poll(ctx);
            for cmd in n.ui.media.wanted.drain(..) {
                self.core.send(Command::Net {
                    network: n.info.network.clone(),
                    cmd,
                });
            }
        }
        while let Ok((network, file, clip)) = self.decoded.1.try_recv() {
            match clip {
                Ok(clip) => {
                    self.clips.insert((network, file), clip.clone());
                    if let Err(e) = self.voice.play_sound(clip) {
                        self.toast(e.into());
                    }
                }
                Err(e) => self.toast(format!("Couldn't play {file}: {e}")),
            }
        }
        while let Ok((network, target, name, bytes)) = self.files.1.try_recv() {
            match bytes {
                Ok(bytes) => self.upload(ctx, &network, target, name, bytes),
                Err(e) => self.toast(format!("{name}: {e}")),
            }
        }
        if let Ok((version, url)) = self.update.try_recv() {
            self.new_version = Some((version, url));
        }
        let input = ctx.input(|i| {
            i.raw.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::PointerMoved(_)
                        | egui::Event::MouseWheel { .. }
                )
            })
        });
        let idle = self.idle.idle_for(input);
        let after = Duration::from_secs(60 * u64::from(self.settings.away_after));
        let away = self.settings.away || (self.settings.away_after > 0 && idle >= after);
        if away != self.away_sent {
            self.away_sent = away;
            for n in &self.view.nets {
                let message = away.then(|| AWAY.to_owned());
                self.send(&n.info.network, NetCommand::SetAway { message });
            }
        }
        if !idle.is_zero() && !away {
            ctx.request_repaint_after(Duration::from_secs(30));
        }
        while let Ok((origin, img)) = self.icons.1.try_recv() {
            if let Some(i) = self.net_idx(&origin) {
                self.view.nets[i].ui.icon = match img {
                    Some(img) => Icon::Ready(ctx.load_texture(
                        format!("icon:{origin}"),
                        img,
                        Default::default(),
                    )),
                    None => Icon::Failed,
                };
            }
        }
        for n in &mut self.view.nets {
            if let (Icon::None, Some(url)) = (&n.ui.icon, &n.info.icon) {
                n.ui.icon = Icon::Loading;
                fetch_icon(
                    n.info.network.clone(),
                    url.clone(),
                    self.icons.0.clone(),
                    ctx,
                );
            }
        }
        let now = (
            self.view.nets.iter().any(Network::unread),
            self.view.nets.iter().map(Network::mentions).sum(),
        );
        if now != self.last_badge {
            self.last_badge = now;
            self.tray.set(now.0, now.1);
            let title = match now.1 {
                0 => "IrChad".into(),
                n => format!("({}) IrChad", badge(n)),
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }
}
