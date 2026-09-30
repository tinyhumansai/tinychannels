//! Outbound delivery: the durable queue, retry policy, and reply segmentation (the splitter itself lives in `tinychannels_bus::delivery`).

pub mod policy;
pub mod progressive;
pub mod queue;
pub mod types;

pub use policy::{
    compute_backoff_ms, exceeded_max_retries, is_entry_eligible_for_recovery_retry,
    is_permanent_delivery_error, negotiate_delivery_durability,
    required_durable_final_capabilities,
};
pub use queue::{
    ack_delivery, drain_selected_pending_deliveries, enqueue_delivery, fail_delivery,
    fail_delivery_after_platform_send, load_pending_deliveries, load_pending_delivery,
    mark_delivery_platform_outcome_unknown, mark_delivery_platform_send_attempt_started,
    move_to_failed, recover_pending_deliveries,
};
pub use ::tinychannels_bus::delivery::segment;
pub use ::tinychannels_bus::delivery::{segment_delay, segment_for_delivery};
pub use types::{
    ActiveDeliveryClaimResult, DeliveryAttemptFailure, DeliveryAttemptResult,
    DeliveryAttemptSuccess, DeliveryQueueError, DeliveryQueueHandler, DeliveryQueueStore,
    DeliveryRecoveryState, DeliveryRecoverySummary, DeliveryRetryEligibility,
    DurabilityNegotiation, MAX_RETRIES, PendingDeliveryDrainDecision, QueuedDelivery,
    UnknownSendReconciliation,
};

#[cfg(test)]
mod test;
