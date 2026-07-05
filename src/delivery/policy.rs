//! Durable delivery retry and capability policy.

use crate::channel::{
    ChannelOutboundIntent, DeliveryDurability, DurableFinalDeliveryCapability,
    DurableFinalDeliveryRequirementMap, OutboundPayload,
};
use crate::delivery::types::{
    DeliveryRetryEligibility, DurabilityNegotiation, MAX_RETRIES, QueuedDelivery,
};

const BACKOFF_MS: [u64; 5] = [0, 5_000, 25_000, 120_000, 600_000];

const PERMANENT_ERROR_PATTERNS: &[&str] = &[
    "no conversation reference found",
    "chat not found",
    "user not found",
    "bot was blocked by the user",
    "forbidden: bot was kicked",
    "chat_id is empty",
    "recipient is not a valid",
    "outbound not configured for channel",
];

/// OpenClaw-compatible recovery backoff for a retry count.
pub fn compute_backoff_ms(retry_count: u32) -> u64 {
    BACKOFF_MS
        .get(retry_count as usize)
        .copied()
        .unwrap_or(600_000)
}

/// Return true when an error should not consume more retry attempts.
pub fn is_permanent_delivery_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    if lower.contains("bot") && lower.contains("not") && lower.contains("member") {
        return true;
    }
    if lower.contains("ambiguous ") && lower.contains(" recipient") {
        return true;
    }
    if lower.contains("user ") && lower.contains(" not in room") {
        return true;
    }
    PERMANENT_ERROR_PATTERNS
        .iter()
        .any(|pattern| lower.contains(pattern))
}

/// Determine whether a pending delivery may be retried now.
pub fn is_entry_eligible_for_recovery_retry(
    entry: &QueuedDelivery,
    now_unix_ms: u64,
) -> DeliveryRetryEligibility {
    let backoff_ms = compute_backoff_ms(entry.retry_count + 1);
    if backoff_ms == 0 {
        return DeliveryRetryEligibility::Eligible;
    }
    let first_replay_after_crash =
        entry.retry_count == 0 && entry.last_attempt_at_unix_ms.is_none();
    if first_replay_after_crash {
        return DeliveryRetryEligibility::Eligible;
    }
    let base_attempt_at = entry
        .last_attempt_at_unix_ms
        .filter(|value| *value > 0)
        .unwrap_or(entry.enqueued_at_unix_ms);
    let next_eligible_at = base_attempt_at.saturating_add(backoff_ms);
    if now_unix_ms >= next_eligible_at {
        DeliveryRetryEligibility::Eligible
    } else {
        DeliveryRetryEligibility::Deferred {
            remaining_backoff_ms: next_eligible_at - now_unix_ms,
        }
    }
}

/// Required durable-final capabilities implied by an outbound intent.
pub fn required_durable_final_capabilities(
    intent: &ChannelOutboundIntent,
) -> DurableFinalDeliveryRequirementMap {
    let mut requirements = DurableFinalDeliveryRequirementMap::new();
    match &intent.payload {
        OutboundPayload::Text { .. } => {
            requirements.insert(DurableFinalDeliveryCapability::Text, true);
        }
        OutboundPayload::Media { .. }
        | OutboundPayload::Voice { .. }
        | OutboundPayload::Files { .. } => {
            requirements.insert(DurableFinalDeliveryCapability::Media, true);
        }
        OutboundPayload::Poll { .. } => {
            requirements.insert(DurableFinalDeliveryCapability::Poll, true);
        }
        OutboundPayload::PresentationBlocks { .. } | OutboundPayload::NativeChannelData { .. } => {
            requirements.insert(DurableFinalDeliveryCapability::Payload, true);
        }
    }
    if intent.reply_to_id.is_some() {
        requirements.insert(DurableFinalDeliveryCapability::ReplyTo, true);
    }
    if intent.thread_id.is_some() {
        requirements.insert(DurableFinalDeliveryCapability::Thread, true);
    }
    requirements
}

/// Negotiate requested durability against adapter-advertised capabilities.
pub fn negotiate_delivery_durability(
    requested: DeliveryDurability,
    required: &DurableFinalDeliveryRequirementMap,
    supported: &DurableFinalDeliveryRequirementMap,
) -> DurabilityNegotiation {
    if requested == DeliveryDurability::Disabled {
        return DurabilityNegotiation {
            durability: DeliveryDurability::Disabled,
            missing_capabilities: Vec::new(),
        };
    }

    let missing_capabilities = required
        .iter()
        .filter_map(|(capability, required)| {
            if *required && supported.get(capability).copied() != Some(true) {
                Some(*capability)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    DurabilityNegotiation {
        durability: if missing_capabilities.is_empty() {
            requested
        } else {
            DeliveryDurability::BestEffort
        },
        missing_capabilities,
    }
}

pub fn exceeded_max_retries(entry: &QueuedDelivery) -> bool {
    entry.retry_count >= MAX_RETRIES
}
