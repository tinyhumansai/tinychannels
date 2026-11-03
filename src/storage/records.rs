//! Projections between bus records and the implementation's stored shapes.
//!
//! The serializable record DTOs are owned by `tinychannels-bus`; these
//! conversions belong here because they encode TinyChannels persistence rules.

use tinychannels_bus::channel::ChannelOutboundIntent;
use tinychannels_bus::module_delivery::{
    DeliveryQueueEntry as QueuedDelivery, DeliveryTerminalResult,
};

pub use tinychannels_bus::storage::records::{
    ChatBinding, StoredDelivery, StoredIntent, StoredResult,
};

/// Convert a wire millisecond count to the persisted signed representation.
///
/// Values above `i64::MAX` saturate at `i64::MAX`.
#[must_use]
pub fn to_stored_ms(ms: u64) -> i64 {
    i64::try_from(ms).unwrap_or(i64::MAX)
}

/// Convert a persisted signed millisecond count to the wire representation.
///
/// Negative values become zero.
#[must_use]
pub fn from_stored_ms(ms: i64) -> u64 {
    u64::try_from(ms).unwrap_or(0)
}

/// Project a channel intent into its persisted DTO.
#[must_use]
pub fn stored_intent_from_channel(intent: &ChannelOutboundIntent) -> StoredIntent {
    StoredIntent {
        idempotency_key: intent.idempotency_key.clone(),
        channel_id: intent.channel_id.clone(),
        conversation_id: intent.conversation_id.clone(),
        channel_ref: intent.channel_ref.clone(),
        conversation_ref: intent.conversation_ref.clone(),
        turn_id: intent.turn_id.clone(),
        reply_to_id: intent.reply_to_id.clone(),
        thread_id: intent.thread_id.clone(),
        durability: intent.durability,
        payload: intent.payload.clone(),
    }
}

/// Rebuild a channel intent from its persisted DTO.
#[must_use]
pub fn channel_from_stored_intent(stored: StoredIntent) -> ChannelOutboundIntent {
    ChannelOutboundIntent {
        idempotency_key: stored.idempotency_key,
        channel_id: stored.channel_id,
        conversation_id: stored.conversation_id,
        channel_ref: stored.channel_ref,
        conversation_ref: stored.conversation_ref,
        turn_id: stored.turn_id,
        reply_to_id: stored.reply_to_id,
        thread_id: stored.thread_id,
        durability: stored.durability,
        payload: stored.payload,
    }
}

/// Project a queued delivery into its persisted DTO.
#[must_use]
pub fn stored_delivery_from_queued(entry: &QueuedDelivery) -> StoredDelivery {
    StoredDelivery {
        intent: stored_intent_from_channel(&entry.intent),
        enqueued_ms: to_stored_ms(entry.enqueued_at_unix_ms),
        retry_count: entry.retry_count,
        last_attempt_ms: entry.last_attempt_at_unix_ms.map(to_stored_ms),
        last_error: entry.last_error.clone(),
        platform_send_started_ms: entry.platform_send_started_at_unix_ms.map(to_stored_ms),
        recovery_state: entry.recovery_state,
        partial_receipt: entry.partial_receipt.clone(),
    }
}

/// Rebuild a queued delivery from its persisted DTO.
#[must_use]
pub fn queued_from_stored_delivery(stored: StoredDelivery) -> QueuedDelivery {
    let intent = channel_from_stored_intent(stored.intent);
    QueuedDelivery {
        id: intent.idempotency_key.clone(),
        intent,
        enqueued_at_unix_ms: from_stored_ms(stored.enqueued_ms),
        retry_count: stored.retry_count,
        last_attempt_at_unix_ms: stored.last_attempt_ms.map(from_stored_ms),
        last_error: stored.last_error,
        platform_send_started_at_unix_ms: stored.platform_send_started_ms.map(from_stored_ms),
        recovery_state: stored.recovery_state,
        partial_receipt: stored.partial_receipt,
    }
}

/// Project a terminal delivery result into its persisted DTO.
#[must_use]
pub fn stored_result_from_terminal(result: &DeliveryTerminalResult) -> StoredResult {
    StoredResult {
        id: result.id.clone(),
        intent_fingerprint: result.intent_fingerprint.clone(),
        completed_ms: to_stored_ms(result.completed_at_unix_ms),
        receipt: result.receipt.clone(),
    }
}

/// Rebuild a terminal delivery result from its persisted DTO.
#[must_use]
pub fn terminal_result_from_stored(stored: StoredResult) -> DeliveryTerminalResult {
    DeliveryTerminalResult {
        id: stored.id,
        intent_fingerprint: stored.intent_fingerprint,
        completed_at_unix_ms: from_stored_ms(stored.completed_ms),
        receipt: stored.receipt,
    }
}

#[cfg(test)]
#[path = "records_tests.rs"]
mod tests;
