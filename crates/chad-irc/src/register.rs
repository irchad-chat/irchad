//! `draft/account-registration`: `REGISTER`, `VERIFY` and their replies.
//!
//! ergo offers it `before-connect`, so this runs before `CAP END`; ergo needs `NICK`
//! sent first (`FAIL REGISTER NEED_NICK`), and the account must equal that nick
//! (`ACCOUNT_NAME_MUST_BE_NICK`), so pass `*`. A success logs the connection in, so no
//! SASL follows on that connection.

use crate::Message;

/// `account` of `*` means the current nick. `email` of `None` sends `*`.
pub fn register(account: &str, email: Option<&str>, password: &str) -> Message {
    Message::new("REGISTER", [account, email.unwrap_or("*"), password])
}

pub fn verify(account: &str, code: &str) -> Message {
    Message::new("VERIFY", [account, code])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Registered {
        account: String,
    },
    /// A code was sent (usually by email); answer with [`verify`].
    NeedsVerification {
        account: String,
        message: String,
    },
    Verified {
        account: String,
    },
    /// `command` is `REGISTER` or `VERIFY`. `code` is the spec's, e.g. `ACCOUNT_EXISTS`,
    /// `WEAK_PASSWORD`, `INVALID_CODE`; the UI maps it to plain words.
    Failed {
        command: String,
        code: String,
        message: String,
    },
}

impl Reply {
    /// `None` for anything that isn't a REGISTER / VERIFY reply.
    pub fn parse(msg: &Message) -> Option<Self> {
        let p = |i| msg.param(i).unwrap_or_default().to_owned();
        let message = msg.params.last().cloned().unwrap_or_default();
        match (msg.command.as_str(), msg.param(0)?) {
            ("REGISTER", "SUCCESS") => Some(Self::Registered { account: p(1) }),
            ("REGISTER", "VERIFICATION_REQUIRED") => Some(Self::NeedsVerification {
                account: p(1),
                message,
            }),
            ("VERIFY", "SUCCESS") => Some(Self::Verified { account: p(1) }),
            ("FAIL", cmd @ ("REGISTER" | "VERIFY")) => Some(Self::Failed {
                command: cmd.to_owned(),
                code: p(1),
                message,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(s: &str) -> Option<Reply> {
        Reply::parse(&Message::parse(s).unwrap())
    }

    #[test]
    fn builds_commands() {
        let line = |m: Message| m.to_line().unwrap();
        assert_eq!(line(register("*", None, "pw")), "REGISTER * * pw");
        assert_eq!(
            line(register("bob", Some("b@x.y"), "two words")),
            "REGISTER bob b@x.y :two words"
        );
        assert_eq!(line(verify("bob", "39gvh")), "VERIFY bob 39gvh");
    }

    #[test]
    fn parses_replies() {
        assert_eq!(
            reply(":chat.localhost REGISTER SUCCESS bob :Account successfully registered"),
            Some(Reply::Registered {
                account: "bob".into()
            })
        );
        assert_eq!(
            reply(":s REGISTER VERIFICATION_REQUIRED bob :Account created, pending verification"),
            Some(Reply::NeedsVerification {
                account: "bob".into(),
                message: "Account created, pending verification".into()
            })
        );
        assert_eq!(
            reply(":s VERIFY SUCCESS bob :Account successfully registered"),
            Some(Reply::Verified {
                account: "bob".into()
            })
        );
        assert_eq!(
            reply(":s FAIL REGISTER ACCOUNT_EXISTS bob :Account already exists"),
            Some(Reply::Failed {
                command: "REGISTER".into(),
                code: "ACCOUNT_EXISTS".into(),
                message: "Account already exists".into()
            })
        );
        assert_eq!(
            reply(":s FAIL VERIFY INVALID_CODE bob :Invalid verification code"),
            Some(Reply::Failed {
                command: "VERIFY".into(),
                code: "INVALID_CODE".into(),
                message: "Invalid verification code".into()
            })
        );
        assert_eq!(reply(":s FAIL CHATHISTORY INVALID_TARGET x :no"), None);
        assert_eq!(reply(":s 001 bob :hi"), None);
    }
}
