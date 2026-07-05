use crate::adapters::{LocalChannelAdapter, LocalOutboundSink};
use crate::channel::{
    ChannelAdapter, ChannelDescriptor, ChannelInboundEnvelope, ChannelInboundSink,
    ChannelOutboundIntent, ChannelSendError, ChannelStaticCapabilities, DeliveryDurability,
    MessageReceipt, OutboundPayload,
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct RecordingOutboundSink {
    ids: Mutex<Vec<String>>,
}

#[async_trait]
impl LocalOutboundSink for RecordingOutboundSink {
    async fn send(
        &self,
        intent: ChannelOutboundIntent,
    ) -> Result<MessageReceipt, ChannelSendError> {
        self.ids
            .lock()
            .expect("ids lock")
            .push(intent.idempotency_key);
        Ok(MessageReceipt {
            primary_platform_message_id: Some("local-1".into()),
            platform_message_ids: vec!["local-1".into()],
            sent_at: 1,
            ..Default::default()
        })
    }
}

struct NoopInboundSink;

#[async_trait]
impl ChannelInboundSink for NoopInboundSink {
    async fn push(&self, _envelope: ChannelInboundEnvelope) -> Result<(), ChannelSendError> {
        Ok(())
    }
}

fn local_adapter(sink: Arc<RecordingOutboundSink>) -> LocalChannelAdapter {
    LocalChannelAdapter::new(
        ChannelDescriptor {
            id: "local".into(),
            display_name: "Local".into(),
            account_id: Some("host".into()),
            ..Default::default()
        },
        ChannelStaticCapabilities {
            reply: true,
            ..Default::default()
        },
        sink,
    )
}

#[tokio::test]
async fn local_adapter_delegates_outbound_delivery() {
    let sink = Arc::new(RecordingOutboundSink::default());
    let adapter = local_adapter(sink.clone());

    let receipt = adapter
        .send(ChannelOutboundIntent {
            idempotency_key: "turn-1:text".into(),
            channel_id: "local".into(),
            conversation_id: "chat-1".into(),
            reply_to_id: None,
            thread_id: None,
            durability: DeliveryDurability::BestEffort,
            payload: OutboundPayload::Text { text: "hi".into() },
        })
        .await
        .expect("send");

    assert_eq!(
        receipt.primary_platform_message_id.as_deref(),
        Some("local-1")
    );
    assert_eq!(
        sink.ids.lock().expect("ids lock").as_slice(),
        ["turn-1:text"]
    );
}

#[tokio::test]
async fn local_adapter_reports_configured_status() {
    let adapter = local_adapter(Arc::new(RecordingOutboundSink::default()));
    adapter.start(&NoopInboundSink).await.expect("start");

    let status = adapter.status().await.expect("status");
    assert_eq!(status.channel_id, "local");
    assert_eq!(status.account_id.as_deref(), Some("host"));
    assert!(
        status
            .states
            .iter()
            .any(|state| { *state == crate::controllers::ChannelAccountState::Configured })
    );
    assert!(adapter.static_capabilities().reply);
}
