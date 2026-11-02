//! Encrypted DMs (Phase 4, D8) on top of [`Net`]. Each device publishes a
//! signed record under user metadata `chad/e2e/d/<id>` and lists itself in
//! `chad/e2e/devices`. A DM goes out as one PRIVMSG per device (theirs, and our
//! other ones) with the ciphertext in `+chad/e2e` and a readable body for plain
//! clients. Devices are trusted on first use and can be verified with SAS emoji.

use std::collections::{HashMap, HashSet};

use chad_e2ee::{Device, Kind, MAX_DEVICES, Olm, Status, Step, Verification, valid_id};
use chad_irc::Message;

use super::{Net, Pending, truncate};
use crate::store::Pin;
use crate::{
    BufferKind, ChatMessage, Crypt, DeviceInfo, Encryption, Event, SendStatus, VerifyState,
    VerifyStep, time,
};

/// Set whenever a device comes or goes, so subscribers know to fetch again. The
/// value (device IDs) is for people reading it; clients use `LIST`.
pub const DEVICES_KEY: &str = "chad/e2e/devices";
const FALLBACK: &str = "🔒 Encrypted message, open IrChad to read";
const ROTATE_MS: i64 = 7 * 86_400_000;
/// D14: messages waiting for their sender's keys, and sends waiting for keys.
const MAX_WAITING: usize = 1000;
const MAX_HELD: usize = 100;
/// Devices remembered per person, current or not.
const MAX_PINS: usize = 100;

pub struct E2e {
    olm: Olm,
    key: [u8; 32],
    device: String,
    /// Folded nick → their devices as last fetched; `None` while fetching.
    devices: HashMap<String, Option<Vec<Device>>>,
    /// Folded nicks whose devices changed while a fetch was underway.
    again: HashSet<String>,
    /// Sends waiting for someone's devices: `(target, local echo)`.
    held: Vec<(String, ChatMessage)>,
    /// Messages from devices we don't know yet: `(buffer, head, message)`.
    waiting: Vec<(String, Message, ChatMessage)>,
    /// Folded nick → a verification in progress with them.
    verifications: HashMap<String, Verification>,
}

/// Each device's record is under this plus its ID; `METADATA <nick> LIST` finds them.
const RECORD_PREFIX: &str = "chad/e2e/d/";

impl Net {
    /// Turns encrypted DMs on with this device's ID and the key that opens its
    /// pickles; a new identity if there are none, or `key` doesn't open them.
    pub fn e2e_init(&mut self, key: [u8; 32], device: &str) {
        if !valid_id(device) {
            return self.error("This device's ID can't be used for encrypted DMs.");
        }
        let sessions = self.store.e2e_sessions();
        let loaded = self
            .store
            .meta("e2e_account")
            .and_then(|a| Olm::load(&a, &sessions, &key));
        let olm = loaded.unwrap_or_else(|| {
            tracing::info!("{}: new E2EE identity", self.info.network);
            self.store
                .set_meta("e2e_rotated", &time::now_ms().to_string());
            Olm::new()
        });
        self.e2e = Some(E2e {
            olm,
            key,
            device: device.to_owned(),
            devices: HashMap::new(),
            again: HashSet::new(),
            held: Vec::new(),
            waiting: Vec::new(),
            verifications: HashMap::new(),
        });
        self.save_e2e();
    }

    fn save_e2e(&mut self) {
        if let Some(e) = &mut self.e2e {
            self.store.save_e2e(e.olm.take_changes(&e.key));
        }
    }

    pub(super) fn e2e_detach(&mut self) {
        let Some(e) = &mut self.e2e else { return };
        e.devices.clear();
        e.again.clear();
        e.waiting.clear();
        let held = std::mem::take(&mut e.held);
        let verifying: Vec<String> = e.verifications.drain().map(|(k, _)| k).collect();
        for (target, mut local) in held {
            local.status = SendStatus::Failed("Not sent: the connection dropped.".into());
            self.emit_messages(&target, vec![local]);
        }
        for nick in verifying {
            self.emit_verification(&nick, VerifyState::Failed("The connection dropped.".into()));
        }
    }

    /// After every connect: rotate the fallback key weekly, check what's
    /// published for us, and fetch the devices of everyone we DM.
    pub(super) fn e2e_synced(&mut self, dms: &[String]) {
        let Some(e) = &mut self.e2e else { return };
        let rotated: i64 = self
            .store
            .meta("e2e_rotated")
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        if time::now_ms() - rotated > ROTATE_MS {
            e.olm.rotate();
            self.store
                .set_meta("e2e_rotated", &time::now_ms().to_string());
            self.save_e2e();
        }
        let me = self.info.nick.clone();
        self.fetch_devices(&me);
        for nick in dms {
            self.fetch_devices(nick);
        }
    }

    /// Asks the server for `nick`'s devices, unless that's already underway.
    pub(super) fn fetch_devices(&mut self, nick: &str) {
        let key = self.fold(nick);
        let Some(e) = &mut self.e2e else { return };
        if !self.connected || matches!(e.devices.get(&key), Some(None)) {
            return;
        }
        if !self.caps.enabled("draft/metadata-2") {
            return self.devices_known(nick, Vec::new());
        }
        if e.devices.len() >= MAX_WAITING {
            // D14: forget the finished ones; they're fetched again when needed.
            e.devices.retain(|_, d| d.is_none());
        }
        e.devices.insert(key, None);
        let m = Message::new("METADATA", [nick, "LIST"]);
        let nick = nick.to_owned();
        self.send_labeled(m, Pending::Devices { nick });
    }

    /// `chad/e2e/devices` changed: fetch again, after any fetch underway (its
    /// answer may predate the change).
    pub(super) fn devices_changed(&mut self, nick: &str) {
        let key = self.fold(nick);
        let Some(e) = &mut self.e2e else { return };
        if matches!(e.devices.get(&key), Some(None)) {
            if e.again.len() < MAX_HELD {
                e.again.insert(key);
            }
            return;
        }
        self.fetch_devices(nick);
    }

    /// The answer to `fetch_devices`: all of `nick`'s metadata.
    pub(super) fn devices_reply(&mut self, nick: &str, lines: impl Iterator<Item = Message>) {
        let owner = self.fold(nick);
        // 761 <me> <target> <key> <visibility> :<value>
        let devices: Vec<(Device, String)> = lines
            .filter(|m| m.command == "761")
            .filter_map(|m| {
                let id = m.param(2)?.strip_prefix(RECORD_PREFIX)?;
                let value = m.param(4)?;
                Some((Device::parse(&owner, id, value)?, value.to_owned()))
            })
            .take(MAX_DEVICES)
            .collect();
        // Trust on first use; a changed key is pinned too, but flagged.
        for (d, record) in &devices {
            let pins = self.store.pins(&owner);
            let full = pins.len() >= MAX_PINS;
            match pins.into_iter().find(|p| p.device.id == d.id) {
                Some(p) if p.device == *d => {}
                Some(p) => {
                    let same = p.device.same_keys(d);
                    self.store.put_pin(
                        &owner,
                        &d.id,
                        record,
                        p.verified && same,
                        p.changed || !same,
                    );
                }
                // D14: a server churning device IDs can't grow the table forever.
                None if full => {}
                None => self.store.put_pin(&owner, &d.id, record, false, false),
            }
        }
        let devices = devices.into_iter().map(|(d, _)| d).collect();
        self.devices_known(nick, devices)
    }

    fn devices_known(&mut self, nick: &str, devices: Vec<Device>) {
        let key = self.fold(nick);
        let me = self.fold(&self.info.nick) == key;
        let Some(e) = &mut self.e2e else { return };
        tracing::debug!("{nick}: {} devices", devices.len());
        e.devices.insert(key.clone(), Some(devices.clone()));
        if me {
            self.publish(&devices);
        }
        // Sends waiting on these devices (theirs, or our own for any DM).
        let held = std::mem::take(&mut self.e2e.as_mut().expect("checked").held);
        let (go, keep): (Vec<_>, Vec<_>) = held
            .into_iter()
            .partition(|(t, _)| me || self.fold(t) == key);
        self.e2e.as_mut().expect("checked").held = keep;
        for (target, local) in go {
            self.send_dm(&target, local);
        }
        let waiting = std::mem::take(&mut self.e2e.as_mut().expect("checked").waiting);
        let (retry, keep): (Vec<_>, Vec<_>) = waiting
            .into_iter()
            .partition(|(_, _, m)| self.fold(&m.nick) == key);
        self.e2e.as_mut().expect("checked").waiting = keep;
        for (target, head, msg) in retry {
            // No second wait: if the device still isn't there, it never will be.
            let msg = self.decrypt(&head, msg);
            self.live(&target, msg);
        }
        self.emit_buffer(&key);
        self.emit_devices(nick);
        if self.e2e.as_mut().expect("checked").again.remove(&key) {
            self.fetch_devices(nick);
        }
    }

    /// Makes sure the server has this device's current record.
    fn publish(&mut self, devices: &[Device]) {
        let Some(e) = &self.e2e else { return };
        let (id, me) = (e.device.clone(), self.fold(&self.info.nick));
        if devices.contains(&e.olm.device(&id)) {
            return;
        }
        let others: Vec<&str> = devices
            .iter()
            .map(|d| d.id.as_str())
            .filter(|d| *d != id)
            .collect();
        if others.len() >= MAX_DEVICES {
            return self.error(format!(
                "You have encrypted DMs on {MAX_DEVICES} devices already, so this one can't join."
            ));
        }
        let record = e.olm.record(&me, &id);
        self.set_metadata(&format!("{RECORD_PREFIX}{id}"), &record);
        // Everyone watching this key fetches our devices again.
        let list = [others.as_slice(), &[id.as_str()]].concat().join(" ");
        self.set_metadata(DEVICES_KEY, &list);
    }

    /// Takes this device's record down, before forgetting the network.
    pub fn unpublish(&mut self) {
        let me = self.fold(&self.info.nick);
        let Some(e) = &self.e2e else { return };
        let Some(Some(devices)) = e.devices.get(&me) else {
            return;
        };
        let id = e.device.clone();
        if !self.connected || !devices.iter().any(|d| d.id == id) {
            return;
        }
        let rest: Vec<&str> = devices
            .iter()
            .map(|d| d.id.as_str())
            .filter(|d| *d != id)
            .collect();
        let rest = rest.join(" ");
        self.set_metadata(&format!("{RECORD_PREFIX}{id}"), "");
        self.set_metadata(DEVICES_KEY, &rest);
    }

    /// A DM to someone: encrypted to every device of theirs and our other ones,
    /// in the clear if they have none, or held until both lists are in.
    pub(super) fn send_dm(&mut self, target: &str, mut local: ChatMessage) {
        let (peer, me) = (self.fold(target), self.fold(&self.info.nick));
        let Some(e) = &mut self.e2e else {
            return self.send_plain(target, local);
        };
        let (theirs, ours) = (e.devices.get(&peer), e.devices.get(&me));
        let (Some(Some(theirs)), Some(Some(ours))) = (theirs, ours) else {
            if e.held.len() >= MAX_HELD {
                local.status = SendStatus::Failed("Not sent: still fetching keys.".into());
                return self.emit_messages(target, vec![local]);
            }
            // Fetch what nobody asked for yet; what's underway releases this.
            let missing = [
                (!e.devices.contains_key(&peer)).then(|| target.to_owned()),
                (!e.devices.contains_key(&me)).then(|| self.info.nick.clone()),
            ];
            e.held.push((target.to_owned(), local.clone()));
            self.emit_messages(target, vec![local]);
            for nick in missing.into_iter().flatten() {
                self.fetch_devices(&nick);
            }
            return;
        };
        if theirs.is_empty() {
            return self.send_plain(target, local);
        }
        let to: Vec<Device> = theirs
            .iter()
            .chain(ours.iter().filter(|d| d.id != e.device))
            .cloned()
            .collect();
        local.crypt = Crypt::Encrypted;
        let mut copies = Vec::new();
        for d in &to {
            let Some(ct) = e.olm.encrypt(d, &local.text) else {
                continue;
            };
            let mut m = Message::new("PRIVMSG", [target, FALLBACK])
                .with_tag("+chad/e2e", ct)
                .with_tag("+chad/e2e-from", e.device.as_str())
                .with_tag("+chad/e2e-to", d.id.as_str());
            if let Some(r) = &local.reply_to {
                m = m.with_tag("+draft/reply", r.as_str());
            }
            if let Some(r) = &local.edits {
                m = m.with_tag("+chad/edit", r.as_str());
            }
            copies.push(m);
        }
        self.save_e2e();
        if copies.is_empty() {
            local.status =
                SendStatus::Failed("Couldn't encrypt this for any of their devices.".into());
            return self.emit_messages(target, vec![local]);
        }
        self.emit_messages(target, vec![local.clone()]);
        let first = copies.remove(0);
        let token = Pending::Send {
            target: target.to_owned(),
            local: Box::new(local),
            rest: copies,
        };
        self.send_labeled(first, token);
    }

    /// An incoming message as this device sees it: decrypted, the stored copy
    /// if it was decrypted before (Olm opens each message once), or `None` if
    /// it's another device's copy or waits for the sender's keys.
    pub(super) fn open(
        &mut self,
        target: &str,
        head: &Message,
        mut msg: ChatMessage,
    ) -> Option<ChatMessage> {
        if head.tag("+chad/e2e").is_none() {
            return Some(msg);
        }
        let Some(e) = &self.e2e else {
            // No keys here: one "couldn't decrypt" per message, not per copy.
            if head.tag("+chad/e2e-id").is_some() {
                return None;
            }
            msg.text.clear();
            msg.crypt = Crypt::Failed;
            return Some(msg);
        };
        if head.tag("+chad/e2e-to") != Some(e.device.as_str()) {
            return None;
        }
        if let Some(id) = head.tag("+chad/e2e-id").filter(|i| i.len() <= 128) {
            msg.msgid = Some(id.to_owned());
        }
        if let Some((_, stored)) = msg.msgid.as_deref().and_then(|id| self.store.get(id)) {
            return Some(stored);
        }
        let sender = self.fold(&msg.nick);
        let from = head.tag("+chad/e2e-from").unwrap_or_default();
        if valid_id(from) && !self.store.pins(&sender).iter().any(|p| p.device.id == from) {
            let e = self.e2e.as_mut().expect("checked");
            if e.waiting.len() < MAX_WAITING {
                e.waiting
                    .push((target.to_owned(), head.clone(), msg.clone()));
                let nick = msg.nick.clone();
                self.fetch_devices(&nick);
                return None;
            }
        }
        Some(self.decrypt(head, msg))
    }

    fn decrypt(&mut self, head: &Message, mut msg: ChatMessage) -> ChatMessage {
        let pin = head.tag("+chad/e2e-from").and_then(|from| {
            let sender = self.fold(&msg.nick);
            self.store
                .pins(&sender)
                .into_iter()
                .find(|p| p.device.id == from)
        });
        let wire = head.tag("+chad/e2e").unwrap_or_default();
        let e = self.e2e.as_mut().expect("only called with e2e on");
        match pin.and_then(|p| e.olm.decrypt(p.device.curve, wire)) {
            Some(text) => {
                msg.text = truncate(&text, super::MAX_TEXT).to_owned();
                msg.crypt = Crypt::Encrypted;
            }
            None => {
                tracing::debug!(
                    "{}: a message from {} didn't decrypt",
                    self.info.network,
                    msg.nick
                );
                msg.text.clear();
                msg.crypt = Crypt::Failed;
            }
        }
        self.save_e2e();
        msg
    }

    /// Whether a DM to `target` may go encrypted: unless they're known to have no devices.
    pub(super) fn may_encrypt(&self, target: &str) -> bool {
        let key = self.fold(target);
        self.e2e
            .as_ref()
            .is_some_and(|e| !matches!(e.devices.get(&key), Some(Some(d)) if d.is_empty()))
    }

    /// Our own devices, or those of someone we DM: worth refetching when they change.
    pub(super) fn wants_devices(&self, key: &str) -> bool {
        self.e2e.is_some()
            && (self.fold(&self.info.nick) == key
                || self
                    .buffers
                    .get(key)
                    .is_some_and(|b| b.kind == BufferKind::Dm))
    }

    /// The lock to show on a DM.
    pub(super) fn encryption(&self, key: &str) -> Encryption {
        let dm = self
            .buffers
            .get(key)
            .is_some_and(|b| b.kind == BufferKind::Dm);
        let Some(devices) = self
            .e2e
            .as_ref()
            .and_then(|e| e.devices.get(key))
            .filter(|_| dm)
        else {
            return Encryption::Unknown;
        };
        match devices {
            None => Encryption::Unknown,
            Some(d) if d.is_empty() => Encryption::Off,
            Some(d) => {
                let pins = self.store.pins(key);
                let pin = |d: &Device| pins.iter().find(|p| p.device.id == d.id);
                if pins.iter().any(|p| p.changed) {
                    Encryption::Changed
                } else if d.iter().all(|d| pin(d).is_some_and(|p| p.verified)) {
                    Encryption::Verified
                } else {
                    Encryption::On
                }
            }
        }
    }

    fn emit_devices(&mut self, nick: &str) {
        let key = self.fold(nick);
        let Some(e) = &self.e2e else { return };
        let Some(Some(devices)) = e.devices.get(&key) else {
            return;
        };
        let pins = self.store.pins(&key);
        let me = self.fold(&self.info.nick) == key;
        let devices = devices
            .iter()
            .map(|d| DeviceInfo {
                id: d.id.clone(),
                fingerprint: d.fingerprint(),
                verified: pins.iter().any(|p: &Pin| p.device.id == d.id && p.verified),
                this: me && d.id == e.device,
            })
            .collect();
        let network = self.id();
        self.events.push(Event::Devices {
            network,
            nick: nick.to_owned(),
            devices,
        });
    }

    // ---- verification -----------------------------------------------------

    pub(super) fn verification(&mut self, nick: &str, step: VerifyStep) {
        let key = self.fold(nick);
        let me = (self.fold(&self.info.nick), String::new());
        let Some(e) = &mut self.e2e else {
            return self.error("Encrypted DMs are off on this device.");
        };
        let me = (me.0, e.device.clone());
        let out = match step {
            VerifyStep::Start => {
                let (v, request) = Verification::start(me, key.clone());
                e.verifications.insert(key.clone(), v);
                self.fetch_devices(nick);
                Some(request)
            }
            VerifyStep::Accept => e.verifications.get_mut(&key).and_then(|v| v.accept()),
            VerifyStep::Cancel => e.verifications.get_mut(&key).map(|v| v.cancel()),
            VerifyStep::Match => {
                let their = self.their_ed(&key);
                let e = self.e2e.as_mut().expect("checked");
                e.verifications
                    .get_mut(&key)
                    .and_then(|v| v.confirm(&e.olm, their))
            }
        };
        self.verify_out(nick, out);
    }

    /// A `+chad/e2e-verify` TAGMSG in the DM with `nick`; `me` if it came from
    /// one of our own sessions.
    pub(super) fn verify_tag(&mut self, nick: &str, me: bool, value: &str) {
        let (key, own) = (self.fold(nick), self.fold(&self.info.nick));
        let Some(step) = Step::parse(value) else {
            return;
        };
        let Some(e) = &mut self.e2e else { return };
        if step.from == e.device {
            return; // our own echo
        }
        if me {
            // Another device of ours answered their request.
            let answered = e
                .verifications
                .get(&key)
                .is_some_and(|v| v.tx == step.tx && v.status == Status::Incoming);
            if answered {
                e.verifications.remove(&key);
                let state = VerifyState::Failed("You answered on another device.".into());
                self.emit_verification(nick, state);
            }
            return;
        }
        if step.to.as_ref().is_some_and(|t| *t != e.device) {
            return;
        }
        if step.kind == Kind::Request {
            if e.verifications.len() >= super::MAX_BUFFERS {
                return;
            }
            let v = Verification::incoming((own, e.device.clone()), key.clone(), &step);
            e.verifications.insert(key.clone(), v);
            self.fetch_devices(nick);
            return self.emit_verification(nick, VerifyState::Incoming);
        }
        let their = self.their_ed_of(&key, &step.from);
        let e = self.e2e.as_mut().expect("checked");
        let Some(v) = e.verifications.get_mut(&key) else {
            return;
        };
        let out = v.handle(&step, their);
        self.verify_out(nick, out);
    }

    /// The signing key we pinned for the device on the other end.
    fn their_ed(&self, key: &str) -> Option<chad_e2ee::Ed25519PublicKey> {
        let them = self.e2e.as_ref()?.verifications.get(key)?.them.clone()?;
        self.their_ed_of(key, &them)
    }

    fn their_ed_of(&self, key: &str, device: &str) -> Option<chad_e2ee::Ed25519PublicKey> {
        let pins = self.store.pins(key);
        pins.into_iter()
            .find(|p| p.device.id == device)
            .map(|p| p.device.ed)
    }

    /// Sends a step, and shows where the verification with `nick` stands now.
    fn verify_out(&mut self, nick: &str, out: Option<Step>) {
        let key = self.fold(nick);
        if let Some(step) = out {
            let m = Message::new("TAGMSG", [nick]).with_tag("+chad/e2e-verify", step.encode());
            self.out.push(m);
        }
        let Some(v) = self.e2e.as_ref().and_then(|e| e.verifications.get(&key)) else {
            return;
        };
        let state = match &v.status {
            Status::Incoming => VerifyState::Incoming,
            Status::Waiting => VerifyState::Waiting,
            Status::Compare(idx) => VerifyState::Compare(
                idx.iter()
                    .map(|&i| chad_e2ee::EMOJI[usize::from(i) & 63])
                    .collect(),
            ),
            Status::Done => VerifyState::Done,
            Status::Failed(why) => VerifyState::Failed((*why).into()),
        };
        let finished = matches!(state, VerifyState::Done | VerifyState::Failed(_));
        let them = v.them.clone();
        if finished {
            self.e2e
                .as_mut()
                .expect("checked")
                .verifications
                .remove(&key);
        }
        let done = state == VerifyState::Done;
        self.emit_verification(nick, state);
        if let (true, Some(id)) = (done, them) {
            self.store.trust(&key, Some(&id));
            self.emit_buffer(&key);
            self.emit_devices(nick);
        }
    }

    fn emit_verification(&mut self, nick: &str, state: VerifyState) {
        let network = self.id();
        self.events.push(Event::Verification {
            network,
            nick: nick.to_owned(),
            state,
        });
    }
}
