//! Pluggable channel and messaging primitives for OpenHuman harness
//! communication.
//!
//! TinyChannels owns portable channel configuration, controller metadata,
//! runtime helpers, and the backend boundary that OpenHuman implements for
//! channel side effects.

pub mod adapters;
pub mod backend;
pub mod channel;
pub mod config;
pub mod context;
pub mod controllers;
pub mod delivery;
pub mod error;
pub mod harness;
pub mod relay;
pub mod routes;
pub mod runtime;
pub mod text;
pub mod traits;

pub use backend::{ChannelBackend, ChannelManager};
pub use channel::{
    ChannelOutboundIntent, DeliveryDurability, OutboundPayload,
    legacy_message_value_from_outbound_intent, outbound_intent_from_legacy_message,
};
pub use config::ChannelsConfig;
pub use controllers::{ChannelAuthMode, ChannelDefinition};
pub use error::{Result, TinyChannelsError};
pub use traits::{Channel, ChannelMessage, SendMessage};
