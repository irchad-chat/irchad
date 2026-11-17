//! The `Client` methods Kotlin calls.

use super::*;

impl Client {
    pub(super) fn view(&self) -> MutexGuard<'_, View> {
        chad_core::lock(&self.view)
    }

    pub(super) fn net(&self, network: String, cmd: NetCommand) {
        self.core.send(Command::Net { network, cmd });
    }
}

#[uniffi::export]
impl Client {
    #[uniffi::constructor]
    pub fn new(
        data_dir: String,
        version: String,
        secrets: Arc<dyn Secrets>,
        voice: Arc<dyn Voice>,
        listener: Arc<dyn Listener>,
    ) -> Arc<Self> {
        #[cfg(target_os = "android")]
        trust_android_roots(&data_dir);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");
        let _g = rt.enter();
        let (core, mut events) = chad_core::start(chad_core::Config {
            data_dir: PathBuf::from(data_dir),
            secrets: Arc::new(Store(secrets)),
            notify: Arc::new(|| {}),
            media: Some(Arc::new(Calls {
                voice,
                next: AtomicU64::new(1),
            })),
            version,
        });
        let view = Arc::new(Mutex::new(View::default()));
        let v = view.clone();
        let c = core.clone();
        let notify_all = Arc::new(AtomicBool::new(false));
        let all = notify_all.clone();
        let l = listener.clone();
        rt.spawn(async move {
            while let Some(ev) = events.recv().await {
                let applied = chad_core::lock(&v).apply(&ev);
                match ev {
                    Event::Messages {
                        network,
                        target,
                        messages,
                    } => {
                        let notes = notes(
                            &chad_core::lock(&v),
                            &network,
                            &target,
                            &messages,
                            &applied.fresh,
                            all.load(Ordering::Relaxed),
                        );
                        for (from, text, channel) in notes {
                            listener.notify(network.clone(), target.clone(), from, text, channel);
                        }
                    }
                    Event::PushAvailable { network, vapid } => {
                        listener.push_available(network, vapid)
                    }
                    Event::Error { error, .. } | Event::SignUpFailed { error, .. } => {
                        listener.notice(error)
                    }
                    Event::JoinFailed { invite, error } => listener.join_failed(invite, error),
                    Event::Discovered { invite, server } => listener.discovered(
                        invite,
                        Server {
                            origin: server.origin,
                            name: server.name,
                            sign_up: server.sign_up,
                            guests: server.guests,
                            discord: server.discord,
                            classic: server.classic,
                        },
                    ),
                    Event::Verification {
                        network,
                        nick,
                        state,
                    } => listener.verification(network, nick, state.into()),
                    Event::Preview {
                        network,
                        url,
                        preview,
                    } => listener.preview(
                        network,
                        url,
                        preview.map(|p| LinkPreview {
                            title: p.title,
                            description: p.description,
                            site: p.site,
                            image: p.image.map(String::from),
                        }),
                    ),
                    Event::Media {
                        network,
                        url,
                        result,
                    } => listener.media(network, url, result.ok().map(|b| b.0.to_vec())),
                    Event::Gifs {
                        network,
                        query,
                        pos,
                        result,
                    } => listener.gifs(
                        network,
                        query,
                        pos,
                        result.map_or_else(
                            |e| GifPage {
                                gifs: Vec::new(),
                                next: None,
                                error: Some(e),
                            },
                            |p| GifPage {
                                gifs: p
                                    .gifs
                                    .into_iter()
                                    .map(|g| Gif {
                                        title: g.title,
                                        preview: g.preview.into(),
                                        url: g.url.into(),
                                        width: g.width,
                                        height: g.height,
                                    })
                                    .collect(),
                                next: p.next,
                                error: None,
                            },
                        ),
                    ),
                    Event::Uploaded {
                        network,
                        target: None,
                        result: Ok(url),
                    } => c.send(Command::Net {
                        network,
                        cmd: NetCommand::SetAvatar { url: url.into() },
                    }),
                    Event::Uploaded { result: Err(e), .. } => {
                        listener.notice(format!("Upload failed: {e}"))
                    }
                    // The app puts the link in the draft; the user sends it.
                    Event::Uploaded {
                        network,
                        target: Some(target),
                        result: Ok(url),
                    } => listener.uploaded(network, target, url.into()),
                    Event::Sounds { network, result } => match result {
                        Ok(clips) => listener.sounds(
                            network,
                            clips
                                .into_iter()
                                .map(|c| Clip {
                                    name: c.name,
                                    file: c.file,
                                })
                                .collect(),
                            None,
                        ),
                        Err(e) => listener.sounds(network, Vec::new(), Some(e)),
                    },
                    Event::Sound {
                        network,
                        file,
                        result,
                    } => listener.sound(network, file, result.ok().map(|b| b.0.to_vec())),
                    Event::Reported { network } => listener.reported(network),
                    Event::OpenUrl { url } => listener.open_url(url),
                    Event::DiscordNeedsName {
                        invite,
                        suggestion,
                        error,
                    } => listener.discord_needs_name(invite, suggestion, error),
                    Event::PasswordSet { network, result } => {
                        listener.password_set(network, result.err())
                    }
                    _ => {}
                }
                listener.changed();
            }
        });
        drop(_g);
        Arc::new(Self {
            core,
            view,
            listener: l,
            notify_all,
            rt,
        })
    }

    /// The sign-in screen's first step, for an invite URL, `irchad://` link or
    /// bare host: `Listener::discovered` says what the server offers, or
    /// `Listener::join_failed` why it can't be joined.
    pub fn discover(&self, invite: String) {
        self.core.send(Command::Discover { invite });
    }

    /// An invite `discover` found a server for, with an account.
    pub fn join(&self, invite: String, account: String, password: String) {
        let auth = Auth::Login { account, password };
        self.core.send(Command::Join { invite, auth });
    }

    pub fn register(
        &self,
        invite: String,
        account: String,
        password: String,
        email: Option<String>,
    ) {
        let auth = Auth::Register {
            account,
            password,
            email: email.filter(|e| !e.trim().is_empty()),
        };
        self.core.send(Command::Join { invite, auth });
    }

    pub fn join_guest(&self, invite: String, nick: String) {
        let auth = Auth::Guest { nick };
        self.core.send(Command::Join { invite, auth });
    }

    /// "Sign in with Discord" (D96) for an invite, or for a `Network::discord`
    /// that failed: `Listener::open_url`, then `discord_callback` with the
    /// redirect, then maybe `Listener::discord_needs_name`. Failures come as
    /// `Listener::join_failed`.
    pub fn discord_start(&self, invite: String) {
        self.core.send(Command::DiscordStart { invite });
    }

    /// An `irchad://oauth?...` URL the OS (or the auth session) handed the app.
    pub fn discord_callback(&self, url: String) {
        self.core.send(Command::DiscordCallback { url });
    }

    /// The account name for a first Discord sign-in.
    pub fn discord_name(&self, name: String) {
        self.core.send(Command::DiscordName { name });
    }

    /// The user closed the browser or the name prompt.
    pub fn discord_cancel(&self) {
        self.core.send(Command::DiscordCancel);
    }

    /// `Listener::notify` for every message (true), or only DMs and mentions.
    pub fn notify_all(&self, on: bool) {
        self.notify_all.store(on, Ordering::Relaxed);
    }

    /// Fetch the network's icon (`Network.icon`), at most 1 MiB:
    /// `Listener::media` with that URL. Decoding is the UI's job (D14).
    pub fn icon(&self, network: String) {
        let url = {
            let v = self.view();
            v.net(&network).and_then(|i| v.nets[i].info.icon.clone())
        };
        let Some(url) = url else { return };
        let listener = self.listener.clone();
        self.rt.spawn(async move {
            let bytes = chad_core::fetch_icon(&url).await.ok();
            listener.media(network, url.to_string(), bytes);
        });
    }

    /// The `@` list in `target` for `query` (what follows the `@`): people
    /// whose nick or display name starts with it, then ones containing it,
    /// recent speakers first. Insert the nick, then pass it to `send` or
    /// `edit` in `mentions` (D66).
    pub fn nicks(&self, network: String, target: String, query: String) -> Vec<Nick> {
        let v = self.view();
        let (Some(ni), Some(b)) = (v.net(&network), v.buf(&network, &target)) else {
            return Vec::new();
        };
        let n = &v.nets[ni];
        let cands = chad_core::complete::completable(
            b.members.as_deref(),
            &b.msgs,
            &b.info,
            &n.info.nick,
            &n.profiles,
        );
        chad_core::complete::suggest(&cands, &format!("@{query}"))
            .unwrap_or_default()
            .into_iter()
            .map(|c| Nick {
                nick: c.nick.clone(),
                shown: c.shown().to_owned(),
            })
            .collect()
    }

    /// Forget a network: disconnect, delete its cache and password.
    pub fn remove(&self, network: String) {
        self.core.send(Command::Remove { network });
    }

    /// Unregister the account on the server, then forget the network.
    /// Failures come as `Listener::notice`, and the network stays.
    pub fn delete_account(&self, network: String) {
        self.net(network, NetCommand::DeleteAccount);
    }

    /// The app went to the background, or came back (D39): connections
    /// pause meanwhile (not one in voice) and catch up when back.
    pub fn background(&self, on: bool) {
        self.core.send(Command::Background { on });
    }

    /// Upload a file through chadd; its link comes as `Listener::uploaded`.
    /// Failures come as `Listener::notice`.
    pub fn upload(&self, network: String, target: String, name: String, bytes: Vec<u8>) {
        let bytes = chad_core::Blob(bytes.into());
        self.net(
            network,
            NetCommand::Upload {
                target: Some(target),
                name,
                bytes,
            },
        );
    }

    /// Asks for the network's custom emoji again, as when a picker opens;
    /// `changed()` follows if the set did.
    pub fn refresh_emoji(&self, network: String) {
        self.net(network, NetCommand::Emoji);
    }

    /// The network's soundboard: `Listener::sounds`.
    pub fn sounds(&self, network: String) {
        self.net(network, NetCommand::Sounds);
    }

    /// One clip's bytes: `Listener::sound`.
    pub fn sound(&self, network: String, file: String) {
        self.net(network, NetCommand::Sound { file });
    }

    /// `mentions`: the nicks picked from `nicks` (D66); the core keeps the
    /// ones still in the text.
    pub fn send(
        &self,
        network: String,
        target: String,
        text: String,
        reply_to: Option<String>,
        mentions: Vec<String>,
    ) {
        self.net(
            network,
            NetCommand::Send {
                target,
                text: format::markdown(&text),
                reply_to,
                mentions,
            },
        );
    }

    /// Replace the text of one of your own messages (D51). `mentions` as in
    /// `send`, starting from the message's own.
    pub fn edit(
        &self,
        network: String,
        target: String,
        msgid: String,
        text: String,
        mentions: Vec<String>,
    ) {
        let text = format::markdown(&text);
        self.net(
            network,
            NetCommand::Edit {
                target,
                msgid,
                text,
                mentions,
            },
        );
    }

    /// The composer changed: `active` while it holds text, false once it's
    /// empty (or sent). The core throttles what goes out.
    pub fn typing(&self, network: String, target: String, active: bool) {
        let state = if active {
            chad_core::Typing::Active
        } else {
            chad_core::Typing::Done
        };
        self.net(network, NetCommand::Typing { target, state });
    }

    /// Load the history around `msgid` (a reply's parent that isn't loaded).
    pub fn load_around(&self, network: String, target: String, msgid: String) {
        self.net(network, NetCommand::LoadAround { target, msgid });
    }

    /// Empty clears it.
    pub fn set_display_name(&self, network: String, name: String) {
        self.net(network, NetCommand::SetDisplayName { name });
    }

    /// Read receipts in DMs for the whole account (D68); `Network.receipts` follows.
    pub fn set_receipts(&self, network: String, on: bool) {
        self.net(network, NetCommand::SetReceipts { on });
    }

    /// Upload a picture through chadd and make it the avatar.
    pub fn upload_avatar(&self, network: String, name: String, bytes: Vec<u8>) {
        let bytes = chad_core::Blob(bytes.into());
        let target = None;
        self.net(
            network,
            NetCommand::Upload {
                target,
                name,
                bytes,
            },
        );
    }

    pub fn clear_avatar(&self, network: String) {
        self.net(network, NetCommand::SetAvatar { url: String::new() });
    }

    /// `Some(reason)` to go away, `None` to come back.
    pub fn set_away(&self, network: String, message: Option<String>) {
        self.net(network, NetCommand::SetAway { message });
    }

    /// Fetch `nick`'s encrypted-DM devices again (ours for our own nick);
    /// read them with `devices`.
    pub fn fetch_devices(&self, network: String, nick: String) {
        self.net(network, NetCommand::Devices { nick });
    }

    /// Take one of our stale devices off the server (D52).
    pub fn remove_device(&self, network: String, id: String) {
        self.net(network, NetCommand::RemoveDevice { id });
    }

    pub fn react(&self, network: String, target: String, msgid: String, emoji: String) {
        self.net(
            network,
            NetCommand::React {
                target,
                msgid,
                emoji,
            },
        );
    }

    pub fn unreact(&self, network: String, target: String, msgid: String, emoji: String) {
        self.net(
            network,
            NetCommand::Unreact {
                target,
                msgid,
                emoji,
            },
        );
    }

    pub fn redact(&self, network: String, target: String, msgid: String) {
        self.net(network, NetCommand::Redact { target, msgid });
    }

    pub fn mark_read(&self, network: String, target: String) {
        self.net(network, NetCommand::MarkRead { target });
    }

    pub fn load_older(&self, network: String, target: String) {
        self.net(network, NetCommand::LoadOlder { target });
    }

    /// Fetch a channel's members; call when it's shown, and after a reconnect.
    pub fn members(&self, network: String, channel: String) {
        self.net(network, NetCommand::Members { channel });
    }

    pub fn join_channel(&self, network: String, channel: String) {
        self.net(network, NetCommand::JoinChannel { channel });
    }

    pub fn part(&self, network: String, channel: String) {
        self.net(network, NetCommand::Part { channel });
    }

    pub fn open_dm(&self, network: String, nick: String) {
        self.net(network, NetCommand::OpenDm { nick });
    }

    pub fn close_dm(&self, network: String, nick: String) {
        self.net(network, NetCommand::CloseDm { nick });
    }

    /// Hide everything from `nick` on this device: by `account` when known
    /// (a message's or member's), else the core finds it or their host.
    pub fn block(&self, network: String, nick: String, account: Option<String>) {
        self.net(network, NetCommand::Block { nick, account });
    }

    /// `mask` from `blocks`.
    pub fn unblock(&self, network: String, mask: String) {
        self.net(network, NetCommand::Unblock { mask });
    }

    /// Who's blocked on `network`, oldest first.
    pub fn blocks(&self, network: String) -> Vec<Blocked> {
        let v = self.view();
        v.net(&network)
            .map(|i| {
                v.nets[i]
                    .blocks
                    .iter()
                    .map(|b| Blocked {
                        mask: b.mask.clone(),
                        nick: b.nick.clone(),
                        account: b.mask.strip_prefix("$a:").map(str::to_owned),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A push (one IRC line, as for `read_push`) from someone blocked on
    /// `network`: don't show it.
    pub fn push_blocked(&self, network: String, line: String) -> bool {
        let Ok(m) = chad_irc::Message::parse(&line) else {
            return false;
        };
        let msg = chad_core::ChatMessage {
            nick: m.source_nick().unwrap_or_default().to_owned(),
            account: m.tag("account").map(str::to_owned),
            ..Default::default()
        };
        let v = self.view();
        // shortcut: a cold start may read the push before the block list loads; it shows then.
        v.net(&network)
            .is_some_and(|i| v.nets[i].blocks.iter().any(|b| b.hides(&msg)))
    }

    /// Who takes reports on `network` (the home channel's `chad/report`);
    /// `None`: hide Report.
    pub fn report_to(&self, network: String) -> Option<String> {
        let v = self.view();
        v.nets[v.net(&network)?].report_to.clone()
    }

    /// Report `nick`, or their message `msgid` with `text` in `target`, to
    /// `report_to`. Answered by `Listener::reported` or `Listener::notice`; no
    /// DM opens on our side.
    pub fn report(
        &self,
        network: String,
        nick: String,
        target: Option<String>,
        msgid: Option<String>,
        text: Option<String>,
        reason: String,
    ) {
        self.net(
            network,
            NetCommand::Report(chad_core::Report {
                nick,
                target,
                msgid,
                text,
                reason,
            }),
        );
    }

    /// Join a channel's voice room, leaving any other call.
    pub fn join_voice(&self, network: String, channel: String) {
        self.net(network, NetCommand::JoinVoice { channel });
    }

    pub fn leave_voice(&self, network: String) {
        self.net(network, NetCommand::LeaveVoice);
    }

    /// A guest makes an account and carries on as it; a failure comes as `notice`.
    pub fn sign_up(
        &self,
        network: String,
        account: String,
        password: String,
        email: Option<String>,
    ) {
        let email = email.filter(|e| !e.trim().is_empty());
        self.net(
            network,
            NetCommand::Register {
                account,
                password,
                email,
            },
        );
    }

    pub fn verify(&self, network: String, code: String) {
        self.net(network, NetCommand::Verify { code });
    }

    /// Start, answer or end a verification with `nick` (see `Listener::verification`).
    pub fn verification(&self, network: String, nick: String, step: SasStep) {
        use chad_core::VerifyStep as V;
        let step = match step {
            SasStep::Start => V::Start,
            SasStep::Accept => V::Accept,
            SasStep::Match => V::Match,
            SasStep::Cancel => V::Cancel,
        };
        self.net(network, NetCommand::Verification { nick, step });
    }

    /// Keep using `nick`'s devices after their keys changed, without verifying.
    pub fn accept_keys(&self, network: String, nick: String) {
        self.net(network, NetCommand::AcceptKeys { nick });
    }

    pub fn sign_in(&self, network: String, password: String) {
        self.net(network, NetCommand::SignIn { password });
    }

    /// A password for other IRC clients on a `Network::discord` account:
    /// `Listener::password_set`.
    pub fn set_password(&self, network: String, password: String) {
        self.net(network, NetCommand::SetPassword { password });
    }

    /// What a link says about itself, through chadd: `Listener::preview`.
    pub fn preview(&self, network: String, url: String) {
        self.net(network, NetCommand::Preview { url });
    }

    /// An image's bytes (own uploads directly, the rest through chadd):
    /// `Listener::media`.
    pub fn media(&self, network: String, url: String) {
        self.net(network, NetCommand::Media { url });
    }

    /// A page of GIFs; empty `query` for trending: `Listener::gifs`.
    pub fn gifs(&self, network: String, query: String, pos: Option<String>) {
        self.net(network, NetCommand::Gifs { query, pos });
    }

    /// Pushes for this network go to this UnifiedPush endpoint; `None` stops them.
    pub fn push(&self, network: String, subscription: Option<PushSubscription>) {
        let subscription = subscription.map(|s| chad_core::PushSubscription {
            endpoint: s.endpoint,
            p256dh: s.p256dh,
            auth: s.auth,
        });
        self.net(network, NetCommand::Push { subscription });
    }

    pub fn networks(&self) -> Vec<Network> {
        let v = self.view();
        v.nets
            .iter()
            .map(|n| {
                let (status, failed, needs_code) = status(&n.info.state);
                let voice = v
                    .call
                    .as_ref()
                    .filter(|(net, _)| *net == n.info.network)
                    .and_then(|(_, s)| match s {
                        chad_core::VoiceState::Connected { channel }
                        | chad_core::VoiceState::Connecting { channel } => Some(channel.clone()),
                        chad_core::VoiceState::Idle => None,
                    });
                Network {
                    network: n.info.network.clone(),
                    name: n.info.name.clone(),
                    nick: n.info.nick.clone(),
                    icon: n.info.icon.as_ref().map(std::string::ToString::to_string),
                    status,
                    failed,
                    needs_code,
                    guest: n.info.guest,
                    discord: n.info.discord,
                    oper: n.info.oper,
                    gifs: n.info.gifs.clone(),
                    uploads: n.info.uploads,
                    receipts: n.info.receipts,
                    unread: n.unread(),
                    mentions: n.mentions(),
                    voice,
                }
            })
            .collect()
    }

    pub fn buffers(&self, network: String) -> Vec<Buffer> {
        let v = self.view();
        let Some(n) = v.net(&network).map(|i| &v.nets[i]) else {
            return Vec::new();
        };
        n.buffers
            .iter()
            .map(|b| Buffer {
                target: b.info.target.clone(),
                dm: b.info.kind == chad_core::BufferKind::Dm,
                topic: b.info.topic.clone(),
                unread: b.info.unread,
                mentions: b.info.mentions,
                online: b.info.online,
                away: b.info.away,
                encryption: match b.info.encryption {
                    Encryption::Unknown => Lock::None,
                    Encryption::Off => Lock::Off,
                    Encryption::On => Lock::On,
                    Encryption::Verified => Lock::Verified,
                    Encryption::Changed => Lock::Changed,
                },
                typing: b.typing().map(|t| n.shown(t).to_owned()).collect(),
                history_start: b.history_start,
                voice: b.voice.clone(),
                voice_allowed: b.info.voice,
                seen: b.seen().and_then(|i| b.msgs[i].msgid.clone()),
                seen_at: b.info.seen.map(|s| s.at),
            })
            .collect()
    }

    pub fn messages(&self, network: String, target: String) -> Vec<Message> {
        let v = self.view();
        let (Some(ni), Some(b)) = (v.net(&network), v.buf(&network, &target)) else {
            return Vec::new();
        };
        let n = &v.nets[ni];
        b.msgs
            .iter()
            .map(|m| {
                let profile = n.profiles.get(&chad_core::view::key(&m.nick));
                let spans = spans(&m.text, &m.mentions, &n.emoji, |x| {
                    chad_core::view::mention_name(&n.profiles, b.members.as_deref(), x).to_owned()
                });
                let text: String = spans.iter().map(|s| s.text.as_str()).collect();
                Message {
                    sticker: chad_core::emoji::sticker(&text, &n.emoji)
                        .filter(|_| m.kind != MessageKind::Action)
                        .map(|e| e.url.to_string()),
                    key: m
                        .msgid
                        .clone()
                        .unwrap_or_else(|| format!("local:{}", m.local_id.unwrap_or_default())),
                    msgid: m.msgid.clone(),
                    time: m.time,
                    nick: m.nick.clone(),
                    shown: n.shown(&m.nick).to_owned(),
                    account: m.account.clone(),
                    avatar: profile
                        .and_then(|p| p.avatar.as_ref())
                        .map(std::string::ToString::to_string),
                    links: links(&spans, &network),
                    text,
                    spans,
                    action: m.kind == MessageKind::Action,
                    notice: m.kind == MessageKind::Notice,
                    reply_to: m.reply_to.clone(),
                    pending: m.status == SendStatus::Pending,
                    failed: match &m.status {
                        SendStatus::Failed(e) => Some(e.clone()),
                        _ => None,
                    },
                    redacted: m.redacted,
                    edited: m.edited,
                    encrypted: m.crypt == Crypt::Encrypted,
                    undecryptable: m.crypt == Crypt::Failed,
                    mention: m.mention,
                    own: m.own,
                    reactions: m
                        .reactions
                        .iter()
                        .map(|r| Reaction {
                            image: chad_core::emoji::lookup(&r.emoji, &n.emoji)
                                .map(|e| e.url.to_string()),
                            emoji: r.emoji.clone(),
                            nicks: r.nicks.clone(),
                        })
                        .collect(),
                    mentions: m.mentions.iter().map(|x| x.nick.clone()).collect(),
                }
            })
            .collect()
    }

    /// A composer's text with each standard `:shortcode:` as its emoji, or
    /// `None` when there's none to replace. The network's custom emoji stay
    /// as typed.
    pub fn replace_shortcodes(
        &self,
        network: String,
        text: String,
        cursor: u32,
    ) -> Option<Shortcoded> {
        let v = self.view();
        let custom = v.net(&network).map_or(&[][..], |i| &v.nets[i].emoji);
        crate::replace_shortcodes(text, cursor, custom)
    }

    /// The network's custom emoji, sorted by name.
    pub fn custom_emoji(&self, network: String) -> Vec<CustomEmoji> {
        let v = self.view();
        let Some(i) = v.net(&network) else {
            return Vec::new();
        };
        v.nets[i]
            .emoji
            .iter()
            .map(|e| CustomEmoji {
                name: e.name.clone(),
                url: e.url.to_string(),
                sticker: e.sticker,
            })
            .collect()
    }

    /// Someone's display name and avatar, as far as we know them.
    pub fn profile(&self, network: String, nick: String) -> Profile {
        let v = self.view();
        let p = v
            .net(&network)
            .and_then(|i| v.nets[i].profiles.get(&chad_core::view::key(&nick)));
        Profile {
            display_name: p.and_then(|p| p.display_name.clone()),
            avatar: p
                .and_then(|p| p.avatar.as_ref())
                .map(std::string::ToString::to_string),
            activity: p.and_then(|p| p.activity.clone()),
            music: p.and_then(|p| p.music.as_ref()).map(|m| Music {
                start: m.start,
                length: m.length,
                url: m.url.as_ref().map(ToString::to_string),
            }),
        }
    }

    /// `nick`'s devices as last fetched (`fetch_devices`).
    pub fn devices(&self, network: String, nick: String) -> Vec<Device> {
        let v = self.view();
        v.net(&network)
            .and_then(|i| v.nets[i].devices.get(&chad_core::view::key(&nick)))
            .map(|ds| {
                ds.iter()
                    .map(|d| Device {
                        id: d.id.clone(),
                        fingerprint: d.fingerprint.clone(),
                        verified: d.verified,
                        this: d.this,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `None` until fetched with `members`.
    pub fn member_list(&self, network: String, channel: String) -> Option<Vec<Member>> {
        let v = self.view();
        let n = &v.nets[v.net(&network)?];
        let ms = v.buf(&network, &channel)?.members.as_ref()?;
        Some(
            ms.iter()
                .map(|m| Member {
                    nick: m.nick.clone(),
                    shown: n.shown(&m.nick).to_owned(),
                    prefix: m.prefix.clone(),
                    away: m.away,
                    oper: m.oper,
                    account: m.account.clone(),
                })
                .collect(),
        )
    }
}
