//! Relay WebSocket frame contract.

use crate::relay::CapabilityDescriptor;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Gateway-to-connector frame names.
pub const FRAME_HELLO: &str = "hello";
pub const FRAME_OUTBOUND: &str = "outbound";
pub const FRAME_INTERRUPT: &str = "interrupt";
pub const FRAME_GOING_IDLE: &str = "going_idle";
pub const FRAME_INBOUND_ACK: &str = "inbound_ack";

/// Connector-to-gateway frame names.
pub const FRAME_DESCRIPTOR: &str = "descriptor";
pub const FRAME_INBOUND: &str = "inbound";
pub const FRAME_OUTBOUND_RESULT: &str = "outbound_result";
pub const FRAME_INTERRUPT_INBOUND: &str = "interrupt_inbound";
pub const FRAME_GOING_IDLE_ACK: &str = "going_idle_ack";
pub const FRAME_PASSTHROUGH_FORWARD: &str = "passthrough_forward";

/// Frames sent by a gateway over the relay socket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayToConnectorFrame {
    Hello {
        platform: String,
        #[serde(rename = "botId")]
        bot_id: String,
    },
    Outbound {
        #[serde(rename = "requestId")]
        request_id: String,
        action: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        platform: Option<String>,
        #[serde(rename = "botId", skip_serializing_if = "Option::is_none")]
        bot_id: Option<String>,
    },
    Interrupt {
        session_key: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    GoingIdle,
    InboundAck {
        #[serde(rename = "bufferId")]
        buffer_id: String,
    },
}

impl GatewayToConnectorFrame {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn from_json(data: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(data)
    }
}

/// Frames sent by a connector over the relay socket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectorToGatewayFrame {
    Descriptor {
        descriptor: CapabilityDescriptor,
    },
    Inbound {
        event: Value,
        #[serde(rename = "bufferId", skip_serializing_if = "Option::is_none")]
        buffer_id: Option<String>,
    },
    OutboundResult {
        #[serde(rename = "requestId")]
        request_id: String,
        result: Value,
    },
    InterruptInbound {
        session_key: String,
        chat_id: String,
    },
    GoingIdleAck,
    PassthroughForward {
        forward: PassthroughForward,
        #[serde(rename = "bufferId", skip_serializing_if = "Option::is_none")]
        buffer_id: Option<String>,
    },
}

impl ConnectorToGatewayFrame {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn from_json(data: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(data)
    }

    pub fn inbound_ack(&self) -> Option<GatewayToConnectorFrame> {
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

    pub fn authenticated_inbound_event(&self) -> Option<AuthenticatedRelayInboundEvent> {
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

/// Inbound relay delivery after the socket/auth layer established relay trust.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticatedRelayInboundEvent {
    pub event: Value,
    pub buffer_id: Option<String>,
    pub delivered_via_authenticated_relay: bool,
}

/// Connector-forwarded passthrough-plane request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct PassthroughForward {
    pub platform: String,
    #[serde(rename = "botId")]
    pub bot_id: String,
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    #[serde(rename = "bodyB64", with = "base64_body")]
    #[schemars(with = "String")]
    pub body: Vec<u8>,
}

fn strip_forged_relay_trust(event: &mut Value) {
    for key in ["source", "access"] {
        if let Some(object) = event.get_mut(key).and_then(Value::as_object_mut) {
            object.remove("delivered_via_upstream_relay");
            object.remove("deliveredViaUpstreamRelay");
        }
    }
}

mod base64_body {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        Ok(STANDARD.decode(encoded).unwrap_or_default())
    }
}
