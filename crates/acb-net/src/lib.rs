//! Our own netcode for ACB MP (retail PRUDP/Net-Z is not kept): UDP,
//! versioned compact binary messages, an authoritative headless server
//! running the real [`acb_game::World`] at 60 Hz, and a client that
//! predicts its own player, reconciles against the server's ticks and
//! interpolates remote players. See NOTES.md "Netcode (ours)".

pub mod client;
pub mod game;
pub mod link;
pub mod proto;
pub mod server;
/// The match rules (re-exported for the server CLI).
pub use acb_game::mode;
