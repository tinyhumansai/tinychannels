//! Durable delivery queue types.

use crate::channel::{ChannelOutboundIntent, MessageReceipt};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Maximum recovery attempts before a pending entry is moved to failed storage.
pub const MAX_RETRIES: u32 = 5;

/// A queued outbound delivery intent persisted before platform send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct QueuedDelivery {
    pub id: String,
    pub intent: ChannelOutboundIntent,
    pub enqueued_at_unix_ms: u64,
    pub retry_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_at_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_send_started_at_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_state: Option<DeliveryRecoveryState>,
}

impl QueuedDelivery {
    pub fn new(
        id: impl Into<String>,
        intent: ChannelOutboundIntent,
        enqueued_at_unix_ms: u64,
    ) -> Self {
        Self {
            id: id.into(),
            intent,
            enqueued_at_unix_ms,
            retry_count: 0,
            last_attempt_at_unix_ms: None,
            last_error: None,
            platform_send_started_at_unix_ms: None,
            recovery_state: None,
        }
    }
}

impl Default for QueuedDelivery {
    fn default() -> Self {
        Self::new("", ChannelOutboundIntent::default(), 0)
    }
}

/// Platform-send crash recovery marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryRecoveryState {
    SendAttemptStarted,
    UnknownAfterSend,
}

/// Queue retry eligibility decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryRetryEligibility {
    Eligible,
    Deferred { remaining_backoff_ms: u64 },
}

/// Unknown-send reconciliation verdict from an adapter or host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UnknownSendReconciliation {
    Sent {
        receipt: MessageReceipt,
        #[serde(skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
    },
    NotSent,
    Unresolved {
        #[serde(default)]
        retryable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

/// Delivery attempt success returned by the host delivery driver.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct DeliveryAttemptSuccess {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<MessageReceipt>,
}

/// Delivery attempt failure returned by the host delivery driver.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct DeliveryAttemptFailure {
    pub error: String,
    pub sent_before_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permanent: Option<bool>,
}

/// Result of one host delivery attempt.
pub type DeliveryAttemptResult =
    std::result::Result<DeliveryAttemptSuccess, DeliveryAttemptFailure>;

/// Recovery counters for one drain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct DeliveryRecoverySummary {
    pub recovered: u32,
    pub failed: u32,
    pub skipped_max_retries: u32,
    pub deferred_backoff: u32,
}

/// Result of claiming an active delivery queue entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveDeliveryClaimResult<T> {
    Claimed(T),
    ClaimedByOtherOwner,
}

/// Delivery queue error.
#[derive(Debug, thiserror::Error)]
pub enum DeliveryQueueError {
    #[error("delivery queue store error: {0}")]
    Store(String),
}

/// Host-owned durable queue storage.
#[async_trait]
pub trait DeliveryQueueStore: Send + Sync {
    async fn save_pending(&self, entry: QueuedDelivery) -> Result<(), DeliveryQueueError>;
    async fn load_pending(&self, id: &str) -> Result<Option<QueuedDelivery>, DeliveryQueueError>;
    async fn load_pending_all(&self) -> Result<Vec<QueuedDelivery>, DeliveryQueueError>;
    async fn remove_pending(&self, id: &str) -> Result<(), DeliveryQueueError>;
    async fn save_failed(&self, entry: QueuedDelivery) -> Result<(), DeliveryQueueError>;
}

/// Host delivery hooks used by recovery drains.
#[async_trait]
pub trait DeliveryQueueHandler: Send + Sync {
    async fn deliver(&self, entry: &QueuedDelivery) -> DeliveryAttemptResult;

    async fn reconcile_unknown_send(
        &self,
        _entry: &QueuedDelivery,
    ) -> Result<Option<UnknownSendReconciliation>, DeliveryQueueError> {
        Ok(None)
    }
}

/// Selection result for targeted reconnect drains.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PendingDeliveryDrainDecision {
    pub matches: bool,
    pub bypass_backoff: bool,
}

/// Capability negotiation result for a durable send request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurabilityNegotiation {
    pub durability: crate::channel::DeliveryDurability,
    pub missing_capabilities: Vec<crate::channel::DurableFinalDeliveryCapability>,
}
