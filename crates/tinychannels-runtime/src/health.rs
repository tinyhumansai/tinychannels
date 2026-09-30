//! Bounded health checks across a set of channels.

use std::sync::Arc;
use std::time::Duration;

use tinychannels_bus::Channel;

/// Outcome of one channel's health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelHealthState {
    Healthy,
    Unhealthy,
    Timeout,
}

/// Classify a `health_check` run under a timeout.
pub fn classify_health_result(
    result: &Result<bool, tokio::time::error::Elapsed>,
) -> ChannelHealthState {
    match result {
        Ok(true) => ChannelHealthState::Healthy,
        Ok(false) => ChannelHealthState::Unhealthy,
        Err(_) => ChannelHealthState::Timeout,
    }
}

/// Run each channel's `health_check`, bounded by `timeout`, in order.
pub async fn check_channels_health(
    channels: &[Arc<dyn Channel>],
    timeout: Duration,
) -> Vec<(String, ChannelHealthState)> {
    let mut results = Vec::with_capacity(channels.len());
    for channel in channels {
        let result = tokio::time::timeout(timeout, channel.health_check()).await;
        let state = classify_health_result(&result);
        tracing::debug!(channel = channel.name(), ?state, "[channels] health check");
        results.push((channel.name().to_string(), state));
    }
    results
}
