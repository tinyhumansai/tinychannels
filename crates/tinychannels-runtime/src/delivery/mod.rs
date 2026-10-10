//! Runtime reply segmentation and delivery pacing.

pub mod segment;

pub use segment::{segment_delay, segment_for_delivery};
