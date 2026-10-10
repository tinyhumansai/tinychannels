//! Generic channel adapters.

pub mod local;

pub use local::{LocalChannelAdapter, LocalOutboundSink};

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;
