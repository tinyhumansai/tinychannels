//! Pure serialized vocabulary, schema declarations and errors for TinyChannels.
//! Provider seams and algorithms live in `tinychannels-runtime`; the root
//! implementation crate preserves legacy module paths through re-exports.
pub mod capabilities;
pub mod channel;
pub mod config;
pub mod context;
pub mod controllers;
pub mod error;
pub mod names;
pub mod relay;
pub mod traits;
pub mod version;
pub use capabilities::{ChannelCapabilities, capabilities_for, provider_id};
pub use channel::{
    ChannelInboundEnvelope, ChannelOutboundIntent, DeliveryDurability, OutboundPayload,
};
pub use config::ChannelsConfig;
pub use controllers::{ChannelAuthMode, ChannelDefinition};
pub use error::{Result, TinyChannelsError};
pub use names::{BUS_NAME, HOST_BUS_NAME, HOST_OBJECT_PATH, METHODS, OBJECT_PATH, methods};
pub use traits::{ChannelMessage, SendMessage};
pub use version::{CONTRACT_VERSION, is_compatible};

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
