use super::*;

#[test]
fn runtime_reexports_keep_exact_bus_dto_identity() {
    let message: tinychannels_bus::SendMessage = traits::SendMessage::new("hi", "room");
    let message: tinychannels_bus::traits::SendMessage =
        message.with_deterministic_idempotency_key("telegram");
    assert!(message.idempotency_key.is_some());
    let envelope: tinychannels_bus::ChannelInboundEnvelope =
        channel::ChannelInboundEnvelope::default();
    let projected: tinychannels_bus::ChannelMessage =
        channel::legacy_message_from_inbound_envelope(&envelope, 7);
    assert_eq!(projected.timestamp, 7);
}

#[test]
fn persisted_legacy_idempotency_digest_remains_byte_identical() {
    let intent = channel::outbound_intent_from_legacy_message(
        "telegram",
        serde_json::json!({"text":"hi", "recipient":"room"}),
    );
    assert_eq!(
        intent.idempotency_key,
        "legacy-send:telegram:348ff41bc06b76435e69051366e46001710bb3590c128b6e88c97e42d5dfb30e"
    );
}
