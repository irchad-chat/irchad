//! One SQLite file per network: the last scrollback, buffers and read markers, so
//! startup shows something before the connection is up. The server holds the
//! history of record (chathistory), so a failed write is logged and forgotten.
//! The exceptions are E2EE (Phase 4): decrypted DMs, which no server can give
//! back, and this device's Olm account and sessions, encrypted with a key from
//! the OS secret store.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension as _, Params, Row, params};

use chad_e2ee::{Changes, Device};

use crate::{
    Block, BufferKind, ChatMessage, Crypt, Mention, MessageKind, Reaction, Seen, SendStatus,
    mention,
};

/// Messages kept per buffer between runs.
const KEEP: u32 = 2000;
/// Per message, so a hostile server can't make one row unbounded (D14).
const MAX_REACTIONS: i64 = 200;

pub struct Store {
    db: Connection,
}

/// Someone's device as first seen (trust on first use).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub device: Device,
    pub verified: bool,
    /// Its keys changed since it was pinned, and nobody has said that's fine.
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredBuffer {
    pub key: String,
    pub name: String,
    pub kind: BufferKind,
    pub topic: String,
    pub read: Option<i64>,
    pub seen: Option<Seen>,
}

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS buffers (
    key TEXT PRIMARY KEY, name TEXT NOT NULL, kind INTEGER NOT NULL,
    topic TEXT NOT NULL DEFAULT '', read INTEGER);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY, buffer TEXT NOT NULL, msgid TEXT UNIQUE, time INTEGER NOT NULL,
    nick TEXT NOT NULL, account TEXT, kind INTEGER NOT NULL, text TEXT NOT NULL,
    reply_to TEXT, redacted INTEGER NOT NULL DEFAULT 0,
    mention INTEGER NOT NULL, own INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS messages_buffer_time ON messages (buffer, time);
CREATE TABLE IF NOT EXISTS reactions (
    msgid TEXT NOT NULL, nick TEXT NOT NULL, emoji TEXT NOT NULL,
    PRIMARY KEY (msgid, nick, emoji));
CREATE TABLE IF NOT EXISTS e2e_sessions (
    id TEXT PRIMARY KEY, peer TEXT NOT NULL, pickle TEXT NOT NULL, used INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS e2e_pins (
    nick TEXT NOT NULL, id TEXT NOT NULL, record TEXT NOT NULL,
    verified INTEGER NOT NULL DEFAULT 0, changed INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (nick, id));
CREATE TABLE IF NOT EXISTS blocks (mask TEXT PRIMARY KEY, nick TEXT NOT NULL);
";

/// Run in order on older files; `PRAGMA user_version` counts how many have.
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE messages ADD COLUMN crypt INTEGER NOT NULL DEFAULT 0",
    // The time of the edit that set `text`, 0 if never edited.
    "ALTER TABLE messages ADD COLUMN edited INTEGER NOT NULL DEFAULT 0",
    // `+chad/mentions` as sent (D66).
    "ALTER TABLE messages ADD COLUMN mentions TEXT NOT NULL DEFAULT ''",
    // A DM's read receipt (D68): read up to `seen` (a message's time), as of `seen_at`.
    "ALTER TABLE buffers ADD COLUMN seen INTEGER",
    "ALTER TABLE buffers ADD COLUMN seen_at INTEGER",
];

fn ok<T: Default>(what: &str, r: rusqlite::Result<T>) -> T {
    r.unwrap_or_else(|e| {
        tracing::warn!("cache {what}: {e}");
        T::default()
    })
}

const MESSAGE_COLUMNS: &str = "msgid, time, nick, account, kind, text, reply_to, redacted, mention, own, crypt, edited, mentions";

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::init(Connection::open(path)?)
    }

    #[cfg(test)]
    pub fn memory() -> Self {
        Self::init(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn init(db: Connection) -> rusqlite::Result<Self> {
        db.execute_batch(SCHEMA)?;
        let done: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, m) in MIGRATIONS.iter().enumerate().skip(done as usize) {
            db.execute_batch(&format!("{m}; PRAGMA user_version = {}", i + 1))?;
        }
        db.execute(
            "DELETE FROM messages WHERE id IN (SELECT id FROM (SELECT id, row_number() OVER
             (PARTITION BY buffer ORDER BY time DESC) AS n FROM messages) WHERE n > ?1)",
            [KEEP],
        )?;
        db.execute(
            "DELETE FROM reactions WHERE msgid NOT IN (SELECT msgid FROM messages WHERE msgid IS NOT NULL)",
            [],
        )?;
        Ok(Self { db })
    }

    /// Runs a statement, prepared once per connection.
    fn exec(&self, sql: &str, p: impl Params) -> rusqlite::Result<usize> {
        self.db.prepare_cached(sql)?.execute(p)
    }

    fn one<T>(
        &self,
        sql: &str,
        p: impl Params,
        f: impl FnOnce(&Row) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<Option<T>> {
        self.db.prepare_cached(sql)?.query_row(p, f).optional()
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        ok(
            "read meta",
            self.one("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)),
        )
    }

    fn try_set_meta(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.exec("INSERT OR REPLACE INTO meta VALUES (?1, ?2)", [key, value])
            .map(drop)
    }

    pub fn set_meta(&self, key: &str, value: &str) {
        ok("write meta", self.try_set_meta(key, value));
    }

    pub fn delete_meta(&self, key: &str) {
        ok(
            "delete meta",
            self.exec("DELETE FROM meta WHERE key = ?1", [key])
                .map(drop),
        );
    }

    pub fn blocks(&self) -> Vec<Block> {
        let read = || {
            let mut st = self
                .db
                .prepare("SELECT mask, nick FROM blocks ORDER BY rowid")?;
            st.query_map([], |r| {
                Ok(Block {
                    mask: r.get(0)?,
                    nick: r.get(1)?,
                })
            })?
            .collect()
        };
        ok("read blocks", read())
    }

    pub fn put_block(&self, b: &Block) {
        ok(
            "write block",
            self.exec(
                "INSERT OR REPLACE INTO blocks VALUES (?1, ?2)",
                [&b.mask, &b.nick],
            )
            .map(drop),
        );
    }

    pub fn remove_block(&self, mask: &str) {
        ok(
            "delete block",
            self.exec("DELETE FROM blocks WHERE mask = ?1", [mask])
                .map(drop),
        );
    }

    pub fn buffers(&self) -> Vec<StoredBuffer> {
        let read = || {
            let mut st = self
                .db
                .prepare("SELECT key, name, kind, topic, read, seen, seen_at FROM buffers")?;
            st.query_map([], |r| {
                Ok(StoredBuffer {
                    key: r.get(0)?,
                    name: r.get(1)?,
                    kind: if r.get::<_, i64>(2)? == 1 {
                        BufferKind::Dm
                    } else {
                        BufferKind::Channel
                    },
                    topic: r.get(3)?,
                    read: r.get(4)?,
                    seen: match (r.get(5)?, r.get(6)?) {
                        (Some(upto), Some(at)) => Some(Seen { upto, at }),
                        _ => None,
                    },
                })
            })?
            .collect()
        };
        ok("read buffers", read())
    }

    pub fn put_buffer(&self, key: &str, name: &str, kind: BufferKind) {
        let kind = i64::from(kind == BufferKind::Dm);
        ok(
            "write buffer",
            self.exec(
                "INSERT INTO buffers (key, name, kind) VALUES (?1, ?2, ?3)
                 ON CONFLICT (key) DO UPDATE SET name = ?2",
                params![key, name, kind],
            )
            .map(drop),
        );
    }

    pub fn set_topic(&self, key: &str, topic: &str) {
        ok(
            "write topic",
            self.exec("UPDATE buffers SET topic = ?2 WHERE key = ?1", [key, topic])
                .map(drop),
        );
    }

    pub fn set_read(&self, key: &str, time: i64) {
        ok(
            "write read marker",
            self.exec(
                "UPDATE buffers SET read = ?2 WHERE key = ?1",
                params![key, time],
            )
            .map(drop),
        );
    }

    pub fn set_seen(&self, key: &str, seen: Option<Seen>) {
        let (upto, at) = (seen.map(|s| s.upto), seen.map(|s| s.at));
        ok(
            "write read receipt",
            self.exec(
                "UPDATE buffers SET seen = ?2, seen_at = ?3 WHERE key = ?1",
                params![key, upto, at],
            )
            .map(drop),
        );
    }

    pub fn remove_buffer(&self, key: &str) {
        let r = self.atomic(|| {
            self.exec(
                "DELETE FROM reactions WHERE msgid IN (SELECT msgid FROM messages WHERE buffer = ?1)",
                [key],
            )?;
            self.exec("DELETE FROM messages WHERE buffer = ?1", [key])?;
            self.exec("DELETE FROM buffers WHERE key = ?1", [key])
        });
        ok("remove buffer", r.map(drop));
    }

    /// `draft/channel-rename`: the buffer and its scrollback move to `new`.
    pub fn rename_buffer(&self, old: &str, new: &str, name: &str) {
        let r = self.atomic(|| {
            self.exec(
                "UPDATE buffers SET key = ?2, name = ?3 WHERE key = ?1",
                [old, new, name],
            )?;
            self.exec(
                "UPDATE messages SET buffer = ?2 WHERE buffer = ?1",
                [old, new],
            )
        });
        ok("rename buffer", r.map(drop));
    }

    /// False if a message with this msgid is already stored.
    pub fn insert(&self, buffer: &str, m: &ChatMessage) -> bool {
        let kind = match m.kind {
            MessageKind::Privmsg => 0,
            MessageKind::Notice => 1,
            MessageKind::Action => 2,
        };
        let crypt = match m.crypt {
            Crypt::Plain => 0,
            Crypt::Encrypted => 1,
            Crypt::Failed => 2,
        };
        ok(
            "write message",
            self.exec(
                "INSERT OR IGNORE INTO messages
                 (buffer, msgid, time, nick, account, kind, text, reply_to, redacted, mention, own, crypt, mentions)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    buffer, m.msgid, m.time, m.nick, m.account, kind, m.text, m.reply_to,
                    m.redacted, m.mention, m.own, crypt, mention::encode(&m.mentions)
                ],
            )
            .map(|n| n == 1),
        )
    }

    /// All of `f` or none of it, nested inside any transaction already open.
    fn atomic<T>(&self, f: impl FnOnce() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        self.db.execute_batch("SAVEPOINT atomic")?;
        match f() {
            Ok(v) => self.release().map(|()| v),
            Err(e) => {
                self.rollback();
                Err(e)
            }
        }
    }

    /// Never leaves the savepoint open: a failed commit rolls back.
    fn release(&self) -> rusqlite::Result<()> {
        self.db
            .execute_batch("RELEASE atomic")
            .inspect_err(|_| self.rollback())
    }

    fn rollback(&self) {
        let _ = self.db.execute_batch("ROLLBACK TO atomic; RELEASE atomic");
    }

    /// Starts many writes in one transaction (nested in any open one), for
    /// when `batch` can't hold the store across them. False if it couldn't;
    /// pass that to `commit`.
    pub fn begin(&self) -> bool {
        ok(
            "begin",
            self.db.execute_batch("SAVEPOINT atomic").map(|()| true),
        )
    }

    /// Ends `begin`'s transaction; each failed write was logged, the rest kept.
    pub fn commit(&self, began: bool) {
        if began {
            ok("commit", self.release());
        }
    }

    /// `f`'s writes in one transaction.
    #[cfg(test)]
    pub fn batch<T>(&self, f: impl FnOnce() -> T) -> T {
        let began = self.begin();
        let out = f();
        self.commit(began);
        out
    }

    /// The newest `limit` messages of a buffer, oldest first.
    pub fn recent(&self, buffer: &str, limit: u32) -> Vec<ChatMessage> {
        let read = || {
            let mut st = self.db.prepare_cached(&format!(
                "SELECT * FROM (SELECT {MESSAGE_COLUMNS}
                 FROM messages WHERE buffer = ?1 ORDER BY time DESC, id DESC LIMIT ?2) ORDER BY time, msgid"
            ))?;
            st.query_map(params![buffer, limit], row)?.collect()
        };
        let mut out: Vec<ChatMessage> = ok("read messages", read());
        // One query for all their reactions, not one per message.
        let read = || {
            let mut st = self.db.prepare_cached(
                "SELECT r.msgid, r.emoji, r.nick FROM reactions r WHERE r.msgid IN
                 (SELECT msgid FROM messages WHERE buffer = ?1 AND msgid IS NOT NULL
                  ORDER BY time DESC, id DESC LIMIT ?2) ORDER BY r.rowid",
            )?;
            st.query_map(params![buffer, limit], |r| {
                Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<Vec<(String, String, String)>>>()
        };
        let mut by_msg: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (msgid, emoji, nick) in ok("read reactions", read()) {
            by_msg.entry(msgid).or_default().push((emoji, nick));
        }
        for m in &mut out {
            if let Some(rows) = m.msgid.as_deref().and_then(|id| by_msg.remove(id)) {
                m.reactions = group(rows);
            }
        }
        out
    }

    /// `(buffer, message)`.
    pub fn get(&self, msgid: &str) -> Option<(String, ChatMessage)> {
        let found = ok(
            "read message",
            self.one(
                &format!("SELECT {MESSAGE_COLUMNS}, buffer FROM messages WHERE msgid = ?1"),
                [msgid],
                |r| Ok((r.get(13)?, row(r)?)),
            ),
        );
        found.map(|(b, mut m): (String, ChatMessage)| {
            m.reactions = self.reactions(msgid);
            (b, m)
        })
    }

    pub fn redact(&self, msgid: &str) {
        ok(
            "redact",
            self.exec(
                "UPDATE messages SET redacted = 1, text = '' WHERE msgid = ?1",
                [msgid],
            )
            .map(drop),
        );
    }

    /// Replaces a message's text and mentions, unless a later edit already
    /// did. True if it did.
    pub fn edit(&self, msgid: &str, text: &str, mentions: &[Mention], time: i64) -> bool {
        ok(
            "edit",
            self.exec(
                "UPDATE messages SET text = ?2, edited = ?3, mentions = ?4
                 WHERE msgid = ?1 AND redacted = 0 AND edited < ?3",
                params![msgid, text, time, mention::encode(mentions)],
            )
            .map(|n| n == 1),
        )
    }

    pub fn react(&self, msgid: &str, nick: &str, emoji: &str, add: bool) {
        let r = if add {
            // The cap is checked in the same statement, so a full message takes no more.
            self.exec(
                "INSERT OR IGNORE INTO reactions SELECT ?1, ?2, ?3
                 WHERE (SELECT count(*) FROM reactions WHERE msgid = ?1) < ?4",
                params![msgid, nick, emoji, MAX_REACTIONS],
            )
        } else {
            self.exec(
                "DELETE FROM reactions WHERE msgid = ?1 AND nick = ?2 AND emoji = ?3",
                [msgid, nick, emoji],
            )
        };
        ok("write reaction", r.map(drop));
    }

    fn reactions(&self, msgid: &str) -> Vec<Reaction> {
        let read = || {
            let mut st = self.db.prepare_cached(
                "SELECT emoji, nick FROM reactions WHERE msgid = ?1 ORDER BY rowid",
            )?;
            st.query_map([msgid], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<(String, String)>>>()
        };
        group(ok("read reactions", read()))
    }

    /// `(unread, mentions)` from other people after the read marker.
    pub fn counts(&self, buffer: &str, read: Option<i64>) -> (u32, u32) {
        ok(
            "count unread",
            self.one(
                "SELECT count(*), coalesce(sum(mention), 0) FROM messages
                 WHERE buffer = ?1 AND own = 0 AND redacted = 0 AND time > ?2",
                params![buffer, read.unwrap_or(i64::MIN)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map(Option::unwrap_or_default),
        )
    }

    /// `(msgid, time)` of the newest message with an id.
    pub fn newest(&self, buffer: &str) -> Option<(String, i64)> {
        self.edge(
            "SELECT msgid, time FROM messages WHERE buffer = ?1 AND msgid IS NOT NULL
             ORDER BY time DESC LIMIT 1",
            buffer,
        )
    }

    pub fn oldest(&self, buffer: &str) -> Option<(String, i64)> {
        self.edge(
            "SELECT msgid, time FROM messages WHERE buffer = ?1 AND msgid IS NOT NULL
             ORDER BY time ASC LIMIT 1",
            buffer,
        )
    }

    fn edge(&self, sql: &str, buffer: &str) -> Option<(String, i64)> {
        ok(
            "read edge",
            self.one(sql, [buffer], |r| Ok((r.get(0)?, r.get(1)?))),
        )
    }

    /// Newest message time across all buffers: where `CHATHISTORY TARGETS` starts.
    pub fn latest_time(&self) -> Option<i64> {
        ok(
            "read latest",
            self.one("SELECT max(time) FROM messages", [], |r| r.get(0))
                .map(Option::flatten),
        )
    }

    /// `(their key, pickle)` of every Olm session, least recently used first.
    pub fn e2e_sessions(&self) -> Vec<(String, String)> {
        let read = || {
            let mut st = self
                .db
                .prepare("SELECT peer, pickle FROM e2e_sessions ORDER BY used")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
        };
        ok("read sessions", read())
    }

    /// All or nothing: Olm state that no server can give back.
    pub fn save_e2e(&self, c: &Changes) -> rusqlite::Result<()> {
        let now = crate::time::now_ms();
        self.atomic(|| {
            if let Some(a) = &c.account {
                self.try_set_meta("e2e_account", a)?;
            }
            for (id, peer, pickle) in &c.sessions {
                self.exec(
                    "INSERT OR REPLACE INTO e2e_sessions VALUES (?1, ?2, ?3, ?4)",
                    params![id, peer, pickle, now],
                )?;
            }
            for id in &c.removed {
                self.exec("DELETE FROM e2e_sessions WHERE id = ?1", [id])?;
            }
            Ok(())
        })
    }

    /// Everyone's pinned devices, under their folded nick.
    pub fn pins(&self, nick: &str) -> Vec<Pin> {
        let read = || {
            let mut st = self.db.prepare_cached(
                "SELECT id, record, verified, changed FROM e2e_pins WHERE nick = ?1",
            )?;
            st.query_map([nick], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get(2)?,
                    r.get(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        };
        ok("read pins", read())
            .into_iter()
            .filter_map(|(id, record, verified, changed)| {
                Some(Pin {
                    device: Device::parse(nick, &id, &record)?,
                    verified,
                    changed,
                })
            })
            .collect()
    }

    pub fn put_pin(
        &self,
        nick: &str,
        id: &str,
        record: &str,
        verified: bool,
        changed: bool,
    ) -> rusqlite::Result<()> {
        self.exec(
            "INSERT OR REPLACE INTO e2e_pins VALUES (?1, ?2, ?3, ?4, ?5)",
            params![nick, id, record, verified, changed],
        )
        .map(drop)
    }

    /// Someone's device was verified, or (`id` = `None`) all their key changes accepted.
    pub fn trust(&self, nick: &str, id: Option<&str>) -> rusqlite::Result<()> {
        match id {
            Some(id) => self.exec(
                "UPDATE e2e_pins SET verified = 1, changed = 0 WHERE nick = ?1 AND id = ?2",
                [nick, id],
            ),
            None => self.exec("UPDATE e2e_pins SET changed = 0 WHERE nick = ?1", [nick]),
        }
        .map(drop)
    }
}

/// `(emoji, nick)` rows, in order, as one reaction per emoji.
fn group(rows: Vec<(String, String)>) -> Vec<Reaction> {
    let mut out: Vec<Reaction> = Vec::new();
    for (emoji, nick) in rows {
        match out.iter_mut().find(|r| r.emoji == emoji) {
            Some(r) => r.nicks.push(nick),
            None => out.push(Reaction {
                emoji,
                nicks: vec![nick],
            }),
        }
    }
    out
}

fn row(r: &Row) -> rusqlite::Result<ChatMessage> {
    let text: String = r.get(5)?;
    Ok(ChatMessage {
        mentions: mention::decode(&r.get::<_, String>(12)?, &text),
        msgid: r.get(0)?,
        local_id: None,
        time: r.get(1)?,
        nick: r.get(2)?,
        account: r.get(3)?,
        kind: match r.get::<_, i64>(4)? {
            1 => MessageKind::Notice,
            2 => MessageKind::Action,
            _ => MessageKind::Privmsg,
        },
        text,
        reply_to: r.get(6)?,
        edits: None,
        edited: r.get::<_, i64>(11)? > 0,
        status: SendStatus::Sent,
        redacted: r.get(7)?,
        reactions: Vec::new(),
        mention: r.get(8)?,
        own: r.get(9)?,
        crypt: match r.get::<_, i64>(10)? {
            1 => Crypt::Encrypted,
            2 => Crypt::Failed,
            _ => Crypt::Plain,
        },
    })
}

#[cfg(test)]
pub(crate) fn msg(msgid: &str, time: i64, nick: &str, text: &str) -> ChatMessage {
    ChatMessage {
        msgid: Some(msgid.into()),
        time,
        nick: nick.into(),
        text: text.into(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_dedupe_count_and_react() {
        let s = Store::memory();
        s.put_buffer("#a", "#A", BufferKind::Channel);
        assert!(s.insert("#a", &msg("m1", 10, "bob", "hi")));
        assert!(!s.insert("#a", &msg("m1", 10, "bob", "hi")));
        let mut own = msg("m2", 20, "me", "yo");
        own.own = true;
        s.insert("#a", &own);
        let mut hl = msg("m3", 30, "bob", "me!");
        hl.mention = true;
        s.insert("#a", &hl);

        assert_eq!(s.counts("#a", None), (2, 1));
        assert_eq!(s.counts("#a", Some(10)), (1, 1));
        s.set_read("#a", 30);
        assert_eq!(s.buffers()[0].read, Some(30));
        assert_eq!(s.newest("#a"), Some(("m3".into(), 30)));
        assert_eq!(s.oldest("#a"), Some(("m1".into(), 10)));
        assert_eq!(s.latest_time(), Some(30));

        s.react("m1", "me", "👍", true);
        s.react("m1", "carol", "👍", true);
        s.redact("m3");
        let r = s.recent("#a", 2);
        assert_eq!(r.len(), 2);
        assert!(r[1].redacted && r[1].text.is_empty());
        assert_eq!(s.get("m1").unwrap().1.reactions[0].nicks, ["me", "carol"]);
        assert_eq!(s.get("m1").unwrap().0, "#a");

        s.remove_buffer("#a");
        assert!(s.buffers().is_empty() && s.recent("#a", 10).is_empty());
    }

    #[test]
    fn transactions_never_stay_open() {
        let s = Store::memory();
        s.batch(|| {
            s.batch(|| s.insert("#a", &msg("m1", 1, "bob", "x")));
            s.remove_buffer("#a");
        });
        assert!(s.db.is_autocommit());
        assert!(s.recent("#a", 10).is_empty());
        // A failing E2EE save rolls back whole and leaves no transaction open.
        s.db.execute_batch("DROP TABLE e2e_sessions").unwrap();
        let c = Changes {
            account: Some("acct".into()),
            sessions: vec![("id".into(), "peer".into(), "p".into())],
            removed: vec![],
        };
        assert!(s.save_e2e(&c).is_err());
        assert!(s.db.is_autocommit());
        assert_eq!(s.meta("e2e_account"), None);
    }

    #[test]
    fn recent_carries_reactions() {
        let s = Store::memory();
        for i in 0..3 {
            s.insert("#a", &msg(&format!("m{i}"), i, "bob", "x"));
        }
        s.react("m2", "bob", "👍", true);
        s.react("m2", "amy", "👍", true);
        s.react("m0", "amy", "x", true);
        let r = s.recent("#a", 2);
        assert!(r[0].reactions.is_empty());
        assert_eq!(r[1].reactions[0].nicks, ["bob", "amy"]);
    }

    #[test]
    fn reopening_prunes_to_keep() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("n.sqlite");
        let s = Store::open(&path).unwrap();
        s.batch(|| {
            for i in 0..KEEP + 5 {
                s.insert("#a", &msg(&format!("m{i}"), i.into(), "bob", "x"));
            }
        });
        s.react("m0", "bob", "x", true);
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.recent("#a", KEEP + 10).len(), KEEP as usize);
        assert_eq!(s.oldest("#a").unwrap().0, "m5");
    }
}
