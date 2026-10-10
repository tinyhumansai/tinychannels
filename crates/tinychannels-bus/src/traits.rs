use serde::{Deserialize, Serialize};

/// A message received from or sent to a channel
///
/// Serde-derived because this type crosses the bus: the module forwards it to
/// the host's `DeliverInbound` callback. Field names are the wire contract —
/// renaming one is a decode failure at the far end, not a compile error.
///
/// `Default` is derived so a caller that fills only some fields can spread
/// `..Default::default()` and keep compiling when an optional field is added.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// The sender's display name as the platform gives it (WhatsApp push
    /// name, Telegram first and last name, Signal profile name, ...). Never
    /// a saved contact name: no channel exposes one. `None` when the
    /// platform gives none. Optional on the wire, so a host or module built
    /// before this field still decodes the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
}

/// Message to send through a channel
///
/// Serde-derived for the same reason as [`ChannelMessage`]: it is the payload
/// of the module's `SendMessage` member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessage {
    pub content: String,
    pub recipient: String,
    pub subject: Option<String>,
    /// Platform thread identifier for threaded replies (e.g. Slack `thread_ts`).
    pub thread_ts: Option<String>,
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
