//! Adapter from the module's delivery state machine to the host's storage.
//!
//! The records live in the module's own scoped slice of the host's storage,
//! reached over the storage bus: the same [`DocumentDeliveryStore`] a linked
//! host would build over a store it owns. There is no per-record callback on
//! the host object any more.
use tinychannels::delivery::{
    DeliveryQueueError, DeliveryQueueStore, DeliveryResultStore, QueuedDelivery,
};
use tinychannels::storage::DocumentDeliveryStore;
use tokio::sync::OnceCell;

use crate::host::HostChannels;

/// Delivery store bound to the host's storage. Storage is connected on first
/// use; a host with no storage object is an absent capability
/// ([`DeliveryQueueError::Unsupported`]), never a silent in-memory substitute.
#[derive(Clone)]
pub(crate) struct HostDeliveryStore {
    host: HostChannels,
    store: std::sync::Arc<OnceCell<DocumentDeliveryStore>>,
}

impl HostDeliveryStore {
    pub(crate) fn from_host(host: &HostChannels) -> Self {
        Self {
            host: host.clone(),
            store: std::sync::Arc::new(OnceCell::new()),
        }
    }

    async fn inner(&self) -> Result<&DocumentDeliveryStore, DeliveryQueueError> {
        self.store
            .get_or_try_init(|| async {
                self.host
                    .documents()
                    .await
                    .map(DocumentDeliveryStore::from_prepared)
                    .map_err(storage_error)
            })
            .await
    }
}

#[async_trait::async_trait]
impl DeliveryQueueStore for HostDeliveryStore {
    async fn save_pending(&self, entry: QueuedDelivery) -> Result<(), DeliveryQueueError> {
        self.inner().await?.save_pending(entry).await
    }

    async fn load_pending(&self, id: &str) -> Result<Option<QueuedDelivery>, DeliveryQueueError> {
        self.inner().await?.load_pending(id).await
    }

    async fn load_pending_all(&self) -> Result<Vec<QueuedDelivery>, DeliveryQueueError> {
        self.inner().await?.load_pending_all().await
    }

    async fn remove_pending(&self, id: &str) -> Result<(), DeliveryQueueError> {
        self.inner().await?.remove_pending(id).await
    }

    async fn save_failed(&self, entry: QueuedDelivery) -> Result<(), DeliveryQueueError> {
        self.inner().await?.save_failed(entry).await
    }
}

#[async_trait::async_trait]
impl DeliveryResultStore for HostDeliveryStore {
    async fn load_failed(&self, id: &str) -> Result<Option<QueuedDelivery>, DeliveryQueueError> {
        self.inner().await?.load_failed(id).await
    }

    async fn load_result(
        &self,
        id: &str,
    ) -> Result<Option<tinychannels_bus::DeliveryTerminalResult>, DeliveryQueueError> {
        self.inner().await?.load_result(id).await
    }

    async fn save_result(
        &self,
        result: tinychannels_bus::DeliveryTerminalResult,
    ) -> Result<(), DeliveryQueueError> {
        self.inner().await?.save_result(result).await
    }
}

fn storage_error(error: tinystoragedrivers_types::StorageError) -> DeliveryQueueError {
    if error.kind() == tinystoragedrivers_types::ErrorKind::Unavailable {
        // The host serves no storage object: an absent capability, not a
        // broken store. Required delivery still refuses Unsupported before any
        // provider I/O, and nothing is substituted in memory.
        DeliveryQueueError::Unsupported
    } else {
        DeliveryQueueError::Store(format!("host storage: {:?}", error.kind()))
    }
}
