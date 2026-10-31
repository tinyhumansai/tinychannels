//! Durable scoped media references backed by the host's existing blob store.

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tinystoragedrivers_api::{BlobReader, BlobStore, DocumentStore};
use tinystoragedrivers_types::{Filter, Precondition, Query, Version};

use crate::storage::schema::media_metadata_spec;
use tinychannels_bus::storage::{MEDIA_BLOB_PREFIX, MEDIA_METADATA as MEDIA_COLLECTION};
const MEDIA_KEY_PREFIX: &str = MEDIA_BLOB_PREFIX;
const OPAQUE_ID_PREFIX: &str = "tinychannels_media_v1_";

/// The identity required to store or resolve media for one channel turn.
pub type MediaScope = tinychannels_bus::HostVoiceScope;

/// A stored media reference and its bounded bytes when read for native upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMedia {
    pub media: tinychannels_bus::MediaReference,
    pub content_type: String,
    pub file_name: Option<String>,
    pub bytes: Vec<u8>,
}

/// Scope and metadata that accompany a media body through the shared store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaMetadata {
    pub scope: MediaScope,
    pub kind: tinychannels_bus::MediaKind,
    pub content_type: String,
    pub file_name: Option<String>,
    pub duration_ms: Option<u64>,
    pub codec: Option<String>,
    pub voice_direction: Option<tinychannels_bus::HostVoiceMediaDirection>,
    pub dedupe_key: Option<String>,
    pub max_bytes: u64,
}

/// A bounded in-memory media fixture or native upload body to persist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaStoreRequest {
    pub metadata: MediaMetadata,
    pub bytes: Vec<u8>,
}

/// An untrusted streaming media body to persist under the configured limit.
pub struct MediaStreamRequest {
    pub metadata: MediaMetadata,
    pub reader: BlobReader,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MediaRecord {
    scope: MediaScope,
    media: tinychannels_bus::MediaReference,
    content_type: String,
    file_name: Option<String>,
    codec: Option<String>,
    digest: String,
    storage_key: String,
    voice_direction: Option<tinychannels_bus::HostVoiceMediaDirection>,
    created_at_unix_ms: u64,
    state: MediaRecordState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MediaRecordState {
    Writing,
    Ready,
    Deleting,
}

/// Blob-backed storage that validates the complete channel scope on every read.
pub struct ScopedMediaStore {
    documents: Arc<dyn DocumentStore>,
    blobs: Arc<dyn BlobStore>,
    writes: tokio::sync::Mutex<()>,
}

impl ScopedMediaStore {
    /// Bind the store to the module's already scoped host storage handles.
    pub fn new(documents: Arc<dyn DocumentStore>, blobs: Arc<dyn BlobStore>) -> Self {
        Self {
            documents,
            blobs,
            writes: tokio::sync::Mutex::new(()),
        }
    }

    /// Persist bounded media and return its durable opaque reference.
    pub async fn store(
        &self,
        request: MediaStoreRequest,
    ) -> anyhow::Result<tinychannels_bus::MediaReference> {
        let MediaStoreRequest { metadata, bytes } = request;
        let MediaMetadata {
            scope,
            kind,
            content_type,
            file_name,
            duration_ms,
            codec,
            voice_direction,
            dedupe_key,
            max_bytes,
        } = metadata;
        validate_scope(&scope)?;
        if bytes.is_empty() || bytes.len() as u64 > max_bytes {
            anyhow::bail!("media exceeds the configured size limit");
        }
        let content_type = normalized_content_type(&content_type);
        validate_content_type(kind, &content_type, &bytes)?;
        self.documents
            .ensure_collection(&media_metadata_spec())
            .await
            .map_err(|_| anyhow::anyhow!("media metadata storage is unavailable"))?;

        let digest = digest(&bytes);
        let opaque_id = match dedupe_key.as_deref() {
            Some(key) if !key.trim().is_empty() => scoped_id(&scope, key)?,
            _ => format!("{OPAQUE_ID_PREFIX}{}", uuid::Uuid::new_v4()),
        };
        let key = blob_key(&opaque_id)?;
        let storage_key = format!("{key}/{}", uuid::Uuid::new_v4());
        let file_name = file_name.and_then(safe_file_name);
        let reference = tinychannels_bus::MediaReference {
            opaque_id: Some(opaque_id.clone()),
            content_type: Some(content_type.clone()),
            file_name: file_name.clone(),
            size_bytes: Some(bytes.len() as u64),
            duration_ms,
            codec: codec.clone(),
            kind,
            ..Default::default()
        };
        let record = MediaRecord {
            scope,
            media: reference.clone(),
            content_type: content_type.clone(),
            file_name,
            codec,
            digest: digest.clone(),
            storage_key: storage_key.clone(),
            voice_direction,
            created_at_unix_ms: now_unix_ms(),
            state: MediaRecordState::Writing,
        };

        let _write = self.writes.lock().await;
        if let Some((existing, _)) = self.record(&opaque_id).await? {
            if existing.state == MediaRecordState::Ready
                && existing.scope == record.scope
                && existing.digest == digest
                && existing.media.kind == record.media.kind
                && existing.content_type == record.content_type
                && existing.media.duration_ms == record.media.duration_ms
                && existing.file_name == record.file_name
                && existing.codec == record.codec
                && existing.voice_direction == record.voice_direction
            {
                return Ok(existing.media);
            }
            anyhow::bail!("media deduplication key conflicts with stored media");
        }

        // Reserve the metadata id before writing the blob. The bus storage
        // grant is module-wide; this record also binds the id to the complete
        // authenticated account/conversation/turn scope.
        let document = serde_json::to_value(&record)?;
        let reservation_version = self
            .documents
            .put(MEDIA_COLLECTION, &opaque_id, document, Precondition::Absent)
            .await
            .map_err(|_| anyhow::anyhow!("media metadata reservation failed"))?;

        if self
            .blobs
            .put_stream(
                &storage_key,
                Box::pin(futures_util::stream::once(async move { Ok(bytes.into()) })),
                record.media.size_bytes.unwrap_or_default(),
                Some(&content_type),
            )
            .await
            .is_err()
        {
            let mut deleting = record.clone();
            deleting.state = MediaRecordState::Deleting;
            if let Ok(version) = self
                .documents
                .put(
                    MEDIA_COLLECTION,
                    &opaque_id,
                    serde_json::to_value(&deleting)?,
                    Precondition::Version(reservation_version),
                )
                .await
                && self.blobs.delete(&storage_key).await.is_ok()
            {
                let _ = self
                    .documents
                    .delete(MEDIA_COLLECTION, &opaque_id, Precondition::Version(version))
                    .await;
            }
            anyhow::bail!("media blob could not be stored");
        }
        let mut ready = record;
        ready.state = MediaRecordState::Ready;
        if self
            .documents
            .put(
                MEDIA_COLLECTION,
                &opaque_id,
                serde_json::to_value(&ready)?,
                Precondition::Version(reservation_version),
            )
            .await
            .is_err()
        {
            // Blob keys are unique per reservation, so cleaning this attempt
            // cannot erase another writer's committed bytes.
            let _ = self.blobs.delete(&storage_key).await;
            anyhow::bail!("media metadata could not be finalized");
        }
        Ok(reference)
    }

    /// Read an untrusted stream incrementally under `max_bytes`, validate its
    /// actual body/MIME, then persist it through the scoped blob port.
    pub async fn store_stream(
        &self,
        request: MediaStreamRequest,
    ) -> anyhow::Result<tinychannels_bus::MediaReference> {
        let MediaStreamRequest { metadata, reader } = request;
        let MediaMetadata {
            scope,
            kind,
            content_type,
            file_name,
            duration_ms,
            codec,
            voice_direction,
            dedupe_key,
            max_bytes,
        } = metadata;
        validate_scope(&scope)?;
        let normalized = normalized_content_type(&content_type);
        if !is_allowed_content_type(kind, &normalized) {
            anyhow::bail!("media content type or body is not allowed");
        }
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            collect_limited(reader, max_bytes),
        )
        .await
        .map_err(|_| anyhow::anyhow!("media download timed out"))??;
        if bytes.is_empty() || bytes.len() as u64 > max_bytes {
            anyhow::bail!("media exceeds the configured size limit");
        }
        self.store(MediaStoreRequest {
            metadata: MediaMetadata {
                scope,
                kind,
                content_type,
                file_name,
                duration_ms,
                codec,
                voice_direction,
                dedupe_key,
                max_bytes,
            },
            bytes,
        })
        .await
    }

    /// Resolve a durable reference only for the scope that stored it.
    pub async fn read(
        &self,
        scope: MediaScope,
        opaque_id: &str,
        max_bytes: u64,
    ) -> anyhow::Result<StoredMedia> {
        validate_scope(&scope)?;
        let (record, _) = self
            .record(opaque_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("media reference is unavailable"))?;
        if record.scope != scope || record.state != MediaRecordState::Ready {
            anyhow::bail!("media reference is outside the configured channel scope");
        }
        let key = record.storage_key.clone();
        let stream = self
            .blobs
            .get_stream(&key)
            .await
            .map_err(|_| anyhow::anyhow!("media blob is unavailable"))?
            .ok_or_else(|| anyhow::anyhow!("media blob is unavailable"))?;
        if stream.meta.len == 0 || stream.meta.len > max_bytes {
            anyhow::bail!("media exceeds the configured read limit");
        }
        let bytes = collect_limited(stream.reader, max_bytes)
            .await
            .map_err(|_| anyhow::anyhow!("media blob could not be read"))?;
        if bytes.len() as u64 != stream.meta.len
            || bytes.len() as u64 > max_bytes
            || digest(&bytes) != record.digest
        {
            anyhow::bail!("stored media failed integrity or size validation");
        }
        Ok(StoredMedia {
            media: record.media,
            content_type: record.content_type,
            file_name: record.file_name,
            bytes,
        })
    }

    /// Delete one media object after validating its complete stored scope.
    pub async fn delete(&self, scope: MediaScope, opaque_id: &str) -> anyhow::Result<bool> {
        validate_scope(&scope)?;
        let Some((mut record, version)) = self.record(opaque_id).await? else {
            return Ok(false);
        };
        if record.scope != scope {
            anyhow::bail!("media reference is outside the configured channel scope");
        }
        if record.state == MediaRecordState::Writing {
            anyhow::bail!("media write is still in progress");
        }
        let tombstone_version = if record.state == MediaRecordState::Deleting {
            version
        } else {
            record.state = MediaRecordState::Deleting;
            self.documents
                .put(
                    MEDIA_COLLECTION,
                    opaque_id,
                    serde_json::to_value(&record)?,
                    Precondition::Version(version),
                )
                .await
                .map_err(|_| anyhow::anyhow!("media cleanup reservation failed"))?
        };
        self.blobs
            .delete(&record.storage_key)
            .await
            .map_err(|_| anyhow::anyhow!("media blob cleanup is pending"))?;
        self.documents
            .delete(
                MEDIA_COLLECTION,
                opaque_id,
                Precondition::Version(tombstone_version),
            )
            .await
            .map_err(|_| anyhow::anyhow!("media cleanup metadata is pending"))?;
        Ok(true)
    }

    /// Reclaim interrupted writes and deletion tombstones older than the
    /// caller's cutoff. Unique per-attempt blob keys make this safe even if a
    /// timed-out writer later resumes: its metadata CAS will fail and it can
    /// only write its own key.
    pub async fn cleanup_pending(
        &self,
        scope: MediaScope,
        created_before_unix_ms: u64,
        limit: usize,
    ) -> anyhow::Result<usize> {
        validate_scope(&scope)?;
        let filter = Filter::And {
            filters: vec![
                Filter::eq("scope", serde_json::to_value(&scope)?),
                Filter::In {
                    field: "state".into(),
                    values: vec![serde_json::json!("writing"), serde_json::json!("deleting")],
                },
                Filter::Range {
                    field: "created_at_unix_ms".into(),
                    gt: None,
                    gte: None,
                    lt: None,
                    lte: Some(serde_json::json!(created_before_unix_ms)),
                },
            ],
        };
        let page = self
            .documents
            .query(
                MEDIA_COLLECTION,
                &Query::filter(filter).limit(limit.clamp(1, 1000)),
            )
            .await
            .map_err(|_| anyhow::anyhow!("media cleanup scan failed"))?;
        let mut cleaned = 0;
        for document in page.items {
            let Ok(mut record) = serde_json::from_value::<MediaRecord>(document.doc.clone()) else {
                continue;
            };
            let tombstone_version = if record.state == MediaRecordState::Deleting {
                document.version
            } else {
                record.state = MediaRecordState::Deleting;
                match self
                    .documents
                    .put(
                        MEDIA_COLLECTION,
                        record.media.opaque_id.as_deref().unwrap_or_default(),
                        serde_json::to_value(&record)?,
                        Precondition::Version(document.version),
                    )
                    .await
                {
                    Ok(version) => version,
                    Err(_) => continue,
                }
            };
            if self.blobs.delete(&record.storage_key).await.is_err() {
                continue;
            }
            let Some(id) = record.media.opaque_id.as_deref() else {
                continue;
            };
            if self
                .documents
                .delete(
                    MEDIA_COLLECTION,
                    id,
                    Precondition::Version(tombstone_version),
                )
                .await
                .is_ok()
            {
                cleaned += 1;
            }
        }
        Ok(cleaned)
    }

    async fn record(&self, opaque_id: &str) -> anyhow::Result<Option<(MediaRecord, Version)>> {
        blob_key(opaque_id)?;
        let Some(document) = self
            .documents
            .get(MEDIA_COLLECTION, opaque_id)
            .await
            .map_err(|_| anyhow::anyhow!("media metadata is unavailable"))?
        else {
            return Ok(None);
        };
        let record = serde_json::from_value(document.doc)
            .map_err(|_| anyhow::anyhow!("media metadata is invalid"))?;
        Ok(Some((record, document.version)))
    }
}

#[async_trait::async_trait]
impl super::MediaRepository for ScopedMediaStore {
    async fn store_media(
        &self,
        request: MediaStoreRequest,
    ) -> anyhow::Result<tinychannels_bus::MediaReference> {
        self.store(request).await
    }

    async fn read_media(
        &self,
        scope: MediaScope,
        opaque_id: String,
        max_bytes: u64,
    ) -> anyhow::Result<StoredMedia> {
        self.read(scope, &opaque_id, max_bytes).await
    }

    async fn store_media_stream(
        &self,
        request: MediaStreamRequest,
    ) -> anyhow::Result<tinychannels_bus::MediaReference> {
        self.store_stream(request).await
    }

    async fn delete_media(&self, scope: MediaScope, opaque_id: String) -> anyhow::Result<bool> {
        self.delete(scope, &opaque_id).await
    }
}

fn validate_scope(scope: &MediaScope) -> anyhow::Result<()> {
    if scope.configured_instance.trim().is_empty()
        || scope.channel.id.trim().is_empty()
        || scope
            .channel
            .account_id
            .as_deref()
            .is_none_or(|account_id| account_id.trim().is_empty())
        || scope.conversation.id.trim().is_empty()
        || scope.turn_id.trim().is_empty()
    {
        anyhow::bail!("media storage requires configured account, conversation, and turn scope");
    }
    Ok(())
}

async fn collect_limited(mut reader: BlobReader, max_bytes: u64) -> anyhow::Result<Vec<u8>> {
    let capacity = usize::try_from(max_bytes.min(64 * 1024)).unwrap_or(64 * 1024);
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = reader.try_next().await? {
        if (bytes.len() as u64).saturating_add(chunk.len() as u64) > max_bytes {
            anyhow::bail!("media exceeds the configured size limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn scoped_id(scope: &MediaScope, dedupe_key: &str) -> anyhow::Result<String> {
    let encoded = serde_json::to_vec(scope)?;
    let mut hash = Sha256::new();
    hash.update(b"tinychannels-media-v1\0");
    hash.update((encoded.len() as u64).to_le_bytes());
    hash.update(encoded);
    hash.update((dedupe_key.len() as u64).to_le_bytes());
    hash.update(dedupe_key.as_bytes());
    Ok(format!(
        "{OPAQUE_ID_PREFIX}{}",
        hex::encode(hash.finalize())
    ))
}

fn blob_key(opaque_id: &str) -> anyhow::Result<String> {
    let suffix = opaque_id.strip_prefix(OPAQUE_ID_PREFIX).unwrap_or_default();
    if !(suffix.len() == 36 || suffix.len() == 64)
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        anyhow::bail!("media reference is invalid");
    }
    Ok(format!("{MEDIA_KEY_PREFIX}{opaque_id}"))
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn safe_file_name(name: String) -> Option<String> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 255
        || name.contains(['/', '\\', '\0'])
        || name == "."
        || name == ".."
    {
        None
    } else {
        Some(name.to_owned())
    }
}

fn normalized_content_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn validate_content_type(
    kind: tinychannels_bus::MediaKind,
    content_type: &str,
    bytes: &[u8],
) -> anyhow::Result<()> {
    if !is_allowed_content_type(kind, content_type) || !content_matches(content_type, bytes) {
        anyhow::bail!("media content type or body is not allowed");
    }
    Ok(())
}

pub(crate) fn is_allowed_content_type(
    kind: tinychannels_bus::MediaKind,
    content_type: &str,
) -> bool {
    match kind {
        tinychannels_bus::MediaKind::Image => {
            matches!(
                content_type,
                "image/jpeg" | "image/png" | "image/gif" | "image/webp"
            )
        }
        tinychannels_bus::MediaKind::Video => {
            matches!(content_type, "video/mp4" | "video/quicktime" | "video/webm")
        }
        tinychannels_bus::MediaKind::Audio => matches!(
            content_type,
            "audio/amr" | "audio/ogg" | "audio/mpeg" | "audio/mp4" | "audio/wav" | "audio/webm"
        ),
        tinychannels_bus::MediaKind::Document => matches!(
            content_type,
            "application/pdf"
                | "text/plain"
                | "text/csv"
                | "application/json"
                | "application/zip"
                | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        ),
        tinychannels_bus::MediaKind::Unknown => false,
    }
}

fn content_matches(content_type: &str, bytes: &[u8]) -> bool {
    match content_type {
        "audio/amr" => bytes.starts_with(b"#!AMR\n"),
        "image/png" => {
            bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                && bytes.len() >= 45
                && bytes.ends_with(b"IEND\xaeB`\x82")
        }
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        "video/mp4" | "video/quicktime" => bytes.get(4..8) == Some(b"ftyp"),
        "video/webm" | "audio/webm" => bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
        "audio/ogg" => bytes.starts_with(b"OggS"),
        "audio/mpeg" => bytes.starts_with(b"ID3") || valid_mpeg_audio_header(bytes),
        "audio/mp4" => bytes.get(4..8) == Some(b"ftyp"),
        "audio/wav" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE"),
        "application/pdf" => bytes.starts_with(b"%PDF-"),
        "application/zip"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            bytes.starts_with(b"PK\x03\x04")
        }
        "text/plain" | "text/csv" | "application/json" => {
            std::str::from_utf8(bytes).is_ok() && !bytes.contains(&0)
        }
        _ => false,
    }
}

fn valid_mpeg_audio_header(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(..4) else {
        return false;
    };
    let [sync, second, third, _] = [header[0], header[1], header[2], header[3]];
    let version = (second >> 3) & 0b11;
    let layer = (second >> 1) & 0b11;
    let bitrate_index = third >> 4;
    let sample_rate_index = (third >> 2) & 0b11;
    sync == 0xff
        && second & 0b1110_0000 == 0b1110_0000
        && version != 0b01
        && layer != 0
        && bitrate_index != 0b1111
        && sample_rate_index != 0b11
}

#[cfg(test)]
#[path = "media_tests.rs"]
mod tests;
