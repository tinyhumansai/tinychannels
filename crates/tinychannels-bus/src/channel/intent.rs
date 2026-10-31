//! Outbound channel intent types.

// Preserve the earlier Rust import path without declaring a second outcome type.
pub use super::receipt::ChannelSendOutcome;

use crate::channel::envelope::{ContentPart, MediaReference};
use crate::channel::types::{ChannelRef, ConversationRef};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

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
#[allow(
    clippy::large_enum_variant,
    reason = "the v22 wire enum keeps ergonomic, unboxed payload variants"
)]
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum OutboundPayload {
    Text {
        text: String,
    },
    Media {
        text: Option<String>,
        media_urls: Vec<String>,
    },
    Voice {
        voice: OutboundVoice,
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
    /// Canonical ordered v22 content. This route carries every content kind.
    Parts {
        parts: Vec<OutboundContentPart>,
    },
}

#[derive(Deserialize)]
#[allow(
    clippy::large_enum_variant,
    reason = "the compatibility wire enum preserves the public unboxed payload shape"
)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum OutboundPayloadWire {
    Text {
        text: String,
    },
    Media {
        text: Option<String>,
        #[serde(alias = "media_urls")]
        media_urls: Vec<String>,
    },
    Voice {
        voice: OutboundVoice,
    },
    Files {
        #[serde(alias = "file_urls")]
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
    Parts {
        parts: Vec<OutboundContentPart>,
    },
}

impl<'de> Deserialize<'de> for OutboundPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut value = Value::deserialize(deserializer)?;
        if value.get("kind").and_then(Value::as_str) == Some("voice")
            && let Some(media_url) = value.get("media_url").and_then(Value::as_str)
        {
            value = serde_json::json!({
                "kind": "voice",
                "voice": {
                    "kind": "media",
                    "media": { "url": media_url, "kind": "audio" },
                    "codec": null,
                    "durationMs": null
                }
            });
        }
        let wire: OutboundPayloadWire =
            serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(match wire {
            OutboundPayloadWire::Text { text } => Self::Text { text },
            OutboundPayloadWire::Media { text, media_urls } => Self::Media { text, media_urls },
            OutboundPayloadWire::Voice { voice } => Self::Voice { voice },
            OutboundPayloadWire::Files { file_urls } => Self::Files { file_urls },
            OutboundPayloadWire::Poll { question, options } => Self::Poll { question, options },
            OutboundPayloadWire::PresentationBlocks { blocks } => {
                Self::PresentationBlocks { blocks }
            }
            OutboundPayloadWire::NativeChannelData { data } => Self::NativeChannelData { data },
            OutboundPayloadWire::Parts { parts } => Self::Parts { parts },
        })
    }
}

/// Explicit outbound voice operation. Synthesis is opt-in and keeps its text.
#[allow(
    clippy::large_enum_variant,
    reason = "the v22 voice wire union keeps its media reference directly typed"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum OutboundVoice {
    Media {
        media: MediaReference,
        codec: Option<String>,
        duration_ms: Option<u64>,
    },
    Synthesize {
        text: String,
        voice: Option<String>,
    },
}

/// Typed outbound parts accepted by migrated provider adapters.
#[allow(
    clippy::large_enum_variant,
    reason = "the v22 wire enum keeps content parts directly represented"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum OutboundContentPart {
    Content { part: ContentPart },
    Voice { voice: OutboundVoice },
}

/// Complete provider-facing v2 send with ordered content and lossless addressing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelOutboundMessage {
    pub channel_id: String,
    pub conversation_id: String,
    /// Full account address when supplied by a v22 caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_ref: Option<ChannelRef>,
    /// Full conversation address, including independent thread and topic IDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_ref: Option<ConversationRef>,
    pub reply_to_id: Option<String>,
    pub thread_id: Option<String>,
    /// Originating agent turn used to scope opaque host media references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub idempotency_key: String,
    pub parts: Vec<OutboundContentPart>,
}

impl ChannelOutboundMessage {
    /// Adapt an intent to typed ordered parts without serializing payloads into text.
    pub fn from_intent(intent: &ChannelOutboundIntent) -> Self {
        use crate::channel::envelope::MediaKind;
        let mut parts = Vec::new();
        match &intent.payload {
            OutboundPayload::Text { text } => parts.push(OutboundContentPart::Content {
                part: ContentPart::Text {
                    text: text.clone(),
                    markdown_dialect: None,
                },
            }),
            OutboundPayload::Media { text, media_urls } => {
                if let Some(text) = text {
                    parts.push(OutboundContentPart::Content {
                        part: ContentPart::Text {
                            text: text.clone(),
                            markdown_dialect: None,
                        },
                    });
                }
                parts.extend(media_urls.iter().map(|url| OutboundContentPart::Content {
                    part: ContentPart::Image {
                        media: MediaReference {
                            url: Some(url.clone()),
                            kind: MediaKind::Image,
                            ..Default::default()
                        },
                        caption: None,
                        alt_text: None,
                    },
                }));
            }
            OutboundPayload::Voice { voice } => parts.push(OutboundContentPart::Voice {
                voice: voice.clone(),
            }),
            OutboundPayload::Files { file_urls } => {
                parts.extend(file_urls.iter().map(|url| OutboundContentPart::Content {
                    part: ContentPart::File {
                        media: MediaReference {
                            url: Some(url.clone()),
                            kind: MediaKind::Document,
                            ..Default::default()
                        },
                        file_name: None,
                        caption: None,
                    },
                }))
            }
            OutboundPayload::Poll { question, options } => {
                parts.push(OutboundContentPart::Content {
                    part: ContentPart::Poll {
                        question: question.clone(),
                        options: options.clone(),
                        multiple_choice: false,
                        closes_at_unix_ms: None,
                        provider_poll_id: None,
                    },
                })
            }
            OutboundPayload::PresentationBlocks { blocks } => {
                parts.push(OutboundContentPart::Content {
                    part: ContentPart::Presentation {
                        blocks: blocks.clone(),
                    },
                })
            }
            OutboundPayload::NativeChannelData { data } => {
                parts.push(OutboundContentPart::Content {
                    part: ContentPart::Native {
                        provider: intent.channel_id.clone(),
                        data: data.clone(),
                    },
                })
            }
            OutboundPayload::Parts { parts: supplied } => parts.extend(supplied.iter().cloned()),
        }
        Self {
            channel_id: intent
                .channel_ref
                .as_ref()
                .map(|channel| channel.id.clone())
                .unwrap_or_else(|| intent.channel_id.clone()),
            conversation_id: intent
                .conversation_ref
                .as_ref()
                .map(|conversation| conversation.id.clone())
                .unwrap_or_else(|| intent.conversation_id.clone()),
            channel_ref: intent.channel_ref.clone(),
            conversation_ref: intent.conversation_ref.clone().map(|mut reference| {
                if reference.thread_id.is_none() {
                    reference.thread_id.clone_from(&intent.thread_id);
                }
                reference
            }),
            reply_to_id: intent.reply_to_id.clone(),
            thread_id: intent.thread_id.clone().or_else(|| {
                intent
                    .conversation_ref
                    .as_ref()
                    .and_then(|reference| reference.thread_id.clone())
            }),
            turn_id: intent.turn_id.clone(),
            idempotency_key: intent.idempotency_key.clone(),
            parts,
        }
    }

    /// Build the documented one-release text-only v21 projection.
    pub fn legacy_text(&self) -> crate::Result<String> {
        let mut text = Vec::new();
        for part in &self.parts {
            match part {
                OutboundContentPart::Content {
                    part:
                        ContentPart::Text {
                            text: value,
                            markdown_dialect: None,
                        },
                } => text.push(value.as_str()),
                _ => {
                    return Err(crate::TinyChannelsError::new(
                        "provider does not support typed channel content",
                    ));
                }
            }
        }
        Ok(text.join("\n"))
    }

    /// Convert a legacy message into the v22 typed envelope without inferring
    /// provider capabilities or parsing its text as structured data.
    pub fn from_legacy_message(
        channel_id: impl Into<String>,
        message: &crate::traits::SendMessage,
    ) -> Self {
        Self {
            channel_id: channel_id.into(),
            conversation_id: message.recipient.clone(),
            channel_ref: None,
            conversation_ref: Some(ConversationRef {
                id: message.recipient.clone(),
                thread_id: message.thread_ts.clone(),
                topic_id: message.topic_id.clone(),
                ..Default::default()
            }),
            reply_to_id: message.reply_to_id.clone(),
            thread_id: message.thread_ts.clone(),
            turn_id: None,
            idempotency_key: message.idempotency_key.clone().unwrap_or_default(),
            parts: vec![OutboundContentPart::Content {
                part: ContentPart::Text {
                    text: message.content.clone(),
                    markdown_dialect: None,
                },
            }],
        }
    }

    /// Convert to the explicit one-release legacy text-only send shape.
    pub fn to_legacy_message(&self) -> crate::Result<crate::traits::SendMessage> {
        Ok(crate::traits::SendMessage {
            content: self.legacy_text()?,
            recipient: self
                .conversation_ref
                .as_ref()
                .map(|conversation| conversation.id.clone())
                .unwrap_or_else(|| self.conversation_id.clone()),
            subject: None,
            thread_ts: self.thread_id.clone().or_else(|| {
                self.conversation_ref
                    .as_ref()
                    .and_then(|conversation| conversation.thread_id.clone())
            }),
            topic_id: self
                .conversation_ref
                .as_ref()
                .and_then(|reference| reference.topic_id.clone()),
            reply_to_id: self.reply_to_id.clone(),
            idempotency_key: (!self.idempotency_key.is_empty())
                .then(|| self.idempotency_key.clone()),
        })
    }
}

/// Logical outbound send request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelOutboundIntent {
    pub idempotency_key: String,
    pub channel_id: String,
    pub conversation_id: String,
    /// Optional lossless account address for v22 callers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_ref: Option<ChannelRef>,
    /// Optional lossless conversation scope for v22 callers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_ref: Option<ConversationRef>,
    pub reply_to_id: Option<String>,
    pub thread_id: Option<String>,
    /// Originating agent turn used to scope opaque host media references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub durability: DeliveryDurability,
    pub payload: OutboundPayload,
}

impl Default for ChannelOutboundIntent {
    fn default() -> Self {
        Self {
            idempotency_key: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            channel_ref: None,
            conversation_ref: None,
            reply_to_id: None,
            thread_id: None,
            turn_id: None,
            durability: DeliveryDurability::BestEffort,
            payload: OutboundPayload::Text {
                text: String::new(),
            },
        }
    }
}

#[cfg(test)]
#[path = "intent_tests.rs"]
mod tests;
