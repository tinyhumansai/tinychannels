//! Inbound projections prepared by the module for host persistence and dispatch.
use crate::channel::SessionKeyPolicy;
use crate::{ChannelInboundEnvelope, ChannelMessage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Original provider data, preserving the normalized envelope when available.
#[allow(
    clippy::large_enum_variant,
    reason = "the legacy and envelope variants preserve the established serialized request shape"
)]
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChannelInboundSource {
    Legacy {
        message: Box<ChannelMessage>,
        #[serde(default)]
        envelope: Option<ChannelInboundEnvelope>,
    },
    Envelope {
        envelope: ChannelInboundEnvelope,
        timestamp: u64,
    },
}

/// Controls whether inbound preparation must fit the legacy text-only view or
/// may preserve typed content for a host that consumes the full envelope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelInboundPreparationMode {
    /// Require a complete legacy text projection; reject content it cannot
    /// represent so existing callers remain fail closed.
    #[default]
    LegacyText,
    /// Return the complete typed envelope and its text-compatible routing
    /// projection for a host that also handles the typed parts.
    Typed,
}

impl ChannelInboundPreparationMode {
    fn is_legacy_text(&self) -> bool {
        *self == Self::LegacyText
    }
}

/// Host-selected namespace and session isolation applied to provider data.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct PrepareChannelInboundRequest {
    pub source: ChannelInboundSource,
    pub namespace: String,
    pub session_policy: SessionKeyPolicy,
    /// Existing callers remain text-only unless they explicitly opt in.
    #[serde(
        default,
        skip_serializing_if = "ChannelInboundPreparationMode::is_legacy_text"
    )]
    pub preparation_mode: ChannelInboundPreparationMode,
}

/// Facts used by the host without repeating channel projection or key algorithms.
/// Authenticated transport provenance must be retained separately by the caller;
/// the envelope deliberately does not serialize its relay trust flag.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct PreparedChannelInbound {
    pub message: ChannelMessage,
    pub envelope: ChannelInboundEnvelope,
    pub history_key: String,
    /// Whether the configured provider treats its thread identifier as a
    /// reply target rather than a distinct persisted conversation.
    #[serde(default)]
    pub history_key_ignores_thread: bool,
    pub thread_id: String,
    pub client_id: String,
    pub session_key: String,
}

/// Optional sender facts from hosted ingress, including legacy channel-only traffic.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct PrepareChannelIdentityRequest {
    pub channel: String,
    pub sender: Option<String>,
    pub reply_target: Option<String>,
    pub thread_ts: Option<String>,
}

/// Stable persisted identifiers; hosts must use these values verbatim.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct PreparedChannelIdentity {
    pub thread_id: String,
    pub client_id: String,
}

#[cfg(test)]
#[path = "module_inbound_tests.rs"]
mod tests;
