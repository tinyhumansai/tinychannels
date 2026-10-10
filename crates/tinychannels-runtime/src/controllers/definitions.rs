//! Runtime connection and credential validation helpers.
use crate::config::ChannelsConfig;
pub use tinychannels_bus::controllers::definitions::*;
/// Validate provider credentials against the declared schema.
pub trait ChannelDefinitionExt {
    fn validate_credentials(
        &self,
        mode: ChannelAuthMode,
        credentials: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String>;
}
impl ChannelDefinitionExt for ChannelDefinition {
    /// Validate that `credentials` contains all required fields for `mode`.
    /// Returns `Ok(())` or an error listing missing fields.
    fn validate_credentials(
        &self,
        mode: ChannelAuthMode,
        credentials: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        let spec = self.auth_mode_spec(mode).ok_or_else(|| {
            format!(
                "channel '{}' does not support auth mode '{}'",
                self.id, mode
            )
        })?;

        let missing: Vec<&str> = spec
            .fields
            .iter()
            .filter(|f| f.required)
            .filter(|f| {
                credentials
                    .get(f.key)
                    .is_none_or(|v| v.as_str().is_some_and(|s| s.is_empty()))
            })
            .map(|f| f.key)
            .collect();

        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "missing required fields for {}.{}: {}",
                self.id,
                mode,
                missing.join(", ")
            ))
        }
    }
}
/// Whether the supplied channel/auth-mode is connected by runtime config.
pub fn channel_config_connected(
    channels: &ChannelsConfig,
    channel_id: &str,
    mode: ChannelAuthMode,
) -> bool {
    match (channel_id, mode) {
        ("telegram", ChannelAuthMode::BotToken) => channels.telegram.is_some(),
        ("discord", ChannelAuthMode::BotToken) => channels.discord.is_some(),
        ("slack", _) => channels.slack.is_some(),
        ("mattermost", _) => channels.mattermost.is_some(),
        ("imessage", ChannelAuthMode::ManagedDm) => channels.imessage.is_some(),
        ("matrix", _) => channels.matrix.is_some(),
        ("signal", _) => channels.signal.is_some(),
        ("whatsapp", _) => channels.whatsapp.is_some(),
        ("linq", _) => channels.linq.is_some(),
        ("email", _) => channels.email.is_some(),
        ("irc", _) => channels.irc.is_some(),
        ("lark", _) => channels.lark.is_some(),
        ("dingtalk", _) => channels.dingtalk.is_some(),
        ("qq", _) => channels.qq.is_some(),
        ("yuanbao", ChannelAuthMode::ApiKey) => channels.yuanbao.is_some(),
        _ => false,
    }
}

#[cfg(test)]
#[path = "definitions_tests.rs"]
mod tests;
