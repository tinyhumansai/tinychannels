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

impl ChannelSendError {
    pub fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        let kind = classify_send_error(&message);
        Self {
            chat_level_not_found: is_chat_level_not_found(&message),
            retryable: default_retryable(kind, &message),
            kind,
            message,
            ..Default::default()
        }
    }
}

pub fn classify_send_error(error_text: &str) -> SendErrorKind {
    let blob = error_text.to_ascii_lowercase();
    if blob.trim().is_empty() {
        return SendErrorKind::Unknown;
    }
    if blob.contains("message_too_long")
        || blob.contains("too long")
        || blob.contains("message is too long")
    {
        return SendErrorKind::TooLong;
    }
    if blob.contains("can't parse entities")
        || blob.contains("cant parse entities")
        || blob.contains("can't find end")
        || blob.contains("unsupported start tag")
        || (blob.contains("entity") && blob.contains("parse"))
        || (blob.contains("bad request") && blob.contains("entit"))
    {
        return SendErrorKind::BadFormat;
    }
    if blob.contains("forbidden")
        || blob.contains("bot was blocked")
        || blob.contains("blocked by the user")
        || blob.contains("user is deactivated")
        || blob.contains("not enough rights")
        || blob.contains("have no rights")
        || blob.contains("not a member")
    {
        return SendErrorKind::Forbidden;
    }
    if is_chat_level_not_found(&blob)
        || SUBCHAT_NOT_FOUND_SUBSTRINGS
            .iter()
            .any(|needle| blob.contains(needle))
    {
        return SendErrorKind::NotFound;
    }
    if blob.contains("flood")
        || blob.contains("too many requests")
        || blob.contains("retry after")
        || blob.contains("rate limit")
    {
        return SendErrorKind::RateLimited;
    }
    if RETRYABLE_ERROR_PATTERNS
        .iter()
        .any(|needle| blob.contains(needle))
        || blob.contains("connecttimeout")
    {
        return SendErrorKind::Transient;
    }
    SendErrorKind::Unknown
}

pub fn is_chat_level_not_found(error_text: &str) -> bool {
    let blob = error_text.to_ascii_lowercase();
    if SUBCHAT_NOT_FOUND_SUBSTRINGS
        .iter()
        .any(|needle| blob.contains(needle))
    {
        return false;
    }
    CHAT_LEVEL_NOT_FOUND_SUBSTRINGS
        .iter()
        .any(|needle| blob.contains(needle))
}

fn default_retryable(kind: SendErrorKind, message: &str) -> bool {
    // Timeouts can leave platform delivery state unknown, so they are not
    // automatically retryable without idempotency/reconciliation.
    if message.to_ascii_lowercase().contains("timeout") {
        return false;
    }
    matches!(kind, SendErrorKind::RateLimited | SendErrorKind::Transient)
}

const CHAT_LEVEL_NOT_FOUND_SUBSTRINGS: &[&str] = &["chat not found"];
const SUBCHAT_NOT_FOUND_SUBSTRINGS: &[&str] = &[
    "message to edit not found",
    "message to reply not found",
    "thread not found",
    "topic_deleted",
    "message_id_invalid",
];
const RETRYABLE_ERROR_PATTERNS: &[&str] = &[
    "connection reset",
    "connection aborted",
    "connection refused",
    "temporarily unavailable",
    "network is unreachable",
    "server disconnected",
    "socket hang up",
];
