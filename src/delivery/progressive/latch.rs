//! Process-wide latch for providers whose backend has no edit route.
//!
//! Whether the edit route exists is a property of the deployed backend, not of
//! a turn, so re-probing it every turn only buys a guaranteed 404 per turn.
//! Latching keeps it to one attempt per provider per process, and it
//! self-heals on the next start once the backend ships the route (#5230).

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

static EDIT_UNSUPPORTED_PROVIDERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// Provider key for the latch: inbound channels arrive provider-prefixed
/// (`telegram:<chat>`) and route existence is per provider.
pub fn edit_capability_key(channel: &str) -> String {
    channel.split(':').next().unwrap_or(channel).to_string()
}

/// `true` once this process has learned the backend serves no edit route for
/// `channel`'s provider. Callers should skip the request entirely.
pub fn channel_edits_unsupported(channel: &str) -> bool {
    EDIT_UNSUPPORTED_PROVIDERS
        .get_or_init(Default::default)
        .lock()
        .map(|set| set.contains(&edit_capability_key(channel)))
        .unwrap_or(false)
}

/// Latch `channel`'s provider as having no message-edit route.
pub fn mark_channel_edits_unsupported(channel: &str) {
    let key = edit_capability_key(channel);
    if let Ok(mut set) = EDIT_UNSUPPORTED_PROVIDERS
        .get_or_init(Default::default)
        .lock()
        && set.insert(key.clone())
    {
        tracing::warn!(
            "[channel-inbound][edit] backend serves no message-edit route for provider='{}' — \
             progressive edits disabled for this process; placeholders will be posted once and \
             cleaned up by delete instead (#5230)",
            key,
        );
    }
}
