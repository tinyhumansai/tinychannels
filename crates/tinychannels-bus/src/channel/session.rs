//! Session-key construction and legacy key compatibility.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Session key isolation policy, matching Hermes' default toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionKeyPolicy {
    pub group_sessions_per_user: bool,
    pub thread_sessions_per_user: bool,
}

impl Default for SessionKeyPolicy {
    fn default() -> Self {
        Self {
            group_sessions_per_user: true,
            thread_sessions_per_user: false,
        }
    }
}

/// Legacy OpenHuman key candidates that may need lookup during migration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct LegacySessionKeys {
    pub conversation_history_key: String,
    pub conversation_memory_key: String,
}
