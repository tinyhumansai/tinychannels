//! Opaque identity-scoped channel runtime vocabulary.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Smallest accepted channel message timeout in seconds.
///
/// Hosts validate the module-prepared runtime limits against this contract
/// value instead of depending on the runtime implementation crate.
pub const MIN_CHANNEL_MESSAGE_TIMEOUT_SECS: u64 = 30;
/// Default timeout for channel message processing, in seconds.
pub const CHANNEL_MESSAGE_TIMEOUT_SECS: u64 = 300;
/// Default interval for refreshing an active typing indicator, in seconds.
pub const CHANNEL_TYPING_REFRESH_INTERVAL_SECS: u64 = 4;
/// Default initial delay before restarting a failed channel listener, in seconds.
pub const DEFAULT_CHANNEL_INITIAL_BACKOFF_SECS: u64 = 2;
/// Default maximum delay before restarting a failed channel listener, in seconds.
pub const DEFAULT_CHANNEL_MAX_BACKOFF_SECS: u64 = 60;
/// Maximum number of legacy channel history messages retained per conversation.
pub const MAX_CHANNEL_HISTORY: usize = 50;

/// Inert reservation returned before any provider, callback worker or relay starts.
///
/// The host installs the same immutable identity adapter at `host_object_path`
/// before calling `StartRuntime(handle)`. Existing provider and relay methods
/// then execute at `object_path`. `StopRuntime(handle)` drains and removes that
/// object without affecting another runtime. Unactivated reservations expire
/// after sixty seconds; started runtimes require explicit stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelRuntimeReservation {
    /// Module-generated opaque identifier; hosts must not mint or reuse it.
    pub handle: String,
    /// Private provider object using the existing Channels interface.
    pub object_path: String,
    /// Private host callback object using the existing ChannelsHost interface.
    pub host_object_path: String,
}

impl ChannelRuntimeReservation {
    /// Check that the opaque handle and both private callback paths agree with
    /// the TinyChannels bus contract.
    ///
    /// The error is deliberately content-free so callers can report it without
    /// exposing a supplied handle or object path.
    pub fn validate(&self) -> crate::Result<()> {
        const INVALID: &str = "invalid channel runtime reservation";

        if self.handle.len() != 32
            || !self
                .handle
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(crate::TinyChannelsError::new(INVALID));
        }

        let runtime_suffix = format!("/runtime_{}", self.handle);
        if self.object_path != format!("{}{}", crate::OBJECT_PATH, runtime_suffix)
            || self.host_object_path != format!("{}{}", crate::HOST_OBJECT_PATH, runtime_suffix)
        {
            return Err(crate::TinyChannelsError::new(INVALID));
        }

        Ok(())
    }
}

/// Opt-in turn dispatch for a reserved runtime. Legacy `StartRuntime` retains
/// serial `DeliverInbound` envelope notifications; this policy uses `ProcessInbound`
/// callbacks with module-owned bounded concurrency and stop/drain ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelRuntimeDispatchPolicy {
    /// Number of active provider listeners, including an optional relay.
    /// The module admits four turns per listener, clamped to eight through 64.
    pub listener_count: u32,
}

#[cfg(test)]
#[path = "module_runtime_tests.rs"]
mod tests;

/// How a configured provider receives incoming messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelIngressKind {
    /// The module owns its native listener and reconnect loop.
    Listener,
    /// An external webhook supplies incoming traffic; outbound remains available.
    ExternalWebhook,
}

/// Provider facts resolved by the module's configured factory without starting it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ConfiguredChannelInfo {
    pub name: String,
    pub ingress: ChannelIngressKind,
    pub interaction: crate::ChannelInteractionInfoV2,
}

/// Legacy message vocabulary whose normalization and hashing stay in the module.
#[allow(
    clippy::large_enum_variant,
    reason = "message preparation variants retain their established serialized contract"
)]
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum ChannelMessagePreparation {
    /// Preserve the rich JSON accepted by the existing controller.
    Legacy(serde_json::Value),
    /// Preserve the existing typed channel send and its idempotency algorithm.
    Typed(crate::SendMessage),
    /// Render an already prepared intent without changing its key or durability.
    Intent(Box<crate::ChannelOutboundIntent>),
}

/// An intent and the corresponding legacy backend JSON prepared inside the module.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct PreparedChannelIntent {
    pub intent: crate::ChannelOutboundIntent,
    pub legacy_message: serde_json::Value,
}

/// Encoded message limit leaving room for both returned payloads in a 16 MiB frame.
pub const PREPARATION_MESSAGE_BYTES: usize = 8 * 1024 * 1024 - 4096;
