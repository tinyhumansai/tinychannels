//! Generic channel adapters.

pub mod local;

pub use local::{LocalChannelAdapter, LocalOutboundSink};

#[cfg(test)]
mod test;
