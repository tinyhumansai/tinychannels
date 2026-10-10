//! Backend abstraction for OpenHuman-owned channel operations.

use crate::channel::{
    ChannelOutboundIntent, legacy_message_value_from_outbound_intent,
    outbound_intent_from_legacy_message, outbound_intent_from_send_message,
};
use crate::config::{ChannelsConfig, YuanbaoConfig, strip_yuanbao_version_prefix};
use crate::controllers::{
    ChannelAuthMode, ChannelConnectionResult, ChannelDefinition, ChannelDisconnectResult,
    ChannelReactionResult, ChannelSendMessageResult, ChannelStatusEntry, ChannelTestResult,
    ChannelThreadListResult, ChannelThreadResult, DiscordChannelListResult, DiscordGuildListResult,
    DiscordLinkCheckResult, DiscordLinkStartResult, DiscordPermissionCheckResult,
    TelegramLoginCheckResult, TelegramLoginStartResult,
};
use crate::traits::SendMessage;
use async_trait::async_trait;
use serde_json::Value;
use tinychannels_runtime::config::YuanbaoConfigExt as _;
use tinychannels_runtime::controllers::ChannelDefinitionExt as _;

/// Pluggable backend contract used by TinyChannels.
///
/// OpenHuman should implement this trait with its own REST/JWT/config storage
/// layer. Tests and downstream embedders can provide in-memory implementations.
#[async_trait]
pub trait ChannelBackend: Send + Sync {
    async fn connect_channel(
        &self,
        config: &ChannelsConfig,
        channel: &str,
        auth_mode: ChannelAuthMode,
        credentials: Value,
    ) -> anyhow::Result<ChannelConnectionResult>;

    async fn disconnect_channel(
        &self,
        config: &ChannelsConfig,
        channel: &str,
        auth_mode: ChannelAuthMode,
        clear_memory: bool,
    ) -> anyhow::Result<ChannelDisconnectResult>;

    async fn channel_status(
        &self,
        config: &ChannelsConfig,
        channel: Option<&str>,
    ) -> anyhow::Result<Vec<ChannelStatusEntry>>;

    async fn test_channel(
        &self,
        config: &ChannelsConfig,
        channel: &str,
        auth_mode: ChannelAuthMode,
        credentials: Value,
    ) -> anyhow::Result<ChannelTestResult>;

    async fn send_message(
        &self,
        config: &ChannelsConfig,
        channel: &str,
        message: SendMessage,
    ) -> anyhow::Result<ChannelSendMessageResult>;

    async fn send_message_value(
        &self,
        _config: &ChannelsConfig,
        _channel: &str,
        _message: Value,
    ) -> anyhow::Result<ChannelSendMessageResult> {
        Err(anyhow::anyhow!(
            "raw channel message payloads are not supported by this backend"
        ))
    }

    async fn send_outbound_intent(
        &self,
        config: &ChannelsConfig,
        intent: ChannelOutboundIntent,
    ) -> anyhow::Result<ChannelSendMessageResult> {
        let channel = intent.channel_id.clone();
        self.send_message_value(
            config,
            &channel,
            legacy_message_value_from_outbound_intent(&intent),
        )
        .await
    }

    async fn send_reaction(
        &self,
        config: &ChannelsConfig,
        channel: &str,
        reaction: Value,
    ) -> anyhow::Result<ChannelReactionResult>;

    async fn create_thread(
        &self,
        config: &ChannelsConfig,
        channel: &str,
        title: &str,
    ) -> anyhow::Result<ChannelThreadResult>;

    async fn update_thread(
        &self,
        config: &ChannelsConfig,
        channel: &str,
        thread_id: &str,
        action: &str,
    ) -> anyhow::Result<ChannelThreadResult>;

    async fn list_threads(
        &self,
        config: &ChannelsConfig,
        channel: &str,
        active: Option<bool>,
    ) -> anyhow::Result<ChannelThreadListResult>;

    async fn telegram_login_start(
        &self,
        config: &ChannelsConfig,
    ) -> anyhow::Result<TelegramLoginStartResult>;

    async fn telegram_login_check(
        &self,
        config: &ChannelsConfig,
        link_token: &str,
    ) -> anyhow::Result<TelegramLoginCheckResult>;

    async fn discord_link_start(
        &self,
        config: &ChannelsConfig,
    ) -> anyhow::Result<DiscordLinkStartResult>;

    async fn discord_link_check(
        &self,
        config: &ChannelsConfig,
        link_token: &str,
    ) -> anyhow::Result<DiscordLinkCheckResult>;

    async fn discord_list_guilds(
        &self,
        config: &ChannelsConfig,
    ) -> anyhow::Result<DiscordGuildListResult>;

    async fn discord_list_channels(
        &self,
        config: &ChannelsConfig,
        guild_id: &str,
    ) -> anyhow::Result<DiscordChannelListResult>;

    async fn discord_check_permissions(
        &self,
        config: &ChannelsConfig,
        guild_id: &str,
        channel_id: &str,
    ) -> anyhow::Result<DiscordPermissionCheckResult>;

    async fn set_default_channel(
        &self,
        _config: &ChannelsConfig,
        _channel: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn get_default_channel(&self, config: &ChannelsConfig) -> anyhow::Result<Option<String>> {
        Ok(config.active_channel.clone())
    }
}

/// Backend-free operations plus backend delegation for runtime effects.
pub struct ChannelManager<B> {
    config: ChannelsConfig,
    backend: B,
}

impl<B> ChannelManager<B> {
    pub fn new(config: ChannelsConfig, backend: B) -> Self {
        Self { config, backend }
    }

    pub fn config(&self) -> &ChannelsConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut ChannelsConfig {
        &mut self.config
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn list_definitions(&self) -> Vec<ChannelDefinition> {
        crate::controllers::all_channel_definitions()
    }

    pub fn describe(&self, channel: &str) -> Option<ChannelDefinition> {
        crate::controllers::find_channel_definition(channel)
    }
}

impl<B: ChannelBackend> ChannelManager<B> {
    pub async fn connect(
        &self,
        channel: &str,
        auth_mode: ChannelAuthMode,
        credentials: Value,
    ) -> anyhow::Result<ChannelConnectionResult> {
        let definition = self
            .describe(channel)
            .ok_or_else(|| anyhow::anyhow!("unknown channel: {channel}"))?;
        let credentials_map = credentials
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("credentials must be a JSON object"))?;
        definition
            .validate_credentials(auth_mode, credentials_map)
            .map_err(anyhow::Error::msg)?;
        let credentials = normalize_connect_credentials(channel, credentials)?;
        self.backend
            .connect_channel(&self.config, channel, auth_mode, credentials)
            .await
    }

    pub async fn disconnect(
        &self,
        channel: &str,
        auth_mode: ChannelAuthMode,
        clear_memory: bool,
    ) -> anyhow::Result<ChannelDisconnectResult> {
        self.backend
            .disconnect_channel(&self.config, channel, auth_mode, clear_memory)
            .await
    }

    pub async fn status(&self, channel: Option<&str>) -> anyhow::Result<Vec<ChannelStatusEntry>> {
        if let Some(channel) = channel {
            self.describe(channel)
                .ok_or_else(|| anyhow::anyhow!("unknown channel: {channel}"))?;
        }
        self.backend.channel_status(&self.config, channel).await
    }

    pub async fn test(
        &self,
        channel: &str,
        auth_mode: ChannelAuthMode,
        credentials: Value,
    ) -> anyhow::Result<ChannelTestResult> {
        let definition = self
            .describe(channel)
            .ok_or_else(|| anyhow::anyhow!("unknown channel: {channel}"))?;
        let credentials_map = credentials
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("credentials must be a JSON object"))?;
        definition
            .validate_credentials(auth_mode, credentials_map)
            .map_err(anyhow::Error::msg)?;
        self.backend
            .test_channel(&self.config, channel, auth_mode, credentials)
            .await
    }

    #[tracing::instrument(skip(self, message), fields(channel = %channel))]
    pub async fn send_message(
        &self,
        channel: &str,
        message: SendMessage,
    ) -> anyhow::Result<ChannelSendMessageResult> {
        self.backend
            .send_outbound_intent(
                &self.config,
                outbound_intent_from_send_message(channel, &message),
            )
            .await
    }

    #[tracing::instrument(skip(self, message), fields(channel = %channel))]
    pub async fn send_message_value(
        &self,
        channel: &str,
        message: Value,
    ) -> anyhow::Result<ChannelSendMessageResult> {
        self.backend
            .send_outbound_intent(
                &self.config,
                outbound_intent_from_legacy_message(channel, message),
            )
            .await
    }

    #[tracing::instrument(skip(self, intent), fields(channel = %intent.channel_id))]
    pub async fn send_outbound_intent(
        &self,
        intent: ChannelOutboundIntent,
    ) -> anyhow::Result<ChannelSendMessageResult> {
        self.backend
            .send_outbound_intent(&self.config, intent)
            .await
    }

    pub async fn send_reaction(
        &self,
        channel: &str,
        reaction: Value,
    ) -> anyhow::Result<ChannelReactionResult> {
        self.backend
            .send_reaction(&self.config, channel, reaction)
            .await
    }

    pub async fn create_thread(
        &self,
        channel: &str,
        title: &str,
    ) -> anyhow::Result<ChannelThreadResult> {
        self.backend
            .create_thread(&self.config, channel, title)
            .await
    }

    pub async fn update_thread(
        &self,
        channel: &str,
        thread_id: &str,
        action: &str,
    ) -> anyhow::Result<ChannelThreadResult> {
        self.backend
            .update_thread(&self.config, channel, thread_id, action)
            .await
    }

    pub async fn list_threads(
        &self,
        channel: &str,
        active: Option<bool>,
    ) -> anyhow::Result<ChannelThreadListResult> {
        self.backend
            .list_threads(&self.config, channel, active)
            .await
    }

    pub async fn telegram_login_start(&self) -> anyhow::Result<TelegramLoginStartResult> {
        self.backend.telegram_login_start(&self.config).await
    }

    pub async fn telegram_login_check(
        &self,
        link_token: &str,
    ) -> anyhow::Result<TelegramLoginCheckResult> {
        self.backend
            .telegram_login_check(&self.config, link_token)
            .await
    }

    pub async fn discord_link_start(&self) -> anyhow::Result<DiscordLinkStartResult> {
        self.backend.discord_link_start(&self.config).await
    }

    pub async fn discord_link_check(
        &self,
        link_token: &str,
    ) -> anyhow::Result<DiscordLinkCheckResult> {
        self.backend
            .discord_link_check(&self.config, link_token)
            .await
    }

    pub async fn discord_list_guilds(&self) -> anyhow::Result<DiscordGuildListResult> {
        self.backend.discord_list_guilds(&self.config).await
    }

    pub async fn discord_list_channels(
        &self,
        guild_id: &str,
    ) -> anyhow::Result<DiscordChannelListResult> {
        self.backend
            .discord_list_channels(&self.config, guild_id)
            .await
    }

    pub async fn discord_check_permissions(
        &self,
        guild_id: &str,
        channel_id: &str,
    ) -> anyhow::Result<DiscordPermissionCheckResult> {
        self.backend
            .discord_check_permissions(&self.config, guild_id, channel_id)
            .await
    }

    pub async fn set_default_channel(&self, channel: &str) -> anyhow::Result<()> {
        let canonical = canonical_default_channel(channel)?;
        self.backend
            .set_default_channel(&self.config, &canonical)
            .await
    }

    pub async fn get_default_channel(&self) -> anyhow::Result<Option<String>> {
        self.backend.get_default_channel(&self.config).await
    }
}

fn normalize_connect_credentials(channel: &str, credentials: Value) -> anyhow::Result<Value> {
    if channel != "yuanbao" {
        return Ok(credentials);
    }

    let mut config: YuanbaoConfig = serde_json::from_value(credentials)?;
    config.apply_env_defaults();
    config.bot_version = strip_yuanbao_version_prefix(&config.bot_version).to_string();
    config.validate().map_err(anyhow::Error::msg)?;
    Ok(serde_json::to_value(config)?)
}

fn canonical_default_channel(channel: &str) -> anyhow::Result<String> {
    let canonical = channel.trim().to_ascii_lowercase();
    if canonical.is_empty() {
        return Err(anyhow::anyhow!("channel must not be empty"));
    }
    if canonical != "web" && crate::controllers::find_channel_definition(&canonical).is_none() {
        return Err(anyhow::anyhow!("unknown channel: {channel}"));
    }
    Ok(canonical)
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
