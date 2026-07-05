//! Crate-wide error helpers.

use thiserror::Error;

/// Result type used by TinyChannels APIs.
pub type Result<T> = std::result::Result<T, TinyChannelsError>;

/// Crate-wide error for portable channel operations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct TinyChannelsError {
    message: String,
}

impl TinyChannelsError {
    /// Creates an error from a human-readable message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the error message.
    pub fn message(&self) -> &str {
        &self.message
    }
}
