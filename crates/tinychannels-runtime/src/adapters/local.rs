//! In-process adapter for host-owned channel surfaces.

use crate::channel::{
    ChannelAdapter, ChannelDescriptor, ChannelInboundSink, ChannelOutboundIntent, ChannelSendError,
    ChannelStaticCapabilities, MessageReceipt,
};
use crate::controllers::{ChannelAccountSnapshot, ChannelAccountState};
use async_trait::async_trait;
use std::sync::Arc;

/// Host callback used by `LocalChannelAdapter` to perform delivery.
#[async_trait]
pub trait LocalOutboundSink: Send + Sync {
    async fn send(&self, intent: ChannelOutboundIntent)
    -> Result<MessageReceipt, ChannelSendError>;
}

/// Minimal generic adapter for local/API/webhook style host integrations.
pub struct LocalChannelAdapter {
    descriptor: ChannelDescriptor,
    static_capabilities: ChannelStaticCapabilities,
    outbound: Arc<dyn LocalOutboundSink>,
}

impl LocalChannelAdapter {
    pub fn new(
        descriptor: ChannelDescriptor,
        static_capabilities: ChannelStaticCapabilities,
        outbound: Arc<dyn LocalOutboundSink>,
    ) -> Self {
        Self {
            descriptor,
            static_capabilities,
            outbound,
        }
    }

    pub fn static_capabilities(&self) -> &ChannelStaticCapabilities {
        &self.static_capabilities
    }
}

#[async_trait]
impl ChannelAdapter for LocalChannelAdapter {
    fn descriptor(&self) -> ChannelDescriptor {
        self.descriptor.clone()
    }

    async fn start(
        &self,
        _sink: &(dyn ChannelInboundSink + Send + Sync),
    ) -> Result<(), ChannelSendError> {
        Ok(())
    }

    async fn stop(&self) -> Result<(), ChannelSendError> {
        Ok(())
    }

    async fn send(
        &self,
        intent: ChannelOutboundIntent,
    ) -> Result<MessageReceipt, ChannelSendError> {
        self.outbound.send(intent).await
    }

    async fn status(&self) -> Result<ChannelAccountSnapshot, ChannelSendError> {
        Ok(ChannelAccountSnapshot {
            channel_id: self.descriptor.id.clone(),
            account_id: self.descriptor.account_id.clone(),
            states: vec![
                ChannelAccountState::Configured,
                ChannelAccountState::Enabled,
            ],
            ..Default::default()
        })
    }
}
