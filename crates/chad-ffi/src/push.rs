//! UnifiedPush subscriptions and the pushes the server sends.

use super::*;

/// A UnifiedPush endpoint and the keys the distributor made for it (base64url).
#[derive(uniffi::Record)]
pub struct PushSubscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

/// What a push from ergo (`draft/webpush`) says.
#[derive(uniffi::Enum, Debug, PartialEq, Eq)]
pub enum Push {
    /// A DM or a mention; `target` is the buffer (their nick for a DM).
    Message {
        target: String,
        from: String,
        text: String,
        channel: bool,
    },
    /// Read on another device: clear `target`'s notification.
    Read { target: String },
}

/// A decrypted push payload, which is one IRC line. ergo keeps only the
/// `time`, `msgid` and `account` tags, so an encrypted DM arrives as its
/// readable fallback (`+chad/e2e` is gone). `None` for anything else, such as
/// the `PING` that tests a new subscription. D14: the line is the server's.
#[uniffi::export]
pub fn read_push(line: String) -> Option<Push> {
    if line.len() > 8192 {
        return None;
    }
    let m = chad_irc::Message::parse(&line).ok()?;
    let p = |i| m.param(i).filter(|s| !s.is_empty());
    match m.command.as_str() {
        "PRIVMSG" | "NOTICE" => {
            let (to, text) = (p(0)?, p(1)?);
            let nick = snippet(m.source_nick().filter(|n| !n.is_empty())?, 64);
            let action = text
                .strip_prefix("\x01ACTION ")
                .map(|t| t.trim_end_matches('\x01'));
            if action.is_none() && text.starts_with('\x01') {
                return None;
            }
            let channel = to.starts_with('#');
            let text = match action {
                Some(a) => format!("* {nick} {}", format::strip(a)),
                None => format::strip(text),
            };
            Some(Push::Message {
                target: if channel {
                    snippet(to, 64)
                } else {
                    nick.clone()
                },
                from: nick,
                text: snippet(&text, 200),
                channel,
            })
        }
        "MARKREAD" => Some(Push::Read {
            target: snippet(p(0)?, 64),
        }),
        _ => None,
    }
}
