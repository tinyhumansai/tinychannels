use crate::channel::ChannelInboundEnvelope;
use serde::{Deserialize, Serialize};

/// Legacy v21 message projection retained for one release during v22 migration.
/// New `DeliverInbound` callbacks carry [`crate::ChannelInboundEnvelope`].
///
/// Serde field names remain the v21 wire contract for compatibility shims.
#[deprecated(
    since = "0.1.34",
    note = "v21 compatibility shim; use ChannelInboundEnvelope for canonical v22 inbound content"
)]
///
/// `Default` is derived so a caller that fills only some fields can spread
/// `..Default::default()` and keep compiling when an optional field is added.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelMessage {
    pub id: String,
    pub sender: String,
    pub reply_target: String,
    pub content: String,
    pub channel: String,
    pub timestamp: u64,
    /// Platform thread identifier (e.g. Slack `ts`, Discord thread ID).
    /// When set, replies should be posted as threaded responses.
    pub thread_ts: Option<String>,
    /// Provider message identifier this inbound message replies to.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "replyToId",
        alias = "reply_to_id"
    )]
    pub reply_to_id: Option<String>,
    /// Independent provider topic identifier where supported.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "topicId",
        alias = "topic_id"
    )]
    pub topic_id: Option<String>,
    /// The sender's display name as the platform gives it (WhatsApp push
    /// name, Telegram first and last name, Signal profile name, ...). Never
    /// a saved contact name: no channel exposes one. `None` when the
    /// platform gives none. Optional on the wire, so a host or module built
    /// before this field still decodes the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
}

/// One ordered provider ingress item. Providers submit messages and lifecycle
/// events through the same sink when the module host supplies it.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", content = "item", rename_all = "snake_case")]
pub enum ChannelIngress {
    Inbound(ChannelInboundEnvelope),
    Event(crate::ChannelEvent),
}

/// Legacy v21 text-only send projection retained for one release.
/// Canonical sends use [`crate::ChannelOutboundMessage`] through
/// `SendChannelIntent`.
#[deprecated(
    since = "0.1.34",
    note = "v21 compatibility shim; use ChannelOutboundMessage for canonical v22 sends"
)]
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SendMessage {
    pub content: String,
    pub recipient: String,
    pub subject: Option<String>,
    /// Platform thread identifier for threaded replies (e.g. Slack `thread_ts`).
    pub thread_ts: Option<String>,
    /// Independent provider topic identifier where supported.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "topicId",
        alias = "topic_id"
    )]
    pub topic_id: Option<String>,
    /// Source message identifier for a provider reply.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "replyToId",
        alias = "reply_to_id"
    )]
    pub reply_to_id: Option<String>,
    /// Caller-provided or generated idempotency key for retry-safe delivery.
    pub idempotency_key: Option<String>,
}

impl SendMessage {
    /// Create a new message with content and recipient
    pub fn new(content: impl Into<String>, recipient: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            recipient: recipient.into(),
            subject: None,
            thread_ts: None,
            topic_id: None,
            reply_to_id: None,
            idempotency_key: None,
        }
    }

    /// Create a new message with content, recipient, and subject
    pub fn with_subject(
        content: impl Into<String>,
        recipient: impl Into<String>,
        subject: impl Into<String>,
    ) -> Self {
        Self {
            content: content.into(),
            recipient: recipient.into(),
            subject: Some(subject.into()),
            thread_ts: None,
            topic_id: None,
            reply_to_id: None,
            idempotency_key: None,
        }
    }

    /// Set the thread identifier for threaded replies.
    pub fn in_thread(mut self, thread_ts: Option<String>) -> Self {
        self.thread_ts = thread_ts;
        self
    }

    /// Set an explicit idempotency key for retry-safe delivery.
    pub fn with_idempotency_key(mut self, idempotency_key: impl Into<String>) -> Self {
        let idempotency_key = idempotency_key.into();
        self.idempotency_key = (!idempotency_key.trim().is_empty()).then_some(idempotency_key);
        self
    }
}
