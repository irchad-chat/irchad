//! Voice rooms and profiles in user metadata, and the official channel list in the
//! home channel's.

use super::*;

impl Net {
    pub(super) fn join_voice(&mut self, channel: &str) {
        let key = self.fold(channel);
        let Some(b) = self
            .buffers
            .get(&key)
            .filter(|b| b.kind == BufferKind::Channel)
        else {
            return;
        };
        let refuse = if self.classic() {
            Some("Voice isn't available on this network.".to_owned())
        } else if self.guest {
            Some("Voice needs an account. Sign up to use it.".to_owned())
        } else if b.voice_off {
            Some(format!("Voice is turned off in {}.", b.name))
        } else {
            None
        };
        if let Some(error) = refuse {
            return self.error(error);
        }
        let channel = b.name.clone();
        if self
            .voice
            .as_ref()
            .is_some_and(|v| self.cm.eq(&v.channel, &channel))
        {
            return;
        }
        self.voice = Some(Voice {
            channel: channel.clone(),
            connected: false,
        });
        self.voice_request = Some(channel);
        self.emit_voice_state();
    }

    /// `conn` joined the room: tell everyone.
    pub fn voice_connected(&mut self) {
        let Some(v) = &mut self.voice else { return };
        v.connected = true;
        let channel = v.channel.clone();
        self.set_own_voice(Some(&channel));
        self.emit_voice_state();
    }

    /// The call is over: we left, or it dropped with `error`.
    pub fn voice_ended(&mut self, error: Option<String>) {
        self.voice_request = None;
        if self.voice_published {
            self.set_own_voice(None);
        }
        if self.voice.take().is_some() {
            self.emit_voice_state();
        }
        if let Some(e) = error {
            self.error(e);
        }
    }

    /// Our `chad/activity`, when there's a connection that can carry it.
    pub(super) fn publish_activity(&mut self, text: &str) {
        if self.connected && !self.guest && self.caps.enabled("draft/metadata-2") {
            self.set_metadata(ACTIVITY_KEY, text);
        }
    }

    /// Our `chad/music` as it stands; none deletes it.
    pub(super) fn publish_music(&mut self) {
        if self.connected && !self.guest && self.caps.enabled("draft/metadata-2") {
            let v = self.music.as_ref().map(crate::Music::encode);
            self.set_metadata(MUSIC_KEY, v.as_deref().unwrap_or_default());
        }
    }

    /// `METADATA * SET key [value]`; an empty value deletes the key.
    pub(super) fn set_metadata(&mut self, key: &str, value: &str) {
        if !self.caps.enabled("draft/metadata-2") {
            return self.error("This server can't store profiles.");
        }
        let mut m = Message::new("METADATA", ["*", "SET", key]);
        if !value.is_empty() {
            m.params.push(value.to_owned());
        }
        self.out.push(m);
    }

    pub(super) fn emit_profile(&mut self, nick: &str) {
        let Some(p) = self.profiles.get(&self.fold(nick)) else {
            return;
        };
        let (display_name, avatar) = (p.display_name.clone(), p.avatar.clone());
        let (activity, music) = (p.activity.clone(), p.music.clone());
        let network = self.id();
        self.events.push(Event::Profile {
            network,
            nick: nick.to_owned(),
            display_name,
            avatar,
            activity,
            music,
        });
    }

    pub(super) fn set_own_voice(&mut self, channel: Option<&str>) {
        if !self.connected || !self.caps.enabled("draft/metadata-2") {
            return;
        }
        self.voice_published = channel.is_some();
        let mut m = Message::new("METADATA", ["*", "SET", VOICE_KEY]);
        if let Some(c) = channel {
            m.params.push(c.to_owned());
        }
        self.out.push(m);
    }

    /// A `chad/voice` value: a user's room, or `off` on a channel. Empty is unset.
    pub(super) fn voice_metadata(&mut self, target: &str, value: &str) {
        if !valid_name(target) {
            return;
        }
        if self.is_channel(target) {
            let key = self.fold(target);
            if let Some(b) = self.buffers.get_mut(&key) {
                b.voice_off = value == "off";
                let off = b.voice_off;
                self.emit_buffer(&key);
                // shortcut: chadd can't read channel metadata (ergo's API has no
                // call for it), so turning voice off is enforced here only.
                if off
                    && self
                        .voice
                        .as_ref()
                        .is_some_and(|v| self.fold(&v.channel) == key)
                {
                    self.voice_ended(Some(format!("Voice was turned off in {target}.")));
                }
            }
            return;
        }
        let room = (valid_name(value) && self.is_channel(value)).then_some(value);
        if self.cm.eq(target, &self.info.nick) {
            match &self.voice {
                // ergo keeps an always-on account's metadata after a crash, so a
                // value seen while reattaching is stale. Another device of ours in
                // a call puts its value back when it sees this.
                None if room.is_some() && !self.synced => self.set_own_voice(None),
                Some(v) if v.connected && room.is_none_or(|r| !self.cm.eq(r, &v.channel)) => {
                    let channel = v.channel.clone();
                    self.set_own_voice(Some(&channel));
                }
                _ => {}
            }
        }
        self.set_voice_of(target, room);
    }

    pub(super) fn set_voice_of(&mut self, nick: &str, room: Option<&str>) {
        let key = self.fold(nick);
        let new = room.map(|r| self.fold(r));
        let old = match &new {
            Some(ch) if self.voice_of.len() < MAX_MEMBERS || self.voice_of.contains_key(&key) => {
                self.voice_of
                    .insert(key, (nick.to_owned(), ch.clone()))
                    .map(|(_, c)| c)
            }
            Some(_) => return,
            None => self.voice_of.remove(&key).map(|(_, c)| c),
        };
        if old != new {
            for ch in [old, new].into_iter().flatten() {
                self.emit_voice_members(&ch);
            }
        }
    }

    pub(super) fn emit_voice_state(&mut self) {
        let state = match &self.voice {
            None => VoiceState::Idle,
            Some(v) if v.connected => VoiceState::Connected {
                channel: v.channel.clone(),
            },
            Some(v) => VoiceState::Connecting {
                channel: v.channel.clone(),
            },
        };
        let network = self.id();
        self.events.push(Event::Voice { network, state });
    }

    pub(super) fn emit_voice_members(&mut self, key: &str) {
        let Some(b) = self.buffers.get(key) else {
            return;
        };
        let mut nicks: Vec<String> = self
            .voice_of
            .values()
            .filter(|(_, c)| c == key)
            .map(|(n, _)| n.clone())
            .collect();
        nicks.sort_by_key(|n| n.to_lowercase());
        let (network, channel) = (self.id(), b.name.clone());
        self.events.push(Event::VoiceMembers {
            network,
            channel,
            nicks,
        });
    }

    /// Discovery's `home` is server input too: it gets JOINed as is.
    pub(crate) fn is_home_name(&self, h: &str) -> bool {
        valid_name(h) && self.is_channel(h)
    }

    /// `chad/channels` on `target`: join what was added since the last list, part
    /// what was dropped. Channels a chad left themselves stay left.
    pub(super) fn official_channels(&mut self, target: &str, value: &str) {
        // Before sync `joined` is half-built; the GET in `synced` brings it again.
        let Some(home) = self
            .home
            .clone()
            .filter(|h| self.synced && self.cm.eq(h, target))
        else {
            return;
        };
        let mut new: Vec<String> = Vec::new();
        for c in value
            .split(' ')
            .filter(|c| valid_name(c) && self.is_channel(c))
        {
            let key = self.fold(c);
            if !new.contains(&key) && new.len() < MAX_BUFFERS {
                new.push(key);
            }
        }
        let old = self.store.meta(OFFICIAL_META).unwrap_or_default();
        let old: Vec<&str> = old.split(' ').filter(|c| !c.is_empty()).collect();
        let join: Vec<String> = new
            .iter()
            .filter(|k| !old.contains(&k.as_str()) && !self.joined.contains(*k))
            .cloned()
            .collect();
        self.join_all(&join);
        let home = self.fold(&home);
        for k in old {
            if !new.iter().any(|n| n == k) && k != home && self.joined.contains(k) {
                self.out.push(Message::new("PART", [k]));
            }
        }
        self.store.set_meta(OFFICIAL_META, &new.join(" "));
    }
}

impl Net {
    /// Someone's display name or avatar: `761` is set, `766` unset.
    pub(super) fn profile_metadata(&mut self, m: &Message) {
        let value = if m.command == "761" { m.param(4) } else { None };
        let (Some(nick), Some(key)) = (m.param(1), m.param(2)) else {
            return;
        };
        let folded = self.fold(nick);
        if !self.profiles.contains_key(&folded) && self.profiles.len() >= MAX_MEMBERS {
            return;
        }
        let p = self.profiles.entry(folded).or_default();
        let text = |max| value.map(|v| one_line(v, max)).filter(|v| !v.is_empty());
        if key == MUSIC_KEY {
            p.music = value.and_then(crate::Music::parse);
        } else if key == ACTIVITY_KEY {
            p.activity = text(MAX_ACTIVITY);
        } else if key == DISPLAY_NAME_KEY {
            p.display_name = text(MAX_DISPLAY_NAME);
        } else {
            p.avatar = value
                .and_then(|v| Url::parse(v).ok())
                .filter(|u| matches!(u.scheme(), "https" | "http"));
        }
        self.emit_profile(nick);
    }
}
