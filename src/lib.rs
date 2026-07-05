//! Pluggable channel and messaging primitives for OpenHuman harness
//! communication.
//!
//! TinyChannels owns portable channel configuration, controller metadata,
//! runtime helpers, and the backend boundary that OpenHuman implements for
//! channel side effects.

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
