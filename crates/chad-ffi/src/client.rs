//! The `Client` methods Kotlin calls.

use super::*;

impl Client {
    pub(super) fn view(&self) -> MutexGuard<'_, View> {
        self.view.lock().unwrap_or_else(|e| e.into_inner())
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
        rt.spawn(async move {
            while let Some(ev) = events.recv().await {
                let applied = v.lock().unwrap_or_else(|e| e.into_inner()).apply(&ev);
                match ev {
                    Event::Messages {
                        network,
                        target,
                        messages,
                    } => {
                        let notes = notes(
                            &v.lock().unwrap_or_else(|e| e.into_inner()),
                            &network,
                            &target,
                            &messages,
                            &applied.fresh,
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
                    // Posted as a plain link, like desktop: every client can open it.
                    Event::Uploaded {
                        network,
                        target: Some(target),
                        result: Ok(url),
                    } => c.send(Command::Net {
                        network,
                        cmd: NetCommand::Send {
                            target,
                            text: url.into(),
                            reply_to: None,
                        },
                    }),
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
                    _ => {}
                }
                listener.changed();
            }
        });
        drop(_g);
        Arc::new(Self {
            core,
            view,
            _rt: rt,
        })
    }

    /// An invite URL, `irchad://` link or bare host, with an account.
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

    /// Forget a network: disconnect, delete its cache and password.
    pub fn remove(&self, network: String) {
        self.core.send(Command::Remove { network });
    }

    /// The app went to the background, or came back (D39): connections
    /// pause meanwhile (not one in voice) and catch up when back.
    pub fn background(&self, on: bool) {
        self.core.send(Command::Background { on });
    }

    /// Upload a file through chadd, then post its link to `target`.
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

    /// The network's soundboard: `Listener::sounds`.
    pub fn sounds(&self, network: String) {
        self.net(network, NetCommand::Sounds);
    }

    /// One clip's bytes: `Listener::sound`.
    pub fn sound(&self, network: String, file: String) {
        self.net(network, NetCommand::Sound { file });
    }

    pub fn send(&self, network: String, target: String, text: String, reply_to: Option<String>) {
        self.net(
            network,
            NetCommand::Send {
                target,
                text: format::markdown(&text),
                reply_to,
            },
        );
    }

    /// Replace the text of one of your own messages (D51).
    pub fn edit(&self, network: String, target: String, msgid: String, text: String) {
        let text = format::markdown(&text);
        self.net(
            network,
            NetCommand::Edit {
                target,
                msgid,
                text,
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

    /// Join a channel's voice room, leaving any other call.
    pub fn join_voice(&self, network: String, channel: String) {
        self.net(network, NetCommand::JoinVoice { channel });
    }

    pub fn leave_voice(&self, network: String) {
        self.net(network, NetCommand::LeaveVoice);
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
                    icon: n.info.icon.as_ref().map(|u| u.to_string()),
                    status,
                    failed,
                    needs_code,
                    guest: n.info.guest,
                    gifs: n.info.gifs.clone(),
                    uploads: n.info.uploads,
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
                let spans: Vec<Span> = format::parse(&m.text).into_iter().map(Span::from).collect();
                Message {
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
                        .map(|u| u.to_string()),
                    links: links(&spans, &network),
                    text: spans.iter().map(|s| s.text.as_str()).collect(),
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
                            emoji: r.emoji.clone(),
                            nicks: r.nicks.clone(),
                        })
                        .collect(),
                }
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
            avatar: p.and_then(|p| p.avatar.as_ref()).map(|u| u.to_string()),
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
