//! Outbound channel intent types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::traits::SendMessage;

/// Delivery durability requested by core when sending agent output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryDurability {
    Required,
    #[default]
    BestEffort,
    Disabled,
}

/// Provider-neutral outbound payload variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutboundPayload {
    Text {
        text: String,
    },
    Media {
        text: Option<String>,
        media_urls: Vec<String>,
    },
    Voice {
        media_url: String,
    },
    Files {
        file_urls: Vec<String>,
    },
    Poll {
        question: String,
        options: Vec<String>,
    },
    PresentationBlocks {
        blocks: Value,
    },
    NativeChannelData {
        data: Value,
    },
}

/// Logical outbound send request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelOutboundIntent {
    pub idempotency_key: String,
    pub channel_id: String,
    pub conversation_id: String,
    pub reply_to_id: Option<String>,
    pub thread_id: Option<String>,
    pub durability: DeliveryDurability,
    pub payload: OutboundPayload,
}

impl Default for ChannelOutboundIntent {
    fn default() -> Self {
        Self {
            idempotency_key: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            reply_to_id: None,
            thread_id: None,
            durability: DeliveryDurability::BestEffort,
            payload: OutboundPayload::Text {
                text: String::new(),
            },
        }
    }
}

/// Build a provider-neutral outbound intent from the legacy rich-message JSON
/// accepted by the `channels.send_message` controller.
///
/// The payload is preserved as native channel data so existing backend request
/// shapes keep working while callers gain a deterministic idempotency key.
pub fn outbound_intent_from_legacy_message(
    channel_id: impl Into<String>,
    message: Value,
) -> ChannelOutboundIntent {
    let channel_id = channel_id.into();
    let idempotency_key = explicit_idempotency_key(&message)
        .unwrap_or_else(|| legacy_message_idempotency_key(&channel_id, &message));
    let conversation_id = first_string_field(&message, &["conversationId", "conversation_id"])
        .or_else(|| first_string_field(&message, &["chatId", "chat_id"]))
        .or_else(|| first_string_field(&message, &["recipient"]))
        .unwrap_or_else(|| channel_id.clone());
    let reply_to_id = first_string_field(
        &message,
        &[
            "replyToMessageId",
            "reply_to_message_id",
            "replyToId",
            "reply_to_id",
        ],
    );
    let thread_id = first_string_field(&message, &["threadId", "thread_id", "thread_ts"]);

    ChannelOutboundIntent {
        idempotency_key,
        channel_id,
        conversation_id,
        reply_to_id,
        thread_id,
        durability: DeliveryDurability::BestEffort,
        payload: OutboundPayload::NativeChannelData { data: message },
    }
}

/// Build an outbound intent from the legacy typed [`SendMessage`] shape.
pub fn outbound_intent_from_send_message(
    channel_id: impl Into<String>,
    message: &SendMessage,
) -> ChannelOutboundIntent {
    let mut body = serde_json::json!({
        "content": message.content,
        "recipient": message.recipient,
        "subject": message.subject,
        "thread_ts": message.thread_ts,
    });
    if let Some(idempotency_key) = message.idempotency_key.as_deref()
        && !idempotency_key.trim().is_empty()
    {
        body["idempotencyKey"] = Value::String(idempotency_key.to_string());
    }
    outbound_intent_from_legacy_message(channel_id, body)
}

/// Render an outbound intent back to the legacy backend message payload.
///
/// This keeps the old JSON shape for native channel data and inserts
/// `idempotencyKey` when the payload did not already carry either camelCase or
/// snake_case idempotency.
pub fn legacy_message_value_from_outbound_intent(intent: &ChannelOutboundIntent) -> Value {
    let payload = match &intent.payload {
        OutboundPayload::Text { text } => serde_json::json!({ "text": text }),
        OutboundPayload::Media { text, media_urls } => {
            serde_json::json!({ "text": text, "mediaUrls": media_urls })
        }
        OutboundPayload::Voice { media_url } => serde_json::json!({ "voiceUrl": media_url }),
        OutboundPayload::Files { file_urls } => serde_json::json!({ "fileUrls": file_urls }),
        OutboundPayload::Poll { question, options } => {
            serde_json::json!({ "poll": { "question": question, "options": options } })
        }
        OutboundPayload::PresentationBlocks { blocks } => {
            serde_json::json!({ "blocks": blocks })
        }
        OutboundPayload::NativeChannelData { data } => data.clone(),
    };
    with_idempotency_key(payload, &intent.idempotency_key)
}

fn explicit_idempotency_key(message: &Value) -> Option<String> {
    first_string_field(message, &["idempotencyKey", "idempotency_key"])
}

fn first_string_field(message: &Value, keys: &[&str]) -> Option<String> {
    let object = message.as_object()?;
    keys.iter()
        .filter_map(|key| object.get(*key))
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

fn legacy_message_idempotency_key(channel_id: &str, message: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(channel_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(canonical_json(message).as_bytes());
    format!("legacy-send:{channel_id}:{}", hex_digest(hasher.finalize()))
}

fn with_idempotency_key(mut payload: Value, idempotency_key: &str) -> Value {
    match &mut payload {
        Value::Object(object) => {
            if !object.contains_key("idempotencyKey") && !object.contains_key("idempotency_key") {
                object.insert(
                    "idempotencyKey".to_string(),
                    Value::String(idempotency_key.to_string()),
                );
            }
            payload
        }
        other => serde_json::json!({
            "payload": other.clone(),
            "idempotencyKey": idempotency_key,
        }),
    }
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).expect("string serialization"),
        Value::Array(values) => {
            let body = values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{body}]")
        }
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);
            let body = entries
                .into_iter()
                .map(|(key, value)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("key serialization"),
                        canonical_json(value)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
    }
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
