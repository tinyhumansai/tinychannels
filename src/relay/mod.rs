//! Relay connector transport.
//!
//! The contract primitives — frames, HMAC auth, the capability descriptor, and
//! the identities/timeouts/handler traits the loop speaks — live in
//! [`tinychannels_bus::relay`] and are re-exported here. What remains in this
//! crate is the part that needs a runtime: the frame transport loop and the
//! WebSocket dialer.

pub mod runtime;
pub mod transport;
#[cfg(feature = "relay-websocket")]
pub mod websocket;

pub use runtime::{
    current_relay_transport, register_relay_transport, relay_runtime_fronts_channel,
    send_outbound_intent, unregister_relay_transport,
};
pub use tinychannels_bus::relay::{
    AuthenticatedRelayInboundEvent, CONTRACT_VERSION, CapabilityDescriptor,
    ConnectorToGatewayFrame, DEFAULT_MAX_MESSAGE_LENGTH, DEFAULT_MAX_SKEW_SECONDS,
    DEFAULT_UPGRADE_TTL_SECONDS, DELIVERY_SIG_HEADER, DELIVERY_TS_HEADER, FRAME_DESCRIPTOR,
    FRAME_GOING_IDLE, FRAME_GOING_IDLE_ACK, FRAME_HELLO, FRAME_INBOUND, FRAME_INBOUND_ACK,
    FRAME_INTERRUPT, FRAME_INTERRUPT_INBOUND, FRAME_OUTBOUND, FRAME_OUTBOUND_RESULT,
    FRAME_PASSTHROUGH_FORWARD, GatewayToConnectorFrame, PassthroughForward, RelayDescriptorOptions,
    RelayFrameDialer, RelayFrameIo, RelayIdentity, RelayInboundHandler,
    RelayInterruptInboundHandler, RelayPassthroughHandler, RelayPlatformEntry,
    RelayReconnectPolicy, RelayTransportError, RelayTransportTimeouts, actions, auth,
    delivery_payload, descriptor, frames, make_token, make_token_at, make_upgrade_token,
    make_upgrade_token_at, relay_send_action_from_outbound_intent, sign, verify_delivery_signature,
    verify_delivery_signature_at, verify_signature, verify_token, verify_token_at,
};
pub use transport::{RelayReconnectHandle, RelayTransport};
#[cfg(feature = "relay-websocket")]
pub use websocket::{
    WebSocketRelayConfig, WebSocketRelayDialer, WebSocketRelayIo, connect_websocket_relay_io,
    websocket_dial_url, websocket_upgrade_authorization,
};

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;
