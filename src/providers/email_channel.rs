#![allow(clippy::uninlined_format_args)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::trim_split_whitespace)]
#![allow(clippy::doc_link_with_quotes)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::unnecessary_map_or)]

use anyhow::Result;
#[cfg(feature = "email")]
use anyhow::anyhow;
#[cfg(feature = "email")]
use async_imap::Session;
#[cfg(feature = "email")]
use async_imap::extensions::idle::IdleResponse;
#[cfg(feature = "email")]
use async_imap::types::Fetch;
#[cfg(feature = "email")]
use async_trait::async_trait;
#[cfg(feature = "email")]
use futures::TryStreamExt;
use lettre::message::{Attachment, MultiPart, SinglePart, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
#[cfg(feature = "email")]
use mail_parser::{MessageParser, MimeHeaders};
#[cfg(feature = "email")]
use rustls::{ClientConfig, RootCertStore};
#[cfg(feature = "email")]
use rustls_pki_types::DnsName;
#[cfg(feature = "email")]
use std::collections::HashSet;
#[cfg(feature = "email")]
use std::sync::Arc;
#[cfg(feature = "email")]
use std::time::Duration;
#[cfg(feature = "email")]
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(feature = "email")]
use tokio::net::TcpStream;
#[cfg(feature = "email")]
use tokio::sync::Mutex;
#[cfg(feature = "email")]
use tokio::sync::mpsc;
#[cfg(feature = "email")]
use tokio::time::{sleep, timeout};
#[cfg(feature = "email")]
use tokio_rustls::TlsConnector;
#[cfg(feature = "email")]
use tokio_rustls::client::TlsStream;
use tracing::info;
#[cfg(feature = "email")]
use tracing::{debug, error, warn};
#[cfg(feature = "email")]
use uuid::Uuid;

pub use crate::config::EmailConfig;
#[cfg(feature = "email")]
use crate::traits::{Channel, ChannelMessage, SendMessage};

#[cfg(feature = "email")]
type ImapSession = Session<TlsStream<TcpStream>>;

/// Email channel — IMAP IDLE for instant push notifications, SMTP for outbound
pub struct EmailChannel {
    pub config: EmailConfig,
    /// Dedupe set for IMAP IDLE, which can re-report a message across
    /// re-establishes. A send-only build never opens a mailbox, so the field
    /// would be dead weight and a `never read` denial there.
    #[cfg(feature = "email")]
    seen_messages: Arc<Mutex<HashSet<String>>>,
}

impl EmailChannel {
    pub fn new(config: EmailConfig) -> Self {
        Self {
            config,
            #[cfg(feature = "email")]
            seen_messages: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Check if a sender email is in the allowlist
    pub fn is_sender_allowed(&self, email: &str) -> bool {
        if self.config.allowed_senders.is_empty() {
            return false; // Empty = deny all
        }
        if self.config.allowed_senders.iter().any(|a| a == "*") {
            return true; // Wildcard = allow all
        }
        let email_lower = email.to_lowercase();
        self.config.allowed_senders.iter().any(|allowed| {
            if allowed.starts_with('@') {
                // Domain match with @ prefix: "@example.com"
                email_lower.ends_with(&allowed.to_lowercase())
            } else if allowed.contains('@') {
                // Full email address match
                allowed.eq_ignore_ascii_case(email)
            } else {
                // Domain match without @ prefix: "example.com"
                email_lower.ends_with(&format!("@{}", allowed.to_lowercase()))
            }
        })
    }

    /// Strip HTML tags from content (basic)
    pub fn strip_html(html: &str) -> String {
        let mut result = String::new();
        let mut in_tag = false;
        for ch in html.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => result.push(ch),
                _ => {}
            }
        }
        let mut normalized = String::with_capacity(result.len());
        for word in result.split_whitespace() {
            if !normalized.is_empty() {
                normalized.push(' ');
            }
            normalized.push_str(word);
        }
        normalized
    }

    /// Extract the sender address from a parsed email
    #[cfg(feature = "email")]
    fn extract_sender(parsed: &mail_parser::Message) -> String {
        parsed
            .from()
            .and_then(|addr| addr.first())
            .and_then(|a| a.address())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".into())
    }

    /// Extract readable text from a parsed email
    #[cfg(feature = "email")]
    fn extract_text(parsed: &mail_parser::Message) -> String {
        if let Some(text) = parsed.body_text(0) {
            return text.to_string();
        }
        if let Some(html) = parsed.body_html(0) {
            return Self::strip_html(html.as_ref());
        }
        for part in parsed.attachments() {
            let part: &mail_parser::MessagePart = part;
            if let Some(ct) = MimeHeaders::content_type(part)
                && ct.ctype() == "text"
                && let Ok(text) = std::str::from_utf8(part.contents())
            {
                let name = MimeHeaders::attachment_name(part).unwrap_or("file");
                return format!("[Attachment: {}]\n{}", name, text);
            }
        }
        "(no readable content)".to_string()
    }

    /// Connect to IMAP server with TLS and authenticate
    #[cfg(feature = "email")]
    async fn connect_imap(&self) -> Result<ImapSession> {
        let addr = format!("{}:{}", self.config.imap_host, self.config.imap_port);
        debug!("Connecting to IMAP server at {}", addr);

        // Connect TCP
        let tcp = TcpStream::connect(&addr).await?;

        // Establish TLS using rustls
        let certs = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.into(),
        };
        let config = ClientConfig::builder()
            .with_root_certificates(certs)
            .with_no_client_auth();
        let tls_stream: TlsConnector = Arc::new(config).into();
        let sni: DnsName = self.config.imap_host.clone().try_into()?;
        let stream = tls_stream.connect(sni.into(), tcp).await?;

        // Create IMAP client
        let client = async_imap::Client::new(stream);

        // Login
        let session = client
            .login(&self.config.username, &self.config.password)
            .await
            .map_err(|(e, _)| anyhow!("IMAP login failed: {}", e))?;

        debug!("IMAP login successful");
        Ok(session)
    }

    /// Fetch and process unseen messages from the selected mailbox
    #[cfg(feature = "email")]
    async fn fetch_unseen(&self, session: &mut ImapSession) -> Result<Vec<ParsedEmail>> {
        // Search for unseen messages
        let uids = session.uid_search("UNSEEN").await?;
        if uids.is_empty() {
            return Ok(Vec::new());
        }

        debug!("Found {} unseen messages", uids.len());

        let mut results = Vec::new();
        let uid_set: String = uids
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");

        // Fetch message bodies
        let messages = session.uid_fetch(&uid_set, "RFC822").await?;
        let messages: Vec<Fetch> = messages.try_collect().await?;

        for msg in messages {
            let uid = msg.uid.unwrap_or(0);
            if let Some(body) = msg.body()
                && let Some(parsed) = MessageParser::default().parse(body)
            {
                let sender = Self::extract_sender(&parsed);
                let subject = parsed.subject().unwrap_or("(no subject)").to_string();
                let body_text = Self::extract_text(&parsed);
                let content = format!("Subject: {}\n\n{}", subject, body_text);
                let msg_id = parsed
                    .message_id()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("gen-{}", Uuid::new_v4()));

                #[allow(clippy::cast_sign_loss)]
                let ts = parsed
                    .date()
                    .map(|d| {
                        let naive = chrono::NaiveDate::from_ymd_opt(
                            d.year as i32,
                            u32::from(d.month),
                            u32::from(d.day),
                        )
                        .and_then(|date| {
                            date.and_hms_opt(
                                u32::from(d.hour),
                                u32::from(d.minute),
                                u32::from(d.second),
                            )
                        });
                        naive.map_or(0, |n| n.and_utc().timestamp() as u64)
                    })
                    .unwrap_or_else(|| {
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0)
                    });

                results.push(ParsedEmail {
                    _uid: uid,
                    msg_id,
                    sender,
                    content,
                    timestamp: ts,
                });
            }
        }

        // Mark fetched messages as seen
        if !results.is_empty() {
            let _ = session
                .uid_store(&uid_set, "+FLAGS (\\Seen)")
                .await?
                .try_collect::<Vec<_>>()
                .await;
        }

        Ok(results)
    }

    /// Run the IDLE loop, returning when a new message arrives or timeout
    /// Note: IDLE consumes the session and returns it via done()
    #[cfg(feature = "email")]
    async fn wait_for_changes(
        &self,
        session: ImapSession,
    ) -> Result<(IdleWaitResult, ImapSession)> {
        let idle_timeout = Duration::from_secs(self.config.idle_timeout_secs);

        // Start IDLE mode - this consumes the session
        let mut idle = session.idle();
        idle.init().await?;

        debug!("Entering IMAP IDLE mode");

        // wait() returns (future, stop_source) - we only need the future
        let (wait_future, _stop_source) = idle.wait();

        // Wait for server notification or timeout
        let result = timeout(idle_timeout, wait_future).await;

        match result {
            Ok(Ok(response)) => {
                debug!("IDLE response: {:?}", response);
                // Done with IDLE, return session to normal mode
                let session = idle.done().await?;
                let wait_result = match response {
                    IdleResponse::NewData(_) => IdleWaitResult::NewMail,
                    IdleResponse::Timeout => IdleWaitResult::Timeout,
                    IdleResponse::ManualInterrupt => IdleWaitResult::Interrupted,
                };
                Ok((wait_result, session))
            }
            Ok(Err(e)) => {
                // Try to clean up IDLE state
                let _ = idle.done().await;
                Err(anyhow!("IDLE error: {}", e))
            }
            Err(_) => {
                // Timeout - RFC 2177 recommends restarting IDLE every 29 minutes
                debug!("IDLE timeout reached, will re-establish");
                let session = idle.done().await?;
                Ok((IdleWaitResult::Timeout, session))
            }
        }
    }

    /// Main IDLE-based listen loop with automatic reconnection
    #[cfg(feature = "email")]
    async fn listen_with_idle(&self, tx: mpsc::Sender<ChannelMessage>) -> Result<()> {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(60);

        loop {
            match self.run_idle_session(&tx).await {
                Ok(()) => {
                    // Clean exit (channel closed)
                    return Ok(());
                }
                Err(e) => {
                    error!(
                        "IMAP session error: {}. Reconnecting in {:?}...",
                        e, backoff
                    );
                    sleep(backoff).await;
                    // Exponential backoff with cap
                    backoff = std::cmp::min(backoff * 2, max_backoff);
                }
            }
        }
    }

    /// Run a single IDLE session until error or clean shutdown
    #[cfg(feature = "email")]
    async fn run_idle_session(&self, tx: &mpsc::Sender<ChannelMessage>) -> Result<()> {
        // Connect and authenticate
        let mut session = self.connect_imap().await?;

        // Select the mailbox
        session.select(&self.config.imap_folder).await?;
        info!(
            "Email IDLE listening on {} (instant push enabled)",
            self.config.imap_folder
        );

        // Check for existing unseen messages first
        self.process_unseen(&mut session, tx).await?;

        loop {
            // Enter IDLE and wait for changes (consumes session, returns it via result)
            match self.wait_for_changes(session).await {
                Ok((IdleWaitResult::NewMail, returned_session)) => {
                    debug!("New mail notification received");
                    session = returned_session;
                    self.process_unseen(&mut session, tx).await?;
                }
                Ok((IdleWaitResult::Timeout, returned_session)) => {
                    // Re-check for mail after IDLE timeout (defensive)
                    session = returned_session;
                    self.process_unseen(&mut session, tx).await?;
                }
                Ok((IdleWaitResult::Interrupted, _)) => {
                    info!("IDLE interrupted, exiting");
                    return Ok(());
                }
                Err(e) => {
                    // Connection likely broken, need to reconnect
                    return Err(e);
                }
            }
        }
    }

    /// Fetch unseen messages and send to channel
    #[cfg(feature = "email")]
    async fn process_unseen(
        &self,
        session: &mut ImapSession,
        tx: &mpsc::Sender<ChannelMessage>,
    ) -> Result<()> {
        let messages = self.fetch_unseen(session).await?;

        for email in messages {
            // Check allowlist
            if !self.is_sender_allowed(&email.sender) {
                warn!("Blocked email from {}", email.sender);
                continue;
            }

            let is_new = {
                let mut seen = self.seen_messages.lock().await;
                seen.insert(email.msg_id.clone())
            };
            if !is_new {
                continue;
            }

            let msg = ChannelMessage {
                id: email.msg_id,
                reply_target: email.sender.clone(),
                sender: email.sender,
                content: email.content,
                channel: "email".to_string(),
                timestamp: email.timestamp,
                thread_ts: None,
            };

            if tx.send(msg).await.is_err() {
                // Channel closed, exit cleanly
                return Ok(());
            }
        }

        Ok(())
    }

    fn create_smtp_transport(&self) -> Result<SmtpTransport> {
        let creds = Credentials::new(self.config.username.clone(), self.config.password.clone());
        let transport = if self.config.smtp_tls {
            SmtpTransport::relay(&self.config.smtp_host)?
                .port(self.config.smtp_port)
                .credentials(creds)
                .build()
        } else {
            SmtpTransport::builder_dangerous(&self.config.smtp_host)
                .port(self.config.smtp_port)
                .credentials(creds)
                .build()
        };
        Ok(transport)
    }

    /// Stub.
    pub fn smtp_credentials(&self) -> crate::providers::mail::SmtpCredentials {
        crate::providers::mail::SmtpCredentials { host: String::new(), port: 0, security: Default::default(), username: String::new(), password: Default::default(), from_name: String::new(), from_email: String::new() }
    }

    /// Stub.
    pub fn imap_credentials(&self) -> crate::providers::mail::ImapCredentials {
        crate::providers::mail::ImapCredentials { host: String::new(), port: 0, username: String::new(), password: Default::default() }
    }

    pub fn send_message(&self, email: Message) -> Result<()> {
        let transport = self.create_smtp_transport()?;
        transport.send(&email)?;
        info!("Email sent");
        Ok(())
    }

    pub fn build_plain_message(
        &self,
        recipient: &str,
        subject: &str,
        body: &str,
    ) -> Result<Message> {
        Message::builder()
            .from(self.config.from_address.parse()?)
            .to(recipient.parse()?)
            .subject(subject)
            .singlepart(SinglePart::plain(body.to_string()))
            .map_err(Into::into)
    }

    pub fn build_message_with_attachment(
        &self,
        recipient: &str,
        subject: &str,
        body: &str,
        attachment_name: &str,
        content_type: ContentType,
        attachment_bytes: Vec<u8>,
    ) -> Result<Message> {
        let attachment =
            Attachment::new(attachment_name.to_string()).body(attachment_bytes, content_type);
        Message::builder()
            .from(self.config.from_address.parse()?)
            .to(recipient.parse()?)
            .subject(subject)
            .multipart(
                MultiPart::mixed()
                    .singlepart(SinglePart::plain(body.to_string()))
                    .singlepart(attachment),
            )
            .map_err(Into::into)
    }
}

/// Internal struct for parsed email data
#[cfg(feature = "email")]
struct ParsedEmail {
    _uid: u32,
    msg_id: String,
    sender: String,
    content: String,
    timestamp: u64,
}

/// Result from waiting on IDLE
#[cfg(feature = "email")]
enum IdleWaitResult {
    NewMail,
    Timeout,
    Interrupted,
}

#[cfg(feature = "email")]
#[async_trait]
impl Channel for EmailChannel {
    fn name(&self) -> &str {
        "email"
    }

    async fn send(&self, message: &SendMessage) -> Result<()> {
        // Use explicit subject if provided, otherwise fall back to legacy parsing or default
        let (subject, body) = if let Some(ref subj) = message.subject {
            (subj.as_str(), message.content.as_str())
        } else if message.content.starts_with("Subject: ") {
            if let Some(pos) = message.content.find('\n') {
                (&message.content[9..pos], message.content[pos + 1..].trim())
            } else {
                ("OpenHuman Message", message.content.as_str())
            }
        } else {
            ("OpenHuman Message", message.content.as_str())
        };

        let email = self.build_plain_message(message.recipient.as_str(), subject, body)?;
        self.send_message(email)?;
        info!("Email sent to {}", message.recipient);
        Ok(())
    }

    async fn listen(&self, tx: mpsc::Sender<ChannelMessage>) -> Result<()> {
        info!(
            "Starting email channel with IDLE support on {}",
            self.config.imap_folder
        );
        self.listen_with_idle(tx).await
    }

    async fn health_check(&self) -> bool {
        // Fully async health check - attempt IMAP connection
        match timeout(Duration::from_secs(10), self.connect_imap()).await {
            Ok(Ok(mut session)) => {
                // Try to logout cleanly
                let _ = session.logout().await;
                true
            }
            Ok(Err(e)) => {
                debug!("Health check failed: {}", e);
                false
            }
            Err(_) => {
                debug!("Health check timed out");
                false
            }
        }
    }
}

#[cfg(all(test, feature = "email"))]
#[path = "email_channel_tests.rs"]
mod tests;

#[cfg(all(feature = "email", any(test, debug_assertions)))]
#[path = "email_channel_test_support_tests.rs"]
pub mod test_support;

/// The send-only surface, exercised in a build that has no IMAP stack.
///
/// This is the half of the split a compile check cannot state on its own: with
/// `email-send` on and `email` off the crate builds either way, so nothing
/// would notice if the send path quietly grew a dependency on the receive half
/// and had to be gated along with it. `voice` in OpenHuman reaches for exactly
/// these three items and nothing else, so this is the contract to keep.
#[cfg(all(test, feature = "email-send", not(feature = "email")))]
#[path = "email_channel_send_only_tests.rs"]
mod send_only_tests;
