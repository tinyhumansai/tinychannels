use super::*;

#[test]
fn stored_intent_preserves_snake_case_payload_list_keys() {
    let stored: StoredIntent = serde_json::from_value(serde_json::json!({
        "idempotency_key": "key", "channel_id": "channel", "conversation_id": "room",
        "reply_to_id": null, "thread_id": null, "durability": "best_effort",
        "payload": {"kind": "media", "media_urls": ["https://example.test/image.png"]}
    }))
    .unwrap();
    let value = serde_json::to_value(&stored).unwrap();
    assert_eq!(
        value["payload"]["media_urls"],
        serde_json::json!(["https://example.test/image.png"])
    );
    assert!(value["payload"].get("mediaUrls").is_none());
}

#[test]
fn historical_delivery_records_default_optional_scope_and_receipts() {
    let stored: StoredDelivery = serde_json::from_value(serde_json::json!({
        "intent": {
            "idempotency_key": "key", "channel_id": "channel", "conversation_id": "room",
            "reply_to_id": null, "thread_id": null, "durability": "required",
            "payload": {"kind": "text", "text": "hello"}
        },
        "enqueued_ms": 42
    }))
    .unwrap();
    assert!(stored.intent.channel_ref.is_none());
    assert!(stored.intent.conversation_ref.is_none());
    assert!(stored.intent.turn_id.is_none());
    assert!(stored.partial_receipt.is_none());
    let result: StoredResult = serde_json::from_value(serde_json::json!({
        "id": "key", "intent_fingerprint": "digest", "completed_ms": 42
    }))
    .unwrap();
    assert!(result.receipt.is_none());
}
