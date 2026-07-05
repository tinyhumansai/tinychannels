use super::*;
use serde_json::json;

#[test]
fn durable_final_delivery_capabilities_match_openclaw_order() {
    assert_eq!(durable_final_delivery_capabilities().len(), 13);
    assert_eq!(
        durable_final_delivery_capabilities()[0],
        DurableFinalDeliveryCapability::Text
    );
    assert_eq!(
        durable_final_delivery_capabilities()[12],
        DurableFinalDeliveryCapability::AfterCommit
    );
}

#[test]
fn message_action_names_preserve_openclaw_contract_order() {
    assert_eq!(channel_message_action_names().len(), 57);
    assert_eq!(channel_message_action_names()[0], "send");
    assert_eq!(channel_message_action_names()[20], "list-pins");
    assert_eq!(channel_message_action_names()[56], "upload-file");
    assert_eq!(
        channel_message_action_names()
            .iter()
            .filter(|name| **name == "set-profile")
            .count(),
        2
    );
}

#[test]
fn access_context_never_serializes_upstream_relay_trust_flag() {
    let access = AccessContext {
        delivered_via_upstream_relay: true,
        ..Default::default()
    };
    let value = serde_json::to_value(access).unwrap();
    assert!(value.get("deliveredViaUpstreamRelay").is_none());
    assert_eq!(value["commandAuthorized"], false);
}

#[test]
fn inbound_media_payload_preserves_attachment_indexes() {
    let payload = InboundMediaPayload::from_media(&[
        MediaReference {
            path: Some("/tmp/image.png".into()),
            content_type: Some("image/png".into()),
            kind: MediaKind::Image,
            ..Default::default()
        },
        MediaReference {
            url: Some("https://example.test/audio.mp3".into()),
            content_type: Some("audio/mpeg".into()),
            kind: MediaKind::Audio,
            transcribed: true,
            ..Default::default()
        },
    ]);

    assert_eq!(payload.media_path.as_deref(), Some("/tmp/image.png"));
    assert_eq!(
        payload.media_urls,
        Some(vec![
            "/tmp/image.png".into(),
            "https://example.test/audio.mp3".into()
        ])
    );
    assert_eq!(
        payload.media_paths,
        Some(vec!["/tmp/image.png".into(), String::new()])
    );
    assert_eq!(payload.media_transcribed_indexes, Some(vec![1]));
}

#[test]
fn receipt_normalizes_multi_part_outbound_results() {
    let receipt = create_message_receipt_from_outbound_results(
        vec![
            MessageReceiptSourceResult {
                channel: Some("telegram".into()),
                message_id: Some("m1".into()),
                ..Default::default()
            },
            MessageReceiptSourceResult {
                channel: Some("telegram".into()),
                message_id: Some("m2".into()),
                ..Default::default()
            },
        ],
        Some(MessageReceiptPartKind::Text),
        Some("topic-1".into()),
        Some("reply-1".into()),
        123,
    );

    assert_eq!(receipt.primary_platform_message_id.as_deref(), Some("m1"));
    assert_eq!(receipt.platform_message_ids, vec!["m1", "m2"]);
    assert_eq!(receipt.thread_id.as_deref(), Some("topic-1"));
    assert_eq!(receipt.reply_to_id.as_deref(), Some("reply-1"));
    assert_eq!(receipt.sent_at, 123);
    assert_eq!(receipt.parts.len(), 2);
    assert_eq!(receipt.parts[1].platform_message_id, "m2");
    assert_eq!(receipt.parts[1].kind, MessageReceiptPartKind::Text);
}

#[test]
fn receipt_uses_alternate_platform_ids_and_deduplicates() {
    let receipt = create_message_receipt_from_outbound_results(
        vec![MessageReceiptSourceResult {
            channel: Some("whatsapp".into()),
            message_id: Some(" ".into()),
            to_jid: Some("jid-1".into()),
            ..Default::default()
        }],
        None,
        None,
        None,
        123,
    );
    assert_eq!(
        resolve_message_receipt_primary_id(&receipt).as_deref(),
        Some("jid-1")
    );

    let receipt = MessageReceipt {
        primary_platform_message_id: Some(" ".into()),
        platform_message_ids: vec![" m1 ".into(), String::new(), "m1".into(), "m2".into()],
        sent_at: 123,
        ..Default::default()
    };
    assert_eq!(
        list_message_receipt_platform_ids(&receipt),
        vec!["m1", "m2"]
    );
    assert_eq!(
        resolve_message_receipt_primary_id(&receipt).as_deref(),
        Some("m1")
    );
}

#[test]
fn receipt_preserves_nested_receipts() {
    let nested = MessageReceipt {
        primary_platform_message_id: Some("platform-1".into()),
        platform_message_ids: vec!["platform-1".into(), "platform-2".into()],
        parts: vec![
            MessageReceiptPart {
                platform_message_id: "platform-1".into(),
                kind: MessageReceiptPartKind::Text,
                index: 0,
                ..Default::default()
            },
            MessageReceiptPart {
                platform_message_id: "platform-2".into(),
                kind: MessageReceiptPartKind::Media,
                index: 1,
                ..Default::default()
            },
        ],
        thread_id: Some("native-thread".into()),
        sent_at: 123,
        ..Default::default()
    };
    let receipt = create_message_receipt_from_outbound_results(
        vec![
            MessageReceiptSourceResult {
                channel: Some("telegram".into()),
                message_id: Some("top-level-ignored".into()),
                receipt: Some(nested),
                ..Default::default()
            },
            MessageReceiptSourceResult {
                channel: Some("telegram".into()),
                message_id: Some("fallback-1".into()),
                ..Default::default()
            },
        ],
        Some(MessageReceiptPartKind::Text),
        None,
        None,
        456,
    );

    assert_eq!(
        receipt.platform_message_ids,
        vec!["platform-1", "platform-2", "fallback-1"]
    );
    assert_eq!(receipt.thread_id.as_deref(), Some("native-thread"));
    assert_eq!(receipt.sent_at, 456);
}

#[test]
fn send_error_taxonomy_matches_hermes_categories() {
    assert_eq!(
        classify_send_error("Bad Request: message is too long"),
        SendErrorKind::TooLong
    );
    assert_eq!(
        classify_send_error("Bad Request: can't parse entities"),
        SendErrorKind::BadFormat
    );
    assert_eq!(
        classify_send_error("Forbidden: bot was blocked by the user"),
        SendErrorKind::Forbidden
    );
    assert_eq!(
        classify_send_error("Too Many Requests: retry after 30"),
        SendErrorKind::RateLimited
    );
    assert_eq!(
        classify_send_error("connection reset by peer"),
        SendErrorKind::Transient
    );
    assert!(is_chat_level_not_found("chat not found"));
    assert!(!is_chat_level_not_found("thread not found"));
}

#[test]
fn timeouts_are_not_retryable_without_reconciliation() {
    let error = ChannelSendError::new("ConnectTimeout while sending");
    assert_eq!(error.kind, SendErrorKind::Transient);
    assert!(!error.retryable);
}

#[test]
fn session_keys_include_scope_topic_and_default_thread_sharing() {
    let channel = ChannelRef {
        id: "telegram".into(),
        account_id: Some("bot-a".into()),
    };
    let conversation = ConversationRef {
        kind: ConversationKind::Group,
        id: "-100123".into(),
        scope_id: Some("tenant-a".into()),
        topic_id: Some("topic-99".into()),
        ..Default::default()
    };
    let sender = SenderRef {
        id: "alice".into(),
        ..Default::default()
    };

    let key = build_session_key(
        "main",
        &channel,
        &conversation,
        &sender,
        SessionKeyPolicy::default(),
    );
    assert_eq!(key, "main:telegram:bot-a:group:tenant-a:-100123:topic-99");

    let isolated = build_session_key(
        "main",
        &channel,
        &conversation,
        &sender,
        SessionKeyPolicy {
            thread_sessions_per_user: true,
            ..Default::default()
        },
    );
    assert_eq!(
        isolated,
        "main:telegram:bot-a:group:tenant-a:-100123:topic-99:alice"
    );
}

#[test]
fn legacy_session_key_candidates_match_openhuman_helpers() {
    let msg = ChannelMessage {
        id: "msg-1".into(),
        channel: "telegram".into(),
        sender: "alice".into(),
        content: "hello".into(),
        reply_target: "-100123".into(),
        timestamp: 123,
        thread_ts: Some("topic-99".into()),
    };
    let keys = conversation_history_key_candidates(&msg);
    assert_eq!(keys.conversation_history_key, "telegram_alice_-100123");
    assert_eq!(keys.conversation_memory_key, "telegram_alice_msg-1");
}

#[test]
fn outbound_intent_carries_idempotency_key() {
    let intent = ChannelOutboundIntent {
        idempotency_key: "idem-1".into(),
        channel_id: "telegram".into(),
        conversation_id: "-100123".into(),
        reply_to_id: None,
        thread_id: Some("topic-99".into()),
        durability: DeliveryDurability::Required,
        payload: OutboundPayload::NativeChannelData {
            data: json!({"x": 1}),
        },
    };
    assert_eq!(intent.idempotency_key, "idem-1");
}

#[test]
fn legacy_message_intent_derives_stable_idempotency_key() {
    let left = outbound_intent_from_legacy_message(
        "telegram",
        json!({
            "text": "hello",
            "threadId": "topic-1",
            "replyToMessageId": "msg-1",
            "buttons": [{"text": "Approve", "value": "yes"}],
        }),
    );
    let right = outbound_intent_from_legacy_message(
        "telegram",
        json!({
            "replyToMessageId": "msg-1",
            "buttons": [{"value": "yes", "text": "Approve"}],
            "threadId": "topic-1",
            "text": "hello",
        }),
    );

    assert_eq!(left.idempotency_key, right.idempotency_key);
    assert!(left.idempotency_key.starts_with("legacy-send:telegram:"));
    assert_eq!(left.channel_id, "telegram");
    assert_eq!(left.conversation_id, "telegram");
    assert_eq!(left.reply_to_id.as_deref(), Some("msg-1"));
    assert_eq!(left.thread_id.as_deref(), Some("topic-1"));
}

#[test]
fn legacy_message_intent_preserves_explicit_idempotency_key() {
    let intent = outbound_intent_from_legacy_message(
        "discord",
        json!({
            "idempotencyKey": "caller-key",
            "recipient": "channel-1",
            "text": "hello",
        }),
    );

    assert_eq!(intent.idempotency_key, "caller-key");
    assert_eq!(intent.conversation_id, "channel-1");
}

#[test]
fn legacy_message_payload_adds_idempotency_without_dropping_rich_fields() {
    let intent = outbound_intent_from_legacy_message(
        "telegram",
        json!({
            "text": "hello",
            "photoUrl": "https://example.test/a.png",
        }),
    );

    let payload = legacy_message_value_from_outbound_intent(&intent);
    assert_eq!(payload["text"], "hello");
    assert_eq!(payload["photoUrl"], "https://example.test/a.png");
    assert_eq!(payload["idempotencyKey"], intent.idempotency_key);
}

#[test]
fn send_message_intent_preserves_legacy_typed_fields() {
    let message = SendMessage::with_subject("hello", "alice", "subject")
        .in_thread(Some("thread-1".to_string()));
    let intent = outbound_intent_from_send_message("discord", &message);
    let payload = legacy_message_value_from_outbound_intent(&intent);

    assert_eq!(intent.channel_id, "discord");
    assert_eq!(intent.conversation_id, "alice");
    assert!(intent.idempotency_key.starts_with("legacy-send:discord:"));
    assert_eq!(payload["content"], "hello");
    assert_eq!(payload["recipient"], "alice");
    assert_eq!(payload["subject"], "subject");
    assert_eq!(payload["thread_ts"], "thread-1");
    assert_eq!(payload["idempotencyKey"], intent.idempotency_key);
}

#[test]
fn send_message_intent_preserves_explicit_idempotency_key() {
    let message = SendMessage::new("hello", "alice").with_idempotency_key("typed-key");
    let intent = outbound_intent_from_send_message("discord", &message);
    let payload = legacy_message_value_from_outbound_intent(&intent);

    assert_eq!(intent.idempotency_key, "typed-key");
    assert_eq!(payload["idempotencyKey"], "typed-key");
}
