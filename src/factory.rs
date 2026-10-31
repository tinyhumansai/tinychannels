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

#[cfg(feature = "imessage")]
use crate::IMessageChannel;
use crate::channel::ChannelAdapter;
use crate::host::ChannelHost;
use crate::providers::irc;
use crate::providers::long_tail::LongTailNative;
use crate::{
    DingTalkChannel, DiscordChannel, IrcChannel, LinqChannel, MattermostChannel, QQChannel,
    SignalChannel, SlackChannel, TelegramChannel, WhatsAppChannel, YuanbaoChannel,
};
use tinychannels_bus::ChannelsConfig;
use tinychannels_bus::HttpProxyPolicy;

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

/// A module-owned HTTP factory configured by its embedding host.
///
/// The policy is received over a confidential module member because URLs may
/// include credentials. Error text deliberately excludes the supplied URLs.
pub struct ConfiguredHttpClients {
    policy: HttpProxyPolicy,
    default_client: reqwest::Client,
    signal_default_client: reqwest::Client,
    proxy_client: reqwest::Client,
    signal_proxy_client: reqwest::Client,
}

impl ConfiguredHttpClients {
    /// Validate and retain a host-provided proxy policy.
    pub fn new(policy: HttpProxyPolicy) -> anyhow::Result<Self> {
        for (label, value) in [
            ("HTTP", policy.http_proxy.as_deref()),
            ("HTTPS", policy.https_proxy.as_deref()),
            ("all-schemes", policy.all_proxy.as_deref()),
        ] {
            let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
                continue;
            };
            if value.len() > 4096 || reqwest::Proxy::all(value).is_err() {
                anyhow::bail!("invalid {label} proxy URL");
            }
        }
        if policy.no_proxy.len() > 128
            || policy.no_proxy.iter().any(|entry| entry.len() > 1024)
            || policy.service_selectors.len() > 64
            || policy
                .service_selectors
                .iter()
                .any(|selector| selector.len() > 128 || selector.trim().is_empty())
        {
            anyhow::bail!("HTTP proxy policy exceeds its limit");
        }
        let default_client = build_client(None, false)?;
        let signal_default_client = build_client(None, true)?;
        let proxy_client = build_client(Some(&policy), false)?;
        let signal_proxy_client = build_client(Some(&policy), true)?;
        Ok(Self {
            policy,
            default_client,
            signal_default_client,
            proxy_client,
            signal_proxy_client,
        })
    }

    fn applies_to(&self, service: &str) -> bool {
        self.policy.service_selectors.iter().any(|selector| {
            if let Some(prefix) = selector.strip_suffix("*") {
                service.starts_with(prefix)
            } else {
                selector == service
            }
        })
    }

    fn client(&self, service: &str, signal: bool) -> reqwest::Client {
        let proxied = self.applies_to(service);
        match (proxied, signal) {
            (false, false) => self.default_client.clone(),
            (false, true) => self.signal_default_client.clone(),
            (true, false) => self.proxy_client.clone(),
            (true, true) => self.signal_proxy_client.clone(),
        }
    }
}

fn build_client(policy: Option<&HttpProxyPolicy>, signal: bool) -> anyhow::Result<reqwest::Client> {
    let builder = reqwest::Client::builder();
    let builder = if signal {
        builder.connect_timeout(std::time::Duration::from_secs(10))
    } else {
        builder
    };
    let Some(policy) = policy else {
        return builder
            .build()
            .map_err(|_| anyhow::anyhow!("failed to build HTTP client"));
    };
    let no_proxy = policy
        .no_proxy
        .iter()
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>()
        .join(",");
    let no_proxy = reqwest::NoProxy::from_string(&no_proxy);
    let mut builder = builder;
    if let Some(value) = policy
        .all_proxy
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let proxy = reqwest::Proxy::all(value)
            .map_err(|_| anyhow::anyhow!("invalid all-schemes proxy URL"))?;
        builder = builder.proxy(proxy.no_proxy(no_proxy.clone()));
    }
    if let Some(value) = policy
        .http_proxy
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let proxy =
            reqwest::Proxy::http(value).map_err(|_| anyhow::anyhow!("invalid HTTP proxy URL"))?;
        builder = builder.proxy(proxy.no_proxy(no_proxy.clone()));
    }
    if let Some(value) = policy
        .https_proxy
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let proxy =
            reqwest::Proxy::https(value).map_err(|_| anyhow::anyhow!("invalid HTTPS proxy URL"))?;
        builder = builder.proxy(proxy.no_proxy(no_proxy));
    }
    builder
        .build()
        .map_err(|_| anyhow::anyhow!("failed to build HTTP client"))
}

impl HttpClientFactory for ConfiguredHttpClients {
    fn client_for(&self, channel: &str) -> reqwest::Client {
        self.client(channel, false)
    }

    fn signal_client(&self) -> reqwest::Client {
        self.client("channel.signal", true)
    }
}

/// Build the configured Telegram channel as its concrete provider type.
///
/// The module uses this to retain the same Telegram object as both the polling
/// owner and the native v2 operation executor.
#[must_use]
pub fn build_telegram_channel(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
    http: &dyn HttpClientFactory,
) -> Option<Arc<TelegramChannel>> {
    let tg = config.telegram.as_ref()?;
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
    if let Some(store) = host.voice_media_store() {
        telegram = telegram.with_voice_media_store(store);
    }
    if let Some(repository) = host.media_repository() {
        telegram = telegram.with_media_repository(repository);
    }
    telegram = telegram.with_voice_synthesis(
        host.synthesizer().is_some()
            && host.voice_codec().is_some()
            && host.voice_media_store().is_some(),
    );
    if let Some(allowlist) = host.allowlist() {
        telegram = telegram.with_allowlist(allowlist);
    }
    if let Some(events) = host.events() {
        telegram = telegram.with_events(events);
    }
    Some(Arc::new(telegram))
}

/// Build the configured Discord channel as its concrete provider type,
/// wired to whatever media, voice and event services the host supplies.
#[must_use]
pub fn build_discord_channel(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
    http: &dyn HttpClientFactory,
) -> Option<Arc<DiscordChannel>> {
    let dc = config.discord.as_ref()?;
    tracing::info!(
        channel = "discord",
        allowed_users_count = dc.allowed_users.len(),
        mention_only = dc.mention_only,
        "[channels] discord enabled (bot token not logged)"
    );
    let mut discord = DiscordChannel::with_http_client(
        dc.bot_token.clone(),
        dc.guild_id.clone(),
        dc.channel_id.clone(),
        dc.allowed_users.clone(),
        dc.listen_to_bots,
        dc.mention_only,
        http.client_for("channel.discord"),
    );
    // Each is optional; Discord degrades to text without them.
    if let Some(transcriber) = host.transcriber() {
        discord = discord.with_transcriber(transcriber);
    }
    if let Some(store) = host.voice_media_store() {
        discord = discord.with_voice_media_store(store);
    }
    if let Some(repository) = host.media_repository() {
        discord = discord.with_media_repository(repository);
    }
    if let Some(codec) = host.voice_codec() {
        discord = discord.with_voice_codec(codec);
    }
    discord = discord.with_voice_synthesis(
        host.synthesizer().is_some()
            && host.voice_codec().is_some()
            && host.voice_media_store().is_some(),
    );
    if let Some(events) = host.events() {
        discord = discord.with_events(events);
    }
    Some(Arc::new(discord))
}

/// Build the configured WhatsApp Business Cloud channel as its concrete type.
///
/// The module uses this to retain the same object as the webhook ingress owner
/// and the native v2 operation executor. The Meta app secret that authenticates
/// webhook POSTs is passed through here; without it the channel sends but
/// refuses every webhook delivery.
#[must_use]
pub fn build_whatsapp_cloud_channel(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
    http: &dyn HttpClientFactory,
) -> Option<Arc<WhatsAppChannel>> {
    let wa = config.whatsapp.as_ref()?;
    if !wa.is_cloud_config() {
        return None;
    }
    tracing::info!(
        channel = "whatsapp",
        backend = "cloud",
        signed_webhooks = wa
            .app_secret
            .as_deref()
            .is_some_and(|secret| !secret.trim().is_empty()),
        "[channels] whatsapp cloud enabled (credentials not logged)"
    );
    let mut channel = WhatsAppChannel::with_http_client(
        wa.access_token.clone().unwrap_or_default(),
        wa.phone_number_id.clone().unwrap_or_default(),
        wa.verify_token.clone().unwrap_or_default(),
        wa.allowed_numbers.clone(),
        http.client_for("channel.whatsapp"),
    )
    .with_app_secret(wa.app_secret.clone());
    if let Some(repository) = host.media_repository() {
        channel = channel.with_media_repository(repository);
    }
    if let Some(store) = host.voice_media_store() {
        channel = channel.with_voice_media_store(store);
    }
    channel = channel.with_voice_synthesis(
        host.synthesizer().is_some()
            && host.voice_codec().is_some()
            && host.voice_media_store().is_some(),
    );
    if let Some(events) = host.events() {
        channel = channel.with_events(events);
    }
    Some(Arc::new(channel))
}

/// Build the configured WhatsApp Web channel as its concrete type.
#[cfg(feature = "whatsapp-web")]
#[must_use]
pub fn build_whatsapp_web_channel(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
) -> Option<Arc<crate::WhatsAppWebChannel>> {
    let wa = config.whatsapp.as_ref()?;
    if !wa.is_web_config() || wa.phone_number_id.is_some() {
        return None;
    }
    tracing::info!(
        channel = "whatsapp",
        backend = "web",
        "[channels] whatsapp web enabled (session not logged)"
    );
    let mut channel = crate::providers::whatsapp_web::WhatsAppWebChannel::from_config(
        crate::providers::whatsapp_web::WhatsAppWebConfig {
            session_path: wa.session_path.clone().unwrap_or_default(),
            pair_phone: wa.pair_phone.clone(),
            pair_code: wa.pair_code.clone(),
            allowed_numbers: wa.allowed_numbers.clone(),
        },
    );
    if let Some(lifecycle) = host.lifecycle() {
        channel = channel.with_lifecycle(lifecycle);
    }
    if let Some(storage) = host.storage() {
        channel = channel.with_storage(storage);
    }
    if let Some(repository) = host.media_repository() {
        channel = channel.with_media_repository(repository);
    }
    if let Some(store) = host.voice_media_store() {
        channel = channel.with_voice_media_store(store);
    }
    channel = channel.with_voice_synthesis(
        host.synthesizer().is_some()
            && host.voice_codec().is_some()
            && host.voice_media_store().is_some(),
    );
    if let Some(events) = host.events() {
        channel = channel.with_events(events);
    }
    Some(Arc::new(channel))
}

/// Build the Slack provider with the host ports it can use: scoped media
/// storage, voice storage, synthesis and the ordered event sink.
pub fn build_slack_channel(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
    http: &dyn HttpClientFactory,
) -> Option<Arc<SlackChannel>> {
    let sl = config.slack.as_ref()?;
    tracing::info!(
        channel = "slack",
        allowed_users_count = sl.allowed_users.len(),
        socket_mode = sl.app_token.is_some(),
        "[channels] slack enabled (tokens not logged)"
    );
    let mut slack = SlackChannel::with_http_client(
        sl.bot_token.clone(),
        sl.channel_id.clone(),
        sl.allowed_users.clone(),
        http.client_for("channel.slack"),
    )
    .with_app_token(sl.app_token.clone());
    if let Some(store) = host.voice_media_store() {
        slack = slack.with_voice_media_store(store);
    }
    if let Some(repository) = host.media_repository() {
        slack = slack.with_media_repository(repository);
    }
    slack = slack.with_voice_synthesis(
        host.synthesizer().is_some()
            && host.voice_codec().is_some()
            && host.voice_media_store().is_some(),
    );
    if let Some(events) = host.events() {
        slack = slack.with_events(events);
    }
    Some(Arc::new(slack))
}

/// Build one long-tail text provider (Lark, Mattermost, IRC, QQ, DingTalk,
/// Yuanbao) with its v2 adapter surface, or `None` when `provider` is not
/// configured, not built in, or its configuration is invalid.
#[must_use]
pub fn build_long_tail_channel(
    config: &ChannelsConfig,
    http: &dyn HttpClientFactory,
    provider: &str,
) -> Option<Arc<dyn LongTailNative>> {
    build_long_tail_channel_inner(config, None, http, provider)
}

/// Build a configured long-tail channel with the scoped host ports available
/// to its native typed adapter.
pub fn build_long_tail_channel_with_host(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
    http: &dyn HttpClientFactory,
    provider: &str,
) -> Option<Arc<dyn LongTailNative>> {
    build_long_tail_channel_inner(config, Some(host), http, provider)
}

fn build_long_tail_channel_inner(
    config: &ChannelsConfig,
    host: Option<&Arc<dyn ChannelHost>>,
    http: &dyn HttpClientFactory,
    provider: &str,
) -> Option<Arc<dyn LongTailNative>> {
    match provider {
        "mattermost" => {
            let mm = config.mattermost.as_ref()?;
            let mut channel = MattermostChannel::with_http_client(
                mm.url.clone(),
                mm.bot_token.clone(),
                mm.channel_id.clone(),
                mm.allowed_users.clone(),
                mm.thread_replies.unwrap_or(true),
                mm.mention_only.unwrap_or(false),
                http.client_for("channel.mattermost"),
            );
            if let Some(repository) = host.and_then(|host| host.media_repository()) {
                channel = channel.with_media_repository(repository);
            }
            if let Some(store) = host.and_then(|host| host.voice_media_store()) {
                channel = channel.with_voice_media_store(store);
            }
            if let Some(transcriber) = host.and_then(|host| host.transcriber()) {
                channel = channel.with_transcriber(transcriber);
            }
            if let Some(events) = host.and_then(|host| host.events()) {
                channel = channel.with_events(events);
            }
            Some(Arc::new(channel))
        }
        "irc" => {
            let irc_cfg = config.irc.as_ref()?;
            Some(Arc::new(IrcChannel::new(irc::IrcChannelConfig {
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
            })))
        }
        #[cfg(feature = "lark")]
        "lark" => {
            let lk = config.lark.as_ref()?;
            let mut channel = crate::providers::LarkChannel::from_config_with_http_client(
                lk,
                http.client_for("channel.lark"),
            );
            if let Some(repository) = host.and_then(|host| host.media_repository()) {
                channel = channel.with_media_repository(repository);
            }
            if let Some(store) = host.and_then(|host| host.voice_media_store()) {
                channel = channel.with_voice_media_store(store);
            }
            if let Some(transcriber) = host.and_then(|host| host.transcriber()) {
                channel = channel.with_transcriber(transcriber);
            }
            if let Some(codec) = host.and_then(|host| host.voice_codec()) {
                channel = channel.with_voice_codec(codec);
            }
            if let Some(events) = host.and_then(|host| host.events()) {
                channel = channel.with_events(events);
            }
            Some(Arc::new(channel))
        }
        "dingtalk" => {
            let dt = config.dingtalk.as_ref()?;
            let mut channel = DingTalkChannel::with_http_client(
                dt.client_id.clone(),
                dt.client_secret.clone(),
                dt.allowed_users.clone(),
                http.client_for("channel.dingtalk"),
            );
            channel =
                channel.with_attachment_http_client(http.client_for("channel.dingtalk.media"));
            if let Some(repository) = host.and_then(|host| host.media_repository()) {
                channel = channel.with_media_repository(repository);
            }
            if let Some(store) = host.and_then(|host| host.voice_media_store()) {
                channel = channel.with_voice_media_store(store);
            }
            channel = channel.with_voice_synthesis(
                host.and_then(|host| host.synthesizer()).is_some()
                    && host.and_then(|host| host.voice_codec()).is_some()
                    && host.and_then(|host| host.voice_media_store()).is_some(),
            );
            if let Some(transcriber) = host.and_then(|host| host.transcriber()) {
                channel = channel.with_transcriber(transcriber);
            }
            if let Some(events) = host.and_then(|host| host.events()) {
                channel = channel.with_events(events);
            }
            Some(Arc::new(channel))
        }
        "qq" => {
            let qq = config.qq.as_ref()?;
            let mut channel = QQChannel::with_http_client(
                qq.app_id.clone(),
                qq.app_secret.clone(),
                qq.allowed_users.clone(),
                http.client_for("channel.qq"),
            );
            if let Some(repository) = host.and_then(|host| host.media_repository()) {
                channel = channel.with_media_repository(repository);
            }
            if let Some(store) = host.and_then(|host| host.voice_media_store()) {
                channel = channel.with_voice_media_store(store);
            }
            if let Some(transcriber) = host.and_then(|host| host.transcriber()) {
                channel = channel.with_transcriber(transcriber);
            }
            if let Some(events) = host.and_then(|host| host.events()) {
                channel = channel.with_events(events);
            }
            Some(Arc::new(channel))
        }
        "yuanbao" => {
            let yb = config.yuanbao.as_ref()?;
            build_yuanbao_channel(yb, host, None, None)
        }
        _ => None,
    }
}

fn build_yuanbao_channel(
    config: &tinychannels_bus::config::YuanbaoConfig,
    host: Option<&Arc<dyn ChannelHost>>,
    _voice_http: Option<reqwest::Client>,
    _cos_endpoint: Option<String>,
) -> Option<Arc<dyn LongTailNative>> {
    let mut channel = match YuanbaoChannel::new(config.clone()) {
        Ok(channel) => channel,
        Err(error) => {
            tracing::warn!("[channels] yuanbao config invalid: {error}");
            return None;
        }
    };
    if let Some(repository) = host.and_then(|host| host.media_repository()) {
        channel = channel.with_media_repository(repository);
    }
    if let Some(store) = host.and_then(|host| host.voice_media_store()) {
        channel = channel.with_voice_media_store(store);
    }
    if let Some(transcriber) = host.and_then(|host| host.transcriber()) {
        channel = channel.with_transcriber(transcriber);
    }
    if let Some(events) = host.and_then(|host| host.events()) {
        channel = channel.with_events(events);
    }
    #[cfg(test)]
    if let Some(client) = _voice_http {
        channel = channel.with_voice_http_client(client);
    }
    #[cfg(test)]
    if let Some(endpoint) = _cos_endpoint {
        channel = channel.with_test_cos_endpoint(endpoint);
    }
    Some(Arc::new(channel))
}

#[cfg(test)]
pub(crate) fn build_yuanbao_channel_with_voice_http_for_test(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
    voice_http: reqwest::Client,
) -> Option<Arc<dyn LongTailNative>> {
    let channel =
        build_yuanbao_channel(config.yuanbao.as_ref()?, Some(host), Some(voice_http), None)?;
    Some(channel)
}

#[cfg(test)]
pub(crate) fn build_yuanbao_channel_with_media_http_for_test(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
    voice_http: reqwest::Client,
    cos_endpoint: String,
) -> Option<Arc<dyn LongTailNative>> {
    build_yuanbao_channel(
        config.yuanbao.as_ref()?,
        Some(host),
        Some(voice_http),
        Some(cos_endpoint),
    )
}

/// Build the Signal provider with the host ports it can use: scoped media and
/// voice storage, speech-to-text, synthesis and the ordered event sink.
pub fn build_signal_channel(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
    http: &dyn HttpClientFactory,
) -> Option<Arc<SignalChannel>> {
    let sig = config.signal.as_ref()?;
    let mut signal = SignalChannel::with_http_client(
        sig.http_url.clone(),
        sig.account.clone(),
        sig.group_id.clone(),
        sig.allowed_from.clone(),
        sig.ignore_attachments,
        sig.ignore_stories,
        http.signal_client(),
    );
    if let Some(store) = host.voice_media_store() {
        signal = signal.with_voice_media_store(store);
    }
    if let Some(repository) = host.media_repository() {
        signal = signal.with_media_repository(repository);
    }
    if let Some(transcriber) = host.transcriber() {
        signal = signal.with_transcriber(transcriber);
    }
    signal = signal.with_voice_synthesis(
        host.synthesizer().is_some()
            && host.voice_codec().is_some()
            && host.voice_media_store().is_some(),
    );
    if let Some(events) = host.events() {
        signal = signal.with_events(events);
    }
    Some(Arc::new(signal))
}

/// Build the iMessage provider with the host ports it can use: scoped media and
/// voice storage, speech-to-text, synthesis and the ordered event sink.
#[cfg(feature = "imessage")]
pub fn build_imessage_channel(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
) -> Option<Arc<IMessageChannel>> {
    let im = config.imessage.as_ref()?;
    let mut imessage = IMessageChannel::new(im.allowed_contacts.clone());
    if let Some(store) = host.voice_media_store() {
        imessage = imessage.with_voice_media_store(store);
    }
    if let Some(repository) = host.media_repository() {
        imessage = imessage.with_media_repository(repository);
    }
    if let Some(transcriber) = host.transcriber() {
        imessage = imessage.with_transcriber(transcriber);
    }
    imessage = imessage.with_voice_synthesis(
        host.synthesizer().is_some()
            && host.voice_codec().is_some()
            && host.voice_media_store().is_some(),
    );
    if let Some(events) = host.events() {
        imessage = imessage.with_events(events);
    }
    Some(Arc::new(imessage))
}

/// Build the Linq provider with the host ports it can use: scoped media and
/// voice storage, speech-to-text, synthesis and the ordered event sink.
pub fn build_linq_channel(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
    http: &dyn HttpClientFactory,
) -> Option<Arc<LinqChannel>> {
    let lq = config.linq.as_ref()?;
    let mut linq = LinqChannel::with_http_client(
        lq.api_token.clone(),
        lq.from_phone.clone(),
        lq.allowed_senders.clone(),
        http.client_for("channel.linq"),
    )
    .with_webhook_secret(lq.webhook_secret.clone());
    if let Some(store) = host.voice_media_store() {
        linq = linq.with_voice_media_store(store);
    }
    if let Some(repository) = host.media_repository() {
        linq = linq.with_media_repository(repository);
    }
    if let Some(transcriber) = host.transcriber() {
        linq = linq.with_transcriber(transcriber);
    }
    linq = linq.with_voice_synthesis(
        host.synthesizer().is_some()
            && host.voice_codec().is_some()
            && host.voice_media_store().is_some(),
    );
    if let Some(events) = host.events() {
        linq = linq.with_events(events);
    }
    Some(Arc::new(linq))
}

/// Build the email provider with the host ports it can use: scoped media and
/// voice storage, speech-to-text, synthesis and the ordered event sink.
#[cfg(feature = "email")]
pub fn build_email_channel(
    config: &ChannelsConfig,
    host: &Arc<dyn ChannelHost>,
) -> Option<Arc<crate::providers::EmailChannel>> {
    let mut email = crate::providers::EmailChannel::new(config.email.as_ref()?.clone());
    if let Some(store) = host.voice_media_store() {
        email = email.with_voice_media_store(store);
    }
    if let Some(repository) = host.media_repository() {
        email = email.with_media_repository(repository);
    }
    if let Some(transcriber) = host.transcriber() {
        email = email.with_transcriber(transcriber);
    }
    email = email.with_voice_synthesis(
        host.synthesizer().is_some()
            && host.voice_codec().is_some()
            && host.voice_media_store().is_some(),
    );
    if let Some(events) = host.events() {
        email = email.with_events(events);
    }
    Some(Arc::new(email))
}

/// One configured provider built from the shared factory.
#[derive(Clone)]
pub enum BuiltChannel {
    Adapter {
        adapter: Arc<dyn ChannelAdapter>,
        edit: Option<Arc<dyn crate::channel::ChannelEdit>>,
        drafts: Option<Arc<dyn crate::channel::ChannelStreamingDraft>>,
        reactions: Option<Arc<dyn crate::channel::LegacyChannelReaction>>,
        typing: Option<Arc<dyn crate::channel::ChannelTyping>>,
    },
}

impl BuiltChannel {
    /// Build a provider that exposes only the base adapter contract.
    pub fn from_adapter(adapter: Arc<dyn ChannelAdapter>) -> Self {
        Self::Adapter {
            adapter,
            edit: None,
            drafts: None,
            reactions: None,
            typing: None,
        }
    }

    /// Build a provider while retaining its optional progressive ports before
    /// the concrete provider type is erased.
    pub fn with_output_ports(
        adapter: Arc<dyn ChannelAdapter>,
        edit: Option<Arc<dyn crate::channel::ChannelEdit>>,
        drafts: Option<Arc<dyn crate::channel::ChannelStreamingDraft>>,
    ) -> Self {
        Self::Adapter {
            adapter,
            edit,
            drafts,
            reactions: None,
            typing: None,
        }
    }

    /// Retain the provider's typing-indicator extension before its concrete
    /// type is erased, so the module can start and stop typing for it.
    pub fn with_typing_port(mut self, typing: Arc<dyn crate::channel::ChannelTyping>) -> Self {
        match &mut self {
            Self::Adapter { typing: port, .. } => *port = Some(typing),
        }
        self
    }

    /// Retain the provider's scoped reaction extension before its concrete
    /// type is erased.
    pub fn with_reaction_port(
        mut self,
        reactions: Arc<dyn crate::channel::LegacyChannelReaction>,
    ) -> Self {
        match &mut self {
            Self::Adapter {
                reactions: port, ..
            } => *port = Some(reactions),
        }
        self
    }

    /// Stable provider identifier for this configured instance.
    pub fn name(&self) -> String {
        match self {
            Self::Adapter { adapter, .. } => adapter.descriptor().id,
        }
    }

    /// Capability facts from the configured provider instance.
    pub fn capabilities(&self) -> tinychannels_bus::ChannelCapabilities {
        match self {
            Self::Adapter { adapter, .. } => adapter.declared_capabilities(),
        }
    }

    /// Configured default recipient, if the provider declares one.
    pub fn proactive_target(&self) -> Option<String> {
        match self {
            Self::Adapter { adapter, .. } => adapter.proactive_target(),
        }
    }

    /// The native transport and lifecycle surface for this configured account.
    pub fn adapter(&self) -> &Arc<dyn ChannelAdapter> {
        match self {
            Self::Adapter { adapter, .. } => adapter,
        }
    }

    /// Provider edit port, when the configured adapter implements it.
    pub fn edit(&self) -> Option<&Arc<dyn crate::channel::ChannelEdit>> {
        match self {
            Self::Adapter { edit, .. } => edit.as_ref(),
        }
    }

    /// Provider streaming draft port, when the configured adapter implements it.
    pub fn drafts(&self) -> Option<&Arc<dyn crate::channel::ChannelStreamingDraft>> {
        match self {
            Self::Adapter { drafts, .. } => drafts.as_ref(),
        }
    }

    /// Provider typing port, when the configured adapter implements it.
    pub fn typing(&self) -> Option<&Arc<dyn crate::channel::ChannelTyping>> {
        match self {
            Self::Adapter { typing, .. } => typing.as_ref(),
        }
    }

    /// Provider reaction port, when the configured adapter implements it.
    pub fn reactions(&self) -> Option<&Arc<dyn crate::channel::LegacyChannelReaction>> {
        match self {
            Self::Adapter { reactions, .. } => reactions.as_ref(),
        }
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
) -> Vec<BuiltChannel> {
    let mut channels: Vec<BuiltChannel> = Vec::new();

    if config.telegram.is_some()
        && let Some(telegram) = build_telegram_channel(config, host, http)
    {
        channels.push(
            BuiltChannel::with_output_ports(
                telegram.clone(),
                Some(telegram.clone()),
                Some(telegram.clone()),
            )
            .with_typing_port(telegram),
        );
    }

    if let Some(discord) = build_discord_channel(config, host, http) {
        channels.push(
            BuiltChannel::with_output_ports(
                discord.clone(),
                Some(discord.clone()),
                Some(discord.clone()),
            )
            .with_typing_port(discord),
        );
    }

    if let Some(slack) = build_slack_channel(config, host, http) {
        channels.push(
            BuiltChannel::with_output_ports(
                slack.clone(),
                Some(slack.clone()),
                Some(slack.clone()),
            )
            .with_typing_port(slack),
        );
    }

    if let Some(mattermost) = build_long_tail_channel_with_host(config, host, http, "mattermost") {
        channels.push(
            BuiltChannel::with_output_ports(
                mattermost.clone(),
                Some(mattermost.clone()),
                Some(mattermost.clone()),
            )
            .with_typing_port(mattermost),
        );
    }

    #[cfg(feature = "imessage")]
    if let Some(imessage) = build_imessage_channel(config, host) {
        channels.push(
            BuiltChannel::with_output_ports(
                imessage.clone(),
                Some(imessage.clone()),
                Some(imessage.clone()),
            )
            .with_typing_port(imessage),
        );
    }

    #[cfg(not(feature = "imessage"))]
    if config.imessage.is_some() {
        tracing::warn!(
            "[channels] imessage is configured but iMessage support is disabled in this build; skipping"
        );
    }

    if let Some(signal) = build_signal_channel(config, host, http) {
        channels.push(
            BuiltChannel::with_output_ports(
                signal.clone(),
                Some(signal.clone()),
                Some(signal.clone()),
            )
            .with_typing_port(signal),
        );
    }

    if let Some(wa) = config.whatsapp.as_ref() {
        // Runtime negotiation: the same stanza can describe either backend, so
        // the shape of the config decides which one is meant.
        match wa.backend_type() {
            "cloud" if wa.is_cloud_config() => {
                if let Some(channel) = build_whatsapp_cloud_channel(config, host, http) {
                    channels.push(
                        BuiltChannel::with_output_ports(
                            channel.clone(),
                            Some(channel.clone()),
                            Some(channel.clone()),
                        )
                        .with_reaction_port(channel.clone())
                        .with_typing_port(channel),
                    );
                }
            }
            "cloud" => tracing::warn!(
                "[channels] whatsapp cloud configured but missing phone_number_id, access_token or verify_token"
            ),
            "web" => {
                if let Some(channel) = build_whatsapp_web(wa, host) {
                    channels.push(channel);
                }
            }
            _ => tracing::warn!(
                "[channels] whatsapp config invalid: neither phone_number_id (Cloud API) nor session_path (Web) is set"
            ),
        }
    }

    if let Some(linq) = build_linq_channel(config, host, http) {
        channels.push(
            BuiltChannel::with_output_ports(linq.clone(), Some(linq.clone()), Some(linq.clone()))
                .with_typing_port(linq),
        );
    }

    #[cfg(feature = "email")]
    if let Some(email) = build_email_channel(config, host) {
        channels.push(
            BuiltChannel::with_output_ports(
                email.clone(),
                Some(email.clone()),
                Some(email.clone()),
            )
            .with_typing_port(email),
        );
    }

    if let Some(irc) = build_long_tail_channel_with_host(config, host, http, "irc") {
        channels.push(
            BuiltChannel::with_output_ports(irc.clone(), Some(irc.clone()), Some(irc.clone()))
                .with_typing_port(irc),
        );
    }

    if let Some(lark) = build_long_tail_channel_with_host(config, host, http, "lark") {
        channels.push(
            BuiltChannel::with_output_ports(lark.clone(), Some(lark.clone()), Some(lark.clone()))
                .with_typing_port(lark),
        );
    }

    if let Some(dingtalk) = build_long_tail_channel_with_host(config, host, http, "dingtalk") {
        channels.push(
            BuiltChannel::with_output_ports(
                dingtalk.clone(),
                Some(dingtalk.clone()),
                Some(dingtalk.clone()),
            )
            .with_typing_port(dingtalk),
        );
    }

    if let Some(qq) = build_long_tail_channel_with_host(config, host, http, "qq") {
        channels.push(
            BuiltChannel::with_output_ports(qq.clone(), Some(qq.clone()), Some(qq.clone()))
                .with_typing_port(qq),
        );
    }

    if let Some(yuanbao) = build_long_tail_channel_with_host(config, host, http, "yuanbao") {
        channels.push(
            BuiltChannel::with_output_ports(
                yuanbao.clone(),
                Some(yuanbao.clone()),
                Some(yuanbao.clone()),
            )
            .with_typing_port(yuanbao),
        );
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
) -> Option<BuiltChannel> {
    #[cfg(feature = "whatsapp-web")]
    {
        let config = ChannelsConfig {
            whatsapp: Some(wa.clone()),
            ..Default::default()
        };
        if !wa.is_web_config() {
            tracing::warn!("[channels] whatsapp web configured but session_path not set");
            return None;
        }
        build_whatsapp_web_channel(&config, host).map(|channel| {
            BuiltChannel::with_output_ports(
                channel.clone(),
                Some(channel.clone()),
                Some(channel.clone()),
            )
            .with_typing_port(channel)
        })
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
