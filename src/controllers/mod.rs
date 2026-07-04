//! Channel controller metadata and backend response types.

pub mod definitions;
pub mod types;

pub use definitions::{
    AuthModeSpec, ChannelAuthMode, ChannelCapability, ChannelDefinition, FieldRequirement,
    all_channel_definitions, find_channel_definition,
};
pub use types::{
    ChannelConnectionResult, ChannelStatusEntry, ChannelTestResult, DiscordLinkCheckResult,
    DiscordLinkStartResult, TelegramLoginCheckResult, TelegramLoginStartResult,
};
