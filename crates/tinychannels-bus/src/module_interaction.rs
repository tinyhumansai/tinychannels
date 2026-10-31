//! Provider interaction vocabulary for running module-owned channels.
use crate::{ChannelCapabilities, LegacyChannelCapabilities, SendMessage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An interaction on one running provider. The module orders these with sends.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ChannelInteractionRequest {
    pub channel: String,
    pub interaction: ChannelInteraction,
}

/// Provider operations used while composing a response.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", content = "request", rename_all = "snake_case")]
pub enum ChannelInteraction {
    Describe,
    /// Return the canonical v2 descriptor. `Describe` keeps its v1 payload
    /// during the compatibility window.
    DescribeV2,
    StartTyping {
        recipient: String,
    },
    /// Refresh an active typing indicator inside the provider owner. The
    /// initial indicator is started by `StartTyping`; this keeps subsequent
    /// provider calls and their cancellation under the module lifecycle.
    StartTypingRefresh {
        recipient: String,
        interval_secs: u64,
    },
    StopTyping {
        recipient: String,
    },
    /// Stop and join the module-owned refresh task before acknowledging.
    StopTypingRefresh {
        handle: String,
    },
    SendDraft {
        message: SendMessage,
    },
    /// Full-scope draft creation or edit through a configured v2 adapter.
    UpsertDraft {
        previous: Option<crate::channel::MessageReceipt>,
        intent: Box<crate::channel::ChannelOutboundIntent>,
    },
    UpdateDraft {
        recipient: String,
        message_id: String,
        text: String,
    },
    FinalizeDraft {
        recipient: String,
        message_id: String,
        text: String,
        thread_ts: Option<String>,
    },
    SendReaction {
        channel: crate::channel::ChannelRef,
        conversation: crate::channel::ConversationRef,
        provider_message_id: String,
        reaction: crate::channel::ChannelReaction,
        remove: bool,
    },
    EditMessage {
        receipt: Box<crate::channel::MessageReceipt>,
        intent: Box<crate::channel::ChannelOutboundIntent>,
    },
    DeleteMessage {
        receipt: crate::channel::MessageReceipt,
    },
    /// Tell the provider that the account has read one inbound message.
    MarkRead {
        channel: crate::channel::ChannelRef,
        conversation: crate::channel::ConversationRef,
        provider_message_id: String,
    },
    /// One raw provider webhook POST. The module authenticates the exact
    /// bytes against the provider signature before it parses anything; the
    /// host only forwards what its HTTP route received.
    DeliverWebhook {
        /// The provider's signature header value, verbatim.
        signature: Option<String>,
        /// The unmodified request body, base64 encoded.
        body_base64: String,
    },
    /// The provider's webhook subscription handshake (`hub.*` query values).
    VerifyWebhook {
        mode: String,
        verify_token: String,
        challenge: String,
    },
}

/// What the module did with one authenticated webhook delivery.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebhookDeliveryOutcome {
    /// Inbound messages published to the host.
    pub messages: u32,
    /// Lifecycle, receipt and reaction events published to the host.
    pub events: u32,
    /// Items dropped because the provider redelivered them.
    pub duplicates: u32,
}

/// Frozen v21 interaction information returned by the existing `Describe`
/// operation. New consumers should use `DescribeV2`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ChannelInteractionInfo {
    pub capabilities: LegacyChannelCapabilities,
    pub proactive_target: Option<String>,
    pub supports_reactions: bool,
    pub supports_draft_updates: bool,
}

/// Canonical configured-instance information returned by `DescribeV2` and
/// configured-channel catalog queries.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ChannelInteractionInfoV2 {
    pub capabilities: ChannelCapabilities,
}

/// Result of a provider interaction.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
pub enum ChannelInteractionResult {
    Completed,
    /// A typed provider send completed with its actual provider receipt.
    Sent(Box<crate::channel::ChannelSendOutcome>),
    Draft(Option<String>),
    Info(Box<ChannelInteractionInfo>),
    InfoV2(Box<ChannelInteractionInfoV2>),
    TypingRefresh {
        handle: String,
    },
    /// An authenticated webhook delivery was accepted.
    WebhookAccepted(WebhookDeliveryOutcome),
    /// The challenge to echo to the provider's subscription handshake.
    WebhookChallenge(String),
}

#[cfg(test)]
#[path = "module_interaction_tests.rs"]
mod tests;
