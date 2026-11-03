use super::*;
use tinychannels_bus::ChannelOutboundIntent;
use tinystoragedrivers_api::StorageBackend;
use tinystoragedrivers_testkit::MemoryBackend;
use tinystoragedrivers_types::{CollectionSpec, Scope};

fn docs() -> Arc<dyn DocumentStore> {
    let backend = MemoryBackend::new();
    let storage = backend
        .for_scope(&Scope::new("tenant:delivery-tests").unwrap())
        .unwrap();
    Arc::clone(storage.documents())
}

fn intent(id: &str, text: &str) -> ChannelOutboundIntent {
    ChannelOutboundIntent {
        idempotency_key: id.into(),
        channel_id: "telegram".into(),
        conversation_id: "room".into(),
        channel_ref: None,
        conversation_ref: None,
        turn_id: None,
        durability: tinychannels_bus::DeliveryDurability::Required,
        payload: crate::OutboundPayload::Text { text: text.into() },
        ..Default::default()
    }
}

fn entry(id: &str, at: u64) -> QueuedDelivery {
    QueuedDelivery::new(id, intent(id, "body"), at)
}

fn result(id: &str, completed_at: u64) -> DeliveryTerminalResult {
    DeliveryTerminalResult {
        id: id.into(),
        intent_fingerprint: "a".repeat(64),
        completed_at_unix_ms: completed_at,
        receipt: None,
    }
}

#[tokio::test]
async fn durable_delivery_preserves_account_conversation_and_turn_identity() {
    let store = DocumentDeliveryStore::new(docs());
    let mut queued = entry("scoped-delivery", 1);
    queued.intent.channel_ref = Some(tinychannels_bus::ChannelRef {
        id: "telegram".into(),
        account_id: Some("telegram:fixture".into()),
    });
    queued.intent.conversation_ref = Some(tinychannels_bus::ConversationRef {
        id: "room".into(),
        thread_id: Some("topic".into()),
        ..Default::default()
    });
    queued.intent.turn_id = Some("cron-run".into());
    store.save_pending(queued.clone()).await.unwrap();

    assert_eq!(
        store.load_pending("scoped-delivery").await.unwrap(),
        Some(queued)
    );
}

#[tokio::test]
async fn prepared_storage_delivers_without_repeating_schema_mutations() {
    use tinystoragedrivers_testkit::{Failing, FaultPlan};

    let documents = docs();
    crate::storage::prepare(&documents).await.unwrap();
    let faults = FaultPlan::new();
    faults.fail_operation_nth("documents.ensure_collection", 1, ErrorKind::Unavailable);
    let documents: Arc<dyn DocumentStore> = Arc::new(Failing::with_plan(documents, faults));
    let store = DocumentDeliveryStore::from_prepared(documents);

    store.save_pending(entry("host-prepared", 1)).await.unwrap();
    assert_eq!(
        store.load_pending("host-prepared").await.unwrap(),
        Some(entry("host-prepared", 1))
    );
}

#[tokio::test]
async fn pending_records_round_trip_in_enqueue_order() {
    let store = DocumentDeliveryStore::new(docs());
    store.save_pending(entry("b", 20)).await.unwrap();
    store.save_pending(entry("a", 10)).await.unwrap();
    store.save_pending(entry("c", 30)).await.unwrap();

    assert_eq!(store.load_pending("a").await.unwrap(), Some(entry("a", 10)));
    assert_eq!(store.load_pending("missing").await.unwrap(), None);
    let ids: Vec<_> = store
        .load_pending_all()
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(ids, ["a", "b", "c"]);

    store.remove_pending("b").await.unwrap();
    store.remove_pending("b").await.unwrap();
    assert_eq!(store.load_pending_all().await.unwrap().len(), 2);
}

#[tokio::test]
async fn records_survive_reopening_the_store_over_the_same_documents() {
    let docs = docs();
    let first = DocumentDeliveryStore::new(Arc::clone(&docs));
    first.save_pending(entry("kept", 1)).await.unwrap();
    first.save_failed(entry("dead", 2)).await.unwrap();
    first
        .save_result(result("done", now_unix_ms()))
        .await
        .unwrap();
    drop(first);

    let second = DocumentDeliveryStore::new(docs);
    assert_eq!(
        second.load_pending("kept").await.unwrap(),
        Some(entry("kept", 1))
    );
    assert_eq!(
        second.load_failed("dead").await.unwrap(),
        Some(entry("dead", 2))
    );
    assert_eq!(
        second.load_result("done").await.unwrap().map(|r| r.id),
        Some("done".into())
    );
}

#[tokio::test]
async fn an_update_keeps_the_intent_and_a_different_intent_is_refused() {
    let store = DocumentDeliveryStore::new(docs());
    store.save_pending(entry("k", 1)).await.unwrap();

    let mut retried = entry("k", 1);
    retried.retry_count = 3;
    retried.last_error = Some("boom".into());
    store.save_pending(retried.clone()).await.unwrap();
    assert_eq!(store.load_pending("k").await.unwrap(), Some(retried));

    let conflicting = QueuedDelivery::new("k", intent("k", "different"), 1);
    assert!(matches!(
        store.save_pending(conflicting).await,
        Err(DeliveryQueueError::Store(message)) if message.contains("different intent")
    ));
}

#[tokio::test]
async fn concurrent_inserts_of_one_key_both_succeed_and_leave_one_record() {
    let store = DocumentDeliveryStore::new(docs());
    let (a, b) = tokio::join!(
        store.save_pending(entry("race", 1)),
        store.save_pending(entry("race", 1))
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(store.load_pending_all().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_full_queue_refuses_new_keys_but_still_updates_existing_ones() {
    let store = DocumentDeliveryStore::new(docs());
    for index in 0..DELIVERY_PENDING_CAPACITY {
        store
            .save_pending(entry(&format!("q-{index}"), index as u64))
            .await
            .unwrap();
    }
    assert!(matches!(
        store.save_pending(entry("overflow", 9)).await,
        Err(DeliveryQueueError::Store(message)) if message.contains("full")
    ));
    let mut update = entry("q-0", 0);
    update.retry_count = 1;
    store.save_pending(update).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_distinct_inserts_never_exceed_the_pending_capacity() {
    let backing = docs();
    let first = DocumentDeliveryStore::new(backing.clone());
    for index in 0..DELIVERY_PENDING_CAPACITY - 1 {
        first
            .save_pending(entry(&format!("q-{index}"), index as u64))
            .await
            .unwrap();
    }
    let mut tasks = Vec::new();
    for n in 0..32 {
        let store = DocumentDeliveryStore::new(backing.clone());
        tasks.push(tokio::spawn(async move {
            store.save_pending(entry(&format!("late-{n}"), 99)).await
        }));
    }
    let mut accepted = 0;
    for task in tasks {
        if task.await.unwrap().is_ok() {
            accepted += 1;
        }
    }
    assert_eq!(accepted, 1);
    assert_eq!(
        first.load_pending_all().await.unwrap().len(),
        DELIVERY_PENDING_CAPACITY
    );
}

#[tokio::test]
async fn invalid_and_oversized_records_are_refused_before_storage() {
    let docs = docs();
    let store = DocumentDeliveryStore::new(Arc::clone(&docs));
    assert!(store.load_pending("").await.is_err());
    assert!(
        store
            .load_failed(&"x".repeat(MAX_ID_BYTES + 1))
            .await
            .is_err()
    );
    assert!(
        store
            .save_pending(QueuedDelivery::new("other", intent("k", "b"), 1))
            .await
            .is_err()
    );
    let oversized =
        QueuedDelivery::new("big", intent("big", &"x".repeat(DELIVERY_RECORD_BYTES)), 1);
    assert!(store.save_pending(oversized).await.is_err());
    assert!(
        store
            .save_failed(QueuedDelivery::new("other", intent("k", "b"), 1))
            .await
            .is_err()
    );
    assert!(
        docs.get(PENDING_COLLECTION, "big").await.unwrap().is_none(),
        "nothing was written"
    );
}

#[tokio::test]
async fn expired_results_read_as_absent_and_are_purged_on_the_next_save() {
    let docs = docs();
    let store = DocumentDeliveryStore::new(Arc::clone(&docs));
    let long_ago = 1;
    store.save_result(result("old", long_ago)).await.unwrap();
    assert!(store.load_result("old").await.unwrap().is_none());

    store
        .save_result(result("new", now_unix_ms()))
        .await
        .unwrap();
    assert!(docs.get(RESULTS_COLLECTION, "old").await.unwrap().is_none());
    assert!(store.load_result("new").await.unwrap().is_some());
}

#[tokio::test]
async fn results_beyond_the_capacity_evict_the_oldest_and_keep_the_newest() {
    let store = DocumentDeliveryStore::new(docs());
    let base = now_unix_ms();
    for index in 0..DELIVERY_RESULT_CAPACITY {
        store
            .save_result(result(&format!("r-{index}"), base + index as u64))
            .await
            .unwrap();
    }
    let newest = format!("r-{DELIVERY_RESULT_CAPACITY}");
    store
        .save_result(result(&newest, base + DELIVERY_RESULT_CAPACITY as u64))
        .await
        .unwrap();
    assert!(
        store.load_result("r-0").await.unwrap().is_none(),
        "oldest evicted"
    );
    assert!(store.load_result("r-1").await.unwrap().is_some());
    assert!(store.load_result(&newest).await.unwrap().is_some());
}

#[tokio::test]
async fn a_storage_outage_is_a_store_error_not_a_missing_record() {
    #[derive(Debug)]
    struct Down;
    #[async_trait]
    impl DocumentStore for Down {
        fn capabilities(&self) -> tinystoragedrivers_types::Capabilities {
            tinystoragedrivers_types::Capabilities::none()
        }
        async fn ensure_collection(
            &self,
            _: &CollectionSpec,
        ) -> tinystoragedrivers_types::Result<()> {
            Err(StorageError::unavailable("down"))
        }
        async fn get(
            &self,
            _: &str,
            _: &str,
        ) -> tinystoragedrivers_types::Result<Option<tinystoragedrivers_types::Versioned<Value>>>
        {
            Err(StorageError::unavailable("down"))
        }
        async fn put(
            &self,
            _: &str,
            _: &str,
            _: Value,
            _: Precondition,
        ) -> tinystoragedrivers_types::Result<tinystoragedrivers_types::Version> {
            Err(StorageError::unavailable("down"))
        }
        async fn delete(
            &self,
            _: &str,
            _: &str,
            _: Precondition,
        ) -> tinystoragedrivers_types::Result<bool> {
            Err(StorageError::unavailable("down"))
        }
        async fn query(
            &self,
            _: &str,
            _: &Query,
        ) -> tinystoragedrivers_types::Result<
            tinystoragedrivers_types::Page<tinystoragedrivers_types::Versioned<Value>>,
        > {
            Err(StorageError::unavailable("down"))
        }
        async fn count(&self, _: &str, _: &Filter) -> tinystoragedrivers_types::Result<u64> {
            Err(StorageError::unavailable("down"))
        }
        async fn delete_where(&self, _: &str, _: &Filter) -> tinystoragedrivers_types::Result<u64> {
            Err(StorageError::unavailable("down"))
        }
        async fn claim(
            &self,
            _: &str,
            _: &Filter,
            _: &[Sort],
            _: &Value,
        ) -> tinystoragedrivers_types::Result<Option<tinystoragedrivers_types::Versioned<Value>>>
        {
            Err(StorageError::unavailable("down"))
        }
        async fn drop_collection(&self, _: &str) -> tinystoragedrivers_types::Result<()> {
            Err(StorageError::unavailable("down"))
        }
    }
    let store = DocumentDeliveryStore::new(Arc::new(Down));
    assert!(matches!(
        store.load_pending("k").await,
        Err(DeliveryQueueError::Store(_))
    ));
    assert!(matches!(
        store.save_pending(entry("k", 1)).await,
        Err(DeliveryQueueError::Store(_))
    ));
    assert!(matches!(
        store.load_pending_all().await,
        Err(DeliveryQueueError::Store(_))
    ));
}

#[tokio::test]
async fn deliveries_are_written_as_snake_case_ms_bodies_under_key_ids() {
    let docs = docs();
    let store = DocumentDeliveryStore::new(Arc::clone(&docs));
    let mut queued = entry("a/b", 5);
    queued.last_attempt_at_unix_ms = Some(7);
    store.save_pending(queued.clone()).await.unwrap();
    store.save_failed(queued.clone()).await.unwrap();

    let stored = docs
        .get(PENDING_COLLECTION, "a%2Fb")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.meta.schema, SCHEMA_VERSION);
    assert_eq!(
        stored.doc,
        serde_json::json!({
            "intent": {
                "idempotency_key": "a/b",
                "channel_id": "telegram",
                "conversation_id": "room",
                "reply_to_id": null,
                "thread_id": null,
                "durability": "required",
                "payload": { "kind": "text", "text": "body" },
            },
            "enqueued_ms": 5,
            "retry_count": 0,
            "last_attempt_ms": 7,
        })
    );
    assert!(docs.get(PENDING_COLLECTION, "a/b").await.unwrap().is_none());
    assert_eq!(store.load_failed("a/b").await.unwrap(), Some(queued));
}

#[tokio::test]
async fn a_delivery_stored_under_the_wrong_id_is_unreadable_not_returned() {
    let docs = docs();
    let store = DocumentDeliveryStore::new(Arc::clone(&docs));
    store.save_pending(entry("real", 1)).await.unwrap();
    let stored = docs.get(PENDING_COLLECTION, "real").await.unwrap().unwrap();
    docs.put(
        PENDING_COLLECTION,
        "other",
        stored.doc,
        Precondition::Absent,
    )
    .await
    .unwrap();

    assert!(matches!(
        store.load_pending("other").await,
        Err(DeliveryQueueError::Store(message)) if message.contains("unreadable")
    ));
}

#[tokio::test]
async fn a_result_stored_under_the_wrong_id_is_unreadable_not_returned() {
    let docs = docs();
    let store = DocumentDeliveryStore::new(Arc::clone(&docs));
    store
        .save_result(result("real", now_unix_ms()))
        .await
        .unwrap();
    let stored = docs.get(RESULTS_COLLECTION, "real").await.unwrap().unwrap();
    docs.put(
        RESULTS_COLLECTION,
        "other",
        stored.doc,
        Precondition::Absent,
    )
    .await
    .unwrap();

    assert!(matches!(
        store.load_result("other").await,
        Err(DeliveryQueueError::Store(message)) if message.contains("unreadable")
    ));
    assert!(store.load_result("real").await.unwrap().is_some());
}

#[tokio::test]
async fn a_result_expires_through_record_metadata_and_keeps_no_time_but_completion() {
    let docs = docs();
    let store = DocumentDeliveryStore::new(Arc::clone(&docs));
    let completed = now_unix_ms();
    store.save_result(result("r", completed)).await.unwrap();

    let stored = docs.get(RESULTS_COLLECTION, "r").await.unwrap().unwrap();
    assert_eq!(
        stored.doc,
        serde_json::json!({
            "id": "r",
            "intent_fingerprint": "a".repeat(64),
            "completed_ms": completed,
        })
    );
    assert_eq!(
        stored.meta.expires_ms,
        Some((completed + DELIVERY_RESULT_TTL_SECONDS * 1000) as i64)
    );
    assert_eq!(stored.meta.schema, SCHEMA_VERSION);
    assert_eq!(
        store.load_result("r").await.unwrap(),
        Some(result("r", completed))
    );
}

#[tokio::test]
async fn an_unreadable_pending_row_does_not_hide_the_readable_ones() {
    let backing = docs();
    let store = DocumentDeliveryStore::new(backing.clone());
    store.save_pending(entry("good", 1)).await.unwrap();
    backing
        .put_with(
            PENDING_COLLECTION,
            "junk",
            serde_json::json!({ "not": "a delivery" }),
            Precondition::Absent,
            write_options(),
        )
        .await
        .unwrap();
    let loaded = store.load_pending_all().await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "good");
    assert!(
        backing
            .get(PENDING_COLLECTION, "junk")
            .await
            .unwrap()
            .is_some()
    );
}
