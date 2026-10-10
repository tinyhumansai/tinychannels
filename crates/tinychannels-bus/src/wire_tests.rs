use super::*;
use serde_json::json;

#[test]
fn relay_passthrough_body_preserves_base64_wire_and_legacy_invalid_input() {
    let wire = json!({"platform":"test", "botId":"bot", "method":"POST", "path":"/fixture", "headers":[], "bodyB64":"AP8q"});
    let decoded: relay::PassthroughForward = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(decoded.body, [0, 255, 42]);
    assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
    let bad: relay::PassthroughForward =
        serde_json::from_value(json!({"bodyB64":"!invalid!"})).unwrap();
    assert!(bad.body.is_empty());
}

#[test]
fn legacy_message_and_normalized_envelope_keep_optional_field_and_trust_wire() {
    let wire = json!({"id":"m1", "sender":"alice", "reply_target":"room", "content":"hi", "channel":"telegram", "timestamp":7, "thread_ts":"topic"});
    let message: ChannelMessage = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), wire);
    let mut envelope = ChannelInboundEnvelope::default();
    envelope.access.delivered_via_upstream_relay = true;
    let encoded = serde_json::to_value(envelope).unwrap();
    assert_eq!(
        encoded["access"],
        json!({"dmDecision":"deny", "groupPolicy":"disabled", "mentionGate":"none", "commandAuthorized":false})
    );
    assert_eq!(encoded["media"], json!([]));
}

#[test]
fn send_message_legacy_wire_remains_snake_case() {
    let message = SendMessage::with_subject("hi", "room", "subject")
        .in_thread(Some("topic".into()))
        .with_idempotency_key("caller-key");
    assert_eq!(
        serde_json::to_value(message).unwrap(),
        json!({"content":"hi", "recipient":"room", "subject":"subject", "thread_ts":"topic", "idempotency_key":"caller-key"})
    );
}

#[test]
fn defaults_keep_safe_delivery_and_disconnection_wire_values() {
    let intent = ChannelOutboundIntent::default();
    assert_eq!(intent.durability, DeliveryDurability::BestEffort);
    assert_eq!(
        intent.payload,
        OutboundPayload::Text {
            text: String::new()
        }
    );
    let disconnected = controllers::ChannelDisconnectResult::default();
    let wire = serde_json::to_value(disconnected).unwrap();
    assert_eq!(wire["disconnected"], false);
    assert_eq!(wire["restartRequired"], false);
    for (kind, spelling) in [
        (channel::ConversationKind::Dm, "dm"),
        (channel::ConversationKind::Group, "group"),
        (channel::ConversationKind::Channel, "channel"),
        (channel::ConversationKind::Thread, "thread"),
        (channel::ConversationKind::Unknown, "unknown"),
    ] {
        assert_eq!(kind.as_session_segment(), spelling);
        assert_eq!(serde_json::to_value(kind).unwrap(), spelling);
    }
    assert!(is_compatible(CONTRACT_VERSION));
    assert!(!is_compatible(CONTRACT_VERSION + 1));
}

#[test]
fn structured_errors_preserve_kind_message_and_details() {
    let details = channel::ChannelSendError {
        message: "fixture refusal".into(),
        kind: channel::SendErrorKind::Forbidden,
        ..Default::default()
    };
    let error = TinyChannelsError::from(details.clone());
    assert_eq!(error.message(), "fixture refusal");
    match error {
        TinyChannelsError::Send {
            kind,
            details: actual,
            ..
        } => {
            assert_eq!(kind, channel::SendErrorKind::Forbidden);
            assert_eq!(*actual, details);
        }
        other => panic!("unexpected error: {other}"),
    }
    for error in [
        TinyChannelsError::new("fixture"),
        TinyChannelsError::Config("fixture".into()),
        TinyChannelsError::Serialization("fixture".into()),
    ] {
        assert_eq!(error.message(), "fixture");
        assert!(error.to_string().contains("fixture"));
    }
}

#[test]
fn relay_descriptor_keeps_explicit_platform_limit() {
    let descriptor = relay::CapabilityDescriptor::from_platform_entry(
        &relay::RelayPlatformEntry {
            name: "fixture".into(),
            label: "Fixture".into(),
            max_message_length: 128,
            emoji: None,
            platform_hint: None,
            pii_safe: true,
        },
        relay::RelayDescriptorOptions::default(),
    );
    assert_eq!(descriptor.max_message_length, 128);
    assert!(descriptor.supports_edit);
    assert!(descriptor.pii_safe);
    assert_eq!(
        relay::CapabilityDescriptor::from_json(&descriptor.to_json().unwrap()).unwrap(),
        descriptor
    );
}
