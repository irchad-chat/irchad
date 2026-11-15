//! The first-launch cards, the same words in every app: what a network is,
//! before the join screen asks for one.

pub struct Card {
    pub title: &'static str,
    pub body: &'static str,
}

/// In order. Each stands alone, so skipping loses nothing but the read.
pub const CARDS: [Card; 4] = [
    Card {
        title: "Welcome to IrChad",
        body: "Chat, voice calls and screen sharing with your friends. \
               Here's how it works. It only takes a moment.",
    },
    Card {
        title: "You join a network",
        body: "A network is a community with its own server, a bit like a Discord \
               server that someone runs themselves. Whoever invited you sends you \
               its link or address.",
    },
    Card {
        title: "You make an account there",
        body: "Each network has its own accounts. Pick a username and a password \
               when you join. The same account works on your computer and your phone.",
    },
    Card {
        title: "Then you're in",
        body: "Chat in text channels, hop into a voice channel to talk or share \
               your screen, and message friends privately.",
    },
];
