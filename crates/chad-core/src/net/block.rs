//! The local block list (traditional IRC ignore): by account, else `*!*@host`.

use super::*;

impl Net {
    /// From someone we've blocked. Never us.
    pub(super) fn blocked(&self, m: &Message) -> bool {
        if self.blocks.is_empty() {
            return false;
        }
        let Some((nick, host)) = m.source.as_deref().and_then(|s| s.split_once('!')) else {
            return false;
        };
        if self.cm.eq(nick, &self.info.nick) {
            return false;
        }
        let account = m
            .tag("account")
            .map(|a| format!("$a:{}", a.to_ascii_lowercase()));
        // shortcut: only the exact `*!*@host` masks we make, no globbing; add it if users type masks.
        let host = host
            .split_once('@')
            .map(|(_, h)| format!("*!*@{}", h.to_ascii_lowercase()));
        self.blocks
            .iter()
            .any(|b| Some(&b.mask) == account.as_ref() || Some(&b.mask) == host.as_ref())
    }

    pub(super) fn block(&mut self, nick: &str, account: Option<String>) {
        if !valid_name(nick) || self.is_channel(nick) || self.cm.eq(nick, &self.info.nick) {
            return;
        }
        let folded = self.fold(nick);
        let account = account.filter(|a| valid_name(a)).or_else(|| {
            self.buffers
                .values()
                .find_map(|b| b.members.get(&folded)?.account.clone())
        });
        match account {
            Some(a) => {
                let mask = format!("$a:{}", a.to_ascii_lowercase());
                self.add_block(mask, nick);
            }
            None if self.connected => {
                // WHOX fields come in a fixed order: token, host, nick, account.
                let m = Message::new("WHO", [nick, "%tnha,2"]);
                let nick = nick.to_owned();
                self.send_labeled(m, Pending::Block { nick });
            }
            None => self.error(format!("Connect first to block {nick}.")),
        }
    }

    /// `354 <me> 2 <host> <nick> <account>` for `nick`.
    pub(super) fn block_reply(&mut self, nick: &str, replies: impl Iterator<Item = Message>) {
        let found = replies
            .filter(|m| m.command == "354" && m.param(1) == Some("2"))
            .find(|m| m.param(3).is_some_and(|n| self.cm.eq(n, nick)));
        let mask = found.as_ref().and_then(|m| match (m.param(2), m.param(4)) {
            (_, Some(a)) if a != "0" && valid_name(a) => {
                Some(format!("$a:{}", a.to_ascii_lowercase()))
            }
            (Some(h), _) if valid_name(h) && !h.contains(['@', '!', '*']) => {
                Some(format!("*!*@{}", h.to_ascii_lowercase()))
            }
            _ => None,
        });
        match mask {
            Some(mask) => self.add_block(mask, nick),
            None => self.error(format!("Couldn't block {nick}: they're not online.")),
        }
    }

    fn add_block(&mut self, mask: String, nick: &str) {
        if self.blocks.iter().any(|b| b.mask == mask) {
            return;
        }
        let b = Block {
            mask,
            nick: nick.to_owned(),
        };
        self.store.put_block(&b);
        self.blocks.push(b);
        self.emit_blocks();
    }

    pub(super) fn unblock(&mut self, mask: &str) {
        let before = self.blocks.len();
        self.blocks.retain(|b| b.mask != mask);
        if self.blocks.len() != before {
            self.store.remove_block(mask);
            self.emit_blocks();
        }
    }

    pub(super) fn emit_blocks(&mut self) {
        let (network, blocks) = (self.id(), self.blocks.clone());
        self.events.push(Event::Blocks { network, blocks });
    }
}
