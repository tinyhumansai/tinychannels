//! Outbound delivery vocabulary that is pure text logic: reply segmentation.
//!
//! Lives in the contract crate so a host can split a reply into human-paced
//! bubbles without pulling in the optional `tinychannels` provider stack. The
//! durable queue and retry policy stay in `tinychannels::delivery`.

pub mod segment;

pub use segment::{segment_delay, segment_for_delivery};
