//! `labeled-response` correlation: match a reply to the request that caused it.
//!
//! Sans-IO: [`Labels::tag`] labels an outgoing line and remembers a caller token
//! (e.g. a `oneshot::Sender`, which turns this into request → future). Feed every
//! item from [`crate::batch::Assembler`] to [`Labels::take`]. The reply is a single
//! labeled line, a `labeled-response` batch, or a bare `ACK` for "no output".
//!
//! Only works with `labeled-response` enabled; without it nothing comes back
//! labeled and every token stays pending until [`Labels::clear`].

use std::collections::HashMap;

use crate::Message;
use crate::batch::Item;

#[derive(Debug)]
pub struct Labels<T> {
    next: u64,
    pending: HashMap<String, T>,
}

impl<T> Default for Labels<T> {
    fn default() -> Self {
        Self {
            next: 0,
            pending: HashMap::new(),
        }
    }
}

impl<T> Labels<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a fresh `label` tag to `msg` and holds `token` until its reply arrives.
    pub fn tag(&mut self, msg: Message, token: T) -> Message {
        self.next += 1;
        let label = self.next.to_string();
        self.pending.insert(label.clone(), token);
        msg.with_tag("label", label)
    }

    /// The token and reply when `item` answers a pending request; otherwise `item`
    /// comes back untouched. Unknown and repeated labels from the server pass through.
    pub fn take(&mut self, item: Item) -> Result<(T, Item), Item> {
        let start = match &item {
            Item::Message(m) => m,
            Item::Batch(b) => &b.start,
        };
        match start.tag("label").and_then(|l| self.pending.remove(l)) {
            Some(token) => Ok((token, item)),
            None => Err(item),
        }
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Drops every pending request, e.g. on disconnect: replies never cross connections.
    pub fn clear(&mut self) -> Vec<T> {
        self.pending.drain().map(|(_, t)| t).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::Assembler;

    #[test]
    fn single_batch_and_ack_replies() {
        let mut labels = Labels::new();
        let mut asm = Assembler::new();
        let who = labels.tag(Message::new("WHO", ["#c"]), "who");
        labels.tag(Message::new("AWAY", [""; 0]), "away");
        labels.tag(Message::new("PRIVMSG", ["#c", "hi"]), "msg");
        assert_eq!(who.to_line().unwrap(), "@label=1 WHO #c");
        assert_eq!(labels.pending(), 3);

        let mut answered = Vec::new();
        for line in [
            "@label=3 :me PRIVMSG #c :hi",
            "@label=999 :s ACK",
            "@label=1 :s BATCH +b labeled-response",
            "@batch=b :s 352 me #c u h s a H :0 a",
            "@batch=b :s 315 me #c :End",
            ":s BATCH -b",
            "@label=2 :s ACK",
            "@label=2 :s ACK",
        ] {
            if let Some(item) = asm.feed(Message::parse(line).unwrap()).unwrap() {
                answered.push(labels.take(item).map(|(t, _)| t).map_err(|_| ()));
            }
        }
        assert_eq!(
            answered,
            [Ok("msg"), Err(()), Ok("who"), Ok("away"), Err(())]
        );
        assert_eq!(labels.pending(), 0);
    }

    #[test]
    fn clear_returns_tokens() {
        let mut labels = Labels::new();
        labels.tag(Message::new("PING", ["x"]), 1);
        assert_eq!(labels.clear(), [1]);
        assert_eq!(labels.pending(), 0);
    }
}
