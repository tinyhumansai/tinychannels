//! Durable delivery queue state machine.

use crate::delivery::policy::{
    exceeded_max_retries, is_entry_eligible_for_recovery_retry, is_permanent_delivery_error,
};
use crate::delivery::types::{
    DeliveryAttemptFailure, DeliveryAttemptSuccess, DeliveryQueueError, DeliveryQueueHandler,
    DeliveryQueueStore, DeliveryRecoveryState, DeliveryRecoverySummary, DeliveryRetryEligibility,
    PendingDeliveryDrainDecision, QueuedDelivery, UnknownSendReconciliation,
};

/// Persist a delivery entry before attempting send.
pub async fn enqueue_delivery(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    entry: QueuedDelivery,
) -> Result<(), DeliveryQueueError> {
    store.save_pending(entry).await
}

/// Remove a successfully delivered entry from the pending queue.
pub async fn ack_delivery(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    id: &str,
) -> Result<(), DeliveryQueueError> {
    store.remove_pending(id).await
}

/// Update a queue entry after a failed delivery attempt.
pub async fn fail_delivery(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    id: &str,
    error: impl Into<String>,
    now_unix_ms: u64,
) -> Result<(), DeliveryQueueError> {
    let Some(mut entry) = store.load_pending(id).await? else {
        return Ok(());
    };
    entry.retry_count = entry.retry_count.saturating_add(1);
    entry.last_attempt_at_unix_ms = Some(now_unix_ms);
    entry.last_error = Some(error.into());
    store.save_pending(entry).await
}

/// Record a failed attempt while preserving post-send uncertainty.
pub async fn fail_delivery_after_platform_send(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    id: &str,
    error: impl Into<String>,
    now_unix_ms: u64,
) -> Result<(), DeliveryQueueError> {
    let Some(mut entry) = store.load_pending(id).await? else {
        return Ok(());
    };
    entry.retry_count = entry.retry_count.saturating_add(1);
    entry.last_attempt_at_unix_ms = Some(now_unix_ms);
    entry.last_error = Some(error.into());
    entry.platform_send_started_at_unix_ms =
        entry.platform_send_started_at_unix_ms.or(Some(now_unix_ms));
    entry.recovery_state = Some(DeliveryRecoveryState::UnknownAfterSend);
    store.save_pending(entry).await
}

/// Mark that platform I/O has started for a delivery entry.
pub async fn mark_delivery_platform_send_attempt_started(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    id: &str,
    now_unix_ms: u64,
) -> Result<(), DeliveryQueueError> {
    let Some(mut entry) = store.load_pending(id).await? else {
        return Ok(());
    };
    entry.platform_send_started_at_unix_ms =
        entry.platform_send_started_at_unix_ms.or(Some(now_unix_ms));
    entry.recovery_state = Some(DeliveryRecoveryState::SendAttemptStarted);
    store.save_pending(entry).await
}

/// Mark that a platform send may have completed, but no durable receipt is known.
pub async fn mark_delivery_platform_outcome_unknown(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    id: &str,
    now_unix_ms: u64,
) -> Result<(), DeliveryQueueError> {
    let Some(mut entry) = store.load_pending(id).await? else {
        return Ok(());
    };
    entry.platform_send_started_at_unix_ms =
        entry.platform_send_started_at_unix_ms.or(Some(now_unix_ms));
    entry.recovery_state = Some(DeliveryRecoveryState::UnknownAfterSend);
    store.save_pending(entry).await
}

/// Move a pending entry to failed storage.
pub async fn move_to_failed(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    id: &str,
) -> Result<(), DeliveryQueueError> {
    let Some(entry) = store.load_pending(id).await? else {
        return Ok(());
    };
    store.save_failed(entry).await?;
    store.remove_pending(id).await
}

/// Load a single pending delivery entry.
pub async fn load_pending_delivery(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    id: &str,
) -> Result<Option<QueuedDelivery>, DeliveryQueueError> {
    store.load_pending(id).await
}

/// Load all pending delivery entries.
pub async fn load_pending_deliveries(
    store: &(dyn DeliveryQueueStore + Send + Sync),
) -> Result<Vec<QueuedDelivery>, DeliveryQueueError> {
    store.load_pending_all().await
}

/// Recover all pending deliveries in enqueue order.
pub async fn recover_pending_deliveries(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    handler: &(dyn DeliveryQueueHandler + Send + Sync),
    now_unix_ms: u64,
) -> Result<DeliveryRecoverySummary, DeliveryQueueError> {
    drain_selected_pending_deliveries(store, handler, now_unix_ms, |_| {
        PendingDeliveryDrainDecision {
            matches: true,
            bypass_backoff: false,
        }
    })
    .await
}

/// Drain selected pending deliveries, useful for reconnect-triggered retries.
pub async fn drain_selected_pending_deliveries<F>(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    handler: &(dyn DeliveryQueueHandler + Send + Sync),
    now_unix_ms: u64,
    select_entry: F,
) -> Result<DeliveryRecoverySummary, DeliveryQueueError>
where
    F: Fn(&QueuedDelivery) -> PendingDeliveryDrainDecision,
{
    let mut entries = store.load_pending_all().await?;
    entries.sort_by_key(|entry| entry.enqueued_at_unix_ms);
    let mut summary = DeliveryRecoverySummary::default();

    for snapshot in entries {
        let Some(entry) = store.load_pending(&snapshot.id).await? else {
            continue;
        };
        let decision = select_entry(&entry);
        if !decision.matches {
            continue;
        }
        if exceeded_max_retries(&entry) {
            move_to_failed(store, &entry.id).await?;
            summary.skipped_max_retries += 1;
            continue;
        }
        if !decision.bypass_backoff {
            match is_entry_eligible_for_recovery_retry(&entry, now_unix_ms) {
                DeliveryRetryEligibility::Eligible => {}
                DeliveryRetryEligibility::Deferred { .. } => {
                    summary.deferred_backoff += 1;
                    continue;
                }
            }
        }
        match drain_queued_entry(store, handler, entry, now_unix_ms).await? {
            DrainResult::Recovered => summary.recovered += 1,
            DrainResult::Failed | DrainResult::MovedToFailed => summary.failed += 1,
        }
    }

    Ok(summary)
}

enum DrainResult {
    Recovered,
    Failed,
    MovedToFailed,
}

async fn drain_queued_entry(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    handler: &(dyn DeliveryQueueHandler + Send + Sync),
    entry: QueuedDelivery,
    now_unix_ms: u64,
) -> Result<DrainResult, DeliveryQueueError> {
    match entry.recovery_state {
        Some(DeliveryRecoveryState::SendAttemptStarted)
        | Some(DeliveryRecoveryState::UnknownAfterSend) => {
            return drain_entry_with_unknown_send_state(store, handler, entry, now_unix_ms).await;
        }
        None => {}
    }

    attempt_delivery(store, handler, &entry, now_unix_ms).await
}

async fn drain_entry_with_unknown_send_state(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    handler: &(dyn DeliveryQueueHandler + Send + Sync),
    entry: QueuedDelivery,
    now_unix_ms: u64,
) -> Result<DrainResult, DeliveryQueueError> {
    let reconciliation = handler.reconcile_unknown_send(&entry).await?;
    match reconciliation {
        Some(UnknownSendReconciliation::Sent { .. }) => {
            ack_delivery(store, &entry.id).await?;
            Ok(DrainResult::Recovered)
        }
        Some(UnknownSendReconciliation::NotSent)
            if entry.recovery_state == Some(DeliveryRecoveryState::SendAttemptStarted) =>
        {
            attempt_delivery(store, handler, &entry, now_unix_ms).await
        }
        Some(UnknownSendReconciliation::Unresolved {
            retryable: true,
            error,
        }) => {
            fail_delivery(
                store,
                &entry.id,
                format!(
                    "unknown-send reconciliation is unresolved: {}",
                    error.unwrap_or_else(|| "unresolved".to_string())
                ),
                now_unix_ms,
            )
            .await?;
            Ok(DrainResult::Failed)
        }
        Some(UnknownSendReconciliation::NotSent)
        | Some(UnknownSendReconciliation::Unresolved { .. })
        | None => {
            move_to_failed(store, &entry.id).await?;
            Ok(DrainResult::MovedToFailed)
        }
    }
}

async fn attempt_delivery(
    store: &(dyn DeliveryQueueStore + Send + Sync),
    handler: &(dyn DeliveryQueueHandler + Send + Sync),
    entry: &QueuedDelivery,
    now_unix_ms: u64,
) -> Result<DrainResult, DeliveryQueueError> {
    match handler.deliver(entry).await {
        Ok(DeliveryAttemptSuccess { .. }) => {
            ack_delivery(store, &entry.id).await?;
            Ok(DrainResult::Recovered)
        }
        Err(DeliveryAttemptFailure {
            error,
            sent_before_error: true,
            ..
        }) => {
            mark_delivery_platform_outcome_unknown(store, &entry.id, now_unix_ms).await?;
            let Some(mut current) = store.load_pending(&entry.id).await? else {
                return Ok(DrainResult::Failed);
            };
            current.last_error = Some(error);
            store.save_pending(current).await?;
            Ok(DrainResult::Failed)
        }
        Err(DeliveryAttemptFailure {
            error, permanent, ..
        }) => {
            if permanent.unwrap_or_else(|| is_permanent_delivery_error(&error)) {
                move_to_failed(store, &entry.id).await?;
                Ok(DrainResult::MovedToFailed)
            } else {
                fail_delivery(store, &entry.id, error, now_unix_ms).await?;
                Ok(DrainResult::Failed)
            }
        }
    }
}
