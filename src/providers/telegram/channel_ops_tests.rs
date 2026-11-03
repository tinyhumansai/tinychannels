use super::*;
use crate::channel::{
    ChannelAdapter, ChannelDelete, ChannelEdit, ChannelInboundSink, ChannelStreamingDraft,
    ChannelTyping, LegacyChannelReaction,
};
use crate::providers::telegram::fixtures::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Hang guard for loops that must terminate on their own.
const GUARD: Duration = Duration::from_secs(20);

fn streaming(server: &MockServer) -> TelegramChannel {
    channel_for(server, &["*"]).with_streaming(StreamMode::Partial, 0, true)
}

#[tokio::test]
async fn send_draft_posts_placeholder_and_returns_message_id() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendMessage",
        ok_with(serde_json::json!({ "message_id": 321 })),
    )
    .await;
    let ch = streaming(&server);

    let id = ch
        .send_draft_provider(&SendMessage::new("", "100:7").in_thread(Some("55".into())))
        .await
        .unwrap();

    assert_eq!(id.as_deref(), Some("321"));
    assert_eq!(
        json_bodies(&server, "sendMessage").await,
        vec![serde_json::json!({
            "chat_id": "100",
            "text": "...",
            "message_thread_id": "7",
            "reply_to_message_id": 55,
        })]
    );
    assert!(ch.last_draft_edit.lock().contains_key("100"));
}

#[tokio::test]
async fn send_uses_separate_topic_and_reply_ids() {
    let server = mock_api().await;
    mount_post(&server, "sendMessage", ok()).await;
    let ch = channel_for(&server, &["*"]);
    let mut message = SendMessage::new("hello", "100");
    message.topic_id = Some("7".into());
    message.thread_ts = Some("55".into());
    message.reply_to_id = Some("56".into());

    ch.send_compat_message(&message).await.unwrap();

    assert_eq!(
        json_bodies(&server, "sendMessage").await,
        vec![serde_json::json!({
            "chat_id": "100",
            "text": "hello",
            "parse_mode": "MarkdownV2",
            "message_thread_id": "7",
            "reply_to_message_id": 56,
        })]
    );
}

#[tokio::test]
async fn send_draft_prefers_explicit_topic_and_reply_ids() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendMessage",
        ok_with(serde_json::json!({ "message_id": 323 })),
    )
    .await;
    let channel = streaming(&server);
    let mut message = SendMessage::new("thinking", "100:8");
    message.topic_id = Some("7".into());
    message.thread_ts = Some("55".into());
    message.reply_to_id = Some("56".into());

    assert_eq!(
        channel
            .send_draft_provider(&message)
            .await
            .unwrap()
            .as_deref(),
        Some("323")
    );
    assert_eq!(
        json_bodies(&server, "sendMessage").await,
        vec![serde_json::json!({
            "chat_id": "100",
            "text": "thinking",
            "message_thread_id": "7",
            "reply_to_message_id": 56,
        })]
    );
}

#[tokio::test]
async fn send_draft_without_message_id_in_result_returns_none() {
    let server = mock_api().await;
    mount_post(&server, "sendMessage", ok_with(serde_json::json!({}))).await;
    let ch = streaming(&server);

    let id = ch
        .send_draft_provider(&SendMessage::new("thinking", "100"))
        .await
        .unwrap();
    assert_eq!(id, None);
    assert_eq!(
        json_bodies(&server, "sendMessage").await[0]["text"],
        "thinking"
    );
}

#[tokio::test]
async fn streaming_draft_routes_forum_topic_and_reply_separately() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendMessage",
        ok_with(serde_json::json!({ "message_id": 322 })),
    )
    .await;
    let channel = streaming(&server);
    let intent = tinychannels_bus::ChannelOutboundIntent {
        idempotency_key: "draft-topic".into(),
        channel_id: "telegram".into(),
        channel_ref: Some(tinychannels_bus::ChannelRef {
            id: "telegram".into(),
            account_id: None,
        }),
        conversation_id: "-10042".into(),
        conversation_ref: Some(tinychannels_bus::ConversationRef {
            id: "-10042".into(),
            kind: tinychannels_bus::ConversationKind::Group,
            topic_id: Some("18".into()),
            ..Default::default()
        }),
        reply_to_id: Some("81".into()),
        payload: tinychannels_bus::OutboundPayload::Text {
            text: "Draft reply".into(),
        },
        ..Default::default()
    };

    let draft = ChannelStreamingDraft::start_draft(&channel, intent)
        .await
        .unwrap();

    assert_eq!(draft.as_deref(), Some("322"));
    assert_eq!(
        json_bodies(&server, "sendMessage").await,
        vec![serde_json::json!({
            "chat_id": "-10042",
            "text": "Draft reply",
            "message_thread_id": "18",
            "reply_to_message_id": 81,
        })]
    );
}

#[tokio::test]
async fn send_draft_surfaces_api_rejection() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendMessage",
        ResponseTemplate::new(400).set_body_string("chat not found"),
    )
    .await;
    let ch = streaming(&server);

    let err = ch
        .send_draft_provider(&SendMessage::new("x", "100"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("sendMessage (draft) failed"), "{err}");
    assert!(err.contains("chat not found"), "{err}");
}

fn typed_text(id: &str, text: String) -> tinychannels_bus::ChannelOutboundMessage {
    tinychannels_bus::ChannelOutboundMessage {
        channel_id: "telegram".into(),
        conversation_id: "-10042".into(),
        channel_ref: Some(tinychannels_bus::ChannelRef {
            id: "forged".into(),
            account_id: Some("untrusted-account".into()),
        }),
        conversation_ref: Some(tinychannels_bus::ConversationRef {
            kind: tinychannels_bus::ConversationKind::Group,
            id: "-10042".into(),
            scope_id: Some("scope".into()),
            parent_id: Some("parent".into()),
            thread_id: Some("77".into()),
            topic_id: Some("topic".into()),
        }),
        reply_to_id: Some("5".into()),
        thread_id: None,
        turn_id: None,
        idempotency_key: id.into(),
        parts: vec![tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Text {
                text,
                markdown_dialect: None,
            },
        }],
    }
}

#[tokio::test]
async fn typed_send_receipt_uses_actual_telegram_ids_and_full_address() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendMessage",
        ok_with(serde_json::json!({ "message_id": 321 })),
    )
    .await;
    let ch = channel_for(&server, &["*"]);
    let outcome = ch
        .send_v2_with_receipt(&typed_text("idem", "hello".into()))
        .await
        .unwrap();
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.receipt.platform_message_ids, ["321"]);
    assert_eq!(outcome.receipt.parts[0].edit_token.as_deref(), Some("321"));
    assert_eq!(
        outcome.receipt.parts[0].delete_token.as_deref(),
        Some("321")
    );
    assert_eq!(
        outcome
            .receipt
            .conversation
            .as_ref()
            .unwrap()
            .topic_id
            .as_deref(),
        Some("topic")
    );
    assert_eq!(outcome.receipt.parts[0].reply_to_id.as_deref(), Some("5"));
}

#[tokio::test]
async fn typed_send_decodes_normalized_telegram_reply_ids() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendMessage",
        ok_with(serde_json::json!({ "message_id": 322 })),
    )
    .await;
    let ch = channel_for(&server, &["*"]);
    let mut message = typed_text("normalized-reply", "hello".into());
    message.reply_to_id = Some("5".into());

    let outcome = ch.send_v2_with_receipt(&message).await.unwrap();

    assert_eq!(outcome.error, None);
    assert_eq!(
        json_bodies(&server, "sendMessage").await[0]["reply_to_message_id"],
        5
    );
}

#[tokio::test]
async fn typed_send_validation_accepts_normalized_reply_ids() {
    let server = mock_api().await;
    let ch = channel_for(&server, &["*"]);
    let mut message = typed_text("preflight", "hello".into());
    message.reply_to_id = Some("5".into());

    ChannelAdapter::validate_typed_send(&ch, &message).unwrap();
}

#[tokio::test]
async fn typed_send_validation_rejects_unsupported_parts_and_invalid_reply_ids() {
    let server = mock_api().await;
    let ch = channel_for(&server, &["*"]);
    let mut unsupported = typed_text("unsupported", "hello".into());
    unsupported.parts = vec![tinychannels_bus::OutboundContentPart::Content {
        part: tinychannels_bus::ContentPart::Image {
            media: Default::default(),
            caption: None,
            alt_text: None,
        },
    }];
    assert!(ChannelAdapter::validate_typed_send(&ch, &unsupported).is_err());

    let mut invalid_reply = typed_text("invalid-reply", "hello".into());
    invalid_reply.reply_to_id = Some("telegram_bad_id".into());
    assert!(ChannelAdapter::validate_typed_send(&ch, &invalid_reply).is_err());

    let mut empty = typed_text("empty", "hello".into());
    empty.parts.clear();
    assert!(ChannelAdapter::validate_typed_send(&ch, &empty).is_ok());
}

#[tokio::test]
async fn typed_send_validation_allows_host_media_when_scoped_repository_is_available() {
    let server = mock_api().await;
    let repository = ReadMediaFixture {
        stored: crate::host::StoredMedia {
            media: tinychannels_bus::MediaReference {
                kind: tinychannels_bus::MediaKind::Image,
                ..Default::default()
            },
            content_type: "image/png".into(),
            file_name: Some("preview.png".into()),
            bytes: png_with_dimensions(1, 1),
        },
        reads: Arc::default(),
    };
    let channel = channel_for(&server, &["*"]).with_media_repository(Arc::new(repository));

    assert!(
        ChannelAdapter::validate_typed_send(&channel, &image_message("opaque-photo-1")).is_ok()
    );
}

#[tokio::test]
async fn typed_send_receipt_rejects_non_text_parts_before_provider_io() {
    let server = mock_api().await;
    let ch = channel_for(&server, &["*"]);
    let mut message = typed_text("unsupported", "hello".into());
    message.parts = vec![tinychannels_bus::OutboundContentPart::Content {
        part: tinychannels_bus::ContentPart::Image {
            media: Default::default(),
            caption: None,
            alt_text: None,
        },
    }];

    let error = ch.send_v2_with_receipt(&message).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("rejected before provider dispatch"),
        "{error}"
    );
    assert!(json_bodies(&server, "sendMessage").await.is_empty());
}

#[tokio::test]
async fn typed_send_receipt_keeps_ordered_multiple_text_parts() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendMessage",
        ok_with(serde_json::json!({ "message_id": 654 })),
    )
    .await;
    let ch = channel_for(&server, &["*"]);
    let mut message = typed_text("multi-text", "first".into());
    message.conversation_ref = None;
    message.conversation_id = "600".into();
    message.thread_id = Some("18".into());
    message
        .parts
        .push(tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Text {
                text: "second".into(),
                markdown_dialect: None,
            },
        });

    let outcome = ch.send_v2_with_receipt(&message).await.unwrap();

    assert_eq!(
        json_bodies(&server, "sendMessage").await[0]["text"],
        "first\nsecond"
    );
    assert_eq!(outcome.receipt.platform_message_ids, ["654"]);
    assert_eq!(
        outcome.receipt.idempotency_key.as_deref(),
        Some("multi-text")
    );
    assert_eq!(outcome.receipt.parts[0].edit_token.as_deref(), Some("654"));
    let bodies = json_bodies(&server, "sendMessage").await;
    assert_eq!(bodies[0]["chat_id"], "600");
    assert_eq!(bodies[0]["message_thread_id"], "18");
    assert_eq!(outcome.receipt.thread_id.as_deref(), Some("18"));
}

#[tokio::test]
async fn typed_send_receipt_applies_tool_call_tag_sanitization() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendMessage",
        ok_with(serde_json::json!({ "message_id": 655 })),
    )
    .await;
    let ch = channel_for(&server, &["*"]);
    let message = typed_text(
        "sanitize",
        "before<tool_call>hidden</tool_call>[REACTION:👍|321] [IMAGE:https://example.test/image.png]".into(),
    );

    let outcome = ch.send_v2_with_receipt(&message).await.unwrap();

    assert_eq!(outcome.error, None);
    let bodies = json_bodies(&server, "sendMessage").await;
    assert_eq!(
        bodies[0]["text"],
        "before\\[REACTION:👍\\|321\\] \\[IMAGE:https://example\\.test/image\\.png\\]"
    );
    assert_eq!(bodies[0]["message_id"], serde_json::Value::Null);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "typed markers remain text, not operations"
    );
    assert_eq!(outcome.receipt.platform_message_ids, ["655"]);
}

#[tokio::test]
async fn typed_send_rejects_non_numeric_reply_reference_before_provider_io() {
    let server = mock_api().await;
    mount_post(&server, "sendMessage", ok()).await;
    let ch = channel_for(&server, &["*"]);
    let mut message = typed_text("bad-reply-id", "hello".into());
    message.reply_to_id = Some("reply:legacy".into());

    let error = ch.send_v2_with_receipt(&message).await.unwrap_err();

    assert!(error.to_string().contains("invalid address"));
    assert!(json_bodies(&server, "sendMessage").await.is_empty());
}

fn edit_intent(conversation_id: &str) -> tinychannels_bus::ChannelOutboundIntent {
    tinychannels_bus::ChannelOutboundIntent {
        idempotency_key: "edit-1".into(),
        channel_id: "telegram".into(),
        conversation_id: conversation_id.into(),
        payload: tinychannels_bus::OutboundPayload::Text {
            text: "updated <tool_call>hidden</tool_call> *text*".into(),
        },
        ..Default::default()
    }
}

fn telegram_receipt(message_id: Option<&str>) -> tinychannels_bus::channel::MessageReceipt {
    tinychannels_bus::channel::MessageReceipt {
        channel: Some(tinychannels_bus::ChannelRef {
            id: "telegram".into(),
            account_id: None,
        }),
        conversation: Some(tinychannels_bus::ConversationRef {
            id: "-10042".into(),
            ..Default::default()
        }),
        primary_platform_message_id: message_id.map(str::to_owned),
        ..Default::default()
    }
}

#[tokio::test]
async fn native_draft_upsert_creates_and_updates_only_matching_text_receipts() {
    use tinychannels_bus::{ContentPart, OutboundContentPart, OutboundPayload};

    let server = mock_api().await;
    mount_post(
        &server,
        "sendMessage",
        ok_with(serde_json::json!({ "message_id": 731 })),
    )
    .await;
    mount_post(&server, "editMessageText", ok()).await;
    let channel = channel_for(&server, &["*"]);

    let created = channel
        .upsert_draft(None, edit_intent("-10042"))
        .await
        .unwrap();
    assert_eq!(created.primary_platform_message_id.as_deref(), Some("731"));
    assert_eq!(created.parts[0].platform_message_id, "731");
    assert_eq!(
        json_bodies(&server, "sendMessage").await[0]["text"],
        "updated  *text*"
    );

    let updated = channel
        .upsert_draft(Some(&created), edit_intent("-10042"))
        .await
        .unwrap();
    assert_eq!(updated, created);
    assert_eq!(
        json_bodies(&server, "editMessageText").await[0]["message_id"],
        731
    );

    let mut invalid_payload = edit_intent("-10042");
    invalid_payload.payload = OutboundPayload::Parts {
        parts: vec![OutboundContentPart::Content {
            part: ContentPart::Image {
                media: Default::default(),
                caption: None,
                alt_text: None,
            },
        }],
    };
    assert!(channel.upsert_draft(None, invalid_payload).await.is_err());

    let mut empty_payload = edit_intent("-10042");
    empty_payload.payload = OutboundPayload::Parts { parts: vec![] };
    assert!(channel.upsert_draft(None, empty_payload).await.is_err());

    let mut mismatched_conversation = edit_intent("-10042");
    mismatched_conversation.conversation_ref = Some(tinychannels_bus::ConversationRef {
        id: "-10043".into(),
        ..Default::default()
    });
    assert!(
        channel
            .upsert_draft(None, mismatched_conversation)
            .await
            .is_err()
    );

    let mut wrong_receipt_scope = created.clone();
    wrong_receipt_scope.conversation.as_mut().unwrap().id = "-10043".into();
    assert!(
        channel
            .upsert_draft(Some(&wrong_receipt_scope), edit_intent("-10042"))
            .await
            .is_err()
    );

    let no_message_id = telegram_receipt(None);
    assert!(
        channel
            .upsert_draft(Some(&no_message_id), edit_intent("-10042"))
            .await
            .is_err()
    );
    let invalid_message_id = telegram_receipt(Some("not-numeric"));
    assert!(
        channel
            .upsert_draft(Some(&invalid_message_id), edit_intent("-10042"))
            .await
            .is_err()
    );

    let mut invalid_reply = edit_intent("-10042");
    invalid_reply.reply_to_id = Some("reply:legacy".into());
    assert!(channel.upsert_draft(None, invalid_reply).await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn native_draft_upsert_marks_provider_failures_as_already_attempted() {
    let cases = [
        (
            "editMessageText",
            ResponseTemplate::new(200).set_body_string("bad json"),
            true,
        ),
        (
            "sendMessage",
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": false })),
            false,
        ),
        (
            "sendMessage",
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "ok": true, "result": {} })),
            false,
        ),
    ];

    for (method_name, response, existing) in cases {
        let server = mock_api().await;
        mount_post(&server, method_name, response).await;
        let channel = channel_for(&server, &["*"]);
        let previous = telegram_receipt(Some("81"));
        let error = channel
            .upsert_draft(existing.then_some(&previous), edit_intent("-10042"))
            .await
            .unwrap_err();

        assert!(error.sent_before_error);
        assert!(!error.retryable);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn streaming_draft_rejects_non_text_payloads_and_mismatched_scope() {
    use tinychannels_bus::OutboundPayload;

    let server = mock_api().await;
    let channel = streaming(&server);
    let mut non_text = edit_intent("-10042");
    non_text.payload = OutboundPayload::Poll {
        question: "Approval?".into(),
        options: vec!["Yes".into(), "No".into()],
    };
    let error = crate::channel::ChannelStreamingDraft::start_draft(&channel, non_text)
        .await
        .unwrap_err();
    assert_eq!(error.kind, crate::channel::SendErrorKind::BadFormat);
    assert!(error.unsupported);
    assert!(!error.retryable);

    let mut wrong_scope = edit_intent("-10042");
    wrong_scope.channel_ref = Some(tinychannels_bus::ChannelRef {
        id: "discord".into(),
        account_id: None,
    });
    let error = crate::channel::ChannelStreamingDraft::start_draft(&channel, wrong_scope)
        .await
        .unwrap_err();
    assert_eq!(error.kind, crate::channel::SendErrorKind::Forbidden);
    assert!(error.message.contains("scope mismatch"));
    assert!(!error.retryable);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[derive(Clone)]
struct ReadMediaFixture {
    stored: crate::host::StoredMedia,
    reads: Arc<std::sync::Mutex<Vec<(crate::host::MediaScope, String, u64)>>>,
}

struct FailingReadMediaFixture {
    stored: crate::host::StoredMedia,
    fail_id: String,
}

#[async_trait::async_trait]
impl crate::host::MediaRepository for FailingReadMediaFixture {
    async fn store_media(
        &self,
        _request: crate::host::MediaStoreRequest,
    ) -> anyhow::Result<tinychannels_bus::MediaReference> {
        anyhow::bail!("fixture is read-only")
    }

    async fn read_media(
        &self,
        _scope: crate::host::MediaScope,
        opaque_id: String,
        _max_bytes: u64,
    ) -> anyhow::Result<crate::host::StoredMedia> {
        if opaque_id == self.fail_id {
            anyhow::bail!("fixture storage read failed")
        }
        Ok(self.stored.clone())
    }
}

#[async_trait::async_trait]
impl crate::host::MediaRepository for ReadMediaFixture {
    async fn store_media(
        &self,
        _request: crate::host::MediaStoreRequest,
    ) -> anyhow::Result<tinychannels_bus::MediaReference> {
        anyhow::bail!("fixture is read-only")
    }

    async fn read_media(
        &self,
        scope: crate::host::MediaScope,
        opaque_id: String,
        max_bytes: u64,
    ) -> anyhow::Result<crate::host::StoredMedia> {
        self.reads
            .lock()
            .unwrap()
            .push((scope, opaque_id, max_bytes));
        Ok(self.stored.clone())
    }
}

fn image_message(opaque_id: &str) -> tinychannels_bus::ChannelOutboundMessage {
    tinychannels_bus::ChannelOutboundMessage {
        channel_id: "telegram".into(),
        conversation_id: "-10042".into(),
        channel_ref: Some(tinychannels_bus::ChannelRef {
            id: "telegram".into(),
            account_id: Some("telegram-main".into()),
        }),
        conversation_ref: Some(tinychannels_bus::ConversationRef {
            id: "-10042".into(),
            kind: tinychannels_bus::ConversationKind::Group,
            thread_id: Some("18".into()),
            ..Default::default()
        }),
        reply_to_id: Some("27".into()),
        thread_id: Some("18".into()),
        turn_id: Some("turn-image-1".into()),
        idempotency_key: "image-send-1".into(),
        parts: vec![tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Image {
                media: tinychannels_bus::MediaReference {
                    opaque_id: Some(opaque_id.into()),
                    kind: tinychannels_bus::MediaKind::Image,
                    ..Default::default()
                },
                caption: Some("photo <tool_call>hidden</tool_call>".into()),
                alt_text: Some("preview".into()),
            },
        }],
    }
}

#[derive(Clone)]
struct ReadVoiceFixture {
    stored: tinychannels_bus::HostReadVoiceMediaResult,
    reads: Arc<std::sync::Mutex<Vec<(tinychannels_bus::HostVoiceScope, String)>>>,
}

#[async_trait::async_trait]
impl crate::host::VoiceMediaStore for ReadVoiceFixture {
    async fn store_voice(
        &self,
        _request: crate::host::VoiceMediaRequest,
    ) -> anyhow::Result<tinychannels_bus::HostStoreVoiceMediaResult> {
        anyhow::bail!("fixture is read-only")
    }

    async fn read_voice(
        &self,
        scope: tinychannels_bus::HostVoiceScope,
        opaque_id: String,
    ) -> anyhow::Result<tinychannels_bus::HostReadVoiceMediaResult> {
        self.reads.lock().unwrap().push((scope, opaque_id));
        Ok(self.stored.clone())
    }
}

fn voice_message(
    opaque_id: &str,
    transcript: Option<String>,
    codec: Option<&str>,
) -> tinychannels_bus::ChannelOutboundMessage {
    let mut message = image_message("voice-send-1");
    message.reply_to_id = Some("27".into());
    message.parts = vec![
        tinychannels_bus::OutboundContentPart::Voice {
            voice: tinychannels_bus::OutboundVoice::Media {
                media: tinychannels_bus::MediaReference {
                    opaque_id: Some(opaque_id.into()),
                    kind: tinychannels_bus::MediaKind::Audio,
                    ..Default::default()
                },
                codec: codec.map(str::to_owned),
                duration_ms: Some(1_001),
            },
        },
        tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Text {
                text: transcript.unwrap_or_default(),
                markdown_dialect: None,
            },
        },
    ];
    message
}

fn png_with_dimensions(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = vec![0; 24];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    bytes
}

fn media_message(part: tinychannels_bus::ContentPart) -> tinychannels_bus::ChannelOutboundMessage {
    let mut message = image_message("opaque-media");
    message.parts = vec![tinychannels_bus::OutboundContentPart::Content { part }];
    message
}

fn structured_message(
    parts: Vec<tinychannels_bus::OutboundContentPart>,
) -> tinychannels_bus::ChannelOutboundMessage {
    let mut message = typed_text("structured-1", String::new());
    message.parts = parts;
    message.conversation_ref = Some(tinychannels_bus::ConversationRef {
        id: "-10042".into(),
        kind: tinychannels_bus::ConversationKind::Group,
        topic_id: Some("18".into()),
        ..Default::default()
    });
    message
}

struct NoopInboundSink;

#[async_trait::async_trait]
impl ChannelInboundSink for NoopInboundSink {
    async fn push(
        &self,
        _envelope: tinychannels_bus::ChannelInboundEnvelope,
    ) -> Result<(), crate::channel::ChannelSendError> {
        Ok(())
    }
}

#[tokio::test]
async fn typed_image_upload_reads_host_scope_and_returns_provider_receipt() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendPhoto",
        ok_with(serde_json::json!({ "message_id": 902 })),
    )
    .await;
    let image = png_with_dimensions(320, 240);
    let repository = ReadMediaFixture {
        stored: crate::host::StoredMedia {
            media: tinychannels_bus::MediaReference {
                kind: tinychannels_bus::MediaKind::Image,
                ..Default::default()
            },
            content_type: "image/png".into(),
            file_name: Some("preview.png".into()),
            bytes: image.clone(),
        },
        reads: Arc::default(),
    };
    let reads = repository.reads.clone();
    let ch = channel_for(&server, &["*"]).with_media_repository(Arc::new(repository));

    let outcome = ch
        .send_v2_with_receipt(&image_message("opaque-photo-1"))
        .await
        .unwrap();

    assert_eq!(outcome.error, None);
    assert_eq!(outcome.receipt.platform_message_ids, ["902"]);
    assert_eq!(outcome.receipt.parts.len(), 1);
    assert_eq!(
        outcome.receipt.parts[0].kind,
        tinychannels_bus::channel::MessageReceiptPartKind::Media
    );
    assert_eq!(outcome.receipt.parts[0].thread_id.as_deref(), Some("18"));
    let read_records = reads.lock().unwrap().clone();
    assert_eq!(read_records.len(), 1);
    assert_eq!(read_records[0].1, "opaque-photo-1");
    assert_eq!(read_records[0].2, 20 * 1024 * 1024);
    assert_eq!(read_records[0].0.configured_instance, "telegram-main");
    assert_eq!(read_records[0].0.channel.id, "telegram");
    assert_eq!(
        read_records[0].0.conversation.thread_id.as_deref(),
        Some("18")
    );
    assert_eq!(read_records[0].0.turn_id, "turn-image-1");
    let bodies = raw_bodies(&server, "sendPhoto").await;
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].contains("name=\"reply_parameters\"\r\n\r\n{\"message_id\":27}"));
    assert!(bodies[0].contains("name=\"message_thread_id\"\r\n\r\n18"));
    assert!(bodies[0].contains("name=\"caption\"\r\n\r\nphoto\r\n"));
    assert!(bodies[0].contains("filename=\"preview.png\""));
    assert!(bodies[0].contains("Content-Type: image/png"));
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests[0]
            .body
            .windows(image.len())
            .any(|window| window == image)
    );
}

#[tokio::test]
async fn typed_voice_uses_scoped_host_audio_and_selects_voice_or_audio_endpoint() {
    for (codec, mime_type, method_name, field) in [
        (Some("ogg_opus"), "audio/ogg", "sendVoice", "voice"),
        (Some("mp3"), "audio/mpeg", "sendAudio", "audio"),
    ] {
        let server = mock_api().await;
        mount_post(
            &server,
            method_name,
            ok_with(serde_json::json!({ "message_id": 911 })),
        )
        .await;
        let reads = Arc::new(std::sync::Mutex::new(Vec::new()));
        let store = ReadVoiceFixture {
            stored: tinychannels_bus::HostReadVoiceMediaResult {
                audio_base64: base64::engine::general_purpose::STANDARD.encode(b"voice audio"),
                mime_type: mime_type.into(),
                file_name: Some("recording.ogg".into()),
            },
            reads: reads.clone(),
        };
        let channel = channel_for(&server, &["*"]).with_voice_media_store(Arc::new(store));

        let outcome = channel
            .send_v2_with_receipt(&voice_message(
                "opaque-voice-1",
                Some("speech".into()),
                codec,
            ))
            .await
            .unwrap();

        assert_eq!(outcome.error, None, "{method_name}");
        assert_eq!(outcome.receipt.platform_message_ids, ["911"]);
        assert_eq!(
            outcome.receipt.parts[0].kind,
            tinychannels_bus::channel::MessageReceiptPartKind::Voice
        );
        let read_records = reads.lock().unwrap().clone();
        assert_eq!(read_records.len(), 1);
        assert_eq!(read_records[0].1, "opaque-voice-1");
        assert_eq!(read_records[0].0.configured_instance, "telegram-main");
        assert_eq!(read_records[0].0.turn_id, "turn-image-1");
        let body = raw_bodies(&server, method_name).await;
        assert_eq!(body.len(), 1);
        assert!(body[0].contains(&format!("name=\"{field}\"; filename=\"recording.ogg\"")));
        assert!(body[0].contains("name=\"caption\"\r\n\r\nspeech"));
        assert!(body[0].contains("name=\"reply_parameters\"\r\n\r\n{\"message_id\":27}"));
        let requests = server.received_requests().await.unwrap();
        assert!(
            requests[0]
                .body
                .windows(11)
                .any(|window| window == b"voice audio")
        );
    }
}

#[tokio::test]
async fn oversized_voice_transcript_is_sent_as_a_follow_up_with_combined_receipt() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendVoice",
        ok_with(serde_json::json!({ "message_id": 911 })),
    )
    .await;
    mount_post(
        &server,
        "sendMessage",
        ok_with(serde_json::json!({ "message_id": 912 })),
    )
    .await;
    let store = ReadVoiceFixture {
        stored: tinychannels_bus::HostReadVoiceMediaResult {
            audio_base64: base64::engine::general_purpose::STANDARD.encode(b"voice audio"),
            mime_type: "audio/ogg".into(),
            file_name: None,
        },
        reads: Arc::default(),
    };
    let channel = channel_for(&server, &["*"]).with_voice_media_store(Arc::new(store));
    let transcript = format!("{}spoken detail", "spoken detail ".repeat(99));

    let outcome = channel
        .send_v2_with_receipt(&voice_message(
            "opaque-voice-2",
            Some(transcript.clone()),
            Some("ogg_opus"),
        ))
        .await
        .unwrap();

    assert_eq!(outcome.error, None);
    assert_eq!(outcome.receipt.platform_message_ids, ["911", "912"]);
    assert_eq!(outcome.receipt.parts.len(), 2);
    assert_eq!(
        outcome.receipt.parts[0].kind,
        tinychannels_bus::channel::MessageReceiptPartKind::Voice
    );
    assert_eq!(
        outcome.receipt.parts[1].kind,
        tinychannels_bus::channel::MessageReceiptPartKind::Text
    );
    assert_eq!(outcome.receipt.parts[1].index, 1);
    let voice = raw_bodies(&server, "sendVoice").await;
    assert_eq!(voice.len(), 1);
    assert!(!voice[0].contains("name=\"caption\""));
    assert_eq!(sent_texts(&server).await, [transcript]);
}

#[tokio::test]
async fn oversized_voice_transcript_failure_keeps_audio_receipt_nonretryable() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendVoice",
        ok_with(serde_json::json!({ "message_id": 913 })),
    )
    .await;
    mount_post(
        &server,
        "sendMessage",
        ResponseTemplate::new(500).set_body_string("text endpoint unavailable"),
    )
    .await;
    let store = ReadVoiceFixture {
        stored: tinychannels_bus::HostReadVoiceMediaResult {
            audio_base64: base64::engine::general_purpose::STANDARD.encode(b"voice audio"),
            mime_type: "audio/ogg".into(),
            file_name: None,
        },
        reads: Arc::default(),
    };
    let channel = channel_for(&server, &["*"]).with_voice_media_store(Arc::new(store));

    let outcome = channel
        .send_v2_with_receipt(&voice_message(
            "opaque-voice-partial",
            Some("spoken detail ".repeat(100)),
            Some("ogg_opus"),
        ))
        .await
        .unwrap();

    assert_eq!(outcome.receipt.platform_message_ids, ["913"]);
    let error = outcome
        .error
        .expect("transcript delivery failure is reported");
    assert!(error.sent_before_error);
    assert!(!error.retryable);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn oversized_voice_transcript_partial_delivery_keeps_all_receipt_ids() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendVoice",
        ok_with(serde_json::json!({ "message_id": 914 })),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(api_path("sendMessage")))
        .respond_with(ok_with(serde_json::json!({ "message_id": 915 })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_post(
        &server,
        "sendMessage",
        ResponseTemplate::new(400).set_body_string("second transcript chunk rejected"),
    )
    .await;
    let store = ReadVoiceFixture {
        stored: tinychannels_bus::HostReadVoiceMediaResult {
            audio_base64: base64::engine::general_purpose::STANDARD.encode(b"voice audio"),
            mime_type: "audio/ogg".into(),
            file_name: None,
        },
        reads: Arc::default(),
    };
    let channel = channel_for(&server, &["*"]).with_voice_media_store(Arc::new(store));

    let outcome = channel
        .send_v2_with_receipt(&voice_message(
            "opaque-voice-partial-chunks",
            Some("spoken detail ".repeat(400)),
            Some("ogg_opus"),
        ))
        .await
        .unwrap();

    assert_eq!(outcome.receipt.platform_message_ids, ["914", "915"]);
    assert_eq!(outcome.receipt.parts.len(), 2);
    assert_eq!(
        outcome.receipt.parts[1].kind,
        tinychannels_bus::channel::MessageReceiptPartKind::Text
    );
    let error = outcome.error.expect("second transcript chunk failed");
    assert!(error.sent_before_error);
    assert!(!error.retryable);
    assert_eq!(error.continuation_message_ids, ["915"]);
}

#[test]
fn telegram_image_dimensions_accept_png_and_jpeg_headers_and_reject_invalid_data() {
    assert_eq!(
        telegram_image_dimensions("image/png", &png_with_dimensions(640, 480)),
        Some((640, 480))
    );
    assert_eq!(telegram_image_dimensions("image/png", b"not png"), None);
    assert_eq!(telegram_image_dimensions("image/gif", b"GIF89a"), None);
    let jpeg_sof = [
        0xff, 0xd8, 0xff, 0xc0, 0x00, 0x07, 0x08, 0x01, 0x2c, 0x01, 0x90,
    ];
    assert_eq!(
        telegram_image_dimensions("image/jpeg", &jpeg_sof),
        Some((400, 300))
    );
    assert_eq!(
        telegram_image_dimensions("image/jpeg", &[0xff, 0xd8, 0xff]),
        None
    );
}

#[tokio::test]
async fn typed_voice_rejects_unprepared_or_unscoped_voice_before_provider_io() {
    use tinychannels_bus::{OutboundContentPart, OutboundVoice};

    let server = mock_api().await;
    let store = Arc::new(ReadVoiceFixture {
        stored: tinychannels_bus::HostReadVoiceMediaResult {
            audio_base64: "%%%".into(),
            mime_type: "audio/ogg".into(),
            file_name: None,
        },
        reads: Arc::default(),
    });
    let channel = channel_for(&server, &["*"]).with_voice_media_store(store);
    assert!(
        channel_for(&server, &["*"])
            .send_v2_with_receipt(&voice_message("voice", None, None))
            .await
            .is_err()
    );

    let synthesized = tinychannels_bus::ChannelOutboundMessage {
        parts: vec![OutboundContentPart::Voice {
            voice: OutboundVoice::Synthesize {
                text: "hello".into(),
                voice: None,
            },
        }],
        ..voice_message("voice", None, None)
    };
    assert!(channel.send_v2_with_receipt(&synthesized).await.is_err());

    let mut unsupported_caption_part = voice_message("voice", None, None);
    unsupported_caption_part
        .parts
        .push(OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Image {
                media: Default::default(),
                caption: None,
                alt_text: None,
            },
        });
    assert!(
        channel
            .send_v2_with_receipt(&unsupported_caption_part)
            .await
            .is_err()
    );

    let mut missing_opaque_id = voice_message("voice", None, None);
    if let OutboundContentPart::Voice {
        voice: OutboundVoice::Media { media, .. },
    } = &mut missing_opaque_id.parts[0]
    {
        media.opaque_id = Some("  ".into());
    }
    assert!(
        channel
            .send_v2_with_receipt(&missing_opaque_id)
            .await
            .is_err()
    );

    let mut missing_turn_id = voice_message("voice", None, None);
    missing_turn_id.turn_id = None;
    assert!(
        channel
            .send_v2_with_receipt(&missing_turn_id)
            .await
            .is_err()
    );
    assert!(
        channel
            .send_v2_with_receipt(&voice_message("voice", None, None))
            .await
            .is_err()
    );

    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn typed_media_selects_native_telegram_upload_methods_by_kind_and_format() {
    use tinychannels_bus::{ContentPart, MediaKind, MediaReference};

    let cases = [
        (
            ContentPart::Video {
                media: MediaReference {
                    opaque_id: Some("video".into()),
                    kind: MediaKind::Video,
                    ..Default::default()
                },
                caption: Some("clip".into()),
                duration_ms: Some(1_500),
                thumbnail: None,
            },
            MediaKind::Video,
            "video/mp4",
            "sendVideo",
            "video",
        ),
        (
            ContentPart::Audio {
                media: MediaReference {
                    opaque_id: Some("audio".into()),
                    kind: MediaKind::Audio,
                    ..Default::default()
                },
                codec: Some("mp3".into()),
                duration_ms: Some(999),
                title: Some("song".into()),
                transcript: Some("lyrics".into()),
            },
            MediaKind::Audio,
            "audio/mpeg",
            "sendAudio",
            "audio",
        ),
        (
            ContentPart::File {
                media: MediaReference {
                    opaque_id: Some("document".into()),
                    kind: MediaKind::Document,
                    ..Default::default()
                },
                file_name: Some("report.pdf".into()),
                caption: Some("report".into()),
            },
            MediaKind::Document,
            "application/pdf",
            "sendDocument",
            "document",
        ),
        (
            ContentPart::Sticker {
                media: MediaReference {
                    opaque_id: Some("sticker".into()),
                    kind: MediaKind::Image,
                    ..Default::default()
                },
                pack_id: None,
                sticker_id: None,
                emoji: Some("🙂".into()),
                alt_text: None,
            },
            MediaKind::Image,
            "image/webp",
            "sendSticker",
            "sticker",
        ),
        (
            ContentPart::Image {
                media: MediaReference {
                    opaque_id: Some("gif".into()),
                    kind: MediaKind::Image,
                    ..Default::default()
                },
                caption: None,
                alt_text: None,
            },
            MediaKind::Image,
            "image/gif",
            "sendDocument",
            "document",
        ),
    ];

    for (index, (part, media_kind, content_type, method, field)) in cases.into_iter().enumerate() {
        let server = mock_api().await;
        mount_post(
            &server,
            method,
            ok_with(serde_json::json!({ "message_id": index as i64 + 1 })),
        )
        .await;
        let ch = channel_for(&server, &["*"]).with_media_repository(Arc::new(ReadMediaFixture {
            stored: crate::host::StoredMedia {
                media: MediaReference {
                    kind: media_kind,
                    ..Default::default()
                },
                content_type: content_type.into(),
                file_name: Some("upload.bin".into()),
                bytes: vec![1, 2, 3, 4],
            },
            reads: Arc::default(),
        }));

        let outcome = ch.send_v2_with_receipt(&media_message(part)).await.unwrap();

        assert_eq!(outcome.error, None, "{method}");
        assert_eq!(
            outcome.receipt.platform_message_ids,
            [(index + 1).to_string()]
        );
        let body = raw_bodies(&server, method).await;
        assert_eq!(body.len(), 1, "{method}");
        assert!(body[0].contains(&format!("name=\"{field}\"")), "{method}");
    }
}

#[tokio::test]
async fn typed_media_rejects_invalid_scoped_uploads_before_provider_io() {
    use tinychannels_bus::{ContentPart, MediaKind, MediaReference};

    let stored = |kind, content_type: &str, bytes| crate::host::StoredMedia {
        media: MediaReference {
            kind,
            ..Default::default()
        },
        content_type: content_type.into(),
        file_name: None,
        bytes,
    };
    let mut cases = vec![
        (
            "image reference without host opaque id",
            ContentPart::Image {
                media: MediaReference {
                    kind: MediaKind::Image,
                    ..Default::default()
                },
                caption: None,
                alt_text: None,
            },
            stored(MediaKind::Image, "image/png", png_with_dimensions(1, 1)),
        ),
        (
            "stored media kind mismatch",
            ContentPart::Image {
                media: MediaReference {
                    opaque_id: Some("mismatch".into()),
                    kind: MediaKind::Image,
                    ..Default::default()
                },
                caption: None,
                alt_text: None,
            },
            stored(MediaKind::Document, "image/png", png_with_dimensions(1, 1)),
        ),
        (
            "empty stored bytes",
            ContentPart::Image {
                media: MediaReference {
                    opaque_id: Some("empty".into()),
                    kind: MediaKind::Image,
                    ..Default::default()
                },
                caption: None,
                alt_text: None,
            },
            stored(MediaKind::Image, "image/png", vec![]),
        ),
        (
            "invalid photo header",
            ContentPart::Image {
                media: MediaReference {
                    opaque_id: Some("bad-header".into()),
                    kind: MediaKind::Image,
                    ..Default::default()
                },
                caption: None,
                alt_text: None,
            },
            stored(MediaKind::Image, "image/png", vec![1, 2, 3]),
        ),
        (
            "photo dimensions outside provider limits",
            ContentPart::Image {
                media: MediaReference {
                    opaque_id: Some("large-photo".into()),
                    kind: MediaKind::Image,
                    ..Default::default()
                },
                caption: None,
                alt_text: None,
            },
            stored(
                MediaKind::Image,
                "image/png",
                png_with_dimensions(6000, 6000),
            ),
        ),
        (
            "caption beyond Bot API limit",
            ContentPart::Image {
                media: MediaReference {
                    opaque_id: Some("long-caption".into()),
                    kind: MediaKind::Image,
                    ..Default::default()
                },
                caption: Some("x".repeat(1025)),
                alt_text: None,
            },
            stored(MediaKind::Image, "image/png", png_with_dimensions(1, 1)),
        ),
        (
            "sticker format other than webp",
            ContentPart::Sticker {
                media: MediaReference {
                    opaque_id: Some("jpeg-sticker".into()),
                    kind: MediaKind::Image,
                    ..Default::default()
                },
                pack_id: None,
                sticker_id: None,
                emoji: None,
                alt_text: None,
            },
            stored(MediaKind::Image, "image/png", png_with_dimensions(1, 1)),
        ),
        (
            "audio format unsupported by sendAudio",
            ContentPart::Audio {
                media: MediaReference {
                    opaque_id: Some("ogg-audio".into()),
                    kind: MediaKind::Audio,
                    ..Default::default()
                },
                codec: Some("opus".into()),
                duration_ms: None,
                title: None,
                transcript: None,
            },
            stored(MediaKind::Audio, "audio/ogg", vec![1, 2, 3]),
        ),
    ];

    let server = mock_api().await;
    for (label, part, media) in cases.drain(..) {
        let adapter =
            channel_for(&server, &["*"]).with_media_repository(Arc::new(ReadMediaFixture {
                stored: media,
                reads: Arc::default(),
            }));
        assert!(
            adapter
                .send_v2_with_receipt(&media_message(part))
                .await
                .is_err(),
            "accepted invalid media: {label}"
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn typed_media_keeps_successful_receipt_when_a_later_part_is_invalid() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendPhoto",
        ok_with(serde_json::json!({ "message_id": 921 })),
    )
    .await;
    let channel = channel_for(&server, &["*"]).with_media_repository(Arc::new(ReadMediaFixture {
        stored: crate::host::StoredMedia {
            media: tinychannels_bus::MediaReference {
                kind: tinychannels_bus::MediaKind::Image,
                ..Default::default()
            },
            content_type: "image/png".into(),
            file_name: Some("photo.png".into()),
            bytes: png_with_dimensions(10, 10),
        },
        reads: Arc::default(),
    }));
    let mut message = image_message("first-image");
    message
        .parts
        .push(tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Image {
                media: Default::default(),
                caption: None,
                alt_text: None,
            },
        });

    let outcome = channel.send_v2_with_receipt(&message).await.unwrap();

    assert_eq!(outcome.receipt.platform_message_ids, ["921"]);
    let error = outcome.error.expect("later invalid media is reported");
    assert!(error.sent_before_error);
    assert!(!error.retryable);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn typed_media_keeps_first_receipt_when_later_scoped_read_fails() {
    let server = mock_api().await;
    mount_post(
        &server,
        "sendPhoto",
        ok_with(serde_json::json!({ "message_id": 922 })),
    )
    .await;
    let channel =
        channel_for(&server, &["*"]).with_media_repository(Arc::new(FailingReadMediaFixture {
            stored: crate::host::StoredMedia {
                media: tinychannels_bus::MediaReference {
                    kind: tinychannels_bus::MediaKind::Image,
                    ..Default::default()
                },
                content_type: "image/png".into(),
                file_name: Some("photo.png".into()),
                bytes: png_with_dimensions(10, 10),
            },
            fail_id: "second-image".into(),
        }));
    let mut message = image_message("first-image");
    message
        .parts
        .push(tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Image {
                media: tinychannels_bus::MediaReference {
                    opaque_id: Some("second-image".into()),
                    kind: tinychannels_bus::MediaKind::Image,
                    ..Default::default()
                },
                caption: None,
                alt_text: None,
            },
        });

    let outcome = channel.send_v2_with_receipt(&message).await.unwrap();

    assert_eq!(outcome.receipt.platform_message_ids, ["922"]);
    let error = outcome
        .error
        .expect("failed scoped read follows a sent part");
    assert!(error.sent_before_error);
    assert!(!error.retryable);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn typed_media_preflight_requires_complete_provider_scope() {
    let server = mock_api().await;
    let with_storage = || {
        channel_for(&server, &["*"]).with_media_repository(Arc::new(ReadMediaFixture {
            stored: crate::host::StoredMedia {
                media: tinychannels_bus::MediaReference {
                    kind: tinychannels_bus::MediaKind::Image,
                    ..Default::default()
                },
                content_type: "image/png".into(),
                file_name: None,
                bytes: png_with_dimensions(1, 1),
            },
            reads: Arc::default(),
        }))
    };

    let mut missing_conversation = image_message("opaque");
    missing_conversation.conversation_id.clear();
    missing_conversation
        .conversation_ref
        .as_mut()
        .unwrap()
        .id
        .clear();
    assert!(
        with_storage()
            .send_v2_with_receipt(&missing_conversation)
            .await
            .is_err()
    );

    let mut missing_account = image_message("opaque");
    missing_account.channel_ref.as_mut().unwrap().account_id = None;
    assert!(
        with_storage()
            .send_v2_with_receipt(&missing_account)
            .await
            .is_err()
    );

    let mut missing_turn = image_message("opaque");
    missing_turn.turn_id = None;
    assert!(
        with_storage()
            .send_v2_with_receipt(&missing_turn)
            .await
            .is_err()
    );

    let mut invalid_thread = image_message("opaque");
    invalid_thread.conversation_ref.as_mut().unwrap().thread_id = Some("thread-x".into());
    assert!(
        with_storage()
            .send_v2_with_receipt(&invalid_thread)
            .await
            .is_err()
    );

    let mut invalid_reply = image_message("opaque");
    invalid_reply.reply_to_id = Some("reply-x".into());
    assert!(
        with_storage()
            .send_v2_with_receipt(&invalid_reply)
            .await
            .is_err()
    );

    assert!(
        channel_for(&server, &["*"])
            .send_v2_with_receipt(&image_message("opaque"))
            .await
            .is_err()
    );

    let mut unsupported_content = image_message("opaque");
    unsupported_content
        .parts
        .push(tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Location {
                latitude: 1.0,
                longitude: 2.0,
                label: None,
                address: None,
                accuracy_meters: None,
                live_until_unix_ms: None,
            },
        });
    assert!(
        with_storage()
            .send_v2_with_receipt(&unsupported_content)
            .await
            .is_err()
    );

    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn adapter_start_rejects_duplicate_listener_and_stop_cancels_active_poll() {
    let server = mock_api().await;
    mount_post(&server, "deleteWebhook", ok()).await;
    mount_post(
        &server,
        "getUpdates",
        ok_with(serde_json::Value::Array(Vec::new())),
    )
    .await;
    let channel = Arc::new(channel_for(&server, &["*"]));
    let sink = NoopInboundSink;
    let listener_channel = Arc::clone(&channel);
    let listener = tokio::spawn(async move { listener_channel.start(&sink).await });

    for _ in 0..1_000 {
        if channel.listener_running.load(Ordering::Acquire) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(channel.listener_running.load(Ordering::Acquire));
    let duplicate = channel.start(&NoopInboundSink).await.unwrap_err();
    assert_eq!(duplicate.kind, crate::channel::SendErrorKind::Forbidden);
    channel.stop().await.unwrap();
    listener.await.unwrap().unwrap();
    assert!(!channel.listener_running.load(Ordering::Acquire));
}

#[tokio::test]
async fn adapter_status_reports_health_and_send_returns_typed_receipt() {
    let server = mock_api().await;
    Mock::given(method("GET"))
        .and(path(api_path("getMe")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {"id": 7}
        })))
        .mount(&server)
        .await;
    mount_post(
        &server,
        "sendMessage",
        ok_with(serde_json::json!({"message_id": 94})),
    )
    .await;
    mount_post(
        &server,
        "sendChatAction",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
    )
    .await;
    mount_post(
        &server,
        "setMessageReaction",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
    )
    .await;
    mount_post(
        &server,
        "editMessageText",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
    )
    .await;
    mount_post(
        &server,
        "deleteMessage",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": true
        })),
    )
    .await;
    let channel = channel_for(&server, &["*"]);

    let descriptor = ChannelAdapter::descriptor(&channel);
    assert_eq!(descriptor.id, "telegram");
    assert_eq!(descriptor.metadata["adapterMode"], "native-v2");
    assert_eq!(ChannelAdapter::proactive_target(&channel), None);
    assert!(ChannelAdapter::declared_capabilities(&channel).reactions);
    let mut configured_target = channel_for(&server, &["*"]);
    configured_target.chat_id = Some("  -10042  ".into());
    assert_eq!(
        ChannelAdapter::proactive_target(&configured_target).as_deref(),
        Some("-10042")
    );
    configured_target.chat_id = Some("   ".into());
    assert_eq!(ChannelAdapter::proactive_target(&configured_target), None);
    let status = ChannelAdapter::status(&channel).await.unwrap();
    assert_eq!(status.channel_id, "telegram");
    assert_eq!(status.health_state.as_deref(), Some("healthy"));

    let intent = tinychannels_bus::ChannelOutboundIntent {
        idempotency_key: "adapter-send".into(),
        channel_id: "telegram".into(),
        conversation_id: "-10042".into(),
        payload: tinychannels_bus::OutboundPayload::Text {
            text: "adapter text".into(),
        },
        ..Default::default()
    };
    let receipt = ChannelAdapter::send(&channel, intent).await.unwrap();
    assert_eq!(receipt.platform_message_ids, ["94"]);
    assert_eq!(receipt.idempotency_key.as_deref(), Some("adapter-send"));

    ChannelTyping::start_typing(&channel, "-10042")
        .await
        .unwrap();
    ChannelTyping::stop_typing(&channel, "-10042")
        .await
        .unwrap();
    LegacyChannelReaction::send_reaction(
        &channel,
        &tinychannels_bus::ChannelRef {
            id: "telegram".into(),
            account_id: None,
        },
        &tinychannels_bus::ConversationRef {
            id: "-10042".into(),
            ..Default::default()
        },
        "94",
        &tinychannels_bus::channel::ChannelReaction {
            value: "🔥".into(),
            ..Default::default()
        },
        false,
    )
    .await
    .unwrap();
    let edit = ChannelEdit::edit(&channel, &receipt, edit_intent("-10042"))
        .await
        .unwrap();
    assert_eq!(edit, receipt);
    ChannelDelete::delete(&channel, &receipt).await.unwrap();
}

#[tokio::test]
async fn structured_sends_map_location_venue_contact_buttons_poll_and_presentation() {
    use tinychannels_bus::ContentPart;
    use tinychannels_bus::OutboundContentPart;

    let cases = vec![
        (
            vec![OutboundContentPart::Content {
                part: ContentPart::Location {
                    latitude: 51.5,
                    longitude: -0.1,
                    label: None,
                    address: None,
                    accuracy_meters: Some(12.5),
                    live_until_unix_ms: None,
                },
            }],
            "sendLocation",
            serde_json::json!({
                "chat_id": "-10042",
                "latitude": 51.5,
                "longitude": -0.1,
                "horizontal_accuracy": 12.5,
                "message_thread_id": "18",
                "reply_parameters": {"message_id": 5}
            }),
            tinychannels_bus::channel::MessageReceiptPartKind::Card,
        ),
        (
            vec![OutboundContentPart::Content {
                part: ContentPart::Location {
                    latitude: 40.7,
                    longitude: -74.0,
                    label: Some("Office".into()),
                    address: Some("1 Main St".into()),
                    accuracy_meters: None,
                    live_until_unix_ms: None,
                },
            }],
            "sendVenue",
            serde_json::json!({
                "chat_id": "-10042",
                "latitude": 40.7,
                "longitude": -74.0,
                "title": "Office",
                "address": "1 Main St",
                "message_thread_id": "18",
                "reply_parameters": {"message_id": 5}
            }),
            tinychannels_bus::channel::MessageReceiptPartKind::Card,
        ),
        (
            vec![OutboundContentPart::Content {
                part: ContentPart::Contact {
                    display_name: "Ada Lovelace".into(),
                    phone_numbers: vec![tinychannels_bus::ContactMethod {
                        value: "+1 555 0100".into(),
                        label: Some("WORK".into()),
                    }],
                    emails: vec![tinychannels_bus::ContactMethod {
                        value: "ada@example.test".into(),
                        label: Some("INTERNET".into()),
                    }],
                    organization: Some("Analytical Engines".into()),
                },
            }],
            "sendContact",
            serde_json::json!({
                "chat_id": "-10042",
                "phone_number": "+1 555 0100",
                "first_name": "Ada",
                "last_name": "Lovelace",
                "vcard": "BEGIN:VCARD\nVERSION:3.0\nFN:Ada Lovelace\nTEL;TYPE=WORK:+1 555 0100\nEMAIL;TYPE=INTERNET:ada@example.test\nORG:Analytical Engines\nEND:VCARD",
                "message_thread_id": "18",
                "reply_parameters": {"message_id": 5}
            }),
            tinychannels_bus::channel::MessageReceiptPartKind::Card,
        ),
        (
            vec![
                OutboundContentPart::Content {
                    part: ContentPart::Text {
                        text: "Choose one".into(),
                        markdown_dialect: None,
                    },
                },
                OutboundContentPart::Content {
                    part: ContentPart::Buttons {
                        rows: vec![vec![tinychannels_bus::ButtonAction {
                            id: "choose-a".into(),
                            label: "Option A".into(),
                            style: None,
                            disabled: false,
                        }]],
                    },
                },
            ],
            "sendMessage",
            serde_json::json!({
                "chat_id": "-10042",
                "text": "Choose one",
                "parse_mode": "MarkdownV2",
                "reply_markup": {"inline_keyboard": [[{"text": "Option A", "callback_data": "choose-a"}]]},
                "message_thread_id": "18",
                "reply_parameters": {"message_id": 5}
            }),
            tinychannels_bus::channel::MessageReceiptPartKind::Text,
        ),
        (
            vec![OutboundContentPart::Content {
                part: ContentPart::Poll {
                    question: "Lunch?".into(),
                    options: vec!["Soup".into(), "Salad".into()],
                    multiple_choice: true,
                    closes_at_unix_ms: None,
                    provider_poll_id: None,
                },
            }],
            "sendPoll",
            serde_json::json!({
                "chat_id": "-10042",
                "question": "Lunch?",
                "options": [{"text": "Soup"}, {"text": "Salad"}],
                "is_anonymous": true,
                "type": "regular",
                "allows_multiple_answers": true,
                "message_thread_id": "18",
                "reply_parameters": {"message_id": 5}
            }),
            tinychannels_bus::channel::MessageReceiptPartKind::Poll,
        ),
        (
            vec![OutboundContentPart::Content {
                part: ContentPart::Presentation {
                    blocks: serde_json::json!([{"type": "heading", "text": "Status"}, "Ready"]),
                },
            }],
            "sendMessage",
            serde_json::json!({
                "chat_id": "-10042",
                "text": "Status\nReady",
                "parse_mode": "MarkdownV2",
                "message_thread_id": "18",
                "reply_parameters": {"message_id": 5}
            }),
            tinychannels_bus::channel::MessageReceiptPartKind::Text,
        ),
    ];

    for (index, (parts, method_name, expected, part_kind)) in cases.into_iter().enumerate() {
        let server = mock_api().await;
        mount_post(
            &server,
            method_name,
            ok_with(serde_json::json!({ "message_id": index as i64 + 301 })),
        )
        .await;
        let outcome = channel_for(&server, &["*"])
            .send_v2_with_receipt(&structured_message(parts))
            .await
            .unwrap();

        assert_eq!(outcome.error, None, "{method_name}");
        assert_eq!(
            outcome.receipt.parts[0].kind, part_kind,
            "{method_name} receipt kind"
        );
        let bodies = json_bodies(&server, method_name).await;
        assert_eq!(bodies, vec![expected], "{method_name} request");
    }
}

#[tokio::test]
async fn structured_sends_reject_invalid_bot_api_payloads_before_network_io() {
    use tinychannels_bus::{ButtonAction, ContactMethod, ContentPart, OutboundContentPart};

    let content = |part| OutboundContentPart::Content { part };
    let button = |id: &str, style, disabled| ButtonAction {
        id: id.into(),
        label: "Continue".into(),
        style,
        disabled,
    };
    let mut cases = vec![
        (
            "location coordinates",
            vec![content(ContentPart::Location {
                latitude: f64::NAN,
                longitude: 0.0,
                label: None,
                address: None,
                accuracy_meters: None,
                live_until_unix_ms: None,
            })],
        ),
        (
            "incomplete venue",
            vec![content(ContentPart::Location {
                latitude: 0.0,
                longitude: 0.0,
                label: Some("Office".into()),
                address: None,
                accuracy_meters: None,
                live_until_unix_ms: None,
            })],
        ),
        (
            "venue with live location metadata",
            vec![content(ContentPart::Location {
                latitude: 0.0,
                longitude: 0.0,
                label: Some("Office".into()),
                address: Some("1 Main St".into()),
                accuracy_meters: Some(1.0),
                live_until_unix_ms: None,
            })],
        ),
        (
            "invalid accuracy",
            vec![content(ContentPart::Location {
                latitude: 0.0,
                longitude: 0.0,
                label: None,
                address: None,
                accuracy_meters: Some(1501.0),
                live_until_unix_ms: None,
            })],
        ),
        (
            "expired live location",
            vec![content(ContentPart::Location {
                latitude: 0.0,
                longitude: 0.0,
                label: None,
                address: None,
                accuracy_meters: None,
                live_until_unix_ms: Some(0),
            })],
        ),
        (
            "contact without a phone",
            vec![content(ContentPart::Contact {
                display_name: "Ada Lovelace".into(),
                phone_numbers: vec![],
                emails: vec![],
                organization: None,
            })],
        ),
        (
            "contact with oversized phone",
            vec![content(ContentPart::Contact {
                display_name: "Ada Lovelace".into(),
                phone_numbers: vec![ContactMethod {
                    value: "1".repeat(257),
                    label: None,
                }],
                emails: vec![],
                organization: None,
            })],
        ),
        (
            "contact with oversized vcard",
            vec![content(ContentPart::Contact {
                display_name: "A".repeat(2050),
                phone_numbers: vec![ContactMethod {
                    value: "+1".into(),
                    label: None,
                }],
                emails: vec![],
                organization: None,
            })],
        ),
        (
            "buttons without text",
            vec![content(ContentPart::Buttons {
                rows: vec![vec![button("continue", None, false)]],
            })],
        ),
        (
            "disabled button",
            vec![
                content(ContentPart::Text {
                    text: "Choose".into(),
                    markdown_dialect: None,
                }),
                content(ContentPart::Buttons {
                    rows: vec![vec![button("continue", None, true)]],
                }),
            ],
        ),
        (
            "styled button",
            vec![
                content(ContentPart::Text {
                    text: "Choose".into(),
                    markdown_dialect: None,
                }),
                content(ContentPart::Buttons {
                    rows: vec![vec![button(
                        "continue",
                        Some(tinychannels_bus::ButtonStyle::Primary),
                        false,
                    )]],
                }),
            ],
        ),
        (
            "empty callback id",
            vec![
                content(ContentPart::Text {
                    text: "Choose".into(),
                    markdown_dialect: None,
                }),
                content(ContentPart::Buttons {
                    rows: vec![vec![button("", None, false)]],
                }),
            ],
        ),
        (
            "empty keyboard",
            vec![
                content(ContentPart::Text {
                    text: "Choose".into(),
                    markdown_dialect: None,
                }),
                content(ContentPart::Buttons { rows: vec![vec![]] }),
            ],
        ),
        (
            "poll with too few options",
            vec![content(ContentPart::Poll {
                question: "Lunch?".into(),
                options: vec!["Soup".into()],
                multiple_choice: false,
                closes_at_unix_ms: None,
                provider_poll_id: None,
            })],
        ),
        (
            "poll with blank option",
            vec![content(ContentPart::Poll {
                question: "Lunch?".into(),
                options: vec!["Soup".into(), "  ".into()],
                multiple_choice: false,
                closes_at_unix_ms: None,
                provider_poll_id: None,
            })],
        ),
        (
            "poll with invalid close time",
            vec![content(ContentPart::Poll {
                question: "Lunch?".into(),
                options: vec!["Soup".into(), "Salad".into()],
                multiple_choice: false,
                closes_at_unix_ms: Some(0),
                provider_poll_id: None,
            })],
        ),
        (
            "empty presentation",
            vec![content(ContentPart::Presentation {
                blocks: serde_json::json!([]),
            })],
        ),
        (
            "unsupported native content",
            vec![content(ContentPart::Native {
                provider: "other".into(),
                data: serde_json::json!({}),
            })],
        ),
    ];

    cases.push((
        "mixed structured content",
        vec![
            content(ContentPart::Location {
                latitude: 0.0,
                longitude: 0.0,
                label: None,
                address: None,
                accuracy_meters: None,
                live_until_unix_ms: None,
            }),
            content(ContentPart::Text {
                text: "extra".into(),
                markdown_dialect: None,
            }),
        ],
    ));

    let server = mock_api().await;
    for (label, parts) in cases {
        let message = structured_message(parts);
        assert!(
            channel_for(&server, &["*"])
                .send_v2_with_receipt(&message)
                .await
                .is_err(),
            "accepted invalid structured payload: {label}"
        );
    }

    let mut missing_conversation = structured_message(vec![content(ContentPart::Location {
        latitude: 0.0,
        longitude: 0.0,
        label: None,
        address: None,
        accuracy_meters: None,
        live_until_unix_ms: None,
    })]);
    missing_conversation.conversation_ref.as_mut().unwrap().id = "  ".into();
    assert!(
        channel_for(&server, &["*"])
            .send_v2_with_receipt(&missing_conversation)
            .await
            .is_err(),
        "accepted a structured send without a conversation"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn structured_send_marks_uncertain_bot_api_replies_as_nonretryable() {
    use tinychannels_bus::{ContentPart, OutboundContentPart};

    let responses = [
        ResponseTemplate::new(200).set_body_string("not json"),
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": false })),
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true, "result": {} })),
        ResponseTemplate::new(502).set_body_json(serde_json::json!({ "ok": true })),
    ];
    for response in responses {
        let server = mock_api().await;
        mount_post(&server, "sendLocation", response).await;
        let message = structured_message(vec![OutboundContentPart::Content {
            part: ContentPart::Location {
                latitude: 51.5,
                longitude: -0.1,
                label: None,
                address: None,
                accuracy_meters: None,
                live_until_unix_ms: None,
            },
        }]);

        let error = channel_for(&server, &["*"])
            .send_v2_with_receipt(&message)
            .await
            .unwrap_err();
        let typed = typed_send_error(error);
        assert!(typed.sent_before_error);
        assert!(!typed.retryable);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    let server = mock_api().await;
    mount_post(
        &server,
        "sendLocation",
        ok_with(serde_json::json!({ "message_id": 812 })),
    )
    .await;
    let mut message = structured_message(vec![OutboundContentPart::Content {
        part: ContentPart::Location {
            latitude: 51.5,
            longitude: -0.1,
            label: None,
            address: None,
            accuracy_meters: None,
            live_until_unix_ms: None,
        },
    }]);
    message.channel_ref = None;
    let outcome = channel_for(&server, &["*"])
        .send_v2_with_receipt(&message)
        .await
        .unwrap();
    assert_eq!(outcome.receipt.channel.unwrap().id, "telegram");
}

#[tokio::test]
async fn callback_ingress_preserves_actor_scope_and_authorizes_only_numeric_id_or_username() {
    let update = serde_json::json!({
        "update_id": 9001,
        "callback_query": {
            "id": "query-4",
            "data": "approve-action",
            "from": {"id": 77, "username": "operator", "first_name": "Display", "is_bot": false},
            "message": {
                "message_id": 81,
                "message_thread_id": 18,
                "chat": {"id": -10042, "type": "supergroup"}
            }
        }
    });
    let ch = TelegramChannel::new("token".into(), vec!["77".into()], false);

    assert!(ch.callback_is_allowed(&update));
    let event = TelegramChannel::callback_event(&update).unwrap();
    assert_eq!(event.event_id, "telegram:update:9001:callback:query-4");
    assert_eq!(event.source_event_id.as_deref(), Some("9001"));
    assert_eq!(event.conversation.as_ref().unwrap().id, "-10042");
    assert_eq!(
        event.conversation.as_ref().unwrap().topic_id.as_deref(),
        Some("18")
    );
    match event.event {
        tinychannels_bus::ChannelEventKind::ButtonCallback {
            query_id,
            action_id,
            provider_message_id,
            actor,
        } => {
            assert_eq!(query_id, "query-4");
            assert_eq!(action_id, "approve-action");
            assert_eq!(provider_message_id.as_deref(), Some("81"));
            assert_eq!(actor.id, "77");
            assert_eq!(actor.name.as_deref(), Some("operator"));
            assert!(!actor.is_bot);
        }
        other => panic!("unexpected callback event {other:?}"),
    }

    let username_only = TelegramChannel::new("token".into(), vec!["operator".into()], false);
    assert!(username_only.callback_is_allowed(&update));
    let display_name_only = TelegramChannel::new("token".into(), vec!["Display".into()], false);
    assert!(!display_name_only.callback_is_allowed(&update));
    assert!(!ch.callback_is_allowed(&serde_json::json!({"message": {}})));

    let mut invalid_action = update.clone();
    invalid_action["callback_query"]["data"] = serde_json::json!("");
    assert!(TelegramChannel::callback_event(&invalid_action).is_none());
    invalid_action["callback_query"]["data"] = serde_json::json!("x".repeat(65));
    assert!(TelegramChannel::callback_event(&invalid_action).is_none());
    assert!(TelegramChannel::callback_event(&serde_json::json!({})).is_none());

    let server = mock_api().await;
    mount_post(
        &server,
        "answerCallbackQuery",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": false,
            "description": "query already expired"
        })),
    )
    .await;
    channel_for(&server, &["*"])
        .acknowledge_callback("query-4")
        .await;
    assert_eq!(
        json_bodies(&server, "answerCallbackQuery").await,
        vec![serde_json::json!({"callback_query_id": "query-4"})]
    );
}

#[test]
fn callback_events_preserve_optional_identity_and_map_conversation_kinds() {
    for (chat_type, expected_kind) in [
        ("private", tinychannels_bus::ConversationKind::Dm),
        ("group", tinychannels_bus::ConversationKind::Group),
        ("supergroup", tinychannels_bus::ConversationKind::Group),
        ("channel", tinychannels_bus::ConversationKind::Channel),
        ("unknown", tinychannels_bus::ConversationKind::Unknown),
    ] {
        let update = serde_json::json!({
            "callback_query": {
                "id": "query",
                "data": "action",
                "from": {"id": 77, "first_name": "Display"},
                "message": {
                    "message_id": 81,
                    "chat": {"id": -10042, "type": chat_type}
                }
            }
        });

        let event = TelegramChannel::callback_event(&update).unwrap();
        assert_eq!(event.event_id, "telegram:callback:query");
        assert_eq!(event.source_event_id, None);
        assert_eq!(event.conversation.as_ref().unwrap().kind, expected_kind);
        assert_eq!(event.conversation.as_ref().unwrap().topic_id, None);
        match event.event {
            tinychannels_bus::ChannelEventKind::ButtonCallback { actor, .. } => {
                assert_eq!(actor.name.as_deref(), Some("Display"));
                assert!(!actor.is_bot);
            }
            other => panic!("unexpected callback event {other:?}"),
        }
    }

    let mut update = serde_json::json!({
        "update_id": 9002,
        "callback_query": {
            "id": "query",
            "data": "action",
            "from": {"id": 77, "username": "operator", "is_bot": true},
            "message": {
                "message_id": 81,
                "message_thread_id": 18,
                "chat": {"id": -10042, "type": "private"}
            }
        }
    });
    let event = TelegramChannel::callback_event(&update).unwrap();
    assert_eq!(event.event_id, "telegram:update:9002:callback:query");
    assert_eq!(event.source_event_id.as_deref(), Some("9002"));
    match event.event {
        tinychannels_bus::ChannelEventKind::ButtonCallback { actor, .. } => {
            assert_eq!(actor.name.as_deref(), Some("operator"));
            assert!(actor.is_bot);
        }
        other => panic!("unexpected callback event {other:?}"),
    }

    for missing in [
        serde_json::json!({"callback_query": {"id": "q"}}),
        serde_json::json!({"callback_query": {"id": "q", "data": "a"}}),
    ] {
        assert!(TelegramChannel::callback_event(&missing).is_none());
    }
    update["callback_query"]["from"]["id"] = serde_json::json!("77");
    assert!(TelegramChannel::callback_event(&update).is_none());
}

#[test]
fn typed_send_errors_preserve_dispatch_certainty_and_provider_classification() {
    assert_eq!(
        TelegramAttemptedError.to_string(),
        "Telegram Bot API operation failed after dispatch"
    );
    assert_eq!(
        TelegramUnsupportedError.to_string(),
        "Telegram Bot API does not support this content operation"
    );
    let forbidden = typed_send_error(anyhow::Error::new(TelegramPreDispatchError));
    assert_eq!(forbidden.kind, crate::channel::SendErrorKind::Forbidden);
    assert!(!forbidden.sent_before_error);
    assert!(!forbidden.retryable);

    let bad_format = typed_send_error(anyhow::Error::new(TelegramBadFormatError));
    assert_eq!(bad_format.kind, crate::channel::SendErrorKind::BadFormat);
    assert!(!bad_format.sent_before_error);

    let unsupported = typed_send_error(anyhow::Error::new(TelegramUnsupportedError));
    assert_eq!(unsupported.kind, crate::channel::SendErrorKind::BadFormat);
    assert!(unsupported.unsupported);
    assert!(!unsupported.sent_before_error);

    let partial_error = crate::channel::ChannelSendError {
        kind: crate::channel::SendErrorKind::RateLimited,
        message: "second chunk rejected".into(),
        retryable: true,
        sent_before_error: true,
        ..Default::default()
    };
    let partial = crate::providers::telegram::channel_send::PartialTextSendError {
        message_ids: vec!["first".into()],
        sent_before_error: true,
        error: partial_error.clone(),
    };
    assert_eq!(
        typed_send_error(anyhow::Error::new(partial)).message,
        partial_error.message
    );

    let uncertain = typed_send_error(anyhow::anyhow!("transport failed"));
    assert_eq!(uncertain.kind, crate::channel::SendErrorKind::Unknown);
    assert!(uncertain.sent_before_error);
    assert!(!uncertain.retryable);
}

#[test]
fn typed_interaction_errors_distinguish_preflight_from_provider_failure() {
    let preflight = typed_interaction_error(anyhow::Error::new(TelegramPreDispatchError), "edit");
    assert_eq!(preflight.kind, crate::channel::SendErrorKind::Forbidden);
    assert_eq!(
        preflight.message,
        "Telegram edit was rejected before provider dispatch"
    );
    assert!(!preflight.sent_before_error);
    assert!(!preflight.retryable);

    let uncertain = typed_interaction_error(anyhow::anyhow!("transport failed"), "edit");
    assert_eq!(uncertain.kind, crate::channel::SendErrorKind::Unknown);
    assert_eq!(uncertain.message, "Telegram edit outcome is uncertain");
    assert!(uncertain.sent_before_error);
    assert!(!uncertain.retryable);
}

#[test]
fn presentation_projection_renders_nested_text_and_approval_actions() {
    let blocks = serde_json::json!([
        "Opening line",
        {
            "type": "heading",
            "title": "Title",
            "heading": "Heading",
            "text": "Body",
            "caption": "Caption",
            "description": "Description"
        },
        {"type": "paragraph", "text": "  "},
        ["Nested line", {"text": "Last line"}]
    ]);
    let (text, keyboard) = presentation_content(&blocks).unwrap();
    assert_eq!(
        text,
        "Opening line\nTitle\nHeading\nBody\nCaption\nDescription\nNested line\nLast line"
    );
    assert_eq!(keyboard, None);

    let approval = serde_json::json!({
        "kind": "approval",
        "id": "approval-42",
        "prompt": "Run this command?",
        "choices": ["Allow once", "Deny"]
    });
    let (prompt, keyboard) = presentation_content(&approval).unwrap();
    assert_eq!(prompt, "Run this command?");
    assert_eq!(
        keyboard.unwrap(),
        serde_json::json!({
            "inline_keyboard": [
                [{"text": "Allow once", "callback_data": tinychannels_bus::channel::approval_action_id("approval-42", 0).unwrap()}],
                [{"text": "Deny", "callback_data": tinychannels_bus::channel::approval_action_id("approval-42", 1).unwrap()}]
            ]
        })
    );
}

#[test]
fn presentation_projection_rejects_unsupported_fields_types_and_empty_content() {
    for value in [
        serde_json::json!({"kind": "approval", "id": "x", "prompt": "p", "choices": ["yes"], "extra": true}),
        serde_json::json!({"kind": "approval", "id": "", "prompt": "p", "choices": ["yes"]}),
        serde_json::json!({"kind": "approval", "id": "x", "prompt": "  ", "choices": ["yes"]}),
        serde_json::json!({"kind": "approval", "id": "x", "prompt": "p", "choices": []}),
        serde_json::json!({"kind": "approval", "id": "x", "prompt": "p", "choices": [""]}),
        serde_json::json!({"type": "image", "text": "not text"}),
        serde_json::json!({"type": 1, "text": "wrong type"}),
        serde_json::json!({"unknown": "field"}),
        serde_json::json!(["", {"text": 1}]),
        serde_json::json!([false]),
        serde_json::json!("   "),
    ] {
        assert!(presentation_content(&value).is_err(), "accepted {value}");
    }
}

#[test]
fn vcard_projection_escapes_content_and_only_accepts_safe_type_labels() {
    assert_eq!(vcard_escape("A,B;C\\D\nE\rF"), "A\\,B\\;C\\\\D\\nEF");
    assert_eq!(safe_vcard_type(Some(" work "), "CELL"), "work");
    assert_eq!(safe_vcard_type(Some("INTERNET"), "CELL"), "INTERNET");
    assert_eq!(safe_vcard_type(Some("X-INJECT:EVIL"), "CELL"), "CELL");
    assert_eq!(safe_vcard_type(None, "INTERNET"), "INTERNET");
}

#[tokio::test]
async fn reaction_removal_sends_empty_reaction_and_rejects_failed_receipts() {
    let server = mock_api().await;
    mount_post(
        &server,
        "setMessageReaction",
        ok_with(serde_json::Value::Null),
    )
    .await;
    let ch = channel_for(&server, &["*"]);
    let conversation = tinychannels_bus::ConversationRef {
        id: "-10042".into(),
        ..Default::default()
    };

    ch.send_reaction_v2(
        &conversation,
        "81",
        &tinychannels_bus::channel::ChannelReaction::default(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        json_bodies(&server, "setMessageReaction").await,
        vec![serde_json::json!({
            "chat_id": "-10042",
            "message_id": 81,
            "reaction": [],
            "is_big": false,
        })]
    );

    let rejected = mock_api().await;
    mount_post(
        &rejected,
        "setMessageReaction",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": false,
            "description": "message cannot be changed"
        })),
    )
    .await;
    let ch = channel_for(&rejected, &["*"]);
    let error = ch
        .send_reaction_v2(
            &conversation,
            "81",
            &tinychannels_bus::channel::ChannelReaction::default(),
            true,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("reaction removal failed"));
}

#[tokio::test]
async fn reaction_add_rejects_invalid_message_and_empty_reaction_before_io() {
    let server = mock_api().await;
    let channel = channel_for(&server, &["*"]);
    let conversation = tinychannels_bus::channel::ConversationRef {
        id: "-10042".into(),
        ..Default::default()
    };

    let invalid_message = channel
        .send_reaction_v2(
            &conversation,
            "message-x",
            &tinychannels_bus::channel::ChannelReaction {
                value: "👍".into(),
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap_err();
    assert!(
        invalid_message
            .downcast_ref::<TelegramPreDispatchError>()
            .is_some()
    );

    let empty_reaction = channel
        .send_reaction_v2(
            &conversation,
            "81",
            &tinychannels_bus::channel::ChannelReaction::default(),
            false,
        )
        .await
        .unwrap_err();
    assert!(
        empty_reaction
            .downcast_ref::<TelegramPreDispatchError>()
            .is_some()
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn edit_message_uses_receipt_identity_and_preserves_receipt() {
    let server = mock_api().await;
    mount_post(
        &server,
        "editMessageText",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
    )
    .await;
    let ch = channel_for(&server, &["*"]);
    let receipt = telegram_receipt(Some("81"));

    let updated = ch
        .edit_message_v2(&receipt, &edit_intent("-10042"))
        .await
        .unwrap();

    assert_eq!(updated, receipt);
    assert_eq!(
        json_bodies(&server, "editMessageText").await,
        vec![serde_json::json!({
            "chat_id": "-10042",
            "message_id": 81,
            "text": "updated  *text*",
            "parse_mode": "MarkdownV2",
        })]
    );
}

#[tokio::test]
async fn edit_and_delete_reject_mismatched_or_incomplete_receipts_before_io() {
    let server = mock_api().await;
    let ch = channel_for(&server, &["*"]);
    let mut wrong_channel = telegram_receipt(Some("81"));
    wrong_channel.channel.as_mut().unwrap().id = "discord".into();
    assert!(
        ch.edit_message_v2(&wrong_channel, &edit_intent("-10042"))
            .await
            .is_err()
    );
    assert!(
        ch.edit_message_v2(&telegram_receipt(None), &edit_intent("-10042"))
            .await
            .is_err()
    );

    let mut mismatched_intent = edit_intent("-10042");
    mismatched_intent.conversation_id = "different-chat".into();
    assert!(
        ch.edit_message_v2(&telegram_receipt(Some("81")), &mismatched_intent)
            .await
            .is_err()
    );
    assert!(ch.delete_message_v2(&telegram_receipt(None)).await.is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn native_interaction_adapters_preserve_uncertain_provider_errors() {
    let server = mock_api().await;
    for method_name in ["setMessageReaction", "editMessageText", "deleteMessage"] {
        mount_post(
            &server,
            method_name,
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "description": "provider rejected the operation"
            })),
        )
        .await;
    }
    let channel = channel_for(&server, &["*"]);
    let conversation = tinychannels_bus::ConversationRef {
        id: "-10042".into(),
        ..Default::default()
    };
    let channel_ref = tinychannels_bus::ChannelRef {
        id: "telegram".into(),
        account_id: None,
    };

    let reaction = channel
        .send_reaction(
            &channel_ref,
            &conversation,
            "81",
            &tinychannels_bus::ChannelReaction::default(),
            true,
        )
        .await
        .unwrap_err();
    let edit = channel
        .edit(&telegram_receipt(Some("81")), edit_intent("-10042"))
        .await
        .unwrap_err();
    let delete = channel
        .delete(&telegram_receipt(Some("81")))
        .await
        .unwrap_err();

    for error in [reaction, edit, delete] {
        assert!(error.sent_before_error);
        assert!(!error.retryable);
        assert_eq!(
            error.kind,
            tinychannels_bus::channel::SendErrorKind::Unknown
        );
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn delete_message_requires_telegram_success_and_true_result() {
    let rejected = mock_api().await;
    mount_post(
        &rejected,
        "deleteMessage",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": false,
            "description": "message is too old"
        })),
    )
    .await;
    let ch = channel_for(&rejected, &["*"]);
    assert!(
        ch.delete_message_v2(&telegram_receipt(Some("81")))
            .await
            .unwrap_err()
            .to_string()
            .contains("deleteMessage failed")
    );

    let server = mock_api().await;
    mount_post(
        &server,
        "deleteMessage",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": true
        })),
    )
    .await;
    channel_for(&server, &["*"])
        .delete_message_v2(&telegram_receipt(Some("81")))
        .await
        .unwrap();
    assert_eq!(
        json_bodies(&server, "deleteMessage").await,
        vec![serde_json::json!({"chat_id": "-10042", "message_id": 81})]
    );
}

#[test]
#[allow(
    deprecated,
    reason = "the normalization fallback is specifically for the v21 provider adapter"
)]
fn inbound_envelope_keeps_chat_channel_and_legacy_identity_fallbacks() {
    let legacy = ChannelMessage {
        id: "telegram_-100_7".into(),
        sender: "legacy-sender".into(),
        sender_name: Some("Legacy display".into()),
        reply_target: "-100:topic-4".into(),
        content: "legacy body".into(),
        channel: "telegram".into(),
        ..Default::default()
    };
    let channel_update = serde_json::json!({
        "message": {
            "message_id": 7,
            "chat": {"type":"channel"},
            "sender_chat": {"id": 9001},
            "reply_to_message": {"message_id": 3}
        }
    });
    let channel = TelegramChannel::inbound_envelope_from_update(&channel_update, &legacy, None);
    assert_eq!(
        channel.conversation.kind,
        tinychannels_bus::ConversationKind::Channel
    );
    assert_eq!(channel.conversation.id, "-100");
    assert_eq!(channel.sender.id, "9001");
    assert_eq!(channel.reply_to_id.as_deref(), Some("3"));
    assert_eq!(channel.sender.name.as_deref(), Some("Legacy display"));

    let unknown_update = serde_json::json!({
        "message": {
            "chat": {"id": 17, "type":"unknown"},
            "from": {}
        }
    });
    let unknown = TelegramChannel::inbound_envelope_from_update(&unknown_update, &legacy, None);
    assert_eq!(
        unknown.conversation.kind,
        tinychannels_bus::ConversationKind::Unknown
    );
    assert_eq!(unknown.conversation.id, "17");
    assert_eq!(unknown.sender.id, "legacy-sender");
}

#[tokio::test]
async fn typed_send_keeps_delivered_chunk_ids_when_later_chunk_fails() {
    let server = mock_api().await;
    Mock::given(method("POST"))
        .and(path(api_path("sendMessage")))
        .respond_with(ok_with(serde_json::json!({ "message_id": 321 })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_post(
        &server,
        "sendMessage",
        ResponseTemplate::new(400).set_body_string("rejected"),
    )
    .await;
    let ch = channel_for(&server, &["*"]);
    let mut message = typed_text("idem", "x".repeat(TELEGRAM_MAX_MESSAGE_LENGTH + 50));
    message
        .parts
        .push(tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Text {
                text: "tail".into(),
                markdown_dialect: None,
            },
        });
    let outcome = ch.send_v2_with_receipt(&message).await.unwrap();
    assert_eq!(outcome.receipt.platform_message_ids, ["321"]);
    assert_eq!(outcome.receipt.parts.len(), 1);
    assert!(outcome.error.as_ref().is_some_and(|error| !error.retryable));
    assert!(outcome.error.as_ref().unwrap().sent_before_error);
    assert_eq!(
        outcome.error.as_ref().unwrap().continuation_message_ids,
        ["321"]
    );
    let public_error = serde_json::to_string(&outcome.error).unwrap();
    assert!(
        !public_error.contains(TOKEN),
        "bot token must not enter structured errors"
    );
}

#[tokio::test]
async fn typed_send_without_provider_message_id_is_ambiguous_and_not_retried() {
    let server = mock_api().await;
    mount_post(&server, "sendMessage", ok_with(serde_json::json!({}))).await;
    let ch = channel_for(&server, &["*"]);
    let outcome = ch
        .send_v2_with_receipt(&typed_text("missing-id", "hello".into()))
        .await
        .unwrap();
    let error = outcome.error.expect("missing ID cannot report success");
    assert!(
        error.sent_before_error,
        "Telegram accepted the request, so retry safety is unknown"
    );
    assert!(outcome.receipt.platform_message_ids.is_empty());
    assert!(error.message.contains("usable message ID"));
    assert!(!error.message.contains(TOKEN));
    assert_eq!(json_bodies(&server, "sendMessage").await.len(), 1);
}

#[tokio::test]
async fn typed_send_connection_refusal_is_definitely_not_sent_and_sanitized() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut ch = TelegramChannel::new(TOKEN.into(), vec!["*".into()], false);
    ch.api_base = format!("http://{address}");
    let outcome = ch
        .send_v2_with_receipt(&typed_text("transport-error", "hello".into()))
        .await
        .unwrap();
    let error = outcome
        .error
        .expect("transport failure must not become success");
    assert!(
        !error.sent_before_error,
        "connection refusal precedes dispatch"
    );
    assert!(error.message.contains("request failed"));
    assert!(!error.message.contains(TOKEN));
    assert!(!serde_json::to_string(&error).unwrap().contains(TOKEN));
}

#[tokio::test]
async fn accepted_request_disconnect_remains_ambiguous() {
    use tokio::io::AsyncReadExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let accepted = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = socket.read(&mut buffer).await.unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or_default();
            if bytes.len() >= header_end + 4 + content_length {
                return bytes;
            }
        }
        bytes
    });
    let mut ch = TelegramChannel::new(TOKEN.into(), vec!["*".into()], false);
    ch.api_base = format!("http://{address}");

    let outcome = ch
        .send_v2_with_receipt(&typed_text("disconnected", "hello".into()))
        .await
        .unwrap();
    let request = accepted.await.unwrap();
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap();
    let body = &request[header_end + 4..];
    assert_eq!(body.len(), content_length);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(body).unwrap()["text"],
        "hello"
    );
    let error = outcome.error.expect("server accepted then disconnected");
    assert!(error.sent_before_error);
    assert!(outcome.receipt.platform_message_ids.is_empty());
}

#[tokio::test]
async fn fallback_connection_refusal_after_markdown_rejection_is_not_sent() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let first_request = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = socket.read(&mut buffer).await.unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or_default();
            if bytes.len() >= header_end + 4 + content_length {
                let body = br#"{"description":"Bad Request: can't parse entities"}"#;
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                socket.write_all(body).await.unwrap();
                return bytes;
            }
        }
        bytes
    });
    let mut ch = TelegramChannel::new(TOKEN.into(), vec!["*".into()], false);
    ch.api_base = format!("http://{address}");

    let outcome = ch
        .send_v2_with_receipt(&typed_text("fallback-refused", "hello".into()))
        .await
        .unwrap();
    let request = first_request.await.unwrap();
    assert!(request.windows(10).any(|window| window == b"parse_mode"));
    let error = outcome.error.expect("fallback connection is refused");
    assert!(!error.sent_before_error);
    assert!(outcome.receipt.platform_message_ids.is_empty());
}

#[tokio::test]
async fn update_draft_edits_message_and_records_edit_time() {
    let server = mock_api().await;
    mount_post(&server, "editMessageText", ok()).await;
    let ch = streaming(&server);

    ch.update_draft_provider("100:3", "12", "partial")
        .await
        .unwrap();

    assert_eq!(
        json_bodies(&server, "editMessageText").await,
        vec![serde_json::json!({ "chat_id": "100", "message_id": 12, "text": "partial" })]
    );
    assert!(ch.last_draft_edit.lock().contains_key("100"));
}

#[tokio::test]
async fn update_draft_truncates_oversized_text_to_limit() {
    let server = mock_api().await;
    mount_post(&server, "editMessageText", ok()).await;
    let ch = streaming(&server);
    let text = "é".repeat(TELEGRAM_MAX_MESSAGE_LENGTH);

    ch.update_draft_provider("100", "12", &text).await.unwrap();

    let sent = json_bodies(&server, "editMessageText").await[0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(sent.len() <= TELEGRAM_MAX_MESSAGE_LENGTH);
    assert_eq!(
        sent.len(),
        TELEGRAM_MAX_MESSAGE_LENGTH,
        "two-byte chars fill the limit"
    );
}

#[tokio::test]
async fn update_draft_rejection_is_swallowed_and_not_recorded() {
    let server = mock_api().await;
    mount_post(
        &server,
        "editMessageText",
        ResponseTemplate::new(400).set_body_string("message is not modified"),
    )
    .await;
    let ch = streaming(&server);

    ch.update_draft_provider("100", "12", "same").await.unwrap();
    assert!(!ch.last_draft_edit.lock().contains_key("100"));
}

#[tokio::test]
async fn update_draft_invalid_message_id_skips_network() {
    let server = mock_api().await;
    let ch = streaming(&server);
    ch.update_draft_provider("100", "abc", "text")
        .await
        .unwrap();
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn finalize_draft_edits_with_markdown_and_clears_rate_limit() {
    let server = mock_api().await;
    mount_post(&server, "editMessageText", ok()).await;
    let ch = streaming(&server);
    ch.last_draft_edit
        .lock()
        .insert("100".into(), std::time::Instant::now());

    ch.finalize_draft_provider("100", "8", "done <tool_call>{}</tool_call>", None)
        .await
        .unwrap();

    let bodies = json_bodies(&server, "editMessageText").await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["parse_mode"], "MarkdownV2");
    assert_eq!(bodies[0]["text"], "done");
    assert!(ch.last_draft_edit.lock().is_empty());
    assert!(json_bodies(&server, "sendMessage").await.is_empty());
}

#[tokio::test]
async fn finalize_draft_retries_plain_edit_when_markdown_rejected() {
    let server = mock_api().await;
    Mock::given(method("POST"))
        .and(path(api_path("editMessageText")))
        .and(body_partial_json(
            serde_json::json!({ "parse_mode": "MarkdownV2" }),
        ))
        .respond_with(ResponseTemplate::new(400))
        .with_priority(1)
        .mount(&server)
        .await;
    mount_post(&server, "editMessageText", ok()).await;
    let ch = streaming(&server);

    ch.finalize_draft_provider("100", "8", "final *text", None)
        .await
        .unwrap();

    let bodies = json_bodies(&server, "editMessageText").await;
    assert_eq!(bodies.len(), 2);
    assert!(bodies[1].get("parse_mode").is_none());
    assert!(json_bodies(&server, "sendMessage").await.is_empty());
}

#[tokio::test]
async fn finalize_draft_falls_back_to_new_message_when_edits_fail() {
    let server = mock_api().await;
    mount_post(&server, "editMessageText", ResponseTemplate::new(400)).await;
    mount_post(&server, "sendMessage", ok()).await;
    let ch = streaming(&server);

    ch.finalize_draft_provider("100:4", "8", "final", Some("77"))
        .await
        .unwrap();

    assert_eq!(json_bodies(&server, "editMessageText").await.len(), 2);
    let sent = json_bodies(&server, "sendMessage").await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["text"], "final");
    assert_eq!(sent[0]["message_thread_id"], "4");
    assert_eq!(sent[0]["reply_to_message_id"], 77);
}

#[tokio::test]
async fn finalize_draft_short_text_with_invalid_id_sends_new_message() {
    let server = mock_api().await;
    mount_post(&server, "sendMessage", ok()).await;
    let ch = streaming(&server);

    ch.finalize_draft_provider("100", "nope", "final", None)
        .await
        .unwrap();

    assert!(json_bodies(&server, "editMessageText").await.is_empty());
    assert_eq!(sent_texts(&server).await, vec!["final".to_string()]);
}

#[tokio::test]
async fn finalize_draft_oversized_text_deletes_draft_and_sends_chunks() {
    let server = mock_api().await;
    mount_post(&server, "deleteMessage", ok_with(serde_json::json!(true))).await;
    mount_post(&server, "sendMessage", ok()).await;
    let ch = streaming(&server);
    let text = "y".repeat(TELEGRAM_MAX_MESSAGE_LENGTH + 1);

    ch.finalize_draft_provider("100", "8", &text, None)
        .await
        .unwrap();

    assert_eq!(
        json_bodies(&server, "deleteMessage").await,
        vec![serde_json::json!({ "chat_id": "100", "message_id": 8 })]
    );
    assert_eq!(json_bodies(&server, "sendMessage").await.len(), 2);
}

#[tokio::test]
async fn finalize_draft_oversized_text_with_invalid_id_sends_chunks_only() {
    let server = mock_api().await;
    mount_post(&server, "sendMessage", ok()).await;
    let ch = streaming(&server);
    let text = "y".repeat(TELEGRAM_MAX_MESSAGE_LENGTH + 1);

    ch.finalize_draft_provider("100", "bad", &text, None)
        .await
        .unwrap();

    assert!(json_bodies(&server, "deleteMessage").await.is_empty());
    assert_eq!(json_bodies(&server, "sendMessage").await.len(), 2);
}

#[tokio::test]
async fn health_check_reflects_get_me_status() {
    let server = mock_api().await;
    Mock::given(method("GET"))
        .and(path(api_path("getMe")))
        .respond_with(ok_with(serde_json::json!({ "username": "b" })))
        .mount(&server)
        .await;
    let ch = channel_for(&server, &["*"]);
    assert!(ChannelAdapter::status(&ch).await.is_ok());

    let failing = mock_api().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&failing)
        .await;
    assert_eq!(
        ChannelAdapter::status(&channel_for(&failing, &["*"]))
            .await
            .unwrap()
            .health_state
            .as_deref(),
        Some("unhealthy")
    );

    let mut dead = TelegramChannel::new("tok".into(), vec!["*".into()], false);
    dead.api_base = dead_base();
    assert_eq!(
        ChannelAdapter::status(&dead)
            .await
            .unwrap()
            .health_state
            .as_deref(),
        Some("unhealthy")
    );
}

#[tokio::test]
async fn start_typing_sends_immediately_and_reuses_task_for_same_recipient() {
    let server = mock_api().await;
    mount_post(&server, "sendChatAction", ok_with(serde_json::json!(true))).await;
    let ch = channel_for(&server, &["*"]);

    ch.start_typing("100:6").await.unwrap();
    assert_eq!(json_bodies(&server, "sendChatAction").await.len(), 1);
    ch.start_typing("100:6").await.unwrap();
    let bodies = json_bodies(&server, "sendChatAction").await;
    assert_eq!(
        bodies.len(),
        2,
        "each v2 typing call is one provider action"
    );
    assert!(bodies.iter().all(|b| b["chat_id"] == "100"
        && b["action"] == "typing"
        && b["message_thread_id"] == "6"));

    ch.stop_typing("100:6").await.unwrap();
}

#[tokio::test]
async fn typing_refresh_reports_acceptance_rejection_and_transport_error() {
    let server = mock_api().await;
    Mock::given(method("POST"))
        .and(path("/accept"))
        .respond_with(ok_with(serde_json::json!(true)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/reject"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": false })))
        .mount(&server)
        .await;
    let client = reqwest::Client::new();
    let body = TelegramChannel::typing_body_for_recipient("100");

    let accept = format!("{}/accept", server.uri());
    let reject = format!("{}/reject", server.uri());
    let dead = format!("{}/x", dead_base());
    assert!(TelegramChannel::refresh_typing_once(&client, &accept, &body, "100").await);
    assert!(!TelegramChannel::refresh_typing_once(&client, &reject, &body, "100").await);
    assert!(!TelegramChannel::refresh_typing_once(&client, &dead, &body, "100").await);

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 2);
    let sent: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(
        sent,
        serde_json::json!({ "chat_id": "100", "action": "typing" })
    );
}

#[test]
fn telegram_advertises_reaction_support() {
    let ch = TelegramChannel::new("tok".into(), vec!["*".into()], false);
    assert!(
        ch.declared_capabilities()
            .actions
            .supported
            .iter()
            .any(|action| action == "react")
    );
}

fn fast_backoff(mut ch: TelegramChannel) -> TelegramChannel {
    ch.poll_error_backoff = Duration::from_millis(1);
    ch.poll_conflict_backoff = Duration::from_millis(1);
    ch
}

/// Mount a one-shot `getUpdates` response, consumed in mount order.
async fn mount_get_updates_once(server: &MockServer, priority: u8, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(api_path("getUpdates")))
        .respond_with(response)
        .up_to_n_times(1)
        .with_priority(priority)
        .mount(server)
        .await;
}

fn updates(result: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true, "result": result }))
}

#[test]
fn telegram_v2_ingress_preserves_plain_and_forum_addresses_without_legacy_topic_guessing() {
    let legacy = ChannelMessage {
        id: "telegram_100_5".into(),
        sender: "alice".into(),
        reply_target: "100:88".into(),
        content: "message".into(),
        channel: "telegram".into(),
        timestamp: 12,
        thread_ts: Some("5".into()),
        ..Default::default()
    };
    let plain = TelegramChannel::inbound_envelope_from_update(
        &serde_json::json!({"message":{"message_id":5,"date":12,"chat":{"id":100,"type":"private"},"from":{"id":7}}}),
        &legacy,
        None,
    );
    assert_eq!(plain.message_id, "telegram_100_5");
    assert_eq!(
        plain.conversation.kind,
        tinychannels_bus::ConversationKind::Dm
    );
    assert_eq!(plain.conversation.id, "100");
    assert_eq!(plain.conversation.topic_id, None);
    assert_eq!(plain.conversation.thread_id, None);
    assert_eq!(plain.sender.id, "7");
    assert_eq!(plain.raw.as_ref().unwrap()["providerMessageId"], "5");

    let forum = TelegramChannel::inbound_envelope_from_update(
        &serde_json::json!({"message":{"message_id":5,"date":12,"message_thread_id":88,"chat":{"id":100,"type":"supergroup"},"from":{"id":7}}}),
        &legacy,
        None,
    );
    assert_eq!(
        forum.conversation.kind,
        tinychannels_bus::ConversationKind::Group
    );
    assert_eq!(forum.conversation.id, "100");
    assert_eq!(forum.conversation.topic_id.as_deref(), Some("88"));
    assert_eq!(forum.conversation.thread_id, None);
}

#[tokio::test]
async fn listen_recovers_from_poll_errors_and_dispatches_updates() {
    let server = mock_api().await;
    // Poll sequence: unparseable body, generic API error, 409 without webhook,
    // 409 webhook conflict that deleteWebhook clears, then a real batch.
    mount_get_updates_once(
        &server,
        1,
        ResponseTemplate::new(200).set_body_string("<html>"),
    )
    .await;
    mount_get_updates_once(
        &server,
        2,
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": false, "error_code": 500, "description": "Internal"
        })),
    )
    .await;
    mount_get_updates_once(
        &server,
        3,
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": false, "error_code": 409, "description": "Conflict: terminated by other getUpdates request"
        })),
    )
    .await;
    mount_get_updates_once(
        &server,
        4,
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": false, "error_code": 409, "description": "Conflict: can't use getUpdates method while webhook is active"
        })),
    )
    .await;
    mount_post(&server, "deleteWebhook", ok_with(serde_json::json!(true))).await;
    mount_get_updates_once(
        &server,
        5,
        updates(serde_json::json!([
            {
                "update_id": 11,
                "message": {
                    "message_id": 5, "chat": { "id": 100, "type": "private" },
                    "from": { "id": 8, "username": "mallory" }, "text": "let me in"
                }
            },
            {
                "update_id": 12,
                "message": {
                    "message_id": 5, "message_thread_id": 88,
                    "chat": { "id": 100, "type": "supergroup" },
                    "from": { "id": 7, "username": "alice" }, "text": "original hello"
                }
            },
            {
                "update_id": 13,
                "edited_message": {
                    "message_id": 5, "edit_date": 1234,
                    "message_thread_id": 88,
                    "chat": { "id": 100, "type": "supergroup" },
                    "from": { "id": 7, "username": "alice" }, "text": "edited hello"
                }
            },
            {
                "update_id": 14,
                "edited_message": {
                    "message_id": 5, "edit_date": 1235,
                    "chat": { "id": 100, "type": "supergroup" },
                    "from": { "id": 8, "username": "mallory" }, "text": "unauthorized edit"
                }
            },
            {
                "update_id": 15,
                "message_reaction": {
                    "chat": { "id": 100, "type": "supergroup" }, "message_id": 5, "message_thread_id": 88,
                    "user": { "id": 7, "username": "alice" },
                    "new_reaction": [{ "type": "emoji", "emoji": "❤" }]
                }
            },
        ])),
    )
    .await;
    // Next poll re-delivers 12 (deduped) and a new message that the closed
    // receiver can no longer take, which ends the loop.
    mount_get_updates_once(
        &server,
        6,
        updates(serde_json::json!([
            {
                "update_id": 12,
                "message": {
                    "message_id": 6, "chat": { "id": 100, "type": "private" },
                    "from": { "id": 7, "username": "alice" }, "text": "hello bot"
                }
            },
            {
                "update_id": 15,
                "message": {
                    "message_id": 9, "chat": { "id": 100, "type": "private" },
                    "from": { "id": 7, "username": "alice" }, "text": "second"
                }
            }
        ])),
    )
    .await;
    // Any later poll delivers yet another update, so the loop ends on the
    // closed receiver no matter how far it ran ahead of the test.
    Mock::given(method("POST"))
        .and(path(api_path("getUpdates")))
        .respond_with(updates(serde_json::json!([
            {
                "update_id": 16,
                "message": {
                    "message_id": 10, "chat": { "id": 100, "type": "private" },
                    "from": { "id": 7, "username": "alice" }, "text": "third"
                }
            }
        ])))
        .with_priority(10)
        .mount(&server)
        .await;
    mount_post(&server, "sendMessage", ok()).await;

    let events = Arc::new(RecordingEvents::default());
    let ch = Arc::new(fast_backoff(
        channel_for(&server, &["alice"]).with_events(events.clone()),
    ));
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let listener = tokio::spawn({
        let ch = ch.clone();
        async move { listen_loop(&ch, tx).await }
    });

    let first = tokio::time::timeout(GUARD, rx.recv())
        .await
        .expect("message dispatched")
        .expect("channel open");
    assert_eq!(first.content, "original hello");
    assert_eq!(first.sender, "alice");
    drop(rx);

    tokio::time::timeout(GUARD, listener)
        .await
        .expect("listen returns once receiver is gone")
        .unwrap()
        .unwrap();

    let recorded = events.events.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].1, "message_updated");
    assert_eq!(recorded[0].2["type"], "message_updated");
    assert_eq!(recorded[0].2["sourceEventId"], "13");
    assert_eq!(recorded[0].2["conversation"]["id"], "100");
    assert_eq!(recorded[0].2["messageId"], "telegram_100_5");
    assert_eq!(recorded[0].2["providerMessageId"], "5");
    assert_eq!(recorded[0].2["conversation"]["kind"], "group");
    assert_eq!(recorded[0].2["conversation"]["topicId"], "88");
    assert_eq!(recorded[0].2["parts"][0]["text"], "edited hello");
    assert_eq!(recorded[1].1, "reaction_added");
    assert_eq!(recorded[1].2["type"], "reaction_added");
    assert_eq!(recorded[1].2["sourceEventId"], "15");
    assert_eq!(
        recorded[1].2["eventId"],
        "telegram:update:15:reaction:added:100:5:❤"
    );
    assert_eq!(recorded[1].2["messageId"], "telegram_100_5");
    assert_eq!(recorded[1].2["providerMessageId"], "5");
    assert_eq!(recorded[1].2["conversation"]["id"], "100");
    assert_eq!(recorded[1].2["conversation"]["topicId"], "88");
    assert_eq!(recorded[1].2["actor"]["id"], "7");
    assert_eq!(recorded[1].2["reaction"]["value"], "❤");
    assert_eq!(json_bodies(&server, "deleteWebhook").await.len(), 1);
    // The unauthorized sender got the approval prompt.
    let prompts = sent_texts(&server).await;
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("operator approval"), "{prompts:?}");
    // Offsets advance past each delivered batch.
    let polls = json_bodies(&server, "getUpdates").await;
    assert!(polls.len() >= 6, "{polls:?}");
    assert_eq!(polls[0]["offset"], 0);
    assert!(polls.iter().any(|poll| poll["offset"] == 16));
}

#[tokio::test]
async fn listener_projects_reaction_removal_without_provider_update_id() {
    let server = mock_api().await;
    mount_get_updates_once(
        &server,
        1,
        updates(serde_json::json!([{
            "message_reaction": {
                "chat": { "id": 100, "type": "supergroup" },
                "message_id": 5,
                "message_thread_id": 88,
                "user": { "id": 7, "username": "alice" },
                "old_reaction": [{ "type": "emoji", "emoji": "❤" }],
                "new_reaction": []
            }
        }])),
    )
    .await;
    mount_get_updates_once(
        &server,
        2,
        updates(serde_json::json!([{
            "update_id": 10,
            "message": {
                "message_id": 6,
                "chat": { "id": 100, "type": "private" },
                "from": { "id": 7, "username": "alice" },
                "text": "stop"
            }
        }])),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(api_path("getUpdates")))
        .respond_with(updates(serde_json::json!([{
            "update_id": 11,
            "message": {
                "message_id": 7,
                "chat": { "id": 100, "type": "private" },
                "from": { "id": 7, "username": "alice" },
                "text": "finish"
            }
        }])))
        .with_priority(10)
        .mount(&server)
        .await;

    let events = Arc::new(RecordingEvents::default());
    let channel = Arc::new(fast_backoff(
        channel_for(&server, &["alice"]).with_events(events.clone()),
    ));
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let listener = tokio::spawn({
        let channel = channel.clone();
        async move { listen_loop(&channel, tx).await }
    });
    let message = tokio::time::timeout(GUARD, rx.recv())
        .await
        .expect("later message dispatched")
        .expect("listener channel remains open");
    assert_eq!(message.content, "stop");
    drop(rx);
    tokio::time::timeout(GUARD, listener)
        .await
        .expect("listener exits after the receiver closes")
        .unwrap()
        .unwrap();

    let recorded = events.events.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].1, "reaction_removed");
    assert_eq!(
        recorded[0].2["eventId"],
        "telegram:reaction:100:5:7:❤:removed"
    );
    assert_eq!(recorded[0].2["actor"]["id"], "7");
    assert_eq!(recorded[0].2["conversation"]["topicId"], "88");
}

#[tokio::test]
async fn listener_projects_authorized_callback_without_provider_update_id() {
    let server = mock_api().await;
    mount_get_updates_once(
        &server,
        1,
        updates(serde_json::json!([{
            "callback_query": {
                "id": "callback-7",
                "from": { "id": 7, "username": "alice", "is_bot": true },
                "message": {
                    "message_id": 5,
                    "message_thread_id": 88,
                    "chat": { "id": 100, "type": "supergroup" }
                },
                "data": "approve-once"
            }
        }])),
    )
    .await;
    mount_get_updates_once(
        &server,
        2,
        updates(serde_json::json!([{
            "update_id": 10,
            "message": {
                "message_id": 6,
                "chat": { "id": 100, "type": "private" },
                "from": { "id": 7, "username": "alice" },
                "text": "stop"
            }
        }])),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(api_path("getUpdates")))
        .respond_with(updates(serde_json::json!([{
            "update_id": 11,
            "message": {
                "message_id": 7,
                "chat": { "id": 100, "type": "private" },
                "from": { "id": 7, "username": "alice" },
                "text": "finish"
            }
        }])))
        .with_priority(10)
        .mount(&server)
        .await;

    let events = Arc::new(RecordingEvents::default());
    let channel = Arc::new(fast_backoff(
        channel_for(&server, &["alice"]).with_events(events.clone()),
    ));
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let listener = tokio::spawn({
        let channel = channel.clone();
        async move { listen_loop(&channel, tx).await }
    });
    let _ = tokio::time::timeout(GUARD, rx.recv())
        .await
        .expect("later message dispatched")
        .expect("listener channel remains open");
    drop(rx);
    tokio::time::timeout(GUARD, listener)
        .await
        .expect("listener exits after the receiver closes")
        .unwrap()
        .unwrap();

    let recorded = events.events.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].1, "button_callback");
    assert_eq!(recorded[0].2["eventId"], "telegram:callback:callback-7");
    assert_eq!(recorded[0].2["sourceEventId"], serde_json::Value::Null);
    assert_eq!(recorded[0].2["conversation"]["topicId"], "88");
    assert_eq!(recorded[0].2["actor"]["isBot"], true);
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
async fn a_deduplicated_update_still_advances_the_offset() {
    let server = mock_api().await;
    // Update 20 was already seen, so it is skipped; the next poll must still
    // confirm it, or Telegram re-sends it forever.
    mount_get_updates_once(
        &server,
        1,
        updates(serde_json::json!([{ "update_id": 20 }])),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(api_path("getUpdates")))
        .respond_with(updates(serde_json::json!([])))
        .with_priority(10)
        .mount(&server)
        .await;

    let ch = Arc::new(fast_backoff(channel_for(&server, &["alice"])));
    assert!(ch.track_update_id(20));
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let listener = tokio::spawn({
        let ch = ch.clone();
        async move { listen_loop(&ch, tx).await }
    });
    let polls = tokio::time::timeout(GUARD, async {
        loop {
            let polls = json_bodies(&server, "getUpdates").await;
            if polls.len() >= 2 {
                return polls;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("polled twice");
    listener.abort();

    assert_eq!(polls[0]["offset"], 0);
    assert_eq!(polls[1]["offset"], 21, "{polls:?}");
}

#[derive(Default)]
struct RejectingInboundEvents(AtomicUsize);

#[async_trait::async_trait]
impl crate::host::EventSink for RejectingInboundEvents {
    async fn publish(
        &self,
        _domain: &str,
        _kind: &str,
        _payload: serde_json::Value,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn publish_ingress(
        &self,
        ingress: tinychannels_bus::ChannelIngress,
    ) -> anyhow::Result<bool> {
        if matches!(ingress, tinychannels_bus::ChannelIngress::Inbound(_)) {
            self.0.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("fixture ingress delivery failed");
        }
        Ok(true)
    }
}

#[tokio::test]
async fn listen_does_not_duplicate_inbound_after_ordered_sink_failure() {
    let server = mock_api().await;
    mount_get_updates_once(
        &server,
        1,
        updates(serde_json::json!([{
            "update_id": 21,
            "message": {
                "message_id": 7,
                "chat": {"id": 100, "type": "private"},
                "from": {"id": 8, "username": "alice"},
                "text": "one delivery attempt"
            }
        }])),
    )
    .await;
    let events = Arc::new(RejectingInboundEvents::default());
    let channel = Arc::new(fast_backoff(
        channel_for(&server, &["alice"]).with_events(events.clone()),
    ));
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let listener = tokio::spawn({
        let channel = channel.clone();
        async move { listen_loop(&channel, tx).await }
    });

    let delivered_on_legacy_path = tokio::time::timeout(GUARD, rx.recv())
        .await
        .expect("listener must finish after the ingress callback fails");
    assert!(
        delivered_on_legacy_path.is_none(),
        "an ambiguous callback failure must not submit the same update through the legacy queue"
    );
    let result = tokio::time::timeout(GUARD, listener)
        .await
        .expect("listener must return after callback failure")
        .unwrap();
    assert!(result.is_err());
    assert_eq!(events.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn listen_backs_off_when_webhook_delete_fails_and_on_transport_error() {
    let server = mock_api().await;
    mount_get_updates_once(
        &server,
        1,
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": false, "error_code": 409, "description": "webhook is active"
        })),
    )
    .await;
    mount_post(&server, "deleteWebhook", ResponseTemplate::new(500)).await;
    mount_get_updates_once(
        &server,
        2,
        updates(serde_json::json!([
            {
                "update_id": 1,
                "message": {
                    "message_id": 2, "chat": { "id": 5, "type": "group" },
                    "from": { "id": 3, "username": "alice" }, "text": "hey @my_bot ping"
                }
            }
        ])),
    )
    .await;
    Mock::given(method("GET"))
        .and(path(api_path("getMe")))
        .respond_with(ok_with(serde_json::json!({ "username": "my_bot" })))
        .mount(&server)
        .await;

    let mut ch = fast_backoff(channel_for(&server, &["alice"]));
    ch.mention_only = true;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let ch = Arc::new(ch);
    let listener = tokio::spawn({
        let ch = ch.clone();
        async move { listen_loop(&ch, tx).await }
    });

    let msg = tokio::time::timeout(GUARD, rx.recv())
        .await
        .expect("message dispatched")
        .expect("channel open");
    assert_eq!(
        msg.content, "hey ping",
        "mention stripped in mention-only group"
    );
    assert_eq!(json_bodies(&server, "deleteWebhook").await.len(), 1);
    listener.abort();

    // Transport failures back off and retry rather than ending the loop.
    let mut dead = fast_backoff(TelegramChannel::new("tok".into(), vec!["*".into()], true));
    dead.api_base = dead_base();
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let outcome = tokio::time::timeout(Duration::from_millis(300), listen_loop(&dead, tx)).await;
    assert!(
        outcome.is_err(),
        "listen keeps retrying on transport errors"
    );
    assert!(dead.bot_username.lock().is_none());
}

#[tokio::test]
async fn loopback_bot_api_retries_poll_then_routes_status_reply() {
    let api = BotApiFake::start().await;
    // The current poll loop retries this API error using poll_error_backoff;
    // retry_after is part of the realistic response shape, not a delay assertion.
    api.fail_next_poll(
        429,
        serde_json::json!({
            "ok": false,
            "error_code": 429,
            "description": "Too Many Requests",
            "parameters": { "retry_after": 1 }
        }),
    );
    api.inject(serde_json::json!({
        "update_id": 1,
        "message": {
            "message_id": 41,
            "chat": { "id": 777, "type": "private" },
            "from": { "id": 7, "username": "alice" },
            "text": "/status"
        }
    }));

    let mut channel = api.channel(&["alice"]);
    channel.poll_error_backoff = Duration::from_millis(1);
    let channel = Arc::new(channel);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let listener = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { listen_loop(&channel, tx).await }
    });

    let inbound = tokio::time::timeout(GUARD, rx.recv())
        .await
        .expect("poll recovers from rate limit")
        .expect("injected command is dispatched");
    assert_eq!(inbound.content, "/status");
    assert_eq!(inbound.sender, "alice");
    assert_eq!(inbound.reply_target, "777");
    assert_eq!(inbound.thread_ts.as_deref(), Some("41"));

    // This is the channel-facing part of the host's /status handler: it
    // turns the received command into a provider reply in the originating chat.
    channel
        .send_compat_message(
            &SendMessage::new("Thread: none\nProvider: openai", inbound.reply_target)
                .in_thread(inbound.thread_ts),
        )
        .await
        .unwrap();
    assert_eq!(
        api.sent(),
        vec![(
            "sendMessage".to_string(),
            serde_json::json!({
                "chat_id": "777",
                "text": "Thread: none\nProvider: openai",
                "parse_mode": "MarkdownV2",
                "reply_to_message_id": 41
            })
        )]
    );

    // A second injected message lets listen exit cleanly when the receiver is
    // dropped, proving the loop processed a subsequent poll after the 429.
    drop(rx);
    api.inject(serde_json::json!({
        "update_id": 2,
        "message": {
            "message_id": 42,
            "chat": { "id": 777, "type": "private" },
            "from": { "id": 7, "username": "alice" },
            "text": "stop"
        }
    }));
    tokio::time::timeout(GUARD, listener)
        .await
        .expect("listener stops after receiver closes")
        .unwrap()
        .unwrap();
}

async fn listen_loop(
    channel: &TelegramChannel,
    tx: tokio::sync::mpsc::Sender<ChannelMessage>,
) -> anyhow::Result<()> {
    let (_cancel, receiver) = tokio::sync::watch::channel(false);
    channel.listen_loop(Some(tx), None, receiver).await
}
