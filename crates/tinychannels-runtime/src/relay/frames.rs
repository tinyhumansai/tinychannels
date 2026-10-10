//! Relay acknowledgement and authenticated inbound projection.
use serde_json::Value;
pub use tinychannels_bus::relay::frames::*;
/// Runtime facts established only after the relay transport authenticated.
pub trait ConnectorToGatewayFrameExt {
    fn inbound_ack(&self) -> Option<GatewayToConnectorFrame>;
    fn authenticated_inbound_event(&self) -> Option<AuthenticatedRelayInboundEvent>;
}
impl ConnectorToGatewayFrameExt for ConnectorToGatewayFrame {
    fn inbound_ack(&self) -> Option<GatewayToConnectorFrame> {
        match self {
            Self::Inbound {
                buffer_id: Some(buffer_id),
                ..
            }
            | Self::PassthroughForward {
                buffer_id: Some(buffer_id),
                ..
            } => Some(GatewayToConnectorFrame::InboundAck {
                buffer_id: buffer_id.clone(),
            }),
            _ => None,
        }
    }

    fn authenticated_inbound_event(&self) -> Option<AuthenticatedRelayInboundEvent> {
        let Self::Inbound { event, buffer_id } = self else {
            return None;
        };
        let mut event = event.clone();
        strip_forged_relay_trust(&mut event);
        Some(AuthenticatedRelayInboundEvent {
            event,
            buffer_id: buffer_id.clone(),
            delivered_via_authenticated_relay: true,
        })
    }
}
fn strip_forged_relay_trust(event: &mut Value) {
    for key in ["source", "access"] {
        if let Some(object) = event.get_mut(key).and_then(Value::as_object_mut) {
            object.remove("delivered_via_upstream_relay");
            object.remove("deliveredViaUpstreamRelay");
        }
    }
}
