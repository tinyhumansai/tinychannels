//! Capability-driven bridge from harness output events to outbound intents.

use crate::channel::{
    ChannelOutboundIntent, ChannelPresentationCapabilities, ChannelStaticCapabilities,
    DeliveryDurability, OutboundPayload,
};
use crate::harness::types::{ChannelOutputEvent, ChannelTurn};
use serde_json::json;

/// Translation knobs for harness output delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeTranslationOptions {
    pub draft_throttle_ms: u64,
    pub flood_backoff_ms: u64,
    pub durability: DeliveryDurability,
}

impl Default for BridgeTranslationOptions {
    fn default() -> Self {
        Self {
            draft_throttle_ms: 750,
            flood_backoff_ms: 3000,
            durability: DeliveryDurability::BestEffort,
        }
    }
}

/// Translate one harness output event into outbound channel intents.
pub fn translate_output_event(
    turn: &ChannelTurn,
    event: ChannelOutputEvent,
    static_capabilities: &ChannelStaticCapabilities,
    presentation: &ChannelPresentationCapabilities,
    options: BridgeTranslationOptions,
) -> Vec<ChannelOutboundIntent> {
    let payload = match event {
        ChannelOutputEvent::TextDelta { text } => {
            if presentation.supports_edit && !static_capabilities.block_streaming {
                OutboundPayload::NativeChannelData {
                    data: json!({
                        "draft": true,
                        "editInPlace": true,
                        "text": text,
                        "throttleMs": options.draft_throttle_ms,
                        "floodBackoffMs": options.flood_backoff_ms,
                    }),
                }
            } else {
                OutboundPayload::Text { text }
            }
        }
        ChannelOutputEvent::FinalMessage { text } => OutboundPayload::Text { text },
        ChannelOutputEvent::ToolProgress { label, message } => OutboundPayload::Text {
            text: match message {
                Some(message) => format!("{label}: {message}"),
                None => label,
            },
        },
        ChannelOutputEvent::ApprovalRequest {
            id,
            prompt,
            choices,
        } => {
            if supports_native_approval(presentation, choices.len()) {
                OutboundPayload::PresentationBlocks {
                    blocks: json!({
                        "kind": "approval",
                        "id": id,
                        "prompt": prompt,
                        "choices": choices,
                    }),
                }
            } else {
                OutboundPayload::Text {
                    text: format!("{prompt}\n\nReply with one of: {}", choices.join(", ")),
                }
            }
        }
        ChannelOutputEvent::Clarification { prompt } => OutboundPayload::Text { text: prompt },
        ChannelOutputEvent::Media { text, media } => OutboundPayload::Media {
            text,
            media_urls: media
                .into_iter()
                .filter_map(|item| item.url.or(item.path))
                .collect(),
        },
        ChannelOutputEvent::Cancellation { reason } => OutboundPayload::Text {
            text: reason.unwrap_or_else(|| "Cancelled.".to_string()),
        },
        ChannelOutputEvent::Lifecycle { event } => OutboundPayload::NativeChannelData {
            data: json!({ "lifecycle": event }),
        },
        ChannelOutputEvent::Native { data } => OutboundPayload::NativeChannelData { data },
    };

    vec![ChannelOutboundIntent {
        idempotency_key: format!("{}:{}", turn.id, intent_suffix(&payload)),
        channel_id: turn.envelope.channel.id.clone(),
        conversation_id: turn.envelope.conversation.id.clone(),
        reply_to_id: Some(turn.envelope.message_id.clone()).filter(|id| !id.is_empty()),
        thread_id: turn
            .envelope
            .conversation
            .topic_id
            .clone()
            .or_else(|| turn.envelope.conversation.thread_id.clone()),
        durability: options.durability,
        payload,
    }]
}

fn supports_native_approval(
    presentation: &ChannelPresentationCapabilities,
    choice_count: usize,
) -> bool {
    presentation
        .max_actions
        .is_some_and(|max| max >= choice_count)
        && presentation.max_actions_per_row.unwrap_or(1) > 0
}

fn intent_suffix(payload: &OutboundPayload) -> &'static str {
    match payload {
        OutboundPayload::Text { .. } => "text",
        OutboundPayload::Media { .. } => "media",
        OutboundPayload::Voice { .. } => "voice",
        OutboundPayload::Files { .. } => "files",
        OutboundPayload::Poll { .. } => "poll",
        OutboundPayload::PresentationBlocks { .. } => "presentation",
        OutboundPayload::NativeChannelData { .. } => "native",
    }
}
