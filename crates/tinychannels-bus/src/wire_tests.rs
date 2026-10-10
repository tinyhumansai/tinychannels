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
