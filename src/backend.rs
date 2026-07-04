//! Backend abstraction for OpenHuman-owned channel operations.

use crate::config::ChannelsConfig;
use crate::controllers::{
    ChannelAuthMode, ChannelConnectionResult, ChannelDefinition, ChannelStatusEntry,
    ChannelTestResult, DiscordLinkCheckResult, DiscordLinkStartResult, TelegramLoginCheckResult,
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
    ) -> anyhow::Result<Value>;

    async fn send_reaction(
        &self,
        config: &ChannelsConfig,
        channel: &str,
        reaction: Value,
    ) -> anyhow::Result<Value>;

    async fn create_thread(
        &self,
        config: &ChannelsConfig,
        channel: &str,
        title: &str,
    ) -> anyhow::Result<Value>;

    async fn update_thread(
        &self,
        config: &ChannelsConfig,
        channel: &str,
        thread_id: &str,
        action: &str,
    ) -> anyhow::Result<Value>;

    async fn list_threads(
        &self,
        config: &ChannelsConfig,
        channel: &str,
        active: Option<bool>,
    ) -> anyhow::Result<Value>;

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

    async fn discord_list_guilds(&self, config: &ChannelsConfig) -> anyhow::Result<Value>;

    async fn discord_list_channels(
        &self,
        config: &ChannelsConfig,
        guild_id: &str,
    ) -> anyhow::Result<Value>;

    async fn discord_check_permissions(
        &self,
        config: &ChannelsConfig,
        guild_id: &str,
        channel_id: &str,
    ) -> anyhow::Result<Value>;

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
        self.backend
            .test_channel(&self.config, channel, auth_mode, credentials)
            .await
    }

    pub async fn send_message(&self, channel: &str, message: SendMessage) -> anyhow::Result<Value> {
        self.backend
            .send_message(&self.config, channel, message)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingBackend {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ChannelBackend for RecordingBackend {
        async fn connect_channel(
            &self,
            _config: &ChannelsConfig,
            channel: &str,
            _auth_mode: ChannelAuthMode,
            _credentials: Value,
        ) -> anyhow::Result<ChannelConnectionResult> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("connect:{channel}"));
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
            _channel: &str,
            _auth_mode: ChannelAuthMode,
            _credentials: Value,
        ) -> anyhow::Result<ChannelTestResult> {
            unimplemented!("not used by this test")
        }

        async fn send_message(
            &self,
            _config: &ChannelsConfig,
            channel: &str,
            message: SendMessage,
        ) -> anyhow::Result<Value> {
            Ok(serde_json::json!({
                "channel": channel,
                "content": message.content,
            }))
        }

        async fn send_reaction(
            &self,
            _config: &ChannelsConfig,
            _channel: &str,
            _reaction: Value,
        ) -> anyhow::Result<Value> {
            unimplemented!("not used by this test")
        }

        async fn create_thread(
            &self,
            _config: &ChannelsConfig,
            _channel: &str,
            _title: &str,
        ) -> anyhow::Result<Value> {
            unimplemented!("not used by this test")
        }

        async fn update_thread(
            &self,
            _config: &ChannelsConfig,
            _channel: &str,
            _thread_id: &str,
            _action: &str,
        ) -> anyhow::Result<Value> {
            unimplemented!("not used by this test")
        }

        async fn list_threads(
            &self,
            _config: &ChannelsConfig,
            _channel: &str,
            _active: Option<bool>,
        ) -> anyhow::Result<Value> {
            unimplemented!("not used by this test")
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

        async fn discord_list_guilds(&self, _config: &ChannelsConfig) -> anyhow::Result<Value> {
            unimplemented!("not used by this test")
        }

        async fn discord_list_channels(
            &self,
            _config: &ChannelsConfig,
            _guild_id: &str,
        ) -> anyhow::Result<Value> {
            unimplemented!("not used by this test")
        }

        async fn discord_check_permissions(
            &self,
            _config: &ChannelsConfig,
            _guild_id: &str,
            _channel_id: &str,
        ) -> anyhow::Result<Value> {
            unimplemented!("not used by this test")
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
    async fn send_message_delegates_to_backend_with_config() {
        let manager = ChannelManager::new(ChannelsConfig::default(), RecordingBackend::default());
        let out = manager
            .send_message("telegram", SendMessage::new("hello", "alice"))
            .await
            .unwrap();
        assert_eq!(out["channel"], "telegram");
        assert_eq!(out["content"], "hello");
    }
}
