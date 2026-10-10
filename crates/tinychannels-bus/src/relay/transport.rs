//! Relay transport contract: identities, timeouts, errors and the handler
//! traits a host implements.
//!
//! The transport *loop* that drives these lives in the `tinychannels` crate;
//! only the vocabulary it speaks belongs to the contract.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_HANDSHAKE_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_OUTBOUND_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_IDLE_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_RECONNECT_BACKOFF_MS: u64 = 1_000;
const DEFAULT_RECONNECT_MAX_BACKOFF_MS: u64 = 30_000;

/// One platform/bot identity advertised to the relay connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RelayIdentity {
    pub platform: String,
    #[serde(rename = "botId")]
    pub bot_id: String,
}

/// Timeouts for transport operations that wait on connector frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct RelayTransportTimeouts {
    pub handshake_ms: u64,
    pub outbound_ms: u64,
    pub idle_ms: u64,
}

impl Default for RelayTransportTimeouts {
    fn default() -> Self {
        Self {
            handshake_ms: DEFAULT_HANDSHAKE_TIMEOUT_MS,
            outbound_ms: DEFAULT_OUTBOUND_TIMEOUT_MS,
            idle_ms: DEFAULT_IDLE_TIMEOUT_MS,
        }
    }
}

/// Reconnect backoff settings for relay runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct RelayReconnectPolicy {
    pub backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl Default for RelayReconnectPolicy {
    fn default() -> Self {
        Self {
            backoff_ms: DEFAULT_RECONNECT_BACKOFF_MS,
            max_backoff_ms: DEFAULT_RECONNECT_MAX_BACKOFF_MS,
        }
    }
}

/// Errors surfaced by the relay transport loop.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RelayTransportError {
    #[error("relay transport is closed")]
    Closed,
    #[error("relay transport timed out waiting for {operation}")]
    Timeout { operation: &'static str },
    #[error("relay transport io error: {0}")]
    Io(String),
    #[error("relay transport handler error: {0}")]
    Handler(String),
}
