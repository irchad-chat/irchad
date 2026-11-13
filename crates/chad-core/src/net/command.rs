//! UI commands: sending messages, typing, read markers.

use super::*;

impl Net {
    pub fn command(&mut self, cmd: NetCommand) {
        let offline = !self.connected;
        match cmd {
            NetCommand::Send {
                target,
                text,
                reply_to,
                mentions,
            } => self.send_text(&target, &text, reply_to, None, &mentions),
            NetCommand::Edit {
                target,
                msgid,
                text,
                mentions,
            } => {
                if !text.trim().is_empty() && msgid.len() <= 128 {
                    // The `* ` tells plain clients it's a correction (D51).
                    let text = format!("* {text}");
                    self.send_text(&target, &text, None, Some(msgid), &mentions);
                }
            }
            NetCommand::MarkRead { target } => self.mark_read(&target),
            NetCommand::Push { subscription } => self.set_push(subscription),
            NetCommand::OpenDm { nick } if valid_name(&nick) && !self.is_channel(&nick) => {
                let key = self.fold(&nick);
                if !self.buffers.contains_key(&key) && self.add_buffer(&nick, BufferKind::Dm) {
                    self.monitor(&[nick.as_str()]);
                    self.fetch_new(&key);
                }
            }
            NetCommand::CloseDm { nick } => {
                let key = self.fold(&nick);
                if self
                    .buffers
                    .get(&key)
                    .is_some_and(|b| b.kind == BufferKind::Dm)
                {
                    self.remove_buffer(&key);
                    if self.connected && self.isupport.get("MONITOR").is_some() {
                        self.out.push(Message::new("MONITOR", ["-", nick.as_str()]));
                    }
                }
            }
            NetCommand::Verify { .. } | NetCommand::SignIn { .. } => {}
            // `conn` takes it while a guest is connected.
            NetCommand::Register { .. } => {
                let error = if self.guest {
                    "Wait until you're connected to sign up."
                } else {
                    "You already have an account here."
                };
                self.events.push(Event::SignUpFailed {
                    network: self.id(),
                    error: error.into(),
                });
            }
            NetCommand::LeaveVoice => self.voice_ended(None),
            NetCommand::SetAway { message } => {
                let message = message.map(|m| truncate(m.trim(), 200).to_owned());
                // AWAY with an empty reason would mean "back".
                let message = message.map(|m| if m.is_empty() { "Away".into() } else { m });
                if self.away != message && self.connected {
                    let mut m = Message::new("AWAY", [""; 0]);
                    m.params.extend(message.clone());
                    self.out.push(m);
                }
                self.away = message;
            }
            // `conn` runs these while connected; offline they fail at once.
            NetCommand::Upload { target, .. } => {
                let network = self.id();
                self.events.push(Event::Uploaded {
                    network,
                    target,
                    result: Err("You're offline right now.".into()),
                });
            }
            NetCommand::Preview { url } => {
                let network = self.id();
                self.events.push(Event::Preview {
                    network,
                    url,
                    preview: None,
                });
            }
            NetCommand::Media { url } => {
                let network = self.id();
                self.events.push(Event::Media {
                    network,
                    url,
                    result: Err("You're offline right now.".into()),
                });
            }
            NetCommand::Games { .. } => {
                let network = self.id();
                self.events.push(Event::Games {
                    network,
                    result: Err("You're offline right now.".into()),
                });
            }
            NetCommand::SetReceipts { on } => self.set_receipts(on),
            NetCommand::SetActivity { text, music } => {
                let text = text.map(|t| one_line(&t, MAX_ACTIVITY));
                let music = music.filter(|_| text.as_ref().is_some_and(|t| !t.is_empty()));
                let old = std::mem::replace(&mut self.activity, text.clone());
                let old_music = std::mem::replace(&mut self.music, music);
                // Music first, so the new line never shows the old song's progress.
                // Starting to manage it also clears one a crash left behind.
                if old_music != self.music || (old.is_none() && text.is_some()) {
                    self.publish_music();
                }
                match (&text, old) {
                    (Some(new), old) if old.as_ref() != Some(new) => self.publish_activity(new),
                    // Stops managing it: take down what we put up.
                    (None, Some(old)) if !old.is_empty() => self.publish_activity(""),
                    _ => {}
                }
            }
            // Offline: the set we have stays.
            NetCommand::Emoji => {}
            NetCommand::Sounds => {
                let network = self.id();
                self.events.push(Event::Sounds {
                    network,
                    result: Err("You're offline right now.".into()),
                });
            }
            NetCommand::Sound { file } => {
                let network = self.id();
                self.events.push(Event::Sound {
                    network,
                    file,
                    result: Err("You're offline right now.".into()),
                });
            }
            NetCommand::Gifs { query, pos } => {
                let network = self.id();
                self.events.push(Event::Gifs {
                    network,
                    query,
                    pos,
                    result: Err("You're offline right now.".into()),
                });
            }
            _ if offline => self.error("You're offline right now."),
            NetCommand::SetDisplayName { name } => {
                let name: String = name
                    .trim()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(MAX_DISPLAY_NAME)
                    .collect();
                self.set_metadata(DISPLAY_NAME_KEY, &name);
            }
            NetCommand::SetAvatar { url } => {
                let ok = url.is_empty()
                    || (url.len() <= 300
                        && Url::parse(&url).is_ok_and(|u| matches!(u.scheme(), "https" | "http")));
                if ok {
                    self.set_metadata(AVATAR_KEY, &url);
                } else {
                    self.error("That isn't an image link.");
                }
            }
            NetCommand::JoinVoice { channel } => self.join_voice(&channel),
            NetCommand::React {
                target,
                msgid,
                emoji,
            } => self.out.push(
                Message::new("TAGMSG", [target])
                    .with_tag("+draft/react", truncate(&emoji, 64))
                    .with_tag("+draft/reply", msgid),
            ),
            NetCommand::Unreact {
                target,
                msgid,
                emoji,
            } => self.out.push(
                Message::new("TAGMSG", [target])
                    .with_tag("+draft/unreact", truncate(&emoji, 64))
                    .with_tag("+draft/reply", msgid),
            ),
            NetCommand::Redact { target, msgid } => {
                self.out.push(Message::new("REDACT", [target, msgid]));
            }
            NetCommand::Typing { target, state } => self.typing(target, state),
            NetCommand::LoadOlder { target } => {
                let key = self.fold(&target);
                match self.store.oldest(&key) {
                    Some((msgid, _)) => self.history(&key, Hist::Before, &format!("msgid={msgid}")),
                    None => self.history(&key, Hist::Latest, "*"),
                }
            }
            NetCommand::LoadAround { target, msgid } => {
                let key = self.fold(&target);
                self.history(&key, Hist::Around, &format!("msgid={msgid}"));
            }
            NetCommand::Members { channel } => {
                let key = self.fold(&channel);
                if self.joined.contains(&key) {
                    self.who(channel);
                } else if self.who_wanted.len() < MAX_BUFFERS {
                    self.who_wanted.insert(key);
                }
            }
            NetCommand::Devices { nick } => self.fetch_devices(&nick),
            NetCommand::Verification { nick, step } => self.verification(&nick, step),
            NetCommand::AcceptKeys { nick } => {
                let key = self.fold(&nick);
                let r = self.store.trust(&key, None);
                self.pin_failed(r);
                self.emit_buffer(&key);
            }
            NetCommand::RemoveDevice { id } => self.remove_device(&id),
            NetCommand::JoinChannel { channel } => self.out.push(Message::new("JOIN", [channel])),
            NetCommand::ListChannels => {
                self.send_labeled(Message::new("LIST", [""; 0]), Pending::Browse)
            }
            NetCommand::Raw { line } => self.raw(&line),
            NetCommand::Part { channel } => self.out.push(Message::new("PART", [channel])),
            NetCommand::OpenDm { .. } => {}
            NetCommand::Block { nick, account } => self.block(&nick, account),
            NetCommand::Report(r) => self.report(r),
            NetCommand::Unblock { mask } => self.unblock(&mask),
            // The connection handles it while live (conn.rs), so here we're offline or a guest.
            NetCommand::DeleteAccount => self.events.push(Event::Error {
                network: self.info.network.clone(),
                error: "Connect first to delete your account.".into(),
            }),
        }
    }

    pub(super) fn who(&mut self, channel: String) {
        if !self.caps.enabled("labeled-response") {
            // Classic (D72): the 353s go through `names`, no label needed.
            return self.out.push(Message::new("NAMES", [channel]));
        }
        let m = Message::new("WHO", [channel.as_str(), "%tcnfa,1"]);
        self.send_labeled(m, Pending::Who { channel });
    }

    pub(super) fn send_text(
        &mut self,
        target: &str,
        text: &str,
        reply_to: Option<String>,
        edits: Option<String>,
        picked: &[String],
    ) {
        let text = text.trim_end_matches(['\n', '\r', ' ']);
        if text.trim().is_empty() {
            return;
        }
        // An encrypted message has to fit in one tag; longer ones go as several.
        if text.len() > chad_e2ee::MAX_PLAINTEXT
            && !self.is_channel(target)
            && self.may_encrypt(target)
        {
            if edits.is_some() {
                return self.error("That edit is too long.");
            }
            let piece = truncate(text, chad_e2ee::MAX_PLAINTEXT);
            let cut = piece.rfind('\n').filter(|&i| i > 0).unwrap_or(piece.len());
            self.send_text(target, &text[..cut], reply_to.clone(), None, &[]);
            return self.send_text(target, &text[cut..], reply_to, None, &[]);
        }
        let mentions = self.resolve_mentions(target, text, picked);
        self.next_local += 1;
        let local = ChatMessage {
            msgid: None,
            local_id: Some(self.next_local),
            time: time::now_ms(),
            nick: self.info.nick.clone(),
            account: None,
            kind: MessageKind::Privmsg,
            text: text.to_owned(),
            reply_to,
            status: SendStatus::Pending,
            redacted: false,
            edits,
            edited: false,
            reactions: Vec::new(),
            mention: false,
            own: true,
            crypt: Crypt::Plain,
            mentions,
        };
        if self.connected && !self.is_channel(target) && self.e2e.is_some() {
            self.send_dm(target, local);
        } else {
            self.send_plain(target, local);
        }
    }

    /// The `picked` nicks that are members of channel `target` with an
    /// account and still in `text`. None in DMs, where everything highlights
    /// and the tag would sit outside the encryption.
    fn resolve_mentions(&self, target: &str, text: &str, picked: &[String]) -> Vec<Mention> {
        let Some(b) = self
            .buffers
            .get(&self.fold(target))
            .filter(|b| b.kind == BufferKind::Channel)
        else {
            return Vec::new();
        };
        let mut out: Vec<Mention> = Vec::new();
        for nick in picked {
            if out.len() >= mention::MAX {
                break;
            }
            let Some(m) = b.members.get(&self.fold(nick)) else {
                continue;
            };
            if let Some(account) = &m.account
                && !out.iter().any(|o| o.nick == m.nick)
                && mention::find(text, &m.nick).next().is_some()
            {
                out.push(Mention {
                    nick: m.nick.clone(),
                    account: account.clone(),
                });
            }
        }
        out
    }

    pub(super) fn send_plain(&mut self, target: &str, mut local: ChatMessage) {
        let (text, reply_to, edits) = (
            local.text.clone(),
            local.reply_to.clone(),
            local.edits.clone(),
        );
        let mentions = (!local.mentions.is_empty()).then(|| mention::encode(&local.mentions));
        let pieces = split(&text);
        let limits = self.caps.enabled("draft/multiline").then(|| {
            let v = self.caps.value("draft/multiline").unwrap_or_default();
            let get = |k: &str| {
                v.split(',')
                    .find_map(|kv| kv.strip_prefix(k)?.strip_prefix('=')?.parse::<usize>().ok())
            };
            (
                get("max-bytes").unwrap_or(4096),
                get("max-lines").unwrap_or(100),
            )
        });
        let fail = match limits {
            _ if !self.connected => Some("You're offline."),
            Some((bytes, lines))
                if pieces.len() > lines
                    || pieces.iter().map(|p| p.0.len()).sum::<usize>() > bytes =>
            {
                Some("That message is too long.")
            }
            _ => None,
        };
        if let Some(why) = fail {
            local.status = SendStatus::Failed(why.into());
            self.emit_messages(target, vec![local]);
            return;
        }
        self.emit_messages(target, vec![local.clone()]);

        let token = Pending::Send {
            target: target.to_owned(),
            local: Box::new(local),
            rest: Vec::new(),
        };
        let reply = |m: Message| {
            let m = match &reply_to {
                Some(id) => m.with_tag("+draft/reply", id.as_str()),
                None => m,
            };
            let m = match &mentions {
                Some(v) => m.with_tag(mention::TAG, v.as_str()),
                None => m,
            };
            match &edits {
                Some(id) => m.with_tag("+chad/edit", id.as_str()),
                None => m,
            }
        };
        if pieces.len() == 1 {
            let m = reply(Message::new("PRIVMSG", [target, pieces[0].0]));
            self.send_labeled(m, token);
        } else if limits.is_some() {
            self.next_batch += 1;
            let id = format!("ml{}", self.next_batch);
            let start = reply(Message::new(
                "BATCH",
                [format!("+{id}").as_str(), "draft/multiline", target],
            ));
            self.send_labeled(start, token);
            for (piece, concat) in pieces {
                let mut m = Message::new("PRIVMSG", [target, piece]).with_tag("batch", id.as_str());
                if concat {
                    m = m.with_tag("draft/multiline-concat", "");
                }
                self.out.push(m);
            }
            self.out.push(Message::new("BATCH", [format!("-{id}")]));
        } else {
            // shortcut: without draft/multiline (never on ergo) each piece is its own
            // message and only the first replaces the pending one.
            let mut pieces = pieces.into_iter();
            if let Some((first, _)) = pieces.next() {
                self.send_labeled(reply(Message::new("PRIVMSG", [target, first])), token);
            }
            for (p, _) in pieces {
                self.out.push(Message::new("PRIVMSG", [target, p]));
            }
        }
    }

    pub(super) fn typing(&mut self, target: String, state: Typing) {
        let key = self.fold(&target);
        let send = match state {
            Typing::Active => self
                .typing_sent
                .get(&key)
                .is_none_or(|t| t.elapsed() >= TYPING_EVERY),
            _ => self.typing_sent.remove(&key).is_some(),
        };
        if state == Typing::Active && send {
            self.typing_sent.insert(key, Instant::now());
        }
        if send {
            self.out
                .push(Message::new("TAGMSG", [target]).with_tag("+typing", state.as_str()));
        }
    }

    pub(super) fn mark_read(&mut self, target: &str) {
        let key = self.fold(target);
        let Some((msgid, t)) = self.store.newest(&key) else {
            return;
        };
        if self.buffers.get(&key).and_then(|b| b.read) >= Some(t) {
            return;
        }
        self.set_read(&key, Some(t));
        if self
            .read_sent
            .get(&key)
            .is_some_and(|at| at.elapsed() < READ_EVERY)
        {
            self.read_due.insert(key, (target.to_owned(), msgid, t));
            return;
        }
        self.send_read(&key, target, &msgid, t);
    }

    fn send_read(&mut self, key: &str, target: &str, msgid: &str, t: i64) {
        self.read_sent.insert(key.to_owned(), Instant::now());
        if self.connected && self.caps.enabled("draft/read-marker") {
            let ts = format!("timestamp={}", time::format(t));
            self.out
                .push(Message::new("MARKREAD", [target, ts.as_str()]));
        }
        self.send_receipt(key, msgid);
    }

    /// When a read position held back by [`READ_EVERY`] can go out.
    pub fn read_due_at(&self) -> Option<Instant> {
        let at = |k: &String| {
            self.read_sent
                .get(k)
                .map_or_else(Instant::now, |at| *at + READ_EVERY)
        };
        self.read_due.keys().map(at).min()
    }

    /// Sends the held-back read positions whose wait is over.
    pub fn flush_reads(&mut self) {
        let now = Instant::now();
        let ready: Vec<String> = self
            .read_due
            .keys()
            .filter(|k| {
                self.read_sent
                    .get(*k)
                    .is_none_or(|at| now >= *at + READ_EVERY)
            })
            .cloned()
            .collect();
        for key in ready {
            if let Some((target, msgid, t)) = self.read_due.remove(&key) {
                self.send_read(&key, &target, &msgid, t);
            }
        }
    }

    /// A line typed in the server buffer, sent as is (`/` and `/quote ` optional).
    fn raw(&mut self, line: &str) {
        let line = line.trim();
        let line = line.strip_prefix('/').unwrap_or(line);
        let line = ["quote ", "raw "]
            .iter()
            .find_map(|p| {
                line.get(..p.len())
                    .filter(|h| h.eq_ignore_ascii_case(p))
                    .map(|_| &line[p.len()..])
            })
            .unwrap_or(line);
        match Message::parse(line) {
            Ok(m) if !line.contains(['\r', '\n']) => {
                self.server_line(&self.info.nick.clone(), format!("> {line}"), true);
                self.out.push(m);
            }
            _ => self.error("That isn't an IRC command."),
        }
    }
}
