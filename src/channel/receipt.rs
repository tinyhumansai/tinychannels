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

pub fn create_message_receipt_from_outbound_results(
    results: Vec<MessageReceiptSourceResult>,
    kind: Option<MessageReceiptPartKind>,
    thread_id: Option<String>,
    reply_to_id: Option<String>,
    sent_at: u64,
) -> MessageReceipt {
    let kind = kind.unwrap_or_default();
    let mut parts = Vec::new();
    let mut platform_message_ids = Vec::new();

    for (result_index, result) in results.iter().enumerate() {
        if let Some(receipt) = result
            .receipt
            .as_ref()
            .filter(|r| has_nested_receipt_data(r))
        {
            if receipt.parts.is_empty() {
                for (part_index, platform_message_id) in
                    receipt.platform_message_ids.iter().enumerate()
                {
                    parts.push(MessageReceiptPart {
                        platform_message_id: platform_message_id.clone(),
                        kind,
                        index: part_index,
                        thread_id: thread_id.clone(),
                        reply_to_id: reply_to_id.clone(),
                        raw: None,
                    });
                }
            } else {
                for (part_index, part) in receipt.parts.iter().enumerate() {
                    let mut part = part.clone();
                    part.index = part.index.max(part_index);
                    if part.thread_id.is_none() {
                        part.thread_id = thread_id.clone();
                    }
                    if part.reply_to_id.is_none() {
                        part.reply_to_id = reply_to_id.clone();
                    }
                    parts.push(part);
                }
            }
            append_unique(
                &mut platform_message_ids,
                receipt.primary_platform_message_id.as_deref(),
            );
            for id in &receipt.platform_message_ids {
                append_unique(&mut platform_message_ids, Some(id));
            }
            for part in &receipt.parts {
                append_unique(&mut platform_message_ids, Some(&part.platform_message_id));
            }
            continue;
        }

        let Some(platform_message_id) = resolve_result_message_id(result) else {
            continue;
        };
        append_unique(&mut platform_message_ids, Some(&platform_message_id));
        parts.push(MessageReceiptPart {
            platform_message_id,
            kind,
            index: result_index,
            thread_id: thread_id.clone(),
            reply_to_id: reply_to_id.clone(),
            raw: Some(result.clone()),
        });
    }

    let first_nested = results.iter().find_map(|result| {
        result
            .receipt
            .as_ref()
            .filter(|r| has_nested_receipt_data(r))
    });

    MessageReceipt {
        primary_platform_message_id: platform_message_ids.first().cloned(),
        platform_message_ids,
        parts,
        thread_id: thread_id.or_else(|| first_nested.and_then(|r| r.thread_id.clone())),
        reply_to_id: reply_to_id.or_else(|| first_nested.and_then(|r| r.reply_to_id.clone())),
        sent_at: if sent_at != 0 {
            sent_at
        } else {
            first_nested.map(|r| r.sent_at).unwrap_or_default()
        },
        raw: Some(results),
        ..Default::default()
    }
}

pub fn list_message_receipt_platform_ids(receipt: &MessageReceipt) -> Vec<String> {
    let mut ids = Vec::new();
    for id in &receipt.platform_message_ids {
        append_unique(&mut ids, Some(id));
    }
    ids
}

pub fn resolve_message_receipt_primary_id(receipt: &MessageReceipt) -> Option<String> {
    receipt
        .primary_platform_message_id
        .as_deref()
        .and_then(normalize_id)
        .or_else(|| {
            list_message_receipt_platform_ids(receipt)
                .into_iter()
                .next()
        })
}

fn has_nested_receipt_data(receipt: &MessageReceipt) -> bool {
    receipt.primary_platform_message_id.is_some()
        || !receipt.platform_message_ids.is_empty()
        || !receipt.parts.is_empty()
}

fn resolve_result_message_id(result: &MessageReceiptSourceResult) -> Option<String> {
    [
        result.message_id.as_deref(),
        result.chat_id.as_deref(),
        result.channel_id.as_deref(),
        result.room_id.as_deref(),
        result.conversation_id.as_deref(),
        result.to_jid.as_deref(),
        result.poll_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find_map(normalize_id)
}

fn normalize_id(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn append_unique(ids: &mut Vec<String>, value: Option<&str>) {
    let Some(value) = value.and_then(normalize_id) else {
        return;
    };
    if !ids.contains(&value) {
        ids.push(value);
    }
}
