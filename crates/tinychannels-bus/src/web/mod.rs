//! Transport-free DTOs for the deployed OpenHuman web chat event stream.
//!
//! These events are a separate vocabulary from [`crate::channel::ChannelEvent`]
//! and the progressive delivery protocol's `ChannelOutputEvent`: `event` is
//! the historical Socket.IO discriminator and is intentionally kept in snake case.

mod compat;
mod events;
mod payload;
mod schema;
mod timing;
mod types;

pub use compat::legacy_event_alias;
pub use events::WebChannelEvent;
pub use payload::{MAX_WEB_TOOL_PAYLOAD_BYTES, cap_web_args, cap_web_output};
pub use schema::web_event_schema_json;
pub use timing::{TurnCostThrottle, TurnTimingSnapshot, TurnTimingState, tokens_per_second};
pub use types::*;

/// Source-compatible alias for callers that used the generic event payload name.
pub type WebEventPayload = WebChannelEvent;
/// Compatibility spelling from the former host module.
pub type TurnUsagePayload = WebTurnUsagePayload;
/// Compatibility spelling from the former host module.
pub type TurnTimingPayload = WebTurnTimingPayload;
/// Compatibility spelling from the former host module.
pub type GuardrailPayload = WebGuardrailPayload;
/// Compatibility spelling from the former host module.
pub type GuardrailReason = WebGuardrailReason;
/// Compatibility spelling from the former host module.
pub type QueueItemPayload = WebQueueItemPayload;
/// Compatibility spelling from the former host module.
pub type SubagentUsagePayload = WebSubagentUsagePayload;
/// Compatibility spelling from the former host module.
pub type SubagentProgressDetail = WebSubagentProgressDetail;

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "compatibility_tests.rs"]
mod compatibility_tests;
