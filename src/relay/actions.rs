//! Relay action projections.

use crate::channel::{ChannelOutboundIntent, OutboundPayload};
use serde_json::{Map, Value, json};

/// Project a provider-neutral outbound intent to the Hermes relay `send` action.
pub fn relay_send_action_from_outbound_intent(intent: &ChannelOutboundIntent) -> Value {
    let mut metadata = Map::new();
    metadata.insert(
        "idempotency_key".to_string(),
        Value::String(intent.idempotency_key.clone()),
    );
    metadata.insert(
        "channel_id".to_string(),
        Value::String(intent.channel_id.clone()),
    );
    metadata.insert(
        "conversation_id".to_string(),
        Value::String(intent.conversation_id.clone()),
    );
    metadata.insert("durability".to_string(), json!(intent.durability));
    if let Some(thread_id) = intent.thread_id.as_deref() {
        metadata.insert(
            "thread_id".to_string(),
            Value::String(thread_id.to_string()),
        );
    }
    if !matches!(intent.payload, OutboundPayload::Text { .. }) {
        metadata.insert("payload".to_string(), json!(intent.payload));
    }

    json!({
        "op": "send",
        "chat_id": intent.conversation_id,
        "content": relay_send_content(&intent.payload),
        "reply_to": intent.reply_to_id,
        "metadata": metadata,
    })
}

fn relay_send_content(payload: &OutboundPayload) -> String {
    match payload {
        OutboundPayload::Text { text } => text.clone(),
        OutboundPayload::Media { text, .. } => text.clone().unwrap_or_default(),
        OutboundPayload::Voice { media_url } => media_url.clone(),
        OutboundPayload::Files { file_urls } => file_urls.join("\n"),
        OutboundPayload::Poll { question, .. } => question.clone(),
        OutboundPayload::PresentationBlocks { .. } => String::new(),
        OutboundPayload::NativeChannelData { data } => data
            .get("content")
            .or_else(|| data.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}
