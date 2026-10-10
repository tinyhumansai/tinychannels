//! Send error taxonomy shared across adapters.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Machine-readable send failure categories.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SendErrorKind {
    TooLong,
    BadFormat,
    Forbidden,
    NotFound,
    RateLimited,
    Transient,
    #[default]
    Unknown,
}

/// Structured adapter send failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelSendError {
    pub kind: SendErrorKind,
    pub message: String,
    pub retryable: bool,
    pub retry_after: Option<f64>,
    pub chat_level_not_found: bool,
    pub continuation_message_ids: Vec<String>,
    pub partial_overflow: Option<Value>,
}

impl Default for ChannelSendError {
    fn default() -> Self {
        Self {
            kind: SendErrorKind::Unknown,
            message: String::new(),
            retryable: false,
            retry_after: None,
            chat_level_not_found: false,
            continuation_message_ids: Vec::new(),
            partial_overflow: None,
        }
    }
}
