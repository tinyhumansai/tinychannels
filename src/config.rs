//! Channel provider configuration for portable OpenHuman channel surfaces.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ChannelsConfig {
    pub cli: bool,
    pub telegram: Option<TelegramConfig>,
    pub discord: Option<DiscordConfig>,
    pub slack: Option<SlackConfig>,
    pub mattermost: Option<MattermostConfig>,
    pub webhook: Option<WebhookConfig>,
    pub imessage: Option<IMessageConfig>,
    pub matrix: Option<MatrixConfig>,
    pub signal: Option<SignalConfig>,
    pub whatsapp: Option<WhatsAppConfig>,
    pub linq: Option<LinqConfig>,
    pub email: Option<EmailConfig>,
    pub irc: Option<IrcConfig>,
    pub lark: Option<LarkConfig>,
    pub dingtalk: Option<DingTalkConfig>,
    pub qq: Option<QQConfig>,
    pub yuanbao: Option<YuanbaoConfig>,
    #[serde(default = "default_channel_message_timeout_secs")]
    pub message_timeout_secs: u64,
    /// The user's preferred *external* channel for proactive messages
    /// (morning briefings, welcome messages, cron output, etc.).
    ///
    /// Delivery is **web-first, then mirror**: the proactive message
    /// handler should deliver to the in-app web channel first, then send a
    /// copy to this external channel if it is set and connected. When `None`
    /// or `"web"`, only the web channel receives the message.
    ///
    /// Valid values: any channel name (`"telegram"`, `"discord"`,
    /// `"slack"`, etc.) or `None` for web-only delivery.
    #[serde(default)]
    pub active_channel: Option<String>,
}

fn default_channel_message_timeout_secs() -> u64 {
    300
}

impl ChannelsConfig {
    /// Whether any configured integration needs a listener runtime.
    /// Used to avoid spawning the channel runtime when only RPC/outbound paths are needed.
    ///
    /// `webhook` is intentionally omitted: it is push-based and owned by the
    /// host HTTP server, so enabling it should not spawn a polling/listener
    /// worker from the channel runtime.
    pub fn has_listening_integrations(&self) -> bool {
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
    }
}

impl Default for ChannelsConfig {
    fn default() -> Self {
        Self {
            cli: true,
            telegram: None,
            discord: None,
            slack: None,
            mattermost: None,
            webhook: None,
            imessage: None,
            matrix: None,
            signal: None,
            whatsapp: None,
            linq: None,
            email: None,
            irc: None,
            lark: None,
            dingtalk: None,
            qq: None,
            yuanbao: None,
            message_timeout_secs: default_channel_message_timeout_secs(),
            active_channel: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StreamMode {
    #[default]
    Off,
    Partial,
}

pub(crate) fn default_draft_update_interval_ms() -> u64 {
    1000
}

fn default_silent_streaming() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TelegramConfig {
    pub bot_token: String,
    /// Default chat for recipient-less *proactive* sends (morning briefings,
    /// cron output, etc.). Mirrors `DiscordConfig::channel_id`: `None` ⇒ proactive
    /// routing skips Telegram rather than POSTing to an empty `chat_id`.
    #[serde(default)]
    pub chat_id: Option<String>,
    pub allowed_users: Vec<String>,
    #[serde(default)]
    pub stream_mode: StreamMode,
    #[serde(default = "default_draft_update_interval_ms")]
    pub draft_update_interval_ms: u64,
    #[serde(default = "default_silent_streaming")]
    pub silent_streaming: bool,
    #[serde(default)]
    pub mention_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscordConfig {
    pub bot_token: String,
    pub guild_id: Option<String>,
    pub channel_id: Option<String>,
    #[serde(default)]
    pub allowed_users: Vec<String>,
    #[serde(default)]
    pub listen_to_bots: bool,
    #[serde(default)]
    pub mention_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SlackConfig {
    pub bot_token: String,
    pub app_token: Option<String>,
    pub channel_id: Option<String>,
    #[serde(default)]
    pub allowed_users: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MattermostConfig {
    pub url: String,
    pub bot_token: String,
    pub channel_id: Option<String>,
    #[serde(default)]
    pub allowed_users: Vec<String>,
    #[serde(default)]
    pub thread_replies: Option<bool>,
    #[serde(default)]
    pub mention_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebhookConfig {
    pub port: u16,
    pub secret: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IMessageConfig {
    pub allowed_contacts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MatrixConfig {
    pub homeserver: String,
    pub access_token: String,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    pub room_id: String,
    pub allowed_users: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SignalConfig {
    pub http_url: String,
    pub account: String,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub allowed_from: Vec<String>,
    #[serde(default)]
    pub ignore_attachments: bool,
    #[serde(default)]
    pub ignore_stories: bool,
}

/// Email channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmailConfig {
    /// IMAP server hostname.
    pub imap_host: String,
    /// IMAP server port (default: 993 for TLS).
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    /// IMAP folder to poll (default: INBOX).
    #[serde(default = "default_imap_folder")]
    pub imap_folder: String,
    /// SMTP server hostname.
    pub smtp_host: String,
    /// SMTP server port (default: 465 for TLS).
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    /// Use TLS for SMTP (default: true).
    #[serde(default = "default_true")]
    pub smtp_tls: bool,
    /// Email username for authentication.
    pub username: String,
    /// Email password for authentication.
    pub password: String,
    /// From address for outgoing emails.
    pub from_address: String,
    /// IDLE timeout in seconds before re-establishing connection.
    #[serde(default = "default_idle_timeout", alias = "poll_interval_secs")]
    pub idle_timeout_secs: u64,
    /// Allowed sender addresses/domains (empty = deny all, ["*"] = allow all).
    #[serde(default)]
    pub allowed_senders: Vec<String>,
}

fn default_imap_port() -> u16 {
    993
}

fn default_smtp_port() -> u16 {
    465
}

fn default_imap_folder() -> String {
    "INBOX".into()
}

fn default_idle_timeout() -> u64 {
    1740
}

fn default_true() -> bool {
    true
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            imap_host: String::new(),
            imap_port: default_imap_port(),
            imap_folder: default_imap_folder(),
            smtp_host: String::new(),
            smtp_port: default_smtp_port(),
            smtp_tls: true,
            username: String::new(),
            password: String::new(),
            from_address: String::new(),
            idle_timeout_secs: default_idle_timeout(),
            allowed_senders: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WhatsAppConfig {
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub phone_number_id: Option<String>,
    #[serde(default)]
    pub verify_token: Option<String>,
    #[serde(default)]
    pub app_secret: Option<String>,
    #[serde(default)]
    pub session_path: Option<String>,
    #[serde(default)]
    pub pair_phone: Option<String>,
    #[serde(default)]
    pub pair_code: Option<String>,
    #[serde(default)]
    pub allowed_numbers: Vec<String>,
}

impl WhatsAppConfig {
    pub fn backend_type(&self) -> &'static str {
        if self.phone_number_id.is_some() {
            "cloud"
        } else if self.session_path.is_some() {
            "web"
        } else {
            "unconfigured"
        }
    }

    pub fn is_cloud_config(&self) -> bool {
        self.phone_number_id.is_some() && self.access_token.is_some() && self.verify_token.is_some()
    }

    pub fn is_web_config(&self) -> bool {
        self.session_path.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LinqConfig {
    pub api_token: String,
    pub from_phone: String,
    #[serde(default)]
    pub allowed_senders: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IrcConfig {
    pub server: String,
    #[serde(default = "default_irc_port")]
    pub port: u16,
    pub nickname: String,
    pub username: Option<String>,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub allowed_users: Vec<String>,
    pub server_password: Option<String>,
    pub nickserv_password: Option<String>,
    pub sasl_password: Option<String>,
    pub verify_tls: Option<bool>,
}

fn default_irc_port() -> u16 {
    6697
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LarkReceiveMode {
    #[default]
    Websocket,
    Webhook,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LarkConfig {
    pub app_id: String,
    pub app_secret: String,
    #[serde(default)]
    pub encrypt_key: Option<String>,
    #[serde(default)]
    pub verification_token: Option<String>,
    #[serde(default)]
    pub allowed_users: Vec<String>,
    #[serde(default)]
    pub use_feishu: bool,
    #[serde(default)]
    pub receive_mode: LarkReceiveMode,
    #[serde(default)]
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DingTalkConfig {
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub allowed_users: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QQConfig {
    pub app_id: String,
    pub app_secret: String,
    #[serde(default)]
    pub allowed_users: Vec<String>,
}

/// Yuanbao channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct YuanbaoConfig {
    pub app_key: String,
    pub app_secret: String,
    #[serde(default)]
    pub bot_id: String,
    #[serde(default = "default_yuanbao_env")]
    pub env: String,
    #[serde(default)]
    pub api_domain: String,
    #[serde(default)]
    pub ws_domain: String,
    #[serde(default)]
    pub route_env: String,
    #[serde(default)]
    pub token: String,
    #[serde(default = "default_yuanbao_bot_version", alias = "plugin_version")]
    pub bot_version: String,
    #[serde(default)]
    pub bot_name: String,
    #[serde(default = "default_yuanbao_dm_policy")]
    pub dm_access: String,
    #[serde(default = "default_yuanbao_group_policy")]
    pub group_access: String,
    #[serde(default)]
    pub allowed_users: Vec<String>,
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    #[serde(default)]
    pub owner_id: String,
    #[serde(default = "default_true")]
    pub group_at_required: bool,
    #[serde(default)]
    pub heartbeat_interval_secs: u64,
    #[serde(default)]
    pub max_reconnect_attempts: u32,
    #[serde(default = "default_yuanbao_max_msg_len")]
    pub max_message_length: usize,
    #[serde(default = "default_yuanbao_max_media_mb")]
    pub max_media_mb: u32,
}

impl Default for YuanbaoConfig {
    fn default() -> Self {
        Self {
            app_key: String::new(),
            app_secret: String::new(),
            bot_id: String::new(),
            env: default_yuanbao_env(),
            api_domain: String::new(),
            ws_domain: String::new(),
            route_env: String::new(),
            token: String::new(),
            bot_version: default_yuanbao_bot_version(),
            bot_name: String::new(),
            dm_access: default_yuanbao_dm_policy(),
            group_access: default_yuanbao_group_policy(),
            allowed_users: Vec::new(),
            allowed_groups: Vec::new(),
            owner_id: String::new(),
            group_at_required: true,
            heartbeat_interval_secs: 0,
            max_reconnect_attempts: 0,
            max_message_length: default_yuanbao_max_msg_len(),
            max_media_mb: default_yuanbao_max_media_mb(),
        }
    }
}

impl YuanbaoConfig {
    pub fn apply_env_defaults(&mut self) {
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

    pub fn validate(&self) -> Result<(), String> {
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

fn default_yuanbao_env() -> String {
    "prod".into()
}

fn default_yuanbao_bot_version() -> String {
    "0.1.0".into()
}

fn default_yuanbao_dm_policy() -> String {
    "open".into()
}

fn default_yuanbao_group_policy() -> String {
    "allowlist".into()
}

fn default_yuanbao_max_msg_len() -> usize {
    4500
}

fn default_yuanbao_max_media_mb() -> u32 {
    50
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
