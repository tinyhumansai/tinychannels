use super::*;

#[test]
fn inbound_vocabulary_preserves_optional_sender_and_existing_message_wire_shape() {
    let wire = serde_json::json!({"channel":"telegram","sender":null,"reply_target":null,"thread_ts":null});
    let request: PrepareChannelIdentityRequest = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
    let request = PrepareChannelInboundRequest {
        source: ChannelInboundSource::Legacy {
            message: Box::new(ChannelMessage::default()),
            envelope: None,
        },
        namespace: "tenant".into(),
        session_policy: Default::default(),
        preparation_mode: ChannelInboundPreparationMode::LegacyText,
    };
    let wire = serde_json::to_value(&request).unwrap();
    assert_eq!(wire["source"]["kind"], "legacy");
    assert_eq!(wire["source"]["envelope"], serde_json::Value::Null);
    assert!(wire.get("preparation_mode").is_none());
    assert!(wire["source"]["message"].get("sender_name").is_none());
    let restored: PrepareChannelInboundRequest = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), wire);

    let mut legacy_wire = wire.clone();
    legacy_wire
        .as_object_mut()
        .unwrap()
        .remove("preparation_mode");
    let restored: PrepareChannelInboundRequest = serde_json::from_value(legacy_wire).unwrap();
    assert_eq!(
        restored.preparation_mode,
        ChannelInboundPreparationMode::LegacyText
    );

    let typed = PrepareChannelInboundRequest {
        preparation_mode: ChannelInboundPreparationMode::Typed,
        ..request
    };
    assert_eq!(
        serde_json::to_value(typed).unwrap()["preparation_mode"],
        "typed"
    );
}

#[test]
fn prepared_inbound_exposes_thread_key_policy_without_leaking_key_format() {
    let prepared = PreparedChannelInbound {
        message: ChannelMessage::default(),
        envelope: crate::ChannelInboundEnvelope::default(),
        history_key: "opaque-key".into(),
        history_key_ignores_thread: true,
        thread_id: "thread".into(),
        client_id: "client".into(),
        session_key: "session".into(),
    };
    let value = serde_json::to_value(&prepared).unwrap();
    assert_eq!(value["history_key_ignores_thread"], true);
    let restored: PreparedChannelInbound = serde_json::from_value(value).unwrap();
    assert!(restored.history_key_ignores_thread);
}
