use super::*;
use tinychannels_bus::config::{RelayRuntimeConfig, RelayRuntimeIdentityConfig};

#[test]
fn relay_runtime_fronts_only_configured_identity_platforms() {
    let mut config = ChannelsConfig::default();
    assert!(!relay_runtime_fronts_channel(&config, "discord"));

    config.relay = Some(RelayRuntimeConfig {
        url: "wss://relay.example/relay".to_string(),
        identities: vec![RelayRuntimeIdentityConfig {
            platform: "discord".to_string(),
            bot_id: "app-1".to_string(),
        }],
        ..Default::default()
    });

    assert!(relay_runtime_fronts_channel(&config, "discord"));
    assert!(!relay_runtime_fronts_channel(&config, "telegram"));
}

#[test]
fn relay_result_maps_message_id_and_failures() {
    let result =
        relay_send_message_result(serde_json::json!({"success": true, "message_id": "m1"}))
            .expect("successful relay result");
    assert_eq!(result.message_id.as_deref(), Some("m1"));
    assert_eq!(
        result.raw,
        Some(serde_json::json!({"success": true, "message_id": "m1"}))
    );

    let err = relay_send_message_result(serde_json::json!({"success": false, "error": "denied"}))
        .unwrap_err();
    assert_eq!(err.to_string(), "relay outbound failed: denied");
}

#[tokio::test]
async fn relay_teardown_removes_only_its_own_transport() {
    use crate::relay::*;
    struct NoNetwork;
    #[async_trait::async_trait]
    impl RelayFrameIo for NoNetwork {
        async fn send(&self, _: GatewayToConnectorFrame) -> Result<(), RelayTransportError> {
            panic!("registry lifecycle must not send network traffic")
        }
        async fn recv(&self) -> Result<Option<ConnectorToGatewayFrame>, RelayTransportError> {
            panic!("registry lifecycle must not receive network traffic")
        }
    }
    let make_transport = || {
        Arc::new(RelayTransport::new(
            vec![],
            Arc::new(NoNetwork),
            RelayTransportTimeouts::default(),
        ))
    };
    let old = make_transport();
    let newer = make_transport();
    register_relay_transport(old.clone());
    register_relay_transport(newer.clone());
    unregister_relay_transport(&old);
    assert!(Arc::ptr_eq(&current_relay_transport().unwrap(), &newer));
    unregister_relay_transport(&newer);
    assert!(current_relay_transport().is_none());
    unregister_relay_transport(&newer);
    assert!(current_relay_transport().is_none());
}
