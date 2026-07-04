//! Pluggable channel and messaging primitives for OpenHuman harness
//! communication.
//!
//! This crate is intentionally blank scaffolding. Module boundaries are reserved
//! for the channel-to-harness communication layer, but public behavior should be
//! added only when the OpenHuman integration contract is concrete.

pub mod channel;
pub mod error;
pub mod harness;

pub use error::{Result, TinyChannelsError};
