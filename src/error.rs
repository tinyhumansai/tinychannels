//! Crate-wide error helpers.

/// Result type used by TinyChannels APIs.
pub type Result<T> = std::result::Result<T, TinyChannelsError>;

/// Minimal error type for the initial scaffold.
#[derive(Debug, Clone, PartialEq, Eq)]
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

impl std::fmt::Display for TinyChannelsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TinyChannelsError {}
