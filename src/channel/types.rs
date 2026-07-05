//! Shared channel reference types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Catalog-driven channel descriptor. `id` is intentionally open-ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelDescriptor {
    pub id: String,
    pub display_name: String,
    pub account_id: Option<String>,
    pub aliases: Vec<String>,
    pub metadata: BTreeMap<String, String>,
}

/// Reference to one channel/account surface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelRef {
    pub id: String,
    pub account_id: Option<String>,
}

/// Provider-neutral conversation kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    Dm,
    Group,
    Channel,
    Thread,
    #[default]
    Unknown,
}

impl ConversationKind {
    pub fn as_session_segment(self) -> &'static str {
        match self {
            Self::Dm => "dm",
            Self::Group => "group",
            Self::Channel => "channel",
            Self::Thread => "thread",
            Self::Unknown => "unknown",
        }
    }
}

/// Reference to an inbound or outbound conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ConversationRef {
    pub kind: ConversationKind,
    pub id: String,
    pub scope_id: Option<String>,
    pub parent_id: Option<String>,
    pub thread_id: Option<String>,
    pub topic_id: Option<String>,
}

impl Default for ConversationRef {
    fn default() -> Self {
        Self {
            kind: ConversationKind::Unknown,
            id: String::new(),
            scope_id: None,
            parent_id: None,
            thread_id: None,
            topic_id: None,
        }
    }
}

/// Reference to the message sender.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct SenderRef {
    pub id: String,
    pub alt_ids: Vec<String>,
    pub name: Option<String>,
    pub is_bot: bool,
    pub roles: Vec<String>,
}

/// Host-owned secret reference. Secret values never live in this crate's types.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct SecretRef {
    pub id: String,
    pub label: Option<String>,
}
