//! The async IMAP [`MailReceiver`] and the message parser (feature `email`).
//!
//! Ported from OpenCompany's `server/ops/imap.rs`. Poll-shaped rather than
//! IDLE-shaped (that is [`EmailChannel::listen`](crate::providers::EmailChannel)):
//! each call opens a short-lived TLS session with the credentials it is
//! handed, so one receiver serves any number of mailboxes.
//!
//! Fetch and acknowledge are separate on purpose. [`MailReceiver::fetch_new`]
//! reads with `BODY.PEEK[]`, which does not set `\Seen`; only
//! [`MailReceiver::mark_seen`] does, once the caller has durably filed the
//! message. A storage failure therefore leaves the message unseen and it is
//! fetched again, instead of being marked and lost.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::TryStreamExt;
use mail_parser::MessageParser;
use rustls::{ClientConfig, RootCertStore};
use rustls_pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::types::{FetchedEmail, ImapCredentials, InboundEmail, MailError, MailReceiver};

/// Parses one RFC 822 message into an [`InboundEmail`]: the first `From`
/// mailbox, the subject, and the first plain-text body part. Anything absent —
/// or a message that does not parse at all — leaves that field empty.
pub fn parse_message(raw: &[u8]) -> InboundEmail {
    let Some(parsed) = MessageParser::default().parse(raw) else {
        return InboundEmail::default();
    };
    let (from_name, from_email) = parsed
        .from()
        .and_then(|address| address.first())
        .map(|addr| {
            (
                addr.name().unwrap_or_default().to_string(),
                addr.address().unwrap_or_default().to_string(),
            )
        })
        .unwrap_or_default();
    InboundEmail {
        from_name,
        from_email,
        subject: parsed.subject().unwrap_or_default().to_string(),
        body: parsed
            .body_text(0)
            .map(|text| text.to_string())
            .unwrap_or_default(),
    }
}

/// The async IMAP [`MailReceiver`]: implicit TLS (trusting the Mozilla root
/// set from `webpki-roots`, no OS trust store), `LOGIN`, `SELECT` of
/// [`Self::mailbox`], then `UID SEARCH UNSEEN` + `UID FETCH (UID BODY.PEEK[])`
/// or `UID STORE +FLAGS.SILENT (\Seen)`.
///
/// Each call is bounded end to end by [`Self::with_timeout`] (default
/// [`Self::DEFAULT_TIMEOUT`]) so an unresponsive server cannot hang a poller.
#[derive(Debug, Clone)]
pub struct AsyncImapReceiver {
    mailbox: String,
    timeout: Duration,
}

impl AsyncImapReceiver {
    /// The end-to-end bound on one call when none is configured.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

    /// A receiver for `INBOX` with the default timeout.
    pub fn new() -> Self {
        Self {
            mailbox: "INBOX".to_string(),
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    /// Selects a mailbox other than `INBOX`.
    #[must_use]
    pub fn with_mailbox(mut self, mailbox: impl Into<String>) -> Self {
        self.mailbox = mailbox.into();
        self
    }

    /// Overrides the end-to-end bound on one call (connect through logout).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The mailbox this receiver selects.
    pub fn mailbox(&self) -> &str {
        &self.mailbox
    }

    /// Opens a TLS session, logs in and selects [`Self::mailbox`].
    async fn connect(&self, creds: &ImapCredentials) -> Result<Session<TlsSocket>, MailError> {
        let tcp = TcpStream::connect((creds.host.as_str(), creds.port))
            .await
            .map_err(|e| MailError::Transport(format!("imap connect: {e}")))?;
        let domain = ServerName::try_from(creds.host.clone())
            .map_err(|e| MailError::Transport(format!("imap tls domain: {e}")))?;
        let tls = tls_connector()?
            .connect(domain, tcp)
            .await
            .map_err(|e| MailError::Transport(format!("imap tls connect: {e}")))?;
        open_session(tls, creds, &self.mailbox).await
    }

    async fn bounded<T>(
        &self,
        work: impl std::future::Future<Output = Result<T, MailError>>,
    ) -> Result<T, MailError> {
        tokio::time::timeout(self.timeout, work)
            .await
            .unwrap_or(Err(MailError::TimedOut))
    }
}

impl Default for AsyncImapReceiver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MailReceiver for AsyncImapReceiver {
    async fn fetch_new(&self, creds: &ImapCredentials) -> Result<Vec<FetchedEmail>, MailError> {
        self.bounded(async {
            let mut session = self.connect(creds).await?;
            let fetched = fetch_unseen_on(&mut session).await?;
            let _ = session.logout().await;
            Ok(fetched)
        })
        .await
    }

    async fn mark_seen(&self, creds: &ImapCredentials, uids: &[u32]) -> Result<(), MailError> {
        if uids.is_empty() {
            return Ok(());
        }
        self.bounded(async {
            let mut session = self.connect(creds).await?;
            store_seen_on(&mut session, uids).await?;
            let _ = session.logout().await;
            Ok(())
        })
        .await
    }
}

type TlsSocket = tokio_rustls::client::TlsStream<TcpStream>;
type Session<S> = async_imap::Session<S>;

/// The bounds `async-imap` (runtime-tokio) needs of a connected stream.
trait ImapStream: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send {}
impl<S: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send> ImapStream for S {}

/// A rustls connector over the Mozilla root set, using the crate's `ring`
/// provider explicitly so it never depends on a process-default provider.
fn tls_connector() -> Result<TlsConnector, MailError> {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.into(),
    };
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| MailError::Transport(format!("imap tls config: {e}")))?
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

/// Logs in and selects `mailbox` over an already-connected `stream`.
/// Transport-agnostic so the protocol exchange is testable over plain TCP.
async fn open_session<S: ImapStream>(
    stream: S,
    creds: &ImapCredentials,
    mailbox: &str,
) -> Result<Session<S>, MailError> {
    let mut session = async_imap::Client::new(stream)
        .login(&creds.username, creds.password.expose())
        .await
        .map_err(|(e, _)| MailError::Transport(format!("imap login: {e}")))?;
    session
        .select(mailbox)
        .await
        .map_err(|e| MailError::Transport(format!("imap select: {e}")))?;
    Ok(session)
}

/// `UID SEARCH UNSEEN` then `UID FETCH <uids> (UID BODY.PEEK[])`.
///
/// UIDs, not sequence numbers, because those shift on expunge. The parentheses
/// are load-bearing: async-imap sends the query verbatim, RFC 3501 requires a
/// multi-item fetch to be a parenthesized list, and strict servers (Stalwart)
/// otherwise drop everything past the first item, so every body came back
/// empty.
async fn fetch_unseen_on<S: ImapStream>(
    session: &mut Session<S>,
) -> Result<Vec<FetchedEmail>, MailError> {
    let unseen = session
        .uid_search("UNSEEN")
        .await
        .map_err(|e| MailError::Transport(format!("imap search: {e}")))?;
    if unseen.is_empty() {
        return Ok(Vec::new());
    }
    let mut uids: Vec<u32> = unseen.into_iter().collect();
    uids.sort_unstable();
    let mut fetches = session
        .uid_fetch(uid_set(&uids), "(UID BODY.PEEK[])")
        .await
        .map_err(|e| MailError::Transport(format!("imap fetch: {e}")))?;
    let mut out = Vec::new();
    while let Some(fetch) = fetches
        .try_next()
        .await
        .map_err(|e| MailError::Transport(format!("imap fetch stream: {e}")))?
    {
        if let (Some(uid), Some(body)) = (fetch.uid, fetch.body()) {
            out.push(FetchedEmail {
                uid,
                email: parse_message(body),
            });
        }
    }
    Ok(out)
}

/// `UID STORE <uids> +FLAGS.SILENT (\Seen)`, draining any untagged replies so
/// they cannot desync the next command on the session.
async fn store_seen_on<S: ImapStream>(
    session: &mut Session<S>,
    uids: &[u32],
) -> Result<(), MailError> {
    let updates = session
        .uid_store(uid_set(uids), "+FLAGS.SILENT (\\Seen)")
        .await
        .map_err(|e| MailError::Transport(format!("imap store: {e}")))?;
    let _: Vec<_> = updates
        .try_collect()
        .await
        .map_err(|e| MailError::Transport(format!("imap store stream: {e}")))?;
    Ok(())
}

fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
#[path = "imap_tests.rs"]
mod tests;
