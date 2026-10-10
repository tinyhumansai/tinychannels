//! Outbound message receipt normalization.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Logical part kind for multi-part rendered messages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageReceiptPartKind {
    Text,
    Media,
    Voice,
    Poll,
    Card,
    Preview,
    #[default]
    Unknown,
}

/// Raw platform result shape normalized into a message receipt.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct MessageReceiptSourceResult {
    pub channel: Option<String>,
    pub message_id: Option<String>,
    pub chat_id: Option<String>,
    pub channel_id: Option<String>,
    pub room_id: Option<String>,
    pub conversation_id: Option<String>,
    pub to_jid: Option<String>,
    pub poll_id: Option<String>,
    pub timestamp: Option<u64>,
    pub meta: Option<Value>,
    pub receipt: Option<MessageReceipt>,
}

/// One platform message produced by a logical outbound send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct MessageReceiptPart {
    pub platform_message_id: String,
    pub kind: MessageReceiptPartKind,
    pub index: usize,
    pub thread_id: Option<String>,
    pub reply_to_id: Option<String>,
    pub raw: Option<MessageReceiptSourceResult>,
}

impl Default for MessageReceiptPart {
    fn default() -> Self {
        Self {
            platform_message_id: String::new(),
            kind: MessageReceiptPartKind::Unknown,
            index: 0,
            thread_id: None,
            reply_to_id: None,
            raw: None,
        }
    }
}

/// Normalized receipt for all platform messages that make up a logical send.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct MessageReceipt {
    pub primary_platform_message_id: Option<String>,
    pub platform_message_ids: Vec<String>,
    pub parts: Vec<MessageReceiptPart>,
    pub thread_id: Option<String>,
    pub reply_to_id: Option<String>,
    pub edit_token: Option<String>,
    pub delete_token: Option<String>,
    pub sent_at: u64,
    pub raw: Option<Vec<MessageReceiptSourceResult>>,
}
