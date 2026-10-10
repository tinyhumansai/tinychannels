//! Construct the configured provider set from a [`ChannelsConfig`].
//!
//! This is the portable half of channel startup: read config, build providers.
//! It lives here rather than in a host because it is the same for every host —
//! and because a second copy is how the two drift. The module
//! (`tinychannels-module`) and the OpenHuman desktop core both call it.
//!
//! # What this function deliberately does NOT do
//!
//! Three things in channel startup are host policy and stay with the caller:
//!
//! - **Credential hydration.** Secrets may live in a keyring, an environment
//!   variable or the config file, and only the host knows which. Hydrate
//!   `config` *before* calling this — a provider whose token is still a
//!   placeholder is built here without complaint, because this function cannot
//!   tell a deliberately-empty token from an unresolved one.
//! - **HTTP transport.** Proxy configuration, TLS backend and timeouts belong to
//!   the host's runtime, so they arrive as a [`HttpClientFactory`].
//! - **Host capabilities.** Speech-to-text, the persisted allowlist, the event
//!   sink and the lifecycle hook come from [`ChannelHost`], which a host
//!   implements. [`NoopHost`](crate::NoopHost) is the correct argument when
//!   there are none; each capability is optional and every provider degrades
//!   rather than failing without one.
//!
//! # Misconfiguration warns, it does not fail
//!
//! A provider whose config is present but unusable (WhatsApp with neither a
//! Cloud nor a Web shape, Yuanbao with an invalid secret) is logged and skipped.
//! Returning a `Result` would mean one bad stanza takes down every other
//! channel, which is the wrong trade for a long-running messaging surface.

use std::sync::Arc;
use tinychannels_runtime::config::WhatsAppConfigExt as _;

use crate::host::ChannelHost;
use crate::providers::irc;
use crate::{
    Channel, DingTalkChannel, DiscordChannel, IMessageChannel, IrcChannel, LinqChannel,
    MattermostChannel, QQChannel, SignalChannel, SlackChannel, TelegramChannel, WhatsAppChannel,
    YuanbaoChannel,
};
use tinychannels_bus::ChannelsConfig;

/// Supplies the HTTP client each network-backed provider should use.
///
/// A host implements this to apply its own proxy settings, TLS backend and
/// timeouts. The `channel` argument is a stable, dotted identifier
/// (`"channel.telegram"`, `"channel.discord"`, …) so a host can vary policy per
/// provider; most return the same client for all of them.
pub trait HttpClientFactory: Send + Sync {
    /// The client for `channel`.
    fn client_for(&self, channel: &str) -> reqwest::Client;

    /// The client for the Signal provider, which wants a connect timeout.
    ///
    /// Split out because Signal talks to a local `signal-cli` HTTP bridge that
    /// may simply not be running; without a connect timeout that presents as a
    /// hang at startup rather than an error.
    fn signal_client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }
}

/// A [`HttpClientFactory`] with no proxy and default timeouts.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultHttpClients;

impl HttpClientFactory for DefaultHttpClients {
    fn client_for(&self, _channel: &str) -> reqwest::Client {
        reqwest::Client::new()
    }
}

/// Build every provider that `config` enables.
///
/// Order is stable and matches the declaration order below, so a caller that
/// indexes the result gets the same provider across runs.
///
/// See the module docs for the three things the caller must do first.
#[must_use]
pub fn build_channels(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
    http: &dyn HttpClientFactory,
) -> Vec<Arc<dyn Channel>> {
    let mut channels: Vec<Arc<dyn Channel>> = Vec::new();

    if let Some(tg) = config.telegram.as_ref() {
        tracing::info!(
            channel = "telegram",
            allowed_users_count = tg.allowed_users.len(),
            mention_only = tg.mention_only,
            "[channels] telegram enabled (bot token not logged)"
        );
        let mut telegram = TelegramChannel::new(
            tg.bot_token.clone(),
            tg.allowed_users.clone(),
            tg.mention_only,
        )
        .with_streaming(
            tg.stream_mode,
            tg.draft_update_interval_ms,
            tg.silent_streaming,
        )
        .with_chat_id(tg.chat_id.clone())
        .with_http_client(http.client_for("channel.telegram"));
        // Each is optional — telegram degrades gracefully without them.
        if let Some(transcriber) = host.transcriber() {
            telegram = telegram.with_transcriber(transcriber);
        }
        if let Some(allowlist) = host.allowlist() {
            telegram = telegram.with_allowlist(allowlist);
        }
        if let Some(events) = host.events() {
            telegram = telegram.with_events(events);
        }
        channels.push(Arc::new(telegram));
    }

    if let Some(dc) = config.discord.as_ref() {
        channels.push(Arc::new(DiscordChannel::with_http_client(
            dc.bot_token.clone(),
            dc.guild_id.clone(),
            dc.channel_id.clone(),
            dc.allowed_users.clone(),
            dc.listen_to_bots,
            dc.mention_only,
            http.client_for("channel.discord"),
        )));
    }

    if let Some(sl) = config.slack.as_ref() {
        channels.push(Arc::new(SlackChannel::with_http_client(
            sl.bot_token.clone(),
            sl.channel_id.clone(),
            sl.allowed_users.clone(),
            http.client_for("channel.slack"),
        )));
    }

    if let Some(mm) = config.mattermost.as_ref() {
        channels.push(Arc::new(MattermostChannel::with_http_client(
            mm.url.clone(),
            mm.bot_token.clone(),
            mm.channel_id.clone(),
            mm.allowed_users.clone(),
            mm.thread_replies.unwrap_or(true),
            mm.mention_only.unwrap_or(false),
            http.client_for("channel.mattermost"),
        )));
    }

    if let Some(im) = config.imessage.as_ref() {
        channels.push(Arc::new(IMessageChannel::new(im.allowed_contacts.clone())));
    }

    if config.matrix.is_some() {
        tracing::warn!(
            "[channels] matrix is configured but Matrix support was removed from this build; skipping"
        );
    }

    if let Some(sig) = config.signal.as_ref() {
        channels.push(Arc::new(SignalChannel::with_http_client(
            sig.http_url.clone(),
            sig.account.clone(),
            sig.group_id.clone(),
            sig.allowed_from.clone(),
            sig.ignore_attachments,
            sig.ignore_stories,
            http.signal_client(),
        )));
    }

    if let Some(wa) = config.whatsapp.as_ref() {
        // Runtime negotiation: the same stanza can describe either backend, so
        // the shape of the config decides which one is meant.
        match wa.backend_type() {
            "cloud" if wa.is_cloud_config() => {
                channels.push(Arc::new(WhatsAppChannel::with_http_client(
                    wa.access_token.clone().unwrap_or_default(),
                    wa.phone_number_id.clone().unwrap_or_default(),
                    wa.verify_token.clone().unwrap_or_default(),
                    wa.allowed_numbers.clone(),
                    http.client_for("channel.whatsapp"),
                )));
            }
            "cloud" => tracing::warn!(
                "[channels] whatsapp cloud configured but missing phone_number_id, access_token or verify_token"
            ),
            "web" => channels.extend(build_whatsapp_web(wa, host)),
            _ => tracing::warn!(
                "[channels] whatsapp config invalid: neither phone_number_id (Cloud API) nor session_path (Web) is set"
            ),
        }
    }

    if let Some(lq) = config.linq.as_ref() {
        channels.push(Arc::new(LinqChannel::with_http_client(
            lq.api_token.clone(),
            lq.from_phone.clone(),
            lq.allowed_senders.clone(),
            http.client_for("channel.linq"),
        )));
    }

    #[cfg(feature = "email")]
    if let Some(email_cfg) = config.email.as_ref() {
        channels.push(Arc::new(crate::providers::EmailChannel::new(
            email_cfg.clone(),
        )));
    }

    if let Some(irc_cfg) = config.irc.as_ref() {
        channels.push(Arc::new(IrcChannel::new(irc::IrcChannelConfig {
            server: irc_cfg.server.clone(),
            port: irc_cfg.port,
            nickname: irc_cfg.nickname.clone(),
            username: irc_cfg.username.clone(),
            channels: irc_cfg.channels.clone(),
            allowed_users: irc_cfg.allowed_users.clone(),
            server_password: irc_cfg.server_password.clone(),
            nickserv_password: irc_cfg.nickserv_password.clone(),
            sasl_password: irc_cfg.sasl_password.clone(),
            verify_tls: irc_cfg.verify_tls.unwrap_or(true),
        })));
    }

    #[cfg(feature = "lark")]
    if let Some(lk) = config.lark.as_ref() {
        channels.push(Arc::new(crate::providers::LarkChannel::from_config(lk)));
    }

    if let Some(dt) = config.dingtalk.as_ref() {
        channels.push(Arc::new(DingTalkChannel::with_http_client(
            dt.client_id.clone(),
            dt.client_secret.clone(),
            dt.allowed_users.clone(),
            http.client_for("channel.dingtalk"),
        )));
    }

    if let Some(qq) = config.qq.as_ref() {
        channels.push(Arc::new(QQChannel::with_http_client(
            qq.app_id.clone(),
            qq.app_secret.clone(),
            qq.allowed_users.clone(),
            http.client_for("channel.qq"),
        )));
    }

    if let Some(yb) = config.yuanbao.as_ref() {
        match YuanbaoChannel::new(yb.clone()) {
            Ok(channel) => channels.push(Arc::new(channel)),
            Err(error) => tracing::warn!("[channels] yuanbao config invalid: {error}"),
        }
    }

    channels
}

/// The WhatsApp Web arm, split out so the feature gate does not sit inside a
/// `match` arm where the `else` branch would need its own `#[cfg]`.
///
/// Returns rather than pushing into the caller's vector: with the gate off the
/// body can never produce a provider, and a `&mut Vec` parameter that is never
/// written to is both a clippy warning and a lie about what the function does.
#[allow(
    unused_variables,
    reason = "both arguments are unused when the gate is off"
)]
fn build_whatsapp_web(
    wa: &crate::config::WhatsAppConfig,
    host: &Arc<dyn ChannelHost>,
) -> Option<Arc<dyn Channel>> {
    #[cfg(feature = "whatsapp-web")]
    {
        if !wa.is_web_config() {
            tracing::warn!("[channels] whatsapp web configured but session_path not set");
            return None;
        }
        let mut channel = crate::WhatsAppWebChannel::new(
            wa.session_path.clone().unwrap_or_default(),
            wa.pair_phone.clone(),
            wa.pair_code.clone(),
            wa.allowed_numbers.clone(),
        );
        if let Some(lifecycle) = host.lifecycle() {
            channel = channel.with_lifecycle(lifecycle);
        }
        Some(Arc::new(channel))
    }
    #[cfg(not(feature = "whatsapp-web"))]
    {
        tracing::warn!(
            "[channels] whatsapp web backend requires the `whatsapp-web` feature; rebuild with it enabled"
        );
        None
    }
}

#[cfg(test)]
#[path = "factory_tests.rs"]
mod factory_tests;
