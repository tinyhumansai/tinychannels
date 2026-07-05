//! Shared response types for channel controller operations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::channel::MessageReceipt;

/// Result returned by `connect_channel`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChannelConnectionResult {
    /// `"connected"` for credential-based modes, `"pending_auth"` for OAuth/managed.
    pub status: String,
    /// Whether the service must be restarted for the channel to become active.
    pub restart_required: bool,
    /// For OAuth/managed modes: the action ID the frontend should handle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_action: Option<String>,
    /// Human-readable status message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Result returned by `disconnect_channel`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelDisconnectResult {
    pub channel: String,
    pub auth_mode: super::definitions::ChannelAuthMode,
    pub disconnected: bool,
    pub restart_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_chunks_deleted: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

impl Default for ChannelDisconnectResult {
    fn default() -> Self {
        Self {
            channel: String::new(),
            auth_mode: super::definitions::ChannelAuthMode::ApiKey,
            disconnected: false,
            restart_required: false,
            memory_chunks_deleted: None,
            message: None,
            raw: None,
        }
    }
}

/// Single entry returned by `channel_status`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChannelStatusEntry {
    pub channel_id: String,
    pub auth_mode: super::definitions::ChannelAuthMode,
    pub connected: bool,
    pub has_credentials: bool,
    /// Live failure reason from the supervised listener when the channel is
    /// configured but its runtime listener is currently in an error state
    /// (issue #3712 — surface a real error instead of a false "Connected").
    /// `None` when healthy, still starting, or when the mode has no runtime
    /// listener (e.g. managed-DM, which routes through the backend bot).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Account/configuration state for a channel adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelAccountState {
    Linked,
    NotLinked,
    Configured,
    NotConfigured,
    Enabled,
    Disabled,
}

/// Last adapter disconnect reason.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelLastDisconnect {
    pub at_unix_ms: u64,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub logged_out: bool,
}

/// Adapter account status snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelAccountSnapshot {
    pub channel_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub states: Vec<ChannelAccountState>,
    pub reconnect_attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_disconnect: Option<ChannelLastDisconnect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_state: Option<String>,
    pub restart_pending: bool,
}

impl Default for ChannelAccountSnapshot {
    fn default() -> Self {
        Self {
            channel_id: String::new(),
            account_id: None,
            states: vec![ChannelAccountState::NotConfigured],
            reconnect_attempts: 0,
            last_disconnect: None,
            health_state: None,
            restart_pending: false,
        }
    }
}

/// Result returned by `test_channel`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChannelTestResult {
    pub success: bool,
    pub message: String,
}

/// Result returned by `send_message`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelSendMessageResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<MessageReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// Result returned by `send_reaction`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelReactionResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// Result returned by thread creation/update operations.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelThreadResult {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// One channel thread row.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelThreadEntry {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// Result returned by thread listing.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelThreadListResult {
    pub threads: Vec<ChannelThreadEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// Result from `telegram_login_start`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TelegramLoginStartResult {
    /// The short-lived link token created by the backend.
    pub link_token: String,
    /// Full Telegram deep link URL the user should open.
    pub telegram_url: String,
    /// Bot username used.
    pub bot_username: String,
}

/// Result from `telegram_login_check`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TelegramLoginCheckResult {
    /// Whether the Telegram user has been linked to the app user.
    pub linked: bool,
    /// Backend-provided status payload (may include telegramUserId, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Result from `discord_link_start`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiscordLinkStartResult {
    /// The short-lived link token to paste into Discord.
    pub link_token: String,
    /// Human-readable instruction shown to the user.
    pub instructions: String,
}

/// Result from `discord_link_check`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiscordLinkCheckResult {
    /// Whether the Discord account has been linked to the app user.
    pub linked: bool,
    /// Backend-provided status payload (may include discordId, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Discord guild row for setup flows.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscordGuildEntry {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// Result returned by Discord guild listing.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscordGuildListResult {
    pub guilds: Vec<DiscordGuildEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// Discord channel row for setup flows.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscordChannelEntry {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// Result returned by Discord channel listing.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscordChannelListResult {
    pub channels: Vec<DiscordChannelEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// Result returned by Discord permission checks.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscordPermissionCheckResult {
    pub can_send_messages: bool,
    pub missing_permissions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}
