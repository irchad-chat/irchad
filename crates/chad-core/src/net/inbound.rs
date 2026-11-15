//! Server input: batches, labeled replies, live messages and history.

use super::*;

impl Net {
    /// Msgids chadd deleted with their accounts (D69): forget our copies, and
    /// keep `next` for the next connect.
    pub fn chadd_redacted(&mut self, ids: &[String], next: u64) {
        let began = self.store.begin();
        for id in ids {
            self.store.redact(id);
        }
        self.store.set_meta(REDACTED_CURSOR, &next.to_string());
        self.store.commit(began);
        for id in ids {
            self.refresh(id);
        }
    }

    /// Where [`Self::chadd_redacted`] left off, to ask chadd from; `None` on a
    /// new device.
    pub fn redacted_since(&self) -> Option<u64> {
        self.store
            .meta(REDACTED_CURSOR)
            .and_then(|c| c.parse().ok())
    }

    pub(super) fn item(&mut self, item: Item) {
        match item {
            Item::Message(m) => self.message(m),
            Item::Batch(b) if b.kind() == "draft/multiline" => {
                if self.blocked(&b.start) {
                    return;
                }
                if let Some((target, msg)) = self.chat_in(&Item::Batch(b)) {
                    self.live(&target, msg);
                }
            }
            // Unrequested history (no labels): keep it, but it says nothing new.
            Item::Batch(b) if b.kind() == "chathistory" => {
                if let Some(t) = b.params().first().filter(|t| valid_name(t)) {
                    let key = self.fold(t);
                    if self.buffers.contains_key(&key) {
                        self.store_history(&key, b.items);
                    }
                }
            }
            Item::Batch(b) => b.items.into_iter().for_each(|i| self.item(i)),
        }
    }

    pub(super) fn labeled(&mut self, token: Pending, item: Item) {
        let items = match item {
            Item::Batch(b) if b.kind() == "labeled-response" => b.items,
            other => vec![other],
        };
        self.reply(token, items);
    }

    /// The reply to a request, labeled or matched without labels (D72).
    pub(super) fn reply(&mut self, token: Pending, items: Vec<Item>) {
        match token {
            Pending::Send {
                target,
                mut local,
                rest,
            } => {
                let echo = items.iter().find_map(|i| self.chat(i));
                if echo.is_some() || items.iter().all(|i| failure(i).is_none()) {
                    // The other devices' copies share the first one's msgid.
                    let id = echo.as_ref().and_then(|(_, m)| m.msgid.clone());
                    for m in rest {
                        self.out.push(match &id {
                            Some(id) => m.with_tag("+chad/e2e-id", id.as_str()),
                            None => m,
                        });
                    }
                }
                match (echo, items.iter().find_map(failure)) {
                    (Some((_, mut msg)), _) => {
                        msg.local_id = local.local_id;
                        if local.crypt == Crypt::Encrypted {
                            // The echo is the ciphertext for another device.
                            msg.text.clone_from(&local.text);
                            msg.crypt = Crypt::Encrypted;
                        }
                        let key = self.fold(&target);
                        if !self.edit(&key, &msg) {
                            self.store.insert(&key, &msg);
                            self.emit_messages(&target, vec![msg]);
                        }
                    }
                    (None, Some(error)) => {
                        local.status = SendStatus::Failed(error);
                        self.emit_messages(&target, vec![*local]);
                    }
                    (None, None) => {
                        local.status = SendStatus::Sent;
                        // No labels and no echo (classic): nothing else will store it.
                        if !self.caps.enabled("labeled-response") {
                            let key = self.fold(&target);
                            self.store.insert(&key, &local);
                        }
                        self.emit_messages(&target, vec![*local]);
                    }
                }
            }
            Pending::History { target, kind } => {
                let inner = match items.into_iter().next() {
                    Some(Item::Batch(b)) if b.kind() == "chathistory" => b.items,
                    Some(Item::Message(m)) => {
                        if let Some(e) = failure(&Item::Message(m)) {
                            tracing::warn!("history for {target}: {e}");
                        }
                        Vec::new()
                    }
                    _ => Vec::new(),
                };
                // The next AFTER page goes on from this one, not from the newest
                // cached: a live message may be cached past the gap.
                let last = inner
                    .iter()
                    .filter_map(|i| self.chat(i))
                    .map(|(_, m)| m.time)
                    .max();
                let n = self.store_history(&target, inner);
                if matches!(kind, Hist::After { .. } | Hist::Latest) {
                    tracing::info!("{}: catch-up {target}: {n} messages", self.info.network);
                }
                let name = self.buffers.get(&target).map(|b| b.name.clone());
                match (kind, name) {
                    // Short of a page from the newest down: that's all there is (a fresh DM).
                    (Hist::Before | Hist::Latest, Some(name)) if n < PAGE as usize => {
                        let network = self.id();
                        self.events.push(Event::HistoryStart {
                            network,
                            target: name,
                        });
                    }
                    (Hist::After { pages }, _) if n >= PAGE as usize && pages < MAX_GAP_PAGES => {
                        if let Some(t) = last {
                            self.fetch_after(&target, t, pages + 1);
                        }
                    }
                    _ => {}
                }
            }
            Pending::Targets => {
                for m in flatten(items) {
                    let Some(t) = m
                        .param(1)
                        .filter(|t| m.param(0) == Some("TARGETS") && valid_name(t))
                    else {
                        continue;
                    };
                    // shortcut: a moderator's reply made while we were offline opens no DM; a live one does.
                    if self.is_channel(t) || m.command != "CHATHISTORY" || self.is_report_to(t) {
                        continue;
                    }
                    let key = self.fold(t);
                    if !self.buffers.contains_key(&key) && self.add_buffer(t, BufferKind::Dm) {
                        self.monitor(&[t]);
                        self.fetch_new(&key);
                    }
                }
            }
            Pending::Devices { nick } => self.devices_reply(&nick, flatten(items)),
            Pending::Block { nick } => self.block_reply(&nick, flatten(items)),
            Pending::Report => self.report_reply(&items),
            Pending::Sync => self.synced(),
            Pending::Lag(at) => {
                self.lag_out = false;
                let ms = u32::try_from(at.elapsed().as_millis()).unwrap_or(u32::MAX);
                self.info.lag_ms = Some(ms);
                self.events.push(Event::Network(self.info.clone()));
            }
            Pending::List => {
                let channels: Vec<String> = flatten(items)
                    .filter(|m| m.command == "322")
                    .filter_map(|m| m.params.into_iter().nth(1))
                    .filter(|c| valid_name(c) && self.is_channel(c))
                    .take(MAX_BUFFERS)
                    .collect();
                self.join_all(&channels);
            }
            Pending::Browse => {
                let mut channels: Vec<ListedChannel> = flatten(items)
                    .filter(|m| m.command == "322")
                    .filter_map(|m| {
                        let name = m.param(1).filter(|c| valid_name(c) && self.is_channel(c))?;
                        Some(ListedChannel {
                            name: name.to_owned(),
                            users: m.param(2).and_then(|u| u.parse().ok()).unwrap_or(0),
                            topic: truncate(m.param(3).unwrap_or_default(), 300).to_owned(),
                        })
                    })
                    .collect();
                channels.sort_by(|a, b| b.users.cmp(&a.users).then_with(|| a.name.cmp(&b.name)));
                channels.truncate(MAX_LIST_SHOWN);
                let network = self.id();
                self.events.push(Event::ChannelList { network, channels });
            }
            Pending::Who { channel } => {
                let key = self.fold(&channel);
                let prefixes: String = self.isupport.prefix().iter().map(|(_, s)| *s).collect();
                let mut members = HashMap::new();
                // 354 <me> <token> <channel> <nick> <flags> <account>
                for m in flatten(items).filter(|m| m.command == "354" && m.param(1) == Some("1")) {
                    let (Some(nick), Some(flags)) = (m.param(3), m.param(4)) else {
                        continue;
                    };
                    if members.len() >= MAX_MEMBERS || !valid_name(nick) {
                        continue;
                    }
                    let member = Member {
                        nick: nick.to_owned(),
                        account: m.param(5).filter(|a| *a != "0").map(str::to_owned),
                        away: flags.starts_with('G'),
                        oper: flags.contains('*'),
                        // multi-prefix sends them all; PREFIX order is highest first.
                        prefix: prefixes
                            .chars()
                            .find(|p| flags.contains(*p))
                            .map(String::from)
                            .unwrap_or_default(),
                    };
                    members.insert(self.fold(nick), member);
                }
                if let Some(b) = self.buffers.get_mut(&key) {
                    b.members = members;
                    self.emit_members(&key);
                }
                self.sync_dm_away();
            }
        }
    }

    pub(super) fn synced(&mut self) {
        self.synced = true;
        let channels: Vec<(String, String)> = self
            .buffers
            .iter()
            .filter(|(_, b)| b.kind == BufferKind::Channel)
            .map(|(k, b)| (k.clone(), b.name.clone()))
            .collect();
        let missing: Vec<_> = channels
            .iter()
            .filter(|(k, _)| !self.joined.contains(k))
            .collect();
        // Just signed up as a guest: the new account is in no channels yet.
        let rejoin = self.store.meta("rejoin").is_some();
        self.store.delete_meta("rejoin");
        if self.persistent && !rejoin {
            // Always-on: the server's list is the truth, so these were left elsewhere.
            for (key, _) in missing {
                self.remove_buffer(key);
            }
        } else {
            let names: Vec<String> = missing.into_iter().map(|(_, n)| n.clone()).collect();
            self.join_all(&names);
        }
        let first = self.store.meta("autojoined").is_none();
        if first && self.joined.is_empty() && !self.classic() {
            match self.home.clone() {
                // Its `chad/channels` brings the rest once we're in.
                Some(home) => self.out.push(Message::new("JOIN", [home])),
                None => self.send_labeled(Message::new("LIST", [""; 0]), Pending::List),
            }
        }
        // Catch up on list changes made while we were away.
        if let Some(home) = self.home.clone()
            && self.joined.contains(&self.fold(&home))
            && self.caps.enabled("draft/metadata-2")
        {
            self.out.push(Message::new(
                "METADATA",
                [home.as_str(), "GET", CHANNELS_KEY, REPORT_KEY],
            ));
        }
        self.store.set_meta("autojoined", "1");
        if let Some(c) = self.invite_channel.take()
            && !self.joined.contains(&self.fold(&c))
        {
            if chad_proto::meeting::is_meeting(&c) {
                self.command(NetCommand::JoinMeeting {
                    channel: c,
                    new: false,
                });
            } else {
                self.out.push(Message::new("JOIN", [c]));
            }
        }

        let keys: Vec<String> = self.buffers.keys().cloned().collect();
        for key in &keys {
            self.fetch_new(key);
        }
        if let Some(t) = self.store.latest_time()
            && self.caps.enabled("draft/chathistory")
        {
            let from = format!("timestamp={}", time::format(t));
            let to = format!("timestamp={}", time::format(time::now_ms() + 86_400_000));
            let m = Message::new("CHATHISTORY", ["TARGETS", &from, &to, "50"]);
            self.send_labeled(m, Pending::Targets);
        }
        let dms: Vec<String> = self
            .buffers
            .values()
            .filter(|b| b.kind == BufferKind::Dm)
            .map(|b| b.name.clone())
            .collect();
        self.monitor(&dms.iter().map(String::as_str).collect::<Vec<_>>());
        self.fetch_receipts(&dms);
        self.e2e_synced(&dms);
    }

    pub(super) fn message(&mut self, m: Message) {
        let from = m.source_nick().unwrap_or_default().to_owned();
        let me = self.cm.eq(&from, &self.info.nick);
        let chat = matches!(
            m.command.as_str(),
            "PRIVMSG" | "NOTICE" | "TAGMSG" | "INVITE"
        );
        // Typing, reactions, CTCP and verification included: blocked is silent.
        if chat && self.blocked(&m) {
            return;
        }
        match m.command.as_str() {
            "PRIVMSG" if !me && m.param(1) == Some("\x01VERSION\x01") => {
                self.ctcp_version(&m, &from);
            }
            // Server notices, which `chat` drops; classic networks show them (D72).
            "PRIVMSG" | "NOTICE"
                if self.classic() && m.source.as_deref().is_none_or(|s| !s.contains('!')) =>
            {
                let from = m.source.clone().unwrap_or_default();
                self.server_line(&from, m.params.last().cloned().unwrap_or_default(), false);
            }
            "PRIVMSG" | "NOTICE" => {
                if let Some((target, msg)) = self.chat_in(&Item::Message(m)) {
                    self.live(&target, msg);
                }
            }
            "TAGMSG" => self.tagmsg(&m, &from, me),
            "REDACT" => {
                if let Some(msgid) = m.param(1) {
                    self.store.redact(msgid);
                    self.refresh(msgid);
                }
            }
            // invite-notify also tells ops about others' invites: only ours.
            "INVITE" if m.param(0).is_some_and(|t| self.cm.eq(t, &self.info.nick)) => {
                let Some(channel) = m.param(1).filter(|c| valid_name(c) && self.is_channel(c))
                else {
                    return;
                };
                if !me && valid_name(&from) && !self.joined.contains(&self.fold(channel)) {
                    self.events.push(Event::Invite {
                        network: self.id(),
                        from,
                        channel: channel.to_owned(),
                    });
                }
            }
            // RPL_INVITING `<me> <nick> <channel>`.
            "341" => {
                if let (Some(nick), Some(channel)) = (m.param(1), m.param(2))
                    && valid_name(nick)
                    && valid_name(channel)
                {
                    self.events.push(Event::Invited {
                        network: self.id(),
                        nick: nick.to_owned(),
                        channel: channel.to_owned(),
                    });
                }
            }
            "JOIN" => self.join(&m, &from, me),
            "PART" | "KICK" => self.part(&m, &from),
            "MODE" | "221" => self.mode(&m),
            "RENAME" => self.rename(&m),
            "QUIT" => self.quit(&from),
            "NICK" => self.nick(&m, &from, me),
            "AWAY" => self.away(&m, &from),
            "ACCOUNT" => {
                let account = m.param(0).filter(|a| *a != "*").map(str::to_owned);
                self.everywhere(&from, |mem| mem.account.clone_from(&account));
            }
            "TOPIC" | "332" => {
                let skip = usize::from(m.command == "332");
                let (Some(chan), Some(topic)) = (m.param(skip), m.param(skip + 1)) else {
                    return;
                };
                let key = self.fold(chan);
                let topic = truncate(topic, 1024).to_owned();
                if let Some(b) = self.buffers.get_mut(&key) {
                    self.store.set_topic(&key, &topic);
                    b.topic = topic;
                    self.emit_buffer(&key);
                }
            }
            "MARKREAD" => {
                let (Some(target), Some(ts)) = (m.param(0), m.param(1)) else {
                    return;
                };
                let key = self.fold(target);
                let read = ts.strip_prefix("timestamp=").and_then(time::parse);
                if read.is_some() && self.buffers.contains_key(&key) {
                    self.set_read(&key, read);
                }
            }
            "730" | "731" => self.monitor_reply(&m),
            // Someone's devices changed: fetch them again if we DM them (or it's us).
            "761" | "766" if m.param(2) == Some(DEVICES_KEY) => {
                if let Some(nick) = m.param(1).filter(|n| valid_name(n) && !self.is_channel(n)) {
                    let key = self.fold(nick);
                    if self.wants_devices(&key) {
                        self.devices_changed(nick);
                    }
                }
            }
            // 761 <me|*> <target> <key> <visibility> :<value>; 766 is "not set".
            "761" | "766" if m.param(2) == Some(CHANNELS_KEY) => {
                // Unset is no change: one slip by an op mustn't part everyone from everything.
                if let (Some(target), Some(value)) = (m.param(1), m.param(4))
                    && m.command == "761"
                {
                    self.official_channels(target, value);
                }
            }
            "761" | "766" if m.param(2) == Some(REPORT_KEY) => {
                let value = if m.command == "761" { m.param(4) } else { None };
                if let Some(target) = m.param(1) {
                    self.report_metadata(target, value);
                }
            }
            "761" | "766" if m.param(2) == Some(RECEIPTS_KEY) => {
                let value = if m.command == "761" { m.param(4) } else { None };
                if let Some(target) = m.param(1) {
                    self.receipts_metadata(target, value);
                }
            }
            "761" | "766" if m.param(2) == Some(VOICE_KEY) => {
                let value = if m.command == "761" { m.param(4) } else { None };
                if let Some(target) = m.param(1) {
                    self.voice_metadata(target, value.unwrap_or_default());
                }
            }
            "761" | "766"
                if matches!(
                    m.param(2),
                    Some(DISPLAY_NAME_KEY | AVATAR_KEY | ACTIVITY_KEY | MUSIC_KEY)
                ) && m
                    .param(1)
                    .is_some_and(|t| valid_name(t) && !self.is_channel(t)) =>
            {
                self.profile_metadata(&m)
            }
            "PERSISTENCE" if m.param(0) == Some("STATUS") => {
                self.persistent = m.param(2) != Some("OFF");
            }
            "CAP" => self.cap(&m),
            "005" => {
                self.isupport.handle(&m);
                self.cm = self.isupport.casemapping();
            }
            // ergo's test push failed. ntfy answers 507 until the distributor has
            // subscribed to a new topic, which races our REGISTER: try again later.
            "FAIL"
                if m.param(0) == Some("WEBPUSH")
                    && m.param(1) == Some("INVALID_PARAMS")
                    && m.param(2) == Some("REGISTER")
                    && self.push_tries < PUSH_TRIES =>
            {
                self.push_registered = false;
                self.push_retry = Some(Instant::now() + Duration::from_secs(5 << self.push_tries));
                self.push_tries += 1;
            }
            "353" => self.names_reply(&m),
            "366" => self.names_end(&m),
            _ => {
                if self.classic() {
                    self.server_numeric(&m);
                }
                if let Some(e) = failure(&Item::Message(m)) {
                    self.error(e);
                }
            }
        }
    }

    /// RPL_NAMREPLY `<me> <symbol> <channel> :[prefixes]nick[!user@host] ...`,
    /// collected until `366`. ergo never sends it (`no-implicit-names`, WHOX instead).
    fn names_reply(&mut self, m: &Message) {
        let (Some(chan), Some(list)) = (m.param(2), m.param(3)) else {
            return;
        };
        let key = self.fold(chan);
        if !self.buffers.contains_key(&key) {
            return;
        }
        let prefixes: String = self.isupport.prefix().iter().map(|(_, s)| *s).collect();
        let members: Vec<(String, Member)> = list
            .split(' ')
            .filter_map(|word| {
                let start = word.find(|c| !prefixes.contains(c))?;
                let (pre, rest) = word.split_at(start);
                let nick = rest.split('!').next()?;
                valid_name(nick).then(|| {
                    let member = Member {
                        nick: nick.to_owned(),
                        account: None,
                        away: false,
                        oper: false,
                        // multi-prefix: highest first.
                        prefix: pre.chars().next().map(String::from).unwrap_or_default(),
                    };
                    (self.fold(nick), member)
                })
            })
            .collect();
        let names = self.names.entry(key).or_default();
        for (k, mem) in members {
            if names.len() >= MAX_MEMBERS {
                break;
            }
            names.insert(k, mem);
        }
    }

    fn names_end(&mut self, m: &Message) {
        let Some(key) = m.param(1).map(|c| self.fold(c)) else {
            return;
        };
        let Some(mut names) = self.names.remove(&key) else {
            return;
        };
        let Some(b) = self.buffers.get_mut(&key) else {
            return;
        };
        // NAMES knows no accounts or away; keep what JOINs and away-notify told us.
        for (k, new) in &mut names {
            if let Some(old) = b.members.get(k) {
                new.account.clone_from(&old.account);
                new.away = old.away;
            }
        }
        b.members = names;
        self.emit_members(&key);
    }

    /// A numeric nothing else handles, as text in the server buffer (D72).
    fn server_numeric(&mut self, m: &Message) {
        let numeric = m.command.len() == 3 && m.command.bytes().all(|b| b.is_ascii_digit());
        // Replies to things we asked for ourselves.
        let ours = [
            "315", "321", "322", "323", "324", "329", "333", "352", "354",
        ];
        if !numeric || ours.contains(&m.command.as_str()) {
            return;
        }
        let text = m.params.get(1..).unwrap_or_default().join(" ");
        let from = m.source.clone().unwrap_or_default();
        self.server_line(&from, text, false);
    }

    /// A line in the classic server buffer, which is made on first use.
    pub(super) fn server_line(&mut self, from: &str, text: String, own: bool) {
        if !self.classic() {
            return;
        }
        let key = SERVER_BUFFER.to_owned();
        if !self.buffers.contains_key(&key) && !self.add_buffer(SERVER_BUFFER, BufferKind::Server) {
            return;
        }
        let msg = ChatMessage {
            time: time::now_ms(),
            nick: truncate(from, MAX_NAME).to_owned(),
            kind: MessageKind::Notice,
            text: truncate(&text, MAX_TEXT).to_owned(),
            own,
            ..Default::default()
        };
        self.store.insert(&key, &msg);
        self.emit_messages(SERVER_BUFFER, vec![msg]);
        self.emit_buffer(&key);
    }

    /// Reactions, typing and verification steps.
    pub(super) fn tagmsg(&mut self, m: &Message, from: &str, me: bool) {
        if m.tag(READ_TAG).is_some() {
            return self.read_receipt(m);
        }
        let Some(target) = self.buffer_target(m) else {
            return;
        };
        if let Some(step) = m.tag("+chad/e2e-verify") {
            self.verify_tag(&target, me, step);
        } else if let (Some((emoji, add)), Some(msgid)) = (reaction(m), m.tag("+draft/reply")) {
            self.store.react(msgid, from, truncate(emoji, 64), add);
            self.refresh(msgid);
        } else if let Some(state) = m.tag("+typing").filter(|_| !me) {
            let state = match state {
                "active" => Typing::Active,
                "paused" => Typing::Paused,
                _ => Typing::Done,
            };
            let network = self.id();
            self.events.push(Event::Typing {
                network,
                target,
                nick: from.to_owned(),
                state,
            });
        }
    }

    /// `draft/channel-rename`.
    pub(super) fn rename(&mut self, m: &Message) {
        let (Some(old), Some(new)) = (
            m.param(0),
            m.param(1).filter(|c| valid_name(c) && self.is_channel(c)),
        ) else {
            return;
        };
        let (ok, nk) = (self.fold(old), self.fold(new));
        // ergo never renames onto an existing channel; don't merge two buffers.
        if ok != nk && self.buffers.contains_key(&nk) {
            return;
        }
        let Some(mut b) = self.buffers.remove(&ok) else {
            return;
        };
        let from = std::mem::replace(&mut b.name, new.to_owned());
        self.buffers.insert(nk.clone(), b);
        self.store.rename_buffer(&ok, &nk, new);
        if self.joined.remove(&ok) {
            self.joined.insert(nk.clone());
        }
        if self.who_wanted.remove(&ok) {
            self.who_wanted.insert(nk.clone());
        }
        self.typing_sent.remove(&ok);
        // Their calls are in the old room; they republish `chad/voice` when they rejoin.
        self.voice_of.retain(|_, (_, c)| *c != ok);
        let network = self.id();
        self.events.push(Event::BufferRenamed {
            network,
            from: from.clone(),
            to: new.to_owned(),
        });
        self.emit_buffer(&nk);
        self.emit_members(&nk);
        self.emit_voice_members(&nk);
        // The LiveKit room is named after the channel.
        if self
            .voice
            .as_ref()
            .is_some_and(|v| self.fold(&v.channel) == ok)
        {
            self.voice_ended(Some(format!(
                "{from} was renamed to {new}. Join voice again."
            )));
        }
    }

    /// CAP NEW/DEL after registration.
    pub(super) fn cap(&mut self, m: &Message) {
        let before: Vec<&str> = WANTED
            .iter()
            .copied()
            .filter(|c| self.caps.enabled(c))
            .collect();
        let out = self.caps.handle(m);
        self.out.extend(out);
        let after: Vec<&str> = WANTED
            .iter()
            .copied()
            .filter(|c| self.caps.enabled(c))
            .collect();
        let diff = |a: &[&str], b: &[&str]| -> Vec<String> {
            a.iter()
                .filter(|c| !b.contains(c))
                .map(|c| (*c).to_owned())
                .collect()
        };
        let (added, removed) = (diff(&after, &before), diff(&before, &after));
        if !added.is_empty() || !removed.is_empty() {
            let network = self.id();
            self.events.push(Event::CapsChanged {
                network,
                added,
                removed,
            });
        }
    }

    /// Answers a CTCP VERSION sent to us by a user (not to a channel), at most
    /// every [`CTCP_EVERY`].
    pub(super) fn ctcp_version(&mut self, m: &Message, from: &str) {
        let to_us = m.param(0).is_some_and(|t| self.cm.eq(t, &self.info.nick));
        let user = m.source.as_deref().is_some_and(|s| s.contains('!'));
        if !to_us || !user || !valid_name(from) {
            return;
        }
        if self.ctcp_at.is_some_and(|t| t.elapsed() < CTCP_EVERY) {
            return;
        }
        self.ctcp_at = Some(Instant::now());
        let reply = format!("\x01VERSION {}\x01", self.version);
        self.out
            .push(Message::new("NOTICE", [from, reply.as_str()]));
    }

    /// A PRIVMSG/NOTICE or multiline batch as `(buffer name, message)`.
    pub(super) fn chat(&self, item: &Item) -> Option<(String, ChatMessage)> {
        if self.own_report(head(item)) {
            return None;
        }
        let (head, text) = match item {
            Item::Message(m) if matches!(m.command.as_str(), "PRIVMSG" | "NOTICE") => {
                (m, m.param(1)?.to_owned())
            }
            Item::Batch(Batch { start, items }) if start.param(1) == Some("draft/multiline") => {
                let mut text = String::new();
                let mut first = true;
                for i in items {
                    let Item::Message(l) = i else { continue };
                    if !first && l.tag("draft/multiline-concat").is_none() {
                        text.push('\n');
                    }
                    first = false;
                    text.push_str(l.param(1).unwrap_or_default());
                    if text.len() > MAX_TEXT {
                        break;
                    }
                }
                (start, text)
            }
            _ => return None,
        };
        let target = self.buffer_target(head)?;
        let nick = head.source_nick()?.to_owned();
        if head.source.as_deref().is_some_and(|s| !s.contains('!')) {
            return None; // server notices
        }
        let own = self.cm.eq(&nick, &self.info.nick);
        let (kind, text) = match text.strip_prefix('\x01') {
            Some(ctcp) => {
                // ACTION only; other CTCP is never shown nor answered.
                let body = ctcp
                    .strip_suffix('\x01')
                    .unwrap_or(ctcp)
                    .strip_prefix("ACTION ")?;
                (MessageKind::Action, body.to_owned())
            }
            None if head.command == "NOTICE" => (MessageKind::Notice, text),
            None => (MessageKind::Privmsg, text),
        };
        let text = truncate(&text, MAX_TEXT).to_owned();
        let dm = !self.is_channel(&target);
        // Channels only: in a DM everything highlights, and we never send it there.
        let mentions = match head.tag(mention::TAG) {
            Some(v) if !dm => mention::decode(v, &text),
            _ => Vec::new(),
        };
        Some((
            target,
            ChatMessage {
                msgid: head
                    .tag("msgid")
                    .filter(|i| i.len() <= 128)
                    .map(str::to_owned),
                local_id: None,
                time: head
                    .tag("time")
                    .and_then(time::parse)
                    .unwrap_or_else(time::now_ms),
                account: head.tag("account").map(str::to_owned),
                kind,
                reply_to: head
                    .tag("+draft/reply")
                    .filter(|i| i.len() <= 128)
                    .map(str::to_owned),
                edits: head
                    .tag("+chad/edit")
                    .filter(|i| i.len() <= 128)
                    .map(str::to_owned),
                status: SendStatus::Sent,
                redacted: false,
                edited: false,
                reactions: Vec::new(),
                mention: !own && (dm || self.mentioned(&text, &mentions)),
                own,
                nick,
                text,
                crypt: Crypt::Plain,
                mentions,
            },
        ))
    }

    /// [`Net::chat`] as this device sees it: decrypted, or `None` if it's
    /// someone else's copy or waits for the sender's keys.
    pub(super) fn chat_in(&mut self, item: &Item) -> Option<(String, ChatMessage)> {
        let (target, msg) = self.chat(item)?;
        let msg = self.open(&target, head(item), msg)?;
        Some((target, msg))
    }

    /// The buffer a message to `params[0]` belongs in: the channel, or the other
    /// side of a DM.
    pub(super) fn buffer_target(&self, m: &Message) -> Option<String> {
        // A multiline batch start is `BATCH +id draft/multiline <target>`.
        let to = m.param(if m.command == "BATCH" { 2 } else { 0 })?;
        let statusmsg = self.isupport.get("STATUSMSG").unwrap_or_default();
        let to = to.trim_start_matches(|c| statusmsg.contains(c));
        let name = if self.is_channel(to) {
            to
        } else if self.cm.eq(to, &self.info.nick) {
            m.source_nick()?
        } else {
            to
        };
        valid_name(name).then(|| name.to_owned())
    }

    pub(super) fn live(&mut self, target: &str, msg: ChatMessage) {
        let key = self.fold(target);
        if self.edit(&key, &msg) {
            return;
        }
        if !self.buffers.contains_key(&key) {
            let kind = if self.is_channel(target) {
                BufferKind::Channel
            } else {
                BufferKind::Dm
            };
            // Messages for channels we're not in are someone else's problem.
            if kind == BufferKind::Channel || !self.add_buffer(target, kind) {
                return;
            }
            self.monitor(&[target]);
        }
        if msg.msgid.is_some() && !self.store.insert(&key, &msg) {
            return;
        }
        // Classic networks (D72) have no msgids, and no history but this cache.
        if msg.msgid.is_none() && self.classic() {
            self.store.insert(&key, &msg);
        }
        self.emit_messages(target, vec![msg]);
        self.emit_buffer(&key);
    }

    /// Stores a CHATHISTORY page and shows it. Returns how many messages it held.
    pub(super) fn store_history(&mut self, key: &str, items: Vec<Item>) -> usize {
        let Some(name) = self.buffers.get(key).map(|b| b.name.clone()) else {
            return 0;
        };
        let mut messages = Vec::new();
        let mut others = Vec::new();
        let mut n = 0;
        for item in items.into_iter().take(PAGE as usize * 2) {
            if let Some((target, msg)) = self.chat(&item) {
                // Copies for other devices still count towards the page.
                n += 1;
                if self.blocked(head(&item)) {
                    continue;
                }
                // Decrypting saves Olm state, so it stays out of the page's
                // transaction: rolling that back would lose ratchet steps.
                messages.extend(self.open(&target, head(&item), msg));
            } else if let Item::Message(m) = item {
                others.push(m);
            }
        }
        let mut touched = HashSet::new();
        let began = self.store.begin();
        for m in others.iter().filter(|m| !self.blocked(m)) {
            match m.command.as_str() {
                "TAGMSG" => {
                    if let (Some((e, add)), Some(id), Some(n)) =
                        (reaction(m), m.tag("+draft/reply"), m.source_nick())
                    {
                        self.store.react(id, n, truncate(e, 64), add);
                        touched.insert(id.to_owned());
                    }
                }
                "REDACT" => {
                    if let Some(id) = m.param(1) {
                        self.store.redact(id);
                        touched.insert(id.to_owned());
                    }
                }
                _ => {}
            }
        }
        for m in messages.iter().filter(|m| m.edits.is_none()) {
            self.store.insert(key, m);
        }
        // After the inserts: an edit can follow its message in the same page.
        for m in &messages {
            if self.edit(key, m) {
                touched.extend(m.edits.clone());
            }
        }
        self.store.commit(began);
        // After the inserts too: a receipt names a message, maybe in this page.
        for m in others.iter().filter(|m| m.command == "TAGMSG") {
            if !self.blocked(m) {
                self.read_receipt(m);
            }
        }
        messages.retain(|m| m.edits.is_none());
        // What the page changed goes out with it, as stored, in one event.
        for id in touched {
            match self.store.get(&id) {
                Some((k, stored)) if k == key => {
                    match messages
                        .iter_mut()
                        .find(|m| m.msgid.as_deref() == Some(&id))
                    {
                        Some(m) => *m = stored,
                        None => messages.push(stored),
                    }
                }
                Some(_) => self.refresh(&id),
                None => {}
            }
        }
        messages.sort_by_key(|m| m.time);
        if !messages.is_empty() {
            self.emit_messages(&name, messages);
        }
        self.emit_buffer(key);
        n
    }
}
