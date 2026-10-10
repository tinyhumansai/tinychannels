//! Process-wide registry for the running relay transport.
//!
//! A channel runtime that fronts one or more platforms through the relay
//! registers its [`RelayTransport`] here, so controller-initiated sends (which
//! have no handle on the runtime) can route an outbound intent through it.

use std::sync::{Arc, OnceLock, RwLock};
use tinychannels_runtime::config::RelayRuntimeConfigExt as _;

use anyhow::Result;
use serde_json::Value;
use tinychannels_bus::controllers::ChannelSendMessageResult;
use tinychannels_bus::{ChannelOutboundIntent, ChannelsConfig};

use super::{RelayTransport, relay_send_action_from_outbound_intent};

static RELAY_TRANSPORT: OnceLock<RwLock<Option<Arc<RelayTransport>>>> = OnceLock::new();

fn relay_transport_slot() -> &'static RwLock<Option<Arc<RelayTransport>>> {
    RELAY_TRANSPORT.get_or_init(|| RwLock::new(None))
}

/// Make `transport` the process's current relay transport.
pub fn register_relay_transport(transport: Arc<RelayTransport>) {
    let mut slot = relay_transport_slot()
        .write()
        .unwrap_or_else(|error| error.into_inner());
    *slot = Some(transport);
    tracing::debug!("[channels][relay] registered runtime transport");
}

/// Clear the slot, but only if it still holds `transport`: a newer runtime
/// that registered in the meantime keeps its transport.
pub fn unregister_relay_transport(transport: &Arc<RelayTransport>) {
    let mut slot = relay_transport_slot()
        .write()
        .unwrap_or_else(|error| error.into_inner());
    if slot
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, transport))
    {
        *slot = None;
        tracing::debug!("[channels][relay] unregistered stopped runtime transport");
    }
}

/// The currently registered transport, if a relay runtime is running.
pub fn current_relay_transport() -> Option<Arc<RelayTransport>> {
    relay_transport_slot()
        .read()
        .ok()
        .and_then(|slot| slot.clone())
}

/// Whether the configured relay listener fronts `channel`'s platform.
pub fn relay_runtime_fronts_channel(config: &ChannelsConfig, channel: &str) -> bool {
    config
        .relay
        .as_ref()
        .filter(|relay| relay.is_listener_configured())
        .is_some_and(|relay| {
            relay
                .identities
                .iter()
                .any(|identity| identity.platform == channel)
        })
}

/// Send `intent` through the registered relay transport. `Ok(None)` means no
/// relay runtime is running and the caller should use another route.
pub async fn send_outbound_intent(
    intent: &ChannelOutboundIntent,
) -> Result<Option<ChannelSendMessageResult>> {
    let Some(transport) = current_relay_transport() else {
        return Ok(None);
    };
    let action = relay_send_action_from_outbound_intent(intent);
    let result = transport
        .send_outbound(action, Some(&intent.channel_id))
        .await
        .map_err(|error| anyhow::anyhow!("relay outbound failed: {error}"))?;
    Ok(Some(relay_send_message_result(result)?))
}

fn relay_send_message_result(result: Value) -> Result<ChannelSendMessageResult> {
    if result.get("success").and_then(Value::as_bool) == Some(false) {
        let error = result
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("relay outbound failed");
        return Err(anyhow::anyhow!("relay outbound failed: {error}"));
    }
    Ok(ChannelSendMessageResult {
        message_id: result
            .get("message_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        raw: Some(result),
        ..Default::default()
    })
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
