//! One SQLite file per network: the last scrollback, buffers and read markers, so
//! startup shows something before the connection is up. The server holds the
//! history of record (chathistory), so a failed write is logged and forgotten.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension as _, Row, params};

use crate::{BufferKind, ChatMessage, MessageKind, Reaction, SendStatus};

/// Messages kept per buffer between runs.
const KEEP: u32 = 2000;
/// Per message, so a hostile server can't make one row unbounded (D14).
const MAX_REACTIONS: i64 = 200;

pub struct Store {
    db: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredBuffer {
    pub key: String,
    pub name: String,
    pub kind: BufferKind,
    pub topic: String,
    pub read: Option<i64>,
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
";

fn ok<T: Default>(what: &str, r: rusqlite::Result<T>) -> T {
    r.unwrap_or_else(|e| {
        tracing::warn!("cache {what}: {e}");
        T::default()
    })
}

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

    pub fn meta(&self, key: &str) -> Option<String> {
        ok(
            "read meta",
            self.db
                .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
                .optional(),
        )
    }

    pub fn set_meta(&self, key: &str, value: &str) {
        ok(
            "write meta",
            self.db
                .execute("INSERT OR REPLACE INTO meta VALUES (?1, ?2)", [key, value])
                .map(drop),
        );
    }

    pub fn buffers(&self) -> Vec<StoredBuffer> {
        let read = || {
            let mut st = self
                .db
                .prepare("SELECT key, name, kind, topic, read FROM buffers")?;
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
            self.db
                .execute(
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
            self.db
                .execute("UPDATE buffers SET topic = ?2 WHERE key = ?1", [key, topic])
                .map(drop),
        );
    }

    pub fn set_read(&self, key: &str, time: i64) {
        ok(
            "write read marker",
            self.db
                .execute(
                    "UPDATE buffers SET read = ?2 WHERE key = ?1",
                    params![key, time],
                )
                .map(drop),
        );
    }

    pub fn remove_buffer(&self, key: &str) {
        ok(
            "remove buffer",
            self.db
                .execute_batch("BEGIN")
                .and_then(|()| {
                    self.db.execute(
                        "DELETE FROM reactions WHERE msgid IN (SELECT msgid FROM messages WHERE buffer = ?1)",
                        [key],
                    )?;
                    self.db.execute("DELETE FROM messages WHERE buffer = ?1", [key])?;
                    self.db.execute("DELETE FROM buffers WHERE key = ?1", [key])?;
                    self.db.execute_batch("COMMIT")
                }),
        );
    }

    /// `draft/channel-rename`: the buffer and its scrollback move to `new`.
    pub fn rename_buffer(&self, old: &str, new: &str, name: &str) {
        ok(
            "rename buffer",
            self.db.execute_batch("BEGIN").and_then(|()| {
                self.db.execute(
                    "UPDATE buffers SET key = ?2, name = ?3 WHERE key = ?1",
                    [old, new, name],
                )?;
                self.db.execute(
                    "UPDATE messages SET buffer = ?2 WHERE buffer = ?1",
                    [old, new],
                )?;
                self.db.execute_batch("COMMIT")
            }),
        );
    }

    /// False if a message with this msgid is already stored.
    pub fn insert(&self, buffer: &str, m: &ChatMessage) -> bool {
        let kind = match m.kind {
            MessageKind::Privmsg => 0,
            MessageKind::Notice => 1,
            MessageKind::Action => 2,
        };
        ok(
            "write message",
            self.db
                .execute(
                    "INSERT OR IGNORE INTO messages
                     (buffer, msgid, time, nick, account, kind, text, reply_to, redacted, mention, own)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        buffer, m.msgid, m.time, m.nick, m.account, kind, m.text, m.reply_to,
                        m.redacted, m.mention, m.own
                    ],
                )
                .map(|n| n == 1),
        )
    }

    /// Wraps many inserts in one transaction.
    pub fn batch<T>(&self, f: impl FnOnce() -> T) -> T {
        let began = self.db.execute_batch("BEGIN").is_ok();
        let out = f();
        if began {
            ok("commit", self.db.execute_batch("COMMIT"));
        }
        out
    }

    /// The newest `limit` messages of a buffer, oldest first.
    pub fn recent(&self, buffer: &str, limit: u32) -> Vec<ChatMessage> {
        let read = || {
            let mut st = self.db.prepare(
                "SELECT * FROM (SELECT msgid, time, nick, account, kind, text, reply_to, redacted, mention, own
                 FROM messages WHERE buffer = ?1 ORDER BY time DESC, id DESC LIMIT ?2) ORDER BY time, msgid",
            )?;
            st.query_map(params![buffer, limit], row)?.collect()
        };
        let mut out: Vec<ChatMessage> = ok("read messages", read());
        for m in &mut out {
            m.reactions = self.reactions(m.msgid.as_deref().unwrap_or_default());
        }
        out
    }

    /// `(buffer, message)`.
    pub fn get(&self, msgid: &str) -> Option<(String, ChatMessage)> {
        let found = ok(
            "read message",
            self.db
                .query_row(
                    "SELECT msgid, time, nick, account, kind, text, reply_to, redacted, mention, own, buffer
                     FROM messages WHERE msgid = ?1",
                    [msgid],
                    |r| Ok((r.get(10)?, row(r)?)),
                )
                .optional(),
        );
        found.map(|(b, mut m): (String, ChatMessage)| {
            m.reactions = self.reactions(msgid);
            (b, m)
        })
    }

    pub fn redact(&self, msgid: &str) {
        ok(
            "redact",
            self.db
                .execute(
                    "UPDATE messages SET redacted = 1, text = '' WHERE msgid = ?1",
                    [msgid],
                )
                .map(drop),
        );
    }

    pub fn react(&self, msgid: &str, nick: &str, emoji: &str, add: bool) {
        if !add {
            ok(
                "remove reaction",
                self.db
                    .execute(
                        "DELETE FROM reactions WHERE msgid = ?1 AND nick = ?2 AND emoji = ?3",
                        [msgid, nick, emoji],
                    )
                    .map(drop),
            );
            return;
        }
        let count: i64 = ok(
            "count reactions",
            self.db.query_row(
                "SELECT count(*) FROM reactions WHERE msgid = ?1",
                [msgid],
                |r| r.get(0),
            ),
        );
        if count < MAX_REACTIONS {
            ok(
                "write reaction",
                self.db
                    .execute(
                        "INSERT OR IGNORE INTO reactions VALUES (?1, ?2, ?3)",
                        [msgid, nick, emoji],
                    )
                    .map(drop),
            );
        }
    }

    fn reactions(&self, msgid: &str) -> Vec<Reaction> {
        let read = || {
            let mut st = self
                .db
                .prepare("SELECT emoji, nick FROM reactions WHERE msgid = ?1 ORDER BY rowid")?;
            st.query_map([msgid], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<(String, String)>>>()
        };
        let mut out: Vec<Reaction> = Vec::new();
        for (emoji, nick) in ok("read reactions", read()) {
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

    /// `(unread, mentions)` from other people after the read marker.
    pub fn counts(&self, buffer: &str, read: Option<i64>) -> (u32, u32) {
        ok(
            "count unread",
            self.db.query_row(
                "SELECT count(*), coalesce(sum(mention), 0) FROM messages
                 WHERE buffer = ?1 AND own = 0 AND redacted = 0 AND time > ?2",
                params![buffer, read.unwrap_or(i64::MIN)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ),
        )
    }

    /// `(msgid, time)` of the newest message with an id.
    pub fn newest(&self, buffer: &str) -> Option<(String, i64)> {
        self.edge(buffer, "DESC")
    }

    pub fn oldest(&self, buffer: &str) -> Option<(String, i64)> {
        self.edge(buffer, "ASC")
    }

    fn edge(&self, buffer: &str, order: &str) -> Option<(String, i64)> {
        ok(
            "read edge",
            self.db
                .query_row(
                    &format!(
                        "SELECT msgid, time FROM messages WHERE buffer = ?1 AND msgid IS NOT NULL
                         ORDER BY time {order} LIMIT 1"
                    ),
                    [buffer],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional(),
        )
    }

    /// Newest message time across all buffers: where `CHATHISTORY TARGETS` starts.
    pub fn latest_time(&self) -> Option<i64> {
        ok(
            "read latest",
            self.db
                .query_row("SELECT max(time) FROM messages", [], |r| r.get(0)),
        )
    }
}

fn row(r: &Row) -> rusqlite::Result<ChatMessage> {
    Ok(ChatMessage {
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
        text: r.get(5)?,
        reply_to: r.get(6)?,
        status: SendStatus::Sent,
        redacted: r.get(7)?,
        reactions: Vec::new(),
        mention: r.get(8)?,
        own: r.get(9)?,
    })
}

#[cfg(test)]
pub(crate) fn msg(msgid: &str, time: i64, nick: &str, text: &str) -> ChatMessage {
    ChatMessage {
        msgid: Some(msgid.into()),
        local_id: None,
        time,
        nick: nick.into(),
        account: None,
        kind: MessageKind::Privmsg,
        text: text.into(),
        reply_to: None,
        status: SendStatus::Sent,
        redacted: false,
        reactions: Vec::new(),
        mention: false,
        own: false,
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
