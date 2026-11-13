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
            Event::ChannelList { network, channels } => {
                if let Some(b) = self.browse.as_mut().filter(|b| b.network == network) {
                    b.channels = Some(channels);
                }
            }
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
                            mentions: Vec::new(),
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
            Event::Games { result, .. } => match result {
                Ok(Some(list)) => {
                    tracing::info!(games = list.games.len(), "game list loaded");
                    let path = self.games_cache.clone();
                    let json = serde_json::to_vec(&list);
                    std::thread::spawn(move || {
                        if let Err(e) = json.map_err(std::io::Error::from).and_then(|j| {
                            // Through a temp file, so a crash never leaves half a list.
                            let tmp = path.with_extension("json.tmp");
                            std::fs::write(&tmp, j)?;
                            std::fs::rename(tmp, path)
                        }) {
                            tracing::warn!("caching the game list: {e}");
                        }
                    });
                    self.set_games(list);
                    self.games_fresh = true;
                }
                Ok(None) => {
                    tracing::info!("game list unchanged");
                    self.games_fresh = true;
                    self.games_error = None;
                }
                Err(e) => {
                    tracing::warn!("game list: {e}");
                    self.games_error = Some(e.to_string());
                }
            },
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
            | Event::ReportTo { .. }
            | Event::Reported { .. }
            | Event::Members { .. }
            | Event::Typing { .. }
            | Event::Voice { .. }
            | Event::VoiceMembers { .. }
            | Event::Profile { .. }
            | Event::Devices { .. }
            | Event::Sounds { .. }
            | Event::Emoji { .. }
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
        if applied.new && self.activity_sent.0.is_some() {
            let text = self.activity_sent.0.clone();
            let music = self.music_sent.clone();
            self.send(&origin, NetCommand::SetActivity { text, music });
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
        let channel = (b.info.kind == BufferKind::Channel).then(|| b.info.target.clone());
        for &i in &applied.fresh {
            let m = &messages[i];
            if !viewing && should_notify(self.settings.notify, m) {
                let who = chatlog::shown(&n.profiles, &m.nick);
                let summary = match b.info.kind {
                    BufferKind::Dm | BufferKind::Server => who.to_owned(),
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
        if let Some(channel) = channel {
            for &i in &applied.fresh {
                self.overlay_message(network, &channel, &messages[i]);
            }
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
        // No global shortcut: push-to-talk and whisper follow their keys
        // while we're focused.
        if let Some(p) = self.ptt.as_ref().filter(|p| p.focused_only()) {
            for &(action, combo) in &p.binds {
                let down = ctx.input(|i| i.focused && combo.down(i));
                p.sink.set(action, down && self.binding_ptt.is_none());
            }
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
        if let Ok(status) = self.update.try_recv() {
            self.new_version = Some(status);
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
        self.activity(ctx);
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
        }
        let mut title = match now.1 {
            0 => "IrChad".to_owned(),
            n => format!("({}) IrChad", badge(n)),
        };
        if let Some((net, key)) = self.panes.get(self.pane)
            && let Some(b) = self.view.buf(net, key)
        {
            let target = &b.info.target;
            if b.info.kind == BufferKind::Dm {
                let n = &self.view.nets[self.net_idx(net).unwrap_or_default()];
                let shown = n.shown(target);
                // No display name: the account, else the nick, as the DM header does.
                let shown = if shown != target {
                    shown
                } else {
                    b.msgs
                        .iter()
                        .rev()
                        .filter(|m| m.nick.eq_ignore_ascii_case(target))
                        .find_map(|m| m.account.as_deref())
                        .unwrap_or(target)
                };
                title += &format!(" | @{shown}");
            } else {
                title += &format!(" | {target}");
            }
            title += &format!(" | {}", self.net_name(net));
        }
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    /// Starts or stops the Rich Presence listener and the poller with the
    /// settings, and sends the best line to every network, at most every
    /// `ACTIVITY_EVERY`.
    fn set_games(&mut self, list: chad_core::GameList) {
        let games = crate::presence::Games::new(list.games);
        self.games.send_replace(Some(std::sync::Arc::new(games)));
        self.games_etag = list.etag;
        self.games_error = None;
    }

    fn activity(&mut self, ctx: &egui::Context) {
        const ACTIVITY_EVERY: Duration = Duration::from_secs(15);
        // chadd has no list for a while after it starts, or when Discord fails it.
        const GAMES_RETRY: Duration = Duration::from_secs(5 * 60);
        if self.settings.share_activity != self.rpc.is_some() {
            self.rpc = self.settings.share_activity.then(|| {
                let ctx = ctx.clone();
                platform::rpc::Rpc::start(move || ctx.request_repaint())
            });
        }
        let looks_for = (self.settings.share_activity, self.settings.share_music);
        let players = &self.settings.music_players;
        let want = Some((looks_for, players)).filter(|(l, _)| l.0 || l.1);
        if self.poll.as_ref().map(|p| (p.looks_for, &p.players)) != want {
            self.poll = (looks_for.0 || looks_for.1).then(|| {
                let ctx = ctx.clone();
                platform::presence::Poll::start(
                    self.games.subscribe(),
                    looks_for,
                    players.clone(),
                    move || ctx.request_repaint(),
                )
            });
        }
        let current = self.rpc.as_ref().and_then(|r| r.rx.borrow().clone());
        if self.games_load.as_ref().is_some_and(|h| h.is_finished()) {
            let cached = self.games_load.take().and_then(|h| h.join().ok().flatten());
            if let Some(list) = cached.filter(|_| self.games.borrow().is_none()) {
                tracing::info!(games = list.games.len(), "cached game list loaded");
                self.set_games(list);
            }
        }
        let games_due = self.games_asked.is_none_or(|t| t.elapsed() >= GAMES_RETRY);
        let wants_games = self.rpc.is_some() || self.focus.is_some();
        if wants_games && !self.games_fresh && self.games_load.is_none() && games_due {
            let net =
                self.view.nets.iter().find(|n| {
                    n.info.games && matches!(n.info.state, chad_core::ConnState::Connected)
                });
            if let Some(n) = net {
                self.games_asked = Some(Instant::now());
                let network = n.info.network.clone();
                let etag = self.games_etag.clone();
                self.send(&network, NetCommand::Games { etag });
            }
        }
        // Sharing off: `None`, so this client leaves `chad/activity` to other devices.
        let shared = self.poll.as_ref().map(|p| {
            let seen = p.rx.borrow();
            let games = self.games.borrow();
            crate::presence::line(
                current.map(|a| a.text(games.as_deref())),
                seen.game.as_deref(),
                seen.music.as_ref(),
                self.settings.music_player_name,
            )
        });
        let (text, music) = shared.map_or((None, None), |(t, m)| (Some(t), m));
        if text == self.activity_sent.0 && music == self.music_sent {
            return;
        }
        let wait = self.activity_sent.1.map_or(Duration::ZERO, |t| {
            ACTIVITY_EVERY.saturating_sub(t.elapsed())
        });
        if !wait.is_zero() && text.as_ref().is_some_and(|t| !t.is_empty()) {
            ctx.request_repaint_after(wait);
            return;
        }
        for n in &self.view.nets {
            let (text, music) = (text.clone(), music.clone());
            self.send(&n.info.network, NetCommand::SetActivity { text, music });
        }
        self.activity_sent = (text, Some(Instant::now()));
        self.music_sent = music;
    }
}
