//! IRCv3 protocol: parse/serialize, CAP, SASL, batch, labeled-response, ISUPPORT.
//! Sans-IO except [`ws`], the WebSocket transport.

pub mod batch;
pub mod cap;
pub mod isupport;
pub mod label;
pub mod message;
pub mod register;
pub mod sasl;
pub mod ws;

pub use message::Message;
