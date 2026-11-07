//! Who's where: JOINs, PARTs, nicks, away and presence from live server input.

use super::*;

impl Net {
    pub(super) fn join(&mut self, m: &Message, from: &str, me: bool) {
        let Some(chan) = m.param(0).filter(|c| valid_name(c) && self.is_channel(c)) else {
            return;
        };
        let key = self.fold(chan);
        if me {
            let new = !self.buffers.contains_key(&key);
            if self.who_wanted.remove(&key) {
                self.who(chan.to_owned());
            }
            if (!new || self.add_buffer(chan, BufferKind::Channel))
                && self.joined.insert(key.clone())
                && self.synced
            {
                self.fetch_new(&key);
                // Who's in voice there; the connect burst covers the rest.
                if self.caps.enabled("draft/metadata-2") {
                    self.out.push(Message::new("METADATA", [chan, "SYNC"]));
                }
            }
        } else {
            let account = m.param(1).filter(|a| *a != "*").map(str::to_owned);
            self.member(&key, from, |mem| mem.account = account);
            // Metadata isn't pushed for people who join after us.
            let folded = self.fold(from);
            if self.caps.enabled("draft/metadata-2")
                && !self.profiles.contains_key(&folded)
                && self.profiles.len() < MAX_MEMBERS
            {
                self.profiles.insert(folded, Profile::default());
                self.out.push(Message::new(
                    "METADATA",
                    [from, "GET", DISPLAY_NAME_KEY, AVATAR_KEY, ACTIVITY_KEY],
                ));
            }
        }
    }

    /// A PART, or a KICK of `params[1]`.
    pub(super) fn part(&mut self, m: &Message, from: &str) {
        let who = match m.command.as_str() {
            "KICK" => m.param(1),
            _ => Some(from),
        };
        let (Some(chan), Some(who)) = (m.param(0), who) else {
            return;
        };
        let key = self.fold(chan);
        if self.cm.eq(who, &self.info.nick) {
            self.joined.remove(&key);
            self.remove_buffer(&key);
        } else {
            let nick = self.fold(who);
            if let Some(b) = self.buffers.get_mut(&key)
                && b.members.remove(&nick).is_some()
            {
                self.emit_members(&key);
            }
        }
    }

    /// Our user modes (oper), or prefix changes in a channel.
    pub(super) fn mode(&mut self, m: &Message) {
        let (Some(target), Some(modes)) = (m.param(0), m.param(1)) else {
            return;
        };

        if self.cm.eq(target, &self.info.nick) {
            let mut add = true;
            let was = self.info.oper;
            for c in modes.chars() {
                match c {
                    '+' => add = true,
                    '-' => add = false,
                    'o' => self.info.oper = add,
                    _ => {}
                }
            }
            if self.info.oper != was {
                self.events.push(Event::Network(self.info.clone()));
            }
            return;
        }

        let key = self.fold(target);
        // Members keep only their highest prefix, so a WHO is simpler than replaying the change.
        if self.joined.contains(&key)
            && self
                .buffers
                .get(&key)
                .is_some_and(|b| !b.members.is_empty())
            && self
                .isupport
                .prefix()
                .iter()
                .any(|(c, _)| modes.contains(*c))
        {
            self.who(target.to_owned());
        }
    }

    pub(super) fn quit(&mut self, from: &str) {
        self.set_voice_of(from, None);
        let nick = self.fold(from);
        let keys: Vec<String> = self
            .buffers
            .iter_mut()
            .filter_map(|(k, b)| b.members.remove(&nick).map(|_| k.clone()))
            .collect();
        keys.iter().for_each(|k| self.emit_members(k));
    }

    pub(super) fn nick(&mut self, m: &Message, from: &str, me: bool) {
        let Some(new) = m.param(0).filter(|n| valid_name(n)) else {
            return;
        };
        if me {
            new.clone_into(&mut self.info.nick);
            self.events.push(Event::Network(self.info.clone()));
        }
        let (old, newk) = (self.fold(from), self.fold(new));
        if let Some(p) = self.profiles.remove(&old) {
            self.profiles.insert(newk.clone(), p);
            self.emit_profile(new);
        }
        if self.receipts_of.remove(&old) {
            self.receipts_of.insert(newk.clone());
        }
        if let Some((_, ch)) = self.voice_of.remove(&old) {
            self.voice_of
                .insert(newk.clone(), (new.to_owned(), ch.clone()));
            self.emit_voice_members(&ch);
        }
        let keys: Vec<String> = self
            .buffers
            .iter_mut()
            .filter_map(|(k, b)| {
                let mut mem = b.members.remove(&old)?;
                new.clone_into(&mut mem.nick);
                b.members.insert(newk.clone(), mem);
                Some(k.clone())
            })
            .collect();
        keys.iter().for_each(|k| self.emit_members(k));
    }

    pub(super) fn away(&mut self, m: &Message, from: &str) {
        let away = m.param(0).is_some_and(|r| !r.is_empty());
        // ergo's auto-away: no device is connected, so any `chad/voice`
        // they left behind is stale (D16). A device that comes back
        // and is in a call puts it back.
        if m.param(0) == Some(ERGO_AUTO_AWAY) {
            self.set_voice_of(from, None);
        }
        self.everywhere(from, |mem| mem.away = away);
        let key = self.fold(from);
        if let Some(b) = self.buffers.get_mut(&key)
            && b.kind == BufferKind::Dm
        {
            b.away = away;
            self.emit_buffer(&key);
        }
    }

    /// MONITOR: DM partners coming online (730) or going offline (731).
    pub(super) fn monitor_reply(&mut self, m: &Message) {
        let online = m.command == "730";
        for nick in m.param(1).unwrap_or_default().split(',') {
            let key = self.fold(nick.split('!').next().unwrap_or(nick));
            if let Some(b) = self.buffers.get_mut(&key)
                && b.kind == BufferKind::Dm
            {
                b.online = Some(online);
                b.away &= online;
                self.emit_buffer(&key);
            }
        }
        self.sync_dm_away();
    }
}
