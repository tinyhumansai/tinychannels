//! TinyChannels' owner-local migration runner.
//!
//! The old bus crate exposed these transformations as a centrally composed
//! `Registry`. They belong to the implementation, so `prepare` now executes
//! them against its already-bound document handle and records completion in
//! `channels.migrations`.

use serde::Deserialize;
use serde_json::Value;
use tinystoragedrivers_api::{DocumentStore, DocumentStoreExt};
use tinystoragedrivers_types::{
    CollectionSpec, ErrorKind, Precondition, Query, StorageError, Versioned, WriteOptions,
};

use super::key;
use super::records::{ChatBinding, to_stored_ms};
use super::schema::{
    DELIVERY_FAILED, DELIVERY_PENDING, DELIVERY_RESULTS, LEGACY_DELIVERY_FAILED,
    LEGACY_DELIVERY_PENDING, LEGACY_DELIVERY_RESULTS, LEGACY_REMOTE_BINDINGS, LEGACY_REMOTE_BUSY,
    LEGACY_REMOTE_META, LEGACY_WHATSAPP_DEVICE, LEGACY_WHATSAPP_IDENTITY,
    LEGACY_WHATSAPP_LID_MAPPING, LEGACY_WHATSAPP_PREKEY, LEGACY_WHATSAPP_SENDER_KEY,
    LEGACY_WHATSAPP_SESSION, LEGACY_WHATSAPP_SIGNED_PREKEY, LEGACY_WHATSAPP_SYNC_KEY, MIGRATIONS,
    REMOTE_BINDINGS, REMOTE_BUSY, REMOTE_META, SCHEMA_VERSION, WHATSAPP_DEVICE, WHATSAPP_IDENTITY,
    WHATSAPP_LID_MAPPING, WHATSAPP_PREKEY, WHATSAPP_SENDER_KEY, WHATSAPP_SESSION,
    WHATSAPP_SIGNED_PREKEY, WHATSAPP_SYNC_KEY,
};
use crate::delivery::QueuedDelivery;
use tinychannels_bus::DeliveryTerminalResult;

const OWNER: &str = "tinychannels";
const MAX_SOURCE_PASSES: usize = 3;

#[derive(Clone, Copy)]
enum Transform {
    Delivery,
    Result,
    Binding,
    Busy,
    Marker,
    WhatsApp { unwrap_value: bool },
}

struct Step {
    name: &'static str,
    source: &'static str,
    target: &'static str,
    transform: Transform,
}

struct Target {
    id: String,
    body: Value,
    expires_ms: Option<i64>,
}

fn steps() -> [Step; 14] {
    [
        Step {
            name: "delivery-pending-v2",
            source: LEGACY_DELIVERY_PENDING,
            target: DELIVERY_PENDING,
            transform: Transform::Delivery,
        },
        Step {
            name: "delivery-failed-v2",
            source: LEGACY_DELIVERY_FAILED,
            target: DELIVERY_FAILED,
            transform: Transform::Delivery,
        },
        Step {
            name: "delivery-results-v2",
            source: LEGACY_DELIVERY_RESULTS,
            target: DELIVERY_RESULTS,
            transform: Transform::Result,
        },
        Step {
            name: "remote-bindings-v2",
            source: LEGACY_REMOTE_BINDINGS,
            target: REMOTE_BINDINGS,
            transform: Transform::Binding,
        },
        Step {
            name: "remote-busy-v2",
            source: LEGACY_REMOTE_BUSY,
            target: REMOTE_BUSY,
            transform: Transform::Busy,
        },
        Step {
            name: "remote-meta-v2",
            source: LEGACY_REMOTE_META,
            target: REMOTE_META,
            transform: Transform::Marker,
        },
        Step {
            name: "whatsapp-device-v2",
            source: LEGACY_WHATSAPP_DEVICE,
            target: WHATSAPP_DEVICE,
            transform: Transform::WhatsApp {
                unwrap_value: false,
            },
        },
        Step {
            name: "whatsapp-identity-v2",
            source: LEGACY_WHATSAPP_IDENTITY,
            target: WHATSAPP_IDENTITY,
            transform: Transform::WhatsApp { unwrap_value: true },
        },
        Step {
            name: "whatsapp-session-v2",
            source: LEGACY_WHATSAPP_SESSION,
            target: WHATSAPP_SESSION,
            transform: Transform::WhatsApp { unwrap_value: true },
        },
        Step {
            name: "whatsapp-prekey-v2",
            source: LEGACY_WHATSAPP_PREKEY,
            target: WHATSAPP_PREKEY,
            transform: Transform::WhatsApp { unwrap_value: true },
        },
        Step {
            name: "whatsapp-signed-prekey-v2",
            source: LEGACY_WHATSAPP_SIGNED_PREKEY,
            target: WHATSAPP_SIGNED_PREKEY,
            transform: Transform::WhatsApp { unwrap_value: true },
        },
        Step {
            name: "whatsapp-sender-key-v2",
            source: LEGACY_WHATSAPP_SENDER_KEY,
            target: WHATSAPP_SENDER_KEY,
            transform: Transform::WhatsApp { unwrap_value: true },
        },
        Step {
            name: "whatsapp-sync-key-v2",
            source: LEGACY_WHATSAPP_SYNC_KEY,
            target: WHATSAPP_SYNC_KEY,
            transform: Transform::WhatsApp {
                unwrap_value: false,
            },
        },
        Step {
            name: "whatsapp-lid-mapping-v2",
            source: LEGACY_WHATSAPP_LID_MAPPING,
            target: WHATSAPP_LID_MAPPING,
            transform: Transform::WhatsApp {
                unwrap_value: false,
            },
        },
    ]
}

/// Run every TinyChannels migration against the owner-bound document handle.
pub(super) async fn run(docs: &dyn DocumentStore) -> Result<(), StorageError> {
    docs.ensure_collection(&CollectionSpec::new(MIGRATIONS))
        .await?;
    for step in steps() {
        docs.ensure_collection(&CollectionSpec::new(step.source))
            .await?;
        let marker_id = key::of([OWNER, step.name])?;
        if let Some(marker) = docs.get(MIGRATIONS, marker_id.as_str()).await? {
            if marker.doc
                == serde_json::json!({
                    "owner": OWNER,
                    "step": step.name,
                    "from": 1,
                    "to": SCHEMA_VERSION,
                    "done": true,
                })
            {
                continue;
            }
            return Err(StorageError::conflict(
                "TinyChannels migration marker is malformed",
            ));
        }
        for pass in 0..=MAX_SOURCE_PASSES {
            // Re-read the full source after every pass. This makes completion
            // contingent on observing an empty source, including records
            // added while an earlier page was being copied.
            let sources = docs.query_all(step.source, &Query::all()).await?;
            if sources.is_empty() {
                break;
            }
            if pass == MAX_SOURCE_PASSES {
                return Err(StorageError::conflict(format!(
                    "TinyChannels migration source {} kept changing; quiesce legacy writers and retry",
                    step.source
                )));
            }
            for source in sources {
                match transform(step.transform, source.clone())? {
                    Some(mut target) => {
                        // Carry the source's absolute deadline unless the
                        // transform explicitly supplies a domain TTL (as the
                        // delivery-result transform does).
                        if target.expires_ms.is_none() {
                            target.expires_ms = source.meta.expires_ms;
                        }
                        copy_then_delete(docs, &step, &source, target).await?;
                    }
                    None => {
                        // The archived TSD v1 runner wrote only transform
                        // outputs, then deleted the source collection after
                        // all batches completed. A filtered-out malformed row
                        // was therefore deliberately discarded.
                        delete_source(docs, step.source, source).await?;
                    }
                }
            }
        }
        let marker = serde_json::json!({
            "owner": OWNER,
            "step": step.name,
            "from": 1,
            "to": SCHEMA_VERSION,
            "done": true,
        });
        match docs
            .put(
                MIGRATIONS,
                marker_id.as_str(),
                marker.clone(),
                Precondition::Absent,
            )
            .await
        {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::Conflict => {
                let current = docs.get(MIGRATIONS, marker_id.as_str()).await?;
                if current.is_none_or(|current| current.doc != marker) {
                    return Err(StorageError::conflict(
                        "TinyChannels migration marker changed",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

async fn copy_then_delete(
    docs: &dyn DocumentStore,
    step: &Step,
    source: &Versioned<Value>,
    target: Target,
) -> Result<(), StorageError> {
    let options = match target.expires_ms {
        Some(expires_ms) => WriteOptions::new()
            .schema(SCHEMA_VERSION)
            .expires_ms(expires_ms),
        None => WriteOptions::new().schema(SCHEMA_VERSION),
    };
    match docs
        .put_with(
            step.target,
            &target.id,
            target.body.clone(),
            Precondition::Absent,
            options,
        )
        .await
    {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::Conflict => {
            let Some(current) = docs.get(step.target, &target.id).await? else {
                return Err(StorageError::conflict(
                    "TinyChannels migration target raced",
                ));
            };
            if current.doc != target.body
                || current.meta.schema != SCHEMA_VERSION
                || current.meta.expires_ms != target.expires_ms
            {
                return Err(StorageError::conflict(format!(
                    "TinyChannels migration target conflict in {}",
                    step.target
                )));
            }
        }
        Err(error) => return Err(error),
    }
    // A successful return can still be followed by a lost acknowledgement or
    // a concurrent replacement. Never remove the legacy source until the
    // exact expected target is durable and readable.
    let Some(current) = docs.get(step.target, &target.id).await? else {
        return Err(StorageError::conflict(
            "TinyChannels migration target was not durable",
        ));
    };
    if current.doc != target.body
        || current.meta.schema != SCHEMA_VERSION
        || current.meta.expires_ms != target.expires_ms
    {
        return Err(StorageError::conflict(format!(
            "TinyChannels migration target changed in {}",
            step.target
        )));
    }
    delete_source(docs, step.source, source.clone()).await
}

async fn delete_source(
    docs: &dyn DocumentStore,
    collection: &str,
    source: Versioned<Value>,
) -> Result<(), StorageError> {
    match docs
        .delete(collection, &source.id, source.unchanged())
        .await
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::Conflict => {
            if docs.get(collection, &source.id).await?.is_none() {
                // Another concurrent opener completed the same conditional
                // delete after writing an equivalent target.
                Ok(())
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

fn transform(kind: Transform, record: Versioned<Value>) -> Result<Option<Target>, StorageError> {
    let id = record.id.clone();
    let body = record.doc.clone();
    let converted = match kind {
        Transform::Delivery => {
            let Ok(entry) = serde_json::from_value::<QueuedDelivery>(body) else {
                return Ok(None);
            };
            if entry.id.is_empty() || entry.intent.idempotency_key != entry.id {
                return Ok(None);
            }
            Some(Target {
                id: key::of([entry.id.as_str()])?.as_str().to_owned(),
                body: serde_json::to_value(super::records::stored_delivery_from_queued(&entry))?,
                expires_ms: None,
            })
        }
        Transform::Result => {
            let Ok(legacy) = serde_json::from_value::<LegacyResult>(body) else {
                return Ok(None);
            };
            Some(Target {
                id: key::of([legacy.result.id.as_str()])?.as_str().to_owned(),
                body: serde_json::to_value(super::records::stored_result_from_terminal(
                    &legacy.result,
                ))?,
                expires_ms: Some(to_stored_ms(legacy.expires_at_unix_ms)),
            })
        }
        Transform::Binding => {
            let Some((channel, target_id)) = id.split_once(':') else {
                return Ok(None);
            };
            let Ok(legacy) = serde_json::from_value::<LegacyBinding>(body) else {
                return Ok(None);
            };
            let body = serde_json::to_value(legacy.binding)?;
            // The v1 updated_at field described last-write time. Current
            // consumers fetch bindings by key and do not order by this field;
            // the bus also deliberately forbids writes to host-owned
            // created_ms metadata, so this legacy timestamp is not retained.
            Some(Target {
                id: key::of([channel, target_id])?.as_str().to_owned(),
                body,
                expires_ms: None,
            })
        }
        Transform::Busy => {
            let Some((channel, target_id)) = id.split_once(':') else {
                return Ok(None);
            };
            Some(Target {
                id: key::of([channel, target_id])?.as_str().to_owned(),
                body: serde_json::json!({"busy": true}),
                expires_ms: None,
            })
        }
        Transform::Marker => Some(Target {
            id,
            body,
            expires_ms: None,
        }),
        Transform::WhatsApp { unwrap_value } => {
            let fields = match (&body["account"], &body["key"]) {
                (Value::String(account), Value::String(key)) => {
                    Some((account.clone(), key.clone()))
                }
                _ => record
                    .id
                    .split_once(':')
                    .map(|(account, key)| (account.to_owned(), key.to_owned())),
            };
            let Some((account, key_part)) = fields else {
                return Ok(None);
            };
            let Some(mut object) = body.as_object().cloned() else {
                return Ok(None);
            };
            object.insert("account".into(), Value::String(account.clone()));
            object.insert("key".into(), Value::String(key_part.clone()));
            if unwrap_value && let Some(bytes) = object.remove("value") {
                object.insert("data_b64".into(), bytes);
            }
            Some(Target {
                id: key::of([account.as_str(), key_part.as_str()])?
                    .as_str()
                    .to_owned(),
                body: Value::Object(object),
                expires_ms: None,
            })
        }
    };
    Ok(converted)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyResult {
    result: DeliveryTerminalResult,
    expires_at_unix_ms: u64,
}

#[derive(Deserialize)]
struct LegacyBinding {
    #[serde(flatten)]
    binding: ChatBinding,
    #[serde(default, rename = "updated_at")]
    _updated_at: Option<String>,
}

#[cfg(test)]
#[path = "migrations_tests.rs"]
mod tests;
