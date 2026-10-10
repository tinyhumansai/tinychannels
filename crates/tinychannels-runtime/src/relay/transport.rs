//! In-process relay transport seams.
use crate::relay::{
    AuthenticatedRelayInboundEvent, ConnectorToGatewayFrame, GatewayToConnectorFrame,
    PassthroughForward,
};
use async_trait::async_trait;
use std::sync::Arc;
pub use tinychannels_bus::relay::transport::*;

/// Minimal frame I/O boundary used by the transport loop.
#[async_trait]
pub trait RelayFrameIo: Send + Sync {
    async fn send(&self, frame: GatewayToConnectorFrame) -> Result<(), RelayTransportError>;
    async fn recv(&self) -> Result<Option<ConnectorToGatewayFrame>, RelayTransportError>;
}

/// Dialer used by reconnect supervisors to acquire a fresh frame I/O.
#[async_trait]
pub trait RelayFrameDialer: Send + Sync {
    async fn dial(&self) -> Result<Arc<dyn RelayFrameIo>, RelayTransportError>;
}

#[async_trait]
impl<T> RelayFrameIo for Arc<T>
where
    T: RelayFrameIo + ?Sized,
{
    async fn send(&self, frame: GatewayToConnectorFrame) -> Result<(), RelayTransportError> {
        (**self).send(frame).await
    }

    async fn recv(&self) -> Result<Option<ConnectorToGatewayFrame>, RelayTransportError> {
        (**self).recv().await
    }
}

/// Handler for authenticated connector-to-gateway inbound events.
#[async_trait]
pub trait RelayInboundHandler: Send + Sync {
    async fn handle(
        &self,
        event: AuthenticatedRelayInboundEvent,
    ) -> Result<(), RelayTransportError>;
}

/// Handler for connector-forwarded passthrough requests.
#[async_trait]
pub trait RelayPassthroughHandler: Send + Sync {
    async fn handle(
        &self,
        forward: PassthroughForward,
        buffer_id: Option<String>,
    ) -> Result<(), RelayTransportError>;
}

/// Handler for connector-to-gateway interrupt requests.
#[async_trait]
pub trait RelayInterruptInboundHandler: Send + Sync {
    async fn handle(&self, session_key: String, chat_id: String)
    -> Result<(), RelayTransportError>;
}
