//! [`ProgressiveReply`]: one turn's progressive delivery, driven by the host.

use std::sync::Arc;

use super::bubbles::{flush_thinking_message, send_filler_message, send_typing_indicator};
use super::draft::flush_streaming_edit;
use super::finalize::{finalize_channel_reply, send_channel_reply};
use super::state::{StreamingState, TypingState};
use super::{ProgressiveSender, channel_supports_progressive_ui};

/// Progressive delivery of one agent turn to one channel.
///
/// The host owns the event loop: it feeds agent stream events in through the
/// `on_*` methods, calls the `*_tick` methods from timers at
/// [`super::EDIT_FLUSH_INTERVAL`], [`super::TYPING_REFRESH_INTERVAL`] and
/// [`super::FILLER_INTERVAL`], and ends the turn with [`Self::finalize`].
pub struct ProgressiveReply {
    sender: Arc<dyn ProgressiveSender>,
    channel: String,
    progressive_ui: bool,
    pub(super) state: StreamingState,
    typing: TypingState,
}

impl ProgressiveReply {
    /// Start a turn on `channel`. Progressive bubbles are enabled only when the
    /// channel's provider supports edit and delete.
    pub fn new(sender: Arc<dyn ProgressiveSender>, channel: impl Into<String>) -> Self {
        let channel = channel.into();
        let progressive_ui = channel_supports_progressive_ui(&channel);
        tracing::debug!(
            "[channel-inbound] progressive reply channel='{}' progressive_ui={}",
            channel,
            progressive_ui
        );
        Self {
            sender,
            channel,
            progressive_ui,
            state: StreamingState::default(),
            typing: TypingState::default(),
        }
    }

    /// The channel this turn replies to.
    pub fn channel(&self) -> &str {
        &self.channel
    }

    /// Current per-turn state (for logging and tests).
    pub fn state(&self) -> &StreamingState {
        &self.state
    }

    /// Visible assistant text arrived.
    pub fn on_text_delta(&mut self, delta: &str) {
        self.state.content.push_str(delta);
        self.state.dirty = true;
    }

    /// A tool call started.
    pub fn on_tool_call(&mut self, tool_name: &str) {
        self.state.last_tool = Some(format!("🔧 {tool_name}…"));
        self.state.dirty = true;
    }

    /// A tool call finished.
    pub fn on_tool_result(&mut self, tool_name: &str, success: bool) {
        let mark = if success { "✓" } else { "✗" };
        self.state.last_tool = Some(format!("🔧 {tool_name} {mark}"));
        self.state.dirty = true;
    }

    /// Model reasoning arrived.
    pub fn on_thinking_delta(&mut self, delta: &str) {
        self.state.thinking_accumulator.push_str(delta);
        self.state.thinking_dirty = true;
    }

    /// Flush pending draft and thinking updates.
    pub async fn edit_tick(&mut self) {
        if !self.progressive_ui {
            return;
        }
        let sender = Arc::clone(&self.sender);
        if self.state.thinking_dirty && !self.state.thinking_edit_disabled {
            flush_thinking_message(sender.as_ref(), &self.channel, &mut self.state).await;
        }
        if self.state.dirty && !self.state.edit_disabled {
            flush_streaming_edit(sender.as_ref(), &self.channel, &mut self.state).await;
        }
    }

    /// Refresh the typing indicator.
    pub async fn typing_tick(&mut self) {
        let sender = Arc::clone(&self.sender);
        send_typing_indicator(sender.as_ref(), &self.channel, &mut self.typing).await;
    }

    /// Post a "still working" filler, where the channel can clean it up.
    pub async fn filler_tick(&mut self) {
        if !self.progressive_ui || self.state.filler_disabled {
            return;
        }
        let sender = Arc::clone(&self.sender);
        send_filler_message(sender.as_ref(), &self.channel, &mut self.state).await;
    }

    /// Deliver the canonical reply and remove every ephemeral bubble.
    pub async fn finalize(&mut self, final_text: &str) {
        let sender = Arc::clone(&self.sender);
        finalize_channel_reply(sender.as_ref(), &self.channel, &mut self.state, final_text).await;
    }

    /// Send a standalone reply without touching the turn's draft state (for a
    /// turn that failed to start).
    pub async fn send_reply(&self, text: &str) {
        send_channel_reply(self.sender.as_ref(), &self.channel, text).await;
    }
}
