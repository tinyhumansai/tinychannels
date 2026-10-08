use async_trait::async_trait;
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

    /// Generate a deterministic idempotency key from this legacy send shape.
    pub fn with_deterministic_idempotency_key(mut self, channel_id: impl AsRef<str>) -> Self {
        let intent = crate::channel::outbound_intent_from_send_message(channel_id.as_ref(), &self);
        self.idempotency_key = Some(intent.idempotency_key);
        self
    }
}

/// Core channel trait — implement for any messaging platform
#[async_trait]
pub trait Channel: Send + Sync {
    /// Human-readable channel name
    fn name(&self) -> &str;

    /// Resolve the delivery target for a *recipient-less* proactive send (cron /
    /// heartbeat), where the caller has no inbound message to reply to.
    ///
    /// Returns the channel's configured default target (e.g. Discord's
    /// `channel_id`) or `None` when the channel has no target it can deliver to
    /// without an explicit recipient. Proactive routing skips channels that
    /// return `None` instead of POSTing to an empty recipient (#3794 review —
    /// Codex P2; Telegram has no configured default chat, so its `send` would
    /// otherwise call the Bot API with an empty `chat_id`). Default `None` keeps
    /// every existing provider opted out until it wires a real target.
    fn proactive_target(&self) -> Option<String> {
        None
    }

    /// Send a message through this channel
    async fn send(&self, message: &SendMessage) -> anyhow::Result<()>;

    /// Start listening for incoming messages (long-running)
    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()>;

    /// Check if channel is healthy
    async fn health_check(&self) -> bool {
        true
    }

    /// Signal that the bot is processing a response (e.g. "typing" indicator).
    /// Implementations should repeat the indicator as needed for their platform.
    async fn start_typing(&self, _recipient: &str) -> anyhow::Result<()> {
        Ok(())
    }

    /// Stop any active typing indicator.
    async fn stop_typing(&self, _recipient: &str) -> anyhow::Result<()> {
        Ok(())
    }

    /// Optional interaction capabilities (remote control, chat approvals,
    /// progressive edits). Defaults to the provider table in
    /// [`crate::capabilities::capabilities_for`], keyed by [`Channel::name`];
    /// override only when an instance differs from its provider's default.
    fn capabilities(&self) -> crate::capabilities::ChannelCapabilities {
        crate::capabilities::capabilities_for(self.name())
    }

    /// Whether this channel supports native emoji reactions on messages.
    /// Channels that return `true` must handle `[REACTION:<emoji>]` content in `send()`.
    fn supports_reactions(&self) -> bool {
        false
    }

    /// Whether this channel supports progressive message updates via draft edits.
    fn supports_draft_updates(&self) -> bool {
        false
    }

    /// Send an initial draft message. Returns a platform-specific message ID for later edits.
    async fn send_draft(&self, _message: &SendMessage) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    /// Update a previously sent draft message with new accumulated content.
    async fn update_draft(
        &self,
        _recipient: &str,
        _message_id: &str,
        _text: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Finalize a draft with the complete response (e.g. apply Markdown formatting).
    async fn finalize_draft(
        &self,
        _recipient: &str,
        _message_id: &str,
        _text: &str,
        _thread_ts: Option<&str>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Extension helpers for legacy [`Channel::send`] callers that need the
/// portable outbound-intent idempotency contract without changing provider
/// implementations yet.
#[async_trait]
pub trait ChannelSendExt: Channel {
    /// Build the outbound intent for a legacy send and pass its idempotency key
    /// through [`SendMessage`] before delegating to [`Channel::send`].
    async fn send_with_outbound_intent(&self, message: &SendMessage) -> anyhow::Result<()> {
        let message = message
            .clone()
            .with_deterministic_idempotency_key(self.name());
        self.send(&message).await
    }
}

impl<T: Channel + ?Sized> ChannelSendExt for T {}

#[cfg(test)]
#[path = "traits_tests.rs"]
mod tests;
