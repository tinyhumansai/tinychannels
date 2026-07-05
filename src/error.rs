//! Crate-wide error helpers.

use crate::channel::ChannelSendError;
use thiserror::Error;

/// Result type used by TinyChannels APIs.
pub type Result<T> = std::result::Result<T, TinyChannelsError>;

/// Crate-wide error for portable channel operations.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum TinyChannelsError {
    #[error("{0}")]
    Message(String),
    #[error("channel send failed ({kind:?}): {message}")]
    Send {
        kind: crate::channel::SendErrorKind,
        message: String,
        details: Box<ChannelSendError>,
    },
    #[error("configuration error: {0}")]
    Config(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}

impl TinyChannelsError {
    /// Creates an error from a human-readable message.
    pub fn new(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// Returns the error message.
    pub fn message(&self) -> &str {
        match self {
            Self::Message(message)
            | Self::Config(message)
            | Self::Serialization(message)
            | Self::Send { message, .. } => message,
        }
    }
}

impl From<ChannelSendError> for TinyChannelsError {
    fn from(error: ChannelSendError) -> Self {
        Self::Send {
            kind: error.kind,
            message: error.message.clone(),
            details: Box::new(error),
        }
    }
}
