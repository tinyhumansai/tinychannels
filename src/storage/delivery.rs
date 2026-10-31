//! The required-delivery queue on the document port.
//!
//! One implementation serves a linked host (a [`DocumentStore`] it owns) and
//! the loaded module (the same trait over the storage bus). Three collections
//! hold the state machine's records:
//!
//! | collection | id | document |
//! | --- | --- | --- |
//! | `channels.delivery_pending` | `Key::of([idempotency key])` | the [`QueuedDelivery`] |
//! | `channels.delivery_failed` | `Key::of([idempotency key])` | the [`QueuedDelivery`] |
//! | `channels.delivery_results` | `Key::of([idempotency key])` | terminal result, expiring through record metadata |
//!
//! Bodies are the storage DTOs of [`super::records`] (snake_case, `*_ms`), not
//! the camelCase bus types, which are unchanged on the wire.
//!
//! Writes that must not race use the port's preconditions: a new key is
//! inserted with `Absent`, an update is a compare-and-swap on the version read.
//! The record bounds the old host callbacks enforced (record size, pending
//! capacity, result capacity and lifetime) are enforced here, in the module,
//! before any I/O.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tinychannels_bus::DeliveryTerminalResult;
use tinychannels_bus::module_delivery::{
    DELIVERY_PENDING_CAPACITY, DELIVERY_RECORD_BYTES, DELIVERY_RESULT_CAPACITY,
    DELIVERY_RESULT_TTL_SECONDS,
};
use tinystoragedrivers_api::{DocumentStore, DocumentStoreExt};
use tinystoragedrivers_types::{
    EXPIRES_FIELD, ErrorKind, Filter, Precondition, Query, Sort, StorageError, Versioned,
    WriteOptions,
};

use super::records::{
    StoredDelivery, StoredResult, queued_from_stored_delivery, stored_delivery_from_queued,
    stored_result_from_terminal, terminal_result_from_stored, to_stored_ms,
};
use super::schema::{self, SCHEMA_VERSION};
use crate::delivery::{
    DeliveryQueueError, DeliveryQueueStore, DeliveryResultStore, QueuedDelivery,
};

const LOG_PREFIX: &str = "[channel-delivery-store]";

/// Collection of entries awaiting a terminal outcome.
pub const PENDING_COLLECTION: &str = schema::DELIVERY_PENDING;
/// Collection of permanently failed entries.
pub const FAILED_COLLECTION: &str = schema::DELIVERY_FAILED;
/// Collection of idempotent terminal results.
pub const RESULTS_COLLECTION: &str = schema::DELIVERY_RESULTS;

/// Compare-and-swap attempts before an update reports contention.
const CAS_ATTEMPTS: usize = 4;
const MAX_ID_BYTES: usize = 256;

/// Serializes the count-then-insert of a new pending entry so concurrent
/// writers (including separate store handles) cannot both pass the capacity
/// check and overshoot [`DELIVERY_PENDING_CAPACITY`]. The module is the single
/// writer of its scope, so an in-process lock is the reservation.
static PENDING_INSERT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// [`DeliveryQueueStore`] and [`DeliveryResultStore`] over a [`DocumentStore`].
#[derive(Clone, Debug)]
pub struct DocumentDeliveryStore {
    docs: Arc<dyn DocumentStore>,
    ready: Arc<tokio::sync::OnceCell<()>>,
}

impl DocumentDeliveryStore {
    /// A store over `docs`. Collections are declared on first use.
    #[must_use]
    pub fn new(docs: Arc<dyn DocumentStore>) -> Self {
        Self {
            docs,
            ready: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    /// Use an owner-bound handle whose schema and legacy import are already
    /// prepared by the caller. Module callback ports cannot mutate schemas.
    #[must_use]
    pub fn from_prepared(docs: Arc<dyn DocumentStore>) -> Self {
        Self {
            docs,
            ready: Arc::new(tokio::sync::OnceCell::new_with(Some(()))),
        }
    }

    async fn docs(&self) -> Result<&Arc<dyn DocumentStore>, DeliveryQueueError> {
        self.ready
            .get_or_try_init(|| async {
                crate::storage::prepare(&self.docs)
                    .await
                    .map_err(store_error)
            })
            .await?;
        Ok(&self.docs)
    }
}

pub(crate) fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn store_error(error: StorageError) -> DeliveryQueueError {
    tracing::warn!("{LOG_PREFIX} storage failure: {:?}", error.kind());
    DeliveryQueueError::Store(super::describe(&error))
}

fn invalid(message: &str) -> DeliveryQueueError {
    DeliveryQueueError::Store(message.into())
}

fn validate_id(id: &str) -> Result<(), DeliveryQueueError> {
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return Err(invalid("delivery id is invalid"));
    }
    Ok(())
}

fn validate_entry(entry: &QueuedDelivery) -> Result<(), DeliveryQueueError> {
    validate_id(&entry.id)?;
    if entry.intent.idempotency_key != entry.id
        || serde_json::to_vec(entry).map_or(true, |bytes| bytes.len() > DELIVERY_RECORD_BYTES)
    {
        return Err(invalid(
            "delivery record is invalid or exceeds the record limit",
        ));
    }
    Ok(())
}

/// The document id of a delivery: the v1-compatible key encoding.
fn doc_id(id: &str) -> Result<String, DeliveryQueueError> {
    validate_id(id)?;
    crate::storage::key::of([id])
        .map(|key| key.as_str().to_owned())
        .map_err(|_| invalid("delivery id is invalid"))
}

/// The write options of every delivery document.
fn write_options() -> WriteOptions {
    WriteOptions::new().schema(SCHEMA_VERSION)
}

fn decode_entry(found: Versioned<Value>) -> Result<QueuedDelivery, DeliveryQueueError> {
    let entry = match serde_json::from_value::<StoredDelivery>(found.doc.clone()) {
        Ok(stored) => queued_from_stored_delivery(stored),
        Err(_) => serde_json::from_value::<QueuedDelivery>(found.doc)
            .map_err(|_| invalid("stored delivery record is unreadable"))?,
    };
    // The id is derived from the intent, so the document must sit under it.
    // Older records used the raw id as their document key; current records
    // hash it so arbitrary idempotency keys remain valid storage identifiers.
    if doc_id(&entry.id).ok().as_deref() != Some(found.id.as_str()) && entry.id != found.id {
        return Err(invalid("stored delivery record is unreadable"));
    }
    validate_entry(&entry)?;
    Ok(entry)
}

fn encode_entry(entry: &QueuedDelivery) -> Result<Value, DeliveryQueueError> {
    serde_json::to_value(stored_delivery_from_queued(entry))
        .map_err(|_| invalid("delivery record could not be encoded"))
}

#[async_trait]
impl DeliveryQueueStore for DocumentDeliveryStore {
    async fn save_pending(&self, entry: QueuedDelivery) -> Result<(), DeliveryQueueError> {
        validate_entry(&entry)?;
        let docs = self.docs().await?;
        let doc = encode_entry(&entry)?;
        let id = doc_id(&entry.id)?;
        let _reservation = PENDING_INSERT.lock().await;
        for _ in 0..CAS_ATTEMPTS {
            let existing = docs
                .get(PENDING_COLLECTION, &id)
                .await
                .map_err(store_error)?;
            let precondition = match existing {
                Some(found) => {
                    let version = found.version;
                    if decode_entry(found)?.intent != entry.intent {
                        return Err(invalid("delivery key conflicts with a different intent"));
                    }
                    Precondition::Version(version)
                }
                None => {
                    let pending = docs
                        .count(PENDING_COLLECTION, &Filter::default())
                        .await
                        .map_err(store_error)?;
                    if pending >= DELIVERY_PENDING_CAPACITY as u64 {
                        return Err(invalid("delivery queue is full"));
                    }
                    Precondition::Absent
                }
            };
            match docs
                .put_with(
                    PENDING_COLLECTION,
                    &id,
                    doc.clone(),
                    precondition,
                    write_options(),
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(error)
                    if matches!(error.kind(), ErrorKind::Conflict | ErrorKind::AlreadyExists) =>
                {
                    tracing::debug!("{LOG_PREFIX} save_pending raced; retrying");
                }
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(invalid("delivery queue is busy"))
    }

    async fn load_pending(&self, id: &str) -> Result<Option<QueuedDelivery>, DeliveryQueueError> {
        let id = doc_id(id)?;
        self.docs()
            .await?
            .get(PENDING_COLLECTION, &id)
            .await
            .map_err(store_error)?
            .map(decode_entry)
            .transpose()
    }

    async fn load_pending_all(&self) -> Result<Vec<QueuedDelivery>, DeliveryQueueError> {
        let query = Query::all().sort(Sort::asc("enqueued_ms"));
        let mut found = self
            .docs()
            .await?
            .query_all(PENDING_COLLECTION, &query)
            .await
            .map_err(store_error)?;
        // A queue over capacity (a legacy migration copies rows uncapped) or
        // an unreadable row must not stop every other entry from draining:
        // take the oldest batch and leave unreadable rows in place.
        // ponytail: unreadable rows still count toward `save_pending`'s
        // capacity; move them to the failed collection if that ever bites.
        found.truncate(DELIVERY_PENDING_CAPACITY);
        Ok(found
            .into_iter()
            .filter_map(|doc| {
                decode_entry(doc)
                    .inspect_err(|_| {
                        tracing::warn!("{LOG_PREFIX} skipping unreadable pending delivery");
                    })
                    .ok()
            })
            .collect())
    }

    async fn remove_pending(&self, id: &str) -> Result<(), DeliveryQueueError> {
        let id = doc_id(id)?;
        self.docs()
            .await?
            .delete(PENDING_COLLECTION, &id, Precondition::None)
            .await
            .map(|_| ())
            .map_err(store_error)
    }

    async fn save_failed(&self, entry: QueuedDelivery) -> Result<(), DeliveryQueueError> {
        validate_entry(&entry)?;
        let doc = encode_entry(&entry)?;
        let id = doc_id(&entry.id)?;
        self.docs()
            .await?
            .put_with(
                FAILED_COLLECTION,
                &id,
                doc,
                Precondition::None,
                write_options(),
            )
            .await
            .map(|_| ())
            .map_err(store_error)
    }
}

#[async_trait]
impl DeliveryResultStore for DocumentDeliveryStore {
    async fn load_failed(&self, id: &str) -> Result<Option<QueuedDelivery>, DeliveryQueueError> {
        let id = doc_id(id)?;
        self.docs()
            .await?
            .get(FAILED_COLLECTION, &id)
            .await
            .map_err(store_error)?
            .map(decode_entry)
            .transpose()
    }

    async fn load_result(
        &self,
        id: &str,
    ) -> Result<Option<DeliveryTerminalResult>, DeliveryQueueError> {
        let id = doc_id(id)?;
        // A result past its `expires_ms` reads as absent.
        let Some(found) = self
            .docs()
            .await?
            .get(RESULTS_COLLECTION, &id)
            .await
            .map_err(store_error)?
        else {
            return Ok(None);
        };
        let record: StoredResult = serde_json::from_value(found.doc.clone())
            .map_err(|_| invalid("stored delivery result is unreadable"))?;
        // The result sits under the id it was saved for; any other id is a stray copy.
        if doc_id(&record.id).ok().as_deref() != Some(found.id.as_str()) {
            return Err(invalid("stored delivery result is unreadable"));
        }
        Ok(Some(terminal_result_from_stored(record)))
    }

    async fn save_result(&self, result: DeliveryTerminalResult) -> Result<(), DeliveryQueueError> {
        validate_id(&result.id)?;
        if serde_json::to_vec(&result).map_or(true, |bytes| bytes.len() > DELIVERY_RECORD_BYTES) {
            return Err(invalid("delivery result exceeds the record limit"));
        }
        let docs = self.docs().await?;
        let expires_ms = to_stored_ms(
            result
                .completed_at_unix_ms
                .saturating_add(DELIVERY_RESULT_TTL_SECONDS.saturating_mul(1000)),
        );
        let id = doc_id(&result.id)?;
        let doc = serde_json::to_value(stored_result_from_terminal(&result))
            .map_err(|_| invalid("result could not be encoded"))?;
        docs.put_with(
            RESULTS_COLLECTION,
            &id,
            doc,
            Precondition::None,
            write_options().expires_ms(expires_ms),
        )
        .await
        .map_err(store_error)?;
        self.trim_results(&id).await
    }
}

impl DocumentDeliveryStore {
    /// Drop expired results, then the oldest beyond the capacity. The record
    /// just written is never the one evicted.
    async fn trim_results(&self, keep: &str) -> Result<(), DeliveryQueueError> {
        let docs = self.docs().await?;
        docs.delete_where(
            RESULTS_COLLECTION,
            &Filter::lte(EXPIRES_FIELD, to_stored_ms(now_unix_ms())),
        )
        .await
        .map_err(store_error)?;
        let query = Query::all().sort(Sort::asc("completed_ms"));
        let results = docs
            .query_all(RESULTS_COLLECTION, &query)
            .await
            .map_err(store_error)?;
        let excess = results.len().saturating_sub(DELIVERY_RESULT_CAPACITY);
        for old in results.into_iter().filter(|r| r.id != keep).take(excess) {
            docs.delete(RESULTS_COLLECTION, &old.id, Precondition::None)
                .await
                .map_err(store_error)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;
