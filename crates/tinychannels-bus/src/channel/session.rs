//! Session-key construction and legacy key compatibility.

use crate::channel::envelope::ChannelInboundEnvelope;
use crate::channel::types::{ChannelRef, ConversationKind, ConversationRef, SenderRef};
use crate::traits::ChannelMessage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Session key isolation policy, matching Hermes' default toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionKeyPolicy {
    pub group_sessions_per_user: bool,
    pub thread_sessions_per_user: bool,
}

impl Default for SessionKeyPolicy {
    fn default() -> Self {
        Self {
            group_sessions_per_user: true,
            thread_sessions_per_user: false,
        }
    }
}

/// Legacy OpenHuman key candidates that may need lookup during migration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct LegacySessionKeys {
    pub conversation_history_key: String,
    pub conversation_memory_key: String,
}

/// Build a deterministic TinyChannels session key.
pub fn build_session_key(
    namespace: &str,
    channel: &ChannelRef,
    conversation: &ConversationRef,
    sender: &SenderRef,
    policy: SessionKeyPolicy,
) -> String {
    let namespace = normalize_namespace(namespace);
    let account = channel.account_id.as_deref().unwrap_or("default");
    let chat_type = conversation.kind.as_session_segment();
    let mut parts = vec![
        namespace,
        channel.id.as_str(),
        account,
        chat_type,
        conversation.id.as_str(),
    ];

    if let Some(scope_id) = conversation.scope_id.as_deref().filter(|s| !s.is_empty()) {
        parts.insert(4, scope_id);
    }

    let thread = conversation
        .topic_id
        .as_deref()
        .or(conversation.thread_id.as_deref())
        .filter(|s| !s.is_empty());
    if let Some(thread) = thread {
        parts.push(thread);
    }

    let participant = sender
        .alt_ids
        .first()
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| (!sender.id.is_empty()).then_some(sender.id.as_str()));
    let isolate_user = match conversation.kind {
        ConversationKind::Dm => false,
        _ if thread.is_some() => policy.thread_sessions_per_user,
        _ => policy.group_sessions_per_user,
    };
    if isolate_user && let Some(participant) = participant {
        parts.push(participant);
    }

    parts.join(":")
}

/// Build a deterministic TinyChannels session key from a normalized inbound envelope.
pub fn build_session_key_for_inbound_envelope(
    namespace: &str,
    envelope: &ChannelInboundEnvelope,
    policy: SessionKeyPolicy,
) -> String {
    build_session_key(
        namespace,
        &envelope.channel,
        &envelope.conversation,
        &envelope.sender,
        policy,
    )
}

/// Return legacy OpenHuman keys for migration lookup before writing a new key.
pub fn conversation_history_key_candidates(msg: &ChannelMessage) -> LegacySessionKeys {
    let base_key = format!("{}_{}_{}", msg.channel, msg.sender, msg.reply_target);
    let conversation_history_key = if msg.channel == "telegram" {
        base_key
    } else if let Some(thread_ts) = msg
        .thread_ts
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        format!("{base_key}_thread:{thread_ts}")
    } else {
        base_key
    };

    LegacySessionKeys {
        conversation_history_key,
        conversation_memory_key: format!("{}_{}_{}", msg.channel, msg.sender, msg.id),
    }
}

/// Derive a stable host-local thread key from inbound channel facts.
pub fn derive_inbound_thread_id(
    channel: &str,
    sender: Option<&str>,
    reply_target: Option<&str>,
    thread_ts: Option<&str>,
) -> String {
    let mut key = format!("channel:{channel}");
    if let Some(sender) = sender.and_then(nonempty) {
        key.push('/');
        key.push_str(sender);
    }
    if let Some(reply_target) = reply_target.and_then(nonempty) {
        key.push('/');
        key.push_str(reply_target);
    }
    let provider = channel.split(':').next().unwrap_or("");
    if !matches!(provider, "telegram" | "tg")
        && let Some(thread_ts) = thread_ts.and_then(nonempty)
    {
        key.push_str("#thread:");
        key.push_str(thread_ts);
    }
    key
}

/// Derive a stable client identifier for an inbound channel sender.
pub fn derive_inbound_client_id(channel: &str, sender: Option<&str>) -> String {
    let channel = channel.trim();
    match sender.map(str::trim).filter(|sender| !sender.is_empty()) {
        Some(sender) if !channel.is_empty() => format!("inbound:{channel}:{sender}"),
        Some(sender) => format!("inbound:{sender}"),
        None => "inbound".to_string(),
    }
}

fn normalize_namespace(namespace: &str) -> &str {
    match namespace.trim() {
        "" | "default" => "main",
        value => value,
    }
}

fn nonempty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}
