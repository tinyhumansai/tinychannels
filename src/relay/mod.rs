//! Relay connector contract primitives.

pub mod actions;
pub mod auth;
pub mod descriptor;
pub mod frames;
pub mod transport;
#[cfg(feature = "relay-websocket")]
pub mod websocket;

pub use actions::relay_send_action_from_outbound_intent;
pub use auth::{
    DEFAULT_MAX_SKEW_SECONDS, DEFAULT_UPGRADE_TTL_SECONDS, DELIVERY_SIG_HEADER, DELIVERY_TS_HEADER,
    delivery_payload, make_token, make_token_at, make_upgrade_token, make_upgrade_token_at, sign,
    verify_delivery_signature, verify_delivery_signature_at, verify_signature, verify_token,
    verify_token_at,
};
pub use descriptor::{
    CONTRACT_VERSION, CapabilityDescriptor, DEFAULT_MAX_MESSAGE_LENGTH, RelayDescriptorOptions,
    RelayPlatformEntry,
};
pub use frames::{
    AuthenticatedRelayInboundEvent, ConnectorToGatewayFrame, FRAME_DESCRIPTOR, FRAME_GOING_IDLE,
    FRAME_GOING_IDLE_ACK, FRAME_HELLO, FRAME_INBOUND, FRAME_INBOUND_ACK, FRAME_INTERRUPT,
    FRAME_INTERRUPT_INBOUND, FRAME_OUTBOUND, FRAME_OUTBOUND_RESULT, FRAME_PASSTHROUGH_FORWARD,
    GatewayToConnectorFrame, PassthroughForward,
};
pub use transport::{
    RelayFrameDialer, RelayFrameIo, RelayIdentity, RelayInboundHandler,
    RelayInterruptInboundHandler, RelayPassthroughHandler, RelayReconnectHandle,
    RelayReconnectPolicy, RelayTransport, RelayTransportError, RelayTransportTimeouts,
};
#[cfg(feature = "relay-websocket")]
pub use websocket::{
    WebSocketRelayConfig, WebSocketRelayDialer, WebSocketRelayIo, connect_websocket_relay_io,
    websocket_dial_url, websocket_upgrade_authorization,
};

#[cfg(test)]
mod test;
