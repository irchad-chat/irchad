//! Opt-in read receipts in DMs (D68): `TAGMSG <nick> +chad/read=<msgid>`, sent
//! only while both accounts have user metadata `chad/receipts=1`.

use super::*;

/// msgids we take in a receipt; ergo's are 26 characters.
const MAX_MSGID: usize = 128;

impl Net {
    pub(super) fn set_receipts(&mut self, on: bool) {
        if self.guest {
            return self.error("Read receipts need an account.");
        }
        if !self.connected {
            return self.error("Connect first to change read receipts.");
        }
        self.set_metadata(RECEIPTS_KEY, if on { "1" } else { "" });
        self.own_receipts(on);
    }

    /// Our account's `chad/receipts`, from us or another of our devices.
    fn own_receipts(&mut self, on: bool) {
        if self.info.receipts == on {
            return;
        }
        self.info.receipts = on;
        if on {
            self.store.set_meta(RECEIPTS_META, "1");
        } else {
            self.store.delete_meta(RECEIPTS_META);
        }
        self.events.push(Event::Network(self.info.clone()));
        let dms: Vec<(String, String)> = self
            .buffers
            .iter()
            .filter(|(_, b)| b.kind == BufferKind::Dm)
            .map(|(k, b)| (k.clone(), b.name.clone()))
            .collect();
        for (key, _) in &dms {
            self.emit_buffer(key);
        }
        let names: Vec<String> = dms.into_iter().map(|(_, n)| n).collect();
        self.fetch_receipts(&names);
    }

    /// Asks whether these DM peers have receipts on; only matters while ours are.
    /// Changes come by `METADATA * SUB` while we share a channel.
    pub(super) fn fetch_receipts(&mut self, nicks: &[String]) {
        if !self.info.receipts || !self.connected || !self.caps.enabled("draft/metadata-2") {
            return;
        }
        for nick in nicks {
            self.out.push(Message::new(
                "METADATA",
                [nick.as_str(), "GET", RECEIPTS_KEY],
            ));
        }
    }

    /// Someone's `chad/receipts`: `761` with a value, `766` unset.
    pub(super) fn receipts_metadata(&mut self, target: &str, value: Option<&str>) {
        if !valid_name(target) || self.is_channel(target) {
            return;
        }
        let on = value == Some("1");
        if self.cm.eq(target, &self.info.nick) {
            return self.own_receipts(on);
        }
        let key = self.fold(target);
        if on {
            if self.receipts_of.len() < MAX_MEMBERS {
                self.receipts_of.insert(key);
            }
            return;
        }
        self.receipts_of.remove(&key);
        // They turned theirs off: what they sent before goes too.
        if let Some(b) = self.buffers.get_mut(&key)
            && b.seen.take().is_some()
        {
            self.store.set_seen(&key, None);
            self.emit_buffer(&key);
        }
    }

    /// We've read `key` up to `msgid`: tell them, if we both send receipts.
    pub(super) fn send_receipt(&mut self, key: &str, msgid: &str) {
        let Some(b) = self.buffers.get(key) else {
            return;
        };
        if b.kind != BufferKind::Dm
            || !self.info.receipts
            || !self.receipts_of.contains(key)
            || !self.connected
            || !self.caps.enabled("message-tags")
        {
            return;
        }
        let target = b.name.clone();
        self.out
            .push(Message::new("TAGMSG", [target]).with_tag(READ_TAG, msgid));
    }

    /// A `+chad/read` TAGMSG, live or from history. Only the other side of a DM
    /// can say they've read it, only up to one of its messages, and never back.
    pub(super) fn read_receipt(&mut self, m: &Message) {
        let Some(id) = m
            .tag(READ_TAG)
            .filter(|i| !i.is_empty() && i.len() <= MAX_MSGID)
        else {
            return;
        };
        let Some(from) = m.source_nick() else { return };
        if !self.info.receipts || self.cm.eq(from, &self.info.nick) {
            return;
        }
        // To us, so `buffer_target` is the sender; a TAGMSG to a channel isn't a DM.
        let Some(target) = self.buffer_target(m) else {
            return;
        };
        let key = self.fold(&target);
        if key != self.fold(from) {
            return;
        }
        let Some(b) = self.buffers.get(&key).filter(|b| b.kind == BufferKind::Dm) else {
            return;
        };
        let Some((buffer, msg)) = self.store.get(id) else {
            return;
        };
        if buffer != key || b.seen.is_some_and(|s| s.upto >= msg.time) {
            return;
        }
        let now = time::now_ms();
        let at = m
            .tag("time")
            .and_then(time::parse)
            .map_or(now, |t| t.min(now))
            .max(msg.time);
        let seen = Some(Seen { upto: msg.time, at });
        if let Some(b) = self.buffers.get_mut(&key) {
            b.seen = seen;
        }
        self.store.set_seen(&key, seen);
        self.emit_buffer(&key);
    }
}
