//! Typed top-level module operations.

use serde::de::DeserializeOwned;
use tinybus::module::{ModuleCallError, ModuleCallFailure, ModuleClient};
use tinychannels_bus::{
    ChannelControllerRequest, ChannelControllerResult, ChannelRuntimeDispatchPolicy,
    ChannelsConfig, HttpProxyPolicy, MODULE_ID,
    controllers::{BotPermissionCheck, DiscordGuild, DiscordTextChannel},
    methods,
};

use crate::runtime::RuntimeStart;
use crate::{ChannelRuntimeDriver, ChannelRuntimeError, ChannelRuntimeOperation};

#[derive(Clone, Copy)]
pub(super) enum DiscordReply {
    Guilds,
    Channels,
    Permissions,
}

/// Typed host client for the TinyChannels module.
///
/// The host supplies the module registry and lifecycle through `ModuleClient`.
/// Calls that contain credentials or proxy policy use TinyBus's strict
/// attested confidential route and never retry as ordinary calls.
#[derive(Clone)]
pub struct TinyChannelsClient {
    module: ModuleClient,
}

impl TinyChannelsClient {
    /// Create a client over the host's TinyBus module access.
    #[must_use]
    pub fn new(module: ModuleClient) -> Self {
        Self { module }
    }

    /// Bind one attested TinyChannels peer for an identity-scoped runtime.
    ///
    /// The returned driver keeps its root calls and every later object call on
    /// the same unique peer, even if the well-known module name changes owners.
    /// OpenHuman supplies the callback implementation, retry policy, reporting,
    /// configuration and credentials when it passes this driver to
    /// `ModuleRuntime::lease`.
    pub async fn runtime_driver(&self) -> Result<ChannelRuntimeDriver, ChannelRuntimeError> {
        self.bind_runtime_driver(RuntimeStart::Serial).await
    }

    /// Bind a runtime whose inbound dispatch explicitly uses this host-selected
    /// concurrency policy. [`Self::runtime_driver`] retains serial dispatch.
    pub async fn runtime_driver_with_dispatch(
        &self,
        policy: ChannelRuntimeDispatchPolicy,
    ) -> Result<ChannelRuntimeDriver, ChannelRuntimeError> {
        self.bind_runtime_driver(RuntimeStart::Dispatch(policy))
            .await
    }

    async fn bind_runtime_driver(
        &self,
        start: RuntimeStart,
    ) -> Result<ChannelRuntimeDriver, ChannelRuntimeError> {
        let bound = self
            .module
            .bind_attested(MODULE_ID)
            .await
            .map_err(|failure| {
                ChannelRuntimeError::module(ChannelRuntimeOperation::Bind, failure.kind)
            })?;
        Ok(ChannelRuntimeDriver::new(bound, start))
    }

    /// List Discord guilds visible to the configured bot.
    ///
    /// `bot_token` and `proxy` are sent only after the module proves the
    /// registry-pinned release archive and extracted library on the same
    /// unique bus peer. Remote failure details are untrusted and may contain
    /// sensitive text; the returned error's `Debug` and `Display` are redacted.
    pub async fn discord_list_guilds(
        &self,
        config: ChannelsConfig,
        bot_token: String,
        proxy: HttpProxyPolicy,
    ) -> Result<Vec<DiscordGuild>, ModuleCallFailure> {
        let request = ChannelControllerRequest::DiscordListGuilds {
            config,
            bot_token,
            proxy,
        };
        self.discord(request, DiscordReply::Guilds).await
    }

    /// List Discord channels visible in a guild.
    pub async fn discord_list_channels(
        &self,
        config: ChannelsConfig,
        guild_id: String,
        bot_token: String,
        proxy: HttpProxyPolicy,
    ) -> Result<Vec<DiscordTextChannel>, ModuleCallFailure> {
        let request = ChannelControllerRequest::DiscordListChannels {
            config,
            guild_id,
            bot_token,
            proxy,
        };
        self.discord(request, DiscordReply::Channels).await
    }

    /// Check the configured Discord bot's permissions in a channel.
    pub async fn discord_check_permissions(
        &self,
        config: ChannelsConfig,
        guild_id: String,
        channel_id: String,
        bot_token: String,
        proxy: HttpProxyPolicy,
    ) -> Result<BotPermissionCheck, ModuleCallFailure> {
        let request = ChannelControllerRequest::DiscordCheckPermissions {
            config,
            guild_id,
            channel_id,
            bot_token,
            proxy,
        };
        self.discord(request, DiscordReply::Permissions).await
    }

    /// Check provider health using the timeout budget chosen by the host.
    ///
    /// The call is confidential because provider configuration and proxy
    /// policy can contain sensitive operational details.
    pub async fn check_health(
        &self,
        config: ChannelsConfig,
        proxy: HttpProxyPolicy,
        timeout_ms: u64,
    ) -> Result<Vec<tinychannels_bus::ChannelHealthReport>, ModuleCallFailure> {
        self.module
            .call_attested_detailed(
                MODULE_ID,
                methods::CHECK_CHANNEL_HEALTH,
                (config, proxy, timeout_ms),
            )
            .await
    }

    async fn discord<T: DeserializeOwned>(
        &self,
        request: ChannelControllerRequest,
        expected: DiscordReply,
    ) -> Result<T, ModuleCallFailure> {
        let response: ChannelControllerResult = self
            .module
            .call_attested_detailed(MODULE_ID, methods::CHANNEL_CONTROLLER, (request,))
            .await?;
        Self::decode_discord(response, expected).ok_or_else(|| self.malformed_reply())
    }

    fn malformed_reply(&self) -> ModuleCallFailure {
        self.module.report_malformed_reply(MODULE_ID);
        ModuleCallFailure {
            kind: ModuleCallError::ModuleFault,
            wire_name: "ai.tinyhumans.tinybus.Error.Failed".into(),
            message: "malformed TinyChannels module response".into(),
        }
    }

    pub(super) fn decode_discord<T: DeserializeOwned>(
        response: ChannelControllerResult,
        expected: DiscordReply,
    ) -> Option<T> {
        let raw = match (expected, response) {
            (DiscordReply::Guilds, ChannelControllerResult::DiscordGuilds(result)) => result.raw,
            (DiscordReply::Channels, ChannelControllerResult::DiscordChannels(result)) => {
                result.raw
            }
            (DiscordReply::Permissions, ChannelControllerResult::DiscordPermissions(result)) => {
                result.raw
            }
            _ => None,
        };
        raw.and_then(|value| serde_json::from_value(value).ok())
    }
}
