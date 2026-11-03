// Scoped allowance for the one-release v21 wire DTO compatibility surface.
#![allow(deprecated)]

use super::*;
use serde_json::json;
use tinychannels_bus::{ContentPart, OutboundContentPart};

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
    assert_eq!(channel_message_action_names().len(), 56);
    assert_eq!(channel_message_action_names()[0], "send");
    assert_eq!(channel_message_action_names()[20], "list-pins");
    assert_eq!(channel_message_action_names()[55], "upload-file");
    assert_eq!(
        channel_message_action_names()
            .iter()
            .filter(|name| **name == "set-profile")
            .count(),
        1
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
    assert_eq!(
        payload.media_types,
        Some(vec!["image/png".into(), "audio/mpeg".into()])
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
fn receipt_platform_ids_include_part_ids_without_an_aggregate_list() {
    let receipt = MessageReceipt {
        parts: vec![MessageReceiptPart {
            platform_message_id: "part-1".into(),
            kind: MessageReceiptPartKind::Text,
            index: 0,
            ..Default::default()
        }],
        ..Default::default()
    };

    assert_eq!(list_message_receipt_platform_ids(&receipt), vec!["part-1"]);
    assert_eq!(
        resolve_message_receipt_primary_id(&receipt).as_deref(),
        Some("part-1")
    );
}

#[test]
fn receipt_uses_provider_message_id_as_scoped_telegram_operation_tokens() {
    let receipt = create_message_receipt_from_outbound_results(
        vec![MessageReceiptSourceResult {
            channel: Some("telegram".into()),
            message_id: Some("42".into()),
            ..Default::default()
        }],
        Some(MessageReceiptPartKind::Text),
        None,
        None,
        123,
    );
    assert_eq!(receipt.parts[0].edit_token.as_deref(), Some("42"));
    assert_eq!(receipt.parts[0].delete_token.as_deref(), Some("42"));
    assert_eq!(receipt.edit_token.as_deref(), Some("42"));
    assert_eq!(receipt.delete_token.as_deref(), Some("42"));
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
    assert_eq!(
        key,
        "v22|namespace=p4:main|channel=p8:telegram|account=p5:bot-a|kind=p5:group|scope=p8:tenant-a|conversation=p7:-100123|parent=a|topic=p8:topic-99|thread=a|participant=a"
    );

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
        "v22|namespace=p4:main|channel=p8:telegram|account=p5:bot-a|kind=p5:group|scope=p8:tenant-a|conversation=p7:-100123|parent=a|topic=p8:topic-99|thread=a|participant=p5:alice"
    );
}

#[test]
fn session_keys_do_not_collide_when_components_contain_separators() {
    let left = build_session_key(
        "main",
        &ChannelRef {
            id: "a:b".into(),
            account_id: Some("c".into()),
        },
        &ConversationRef {
            kind: ConversationKind::Group,
            id: "room".into(),
            ..Default::default()
        },
        &SenderRef::default(),
        SessionKeyPolicy::default(),
    );
    let right = build_session_key(
        "main",
        &ChannelRef {
            id: "a".into(),
            account_id: Some("b:c".into()),
        },
        &ConversationRef {
            kind: ConversationKind::Group,
            id: "room".into(),
            ..Default::default()
        },
        &SenderRef::default(),
        SessionKeyPolicy::default(),
    );

    assert_ne!(left, right);
}

#[test]
fn empty_topic_and_nonempty_thread_are_both_preserved() {
    let key = build_session_key(
        "main",
        &ChannelRef {
            id: "discord".into(),
            ..Default::default()
        },
        &ConversationRef {
            kind: ConversationKind::Group,
            id: "room".into(),
            topic_id: Some(String::new()),
            thread_id: Some("thread-1".into()),
            ..Default::default()
        },
        &SenderRef::default(),
        SessionKeyPolicy::default(),
    );

    assert_eq!(
        key,
        "v22|namespace=p4:main|channel=p7:discord|account=a|kind=p5:group|scope=a|conversation=p4:room|parent=a|topic=p0:|thread=p8:thread-1|participant=a"
    );
}

#[test]
fn session_keys_build_from_inbound_envelopes() {
    let envelope = ChannelInboundEnvelope {
        channel: ChannelRef {
            id: "telegram".into(),
            account_id: Some("bot-a".into()),
        },
        conversation: ConversationRef {
            kind: ConversationKind::Group,
            id: "-100123".into(),
            scope_id: Some("tenant-a".into()),
            topic_id: Some("topic-99".into()),
            ..Default::default()
        },
        sender: SenderRef {
            id: "alice".into(),
            ..Default::default()
        },
        ..Default::default()
    };

    let shared =
        build_session_key_for_inbound_envelope("default", &envelope, SessionKeyPolicy::default());
    assert_eq!(
        shared,
        "v22|namespace=p4:main|channel=p8:telegram|account=p5:bot-a|kind=p5:group|scope=p8:tenant-a|conversation=p7:-100123|parent=a|topic=p8:topic-99|thread=a|participant=a"
    );

    let isolated = build_session_key_for_inbound_envelope(
        "default",
        &envelope,
        SessionKeyPolicy {
            thread_sessions_per_user: true,
            ..Default::default()
        },
    );
    assert_eq!(
        isolated,
        "v22|namespace=p4:main|channel=p8:telegram|account=p5:bot-a|kind=p5:group|scope=p8:tenant-a|conversation=p7:-100123|parent=a|topic=p8:topic-99|thread=a|participant=p5:alice"
    );
}

#[test]
fn legacy_inbound_envelope_preserves_telegram_topics_separately_from_threads() {
    let msg = ChannelMessage {
        id: "msg-1".into(),
        channel: "telegram".into(),
        sender: "alice".into(),
        content: "hello".into(),
        reply_target: "-100123".into(),
        timestamp: 123,
        topic_id: None,
        reply_to_id: None,
        thread_ts: Some(" topic-99 ".into()),
        sender_name: None,
    };

    let envelope = inbound_envelope_from_legacy_message(&msg);

    assert_eq!(envelope.channel.id, "telegram");
    assert_eq!(envelope.message_id, "msg-1");
    assert_eq!(envelope.conversation.id, "-100123");
    assert_eq!(envelope.conversation.thread_id, None);
    assert_eq!(envelope.conversation.topic_id.as_deref(), Some("topic-99"));
    assert_eq!(envelope.sender.id, "alice");
    assert_eq!(envelope.text, "hello");
    assert_eq!(
        envelope.parts,
        vec![ContentPart::Text {
            text: "hello".into(),
            markdown_dialect: None,
        }]
    );

    let keys = conversation_history_key_candidates(&msg);
    assert_eq!(keys.conversation_history_key, "telegram_alice_-100123");
}

#[test]
fn legacy_inbound_timestamp_seconds_are_normalized_to_unix_milliseconds() {
    let seconds = 1_700_000_000;
    let millis = 1_700_000_000_123;
    let mut msg = ChannelMessage {
        timestamp: seconds,
        ..Default::default()
    };
    assert_eq!(
        inbound_envelope_from_legacy_message(&msg).timestamp_unix_ms,
        Some(millis - 123)
    );

    msg.timestamp = millis;
    assert_eq!(
        inbound_envelope_from_legacy_message(&msg).timestamp_unix_ms,
        Some(millis)
    );
}

#[test]
fn legacy_inbound_envelope_recognizes_qualified_telegram_channel_ids() {
    for channel in ["tg:123", "telegram:123"] {
        let msg = ChannelMessage {
            channel: channel.into(),
            topic_id: None,
            reply_to_id: None,
            thread_ts: Some("topic-99".into()),
            ..Default::default()
        };
        let envelope = inbound_envelope_from_legacy_message(&msg);
        assert_eq!(envelope.conversation.topic_id.as_deref(), Some("topic-99"));
        assert_eq!(envelope.conversation.thread_id, None);
    }
}

#[test]
fn legacy_inbound_envelope_projects_back_to_legacy_messages() {
    let msg = ChannelMessage {
        id: "msg-1".into(),
        channel: "telegram".into(),
        sender: "alice".into(),
        content: "hello".into(),
        reply_target: "-100123".into(),
        timestamp: 123,
        topic_id: None,
        reply_to_id: None,
        thread_ts: Some("topic-99".into()),
        sender_name: None,
    };
    let envelope = inbound_envelope_from_legacy_message(&msg);

    let projected = legacy_message_from_inbound_envelope(&envelope, 456).unwrap();

    assert_eq!(projected.id, "msg-1");
    assert_eq!(projected.channel, "telegram");
    assert_eq!(projected.sender, "alice");
    assert_eq!(projected.content, "hello");
    assert_eq!(projected.reply_target, "-100123");
    assert_eq!(projected.thread_ts, None);
    assert_eq!(projected.topic_id.as_deref(), Some("topic-99"));
    assert_eq!(projected.reply_to_id, msg.reply_to_id);
    assert_eq!(projected.timestamp, 456);
}

#[test]
fn inbound_envelope_roundtrip_keeps_reply_thread_and_topic_distinct() {
    let msg = ChannelMessage {
        channel: "slack:account-1".into(),
        id: "source".into(),
        sender: "alice".into(),
        reply_target: "room".into(),
        content: "reply".into(),
        timestamp: 123,
        reply_to_id: Some("parent-message".into()),
        thread_ts: Some("thread-a".into()),
        topic_id: Some("topic-b".into()),
        ..Default::default()
    };
    let envelope = inbound_envelope_from_legacy_message(&msg);
    assert_eq!(envelope.reply_to_id.as_deref(), Some("parent-message"));
    assert_eq!(envelope.conversation.thread_id.as_deref(), Some("thread-a"));
    assert_eq!(envelope.conversation.topic_id.as_deref(), Some("topic-b"));
    let projected = legacy_message_from_inbound_envelope(&envelope, 456).unwrap();
    assert_eq!(projected.reply_to_id, msg.reply_to_id);
    assert_eq!(projected.thread_ts, msg.thread_ts);
    assert_eq!(projected.topic_id, msg.topic_id);
}

#[test]
fn typed_text_parts_are_authoritative_over_stale_compatibility_text() {
    let envelope = ChannelInboundEnvelope {
        text: "stale projection".into(),
        parts: vec![
            ContentPart::Text {
                text: "first".into(),
                markdown_dialect: None,
            },
            ContentPart::Text {
                text: "second".into(),
                markdown_dialect: None,
            },
        ],
        ..Default::default()
    };

    let projected = legacy_message_from_inbound_envelope(&envelope, 1).unwrap();

    assert_eq!(projected.content, "first\nsecond");
}

#[test]
fn legacy_inbound_projection_rejects_rich_parts_instead_of_dropping_them() {
    let envelope = ChannelInboundEnvelope {
        text: "compat text".into(),
        parts: vec![ContentPart::Image {
            media: MediaReference {
                url: Some("https://example.test/image.png".into()),
                kind: MediaKind::Image,
                ..Default::default()
            },
            caption: None,
            alt_text: None,
        }],
        ..Default::default()
    };

    assert!(legacy_message_from_inbound_envelope(&envelope, 1).is_err());
}

#[test]
fn missing_inbound_parts_fall_back_to_the_legacy_text_field() {
    let envelope = ChannelInboundEnvelope {
        text: "legacy text".into(),
        ..Default::default()
    };

    let projected = legacy_message_from_inbound_envelope(&envelope, 1).unwrap();

    assert_eq!(projected.content, "legacy text");
}

#[test]
fn telegram_legacy_projection_keeps_simultaneous_thread_and_topic_ids() {
    let envelope = ChannelInboundEnvelope {
        channel: ChannelRef {
            id: "telegram:account".into(),
            account_id: Some("account".into()),
        },
        message_id: "message".into(),
        reply_to_id: Some("reply-parent".into()),
        conversation: ConversationRef {
            id: "chat".into(),
            thread_id: Some("thread-opaque".into()),
            topic_id: Some("topic-opaque".into()),
            ..Default::default()
        },
        ..Default::default()
    };

    let legacy = legacy_message_from_inbound_envelope(&envelope, 1).unwrap();
    let roundtrip = inbound_envelope_from_legacy_message(&legacy);

    assert_eq!(roundtrip.reply_to_id.as_deref(), Some("reply-parent"));
    assert_eq!(
        roundtrip.conversation.thread_id.as_deref(),
        Some("thread-opaque")
    );
    assert_eq!(
        roundtrip.conversation.topic_id.as_deref(),
        Some("topic-opaque")
    );
}

#[test]
fn the_sender_name_survives_the_envelope_both_ways() {
    let msg = ChannelMessage {
        id: "msg-1".into(),
        channel: "whatsapp".into(),
        sender: "+15550001111".into(),
        sender_name: Some("Alice".into()),
        ..Default::default()
    };
    let envelope = inbound_envelope_from_legacy_message(&msg);
    assert_eq!(envelope.sender.name.as_deref(), Some("Alice"));

    let projected = legacy_message_from_inbound_envelope(&envelope, 1).unwrap();
    assert_eq!(projected.sender_name.as_deref(), Some("Alice"));
}

#[test]
fn the_sender_name_is_optional_on_the_wire() {
    // A message from a build before the field decodes, with no name.
    let old = json!({
        "id": "m", "sender": "+1555", "reply_target": "+1555", "content": "hi",
        "channel": "whatsapp", "timestamp": 1, "thread_ts": null
    });
    let decoded: ChannelMessage = serde_json::from_value(old).unwrap();
    assert_eq!(decoded.sender_name, None);

    // No name is not written, so an older decoder sees the same shape.
    let unnamed = serde_json::to_value(&decoded).unwrap();
    assert!(unnamed.get("sender_name").is_none());
    let named = ChannelMessage {
        sender_name: Some("Alice".into()),
        ..decoded
    };
    assert_eq!(
        serde_json::to_value(&named).unwrap()["sender_name"],
        "Alice"
    );
}

#[test]
fn legacy_inbound_envelope_preserves_non_telegram_threads() {
    let msg = ChannelMessage {
        id: "msg-1".into(),
        channel: "discord".into(),
        sender: "alice".into(),
        content: "hello".into(),
        reply_target: "channel-123".into(),
        timestamp: 123,
        topic_id: None,
        reply_to_id: None,
        thread_ts: Some(" thread-99 ".into()),
        sender_name: None,
    };

    let envelope = inbound_envelope_from_legacy_message(&msg);

    assert_eq!(envelope.channel.id, "discord");
    assert_eq!(envelope.conversation.id, "channel-123");
    assert_eq!(
        envelope.conversation.thread_id.as_deref(),
        Some("thread-99")
    );
    assert_eq!(envelope.conversation.topic_id, None);

    let keys = conversation_history_key_candidates(&msg);
    assert_eq!(
        keys.conversation_history_key,
        "discord_alice_channel-123_thread:thread-99"
    );
}

#[test]
fn legacy_inbound_envelope_discards_whitespace_only_thread_ids() {
    let msg = ChannelMessage {
        channel: "discord".into(),
        topic_id: None,
        reply_to_id: None,
        thread_ts: Some(" \t ".into()),
        ..Default::default()
    };

    let envelope = inbound_envelope_from_legacy_message(&msg);

    assert_eq!(envelope.conversation.thread_id, None);
    assert_eq!(envelope.conversation.topic_id, None);
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
        topic_id: None,
        reply_to_id: None,
        thread_ts: Some("topic-99".into()),
        sender_name: None,
    };
    let keys = conversation_history_key_candidates(&msg);
    assert_eq!(keys.conversation_history_key, "telegram_alice_-100123");
    assert_eq!(keys.conversation_memory_key, "telegram_alice_msg-1");
}

#[test]
fn legacy_session_key_candidates_preserve_unescaped_stored_keys() {
    let msg = ChannelMessage {
        id: "message_part".into(),
        channel: "a_b".into(),
        sender: "c".into(),
        reply_target: "room_part".into(),
        topic_id: None,
        reply_to_id: None,
        thread_ts: Some("thread_part".into()),
        ..Default::default()
    };

    let keys = conversation_history_key_candidates(&msg);

    // These are migration lookup keys written by the former OpenHuman
    // helpers, before current keys began escaping identity separators.
    assert_eq!(
        keys.conversation_history_key,
        "a_b_c_room_part_thread:thread_part"
    );
    assert_eq!(keys.conversation_memory_key, "a_b_c_message_part");
}

#[test]
fn outbound_intent_carries_idempotency_key() {
    let intent = ChannelOutboundIntent {
        idempotency_key: "idem-1".into(),
        channel_id: "telegram".into(),
        conversation_id: "-100123".into(),
        channel_ref: None,
        conversation_ref: None,
        reply_to_id: None,
        thread_id: Some("topic-99".into()),
        turn_id: None,
        durability: DeliveryDurability::Required,
        payload: OutboundPayload::NativeChannelData {
            data: json!({"x": 1}),
        },
    };
    assert_eq!(intent.idempotency_key, "idem-1");
}

#[test]
fn legacy_outbound_adapter_preserves_originating_turn_identity() {
    let intent = outbound_intent_from_legacy_message(
        "telegram",
        json!({"text": "hello", "turnId": "turn-42"}),
    );
    assert_eq!(intent.turn_id.as_deref(), Some("turn-42"));
    assert_eq!(
        legacy_message_value_from_outbound_intent(&intent)["turnId"],
        "turn-42"
    );
    let typed = tinychannels_bus::ChannelOutboundMessage::from_intent(&intent);
    assert_eq!(typed.turn_id.as_deref(), Some("turn-42"));
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
fn legacy_message_payload_replaces_a_blank_idempotency_key() {
    let intent = outbound_intent_from_legacy_message(
        "telegram",
        json!({"text": "hello", "idempotencyKey": "  "}),
    );
    let payload = legacy_message_value_from_outbound_intent(&intent);

    assert!(!intent.idempotency_key.is_empty());
    assert_eq!(payload["idempotencyKey"], intent.idempotency_key);
}

#[test]
fn send_message_intent_preserves_legacy_typed_fields() {
    let mut message = SendMessage::with_subject("hello", "alice", "subject")
        .in_thread(Some("thread-1".to_string()));
    message.topic_id = Some("topic-2".into());
    message.reply_to_id = Some("message-3".into());
    let intent = outbound_intent_from_send_message("discord", &message);
    let payload = legacy_message_value_from_outbound_intent(&intent);

    assert_eq!(intent.channel_id, "discord");
    assert_eq!(intent.conversation_id, "alice");
    assert!(intent.idempotency_key.starts_with("legacy-send:discord:"));
    assert_eq!(payload["content"], "hello");
    assert_eq!(payload["recipient"], "alice");
    assert_eq!(payload["subject"], "subject");
    assert_eq!(payload["thread_ts"], "thread-1");
    assert_eq!(intent.reply_to_id.as_deref(), Some("message-3"));
    assert_eq!(
        intent
            .conversation_ref
            .as_ref()
            .unwrap()
            .topic_id
            .as_deref(),
        Some("topic-2")
    );
    assert_eq!(payload["topicId"], "topic-2");
    assert_eq!(payload["replyToId"], "message-3");
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

#[test]
fn socket_inbound_client_id_keys_per_sender() {
    // Distinct senders in the same shared channel must produce distinct
    // client_id labels so downstream consumers that key on client_id
    // (audit log, future session caches) stay segregated. The
    // thread_id is already per-sender; this is the matching client_id
    // half of the pair.
    let alice = derive_inbound_client_id("discord", Some("alice"));
    let bob = derive_inbound_client_id("discord", Some("bob"));
    assert_ne!(alice, bob, "co-channel senders must not collapse");
    assert!(alice.starts_with("inbound"));
    assert!(bob.starts_with("inbound"));
}

#[test]
fn socket_inbound_client_id_legacy_fallback_keeps_bare_inbound() {
    // Legacy publishers that don't fill `sender` keep the historical
    // `"inbound"` literal so single-DM flows (where there's no
    // co-channel surface) are unchanged.
    assert_eq!(derive_inbound_client_id("discord", None), "inbound");
    assert_eq!(derive_inbound_client_id("discord", Some("")), "inbound");
    assert_eq!(derive_inbound_client_id("discord", Some("   ")), "inbound");
}

#[test]
fn socket_inbound_keys_per_sender_combined_with_thread_id() {
    // Regression: in a shared Discord channel, two distinct senders
    // sending into the same channel/reply_target produce a fully
    // distinct (client_id, thread_id) pair. This is the surface the
    // wallet preparer-binding and parked-approval routing both rely
    // on for per-user isolation.
    let alice_thread = derive_inbound_thread_id("discord", Some("alice"), Some("#general"), None);
    let bob_thread = derive_inbound_thread_id("discord", Some("bob"), Some("#general"), None);
    let alice_client = derive_inbound_client_id("discord", Some("alice"));
    let bob_client = derive_inbound_client_id("discord", Some("bob"));

    assert_ne!(alice_thread, bob_thread);
    assert_ne!(alice_client, bob_client);
    assert_ne!(
        (alice_client.as_str(), alice_thread.as_str()),
        (bob_client.as_str(), bob_thread.as_str()),
    );
}

#[test]
fn legacy_channel_only_keeps_old_shape() {
    // Publishers that don't pass sender must still produce a stable
    // key so existing single-DM flows are unchanged.
    assert_eq!(
        derive_inbound_thread_id("telegram", None, None, None),
        "channel:telegram"
    );
}

#[test]
fn distinct_senders_get_distinct_keys() {
    let a = derive_inbound_thread_id("discord", Some("alice"), Some("#general"), None);
    let b = derive_inbound_thread_id("discord", Some("bob"), Some("#general"), None);
    assert_ne!(a, b, "two senders in same channel must not collapse");
}

#[test]
fn slack_thread_anchor_splits_subthreads() {
    let parent = derive_inbound_thread_id("slack", Some("u1"), Some("C1"), None);
    let thread = derive_inbound_thread_id("slack", Some("u1"), Some("C1"), Some("1700.001"));
    assert_ne!(parent, thread);
}

#[test]
fn telegram_ignores_thread_ts() {
    // Telegram uses thread_ts for transport routing only; memory key
    // must stay stable across thread_ts updates inside the same DM.
    let a = derive_inbound_thread_id("telegram", Some("u1"), Some("c1"), Some("100"));
    let b = derive_inbound_thread_id("telegram", Some("u1"), Some("c1"), Some("200"));
    assert_eq!(a, b);
}

#[test]
fn telegram_chat_id_shape_still_ignores_thread_ts() {
    // Regression: in production the socket layer addresses Telegram
    // with raw chat ids like `tg:123` and `telegram:123` (matching
    // the `<provider>:message` event name shape). The thread_ts
    // carve-out must recognise both, not only the literal slug.
    for channel in ["tg:123", "telegram:123", "tg", "telegram"] {
        let a = derive_inbound_thread_id(channel, Some("u1"), Some("c1"), Some("100"));
        let b = derive_inbound_thread_id(channel, Some("u1"), Some("c1"), Some("200"));
        assert_eq!(
            a, b,
            "channel '{channel}' should ignore thread_ts (telegram provider)"
        );
    }
}

#[test]
fn non_telegram_channel_id_shape_still_splits_on_thread_ts() {
    // Inverse: a `slack:<workspace>` style channel must continue to
    // honour thread_ts so Slack subthreads stay distinct.
    let a = derive_inbound_thread_id("slack:T1", Some("u1"), Some("c1"), Some("100"));
    let b = derive_inbound_thread_id("slack:T1", Some("u1"), Some("c1"), Some("200"));
    assert_ne!(a, b);
}

#[test]
fn empty_optional_fields_are_skipped() {
    let only_sender = derive_inbound_thread_id("discord", Some("alice"), Some("   "), None);
    assert_eq!(only_sender, "channel:discord/alice");
}

#[test]
fn inbound_thread_and_client_ids_do_not_collide_on_separators() {
    let left_thread = derive_inbound_thread_id("a/b", Some("c"), None, None);
    let right_thread = derive_inbound_thread_id("a", Some("b/c"), None, None);
    assert_ne!(left_thread, right_thread);

    let left_client = derive_inbound_client_id("a:b", Some("c"));
    let right_client = derive_inbound_client_id("a", Some("b:c"));
    assert_ne!(left_client, right_client);
}

#[test]
fn outbound_payload_variants_keep_legacy_and_relay_content() {
    let cases = [
        (
            OutboundPayload::Text {
                text: "hello".into(),
            },
            json!({"text":"hello"}),
            "hello",
        ),
        (
            OutboundPayload::Parts {
                parts: vec![OutboundContentPart::Content {
                    part: ContentPart::Text {
                        text: "typed parts".into(),
                        markdown_dialect: None,
                    },
                }],
            },
            json!({"parts":[{"kind":"content","part":{"kind":"text","text":"typed parts"}}]}),
            "",
        ),
        (
            OutboundPayload::Media {
                text: Some("caption".into()),
                media_urls: vec!["image".into()],
            },
            json!({"text":"caption","mediaUrls":["image"]}),
            "caption",
        ),
        (
            OutboundPayload::Voice {
                voice: tinychannels_bus::OutboundVoice::Synthesize {
                    text: "audio".into(),
                    voice: None,
                },
            },
            json!({"voice":{"kind":"synthesize","text":"audio","voice":null}}),
            "audio",
        ),
        (
            OutboundPayload::Files {
                file_urls: vec!["a".into(), "b".into()],
            },
            json!({"fileUrls":["a","b"]}),
            "a\nb",
        ),
        (
            OutboundPayload::Poll {
                question: "choose".into(),
                options: vec!["a".into()],
            },
            json!({"poll":{"question":"choose","options":["a"]}}),
            "choose",
        ),
        (
            OutboundPayload::PresentationBlocks {
                blocks: json!([{"text":"block"}]),
            },
            json!({"blocks":[{"text":"block"}]}),
            "",
        ),
        (
            OutboundPayload::NativeChannelData {
                data: json!({"text":"native","enabled":true,"count":2}),
            },
            json!({"text":"native","enabled":true,"count":2}),
            "native",
        ),
    ];
    for (payload, mut legacy, content) in cases {
        let intent = ChannelOutboundIntent {
            idempotency_key: "key".into(),
            payload,
            ..Default::default()
        };
        legacy
            .as_object_mut()
            .unwrap()
            .insert("idempotencyKey".into(), json!("key"));
        assert_eq!(legacy_message_value_from_outbound_intent(&intent), legacy);
        assert_eq!(
            crate::relay::relay_send_action_from_outbound_intent(&intent)["content"],
            content
        );
    }
    let intent = outbound_intent_from_legacy_message("fixture", json!(true));
    assert_eq!(
        legacy_message_value_from_outbound_intent(&intent),
        json!({"payload":true,"idempotencyKey":intent.idempotency_key})
    );
}

#[test]
fn nested_receipt_ids_recover_parts_and_delivery_time() {
    let receipt = create_message_receipt_from_outbound_results(
        vec![
            MessageReceiptSourceResult {
                receipt: Some(MessageReceipt {
                    platform_message_ids: vec!["one".into(), "two".into()],
                    sent_at: 42,
                    ..Default::default()
                }),
                ..Default::default()
            },
            MessageReceiptSourceResult::default(),
        ],
        Some(MessageReceiptPartKind::Media),
        Some("thread".into()),
        Some("reply".into()),
        0,
    );
    assert_eq!(receipt.sent_at, 42);
    assert_eq!(receipt.parts.len(), 2);
    assert_eq!(receipt.parts[1].platform_message_id, "two");
    assert_eq!(receipt.parts[1].thread_id.as_deref(), Some("thread"));
    assert_eq!(receipt.parts[1].reply_to_id.as_deref(), Some("reply"));
    assert_eq!(receipt.parts[1].kind, MessageReceiptPartKind::Media);
    let empty = create_message_receipt_from_outbound_results(Vec::new(), None, None, None, 0);
    assert_eq!(empty.sent_at, 0);
    assert!(empty.parts.is_empty());
}

#[test]
fn typed_text_parts_preserve_order_and_delivery_identity_without_flattening() {
    let parts = ["first", "second"]
        .into_iter()
        .map(|text| OutboundContentPart::Content {
            part: ContentPart::Text {
                text: text.into(),
                markdown_dialect: None,
            },
        })
        .collect::<Vec<_>>();
    let intent = ChannelOutboundIntent {
        idempotency_key: "stable-key".into(),
        turn_id: Some("turn-42".into()),
        payload: OutboundPayload::Parts {
            parts: parts.clone(),
        },
        ..Default::default()
    };
    let legacy = legacy_message_value_from_outbound_intent(&intent);
    assert_eq!(legacy["parts"], serde_json::to_value(&parts).unwrap());
    assert_eq!(legacy["idempotencyKey"], "stable-key");
    assert_eq!(legacy["turnId"], "turn-42");
    let typed = tinychannels_bus::ChannelOutboundMessage::from_intent(&intent);
    assert_eq!(typed.parts, parts);
    assert_eq!(typed.idempotency_key, "stable-key");
    assert_eq!(typed.turn_id.as_deref(), Some("turn-42"));
}

#[test]
fn typed_projection_keeps_non_text_parts_intact() {
    let parts = vec![OutboundContentPart::Content {
        part: ContentPart::Native {
            provider: "fixture".into(),
            data: json!({"text":"not portable text"}),
        },
    }];
    let intent = ChannelOutboundIntent {
        payload: OutboundPayload::Parts {
            parts: parts.clone(),
        },
        ..Default::default()
    };
    let legacy = legacy_message_value_from_outbound_intent(&intent);
    assert!(legacy.get("text").is_none());
    assert_eq!(legacy["parts"], serde_json::to_value(&parts).unwrap());
    assert_eq!(
        tinychannels_bus::ChannelOutboundMessage::from_intent(&intent).parts,
        parts
    );
}

#[test]
fn typed_inbound_projection_keeps_reply_thread_and_topic_distinct() {
    let envelope = ChannelInboundEnvelope {
        reply_to_id: Some("source-message".into()),
        conversation: ConversationRef {
            id: "chat".into(),
            thread_id: Some("thread-id".into()),
            topic_id: Some("topic-id".into()),
            ..Default::default()
        },
        parts: vec![ContentPart::Text {
            text: "typed text".into(),
            markdown_dialect: None,
        }],
        ..Default::default()
    };
    let projected = typed_message_from_inbound_envelope(&envelope, 42);
    assert_eq!(projected.content, "typed text");
    assert_eq!(projected.reply_to_id.as_deref(), Some("source-message"));
    assert_eq!(projected.thread_ts.as_deref(), Some("thread-id"));
    assert_eq!(projected.topic_id.as_deref(), Some("topic-id"));
}
