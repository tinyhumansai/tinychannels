//! Versioned, provider-neutral channel events carried from a module to its host.

use crate::channel::{ChannelRef, ContentPart, ConversationRef, SenderRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Build the stable action identifier used by native approval buttons.
///
/// The identifier contains only the opaque request ID and zero-based choice
/// index; it never includes the displayed choice text. Telegram callback data
/// is limited to 64 bytes, so callers must fall back to a non-button approval
/// flow when this returns `None`. Hosts can use [`parse_approval_action_id`]
/// to correlate the callback with the pending request and choice.
pub fn approval_action_id(request_id: &str, choice_index: usize) -> Option<String> {
    if request_id.is_empty() {
        return None;
    }
    let action_id = format!("approval-v1:{request_id}:{choice_index}");
    (action_id.len() <= 64).then_some(action_id)
}

/// Parse an action identifier produced by [`approval_action_id`].
pub fn parse_approval_action_id(action_id: &str) -> Option<(&str, usize)> {
    let rest = action_id.strip_prefix("approval-v1:")?;
    let (request_id, choice_index) = rest.rsplit_once(':')?;
    if request_id.is_empty() || action_id.len() > 64 {
        return None;
    }
    Some((request_id, choice_index.parse().ok()?))
}

/// One normalized event emitted by a channel provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelEvent {
    /// Event vocabulary version. This DTO currently represents contract v2.
    pub version: u16,
    /// Stable adapter event ID, suitable for host-side deduplication.
    pub event_id: String,
    /// Provider event ID, retained verbatim when supplied.
    pub source_event_id: Option<String>,
    /// Provider timestamp in Unix milliseconds, when supplied.
    pub source_timestamp: Option<u64>,
    /// Configured provider/account identity.
    pub channel: ChannelRef,
    /// Full conversation address. Presence events may omit it.
    pub conversation: Option<ConversationRef>,
    /// Typed event payload.
    #[serde(flatten)]
    pub event: ChannelEventKind,
}

/// Channel event vocabulary. The discriminator is the `type` field on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ChannelEventKind {
    /// A provider reports a user's typing state.
    Typing {
        actor: SenderRef,
        active: bool,
        expires_at: Option<u64>,
    },
    /// A provider confirms that a message was delivered or read.
    ReadReceipt {
        message_id: String,
        actor: Option<SenderRef>,
        timestamp: Option<u64>,
    },
    /// A provider confirms delivery of a message.
    DeliveredReceipt {
        message_id: String,
        actor: Option<SenderRef>,
        timestamp: Option<u64>,
    },
    /// A user added a reaction.
    ReactionAdded {
        message_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_message_id: Option<String>,
        actor: SenderRef,
        reaction: ChannelReaction,
    },
    /// A user removed a reaction.
    ReactionRemoved {
        message_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_message_id: Option<String>,
        actor: SenderRef,
        reaction: ChannelReaction,
    },
    /// A sent reaction's provider result.
    OutboundReaction {
        message_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_message_id: Option<String>,
        reaction: ChannelReaction,
        success: bool,
    },
    /// A provider edited an existing message.
    MessageUpdated {
        message_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_message_id: Option<String>,
        parts: Vec<ContentPart>,
    },
    /// A provider deleted an existing message.
    MessageDeleted {
        message_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_message_id: Option<String>,
    },
    /// A user activated an opaque provider button. The host applies any
    /// approval or business policy after validating this actor and scope.
    ButtonCallback {
        query_id: String,
        action_id: String,
        provider_message_id: Option<String>,
        actor: SenderRef,
    },
    /// Account or user presence changed.
    Presence {
        actor: SenderRef,
        state: String,
        expires_at: Option<u64>,
    },
    /// Provider connection/lifecycle status changed.
    Lifecycle {
        state: String,
        reason: Option<String>,
    },
}

/// Provider reaction identity, including optional custom emoji identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelReaction {
    /// Displayable reaction text, such as an emoji.
    pub value: String,
    /// Provider-specific stable custom-emoji identity.
    pub custom_emoji_id: Option<String>,
}

#[cfg(test)]
#[path = "event_tests.rs"]
mod tests;
