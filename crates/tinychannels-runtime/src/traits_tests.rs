// Scoped allowance for the one-release v21 wire DTO compatibility surface.
#![allow(deprecated)]

use crate::channel;
use crate::traits::{Channel, ChannelMessage, ChannelSendExt, SendMessage};
use async_trait::async_trait;

struct LegacyChannel;

#[async_trait]
impl crate::traits::Channel for LegacyChannel {
    fn name(&self) -> &str {
        "legacy"
    }

    async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
        Ok(())
    }

    async fn listen(&self, _tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        Ok(())
    }
}

struct CapableChannel;

#[async_trait]
impl crate::traits::Channel for CapableChannel {
    fn name(&self) -> &str {
        "capable"
    }

    async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
        Ok(())
    }

    async fn listen(&self, _tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        Ok(())
    }

    fn supports_remote_control(&self) -> bool {
        true
    }

    fn supports_chat_approvals(&self) -> bool {
        true
    }

    fn history_key_ignores_thread(&self) -> bool {
        true
    }

    fn supports_reactions(&self) -> bool {
        true
    }

    fn supports_draft_updates(&self) -> bool {
        true
    }

    fn supports_edit(&self) -> bool {
        true
    }

    fn supports_reply_to_id(&self) -> bool {
        true
    }

    fn supports_thread_id(&self) -> bool {
        true
    }

    fn supports_topic_id(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn legacy_channel_defaults_are_safe_and_capabilities_are_composed() {
    let channel = LegacyChannel;
    assert_eq!(channel.name(), "legacy");
    assert_eq!(channel.proactive_target(), None);
    assert!(channel.health_check().await);
    channel.start_typing("room").await.unwrap();
    channel.stop_typing("room").await.unwrap();
    assert_eq!(channel.begin_typing_refresh("room", 5).await.unwrap(), None);
    channel.end_typing_refresh("handle").await.unwrap();
    assert_eq!(
        channel
            .send_draft(&SendMessage::new("draft", "room"))
            .await
            .unwrap(),
        None
    );
    channel
        .update_draft("room", "draft-1", "updated")
        .await
        .unwrap();
    channel
        .finalize_draft("room", "draft-1", "final", None)
        .await
        .unwrap();

    let capabilities = channel.capabilities();
    assert!(!capabilities.remote_control);
    assert!(!capabilities.chat_approvals);
    assert!(!capabilities.reactions);
    assert!(!capabilities.progressive_edits);
    assert!(capabilities.content.text.inbound);
    assert!(capabilities.content.text.outbound);
    assert_eq!(capabilities.actions.supported, ["send"]);

    let capable = CapableChannel.capabilities();
    assert!(capable.remote_control);
    assert!(capable.chat_approvals);
    assert!(capable.history_key_ignores_thread);
    assert!(capable.progressive_edits);
    assert!(capable.streaming_reply_pipeline);
    assert!(capable.reactions);
    assert!(capable.addressing.reply_to_id);
    assert!(capable.addressing.thread_id);
    assert!(capable.addressing.topic_id);
    for action in [
        "draft-update",
        "edit",
        "reply",
        "thread-reply",
        "react",
        "reactions",
    ] {
        assert!(
            capable
                .actions
                .supported
                .iter()
                .any(|value| value == action)
        );
    }
}

#[tokio::test]
async fn legacy_send_helpers_preserve_text_contract_and_empty_receipt() {
    let channel = LegacyChannel;
    let message = SendMessage::new("hello", "room").with_idempotency_key("caller-key");
    channel.send_idempotent(&message).await.unwrap();
    channel.send_with_outbound_intent(&message).await.unwrap();

    let typed = tinychannels_bus::ChannelOutboundMessage {
        channel_id: "legacy".into(),
        conversation_id: "room".into(),
        channel_ref: None,
        conversation_ref: None,
        reply_to_id: None,
        thread_id: None,
        turn_id: None,
        idempotency_key: "typed-1".into(),
        parts: vec![tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Text {
                text: "typed hello".into(),
                markdown_dialect: None,
            },
        }],
    };
    channel.validate_typed_send(&typed).unwrap();
    channel.send_typed(&typed).await.unwrap();
    assert_eq!(
        channel.send_typed_with_receipt(&typed).await.unwrap(),
        crate::channel::ChannelSendOutcome::default()
    );

    let rich = tinychannels_bus::ChannelOutboundMessage {
        parts: vec![tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Image {
                media: Default::default(),
                caption: None,
                alt_text: None,
            },
        }],
        ..typed
    };
    assert!(channel.validate_typed_send(&rich).is_err());
    assert!(channel.send_typed(&rich).await.is_err());
}

#[test]
fn channel_message_clone_preserves_legacy_wire_fields() {
    let message = ChannelMessage {
        id: "42".into(),
        sender: "alice".into(),
        reply_target: "alice".into(),
        content: "ping".into(),
        channel: "dummy".into(),
        timestamp: 999,
        topic_id: None,
        reply_to_id: None,
        thread_ts: None,
        sender_name: None,
    };

    let cloned = message.clone();
    assert_eq!(cloned.id, "42");
    assert_eq!(cloned.sender, "alice");
    assert_eq!(cloned.reply_target, "alice");
    assert_eq!(cloned.content, "ping");
    assert_eq!(cloned.channel, "dummy");
    assert_eq!(cloned.timestamp, 999);
}

#[test]
fn send_message_generates_deterministic_idempotency_key() {
    let message = SendMessage::new("hello", "alice").in_thread(Some("thread-1".to_string()));
    let again = SendMessage::new("hello", "alice").in_thread(Some("thread-1".to_string()));
    let intent = channel::outbound_intent_from_send_message("telegram", &message);
    let repeated = channel::outbound_intent_from_send_message("telegram", &again);

    assert_eq!(intent.idempotency_key, repeated.idempotency_key);
    assert!(intent.idempotency_key.starts_with("legacy-send:telegram:"));
}

#[test]
fn explicit_send_message_idempotency_key_is_preserved() {
    let message = SendMessage::new("hello", "alice").with_idempotency_key("caller-key");

    let intent = channel::outbound_intent_from_send_message("telegram", &message);
    assert_eq!(intent.idempotency_key, "caller-key");
}

#[test]
fn v1_send_message_is_immediately_converted_to_a_scoped_typed_intent() {
    let message = SendMessage::new("hello", "alice")
        .in_thread(Some("thread-1".to_string()))
        .with_idempotency_key("caller-key");

    let intent = channel::outbound_intent_from_send_message("telegram", &message);
    assert_eq!(intent.channel_id, "telegram");
    assert_eq!(intent.conversation_id, "alice");
    assert_eq!(intent.idempotency_key, "caller-key");
    assert_eq!(intent.thread_id.as_deref(), Some("thread-1"));
    assert_eq!(
        intent
            .conversation_ref
            .as_ref()
            .and_then(|scope| scope.thread_id.as_deref()),
        Some("thread-1")
    );
    assert!(matches!(
        intent.payload,
        channel::OutboundPayload::NativeChannelData { .. }
    ));
}

#[test]
fn webhook_rejection_keeps_its_safe_wire_error_text() {
    let error = crate::traits::WebhookRejected("signature was rejected".into());
    assert_eq!(error.to_string(), "signature was rejected");
    let source: &dyn std::error::Error = &error;
    assert!(source.source().is_none());
}

struct Recording(std::sync::Mutex<Vec<String>>);

#[async_trait]
impl Channel for Recording {
    fn name(&self) -> &str {
        "recording"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(format!("send:{}", message.content));
        Ok(())
    }

    async fn listen(&self, _tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send_idempotent(&self, message: &SendMessage) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(format!("idem:{}", message.content));
        Ok(())
    }
}

#[tokio::test]
async fn send_with_outbound_intent_routes_through_send_idempotent() {
    let channel = Recording(Default::default());
    channel
        .send_with_outbound_intent(&SendMessage::new("x", "bob"))
        .await
        .unwrap();
    assert_eq!(*channel.0.lock().unwrap(), vec!["idem:x".to_string()]);
}
