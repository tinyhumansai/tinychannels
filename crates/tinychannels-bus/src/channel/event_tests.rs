use super::*;

fn event_with_payload(event: ChannelEventKind) -> ChannelEvent {
    ChannelEvent {
        version: 2,
        event_id: "event-1".into(),
        source_event_id: Some("provider-event-1".into()),
        source_timestamp: Some(12),
        channel: ChannelRef {
            id: "telegram".into(),
            account_id: None,
        },
        conversation: Some(ConversationRef {
            id: "chat-1".into(),
            ..Default::default()
        }),
        event,
    }
}

#[test]
fn provider_event_retains_target_message_and_event_identity() {
    let event = event_with_payload(ChannelEventKind::ReactionAdded {
        message_id: "message-2".into(),
        provider_message_id: Some("provider-message-2".into()),
        reaction: ChannelReaction {
            value: "👍".into(),
            custom_emoji_id: None,
        },
        actor: SenderRef {
            id: "user-3".into(),
            ..Default::default()
        },
    });
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["version"], 2);
    assert_eq!(value["type"], "reaction_added");
    assert_eq!(value["eventId"], "event-1");
    assert_eq!(value["sourceEventId"], "provider-event-1");
    assert_eq!(value["sourceTimestamp"], 12);
    assert_eq!(value["messageId"], "message-2");
    assert_eq!(value["providerMessageId"], "provider-message-2");
    assert_eq!(value["actor"]["id"], "user-3");
    assert_eq!(
        serde_json::from_value::<ChannelEvent>(value).unwrap(),
        event
    );
}

#[test]
fn read_receipt_identity_and_custom_emoji_survive_round_trip() {
    let mut event = event_with_payload(ChannelEventKind::ReadReceipt {
        message_id: "message-1".into(),
        actor: Some(SenderRef {
            id: "reader-1".into(),
            ..Default::default()
        }),
        timestamp: Some(42),
    });
    event.event_id = "event-read".into();
    event.conversation = None;
    event.source_timestamp = Some(43);
    let encoded = serde_json::to_value(&event).unwrap();
    assert_eq!(encoded["actor"]["id"], "reader-1");
    assert_eq!(encoded["timestamp"], 42);
    assert_eq!(encoded["sourceTimestamp"], 43);
    assert_eq!(
        serde_json::from_value::<ChannelEvent>(encoded).unwrap(),
        event
    );

    let reaction = ChannelReaction {
        value: "fire".into(),
        custom_emoji_id: Some("provider-emoji-9".into()),
    };
    let encoded = serde_json::to_value(&reaction).unwrap();
    assert_eq!(encoded["customEmojiId"], "provider-emoji-9");
    assert_eq!(
        serde_json::from_value::<ChannelReaction>(encoded).unwrap(),
        reaction
    );

    let unicode = serde_json::json!({"value":"👍"});
    assert_eq!(
        serde_json::from_value::<ChannelReaction>(unicode).unwrap(),
        ChannelReaction {
            value: "👍".into(),
            custom_emoji_id: None,
        }
    );
}

#[test]
fn event_wire_preserves_absent_and_zero_timestamps_and_false_states() {
    for timestamp in [None, Some(0)] {
        let mut event = event_with_payload(ChannelEventKind::ReadReceipt {
            message_id: "message-1".into(),
            actor: None,
            timestamp,
        });
        event.source_timestamp = timestamp;
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["timestamp"], serde_json::json!(timestamp));
        assert_eq!(value["sourceTimestamp"], serde_json::json!(timestamp));
        assert_eq!(
            serde_json::from_value::<ChannelEvent>(value).unwrap(),
            event
        );
    }
    let event = event_with_payload(ChannelEventKind::Typing {
        actor: SenderRef {
            id: "user-3".into(),
            ..Default::default()
        },
        active: false,
        expires_at: Some(0),
    });
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["active"], false);
    assert_eq!(value["expiresAt"], 0);
    assert_eq!(
        serde_json::from_value::<ChannelEvent>(value).unwrap(),
        event
    );
}
