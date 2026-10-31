//! Compatibility path for the canonical web channel output projector.
//!
//! Projection is owned by the web adapter so callers through either public
//! path use the same typed content, lifecycle, and wire normalization rules.

pub use crate::adapters::web::{WebTurnProjector, context_mut};

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;
