//! Inbound email on the typed path: subject and thread headers, attachments
//! and voice attachments, published on the ordered ingress path, with the
//! legacy channel as the fallback for text.

use super::native::{EMAIL_MAX_FILE_BYTES, EMAIL_MAX_VOICE_BYTES, bare_id};
use super::{EmailChannel, ParsedEmail};
#[cfg(test)]
use crate::providers::native::inbound::publish;
use crate::providers::native::inbound::{
    InboundFile, Ingest, Ingested, assemble, media_of, normalize_mime,
};
#[cfg(test)]
use crate::traits::ChannelMessage;
use mail_parser::{HeaderValue, MessageParser, MimeHeaders};
use serde_json::json;
use tinychannels_bus::{
    ChannelInboundEnvelope, ChannelRef, ConversationKind, ConversationRef, SenderRef,
};
#[cfg(test)]
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};
use tracing::info;

/// Bound on one IMAP connect + TLS + login attempt. Without it a server that
/// accepts TCP and never answers would hold the reconnect loop forever.
pub(super) const IMAP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

fn email_ref() -> ChannelRef {
    ChannelRef {
        id: "email".into(),
        account_id: None,
    }
}

/// Every message id a header names, bare.
fn ids_of(value: &HeaderValue<'_>) -> Vec<String> {
    match value {
        HeaderValue::Text(text) => vec![bare_id(text).to_owned()],
        HeaderValue::TextList(list) => list.iter().map(|id| bare_id(id).to_owned()).collect(),
        _ => Vec::new(),
    }
}

impl EmailChannel {
    /// Readable body text; empty when the mail only carries attachments.
    fn body_text(parsed: &mail_parser::Message<'_>) -> String {
        if let Some(text) = parsed.body_text(0) {
            return text.trim().to_owned();
        }
        parsed
            .body_html(0)
            .map(|html| Self::strip_html(html.as_ref()))
            .unwrap_or_default()
    }

    /// Hand one fetched, allowed, first-seen email to the host. False when the
    /// legacy receiver is gone and the listener should stop.
    #[cfg(test)]
    pub(super) async fn dispatch_email(
        &self,
        email: ParsedEmail,
        tx: &mpsc::Sender<ChannelMessage>,
    ) -> bool {
        if self.native.events.is_some() && self.publish_typed(&email).await {
            return true;
        }
        let message = ChannelMessage {
            id: email.msg_id,
            reply_target: email.sender.clone(),
            sender: email.sender,
            content: email.content,
            channel: "email".to_string(),
            timestamp: email.timestamp,
            topic_id: None,
            reply_to_id: None,
            thread_ts: None,
            sender_name: None,
        };
        tx.send(message).await.is_ok()
    }

    /// Publish `email` through the compatibility event sink.
    #[cfg(test)]
    async fn publish_typed(&self, email: &ParsedEmail) -> bool {
        let Some(envelope) = self.typed_envelope(email).await else {
            return false;
        };
        publish("email", self.native.events.as_ref(), envelope).await
    }

    /// Deliver an email through the ordered native ingress sink. Rejections
    /// are propagated to listener supervision and never fall back to the
    /// legacy channel queue.
    pub(super) async fn dispatch_typed_email(
        &self,
        email: &ParsedEmail,
        sink: &(dyn crate::channel::ChannelInboundSink + Send + Sync),
    ) -> anyhow::Result<()> {
        let Some(envelope) = self.typed_envelope(email).await else {
            anyhow::bail!("Email message could not be parsed for typed ingress");
        };
        sink.push_ingress(tinychannels_bus::ChannelIngress::Inbound(envelope))
            .await
            .map_err(|_| anyhow::Error::new(super::InboundSinkRejected))
    }

    /// Construct a typed envelope with host-scoped media and voice parts.
    async fn typed_envelope(&self, email: &ParsedEmail) -> Option<ChannelInboundEnvelope> {
        let parsed = MessageParser::default().parse(&email.raw)?;
        let subject = parsed.subject().unwrap_or("(no subject)").to_owned();
        let in_reply_to = ids_of(parsed.in_reply_to()).into_iter().next();
        let references = ids_of(parsed.references());
        // The thread is rooted at the oldest message it names, else at this one.
        let thread = references
            .first()
            .cloned()
            .or_else(|| in_reply_to.clone())
            .unwrap_or_else(|| email.msg_id.clone());
        self.subjects.remember(&email.msg_id, &subject);
        let conversation = ConversationRef {
            kind: ConversationKind::Dm,
            id: email.sender.clone(),
            thread_id: Some(thread),
            ..Default::default()
        };
        let mut ingested = Ingested::default();
        let ingest = Ingest {
            provider: "email",
            channel: email_ref(),
            repository: self.native.media_repository.as_ref(),
            voice_store: self.native.voice_media_store.as_ref(),
            transcriber: self.native.transcriber.as_ref(),
            file_bytes: EMAIL_MAX_FILE_BYTES,
            voice_bytes: EMAIL_MAX_VOICE_BYTES,
        };
        for (index, part) in parsed.attachments().enumerate() {
            let part: &mail_parser::MessagePart = part;
            let mime = MimeHeaders::content_type(part)
                .map(|ct| match ct.subtype() {
                    Some(subtype) => format!("{}/{subtype}", ct.ctype()),
                    None => ct.ctype().to_owned(),
                })
                .map(|mime| normalize_mime(&mime))
                .unwrap_or_else(|| "application/octet-stream".into());
            // Text parts are the body (or an inline quoted copy), not files.
            if mime.starts_with("text/") || mime.starts_with("multipart/") {
                continue;
            }
            ingest
                .add(
                    &mut ingested,
                    &conversation,
                    &email.msg_id,
                    InboundFile {
                        bytes: part.contents().to_vec(),
                        // Any audio attachment is treated as a voice note.
                        voice: mime.starts_with("audio/"),
                        mime,
                        name: MimeHeaders::attachment_name(part).map(str::to_owned),
                        duration_ms: None,
                        dedupe_key: format!("{}:{index}", email.msg_id),
                        transcript: None,
                    },
                )
                .await;
        }
        let body = Self::body_text(&parsed);
        let text = if body.is_empty() {
            format!("Subject: {subject}")
        } else {
            format!("Subject: {subject}\n\n{body}")
        };
        let parts = assemble(&text, None, ingested);
        let envelope = ChannelInboundEnvelope {
            channel: email_ref(),
            message_id: email.msg_id.clone(),
            reply_to_id: in_reply_to.clone(),
            timestamp_unix_ms: Some(email.timestamp.saturating_mul(1000)),
            conversation,
            sender: SenderRef {
                id: email.sender.clone(),
                ..Default::default()
            },
            text,
            media: media_of(&parts),
            parts,
            raw: Some(json!({
                "subject": subject,
                "messageId": email.msg_id,
                "inReplyTo": in_reply_to,
                "references": references,
            })),
            ..Default::default()
        };
        Some(envelope)
    }
}

#[cfg(test)]
#[path = "inbound_tests.rs"]
mod tests;

// IMAP polling and message decoding remain owned by the email provider.
use super::*;

impl EmailChannel {
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
    pub(super) fn extract_sender(parsed: &mail_parser::Message) -> String {
        parsed
            .from()
            .and_then(|addr| addr.first())
            .and_then(|a| a.address())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".into())
    }

    /// Extract readable text from a parsed email
    #[cfg(feature = "email")]
    pub(super) fn extract_text(parsed: &mail_parser::Message) -> String {
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
                return text.to_string();
            }
        }
        "(no readable content)".to_string()
    }

    /// Connect to IMAP server with TLS and authenticate
    #[cfg(feature = "email")]
    pub(super) async fn connect_imap(&self) -> Result<ImapSession> {
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
        self.login_imap(stream).await
    }

    /// Log in over an already-connected stream.
    #[cfg(feature = "email")]
    pub(super) async fn login_imap<S: ImapStream>(&self, stream: S) -> Result<Session<S>> {
        let client = async_imap::Client::new(stream);

        // Login
        let session = client
            .login(&self.config.username, &self.config.password)
            .await
            .map_err(|(e, _)| match e {
                async_imap::error::Error::No(_) | async_imap::error::Error::Bad(_) => {
                    anyhow::Error::new(super::ImapLoginRejected(e.to_string()))
                }
                other => anyhow!("IMAP login failed: {}", other),
            })?;

        debug!("IMAP login successful");
        Ok(session)
    }

    /// Fetch and process unseen messages from the selected mailbox
    #[cfg(feature = "email")]
    pub(super) async fn fetch_unseen<S: ImapStream>(
        &self,
        session: &mut Session<S>,
        uid_validity: u32,
    ) -> Result<Vec<ParsedEmail>> {
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
        let messages = session.uid_fetch(&uid_set, "(UID BODY.PEEK[])").await?;
        let messages: Vec<Fetch> = messages.try_collect().await?;

        for msg in messages {
            let uid = msg.uid.unwrap_or(0);
            if let Some(body) = msg.body()
                && let Some(parsed) = MessageParser::default().parse(body)
            {
                let sender = Self::extract_sender(&parsed);
                #[cfg(test)]
                let subject = parsed.subject().unwrap_or("(no subject)").to_string();
                #[cfg(test)]
                let body_text = Self::extract_text(&parsed);
                #[cfg(test)]
                let content = format!("Subject: {}\n\n{}", subject, body_text);
                let msg_id = parsed
                    .message_id()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        let identity = format!(
                            "{}:{}:{}:{}:{}",
                            self.config.imap_host.to_ascii_lowercase(),
                            self.config.username.to_ascii_lowercase(),
                            self.config.imap_folder,
                            uid_validity,
                            uid
                        );
                        format!("email:{}", hex::encode(sha2::Sha256::digest(identity)))
                    });

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
                    #[cfg(test)]
                    content,
                    timestamp: ts,
                    raw: body.to_vec(),
                });
            }
        }

        Ok(results)
    }

    /// Run the IDLE loop, returning when a new message arrives or timeout
    /// Note: IDLE consumes the session and returns it via done()
    #[cfg(feature = "email")]
    pub(super) async fn wait_for_changes<S: ImapStream>(
        &self,
        session: Session<S>,
    ) -> Result<(IdleWaitResult, Session<S>)> {
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
                Ok((IdleWaitResult::from_response(&response), session))
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

    #[cfg(feature = "email")]
    pub(super) async fn listen_typed_with<S, F, Fut>(
        &self,
        sink: &(dyn ChannelInboundSink + Send + Sync),
        connect: F,
    ) -> std::result::Result<(), ChannelSendError>
    where
        S: ImapStream,
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Session<S>>>,
    {
        let (cancel, mut cancellation) = watch::channel(false);
        let generation = self
            .listener_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        {
            let mut active = self
                .listener_cancel
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if active.is_some() {
                return Err(ChannelSendError {
                    message: "Email listener is already running".into(),
                    retryable: true,
                    ..Default::default()
                });
            }
            *active = Some((generation, cancel));
        }
        let _registration = ListenerRegistration {
            slot: self.listener_cancel.clone(),
            generation,
        };

        let result = tokio::select! {
            biased;
            _ = cancellation.wait_for(|stopped| *stopped) => Ok(()),
            result = self.listen_with_reconnect_to(EmailDelivery::Typed(sink), connect) => result,
        };
        result.map_err(|_| ChannelSendError {
            kind: tinychannels_bus::channel::SendErrorKind::Transient,
            message: "Email listener stopped after an inbound delivery error".into(),
            retryable: true,
            ..Default::default()
        })
    }

    /// The reconnect loop over any session source, so it is testable without
    /// a TLS server.
    #[cfg(all(feature = "email", test))]
    pub(super) async fn listen_with_reconnect<S, F, Fut>(
        &self,
        tx: &mpsc::Sender<ChannelMessage>,
        connect: F,
    ) -> Result<()>
    where
        S: ImapStream,
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Session<S>>>,
    {
        self.listen_with_reconnect_to(EmailDelivery::Legacy(tx), connect)
            .await
    }

    #[cfg(feature = "email")]
    pub(super) async fn listen_with_reconnect_to<S, F, Fut>(
        &self,
        delivery: EmailDelivery<'_>,
        connect: F,
    ) -> Result<()>
    where
        S: ImapStream,
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Session<S>>>,
    {
        let mut backoff = self.reconnect_backoff;
        let max_backoff = self.max_reconnect_backoff;
        let mut streak = crate::runtime::FailureStreak::default();

        loop {
            let logged_in = std::sync::atomic::AtomicBool::new(false);
            let attempt = async {
                let session = timeout(IMAP_CONNECT_TIMEOUT, connect())
                    .await
                    .map_err(|_| {
                        anyhow!("IMAP connect timed out after {IMAP_CONNECT_TIMEOUT:?}")
                    })??;
                logged_in.store(true, std::sync::atomic::Ordering::Relaxed);
                Ok::<_, anyhow::Error>(session)
            };
            match self.run_idle_session(&delivery, attempt).await {
                Ok(()) => {
                    // Clean exit (channel closed)
                    return Ok(());
                }
                Err(error) if error.downcast_ref::<InboundSinkRejected>().is_some() => {
                    return Err(error);
                }
                Err(e) => {
                    // A session that logged in was healthy: it ends any streak
                    // and the backoff starts over.
                    if logged_in.load(std::sync::atomic::Ordering::Relaxed) {
                        streak.record_success();
                        backoff = self.reconnect_backoff;
                    }
                    let escalate = streak.record_failure();
                    if escalate || e.downcast_ref::<super::ImapLoginRejected>().is_some() {
                        error!(
                            "IMAP session error ({} in a row): {}. Reconnecting in {:?}...",
                            streak.failures(),
                            e,
                            backoff
                        );
                    } else {
                        warn!(
                            "IMAP session error: {}. Reconnecting in {:?}...",
                            e, backoff
                        );
                    }
                    sleep(backoff).await;
                    // Exponential backoff with cap
                    backoff = std::cmp::min(backoff * 2, max_backoff);
                }
            }
        }
    }

    /// Run a single IDLE session until error or clean shutdown
    #[cfg(feature = "email")]
    pub(super) async fn run_idle_session<S: ImapStream>(
        &self,
        delivery: &EmailDelivery<'_>,
        connect: impl std::future::Future<Output = Result<Session<S>>>,
    ) -> Result<()> {
        // Connect and authenticate
        let mut session = connect.await?;

        // Select the mailbox
        let uid_validity = session
            .select(&self.config.imap_folder)
            .await?
            .uid_validity
            .unwrap_or_default();
        info!(
            "Email IDLE listening on {} (instant push enabled)",
            self.config.imap_folder
        );

        // Check for existing unseen messages first
        self.process_unseen_to(&mut session, delivery, uid_validity)
            .await?;

        loop {
            // Enter IDLE and wait for changes (consumes session, returns it via result)
            match self.wait_for_changes(session).await {
                Ok((IdleWaitResult::NewMail, returned_session)) => {
                    debug!("New mail notification received");
                    session = returned_session;
                    self.process_unseen_to(&mut session, delivery, uid_validity)
                        .await?;
                }
                Ok((IdleWaitResult::Timeout, returned_session)) => {
                    // Re-check for mail after IDLE timeout (defensive)
                    session = returned_session;
                    self.process_unseen_to(&mut session, delivery, uid_validity)
                        .await?;
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
    #[cfg(test)]
    pub(super) async fn process_unseen<S: ImapStream>(
        &self,
        session: &mut Session<S>,
        tx: &mpsc::Sender<ChannelMessage>,
    ) -> Result<()> {
        self.process_unseen_to(session, &EmailDelivery::Legacy(tx), 0)
            .await
    }

    #[cfg(feature = "email")]
    pub(super) async fn process_unseen_to<S: ImapStream>(
        &self,
        session: &mut Session<S>,
        delivery: &EmailDelivery<'_>,
        uid_validity: u32,
    ) -> Result<()> {
        let messages = self.fetch_unseen(session, uid_validity).await?;

        for email in messages {
            let uid = email._uid;
            let message_id = email.msg_id.clone();
            // Check allowlist
            if !self.is_sender_allowed(&email.sender) {
                warn!("Blocked email from {}", email.sender);
                self.mark_uid_seen(session, uid).await?;
                continue;
            }

            if self.seen_messages.lock().await.contains(&email.msg_id) {
                self.mark_uid_seen(session, uid).await?;
                continue;
            }

            let accepted = match delivery {
                #[cfg(test)]
                EmailDelivery::Legacy(tx) => self.dispatch_email(email, tx).await,
                EmailDelivery::Typed(sink) => {
                    self.dispatch_typed_email(&email, *sink).await?;
                    true
                }
            };
            if !accepted {
                // Channel closed, exit cleanly
                return Ok(());
            }

            self.seen_messages.lock().await.insert(message_id);
            self.mark_uid_seen(session, uid).await?;
        }

        Ok(())
    }

    #[cfg(feature = "email")]
    pub(super) async fn mark_uid_seen<S: ImapStream>(
        &self,
        session: &mut Session<S>,
        uid: u32,
    ) -> Result<()> {
        if uid == 0 {
            return Ok(());
        }
        session
            .run_command_and_check_ok(format!("UID STORE {uid} +FLAGS (\\Seen)"))
            .await?;
        Ok(())
    }
}
