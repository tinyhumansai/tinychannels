//! Adapter traits for direct providers, relay connectors, and host bridges.

use crate::channel::{
    ChannelDescriptor, ChannelInboundEnvelope, ChannelOutboundIntent, ChannelSendError,
    MessageReceipt,
};
use crate::controllers::ChannelAccountSnapshot;
use async_trait::async_trait;
use serde_json::Value;

/// Inbound receive acknowledgement timing.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ChannelReceiveAckPolicy {
    #[default]
    AfterReceiveRecord,
    AfterAgentDispatch,
    AfterDurableSend,
    Manual,
}

/// Sink used by adapters to deliver normalized inbound envelopes.
#[async_trait]
pub trait ChannelInboundSink: Send + Sync {
    async fn push(&self, envelope: ChannelInboundEnvelope) -> Result<(), ChannelSendError>;
}

/// Base channel adapter contract.
#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    fn descriptor(&self) -> ChannelDescriptor;

    fn receive_ack_policy(&self) -> ChannelReceiveAckPolicy {
        ChannelReceiveAckPolicy::AfterReceiveRecord
    }

    async fn start(
        &self,
        sink: &(dyn ChannelInboundSink + Send + Sync),
    ) -> Result<(), ChannelSendError>;

    async fn stop(&self) -> Result<(), ChannelSendError>;

    async fn send(&self, intent: ChannelOutboundIntent)
    -> Result<MessageReceipt, ChannelSendError>;

    async fn status(&self) -> Result<ChannelAccountSnapshot, ChannelSendError>;
}

/// Optional setup/config extension.
#[async_trait]
pub trait ChannelSetup: Send + Sync {
    async fn validate_setup(&self, input: Value) -> Result<Value, ChannelSendError>;
}

/// Optional directory extension.
#[async_trait]
pub trait ChannelDirectory: Send + Sync {
    async fn list_conversations(&self, query: Option<&str>)
    -> Result<Vec<Value>, ChannelSendError>;
}

/// Optional target resolver extension.
#[async_trait]
pub trait ChannelResolver: Send + Sync {
    async fn resolve_target(&self, target: &str) -> Result<Value, ChannelSendError>;
}

/// Optional typing indicator extension.
#[async_trait]
pub trait ChannelTyping: Send + Sync {
    async fn start_typing(&self, conversation_id: &str) -> Result<(), ChannelSendError>;
    async fn stop_typing(&self, conversation_id: &str) -> Result<(), ChannelSendError>;
}

/// Optional reaction extension.
#[async_trait]
pub trait ChannelReaction: Send + Sync {
    async fn send_reaction(
        &self,
        conversation_id: &str,
        message_id: &str,
        reaction: &str,
    ) -> Result<(), ChannelSendError>;
}

/// Optional edit extension.
#[async_trait]
pub trait ChannelEdit: Send + Sync {
    async fn edit(
        &self,
        receipt: &MessageReceipt,
        intent: ChannelOutboundIntent,
    ) -> Result<MessageReceipt, ChannelSendError>;
}

/// Optional delete/unsend extension.
#[async_trait]
pub trait ChannelDelete: Send + Sync {
    async fn delete(&self, receipt: &MessageReceipt) -> Result<(), ChannelSendError>;
}

/// Optional streaming draft extension.
#[async_trait]
pub trait ChannelStreamingDraft: Send + Sync {
    async fn upsert_draft(
        &self,
        previous: Option<&MessageReceipt>,
        intent: ChannelOutboundIntent,
    ) -> Result<MessageReceipt, ChannelSendError>;
}
