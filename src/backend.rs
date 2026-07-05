//! Backend abstraction for OpenHuman-owned channel operations.

use crate::config::{ChannelsConfig, YuanbaoConfig, strip_yuanbao_version_prefix};
use crate::controllers::{
    ChannelAuthMode, ChannelConnectionResult, ChannelDefinition, ChannelReactionResult,
    ChannelSendMessageResult, ChannelStatusEntry, ChannelTestResult, ChannelThreadListResult,
    ChannelThreadResult, DiscordChannelListResult, DiscordGuildListResult, DiscordLinkCheckResult,
    DiscordLinkStartResult, DiscordPermissionCheckResult, TelegramLoginCheckResult,
    TelegramLoginStartResult,
};
use crate::traits::SendMessage;
use async_trait::async_trait;
use serde_json::Value;

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
    ) -> anyhow::Result<ChannelConnectionResult>;

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
    ) -> anyhow::Result<ChannelConnectionResult> {
        self.backend
            .disconnect_channel(&self.config, channel, auth_mode, clear_memory)
            .await
    }

    pub async fn status(&self, channel: Option<&str>) -> anyhow::Result<Vec<ChannelStatusEntry>> {
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
            .send_message(&self.config, channel, message)
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
        self.backend
            .set_default_channel(&self.config, channel)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controllers::{ChannelThreadEntry, DiscordChannelEntry, DiscordGuildEntry};
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingBackend {
        calls: Mutex<Vec<String>>,
        credentials: Mutex<Vec<Value>>,
    }

    #[async_trait]
    impl ChannelBackend for RecordingBackend {
        async fn connect_channel(
            &self,
            _config: &ChannelsConfig,
            channel: &str,
            _auth_mode: ChannelAuthMode,
            credentials: Value,
        ) -> anyhow::Result<ChannelConnectionResult> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("connect:{channel}"));
            self.credentials.lock().unwrap().push(credentials);
            Ok(ChannelConnectionResult {
                status: "connected".into(),
                restart_required: false,
                auth_action: None,
                message: None,
            })
        }

        async fn disconnect_channel(
            &self,
            _config: &ChannelsConfig,
            _channel: &str,
            _auth_mode: ChannelAuthMode,
            _clear_memory: bool,
        ) -> anyhow::Result<ChannelConnectionResult> {
            unimplemented!("not used by this test")
        }

        async fn channel_status(
            &self,
            _config: &ChannelsConfig,
            _channel: Option<&str>,
        ) -> anyhow::Result<Vec<ChannelStatusEntry>> {
            unimplemented!("not used by this test")
        }

        async fn test_channel(
            &self,
            _config: &ChannelsConfig,
            channel: &str,
            _auth_mode: ChannelAuthMode,
            _credentials: Value,
        ) -> anyhow::Result<ChannelTestResult> {
            self.calls.lock().unwrap().push(format!("test:{channel}"));
            Ok(ChannelTestResult {
                success: true,
                message: format!("tested {channel}"),
            })
        }

        async fn send_message(
            &self,
            _config: &ChannelsConfig,
            channel: &str,
            message: SendMessage,
        ) -> anyhow::Result<ChannelSendMessageResult> {
            Ok(ChannelSendMessageResult {
                message_id: Some("msg-1".into()),
                raw: Some(serde_json::json!({
                "channel": channel,
                "content": message.content,
                })),
                ..Default::default()
            })
        }

        async fn send_reaction(
            &self,
            _config: &ChannelsConfig,
            _channel: &str,
            _reaction: Value,
        ) -> anyhow::Result<ChannelReactionResult> {
            Ok(ChannelReactionResult {
                success: true,
                ..Default::default()
            })
        }

        async fn create_thread(
            &self,
            _config: &ChannelsConfig,
            _channel: &str,
            title: &str,
        ) -> anyhow::Result<ChannelThreadResult> {
            Ok(ChannelThreadResult {
                thread_id: "thread-1".into(),
                title: Some(title.into()),
                raw: None,
            })
        }

        async fn update_thread(
            &self,
            _config: &ChannelsConfig,
            _channel: &str,
            thread_id: &str,
            _action: &str,
        ) -> anyhow::Result<ChannelThreadResult> {
            Ok(ChannelThreadResult {
                thread_id: thread_id.into(),
                title: None,
                raw: None,
            })
        }

        async fn list_threads(
            &self,
            _config: &ChannelsConfig,
            _channel: &str,
            _active: Option<bool>,
        ) -> anyhow::Result<ChannelThreadListResult> {
            Ok(ChannelThreadListResult {
                threads: vec![ChannelThreadEntry {
                    thread_id: "thread-1".into(),
                    title: Some("Demo".into()),
                    active: Some(true),
                    raw: None,
                }],
                raw: None,
            })
        }

        async fn telegram_login_start(
            &self,
            _config: &ChannelsConfig,
        ) -> anyhow::Result<TelegramLoginStartResult> {
            unimplemented!("not used by this test")
        }

        async fn telegram_login_check(
            &self,
            _config: &ChannelsConfig,
            _link_token: &str,
        ) -> anyhow::Result<TelegramLoginCheckResult> {
            unimplemented!("not used by this test")
        }

        async fn discord_link_start(
            &self,
            _config: &ChannelsConfig,
        ) -> anyhow::Result<DiscordLinkStartResult> {
            unimplemented!("not used by this test")
        }

        async fn discord_link_check(
            &self,
            _config: &ChannelsConfig,
            _link_token: &str,
        ) -> anyhow::Result<DiscordLinkCheckResult> {
            unimplemented!("not used by this test")
        }

        async fn discord_list_guilds(
            &self,
            _config: &ChannelsConfig,
        ) -> anyhow::Result<DiscordGuildListResult> {
            Ok(DiscordGuildListResult {
                guilds: vec![DiscordGuildEntry {
                    id: "guild-1".into(),
                    name: "Guild".into(),
                    raw: None,
                }],
                raw: None,
            })
        }

        async fn discord_list_channels(
            &self,
            _config: &ChannelsConfig,
            _guild_id: &str,
        ) -> anyhow::Result<DiscordChannelListResult> {
            Ok(DiscordChannelListResult {
                channels: vec![DiscordChannelEntry {
                    id: "channel-1".into(),
                    name: "general".into(),
                    kind: Some("text".into()),
                    raw: None,
                }],
                raw: None,
            })
        }

        async fn discord_check_permissions(
            &self,
            _config: &ChannelsConfig,
            _guild_id: &str,
            _channel_id: &str,
        ) -> anyhow::Result<DiscordPermissionCheckResult> {
            Ok(DiscordPermissionCheckResult {
                can_send_messages: true,
                missing_permissions: Vec::new(),
                raw: None,
            })
        }
    }

    #[tokio::test]
    async fn connect_validates_credentials_before_delegating() {
        let manager = ChannelManager::new(ChannelsConfig::default(), RecordingBackend::default());
        let err = manager
            .connect("telegram", ChannelAuthMode::BotToken, serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("bot_token"));

        manager
            .connect(
                "telegram",
                ChannelAuthMode::BotToken,
                serde_json::json!({ "bot_token": "123:abc" }),
            )
            .await
            .unwrap();
        assert_eq!(
            manager.backend.calls.lock().unwrap().as_slice(),
            ["connect:telegram"]
        );
    }

    #[tokio::test]
    async fn connect_normalizes_yuanbao_credentials_before_delegating() {
        let manager = ChannelManager::new(ChannelsConfig::default(), RecordingBackend::default());
        manager
            .connect(
                "yuanbao",
                ChannelAuthMode::ApiKey,
                serde_json::json!({
                    "app_key": "app",
                    "app_secret": "secret",
                    "bot_version": "openhuman/1.2.3"
                }),
            )
            .await
            .unwrap();

        let credentials = manager.backend.credentials.lock().unwrap();
        let sent = credentials.last().expect("recorded credentials");
        assert_eq!(sent["api_domain"], "https://bot.yuanbao.tencent.com");
        assert_eq!(
            sent["ws_domain"],
            "wss://bot-wss.yuanbao.tencent.com/wss/connection"
        );
        assert_eq!(sent["bot_version"], "1.2.3");
    }

    #[tokio::test]
    async fn test_validates_credentials_before_delegating() {
        let manager = ChannelManager::new(ChannelsConfig::default(), RecordingBackend::default());
        let err = manager
            .test("telegram", ChannelAuthMode::BotToken, serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("bot_token"));

        let result = manager
            .test(
                "telegram",
                ChannelAuthMode::BotToken,
                serde_json::json!({ "bot_token": "123:abc" }),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(
            manager.backend.calls.lock().unwrap().as_slice(),
            ["test:telegram"]
        );
    }

    #[tokio::test]
    async fn send_message_delegates_to_backend_with_config() {
        let manager = ChannelManager::new(ChannelsConfig::default(), RecordingBackend::default());
        let out = manager
            .send_message("telegram", SendMessage::new("hello", "alice"))
            .await
            .unwrap();
        let raw = out.raw.expect("raw send payload");
        assert_eq!(raw["channel"], "telegram");
        assert_eq!(raw["content"], "hello");
    }

    #[tokio::test]
    async fn manager_wraps_thread_and_discord_backend_methods() {
        let manager = ChannelManager::new(ChannelsConfig::default(), RecordingBackend::default());

        assert!(
            manager
                .send_reaction("discord", serde_json::json!({}))
                .await
                .unwrap()
                .success
        );
        assert_eq!(
            manager
                .create_thread("discord", "Demo")
                .await
                .unwrap()
                .thread_id,
            "thread-1"
        );
        assert_eq!(
            manager
                .update_thread("discord", "thread-2", "archive")
                .await
                .unwrap()
                .thread_id,
            "thread-2"
        );
        assert_eq!(
            manager
                .list_threads("discord", Some(true))
                .await
                .unwrap()
                .threads
                .len(),
            1
        );
        assert_eq!(
            manager.discord_list_guilds().await.unwrap().guilds[0].id,
            "guild-1"
        );
        assert_eq!(
            manager
                .discord_list_channels("guild-1")
                .await
                .unwrap()
                .channels[0]
                .id,
            "channel-1"
        );
        assert!(
            manager
                .discord_check_permissions("guild-1", "channel-1")
                .await
                .unwrap()
                .can_send_messages
        );
    }
}
