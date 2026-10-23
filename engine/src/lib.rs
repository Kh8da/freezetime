//! Everything freezetime does that isn't UI or Windows-specific: reading the
//! CS2 Game State Integration feed, deciding when to pause music, keeping match
//! history, and the numbers behind tilt warnings and session recaps.

pub mod cs2;
pub mod engine;
pub mod gsi;
pub mod listener;
pub mod service;
pub mod sim;
pub mod stats;
pub mod store;
pub mod tracker;
pub mod vdf;
