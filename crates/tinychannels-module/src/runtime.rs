//! Bounded runtime leases with immutable callback destinations.
use crate::host::HostChannels;
use crate::service::Channels;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tinybus::{BusName, Connection, Error, Result};
use tinychannels_bus::{ChannelRuntimeReservation, HOST_OBJECT_PATH, OBJECT_PATH};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::Instant;

const CAPACITY: usize = 16;
const RESERVATION_LIFETIME: Duration = Duration::from_secs(60);
const JOIN_BUDGET: Duration = Duration::from_secs(10);

struct Slot {
    reservation: ChannelRuntimeReservation,
    expires: Instant,
    canceled: AtomicBool,
    activated: AtomicBool,
    child: Mutex<Option<Arc<Channels>>>,
    worker: Mutex<Option<JoinHandle<Result<()>>>>,
    activation_result: Mutex<Option<bool>>,
    dispatch_limit: Mutex<Option<Option<usize>>>,
}

/// Owns separate provider, relay and delivery resources for each host identity.
pub(crate) struct Runtimes {
    connection: Connection,
    storage_callback: Option<BusName>,
    terminal: AtomicBool,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

impl Runtimes {
    #[cfg(test)]
    pub(crate) fn new(connection: Connection) -> Self {
        Self::new_with_storage_callback(connection, None)
    }

    pub(crate) fn new_with_storage_callback(
        connection: Connection,
        storage_callback: Option<BusName>,
    ) -> Self {
        Self {
            connection,
            storage_callback,
            terminal: AtomicBool::new(false),
            slots: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn reserve(&self) -> Result<ChannelRuntimeReservation> {
        let mut slots = self.slots.lock().await;
        if self.terminal.load(Ordering::SeqCst) {
            return Err(closed());
        }
        // Only inert reservations expire; a started owner is never evicted.
        slots.retain(|_, slot| {
            slot.activated.load(Ordering::SeqCst) || slot.expires > Instant::now()
        });
        if slots.len() >= CAPACITY {
            return Err(Error::failed("channel runtime capacity is occupied"));
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let reservation = ChannelRuntimeReservation {
            handle: id.clone(),
            object_path: format!("{OBJECT_PATH}/runtime_{id}"),
            host_object_path: format!("{HOST_OBJECT_PATH}/runtime_{id}"),
        };
        slots.insert(
            id,
            Arc::new(Slot {
                reservation: reservation.clone(),
                expires: Instant::now() + RESERVATION_LIFETIME,
                canceled: AtomicBool::new(false),
                activated: AtomicBool::new(false),
                child: Mutex::new(None),
                worker: Mutex::new(None),
                activation_result: Mutex::new(None),
                dispatch_limit: Mutex::new(None),
            }),
        );
        Ok(reservation)
    }

    async fn get(&self, id: &str) -> Result<Arc<Slot>> {
        self.slots.lock().await.get(id).cloned().ok_or_else(unknown)
    }

    /// Activate only after the host has installed the reserved callback object.
    /// Dropping the reply waiter leaves startup owned by the retained worker.
    pub(crate) async fn start(&self, id: String) -> Result<()> {
        self.start_with_limit(id, None).await
    }

    pub(crate) async fn start_with_dispatch(
        &self,
        id: String,
        policy: tinychannels_bus::ChannelRuntimeDispatchPolicy,
    ) -> Result<()> {
        let count = usize::try_from(policy.listener_count)
            .map_err(|_| Error::failed("listener count exceeds this platform's capacity"))?;
        self.start_with_limit(
            id,
            Some(tinychannels::runtime::compute_max_in_flight_messages(count)),
        )
        .await
    }

    async fn start_with_limit(&self, id: String, limit: Option<usize>) -> Result<()> {
        let slot = self.get(&id).await?;
        let mut worker = slot.worker.lock().await;
        if self.terminal.load(Ordering::SeqCst) || slot.canceled.load(Ordering::SeqCst) {
            return Err(closed());
        }
        let mut policy = slot.dispatch_limit.lock().await;
        if let Some(previous) = *policy {
            if previous != limit {
                return Err(Error::failed("runtime dispatch policy is immutable"));
            }
        } else {
            *policy = Some(limit);
        }
        drop(policy);
        if let Some(result) = *slot.activation_result.lock().await {
            return if result {
                Ok(())
            } else {
                Err(activation_failed())
            };
        }
        if worker.is_none() {
            if slot.expires <= Instant::now() {
                return Err(unknown());
            }
            // Serialize marking active with the registry's expiry scan.
            let slots = self.slots.lock().await;
            if !slots
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &slot))
            {
                return Err(unknown());
            }
            slot.activated.store(true, Ordering::SeqCst);
            drop(slots);
            let connection = self.connection.clone();
            let storage_callback = self.storage_callback.clone();
            let owned = slot.clone();
            *worker = Some(tokio::spawn(async move {
                let host = match limit {
                    Some(limit) => HostChannels::new_at_with_dispatch_and_storage_callback(
                        connection.clone(),
                        owned.reservation.host_object_path.clone(),
                        limit,
                        storage_callback,
                    ),
                    None => HostChannels::new_at_with_storage_callback(
                        connection.clone(),
                        owned.reservation.host_object_path.clone(),
                        storage_callback,
                    ),
                };
                let child = Arc::new(Channels::new(host, None));
                *owned.child.lock().await = Some(child.clone());
                let result = async {
                    if owned.canceled.load(Ordering::SeqCst) {
                        return Err(closed());
                    }
                    connection
                        .serve_at(owned.reservation.object_path.clone().try_into()?, child)
                        .await
                }
                .await;
                *owned.activation_result.lock().await = Some(result.is_ok());
                result
            }));
        }
        let result =
            tokio::time::timeout(JOIN_BUDGET, worker.as_mut().ok_or_else(activation_failed)?)
                .await
                .map_err(|_| {
                    Error::failed("channel runtime startup is pending; retry StartRuntime")
                })?;
        worker.take();
        result.map_err(|_| activation_failed())?
    }

    /// Cancel startup, join native shutdown, then remove only this object's path.
    pub(crate) async fn stop(&self, id: String) -> Result<()> {
        let slot = match self.get(&id).await {
            Ok(slot) => slot,
            Err(_) => return Ok(()),
        };
        slot.canceled.store(true, Ordering::SeqCst);
        let mut worker = slot.worker.lock().await;
        if let Some(task) = worker.as_mut() {
            // Activation failure still owns a child that must be drained.
            let _ = tokio::time::timeout(JOIN_BUDGET, task).await.map_err(|_| {
                Error::failed("channel runtime startup cleanup is pending; retry StopRuntime")
            })?;
            worker.take();
        }
        if let Some(child) = slot.child.lock().await.as_ref() {
            Box::pin(child.shutdown()).await?;
        }
        let path = slot.reservation.object_path.clone().try_into()?;
        self.connection.unserve(&path).await;
        let mut slots = self.slots.lock().await;
        if slots
            .get(&id)
            .is_some_and(|current| Arc::ptr_eq(current, &slot))
        {
            slots.remove(&id);
        }
        Ok(())
    }

    pub(crate) async fn shutdown(&self) -> Result<()> {
        self.terminal.store(true, Ordering::SeqCst);
        let ids: Vec<_> = self.slots.lock().await.keys().cloned().collect();
        let mut failure = None;
        for id in ids {
            if let Err(error) = self.stop(id).await {
                failure.get_or_insert(error);
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn closed() -> Error {
    Error::failed("channel runtime is closed")
}
fn unknown() -> Error {
    Error::failed("unknown or expired channel runtime")
}
fn activation_failed() -> Error {
    Error::failed("channel runtime activation failed")
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
#[allow(
    deprecated,
    reason = "runtime test doubles still exercise the v21 provider seam"
)]
mod tests;
