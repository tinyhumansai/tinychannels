//! Serialized record DTOs stored by TinyChannels implementations.
//!
//! These types describe the persisted document shapes. Conversion between
//! channel messages and these records belongs to the `tinychannels` crate.

use crate::channel::{DeliveryDurability, MessageReceipt, OutboundPayload};
use crate::module_delivery::DeliveryRecoveryState;
use serde::{Deserialize, Serialize};

/// A chat's binding to a host conversation thread. When it was last bound is
/// the record's own `updated_ms` metadata, not a field here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatBinding {
    pub thread_id: String,
    pub sender_key: String,
    /// Title captured at `/new` time so `/status` can show it without listing
    /// every thread.
    #[serde(default)]
    pub title: Option<String>,
}

/// The outbound intent's persisted snake_case shape. Payload list keys retain
/// their historical storage names while typed v2 content keeps its wire shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredIntent {
    pub idempotency_key: String,
    pub channel_id: String,
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_ref: Option<crate::ChannelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_ref: Option<crate::ConversationRef>,
    pub reply_to_id: Option<String>,
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub durability: DeliveryDurability,
    #[serde(with = "stored_payload")]
    pub payload: OutboundPayload,
}

/// A pending or failed delivery as stored. The entry id is the intent's
/// idempotency key, so the body does not repeat it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredDelivery {
    pub intent: StoredIntent,
    pub enqueued_ms: i64,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_send_started_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_state: Option<DeliveryRecoveryState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_receipt: Option<MessageReceipt>,
}

/// A terminal result as stored. Its lifetime is the record's `expires_ms`
/// metadata, not a body field. The id is retained because a long idempotency
/// key may have a hashed storage document id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredResult {
    pub id: String,
    pub intent_fingerprint: String,
    pub completed_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<MessageReceipt>,
}

/// Keep the document-store payload's list keys snake_case while TinyBus uses
/// camelCase. Typed v2 payload fields otherwise retain their contract shape.
mod stored_payload {
    use super::OutboundPayload;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;

    pub fn serialize<S>(payload: &OutboundPayload, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut value = serde_json::to_value(payload).map_err(serde::ser::Error::custom)?;
        if let Value::Object(fields) = &mut value {
            let (wire_key, storage_key) = match fields.get("kind").and_then(Value::as_str) {
                Some("media") => ("mediaUrls", "media_urls"),
                Some("files") => ("fileUrls", "file_urls"),
                _ => ("", ""),
            };
            if !wire_key.is_empty()
                && let Some(value) = fields.remove(wire_key)
            {
                fields.insert(storage_key.to_owned(), value);
            }
        }
        value.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<OutboundPayload, D::Error>
    where
        D: Deserializer<'de>,
    {
        OutboundPayload::deserialize(deserializer)
    }
}

#[cfg(test)]
#[path = "records_tests.rs"]
mod tests;
