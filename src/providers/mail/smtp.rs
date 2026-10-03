//! The async `lettre` SMTP transport behind [`MailSender`] (feature `email-send`).
//!
//! Ported from OpenCompany's `server/ops/smtp.rs`. Unlike
//! [`EmailChannel::send_message`](crate::providers::EmailChannel::send_message),
//! which drives a blocking `SmtpTransport` from one global `EmailConfig`, this
//! sender is async (it never blocks a runtime worker) and takes its credentials
//! per call, so one instance serves every account a host holds.

use std::time::Duration;

use async_trait::async_trait;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use super::types::{MailCredentials, MailError, MailSender, OutboundEmail, SmtpSecurity};

/// The async SMTP [`MailSender`].
///
/// Every send is bounded end to end by [`Self::with_timeout`] (default
/// [`Self::DEFAULT_TIMEOUT`]): `lettre`'s own timeout bounds individual phases
/// and not reliably all of them, so a relay that accepts the connection and
/// then stalls could otherwise hold the caller indefinitely.
#[derive(Debug, Clone)]
pub struct LettreMailSender {
    timeout: Duration,
}

impl LettreMailSender {
    /// The end-to-end bound on one delivery when none is configured.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

    /// A sender with the default timeout.
    pub fn new() -> Self {
        Self {
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    /// Overrides the end-to-end bound on one delivery (connect through the
    /// server's final reply).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Default for LettreMailSender {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MailSender for LettreMailSender {
    async fn send(&self, creds: &MailCredentials, email: &OutboundEmail) -> Result<(), MailError> {
        // Built before the clock starts: an unparseable address is the
        // caller's mistake and must fail without dialing anyone.
        let (transport, message) = prepare(creds, email)?;
        match tokio::time::timeout(self.timeout, transport.send(message)).await {
            Ok(Ok(_)) => {
                tracing::debug!(
                    from = creds.from_email(),
                    "[mail][smtp] message accepted"
                );
                Ok(())
            }
            Ok(Err(error)) => Err(MailError::Transport(format!("smtp send: {error}"))),
            Err(_) => Err(MailError::TimedOut),
        }
    }
}

/// Builds the message and a transport for `creds`.
fn prepare(
    creds: &MailCredentials,
    email: &OutboundEmail,
) -> Result<(AsyncSmtpTransport<Tokio1Executor>, Message), MailError> {
    // Selecting the transport by variant is what makes this an adapter: a new
    // provider adds a variant and this stops compiling until it is handled.
    let MailCredentials::Smtp(creds) = creds;

    let from = if creds.from_name.is_empty() {
        creds.from_email.clone()
    } else {
        format!("{} <{}>", creds.from_name, creds.from_email)
    };
    let message = Message::builder()
        .from(
            from.parse()
                .map_err(|e| MailError::InvalidAddress(format!("from {from:?}: {e}")))?,
        )
        .to(email
            .to
            .parse()
            .map_err(|e| MailError::InvalidAddress(format!("to {:?}: {e}", email.to)))?)
        .subject(&email.subject)
        .body(email.body.clone())
        .map_err(|e| MailError::InvalidAddress(format!("message: {e}")))?;

    let mut builder = match creds.security {
        SmtpSecurity::None => {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&creds.host).port(creds.port)
        }
        SmtpSecurity::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&creds.host)
            .map_err(|e| MailError::Transport(format!("smtp starttls: {e}")))?
            .port(creds.port),
        SmtpSecurity::Ssl => AsyncSmtpTransport::<Tokio1Executor>::relay(&creds.host)
            .map_err(|e| MailError::Transport(format!("smtp relay: {e}")))?
            .port(creds.port),
    };
    // An empty username means the relay takes unauthenticated mail. Configuring
    // credentials anyway would make lettre attempt AUTH with an empty secret,
    // which fails on a listener that advertises no mechanism.
    if !creds.username.is_empty() {
        builder = builder.credentials(Credentials::new(
            creds.username.clone(),
            creds.password.expose().to_string(),
        ));
    }
    Ok((builder.build(), message))
}

#[cfg(test)]
#[path = "smtp_tests.rs"]
mod tests;
