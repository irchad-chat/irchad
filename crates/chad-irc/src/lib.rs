//! IRCv3 protocol: parse/serialize, CAP, SASL, batch, labeled-response. No I/O policy.

pub mod message;

pub use message::Message;
