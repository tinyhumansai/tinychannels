//! Runtime mechanics shared by TinyChannels hosts.
//!
//! This crate owns shared provider seams and algorithms, without concrete
//! provider transports or host policy. Hosts observe lifecycle through typed adapters.

use rand::RngExt as _;

mod cli;
mod dispatch;
mod health;
#[cfg(test)]
#[path = "health_tests.rs"]
mod health_tests;
#[allow(
    deprecated,
    reason = "legacy session projection remains during v22 migration"
)]
mod session;
pub use channel::{Channel, ChannelSendExt, SendMessageExt};
pub use cli::CliChannel;
pub use dispatch::{RuntimeChannelMessage, run_dispatch_loop};
pub use health::{ChannelHealthState, classify_health_result};
#[allow(deprecated, reason = "legacy session API remains during v22 migration")]
pub use session::{ChannelSession, run_in_session};

/// Maximum reconnect jitter added to a listener retry.
pub const MAX_JITTER_MS: u64 = 1_000;

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
        tracing::error!("{}", crate::context::worker_crash_message(&error));
    }
}

#[cfg(test)]
#[path = "runtime_extras_tests.rs"]
mod runtime_extras_tests;

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;

pub mod adapters;
pub mod channel;
pub mod context;
pub mod delivery;
pub mod relay;
pub mod security;
pub mod text;
pub mod traits;
pub use tinychannels_bus::{capabilities, error, names, version};
pub use traits::WebhookRejected;

pub mod controllers;

pub mod config;

#[cfg(test)]
#[path = "relocation_tests.rs"]
mod relocation_tests;

/// Projection of portable channel output onto the deployed web event wire.
pub mod web;
