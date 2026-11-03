use super::*;
use serde_json::json;
use std::sync::Arc;
use tinystoragedrivers_api::{DocumentStore, StorageBackend};
use tinystoragedrivers_testkit::{Failing, FaultPlan, MemoryBackend};
use tinystoragedrivers_types::{
    Capabilities, CollectionSpec, Filter, Page, Precondition, Query, Scope, Sort, StorageError,
    Version, Versioned, WriteOptions,
};

#[test]
fn every_migration_step_source_is_declared_for_host_storage_grants() {
    let declared: std::collections::HashSet<_> = crate::storage::collections()
        .into_iter()
        .map(|spec| spec.name)
        .collect();

    for step in steps() {
        assert!(
            declared.contains(step.source),
            "migration step source not declared: {}",
            step.source
        );
    }
}

// Hand-seeded underscore-collection rows below exercise historical schema
// transforms. The released v0.1.38 public-writer fixture is tested separately.

async fn storage() -> (MemoryBackend, tinystoragedrivers_api::Storage) {
    let backend = MemoryBackend::new();
    let storage = backend
        .for_scope(&Scope::new("tenant:migrations-test").unwrap())
        .unwrap();
    (backend, storage)
}

async fn seed(docs: &dyn DocumentStore, collection: &str, id: &str, body: Value) {
    docs.ensure_collection(&CollectionSpec::new(collection))
        .await
        .unwrap();
    docs.put_with(
        collection,
        id,
        body,
        Precondition::Absent,
        WriteOptions::new().schema(1),
    )
    .await
    .unwrap();
}

fn legacy_delivery(id: &str) -> Value {
    json!({
        "id": id,
        "intent": {
            "idempotencyKey": id,
            "channelId": "telegram",
            "conversationId": "room",
            "replyToId": null,
            "threadId": "thread-1",
            "durability": "required",
            "payload": { "kind": "text", "text": "hello" },
        },
        "enqueuedAtUnixMs": 1_700_000_000_123_u64,
        "retryCount": 2,
        "lastAttemptAtUnixMs": 1_700_000_000_456_u64,
        "lastError": "network unavailable",
        "platformSendStartedAtUnixMs": null,
        "recoveryState": "unknown_after_send",
    })
}

#[tokio::test]
async fn prepare_migrates_legacy_delivery_before_returning_and_is_idempotent() {
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    docs.ensure_collection(&CollectionSpec::new("channels_delivery_pending"))
        .await
        .unwrap();
    docs.put_with(
        "channels_delivery_pending",
        "old/op",
        legacy_delivery("old/op"),
        Precondition::Absent,
        WriteOptions::new().schema(1).expires_ms(4_102_444_800_000),
    )
    .await
    .unwrap();

    crate::storage::prepare(&docs).await.unwrap();

    let migrated = docs
        .get(DELIVERY_PENDING, "old%2Fop")
        .await
        .unwrap()
        .expect("migrated delivery exists before prepare returns");
    assert_eq!(migrated.meta.schema, SCHEMA_VERSION);
    assert_eq!(migrated.meta.expires_ms, Some(4_102_444_800_000));
    assert_eq!(migrated.doc["intent"]["idempotency_key"], "old/op");
    assert_eq!(migrated.doc["enqueued_ms"], 1_700_000_000_123_i64);
    assert!(
        docs.get("channels_delivery_pending", "old/op")
            .await
            .unwrap()
            .is_none()
    );
    let version = migrated.version;

    crate::storage::prepare(&docs).await.unwrap();

    assert_eq!(
        docs.get(DELIVERY_PENDING, "old%2Fop")
            .await
            .unwrap()
            .unwrap()
            .version,
        version,
        "a completed owner step is not replayed"
    );
}

#[tokio::test]
async fn released_v0138_public_writer_records_open_and_read_through_current_stores() {
    use crate::delivery::{DeliveryQueueStore, DeliveryResultStore};
    use crate::remote::session_store::RemoteSessionStore;
    use tinystoragedrivers_types::WriteOptions;

    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/legacy-v0.1.38/records.json"
    ))
    .expect("archived public writer fixture is valid JSON");
    let result_expiry_ms = fixture["channels.delivery_results"][0]["expires_ms"]
        .as_i64()
        .expect("fixture result expiry");
    let connection =
        tinystoragedrivers_document::memory::MemoryConnection::with_clock(Arc::new(move || {
            u64::try_from(result_expiry_ms - 1).unwrap()
        }));
    let storage = connection.for_scope(Scope::new("tenant:released-writer-fixture").unwrap());
    let docs = storage.documents();

    for (collection, records) in fixture.as_object().expect("fixture has collections") {
        let collection = collection.as_str();
        docs.ensure_collection(&CollectionSpec::new(collection))
            .await
            .unwrap();
        for record in records.as_array().expect("collection has records") {
            let id = record["id"].as_str().expect("record id");
            let schema = record["schema"].as_u64().expect("record schema") as u32;
            let mut options = WriteOptions::new().schema(schema);
            if let Some(expires_ms) = record["expires_ms"].as_i64() {
                options = options.expires_ms(expires_ms);
            }
            docs.put_with(
                collection,
                id,
                record["body"].clone(),
                Precondition::Absent,
                options,
            )
            .await
            .unwrap();
        }
    }

    // The released writer already wrote the current collection names. Opening
    // current stores must preserve and decode those rows, even though the
    // host-owned created_ms metadata is regenerated by the destination port.
    crate::storage::prepare(&docs).await.unwrap();
    let queue = crate::storage::DocumentDeliveryStore::new(Arc::clone(&docs));
    let pending = queue
        .load_pending("fixture-pending-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.intent.channel_id, "telegram");
    assert_eq!(pending.intent.conversation_id, "room-42");
    assert_eq!(pending.intent.thread_id.as_deref(), Some("thread-9"));
    assert_eq!(pending.enqueued_at_unix_ms, 1_700_000_000_123);

    let failed = queue
        .load_failed("fixture-failed-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.intent.channel_id, "telegram");
    assert_eq!(failed.intent.conversation_id, "room-42");
    assert_eq!(failed.retry_count, 0);

    let result = queue
        .load_result("fixture-result-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.id, "fixture-result-1");
    assert_eq!(
        result.completed_at_unix_ms,
        fixture["channels.delivery_results"][0]["body"]["completed_ms"]
            .as_u64()
            .expect("fixture completion timestamp")
    );

    let remote = RemoteSessionStore::new(Arc::clone(&docs));
    let binding = remote
        .binding("telegram", "room:42")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(binding.thread_id, "thread-9");
    assert_eq!(binding.sender_key, "telegram_bot_room:42");
    assert_eq!(binding.title.as_deref(), Some("Fixture chat"));
    assert!(remote.is_busy("telegram", "room:42").await.unwrap());
}

#[tokio::test]
async fn conflicting_migration_target_preserves_the_legacy_source() {
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    docs.ensure_collection(&CollectionSpec::new("channels_delivery_pending"))
        .await
        .unwrap();
    docs.put_with(
        "channels_delivery_pending",
        "op",
        legacy_delivery("op"),
        Precondition::Absent,
        WriteOptions::new().schema(1),
    )
    .await
    .unwrap();
    docs.ensure_collection(&CollectionSpec::new(DELIVERY_PENDING))
        .await
        .unwrap();
    docs.put_with(
        DELIVERY_PENDING,
        "op",
        json!({"intent": {"idempotency_key": "different"}}),
        Precondition::Absent,
        WriteOptions::new().schema(SCHEMA_VERSION),
    )
    .await
    .unwrap();

    let error = crate::storage::prepare(&docs).await.unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Conflict);
    assert!(
        docs.get("channels_delivery_pending", "op")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        docs.get(DELIVERY_PENDING, "op").await.unwrap().unwrap().doc["intent"]["idempotency_key"],
        "different"
    );
}

#[tokio::test]
async fn retry_accepts_an_exact_target_written_before_source_deletion() {
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    let legacy = legacy_delivery("op");
    seed(
        docs.as_ref(),
        "channels_delivery_pending",
        "op",
        legacy.clone(),
    )
    .await;
    let queued: QueuedDelivery = serde_json::from_value(legacy).unwrap();
    docs.ensure_collection(&CollectionSpec::new(DELIVERY_PENDING))
        .await
        .unwrap();
    docs.put_with(
        DELIVERY_PENDING,
        "op",
        serde_json::to_value(crate::storage::records::stored_delivery_from_queued(
            &queued,
        ))
        .unwrap(),
        Precondition::Absent,
        WriteOptions::new().schema(SCHEMA_VERSION),
    )
    .await
    .unwrap();

    crate::storage::prepare(&docs).await.unwrap();

    assert!(docs.get(DELIVERY_PENDING, "op").await.unwrap().is_some());
    assert!(
        docs.get("channels_delivery_pending", "op")
            .await
            .unwrap()
            .is_none(),
        "an exact durable target makes retry safe to finish source cleanup"
    );
}

#[tokio::test]
async fn target_readback_failure_keeps_source_for_safe_retry() {
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    seed(
        docs.as_ref(),
        "channels_delivery_pending",
        "op",
        legacy_delivery("op"),
    )
    .await;

    let faults = FaultPlan::new();
    // First read checks the owner marker; the second verifies the target
    // after writing it. Fail that verification to ensure source deletion is
    // never attempted on an unverified copy.
    faults.fail_operation_nth("documents.get", 2, ErrorKind::Backend);
    let failing: Arc<dyn DocumentStore> = Arc::new(Failing::with_plan(Arc::clone(&docs), faults));
    let error = crate::storage::prepare(&failing).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Backend);
    assert!(docs.get(DELIVERY_PENDING, "op").await.unwrap().is_some());
    assert!(
        docs.get("channels_delivery_pending", "op")
            .await
            .unwrap()
            .is_some()
    );

    crate::storage::prepare(&docs).await.unwrap();
    assert!(
        docs.get("channels_delivery_pending", "op")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn source_delete_conflict_keeps_legacy_row_for_safe_retry() {
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    seed(
        docs.as_ref(),
        "channels_delivery_pending",
        "op",
        legacy_delivery("op"),
    )
    .await;

    let faults = FaultPlan::new();
    faults.fail_operation_nth("documents.delete", 1, ErrorKind::Conflict);
    let failing: Arc<dyn DocumentStore> = Arc::new(Failing::with_plan(Arc::clone(&docs), faults));
    let error = crate::storage::prepare(&failing).await.unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Conflict);
    assert!(docs.get(DELIVERY_PENDING, "op").await.unwrap().is_some());
    assert!(
        docs.get("channels_delivery_pending", "op")
            .await
            .unwrap()
            .is_some(),
        "a failed conditional delete must keep the legacy source available"
    );

    crate::storage::prepare(&docs).await.unwrap();
    assert!(
        docs.get("channels_delivery_pending", "op")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn malformed_owner_marker_refuses_to_skip_a_migration() {
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    let marker_id = key::of(["tinychannels", "delivery-pending-v2"]).unwrap();
    docs.ensure_collection(&CollectionSpec::new(MIGRATIONS))
        .await
        .unwrap();
    docs.put(
        MIGRATIONS,
        marker_id.as_str(),
        json!({"owner": OWNER, "step": "delivery-pending-v2", "done": true}),
        Precondition::Absent,
    )
    .await
    .unwrap();

    let error = crate::storage::prepare(&docs).await.unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Conflict);
    assert!(error.to_string().contains("marker is malformed"));
}

#[tokio::test]
async fn marker_conflict_without_a_durable_marker_keeps_open_incomplete() {
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    let faults = FaultPlan::new();
    // The first plain put is the owner's completion marker, after all source
    // rows have been checked. A conflict without a readable matching marker
    // must not let prepare report success.
    faults.fail_operation_nth("documents.put", 1, ErrorKind::Conflict);
    let failing: Arc<dyn DocumentStore> = Arc::new(Failing::with_plan(Arc::clone(&docs), faults));

    let error = crate::storage::prepare(&failing).await.unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Conflict);
    let marker_id = key::of(["tinychannels", "delivery-pending-v2"]).unwrap();
    assert!(
        docs.get(MIGRATIONS, marker_id.as_str())
            .await
            .unwrap()
            .is_none()
    );
}

#[derive(Debug)]
struct GrowOnFirstSourceQuery {
    inner: Arc<dyn DocumentStore>,
    inserted: std::sync::atomic::AtomicUsize,
    max_insertions: usize,
}

#[async_trait::async_trait]
impl DocumentStore for GrowOnFirstSourceQuery {
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    async fn ensure_collection(
        &self,
        spec: &CollectionSpec,
    ) -> std::result::Result<(), StorageError> {
        self.inner.ensure_collection(spec).await
    }

    async fn get(
        &self,
        collection: &str,
        id: &str,
    ) -> std::result::Result<Option<Versioned<Value>>, StorageError> {
        self.inner.get(collection, id).await
    }

    async fn put(
        &self,
        collection: &str,
        id: &str,
        value: Value,
        precondition: Precondition,
    ) -> std::result::Result<Version, StorageError> {
        self.inner.put(collection, id, value, precondition).await
    }

    async fn put_with(
        &self,
        collection: &str,
        id: &str,
        value: Value,
        precondition: Precondition,
        options: WriteOptions,
    ) -> std::result::Result<Version, StorageError> {
        self.inner
            .put_with(collection, id, value, precondition, options)
            .await
    }

    async fn delete(
        &self,
        collection: &str,
        id: &str,
        precondition: Precondition,
    ) -> std::result::Result<bool, StorageError> {
        self.inner.delete(collection, id, precondition).await
    }

    async fn query(
        &self,
        collection: &str,
        query: &Query,
    ) -> std::result::Result<Page<Versioned<Value>>, StorageError> {
        let page = self.inner.query(collection, query).await?;
        let insertion = self
            .inserted
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if collection == "channels_delivery_pending" && insertion < self.max_insertions {
            let id = format!("late-op-{insertion}");
            self.inner
                .put_with(
                    collection,
                    &id,
                    legacy_delivery(&id),
                    Precondition::Absent,
                    WriteOptions::new().schema(1),
                )
                .await?;
        }
        Ok(page)
    }

    async fn count(
        &self,
        collection: &str,
        filter: &Filter,
    ) -> std::result::Result<u64, StorageError> {
        self.inner.count(collection, filter).await
    }

    async fn delete_where(
        &self,
        collection: &str,
        filter: &Filter,
    ) -> std::result::Result<u64, StorageError> {
        self.inner.delete_where(collection, filter).await
    }

    async fn claim(
        &self,
        collection: &str,
        filter: &Filter,
        sort: &[Sort],
        patch: &Value,
    ) -> std::result::Result<Option<Versioned<Value>>, StorageError> {
        self.inner.claim(collection, filter, sort, patch).await
    }

    async fn drop_collection(&self, collection: &str) -> std::result::Result<(), StorageError> {
        self.inner.drop_collection(collection).await
    }
}

#[tokio::test]
async fn prepare_rechecks_source_after_a_snapshot_grows() {
    let (_backend, storage) = storage().await;
    let inner = storage.documents().clone();
    seed(
        inner.as_ref(),
        "channels_delivery_pending",
        "first-op",
        legacy_delivery("first-op"),
    )
    .await;
    let growing: Arc<dyn DocumentStore> = Arc::new(GrowOnFirstSourceQuery {
        inner: Arc::clone(&inner),
        inserted: std::sync::atomic::AtomicUsize::new(0),
        max_insertions: 1,
    });

    crate::storage::prepare(&growing).await.unwrap();

    assert!(
        inner
            .get(DELIVERY_PENDING, "first-op")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        inner
            .get(DELIVERY_PENDING, "late-op-0")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        inner
            .get("channels_delivery_pending", "late-op-0")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn prepare_refuses_to_mark_complete_if_source_keeps_growing() {
    let (_backend, storage) = storage().await;
    let inner = storage.documents().clone();
    seed(
        inner.as_ref(),
        "channels_delivery_pending",
        "first-op",
        legacy_delivery("first-op"),
    )
    .await;
    let growing: Arc<dyn DocumentStore> = Arc::new(GrowOnFirstSourceQuery {
        inner: Arc::clone(&inner),
        inserted: std::sync::atomic::AtomicUsize::new(0),
        max_insertions: 4,
    });

    let error = crate::storage::prepare(&growing).await.unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Conflict);
    assert!(
        inner
            .get("channels_delivery_pending", "late-op-2")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        inner
            .get(DELIVERY_PENDING, "late-op-2")
            .await
            .unwrap()
            .is_none()
    );
    let marker = key::of(["tinychannels", "delivery-pending-v2"]).unwrap();
    assert!(
        inner
            .get(MIGRATIONS, marker.as_str())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn retry_refuses_a_matching_body_with_a_different_deadline() {
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    let source_expiry = 4_102_444_800_000_i64;
    docs.ensure_collection(&CollectionSpec::new("channels_delivery_pending"))
        .await
        .unwrap();
    docs.put_with(
        "channels_delivery_pending",
        "op",
        legacy_delivery("op"),
        Precondition::Absent,
        WriteOptions::new().schema(1).expires_ms(source_expiry),
    )
    .await
    .unwrap();
    let queued: QueuedDelivery = serde_json::from_value(legacy_delivery("op")).unwrap();
    docs.put_with(
        DELIVERY_PENDING,
        "op",
        serde_json::to_value(crate::storage::records::stored_delivery_from_queued(
            &queued,
        ))
        .unwrap(),
        Precondition::Absent,
        WriteOptions::new()
            .schema(SCHEMA_VERSION)
            .expires_ms(source_expiry - 1),
    )
    .await
    .unwrap();

    let error = crate::storage::prepare(&docs).await.unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Conflict);
    assert!(
        docs.get("channels_delivery_pending", "op")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn prepare_preserves_legacy_result_remote_and_whatsapp_payloads() {
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    let expires_ms = 4_102_444_800_000_i64;
    seed(
        docs.as_ref(),
        "channels_delivery_results",
        "result-id",
        json!({
            "result": { "id": "result-id", "intentFingerprint": "fp", "completedAtUnixMs": 1_700_000_000_000_u64 },
            "completedAtUnixMs": 1_700_000_000_000_u64,
            "expiresAtUnixMs": expires_ms as u64,
        }),
    )
    .await;
    seed(
        docs.as_ref(),
        "channels_delivery_failed",
        "failed-op",
        legacy_delivery("failed-op"),
    )
    .await;
    seed(
        docs.as_ref(),
        "channel_remote_bindings",
        "discord:room:42",
        json!({
            "thread_id": "thread-id", "sender_key": "sender-key", "title": "Room",
            "updated_at": "2026-01-01T00:00:00Z",
        }),
    )
    .await;
    seed(
        docs.as_ref(),
        "channel_remote_busy",
        "discord:room:42",
        json!({ "busy": true, "updated_at": "ignored" }),
    )
    .await;
    seed(
        docs.as_ref(),
        "channel_remote_meta",
        "legacy-import",
        json!({ "imported": 3 }),
    )
    .await;
    seed(
        docs.as_ref(),
        "whatsapp_identity",
        "account:identity-key",
        json!({ "account": "account", "key": "identity-key", "value": "+/9aZ0=" }),
    )
    .await;
    seed(
        docs.as_ref(),
        "whatsapp_device",
        "account:device",
        json!({ "account": "account", "key": "device", "device": { "registration_id": 7 } }),
    )
    .await;
    seed(
        docs.as_ref(),
        "whatsapp_session",
        "account:s",
        json!({ "account": "account", "key": "s", "value": "AQID" }),
    )
    .await;
    seed(
        docs.as_ref(),
        "whatsapp_prekey",
        "account:7",
        json!({ "account": "account", "key": "7", "value": "+/9aZ0=", "uploaded": true }),
    )
    .await;
    seed(
        docs.as_ref(),
        "whatsapp_signed_prekey",
        "account:8",
        json!({ "account": "account", "key": "8", "value": "AQIDBA==" }),
    )
    .await;
    seed(
        docs.as_ref(),
        "whatsapp_sender_key",
        "account:group",
        json!({ "account": "account", "key": "group", "value": "BAUG" }),
    )
    .await;
    seed(
        docs.as_ref(),
        "whatsapp_sync_key",
        "account:sync",
        json!({ "account": "account", "key": "sync", "sync_key": { "bytes": [1, 2, 3] } }),
    )
    .await;
    seed(
        docs.as_ref(),
        "whatsapp_lid_mapping",
        "account:100",
        json!({ "account": "account", "key": "100", "entry": { "lid": "100", "phone_number": "55" } }),
    )
    .await;

    crate::storage::prepare(&docs).await.unwrap();

    let result = docs
        .get(DELIVERY_RESULTS, "result-id")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        result.doc,
        json!({ "id": "result-id", "intent_fingerprint": "fp", "completed_ms": 1_700_000_000_000_i64 })
    );
    assert_eq!(result.meta.expires_ms, Some(expires_ms));
    assert_eq!(
        docs.get(DELIVERY_FAILED, "failed-op")
            .await
            .unwrap()
            .unwrap()
            .doc["intent"]["idempotency_key"],
        "failed-op"
    );
    assert_eq!(
        docs.get(REMOTE_BINDINGS, "discord/room:42")
            .await
            .unwrap()
            .unwrap()
            .doc,
        json!({ "thread_id": "thread-id", "sender_key": "sender-key", "title": "Room" })
    );
    assert_eq!(
        docs.get(REMOTE_BUSY, "discord/room:42")
            .await
            .unwrap()
            .unwrap()
            .doc,
        json!({ "busy": true })
    );
    assert_eq!(
        docs.get(REMOTE_META, "legacy-import")
            .await
            .unwrap()
            .unwrap()
            .doc,
        json!({ "imported": 3 })
    );
    assert_eq!(
        docs.get(WHATSAPP_IDENTITY, "account/identity-key")
            .await
            .unwrap()
            .unwrap()
            .doc,
        json!({ "account": "account", "key": "identity-key", "data_b64": "+/9aZ0=" })
    );
    assert_eq!(
        docs.get(WHATSAPP_DEVICE, "account/device")
            .await
            .unwrap()
            .unwrap()
            .doc,
        json!({ "account": "account", "key": "device", "device": { "registration_id": 7 } })
    );
    assert_eq!(
        docs.get(WHATSAPP_SESSION, "account/s")
            .await
            .unwrap()
            .unwrap()
            .doc,
        json!({ "account": "account", "key": "s", "data_b64": "AQID" })
    );
    assert_eq!(
        docs.get(WHATSAPP_PREKEY, "account/7")
            .await
            .unwrap()
            .unwrap()
            .doc,
        json!({ "account": "account", "key": "7", "data_b64": "+/9aZ0=", "uploaded": true })
    );
    assert_eq!(
        docs.get(WHATSAPP_SIGNED_PREKEY, "account/8")
            .await
            .unwrap()
            .unwrap()
            .doc,
        json!({ "account": "account", "key": "8", "data_b64": "AQIDBA==" })
    );
    assert_eq!(
        docs.get(WHATSAPP_SENDER_KEY, "account/group")
            .await
            .unwrap()
            .unwrap()
            .doc,
        json!({ "account": "account", "key": "group", "data_b64": "BAUG" })
    );
    assert_eq!(
        docs.get(WHATSAPP_SYNC_KEY, "account/sync")
            .await
            .unwrap()
            .unwrap()
            .doc,
        json!({ "account": "account", "key": "sync", "sync_key": { "bytes": [1, 2, 3] } })
    );
    assert_eq!(
        docs.get(WHATSAPP_LID_MAPPING, "account/100")
            .await
            .unwrap()
            .unwrap()
            .doc,
        json!({ "account": "account", "key": "100", "entry": { "lid": "100", "phone_number": "55" } })
    );
}

#[tokio::test]
async fn malformed_legacy_rows_are_removed_without_blocking_valid_rows() {
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    seed(
        docs.as_ref(),
        "channels_delivery_pending",
        "malformed",
        json!({ "id": 5 }),
    )
    .await;
    seed(
        docs.as_ref(),
        "channels_delivery_pending",
        "valid",
        legacy_delivery("valid"),
    )
    .await;

    crate::storage::prepare(&docs).await.unwrap();

    assert!(
        docs.get("channels_delivery_pending", "malformed")
            .await
            .unwrap()
            .is_none()
    );
    assert!(docs.get(DELIVERY_PENDING, "valid").await.unwrap().is_some());
}

#[tokio::test]
async fn delivery_with_mismatched_legacy_ids_is_dropped_without_blocking_open() {
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    let mut mismatched = legacy_delivery("expected");
    mismatched["id"] = json!("different");
    seed(
        docs.as_ref(),
        "channels_delivery_pending",
        "different",
        mismatched,
    )
    .await;

    crate::storage::prepare(&docs).await.unwrap();

    assert!(
        docs.get("channels_delivery_pending", "different")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        docs.get(DELIVERY_PENDING, "different")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn concurrent_owner_opens_complete_one_legacy_migration_safely() {
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    seed(
        docs.as_ref(),
        "channels_delivery_pending",
        "shared",
        legacy_delivery("shared"),
    )
    .await;

    let (first, second) = tokio::join!(
        crate::storage::prepare(&docs),
        crate::storage::prepare(&docs)
    );

    assert!(first.is_ok(), "first opener: {first:?}");
    assert!(second.is_ok(), "concurrent opener: {second:?}");
    assert!(
        docs.get("channels_delivery_pending", "shared")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        docs.get(DELIVERY_PENDING, "shared")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn an_over_capacity_legacy_pending_queue_still_loads_its_oldest_batch() {
    use crate::delivery::DeliveryQueueStore as _;
    let (_backend, storage) = storage().await;
    let docs = storage.documents().clone();
    let over = tinychannels_bus::module_delivery::DELIVERY_PENDING_CAPACITY + 1;
    for index in 0..over {
        let id = format!("op-{index}");
        seed(
            docs.as_ref(),
            "channels_delivery_pending",
            &id,
            legacy_delivery(&id),
        )
        .await;
    }
    let store = crate::storage::DocumentDeliveryStore::new(docs);
    let loaded = store.load_pending_all().await.unwrap_or_else(|error| {
        panic!("{over} migrated legacy pending rows made the whole queue unloadable: {error:?}")
    });
    assert_eq!(loaded.len(), over - 1);
}
