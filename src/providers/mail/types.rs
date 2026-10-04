//! Mail transport vocabulary: provider-tagged credentials, the messages that
//! cross the seam, and the [`MailSender`] / [`MailReceiver`] traits.
//!
//! Dependency-free on purpose, so it compiles in every build: a host can hold
//! credentials, write its own sender (or a test double), and accept a
//! `dyn MailSender` without enabling `email-send` and linking `lettre`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Which transport a set of [`MailCredentials`] belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailProvider {
    /// Any SMTP submission server. Also covers AWS SES, Mailgun, SendGrid and
    /// Postmark through their SMTP endpoints.
    #[default]
    Smtp,
}

impl std::fmt::Display for MailProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MailProvider::Smtp => f.write_str("smtp"),
        }
    }
}

impl std::str::FromStr for MailProvider {
    type Err = MailError;

    /// Parses a provider name, case-insensitively and ignoring surrounding
    /// whitespace — the spelling an operator writes in an environment variable.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "smtp" => Ok(MailProvider::Smtp),
            other => Err(MailError::Config(format!(
                "unknown mail provider {other:?}; supported: smtp"
            ))),
        }
    }
}

/// Why a mail operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MailError {
    /// The configuration cannot be used: an unknown provider name, or
    /// credentials for a provider this sender does not implement.
    #[error("mail configuration error: {0}")]
    Config(String),
    /// A `from` or `to` address does not parse. Nothing was sent.
    #[error("invalid mail address: {0}")]
    InvalidAddress(String),
    /// The transport failed: connect, TLS, authentication, a protocol error,
    /// or the server refusing the message. The message was not accepted.
    #[error("mail transport error: {0}")]
    Transport(String),
    /// The exchange did not finish within the transport's deadline. Treat it
    /// like a refusal: the message must not be recorded as delivered.
    #[error("mail transport timed out")]
    TimedOut,
}

/// A mail password. Reads in plain from configuration, never writes out.
///
/// `Debug` **and** `Serialize` both render `"[redacted]"`, so any struct that
/// embeds one may derive either without leaking it — the guard sits on the
/// field's type rather than on each container. `Deserialize` accepts the
/// plaintext, which is how a configured or stored credential gets in. Read the
/// value with [`Self::expose`] at the one place it is presented to a server.
#[derive(Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct MailSecret(String);

impl MailSecret {
    /// The text written in place of the secret by `Debug` and `Serialize`.
    pub const REDACTED: &'static str = "[redacted]";

    /// Wraps a secret value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret itself — call only where it is handed to a transport.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether no secret was supplied.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for MailSecret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for MailSecret {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Debug for MailSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(Self::REDACTED)
    }
}

impl Serialize for MailSecret {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(Self::REDACTED)
    }
}

/// How an SMTP connection is secured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpSecurity {
    /// No transport security (a local relay or a test fixture).
    None,
    /// STARTTLS upgrade on the submission port (usually 587).
    #[default]
    Starttls,
    /// Implicit TLS, SMTPS (usually 465).
    Ssl,
}

/// One SMTP account. The password is a [`MailSecret`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SmtpCredentials {
    /// SMTP server host.
    pub host: String,
    /// SMTP server port.
    pub port: u16,
    /// Transport security mode.
    #[serde(default)]
    pub security: SmtpSecurity,
    /// Login username. Empty means the relay takes unauthenticated mail, so no
    /// `AUTH` is attempted.
    pub username: String,
    /// Login password.
    pub password: MailSecret,
    /// Display name on the `From` header; empty for a bare address.
    #[serde(default)]
    pub from_name: String,
    /// Envelope and `From` address.
    pub from_email: String,
}

/// One IMAP mailbox login. The password is a [`MailSecret`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImapCredentials {
    /// IMAP server host (implicit TLS).
    pub host: String,
    /// IMAP server port (usually 993).
    pub port: u16,
    /// Login username.
    pub username: String,
    /// Login password.
    pub password: MailSecret,
}

/// Credentials for one outbound provider, tagged by `provider` on the wire so
/// a stored blob names its own transport.
///
/// Deliberately exhaustive: adding a provider adds a variant, and every
/// `match` over this type — each [`MailSender`] among them — stops compiling
/// until someone decides what it does with the new one.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "lowercase")]
pub enum MailCredentials {
    /// An SMTP submission server.
    Smtp(SmtpCredentials),
}

impl MailCredentials {
    /// Which transport these credentials need.
    pub fn provider(&self) -> MailProvider {
        match self {
            MailCredentials::Smtp(_) => MailProvider::Smtp,
        }
    }

    /// The address mail sent with these credentials comes from.
    pub fn from_email(&self) -> &str {
        match self {
            MailCredentials::Smtp(creds) => &creds.from_email,
        }
    }

    /// The `From` display name, empty when none is configured.
    pub fn from_name(&self) -> &str {
        match self {
            MailCredentials::Smtp(creds) => &creds.from_name,
        }
    }
}

/// One outbound plain-text message handed to a [`MailSender`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboundEmail {
    /// Recipient address.
    pub to: String,
    /// Subject line.
    pub subject: String,
    /// Plain-text body.
    pub body: String,
}

/// One inbound message, reduced to its sender, subject and plain-text body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InboundEmail {
    /// Sender display name, empty when absent.
    pub from_name: String,
    /// Sender address, empty when absent.
    pub from_email: String,
    /// Subject, empty when absent.
    pub subject: String,
    /// First plain-text body part, empty when there is none.
    pub body: String,
}

/// A fetched but not yet acknowledged message, with the IMAP UID it was
/// fetched under. A UID (unlike a sequence number) does not shift on expunge,
/// so filing and [`MailReceiver::mark_seen`] stay correctly paired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedEmail {
    /// IMAP UID within the selected mailbox.
    pub uid: u32,
    /// The parsed message.
    pub email: InboundEmail,
}

/// The outbound seam. Credentials travel with each call, so one sender serves
/// any number of accounts.
#[async_trait]
pub trait MailSender: Send + Sync {
    /// Sends `email` with `creds`. `Ok` means the server accepted the message.
    ///
    /// # Errors
    ///
    /// [`MailError::InvalidAddress`] before any connection is made for an
    /// address that does not parse, [`MailError::Transport`] or
    /// [`MailError::TimedOut`] when the message was not accepted, and
    /// [`MailError::Config`] for credentials of a provider the sender does not
    /// implement (it must not panic: the binary may lack that feature).
    async fn send(&self, creds: &MailCredentials, email: &OutboundEmail) -> Result<(), MailError>;
}

/// The inbound seam: fetch unseen messages **without** marking them, then
/// acknowledge only what the caller durably filed.
#[async_trait]
pub trait MailReceiver: Send + Sync {
    /// Fetches messages not yet `\Seen`, leaving the flag unset — a message is
    /// re-fetched until [`Self::mark_seen`] is called for its UID.
    ///
    /// # Errors
    ///
    /// [`MailError::Transport`] or [`MailError::TimedOut`].
    async fn fetch_new(&self, creds: &ImapCredentials) -> Result<Vec<FetchedEmail>, MailError>;

    /// Marks `uids` `\Seen`. Pass only UIDs already durably filed: a storage
    /// failure must never cause an unfiled message to be marked and lost.
    ///
    /// # Errors
    ///
    /// [`MailError::Transport`] or [`MailError::TimedOut`].
    async fn mark_seen(&self, creds: &ImapCredentials, uids: &[u32]) -> Result<(), MailError>;
}
