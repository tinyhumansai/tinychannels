//! Channel controller metadata and backend response types.

pub mod definitions;
pub mod types;

pub use definitions::{
    AuthModeSpec, ChannelAuthMode, ChannelCapability, ChannelDefinition, FieldRequirement,
    all_channel_definitions, find_channel_definition,
};
pub use types::{
    ChannelAccountSnapshot, ChannelAccountState, ChannelConnectionResult, ChannelDisconnectResult,
    ChannelLastDisconnect, ChannelReactionResult, ChannelSendMessageResult, ChannelStatusEntry,
    ChannelTestResult, ChannelThreadEntry, ChannelThreadListResult, ChannelThreadResult,
    DiscordChannelEntry, DiscordChannelListResult, DiscordGuildEntry, DiscordGuildListResult,
    DiscordLinkCheckResult, DiscordLinkStartResult, DiscordPermissionCheckResult,
    TelegramLoginCheckResult, TelegramLoginStartResult,
};
