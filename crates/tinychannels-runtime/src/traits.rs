//! In-process provider seams and legacy send helpers.
use async_trait::async_trait;
pub use tinychannels_bus::traits::*;

/// Runtime idempotency construction for the legacy send DTO.
pub trait SendMessageExt {
    fn with_deterministic_idempotency_key(self, channel_id: impl AsRef<str>) -> Self;
}
impl SendMessageExt for SendMessage {
    /// Generate a deterministic idempotency key from this legacy send shape.
    fn with_deterministic_idempotency_key(mut self, channel_id: impl AsRef<str>) -> Self {
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
