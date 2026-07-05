//! Channel controller metadata and backend response types.

pub mod credentials;
pub mod definitions;
pub mod schemas;
pub mod types;

pub use credentials::{channel_credential_provider, parse_allowed_users};
pub use definitions::{
    AuthModeSpec, ChannelAuthMode, ChannelCapability, ChannelDefinition, FieldRequirement,
    all_channel_definitions, channel_config_connected, find_channel_definition,
};
pub use schemas::{
    ChannelControllerField, ChannelControllerFieldType, ChannelControllerSchema,
    all_channel_controller_schemas, channel_controller_schema,
};
pub use types::{
    ChannelAccountSnapshot, ChannelAccountState, ChannelConnectionResult, ChannelDisconnectResult,
    ChannelLastDisconnect, ChannelReactionResult, ChannelSendMessageResult, ChannelStatusEntry,
    ChannelTestResult, ChannelThreadEntry, ChannelThreadListResult, ChannelThreadResult,
    DiscordChannelEntry, DiscordChannelListResult, DiscordGuildEntry, DiscordGuildListResult,
    DiscordLinkCheckResult, DiscordLinkStartResult, DiscordPermissionCheckResult,
    TelegramLoginCheckResult, TelegramLoginStartResult,
};
