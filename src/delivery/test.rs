use crate::channel::{
    ChannelOutboundIntent, DeliveryDurability, DurableFinalDeliveryCapability, MessageReceipt,
    OutboundPayload,
};
use crate::delivery::{
    DeliveryAttemptFailure, DeliveryAttemptResult, DeliveryAttemptSuccess, DeliveryQueueError,
    DeliveryQueueHandler, DeliveryQueueStore, DeliveryRecoveryState, DeliveryRetryEligibility,
    PendingDeliveryDrainDecision, QueuedDelivery, UnknownSendReconciliation, ack_delivery,
    compute_backoff_ms, drain_selected_pending_deliveries, enqueue_delivery, fail_delivery,
    is_entry_eligible_for_recovery_retry, is_permanent_delivery_error,
    mark_delivery_platform_outcome_unknown, mark_delivery_platform_send_attempt_started,
    move_to_failed, negotiate_delivery_durability, recover_pending_deliveries,
    required_durable_final_capabilities,
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct MemoryStore {
    pending: Mutex<BTreeMap<String, QueuedDelivery>>,
    failed: Mutex<BTreeMap<String, QueuedDelivery>>,
}

impl MemoryStore {
    fn pending(&self) -> Vec<QueuedDelivery> {
        self.pending
            .lock()
            .expect("pending lock")
            .values()
            .cloned()
            .collect()
    }

    fn failed(&self) -> Vec<QueuedDelivery> {
        self.failed
            .lock()
            .expect("failed lock")
            .values()
            .cloned()
            .collect()
    }
}

#[async_trait]
impl DeliveryQueueStore for MemoryStore {
    async fn save_pending(&self, entry: QueuedDelivery) -> Result<(), DeliveryQueueError> {
        self.pending
            .lock()
            .expect("pending lock")
            .insert(entry.id.clone(), entry);
        Ok(())
    }

    async fn load_pending(&self, id: &str) -> Result<Option<QueuedDelivery>, DeliveryQueueError> {
        Ok(self.pending.lock().expect("pending lock").get(id).cloned())
    }

    async fn load_pending_all(&self) -> Result<Vec<QueuedDelivery>, DeliveryQueueError> {
        Ok(self.pending())
    }

    async fn remove_pending(&self, id: &str) -> Result<(), DeliveryQueueError> {
        self.pending.lock().expect("pending lock").remove(id);
        Ok(())
    }

    async fn save_failed(&self, entry: QueuedDelivery) -> Result<(), DeliveryQueueError> {
        self.failed
            .lock()
            .expect("failed lock")
            .insert(entry.id.clone(), entry);
        Ok(())
    }
}

struct StaticHandler {
    result: DeliveryAttemptResult,
    reconciliation: Option<UnknownSendReconciliation>,
    delivered: Arc<Mutex<Vec<String>>>,
}

impl StaticHandler {
    fn success() -> Self {
        Self {
            result: Ok(DeliveryAttemptSuccess::default()),
            reconciliation: None,
            delivered: Arc::default(),
        }
    }

    fn failure(error: impl Into<String>) -> Self {
        Self {
            result: Err(DeliveryAttemptFailure {
                error: error.into(),
                ..Default::default()
            }),
            reconciliation: None,
            delivered: Arc::default(),
        }
    }

    fn with_reconciliation(reconciliation: UnknownSendReconciliation) -> Self {
        Self {
            result: Ok(DeliveryAttemptSuccess::default()),
            reconciliation: Some(reconciliation),
            delivered: Arc::default(),
        }
    }

    fn delivered_ids(&self) -> Vec<String> {
        self.delivered.lock().expect("delivered lock").clone()
    }
}

#[async_trait]
impl DeliveryQueueHandler for StaticHandler {
    async fn deliver(&self, entry: &QueuedDelivery) -> DeliveryAttemptResult {
        self.delivered
            .lock()
            .expect("delivered lock")
            .push(entry.id.clone());
        self.result.clone()
    }

    async fn reconcile_unknown_send(
        &self,
        _entry: &QueuedDelivery,
    ) -> Result<Option<UnknownSendReconciliation>, DeliveryQueueError> {
        Ok(self.reconciliation.clone())
    }
}

fn intent(payload: OutboundPayload) -> ChannelOutboundIntent {
    ChannelOutboundIntent {
        idempotency_key: "idem-1".into(),
        channel_id: "telegram".into(),
        conversation_id: "chat-1".into(),
        reply_to_id: None,
        thread_id: None,
        durability: DeliveryDurability::Required,
        payload,
    }
}

fn text_entry(id: &str, enqueued_at_unix_ms: u64) -> QueuedDelivery {
    QueuedDelivery::new(
        id,
        intent(OutboundPayload::Text { text: "hi".into() }),
        enqueued_at_unix_ms,
    )
}

#[test]
fn computes_openclaw_backoff_schedule() {
    assert_eq!(compute_backoff_ms(0), 0);
    assert_eq!(compute_backoff_ms(1), 5_000);
    assert_eq!(compute_backoff_ms(2), 25_000);
    assert_eq!(compute_backoff_ms(3), 120_000);
    assert_eq!(compute_backoff_ms(4), 600_000);
    assert_eq!(compute_backoff_ms(5), 600_000);
}

#[test]
fn classifies_permanent_delivery_errors() {
    for message in [
        "No conversation reference found for user:abc",
        "Forum send failed: chat not found (chat_id=user:123)",
        "403: Forbidden: bot is not a member of the channel chat",
        "user not found",
        "Bot was blocked by the user",
        "Forbidden: bot was kicked from the group chat",
        "chat_id is empty",
        "Outbound not configured for channel: demo-channel",
        "MatrixError: [403] User @bot:matrix.example.com not in room !room:matrix.example.com",
    ] {
        assert!(is_permanent_delivery_error(message), "{message}");
    }

    for message in [
        "network down",
        "ETIMEDOUT",
        "socket hang up",
        "rate limited",
        "500 Internal Server Error",
    ] {
        assert!(!is_permanent_delivery_error(message), "{message}");
    }
}

#[test]
fn first_crash_replay_is_eligible_without_attempt_timestamp() {
    let entry = text_entry("entry-1", 1_000);
    assert_eq!(
        is_entry_eligible_for_recovery_retry(&entry, 1_000),
        DeliveryRetryEligibility::Eligible
    );
}

#[test]
fn retry_waits_for_backoff_window() {
    let mut entry = text_entry("entry-1", 1_000);
    entry.retry_count = 3;
    entry.last_attempt_at_unix_ms = Some(10_000);
    assert_eq!(
        is_entry_eligible_for_recovery_retry(&entry, 30_000),
        DeliveryRetryEligibility::Deferred {
            remaining_backoff_ms: 580_000
        }
    );
}

#[tokio::test]
async fn enqueue_ack_and_move_to_failed_update_store() {
    let store = MemoryStore::default();
    enqueue_delivery(&store, text_entry("entry-1", 1_000))
        .await
        .expect("enqueue");
    assert_eq!(store.pending().len(), 1);

    ack_delivery(&store, "entry-1").await.expect("ack");
    assert!(store.pending().is_empty());

    enqueue_delivery(&store, text_entry("entry-2", 1_000))
        .await
        .expect("enqueue");
    move_to_failed(&store, "entry-2")
        .await
        .expect("move failed");
    assert!(store.pending().is_empty());
    assert_eq!(store.failed().len(), 1);
}

#[tokio::test]
async fn failure_markers_preserve_retry_and_unknown_send_state() {
    let store = MemoryStore::default();
    enqueue_delivery(&store, text_entry("entry-1", 1_000))
        .await
        .expect("enqueue");

    mark_delivery_platform_send_attempt_started(&store, "entry-1", 2_000)
        .await
        .expect("mark started");
    mark_delivery_platform_outcome_unknown(&store, "entry-1", 3_000)
        .await
        .expect("mark unknown");
    fail_delivery(&store, "entry-1", "provider lookup timed out", 4_000)
        .await
        .expect("fail");

    let entry = store.pending().pop().expect("pending entry");
    assert_eq!(entry.retry_count, 1);
    assert_eq!(entry.platform_send_started_at_unix_ms, Some(2_000));
    assert_eq!(
        entry.recovery_state,
        Some(DeliveryRecoveryState::UnknownAfterSend)
    );
    assert_eq!(
        entry.last_error.as_deref(),
        Some("provider lookup timed out")
    );
}

#[tokio::test]
async fn recovery_acks_successful_deliveries() {
    let store = MemoryStore::default();
    enqueue_delivery(&store, text_entry("entry-1", 1_000))
        .await
        .expect("enqueue");

    let summary = recover_pending_deliveries(&store, &StaticHandler::success(), 2_000)
        .await
        .expect("recover");

    assert_eq!(summary.recovered, 1);
    assert!(store.pending().is_empty());
}

#[tokio::test]
async fn recovery_records_transient_failure_for_retry() {
    let store = MemoryStore::default();
    enqueue_delivery(&store, text_entry("entry-1", 1_000))
        .await
        .expect("enqueue");

    let summary =
        recover_pending_deliveries(&store, &StaticHandler::failure("network down"), 2_000)
            .await
            .expect("recover");

    let entry = store.pending().pop().expect("pending entry");
    assert_eq!(summary.failed, 1);
    assert_eq!(entry.retry_count, 1);
    assert_eq!(entry.last_attempt_at_unix_ms, Some(2_000));
    assert_eq!(entry.last_error.as_deref(), Some("network down"));
}

#[tokio::test]
async fn recovery_moves_permanent_errors_to_failed() {
    let store = MemoryStore::default();
    enqueue_delivery(&store, text_entry("entry-1", 1_000))
        .await
        .expect("enqueue");

    let summary = recover_pending_deliveries(
        &store,
        &StaticHandler::failure("No conversation reference found for user:abc"),
        2_000,
    )
    .await
    .expect("recover");

    assert_eq!(summary.failed, 1);
    assert!(store.pending().is_empty());
    assert_eq!(store.failed().len(), 1);
}

#[tokio::test]
async fn recovery_moves_entries_that_exceeded_max_retries() {
    let store = MemoryStore::default();
    let mut entry = text_entry("entry-1", 1_000);
    entry.retry_count = crate::delivery::MAX_RETRIES;
    enqueue_delivery(&store, entry).await.expect("enqueue");

    let handler = StaticHandler::success();
    let summary = recover_pending_deliveries(&store, &handler, 2_000)
        .await
        .expect("recover");

    assert_eq!(summary.skipped_max_retries, 1);
    assert!(handler.delivered_ids().is_empty());
    assert!(store.pending().is_empty());
    assert_eq!(store.failed().len(), 1);
}

#[tokio::test]
async fn recovery_does_not_blindly_replay_unknown_after_send() {
    let store = MemoryStore::default();
    let mut entry = text_entry("entry-1", 1_000);
    entry.recovery_state = Some(DeliveryRecoveryState::UnknownAfterSend);
    entry.platform_send_started_at_unix_ms = Some(1_500);
    enqueue_delivery(&store, entry).await.expect("enqueue");

    let handler = StaticHandler::success();
    let summary = recover_pending_deliveries(&store, &handler, 2_000)
        .await
        .expect("recover");

    assert_eq!(summary.failed, 1);
    assert!(handler.delivered_ids().is_empty());
    assert!(store.pending().is_empty());
    assert_eq!(store.failed().len(), 1);
}

#[tokio::test]
async fn started_entries_replay_only_when_reconciled_not_sent() {
    let store = MemoryStore::default();
    let mut entry = text_entry("entry-1", 1_000);
    entry.recovery_state = Some(DeliveryRecoveryState::SendAttemptStarted);
    entry.platform_send_started_at_unix_ms = Some(1_500);
    enqueue_delivery(&store, entry).await.expect("enqueue");

    let handler = StaticHandler::with_reconciliation(UnknownSendReconciliation::NotSent);
    let summary = recover_pending_deliveries(&store, &handler, 2_000)
        .await
        .expect("recover");

    assert_eq!(summary.recovered, 1);
    assert_eq!(handler.delivered_ids(), vec!["entry-1"]);
    assert!(store.pending().is_empty());
}

#[tokio::test]
async fn reconciled_sent_unknown_delivery_is_acked() {
    let store = MemoryStore::default();
    let mut entry = text_entry("entry-1", 1_000);
    entry.recovery_state = Some(DeliveryRecoveryState::UnknownAfterSend);
    entry.platform_send_started_at_unix_ms = Some(1_500);
    enqueue_delivery(&store, entry).await.expect("enqueue");

    let handler = StaticHandler::with_reconciliation(UnknownSendReconciliation::Sent {
        receipt: MessageReceipt {
            primary_platform_message_id: Some("platform-1".into()),
            platform_message_ids: vec!["platform-1".into()],
            sent_at: 2_000,
            ..Default::default()
        },
        message_id: Some("platform-1".into()),
    });
    let summary = recover_pending_deliveries(&store, &handler, 2_000)
        .await
        .expect("recover");

    assert_eq!(summary.recovered, 1);
    assert!(handler.delivered_ids().is_empty());
    assert!(store.pending().is_empty());
}

#[tokio::test]
async fn retryable_unresolved_reconciliation_stays_pending() {
    let store = MemoryStore::default();
    let mut entry = text_entry("entry-1", 1_000);
    entry.recovery_state = Some(DeliveryRecoveryState::UnknownAfterSend);
    entry.platform_send_started_at_unix_ms = Some(1_500);
    enqueue_delivery(&store, entry).await.expect("enqueue");

    let handler = StaticHandler::with_reconciliation(UnknownSendReconciliation::Unresolved {
        retryable: true,
        error: Some("provider lookup timed out".into()),
    });
    let summary = recover_pending_deliveries(&store, &handler, 2_000)
        .await
        .expect("recover");

    let entry = store.pending().pop().expect("pending entry");
    assert_eq!(summary.failed, 1);
    assert_eq!(entry.retry_count, 1);
    assert_eq!(
        entry.recovery_state,
        Some(DeliveryRecoveryState::UnknownAfterSend)
    );
    assert!(
        entry
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("provider lookup timed out")
    );
}

#[tokio::test]
async fn targeted_drain_can_bypass_backoff_for_matching_entries() {
    let store = MemoryStore::default();
    let mut entry = text_entry("entry-1", 1_000);
    entry.retry_count = 1;
    entry.last_attempt_at_unix_ms = Some(2_000);
    entry.last_error = Some("No active DirectChat listener".into());
    enqueue_delivery(&store, entry).await.expect("enqueue");

    let handler = StaticHandler::success();
    let summary = drain_selected_pending_deliveries(&store, &handler, 2_100, |entry| {
        PendingDeliveryDrainDecision {
            matches: entry.last_error.as_deref() == Some("No active DirectChat listener"),
            bypass_backoff: true,
        }
    })
    .await
    .expect("drain");

    assert_eq!(summary.recovered, 1);
    assert_eq!(handler.delivered_ids(), vec!["entry-1"]);
}

#[test]
fn derives_durable_capabilities_from_payload_shape() {
    let mut intent = intent(OutboundPayload::Media {
        text: Some("image".into()),
        media_urls: vec!["https://example.test/image.png".into()],
    });
    intent.reply_to_id = Some("root".into());
    intent.thread_id = Some("thread".into());

    let requirements = required_durable_final_capabilities(&intent);
    assert_eq!(
        requirements.get(&DurableFinalDeliveryCapability::Media),
        Some(&true)
    );
    assert_eq!(
        requirements.get(&DurableFinalDeliveryCapability::ReplyTo),
        Some(&true)
    );
    assert_eq!(
        requirements.get(&DurableFinalDeliveryCapability::Thread),
        Some(&true)
    );
}

#[test]
fn durability_degrades_to_best_effort_when_capability_missing() {
    let mut supported = crate::channel::DurableFinalDeliveryRequirementMap::new();
    supported.insert(DurableFinalDeliveryCapability::Text, true);
    let requirements =
        required_durable_final_capabilities(&intent(OutboundPayload::Text { text: "hi".into() }));
    assert_eq!(
        negotiate_delivery_durability(DeliveryDurability::Required, &requirements, &supported)
            .durability,
        DeliveryDurability::Required
    );

    let media_requirements = required_durable_final_capabilities(&intent(OutboundPayload::Media {
        text: None,
        media_urls: vec!["https://example.test/image.png".into()],
    }));
    let negotiated = negotiate_delivery_durability(
        DeliveryDurability::Required,
        &media_requirements,
        &supported,
    );
    assert_eq!(negotiated.durability, DeliveryDurability::BestEffort);
    assert_eq!(
        negotiated.missing_capabilities,
        vec![DurableFinalDeliveryCapability::Media]
    );
}

#[test]
fn every_durable_capability_downgrades_when_unsupported_and_preserves_when_supported() {
    for capability in crate::channel::durable_final_delivery_capabilities() {
        let mut required = crate::channel::DurableFinalDeliveryRequirementMap::new();
        required.insert(*capability, true);

        // Adapter advertises nothing: a required capability forces best-effort.
        let unsupported = negotiate_delivery_durability(
            DeliveryDurability::Required,
            &required,
            &crate::channel::DurableFinalDeliveryRequirementMap::new(),
        );
        assert_eq!(
            unsupported.durability,
            DeliveryDurability::BestEffort,
            "{capability:?} should downgrade when unsupported"
        );
        assert_eq!(
            unsupported.missing_capabilities,
            vec![*capability],
            "{capability:?} should be reported missing"
        );

        // Adapter advertises the capability: requested durability is preserved.
        let mut supported = crate::channel::DurableFinalDeliveryRequirementMap::new();
        supported.insert(*capability, true);
        let honored =
            negotiate_delivery_durability(DeliveryDurability::Required, &required, &supported);
        assert_eq!(
            honored.durability,
            DeliveryDurability::Required,
            "{capability:?} should preserve durability when supported"
        );
        assert!(
            honored.missing_capabilities.is_empty(),
            "{capability:?} should report no missing capabilities when supported"
        );
    }
}

#[test]
fn capability_advertised_as_false_counts_as_unsupported() {
    for capability in crate::channel::durable_final_delivery_capabilities() {
        let mut required = crate::channel::DurableFinalDeliveryRequirementMap::new();
        required.insert(*capability, true);
        let mut supported = crate::channel::DurableFinalDeliveryRequirementMap::new();
        supported.insert(*capability, false);

        let negotiated =
            negotiate_delivery_durability(DeliveryDurability::Required, &required, &supported);
        assert_eq!(
            negotiated.durability,
            DeliveryDurability::BestEffort,
            "{capability:?}=false must not satisfy a requirement"
        );
        assert_eq!(negotiated.missing_capabilities, vec![*capability]);
    }
}

#[test]
fn disabled_durability_short_circuits_even_with_unmet_requirements() {
    let mut required = crate::channel::DurableFinalDeliveryRequirementMap::new();
    required.insert(DurableFinalDeliveryCapability::Media, true);
    let negotiated = negotiate_delivery_durability(
        DeliveryDurability::Disabled,
        &required,
        &crate::channel::DurableFinalDeliveryRequirementMap::new(),
    );
    assert_eq!(negotiated.durability, DeliveryDurability::Disabled);
    assert!(negotiated.missing_capabilities.is_empty());
}

#[test]
fn multiple_missing_capabilities_are_all_reported() {
    let mut intent = intent(OutboundPayload::Media {
        text: None,
        media_urls: vec!["https://example.test/image.png".into()],
    });
    intent.reply_to_id = Some("root".into());
    intent.thread_id = Some("thread".into());
    let required = required_durable_final_capabilities(&intent);

    // Only Media is advertised; ReplyTo and Thread remain unmet.
    let mut supported = crate::channel::DurableFinalDeliveryRequirementMap::new();
    supported.insert(DurableFinalDeliveryCapability::Media, true);

    let negotiated =
        negotiate_delivery_durability(DeliveryDurability::Required, &required, &supported);
    assert_eq!(negotiated.durability, DeliveryDurability::BestEffort);
    assert!(
        negotiated
            .missing_capabilities
            .contains(&DurableFinalDeliveryCapability::ReplyTo)
    );
    assert!(
        negotiated
            .missing_capabilities
            .contains(&DurableFinalDeliveryCapability::Thread)
    );
    assert!(
        !negotiated
            .missing_capabilities
            .contains(&DurableFinalDeliveryCapability::Media)
    );
}

#[test]
fn payload_shape_maps_to_expected_durable_capability() {
    use DurableFinalDeliveryCapability::*;
    let cases = [
        (OutboundPayload::Text { text: "hi".into() }, Text),
        (
            OutboundPayload::Media {
                text: None,
                media_urls: vec!["https://example.test/i.png".into()],
            },
            Media,
        ),
        (
            OutboundPayload::Voice {
                media_url: "https://example.test/v.ogg".into(),
            },
            Media,
        ),
        (
            OutboundPayload::Files {
                file_urls: vec!["https://example.test/f.pdf".into()],
            },
            Media,
        ),
        (
            OutboundPayload::Poll {
                question: "q".into(),
                options: vec!["a".into(), "b".into()],
            },
            Poll,
        ),
        (
            OutboundPayload::PresentationBlocks {
                blocks: serde_json::json!([]),
            },
            Payload,
        ),
        (
            OutboundPayload::NativeChannelData {
                data: serde_json::json!({}),
            },
            Payload,
        ),
    ];

    for (payload, expected) in cases {
        let requirements = required_durable_final_capabilities(&intent(payload.clone()));
        assert_eq!(
            requirements.get(&expected),
            Some(&true),
            "{payload:?} should require {expected:?}"
        );
        // No reply/thread on the base intent, so those keys stay absent.
        assert!(!requirements.contains_key(&ReplyTo));
        assert!(!requirements.contains_key(&Thread));
    }
}
