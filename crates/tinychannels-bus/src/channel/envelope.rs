//! Normalized inbound channel envelopes.

use crate::channel::types::{ChannelRef, ConversationRef, SenderRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Direct-message access verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SenderDmDecision {
    Allow,
    Pairing,
    Deny,
}

/// Group access policy that produced the inbound authorization result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupAccessPolicy {
    Open,
    Allowlist,
    Disabled,
}

/// Mention gate that allowed a message through without explicit addressing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MentionGate {
    ExplicitMention,
    ReplyToBot,
    BotThreadParticipant,
    NativeCommand,
    None,
}

/// Access facts carried with an inbound envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct AccessContext {
    pub dm_decision: SenderDmDecision,
    pub group_policy: GroupAccessPolicy,
    pub mention_gate: MentionGate,
    pub command_authorized: bool,
    #[serde(skip)]
    pub delivered_via_upstream_relay: bool,
}

impl Default for AccessContext {
    fn default() -> Self {
        Self {
            dm_decision: SenderDmDecision::Deny,
            group_policy: GroupAccessPolicy::Disabled,
            mention_gate: MentionGate::None,
            command_authorized: false,
            delivered_via_upstream_relay: false,
        }
    }
}

/// Provider-neutral media kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Document,
    #[default]
    Unknown,
}

/// One inbound media reference, preserving provider attachment order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct MediaReference {
    pub path: Option<String>,
    pub url: Option<String>,
    pub content_type: Option<String>,
    pub kind: MediaKind,
    pub transcribed: bool,
    pub platform_message_id: Option<String>,
    pub local_cache_path: Option<String>,
}

/// Legacy prompt-media fields projected from normalized media references.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "PascalCase")]
pub struct InboundMediaPayload {
    pub media_path: Option<String>,
    pub media_url: Option<String>,
    pub media_type: Option<String>,
    pub media_paths: Option<Vec<String>>,
    pub media_urls: Option<Vec<String>>,
    pub media_types: Option<Vec<String>>,
    pub media_transcribed_indexes: Option<Vec<usize>>,
}

/// Normalized inbound event facts consumed by hosts and harness bridges.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelInboundEnvelope {
    pub channel: ChannelRef,
    pub message_id: String,
    pub conversation: ConversationRef,
    pub sender: SenderRef,
    pub text: String,
    pub access: AccessContext,
    pub media: Vec<MediaReference>,
    pub raw: Option<Value>,
}
