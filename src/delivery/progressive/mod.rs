//! Progressive reply delivery over a message-relay backend.
//!
//! While an agent turn runs, a chat user should see activity: a typing
//! indicator, an evolving draft of the reply, an ephemeral "💭 Thinking:"
//! bubble, and a rotating "still working" filler during long turns. When the
//! turn ends, the canonical reply replaces the draft and every ephemeral
//! bubble is deleted.
//!
//! This module owns that choreography: the timing, the per-turn state, the
//! edit/delete/typing failure latches, and the finalize invariants (never post
//! a duplicate bubble, never leave a stale draft on screen). The host supplies
//! the transport as a [`ProgressiveSender`]; OpenHuman implements it over its
//! backend REST relay.
//!
//! Drive a turn with [`ProgressiveReply`].

mod bubbles;
mod draft;
mod finalize;
mod latch;
mod reply;
mod state;

use async_trait::async_trait;
use serde_json::Value;

pub use latch::{channel_edits_unsupported, edit_capability_key, mark_channel_edits_unsupported};
pub use reply::ProgressiveReply;
pub use state::{StreamingState, TypingState, extract_message_id, latest_thinking_snippet};

/// Minimum interval between progressive edits of the outbound message. Stays
/// comfortably below Telegram's ~1 edit/sec per-chat cap; Slack has a similar
/// soft limit.
pub const EDIT_FLUSH_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_millis(1000);

/// Consecutive edit failures tolerated before falling back to atomic-final
/// delivery.
pub const MAX_EDIT_FAILURES: u32 = 2;

/// How often to re-send the typing indicator. Telegram's `sendChatAction`
/// lasts about 5 seconds, so a 4 second refresh keeps it continuous.
pub const TYPING_REFRESH_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(4);

/// Consecutive typing failures before the indicator is disabled for the turn.
/// One failure is usually "endpoint doesn't exist"; two is conclusive.
pub const MAX_TYPING_FAILURES: u32 = 2;

/// How often to post a "still working" filler during long turns.
pub const FILLER_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(13);

/// Consecutive filler failures before fillers are disabled for the turn.
pub const MAX_FILLER_FAILURES: u32 = 2;

/// Maximum Unicode scalars in a filler derived from the thinking stream.
pub const MAX_FILLER_CHARS: usize = 200;

/// Rotating fallback fillers, used when the thinking stream has nothing new.
pub const STATIC_FILLERS: &[&str] = &[
    "💭 Still working on it…",
    "💭 Just a moment…",
    "💭 Almost there…",
];

/// Maximum length of the thinking snippet shown in the ephemeral bubble.
pub const MAX_THINKING_DISPLAY_CHARS: usize = 500;

/// Why a progressive send, edit, delete or typing call failed.
///
/// The variants need different recoveries, which is why they are typed rather
/// than matched on error text. Conflating route absence with message absence
/// is what once left a permanent "💭 Thinking:" bubble in the chat (#5230).
#[derive(Debug, thiserror::Error)]
pub enum ProgressiveSendError {
    /// The host cannot reach its backend at all right now (no credential, no
    /// config). Nothing was attempted; callers bail quietly without counting a
    /// failure.
    #[error("progressive sender unavailable")]
    Unavailable,
    /// The backend serves no message-edit route. The message itself is
    /// untouched and still ours: keep its id so it can be deleted.
    #[error("backend has no message-edit route")]
    EditUnsupported,
    /// The message is gone provider-side. Its id is worthless.
    #[error("message not found")]
    MessageGone,
    /// Anything else (5xx, transport, rate limit). Counts against the
    /// per-turn failure budget.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Transport for progressive delivery, implemented by the host.
///
/// `channel` is the backend's channel id (possibly provider-prefixed, e.g.
/// `telegram:<chat>`). Implementations attach whatever the backend needs
/// (credentials, idempotency keys) and map their errors onto
/// [`ProgressiveSendError`].
#[async_trait]
pub trait ProgressiveSender: Send + Sync {
    /// Post a new message. Returns the backend's raw response; the message id
    /// is read from it with [`extract_message_id`].
    async fn send(&self, channel: &str, text: &str) -> Result<Value, ProgressiveSendError>;

    /// Replace the text of a posted message.
    async fn edit(
        &self,
        channel: &str,
        message_id: &str,
        text: &str,
    ) -> Result<(), ProgressiveSendError>;

    /// Delete a posted message.
    async fn delete(&self, channel: &str, message_id: &str) -> Result<(), ProgressiveSendError>;

    /// Show the typing indicator once.
    async fn typing(&self, channel: &str) -> Result<(), ProgressiveSendError>;
}

/// Whether `channel` supports the progressive placeholders (draft, thinking,
/// filler bubbles). They need both edit and delete; a provider without them
/// would fill the chat with permanent placeholders. Driven by
/// [`crate::capabilities::capabilities_for`], so unknown providers fail safe.
pub fn channel_supports_progressive_ui(channel: &str) -> bool {
    crate::capabilities::capabilities_for(channel).progressive_edits
}

#[cfg(test)]
mod test;
