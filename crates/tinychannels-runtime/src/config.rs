//! Runtime configuration preparation, preserving shared bus DTOs.
use crate::relay::RelayIdentity;
pub use tinychannels_bus::config::*;

/// Runtime preparation and classification for [`ChannelsConfig`].
pub trait ChannelsConfigExt {
    fn has_listening_integrations(&self) -> bool;
}
impl ChannelsConfigExt for ChannelsConfig {
    /// Whether any configured integration needs a listener runtime.
    /// Used to avoid spawning the channel runtime when only RPC/outbound paths are needed.
    ///
    /// `webhook` is intentionally omitted: it is push-based and owned by the
    /// host HTTP server, so enabling it should not spawn a polling/listener
    /// worker from the channel runtime.
    ///
    fn has_listening_integrations(&self) -> bool {
        self.telegram.is_some()
            || self.discord.is_some()
            || self.slack.is_some()
            || self.mattermost.is_some()
            || self.imessage.is_some()
            || self.signal.is_some()
            || self.linq.is_some()
            || self.email.is_some()
            || self.irc.is_some()
            || self.lark.is_some()
            || self.dingtalk.is_some()
            || self.qq.is_some()
            || self.yuanbao.is_some()
            || self.matrix.is_some()
            || self.whatsapp.is_some()
            || self
                .relay
                .as_ref()
                .is_some_and(RelayRuntimeConfig::is_listener_configured)
    }
}

/// Runtime preparation and classification for [`RelayRuntimeConfig`].
pub trait RelayRuntimeConfigExt {
    fn is_listener_configured(&self) -> bool;
    fn relay_identities(&self) -> Vec<crate::relay::RelayIdentity>;
}
impl RelayRuntimeConfigExt for RelayRuntimeConfig {
    fn is_listener_configured(&self) -> bool {
        !self.url.trim().is_empty() && !self.identities.is_empty()
    }

    fn relay_identities(&self) -> Vec<RelayIdentity> {
        self.identities
            .iter()
            .map(|identity| RelayIdentity {
                platform: identity.platform.clone(),
                bot_id: identity.bot_id.clone(),
            })
            .collect()
    }
}

/// Runtime preparation and classification for [`WhatsAppConfig`].
pub trait WhatsAppConfigExt {
    fn backend_type(&self) -> &'static str;
    fn is_cloud_config(&self) -> bool;
    fn is_web_config(&self) -> bool;
}
impl WhatsAppConfigExt for WhatsAppConfig {
    fn backend_type(&self) -> &'static str {
        if self.phone_number_id.is_some() {
            "cloud"
        } else if self.session_path.is_some() {
            "web"
        } else {
            "unconfigured"
        }
    }

    fn is_cloud_config(&self) -> bool {
        self.phone_number_id.is_some() && self.access_token.is_some() && self.verify_token.is_some()
    }

    fn is_web_config(&self) -> bool {
        self.session_path.is_some()
    }
}

/// Runtime preparation and classification for [`YuanbaoConfig`].
pub trait YuanbaoConfigExt {
    fn apply_env_defaults(&mut self);
    fn validate(&self) -> Result<(), String>;
}
impl YuanbaoConfigExt for YuanbaoConfig {
    fn apply_env_defaults(&mut self) {
        if self.api_domain.is_empty() {
            self.api_domain = match self.env.as_str() {
                "pre" => "https://bot-pre.yuanbao.tencent.com".into(),
                _ => "https://bot.yuanbao.tencent.com".into(),
            };
        }
        if self.ws_domain.is_empty() {
            self.ws_domain = match self.env.as_str() {
                "pre" => "wss://bot-wss-pre.yuanbao.tencent.com/wss/connection".into(),
                _ => "wss://bot-wss.yuanbao.tencent.com/wss/connection".into(),
            };
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.app_key.is_empty() {
            return Err("`app_key` is required".into());
        }
        if self.ws_domain.is_empty() {
            return Err("`ws_domain` is required".into());
        }
        if self.token.is_empty() && self.app_secret.is_empty() {
            return Err("either `token` or `app_secret` must be set".into());
        }
        if self.api_domain.is_empty() && self.token.is_empty() {
            return Err("`api_domain` is required when `token` is not pre-provisioned".into());
        }
        Ok(())
    }
}

pub fn strip_yuanbao_version_prefix(version: &str) -> &str {
    version.strip_prefix("openhuman/").unwrap_or(version)
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
