//! DM encryption (D8): an Olm account per device and network, a session per
//! pair of devices, signed device records and SAS verification. Sans-IO: the
//! caller sends what this returns and keeps the pickles.

use std::collections::HashMap;

use sha2::{Digest, Sha256};
use vodozemac::Ed25519Signature;
use vodozemac::olm::{Account, AccountPickle, OlmMessage, Session, SessionConfig, SessionPickle};
use vodozemac::sas::{EstablishedSas, Mac, Sas};
pub use vodozemac::{Curve25519PublicKey, Ed25519PublicKey};
use vodozemac::{base64_decode, base64_encode};

/// Devices per account that we publish or accept. One `METADATA GET` fetches them
/// all within IRC's 15 parameters.
pub const MAX_DEVICES: usize = 12;
/// Plaintext bytes per message, so the ciphertext fits ergo's 4094-byte client tag budget.
pub const MAX_PLAINTEXT: usize = 2000;
/// Sessions kept per peer device. Older ones can't decrypt late messages.
const MAX_SESSIONS: usize = 4;

/// Device IDs and verification IDs, as they go into metadata keys.
pub fn valid_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn random_id() -> String {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|b| format!("{b:02x}")).collect()
}

/// A new key for the pickles, kept in the OS secret store.
pub fn new_pickle_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).expect("no OS randomness");
    k
}

fn signed(owner: &str, id: &str, curve: &str, ed: &str, fallback: &str) -> String {
    format!("chad-e2e-device\n{owner}\n{id}\n{curve}\n{ed}\n{fallback}")
}

/// A device's keys from its `chad/e2e/d/<id>` record, with the signature checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub curve: Curve25519PublicKey,
    pub ed: Ed25519PublicKey,
    pub fallback: Curve25519PublicKey,
}

impl Device {
    /// `<curve25519> <ed25519> <fallback> <signature>`, base64. The signature
    /// covers `owner` (the folded nick it's published under) and `id` too, so a
    /// record can't be moved to another device or person.
    pub fn parse(owner: &str, id: &str, value: &str) -> Option<Self> {
        let [c, e, f, s] = value.split(' ').collect::<Vec<_>>().try_into().ok()?;
        let d = Self {
            id: id.to_owned(),
            curve: Curve25519PublicKey::from_base64(c).ok()?,
            ed: Ed25519PublicKey::from_base64(e).ok()?,
            fallback: Curve25519PublicKey::from_base64(f).ok()?,
        };
        let sig = Ed25519Signature::from_base64(s).ok()?;
        d.ed.verify(signed(owner, id, c, e, f).as_bytes(), &sig)
            .ok()?;
        valid_id(id).then_some(d)
    }

    /// The same device, maybe with a newer fallback key.
    pub fn same_keys(&self, other: &Device) -> bool {
        self.curve == other.curve && self.ed == other.ed
    }

    /// The signing key in groups of four, for people to compare.
    pub fn fingerprint(&self) -> String {
        let b = self.ed.to_base64();
        let groups: Vec<&str> = b
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap_or_default())
            .collect();
        groups.join(" ")
    }
}

/// Encrypted pickles to save after an operation.
#[derive(Debug, Default)]
pub struct Changes {
    pub account: Option<String>,
    /// `(session id, their curve25519 key, pickle)`.
    pub sessions: Vec<(String, String, String)>,
    /// Session IDs to delete.
    pub removed: Vec<String>,
}

/// This device's Olm account and its sessions.
pub struct Olm {
    account: Account,
    /// Their curve25519 key (base64) → sessions, most recently used last.
    sessions: HashMap<String, Vec<Session>>,
    account_dirty: bool,
    dirty: Vec<(String, String)>,
    removed: Vec<String>,
}

impl Default for Olm {
    fn default() -> Self {
        Self::new()
    }
}

impl Olm {
    pub fn new() -> Self {
        let mut account = Account::new();
        account.generate_fallback_key();
        Self {
            account,
            sessions: HashMap::new(),
            account_dirty: true,
            dirty: Vec::new(),
            removed: Vec::new(),
        }
    }

    /// From the saved account and `(their key, session)` pickles, oldest first.
    /// `None` if `key` doesn't open the account. A session that won't open is skipped.
    pub fn load(account: &str, sessions: &[(String, String)], key: &[u8; 32]) -> Option<Self> {
        let account = Account::from_pickle(AccountPickle::from_encrypted(account, key).ok()?);
        let mut olm = Self {
            account,
            sessions: HashMap::new(),
            account_dirty: false,
            dirty: Vec::new(),
            removed: Vec::new(),
        };
        for (peer, pickle) in sessions {
            if let Ok(p) = SessionPickle::from_encrypted(pickle, key) {
                olm.sessions
                    .entry(peer.clone())
                    .or_default()
                    .push(Session::from_pickle(p));
            }
        }
        Some(olm)
    }

    /// The keys we publish.
    pub fn device(&self, id: &str) -> Device {
        Device {
            id: id.to_owned(),
            curve: self.account.curve25519_key(),
            ed: self.account.ed25519_key(),
            fallback: self.fallback(),
        }
    }

    fn fallback(&self) -> Curve25519PublicKey {
        // Never marked published, so the current one is always listed.
        let keys = self.account.fallback_key();
        *keys
            .values()
            .next()
            .expect("generated in new() and rotate()")
    }

    /// The `chad/e2e/d/<id>` value: our keys, signed for `owner` (our folded nick).
    pub fn record(&self, owner: &str, id: &str) -> String {
        let (c, e, f) = (
            self.account.curve25519_key().to_base64(),
            self.account.ed25519_key().to_base64(),
            self.fallback().to_base64(),
        );
        let sig = self.account.sign(signed(owner, id, &c, &e, &f));
        format!("{c} {e} {f} {}", sig.to_base64())
    }

    /// A new fallback key. The previous one keeps working until the next rotation.
    pub fn rotate(&mut self) {
        self.account.generate_fallback_key();
        self.account_dirty = true;
    }

    /// `<type>:<base64>`, for one device. `None` if its keys are unusable.
    pub fn encrypt(&mut self, to: &Device, plaintext: &str) -> Option<String> {
        let peer = to.curve.to_base64();
        let list = self.sessions.entry(peer.clone()).or_default();
        if list.is_empty() {
            let s = self
                .account
                .create_outbound_session(SessionConfig::version_1(), to.curve, to.fallback)
                .ok()?;
            list.push(s);
        }
        let s = list.last_mut()?;
        let (ty, body) = s.encrypt(plaintext).ok()?.to_parts();
        self.dirty.push((peer, s.session_id()));
        Some(format!("{ty}:{}", base64_encode(body)))
    }

    /// What [`Olm::encrypt`] made, from the device with curve25519 key `from`.
    /// `None` if it's garbled, tampered with or a replay.
    pub fn decrypt(&mut self, from: Curve25519PublicKey, wire: &str) -> Option<String> {
        let (ty, body) = wire.split_once(':')?;
        let message = OlmMessage::from_parts(ty.parse().ok()?, &base64_decode(body).ok()?).ok()?;
        let peer = from.to_base64();
        let list = self.sessions.entry(peer.clone()).or_default();
        let found = match &message {
            // A replayed pre-key message must not start a second session.
            OlmMessage::PreKey(p) => list
                .iter()
                .position(|s| s.session_id() == p.session_id())
                .map(|i| (i, list[i].decrypt(&message).ok())),
            OlmMessage::Normal(_) => (0..list.len())
                .rev()
                .find_map(|i| list[i].decrypt(&message).ok().map(|p| (i, Some(p)))),
        };
        let plaintext = match (found, &message) {
            (Some((i, Some(p))), _) => {
                let s = list.remove(i);
                list.push(s);
                p
            }
            (None, OlmMessage::PreKey(p)) => {
                let r = self
                    .account
                    .create_inbound_session(SessionConfig::version_1(), from, p)
                    .ok()?;
                self.account_dirty = true;
                list.push(r.session);
                if list.len() > MAX_SESSIONS {
                    self.removed.push(list.remove(0).session_id());
                }
                r.plaintext
            }
            _ => return None,
        };
        let s = list.last()?;
        self.dirty.push((peer, s.session_id()));
        String::from_utf8(plaintext).ok()
    }

    /// The pickles changed since last time, encrypted with `key`.
    pub fn take_changes(&mut self, key: &[u8; 32]) -> Changes {
        let account =
            std::mem::take(&mut self.account_dirty).then(|| self.account.pickle().encrypt(key));
        let mut sessions: Vec<(String, String, String)> = Vec::new();
        for (peer, id) in std::mem::take(&mut self.dirty) {
            if sessions.iter().any(|(i, _, _)| *i == id) {
                continue;
            }
            let found = self.sessions.get(&peer).into_iter().flatten();
            if let Some(s) = found.into_iter().find(|s| s.session_id() == id) {
                sessions.push((id, peer, s.pickle().encrypt(key)));
            }
        }
        Changes {
            account,
            sessions,
            removed: std::mem::take(&mut self.removed),
        }
    }

    fn ed(&self) -> String {
        self.account.ed25519_key().to_base64()
    }
}

/// The SAS emoji, by index (the same list as Matrix's).
pub const EMOJI: [(&str, &str); 64] = [
    ("🐶", "Dog"),
    ("🐱", "Cat"),
    ("🦁", "Lion"),
    ("🐎", "Horse"),
    ("🦄", "Unicorn"),
    ("🐷", "Pig"),
    ("🐘", "Elephant"),
    ("🐰", "Rabbit"),
    ("🐼", "Panda"),
    ("🐓", "Rooster"),
    ("🐧", "Penguin"),
    ("🐢", "Turtle"),
    ("🐟", "Fish"),
    ("🐙", "Octopus"),
    ("🦋", "Butterfly"),
    ("🌷", "Flower"),
    ("🌳", "Tree"),
    ("🌵", "Cactus"),
    ("🍄", "Mushroom"),
    ("🌏", "Globe"),
    ("🌙", "Moon"),
    ("☁️", "Cloud"),
    ("🔥", "Fire"),
    ("🍌", "Banana"),
    ("🍎", "Apple"),
    ("🍓", "Strawberry"),
    ("🌽", "Corn"),
    ("🍕", "Pizza"),
    ("🎂", "Cake"),
    ("❤️", "Heart"),
    ("😀", "Smiley"),
    ("🤖", "Robot"),
    ("🎩", "Hat"),
    ("👓", "Glasses"),
    ("🔧", "Spanner"),
    ("🎅", "Santa"),
    ("👍", "Thumbs up"),
    ("☂️", "Umbrella"),
    ("⌛", "Hourglass"),
    ("⏰", "Clock"),
    ("🎁", "Gift"),
    ("💡", "Light bulb"),
    ("📕", "Book"),
    ("✏️", "Pencil"),
    ("📎", "Paperclip"),
    ("✂️", "Scissors"),
    ("🔒", "Lock"),
    ("🔑", "Key"),
    ("🔨", "Hammer"),
    ("☎️", "Telephone"),
    ("🏁", "Flag"),
    ("🚂", "Train"),
    ("🚲", "Bicycle"),
    ("✈️", "Aeroplane"),
    ("🚀", "Rocket"),
    ("🏆", "Trophy"),
    ("⚽", "Ball"),
    ("🎸", "Guitar"),
    ("🎺", "Trumpet"),
    ("🔔", "Bell"),
    ("⚓", "Anchor"),
    ("🎧", "Headphones"),
    ("📁", "Folder"),
    ("📌", "Pin"),
];

/// One `+chad/e2e-verify` tag value: `<tx> <from device> <to device or -> <kind> [arg]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub tx: String,
    pub from: String,
    pub to: Option<String>,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Request,
    /// The responder's commitment to its SAS key.
    Accept(String),
    Key(String),
    Mac(String),
    Cancel,
}

impl Step {
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.split(' ');
        let (tx, from, to, kind) = (it.next()?, it.next()?, it.next()?, it.next()?);
        let arg = it.next().filter(|a| a.len() <= 128 && it.next().is_none());
        let kind = match (kind, arg) {
            ("request", None) => Kind::Request,
            ("accept", Some(a)) => Kind::Accept(a.into()),
            ("key", Some(a)) => Kind::Key(a.into()),
            ("mac", Some(a)) => Kind::Mac(a.into()),
            ("cancel", None) => Kind::Cancel,
            _ => return None,
        };
        let to = (to != "-").then(|| to.to_owned());
        (valid_id(tx) && valid_id(from) && to.as_deref().is_none_or(valid_id)).then(|| Self {
            tx: tx.into(),
            from: from.into(),
            to,
            kind,
        })
    }

    pub fn encode(&self) -> String {
        let (kind, arg) = match &self.kind {
            Kind::Request => ("request", None),
            Kind::Accept(a) => ("accept", Some(a)),
            Kind::Key(a) => ("key", Some(a)),
            Kind::Mac(a) => ("mac", Some(a)),
            Kind::Cancel => ("cancel", None),
        };
        let to = self.to.as_deref().unwrap_or("-");
        let mut s = format!("{} {} {to} {kind}", self.tx, self.from);
        if let Some(a) = arg {
            s.push(' ');
            s.push_str(a);
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// They asked: [`Verification::accept`] or [`Verification::cancel`].
    Incoming,
    Waiting,
    /// Emoji indices to compare: [`Verification::confirm`] or cancel.
    Compare([u8; 7]),
    Done,
    Failed(&'static str),
}

/// One side of a SAS verification between our device and one of theirs.
///
/// The initiator sends `request`. The device that accepts commits to its SAS key
/// (so neither side can pick its key after seeing the other's). Keys are swapped,
/// people compare the emoji, and each side MACs its own signing key, which the
/// other checks against the key it pinned.
pub struct Verification {
    pub tx: String,
    pub status: Status,
    /// The other device, once known.
    pub them: Option<String>,
    ours: bool,
    me: (String, String),
    them_owner: String,
    sas: Option<Sas>,
    /// Initiator's and responder's SAS keys.
    keys: Option<(String, String)>,
    established: Option<EstablishedSas>,
    commitment: Option<String>,
    their_mac: Option<String>,
    confirmed: bool,
}

fn commit(key: &str, tx: &str) -> String {
    base64_encode(Sha256::digest(format!("{key}|{tx}")))
}

impl Verification {
    fn new(tx: String, ours: bool, me: (String, String), them_owner: String) -> Self {
        Self {
            tx,
            status: if ours {
                Status::Waiting
            } else {
                Status::Incoming
            },
            them: None,
            ours,
            me,
            them_owner,
            sas: None,
            keys: None,
            established: None,
            commitment: None,
            their_mac: None,
            confirmed: false,
        }
    }

    /// We ask `them_owner` (folded nick) to verify. `me` is our folded nick and device.
    pub fn start(me: (String, String), them_owner: String) -> (Self, Step) {
        let v = Self::new(random_id(), true, me, them_owner);
        let step = v.step(Kind::Request);
        (v, step)
    }

    /// They asked with `request`.
    pub fn incoming(me: (String, String), them_owner: String, request: &Step) -> Self {
        let mut v = Self::new(request.tx.clone(), false, me, them_owner);
        v.them = Some(request.from.clone());
        v
    }

    fn step(&self, kind: Kind) -> Step {
        Step {
            tx: self.tx.clone(),
            from: self.me.1.clone(),
            to: self.them.clone(),
            kind,
        }
    }

    pub fn accept(&mut self) -> Option<Step> {
        if self.status != Status::Incoming {
            return None;
        }
        let sas = Sas::new();
        let c = commit(&sas.public_key().to_base64(), &self.tx);
        self.sas = Some(sas);
        self.status = Status::Waiting;
        Some(self.step(Kind::Accept(c)))
    }

    pub fn cancel(&mut self) -> Step {
        self.status = Status::Failed("Verification was cancelled.");
        self.step(Kind::Cancel)
    }

    /// A step from their device. `their_ed` is the signing key we pinned for it.
    pub fn handle(&mut self, step: &Step, their_ed: Option<Ed25519PublicKey>) -> Option<Step> {
        if step.tx != self.tx || matches!(self.status, Status::Done | Status::Failed(_)) {
            return None;
        }
        if self.them.as_ref().is_some_and(|t| *t != step.from) {
            return None;
        }
        match &step.kind {
            Kind::Accept(c) if self.ours && self.them.is_none() => {
                self.them = Some(step.from.clone());
                self.commitment = Some(c.clone());
                let sas = Sas::new();
                let key = sas.public_key().to_base64();
                self.sas = Some(sas);
                Some(self.step(Kind::Key(key)))
            }
            Kind::Key(k) if self.established.is_none() => {
                let sas = self.sas.take()?;
                let mine = sas.public_key().to_base64();
                if self.ours && self.commitment.as_deref() != Some(&commit(k, &self.tx)) {
                    self.status = Status::Failed("Their key didn't match what they promised.");
                    return None;
                }
                let Ok(est) = sas.diffie_hellman_with_raw(k) else {
                    self.status = Status::Failed("They sent a bad key.");
                    return None;
                };
                let keys = if self.ours {
                    (mine.clone(), k.clone())
                } else {
                    (k.clone(), mine.clone())
                };
                let info = self.info("CHAD_SAS", &keys);
                self.status = Status::Compare(est.bytes(&info).emoji_indices());
                self.established = Some(est);
                self.keys = Some(keys);
                // The responder answers with its key; the initiator already sent its own.
                (!self.ours).then(|| self.step(Kind::Key(mine)))
            }
            Kind::Mac(m) => {
                self.their_mac = Some(m.clone());
                self.finish(their_ed);
                None
            }
            Kind::Cancel => {
                self.status = Status::Failed("They cancelled the verification.");
                None
            }
            _ => None,
        }
    }

    /// The emoji match on both screens.
    pub fn confirm(&mut self, olm: &Olm, their_ed: Option<Ed25519PublicKey>) -> Option<Step> {
        if !matches!(self.status, Status::Compare(_)) || self.confirmed {
            return None;
        }
        let info = self.mac_info(true);
        let mac = self.established.as_ref()?.calculate_mac(&olm.ed(), &info);
        self.confirmed = true;
        self.status = Status::Waiting;
        let step = self.step(Kind::Mac(mac.to_base64()));
        self.finish(their_ed);
        Some(step)
    }

    fn finish(&mut self, their_ed: Option<Ed25519PublicKey>) {
        let (Some(est), Some(mac), true) = (&self.established, &self.their_mac, self.confirmed)
        else {
            return;
        };
        let info = self.mac_info(false);
        let ok = their_ed.is_some_and(|ed| {
            Mac::from_base64(mac).is_ok_and(|m| est.verify_mac(&ed.to_base64(), &info, &m).is_ok())
        });
        self.status = if ok {
            Status::Done
        } else {
            Status::Failed("Their device key didn't check out. Don't trust it.")
        };
    }

    fn info(&self, label: &str, (ik, rk): &(String, String)) -> String {
        let (me, them) = (
            format!("{}|{}", self.me.0, self.me.1),
            format!(
                "{}|{}",
                self.them_owner,
                self.them.as_deref().unwrap_or_default()
            ),
        );
        let (init, resp) = if self.ours { (me, them) } else { (them, me) };
        format!("{label}|{init}|{ik}|{resp}|{rk}|{}", self.tx)
    }

    /// For the MAC we send (`outgoing`) or the one they sent.
    fn mac_info(&self, outgoing: bool) -> String {
        let me = format!("{}|{}", self.me.0, self.me.1);
        let them = format!(
            "{}|{}",
            self.them_owner,
            self.them.as_deref().unwrap_or_default()
        );
        let (from, to) = if outgoing { (me, them) } else { (them, me) };
        format!("CHAD_MAC|{from}|{to}|{}", self.tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Olm, Device, Olm, Device) {
        let (a, b) = (Olm::new(), Olm::new());
        let da = Device::parse("alice", "a1", &a.record("alice", "a1")).unwrap();
        let db = Device::parse("bob", "b1", &b.record("bob", "b1")).unwrap();
        (a, da, b, db)
    }

    #[test]
    fn records_are_bound_to_owner_and_device() {
        let a = Olm::new();
        let rec = a.record("alice", "a1");
        assert!(
            rec.len() + "chad/e2e/d/".len() + 32 <= 350,
            "fits ergo's metadata limit"
        );
        assert_eq!(Device::parse("alice", "a1", &rec).unwrap(), a.device("a1"));
        assert!(Device::parse("mallory", "a1", &rec).is_none());
        assert!(Device::parse("alice", "a2", &rec).is_none());
        assert!(Device::parse("alice", "a1", &rec.replace(' ', "  ")).is_none());
        assert!(Device::parse("alice", "A1", &a.record("alice", "A1")).is_none());
    }

    #[test]
    fn round_trip_both_ways_and_rejects_tampering_and_replay() {
        let (mut a, da, mut b, db) = pair();
        let m1 = a.encrypt(&db, "hi bob").unwrap();
        assert!(m1.starts_with("0:"), "pre-key");
        let m2 = a.encrypt(&db, "still there?").unwrap();
        // Out of order is fine.
        assert_eq!(b.decrypt(da.curve, &m2).as_deref(), Some("still there?"));
        assert_eq!(b.decrypt(da.curve, &m1).as_deref(), Some("hi bob"));
        assert!(b.decrypt(da.curve, &m1).is_none(), "replay");

        let reply = b.encrypt(&da, "hey").unwrap();
        assert!(reply.starts_with("1:"), "b answers on the inbound session");
        assert_eq!(a.decrypt(db.curve, &reply).as_deref(), Some("hey"));

        let mut bad = a.encrypt(&db, "x").unwrap().into_bytes();
        let n = bad.len() - 5;
        bad[n] = if bad[n] == b'A' { b'B' } else { b'A' };
        assert!(
            b.decrypt(da.curve, std::str::from_utf8(&bad).unwrap())
                .is_none()
        );
        assert!(
            b.decrypt(db.curve, &a.encrypt(&db, "wrong sender").unwrap())
                .is_none()
        );
        assert!(b.decrypt(da.curve, "garbage").is_none());
    }

    #[test]
    fn pickles_survive_and_fallback_rotates() {
        let (mut a, _, mut b, db) = pair();
        let key = new_pickle_key();
        let m = a.encrypt(&db, "one").unwrap();
        let da = a.device("a1");
        b.decrypt(da.curve, &m).unwrap();
        let ch = b.take_changes(&key);
        let account = ch.account.unwrap();
        let sessions: Vec<_> = ch.sessions.into_iter().map(|(_, p, s)| (p, s)).collect();
        let mut b = Olm::load(&account, &sessions, &key).unwrap();
        assert!(Olm::load(&account, &sessions, &new_pickle_key()).is_none());
        let m = a.encrypt(&db, "two").unwrap();
        assert_eq!(b.decrypt(da.curve, &m).as_deref(), Some("two"));

        // A message to the previous fallback key still opens after one rotation.
        let old = b.device("b1");
        b.rotate();
        assert_ne!(b.device("b1").fallback, old.fallback);
        let mut c = Olm::new();
        let m = c.encrypt(&old, "late").unwrap();
        assert_eq!(b.decrypt(c.device("c1").curve, &m).as_deref(), Some("late"));
    }

    #[test]
    fn sas_end_to_end() {
        let (a, da, b, db) = pair();
        let (me_a, me_b) = (
            ("alice".to_owned(), "a1".to_owned()),
            ("bob".to_owned(), "b1".to_owned()),
        );
        let (mut va, req) = Verification::start(me_a, "bob".into());
        assert_eq!(Step::parse(&req.encode()).unwrap(), req);
        let mut vb = Verification::incoming(me_b, "alice".into(), &req);
        assert_eq!(vb.status, Status::Incoming);
        let accept = vb.accept().unwrap();
        let key_a = va.handle(&accept, Some(db.ed)).unwrap();
        let key_b = vb.handle(&key_a, Some(da.ed)).unwrap();
        assert!(va.handle(&key_b, Some(db.ed)).is_none());
        let (Status::Compare(ea), Status::Compare(eb)) = (va.status.clone(), vb.status.clone())
        else {
            panic!("{:?} {:?}", va.status, vb.status);
        };
        assert_eq!(ea, eb);
        let mac_a = va.confirm(&a, Some(db.ed)).unwrap();
        vb.handle(&mac_a, Some(da.ed));
        assert_eq!(
            vb.status,
            Status::Compare(eb),
            "waits for bob's own confirmation"
        );
        let mac_b = vb.confirm(&b, Some(da.ed)).unwrap();
        assert_eq!(vb.status, Status::Done);
        va.handle(&mac_b, Some(db.ed));
        assert_eq!(va.status, Status::Done);
    }

    #[test]
    fn sas_catches_a_swapped_device_key() {
        let (a, da, b, db) = pair();
        let mallory = Olm::new().device("b1");
        let (mut va, req) = Verification::start(("alice".into(), "a1".into()), "bob".into());
        let mut vb = Verification::incoming(("bob".into(), "b1".into()), "alice".into(), &req);
        let key_a = va.handle(&vb.accept().unwrap(), None).unwrap();
        let key_b = vb.handle(&key_a, None).unwrap();
        va.handle(&key_b, None);
        let mac_b = vb.confirm(&b, Some(da.ed)).unwrap();
        va.confirm(&a, Some(mallory.ed));
        va.handle(&mac_b, Some(mallory.ed));
        assert!(matches!(va.status, Status::Failed(_)));
        let _ = db;
    }

    #[test]
    fn sas_commitment_is_checked() {
        let (mut va, req) = Verification::start(("alice".into(), "a1".into()), "bob".into());
        let mut vb = Verification::incoming(("bob".into(), "b1".into()), "alice".into(), &req);
        va.handle(&vb.accept().unwrap(), None).unwrap();
        let mut forged = vb.step(Kind::Key(Sas::new().public_key().to_base64()));
        forged.to = Some("a1".into());
        assert!(va.handle(&forged, None).is_none());
        assert!(matches!(va.status, Status::Failed(_)));
    }

    #[test]
    fn steps_parse_strictly() {
        for bad in [
            "",
            "t a - request x",
            "t a - nope",
            "T a - request",
            "t a b key",
            "t a b key x y",
        ] {
            assert!(Step::parse(bad).is_none(), "{bad:?}");
        }
        assert_eq!(
            Step::parse("t a b key k").unwrap().kind,
            Kind::Key("k".into())
        );
    }
}
