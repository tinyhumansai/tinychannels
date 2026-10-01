//! Runtime mechanics shared by TinyChannels hosts.
//!
//! This crate deliberately owns no provider, persistence, event bus, or host
//! policy. Hosts observe listener lifecycle through [`ListenerObserver`].

use std::sync::Arc;
use std::time::Duration;

use rand::RngExt as _;
use tinychannels_bus::{Channel, ChannelMessage};
use tokio_util::sync::CancellationToken;

mod cli;
mod dispatch;
mod health;
mod session;

pub use cli::CliChannel;
pub use dispatch::{RuntimeChannelMessage, run_dispatch_loop};
pub use health::{ChannelHealthState, check_channels_health, classify_health_result};
pub use session::{ChannelSession, run_in_session};

/// Maximum reconnect jitter added to a listener retry.
pub const MAX_JITTER_MS: u64 = 1_000;

/// Host callback for listener lifecycle facts.
pub trait ListenerObserver: Send + Sync {
    /// A listener is about to enter its receive loop.
    fn connected(&self, _channel: &str) {}
    /// A listener exited and will be retried.
    fn disconnected(&self, _channel: &str, _reason: &str, _failed: bool) {}
    /// A retry has been scheduled after a listener exit.
    fn restarted(&self, _channel: &str) {}
}

/// A listener observer with no host side effects.
#[derive(Debug, Default)]
pub struct NoopListenerObserver;
impl ListenerObserver for NoopListenerObserver {}

/// Compute the bounded listener queue capacity for a provider count.
pub fn compute_max_in_flight_messages(channel_count: usize) -> usize {
    channel_count.saturating_mul(4).clamp(8, 64)
}

/// Deterministically choose a broadly-supported acknowledgement reaction.
pub fn select_acknowledgment_reaction(content: &str) -> &'static str {
    let lower = content.to_lowercase();
    let variant = content
        .len()
        .wrapping_add(content.chars().next().map_or(0, |ch| ch as usize))
        & 1;
    let contains = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    let starts = |words: &[&str]| words.iter().any(|word| lower.starts_with(word));
    let options: &[&str] = if contains(&["thank", "thx", "appreciate", "grateful", "cheers"]) {
        &["❤️", "🙏"]
    } else if contains(&[
        "amazing",
        "awesome",
        "incredible",
        "love it",
        "congrat",
        "!!",
    ]) {
        &["🔥", "🎉"]
    } else if contains(&[
        "price", "btc", "eth", "crypto", "trade", "pump", "dump", "market", "token", "wallet",
        "defi", "nft", "sol", "bnb",
    ]) {
        &["💯", "⚡"]
    } else if contains(&[
        "code",
        "function",
        "api",
        "deploy",
        "build",
        "debug",
        "script",
        "git",
        "rust",
        "python",
        "js",
        "typescript",
    ]) {
        &["👨‍💻", "🤓"]
    } else if starts(&[
        "hi",
        "hello",
        "hey",
        "sup",
        "good morning",
        "good evening",
        "good afternoon",
    ]) || lower == "yo"
        || lower.starts_with("yo ")
    {
        &["🤗", "😁"]
    } else if lower.contains('?')
        || starts(&[
            "how",
            "what",
            "why",
            "when",
            "where",
            "who",
            "can you",
            "could you",
            "would you",
            "is ",
            "are ",
            "do you",
            "does",
        ])
    {
        &["🤔", "✍️"]
    } else {
        &["👀", "✍️"]
    };
    options[variant % options.len()]
}

/// Spawn a reconnecting listener. Host-specific observability is delivered to
/// `observer`; the retry policy remains identical for every host.
pub fn spawn_supervised_listener(
    channel: Arc<dyn Channel>,
    tx: tokio::sync::mpsc::Sender<ChannelMessage>,
    initial_backoff_secs: u64,
    max_backoff_secs: u64,
    observer: Arc<dyn ListenerObserver>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let name = channel.name().to_owned();
        let mut backoff = initial_backoff_secs.max(1);
        let max_backoff = max_backoff_secs.max(backoff);
        loop {
            observer.connected(&name);
            let result = channel.listen(tx.clone()).await;
            if tx.is_closed() {
                break;
            }
            match result {
                Ok(()) => observer.disconnected(&name, "exited unexpectedly", false),
                Err(error) => observer.disconnected(&name, &error.to_string(), true),
            }
            observer.restarted(&name);
            tokio::time::sleep(
                Duration::from_secs(backoff) + Duration::from_millis(jitter_millis(backoff)),
            )
            .await;
            backoff = backoff.saturating_mul(2).min(max_backoff);
        }
    })
}

/// Sample full reconnect jitter, bounded to avoid dwarfing the base retry.
pub fn jitter_millis(backoff_secs: u64) -> u64 {
    let window = backoff_secs.saturating_mul(1_000).min(MAX_JITTER_MS);
    if window != 0 {
        rand::rng().random_range(0..window)
    } else {
        0
    }
}

/// Log a failed worker join without imposing host-specific error reporting.
pub fn log_worker_join_result(result: Result<(), tokio::task::JoinError>) {
    if let Err(error) = result {
        tracing::error!("Channel message worker crashed: {error}");
    }
}

/// Maintain a typing indicator until `cancellation_token` is cancelled.
pub fn spawn_scoped_typing_task(
    channel: Arc<dyn Channel>,
    recipient: String,
    cancellation_token: CancellationToken,
    refresh_interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancellation_token.cancelled() => break,
                _ = tokio::time::sleep(refresh_interval) => {
                    if let Err(error) = channel.start_typing(&recipient).await {
                        tracing::debug!(channel = channel.name(), "typing start failed: {error}");
                    }
                }
            }
        }
        if let Err(error) = channel.stop_typing(&recipient).await {
            tracing::debug!(channel = channel.name(), "typing stop failed: {error}");
        }
    })
}

#[cfg(test)]
#[path = "runtime_extras_tests.rs"]
mod runtime_extras_tests;

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
