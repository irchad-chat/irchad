//! Batch reassembly (IRCv3 `batch`), including nested batches.
//!
//! Sans-IO: feed every incoming line to [`Assembler::feed`]. Lines outside any
//! batch come straight back; batched lines are held until their outermost batch
//! closes, then the whole tree comes back as one [`Item::Batch`].
//!
//! The server is hostile input (D14): open batches and held bytes are capped, and
//! going over is an [`Overflow`] the caller should treat as a reason to disconnect.

use std::collections::HashMap;

use crate::Message;

/// Bounds nesting depth too. ergo nests at most chathistory → multiline (plus labeled-response).
const MAX_OPEN: usize = 64;
/// A 1000-line chathistory page of 4 KiB multiline messages fits with room to spare.
const MAX_HELD_BYTES: usize = 32 << 20;

#[derive(Debug, thiserror::Error)]
#[error("server exceeded batch limits")]
pub struct Overflow;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Message(Message),
    Batch(Batch),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    /// The `BATCH +id type params…` line, with its tags (e.g. `label`).
    pub start: Message,
    /// Members in arrival order. A nested batch sits where it closed.
    pub items: Vec<Item>,
}

impl Batch {
    /// e.g. `chathistory`, `draft/multiline`, `labeled-response`.
    pub fn kind(&self) -> &str {
        self.start.param(1).unwrap_or("")
    }

    /// Params after the type, e.g. the target of a `chathistory` batch.
    pub fn params(&self) -> &[String] {
        self.start.params.get(2..).unwrap_or(&[])
    }
}

#[derive(Debug, Default)]
pub struct Assembler {
    /// Open batches by reference tag, with their parent's tag if nested.
    open: HashMap<String, Open>,
    held_bytes: usize,
}

#[derive(Debug)]
struct Open {
    batch: Batch,
    parent: Option<String>,
    /// Bytes held in this batch and its closed children.
    bytes: usize,
}

impl Assembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a line or a finished top-level batch, or `None` while one is still open.
    /// Lines tagged with an unknown batch, stray `BATCH -id` and a `BATCH +id` reusing an
    /// open id pass through as messages. On [`Overflow`] all open batches are dropped.
    pub fn feed(&mut self, msg: Message) -> Result<Option<Item>, Overflow> {
        let parent = msg
            .tag("batch")
            .filter(|p| self.open.contains_key(*p))
            .map(str::to_owned);
        if msg.command == "BATCH" {
            let reference = msg.param(0).unwrap_or("");
            if let Some(id) = reference
                .strip_prefix('+')
                .filter(|id| !id.is_empty() && !self.open.contains_key(*id))
            {
                if self.open.len() >= MAX_OPEN {
                    self.clear();
                    return Err(Overflow);
                }
                let id = id.to_owned();
                let bytes = self.hold(&msg)?;
                let batch = Batch {
                    start: msg,
                    items: Vec::new(),
                };
                self.open.insert(
                    id,
                    Open {
                        batch,
                        parent,
                        bytes,
                    },
                );
                return Ok(None);
            }
            if let Some(open) = reference
                .strip_prefix('-')
                .and_then(|id| self.open.remove(id))
            {
                return Ok(self.attach(open.parent, Item::Batch(open.batch), open.bytes));
            }
        }
        let bytes = if parent.is_some() {
            self.hold(&msg)?
        } else {
            0
        };
        Ok(self.attach(parent, Item::Message(msg), bytes))
    }

    /// Drops open batches, e.g. on disconnect: a server never resumes them.
    pub fn clear(&mut self) {
        self.open.clear();
        self.held_bytes = 0;
    }

    /// Counts a line about to be buffered against the budget.
    fn hold(&mut self, msg: &Message) -> Result<usize, Overflow> {
        let bytes = msg.command.len()
            + msg.source.as_ref().map_or(0, String::len)
            + msg.params.iter().map(|p| p.len() + 1).sum::<usize>()
            + msg
                .tags
                .iter()
                .map(|(k, v)| k.len() + v.len() + 2)
                .sum::<usize>();
        self.held_bytes += bytes;
        if self.held_bytes > MAX_HELD_BYTES {
            self.clear();
            return Err(Overflow);
        }
        Ok(bytes)
    }

    fn attach(&mut self, parent: Option<String>, item: Item, bytes: usize) -> Option<Item> {
        match parent.and_then(|p| self.open.get_mut(&p)) {
            Some(open) => {
                open.batch.items.push(item);
                open.bytes += bytes;
                None
            }
            None => {
                self.held_bytes -= bytes;
                Some(item)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(a: &mut Assembler, lines: &[&str]) -> Vec<Item> {
        lines
            .iter()
            .filter_map(|l| a.feed(Message::parse(l).unwrap()).unwrap())
            .collect()
    }

    fn texts(b: &Batch) -> Vec<&str> {
        b.items
            .iter()
            .filter_map(|i| match i {
                Item::Message(m) => m.param(1),
                Item::Batch(_) => None,
            })
            .collect()
    }

    #[test]
    fn unbatched_passes_through() {
        let mut a = Assembler::new();
        let out = feed_all(&mut a, &[":a PRIVMSG #c :hi"]);
        assert!(matches!(&out[..], [Item::Message(m)] if m.param(1) == Some("hi")));
    }

    #[test]
    fn nested_multiline_in_chathistory() {
        let mut a = Assembler::new();
        let out = feed_all(
            &mut a,
            &[
                "@label=7 :srv BATCH +h chathistory #c",
                "@batch=h :a PRIVMSG #c :one",
                "@batch=h :srv BATCH +m draft/multiline #c",
                "@batch=m :a PRIVMSG #c :line1",
                "@batch=m :a PRIVMSG #c :line2",
                ":srv BATCH -m",
                "@batch=h :a PRIVMSG #c :two",
                ":x PRIVMSG #c :interleaved",
                ":srv BATCH -h",
            ],
        );
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Item::Message(m) if m.param(1) == Some("interleaved")));
        let Item::Batch(h) = &out[1] else { panic!() };
        assert_eq!(h.kind(), "chathistory");
        assert_eq!(h.params(), ["#c"]);
        assert_eq!(h.start.tag("label"), Some("7"));
        assert_eq!(texts(h), ["one", "two"]);
        let Item::Batch(m) = &h.items[1] else {
            panic!()
        };
        assert_eq!(m.kind(), "draft/multiline");
        assert_eq!(texts(m), ["line1", "line2"]);
        assert!(a.open.is_empty());
        assert_eq!(a.held_bytes, 0);
    }

    #[test]
    fn unknown_references_pass_through() {
        let mut a = Assembler::new();
        let out = feed_all(
            &mut a,
            &["@batch=nope :a PRIVMSG #c :hi", ":srv BATCH -nope"],
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn hostile_server_is_bounded() {
        let mut a = Assembler::new();
        let open = |i: usize| Message::parse(&format!(":s BATCH +b{i} x")).unwrap();
        for i in 0..MAX_OPEN {
            a.feed(open(i)).unwrap();
        }
        assert!(a.feed(open(MAX_OPEN)).is_err());
        assert!(a.open.is_empty());

        a.feed(open(0)).unwrap();
        let big = Message::new("PRIVMSG", ["#c", &"x".repeat(1 << 20)]).with_tag("batch", "b0");
        let fed = (0..64).map(|_| a.feed(big.clone())).find(Result::is_err);
        assert!(fed.is_some());
        assert_eq!(a.held_bytes, 0);
    }

    #[test]
    fn reused_open_id_passes_through() {
        let mut a = Assembler::new();
        let out = feed_all(
            &mut a,
            &[":s BATCH +b x", "@batch=b :s BATCH +b y", ":s BATCH -b"],
        );
        let Item::Batch(b) = &out[0] else { panic!() };
        assert_eq!(b.kind(), "x");
        assert!(matches!(&b.items[..], [Item::Message(m)] if m.param(0) == Some("+b")));
    }
}
