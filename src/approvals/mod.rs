//! In-chat approval prompts for supervised tool calls.
//!
//! When a host parks a tool call for approval, the user who started the turn
//! must be asked in the chat the turn came from, and their `yes`/`no` reply
//! routed back. [`ApprovalSurface`] owns the channel half of that:
//!
//! 1. [`ApprovalSurface::record_inbound`] remembers where each conversation's
//!    last message came from (reply target and thread).
//! 2. [`ApprovalSurface::surface`] sends the prompt back to that chat.
//!
//! The approval decision itself (parking, TTL, parsing the reply) stays with
//! the host. Only channels whose capabilities set `chat_approvals` take part.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::capabilities::capabilities_for;
use crate::context::conversation_history_key_parts;
use crate::traits::{Channel, ChannelSendExt, SendMessage};

const LOG_PREFIX: &str = "[channel-approval]";

/// Render an approval request as a chat message.
pub fn format_approval_prompt(tool_name: &str, action_summary: &str) -> String {
    format!(
        "🔐 Approval needed\nTool: `{tool_name}`\nAction: {action_summary}\n\nReply `yes` to approve or `no` to deny."
    )
}

/// Where to deliver a prompt for a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyContext {
    pub reply_target: String,
    pub thread_ts: Option<String>,
}

/// A parked tool call to ask about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPrompt {
    pub request_id: String,
    pub tool_name: String,
    pub action_summary: String,
    /// Conversation history key of the turn that parked the call.
    pub thread_id: String,
    /// Channel the turn came from; the approval context's client id.
    pub channel: String,
}

/// What [`ApprovalSurface::surface`] did.
#[derive(Debug)]
pub enum SurfaceOutcome {
    /// The prompt was sent.
    Sent,
    /// The channel has no in-chat approval capability.
    Unsupported,
    /// No inbound message was recorded for this conversation, so there is
    /// nowhere to send the prompt. The parked call will time out denied.
    NoReplyContext,
    /// The channel is not running in this runtime.
    ChannelNotRegistered,
    /// Sending failed.
    SendFailed(anyhow::Error),
}

/// Routes approval prompts back to the chat each conversation came from.
pub struct ApprovalSurface {
    channels_by_name: Arc<HashMap<String, Arc<dyn Channel>>>,
    reply_index: Mutex<HashMap<String, ReplyContext>>,
}

impl ApprovalSurface {
    /// A surface over the runtime's channels, keyed by [`Channel::name`].
    pub fn new(channels_by_name: Arc<HashMap<String, Arc<dyn Channel>>>) -> Self {
        Self {
            channels_by_name,
            reply_index: Mutex::new(HashMap::new()),
        }
    }

    /// Remember where a conversation's latest inbound message came from.
    /// Ignored for channels without in-chat approvals.
    pub fn record_inbound(
        &self,
        channel: &str,
        sender: &str,
        reply_target: &str,
        thread_ts: Option<&str>,
    ) {
        if !capabilities_for(channel).chat_approvals {
            return;
        }
        let key = conversation_history_key_parts(channel, sender, reply_target, thread_ts);
        let ctx = ReplyContext {
            reply_target: reply_target.to_string(),
            thread_ts: thread_ts.map(str::to_string),
        };
        self.reply_index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, ctx);
    }

    /// The recorded reply context for a conversation key.
    pub fn reply_context(&self, thread_id: &str) -> Option<ReplyContext> {
        self.reply_index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .cloned()
    }

    /// Send `prompt` to the chat its conversation came from.
    pub async fn surface(&self, prompt: &ApprovalPrompt) -> SurfaceOutcome {
        let ApprovalPrompt {
            request_id,
            tool_name,
            thread_id,
            channel: channel_name,
            ..
        } = prompt;
        if !capabilities_for(channel_name).chat_approvals {
            return SurfaceOutcome::Unsupported;
        }
        let Some(reply_ctx) = self.reply_context(thread_id) else {
            tracing::warn!(
                "{LOG_PREFIX} no reply context recorded for channel={channel_name} \
                 thread_id={thread_id} (approval request_id={request_id} tool={tool_name}); \
                 cannot surface prompt — the parked turn will TTL-deny"
            );
            return SurfaceOutcome::NoReplyContext;
        };
        let Some(channel) = self.channels_by_name.get(channel_name.as_str()).cloned() else {
            tracing::warn!(
                "{LOG_PREFIX} channel={channel_name} not registered in runtime; \
                 dropping approval prompt for request_id={request_id}"
            );
            return SurfaceOutcome::ChannelNotRegistered;
        };

        let body = format_approval_prompt(tool_name, &prompt.action_summary);
        let send = SendMessage::new(body, &reply_ctx.reply_target).in_thread(reply_ctx.thread_ts);
        tracing::info!(
            "{LOG_PREFIX} surfacing approval prompt channel={channel_name} request_id={request_id} \
             tool={tool_name} thread_id={thread_id} reply_target={}",
            reply_ctx.reply_target
        );
        match channel.send_with_outbound_intent(&send).await {
            Ok(()) => SurfaceOutcome::Sent,
            Err(err) => {
                tracing::warn!(
                    "{LOG_PREFIX} failed to send approval prompt channel={channel_name} \
                     request_id={request_id} tool={tool_name}: {err}"
                );
                SurfaceOutcome::SendFailed(err)
            }
        }
    }
}

#[cfg(test)]
mod test;
