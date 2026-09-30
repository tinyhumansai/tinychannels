//! Logout-scoped cancellation for channel runtimes.
//!
//! An explicit logout must stop every channel listener regardless of how the
//! model provider is authenticated. [`ChannelSession`] holds the current
//! session token; [`ChannelSession::invalidate`] cancels it and installs a
//! fresh one so the next start runs in a new session.

use std::future::Future;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

/// A replaceable session token for channel runtimes.
#[derive(Debug, Default)]
pub struct ChannelSession {
    token: Mutex<CancellationToken>,
}

impl ChannelSession {
    /// A fresh, uncancelled session.
    pub fn new() -> Self {
        Self::default()
    }

    /// The current session token. Runtimes started now stop when it cancels.
    pub fn current(&self) -> CancellationToken {
        self.token.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Cancel the current session and start a new one.
    pub fn invalidate(&self) {
        let mut token = self.token.lock().unwrap_or_else(|e| e.into_inner());
        token.cancel();
        *token = CancellationToken::new();
        tracing::debug!("[channels] invalidated channel runtimes on logout");
    }
}

/// Run `runtime` until it finishes or `session` is cancelled. A cancelled
/// session is a clean stop, not an error.
pub async fn run_in_session(
    session: CancellationToken,
    runtime: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<()> {
    tokio::select! {
        biased;
        _ = session.cancelled() => {
            tracing::info!("[channels] stopping runtime after logout");
            Ok(())
        }
        result = runtime => result,
    }
}
