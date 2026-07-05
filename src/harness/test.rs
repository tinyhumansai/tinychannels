use crate::channel::{
    ChannelInboundEnvelope, ChannelPresentationCapabilities, ChannelRef, ChannelStaticCapabilities,
    ConversationKind, ConversationRef, MediaKind, MediaReference, OutboundPayload, SenderRef,
};
use crate::harness::{
    BridgeTranslationOptions, ChannelOutputEvent, ChannelTurn, TurnAdmissionVerdict,
    translate_output_event,
};

fn turn() -> ChannelTurn {
    ChannelTurn {
        id: "turn-1".into(),
        session_key: "session-1".into(),
        admission: TurnAdmissionVerdict::Dispatch,
        envelope: ChannelInboundEnvelope {
            channel: ChannelRef {
                id: "telegram".into(),
                account_id: Some("bot".into()),
            },
            message_id: "msg-1".into(),
            conversation: ConversationRef {
                kind: ConversationKind::Group,
                id: "chat-1".into(),
                topic_id: Some("topic-1".into()),
                ..Default::default()
            },
            sender: SenderRef {
                id: "alice".into(),
                ..Default::default()
            },
            text: "hello".into(),
            ..Default::default()
        },
        lifecycle: Vec::new(),
    }
}

#[test]
fn text_delta_uses_draft_when_editing_supported() {
    let intents = translate_output_event(
        &turn(),
        ChannelOutputEvent::TextDelta { text: "hi".into() },
        &ChannelStaticCapabilities::default(),
        &ChannelPresentationCapabilities {
            supports_edit: true,
            ..Default::default()
        },
        BridgeTranslationOptions::default(),
    );
    assert!(matches!(
        intents[0].payload,
        OutboundPayload::NativeChannelData { .. }
    ));
}

#[test]
fn text_delta_degrades_to_segment_send_without_editing() {
    let intents = translate_output_event(
        &turn(),
        ChannelOutputEvent::TextDelta { text: "hi".into() },
        &ChannelStaticCapabilities::default(),
        &ChannelPresentationCapabilities::default(),
        BridgeTranslationOptions::default(),
    );
    assert!(matches!(intents[0].payload, OutboundPayload::Text { .. }));
}

#[test]
fn approval_degrades_to_text_when_native_actions_unavailable() {
    let intents = translate_output_event(
        &turn(),
        ChannelOutputEvent::ApprovalRequest {
            id: "approve-1".into(),
            prompt: "Run command?".into(),
            choices: vec!["approve".into(), "deny".into()],
        },
        &ChannelStaticCapabilities::default(),
        &ChannelPresentationCapabilities::default(),
        BridgeTranslationOptions::default(),
    );
    let OutboundPayload::Text { text } = &intents[0].payload else {
        panic!("expected text fallback");
    };
    assert!(text.contains("Reply with one of: approve, deny"));
}

#[test]
fn media_event_projects_media_urls() {
    let intents = translate_output_event(
        &turn(),
        ChannelOutputEvent::Media {
            text: Some("image".into()),
            media: vec![MediaReference {
                kind: MediaKind::Image,
                url: Some("https://example.test/image.png".into()),
                ..Default::default()
            }],
        },
        &ChannelStaticCapabilities::default(),
        &ChannelPresentationCapabilities::default(),
        BridgeTranslationOptions::default(),
    );
    let OutboundPayload::Media { media_urls, .. } = &intents[0].payload else {
        panic!("expected media payload");
    };
    assert_eq!(
        media_urls,
        &vec!["https://example.test/image.png".to_string()]
    );
    assert_eq!(intents[0].thread_id.as_deref(), Some("topic-1"));
}
