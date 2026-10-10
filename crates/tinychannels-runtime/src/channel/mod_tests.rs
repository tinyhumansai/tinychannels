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
        "main:telegram:bot-a:group:tenant-a:-100123:topic-99"
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
        "main:telegram:bot-a:group:tenant-a:-100123:topic-99:alice"
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

    let keys = conversation_history_key_candidates(&msg);
    assert_eq!(keys.conversation_history_key, "telegram_alice_-100123");
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
        thread_ts: Some("topic-99".into()),
        sender_name: None,
    };
    let envelope = inbound_envelope_from_legacy_message(&msg);

    let projected = legacy_message_from_inbound_envelope(&envelope, 456);

    assert_eq!(projected.id, "msg-1");
    assert_eq!(projected.channel, "telegram");
    assert_eq!(projected.sender, "alice");
    assert_eq!(projected.content, "hello");
    assert_eq!(projected.reply_target, "-100123");
    assert_eq!(projected.thread_ts.as_deref(), Some("topic-99"));
    assert_eq!(projected.timestamp, 456);
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

    let projected = legacy_message_from_inbound_envelope(&envelope, 1);
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
fn legacy_session_key_candidates_match_openhuman_helpers() {
    let msg = ChannelMessage {
        id: "msg-1".into(),
        channel: "telegram".into(),
        sender: "alice".into(),
        content: "hello".into(),
        reply_target: "-100123".into(),
        timestamp: 123,
        thread_ts: Some("topic-99".into()),
        sender_name: None,
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
            OutboundPayload::Media {
                text: Some("caption".into()),
                media_urls: vec!["image".into()],
            },
            json!({"text":"caption","mediaUrls":["image"]}),
            "caption",
        ),
        (
            OutboundPayload::Voice {
                media_url: "audio".into(),
            },
            json!({"voiceUrl":"audio"}),
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
