//! Pluggable channel and messaging primitives for OpenHuman harness
//! communication.
//!
//! This crate is intentionally blank scaffolding. Module boundaries are reserved
//! for the channel-to-harness communication layer, but public behavior should be
//! added only when the OpenHuman integration contract is concrete.

pub mod backend;
pub mod channel;
pub mod config;
pub mod context;
pub mod controllers;
pub mod error;
pub mod harness;
pub mod routes;
pub mod runtime;
pub mod traits;

pub use backend::{ChannelBackend, ChannelManager};
pub use config::ChannelsConfig;
pub use controllers::{ChannelAuthMode, ChannelDefinition};
pub use error::{Result, TinyChannelsError};
pub use traits::{Channel, ChannelMessage, SendMessage};
