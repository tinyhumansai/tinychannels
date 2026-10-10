//! Channel controller metadata and backend response types.

pub mod credentials;
pub mod definitions;
pub mod forms;
pub use tinychannels_bus::controllers::schemas;
pub use tinychannels_bus::controllers::types;

pub use credentials::{channel_credential_provider, parse_allowed_users};
pub use definitions::{
    AuthModeSpec, ChannelAuthMode, ChannelCapability, ChannelDefinition, ChannelDefinitionExt,
    FieldRequirement, all_channel_definitions, channel_config_connected, find_channel_definition,
};
pub use forms::{build_email_config, parse_email_senders, parse_optional_bool, parse_port_field};
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
