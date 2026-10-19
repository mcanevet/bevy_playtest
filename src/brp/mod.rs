//! Bevy Remote Protocol client and wire types for the external integration
//! mode. Cloned from Bevy upstream (see module docs in `client.rs`).

#[cfg(feature = "brp")]
pub mod client;
pub mod wire;

#[cfg(feature = "brp")]
pub use client::{BrpClient, BrpClientError};
