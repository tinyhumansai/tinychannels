//! Live verification of email (IMAP) credentials before they are persisted.

use std::time::Duration;

use super::email_channel::{EmailChannel, EmailConfig};
use crate::traits::Channel as _;

/// Outer bound on the IMAP login probe. The channel's own health check has an
/// inner budget; this cap also lets the caller tell a timeout from a failed
/// login.
pub const EMAIL_VERIFY_TIMEOUT: Duration = Duration::from_secs(20);

/// Live-verify IMAP credentials by attempting a login, so a wrong host or
/// password fails at connect time instead of wedging the listener on the next
/// restart. Returns a user-facing error message on failure.
pub async fn verify_email_credentials(cfg: &EmailConfig) -> Result<(), String> {
    let probe = EmailChannel::new(cfg.clone());
    match tokio::time::timeout(EMAIL_VERIFY_TIMEOUT, probe.health_check()).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(
            "IMAP connection failed — check the host, port, email address, and app password"
                .to_string(),
        ),
        Err(_) => Err(format!(
            "IMAP connection to {} timed out — check the host and port",
            cfg.imap_host
        )),
    }
}
