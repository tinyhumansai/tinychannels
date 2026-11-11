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
use futures::TryStreamExt;
#[cfg(feature = "email")]
#[cfg(any(test, debug_assertions))]
use mail_parser::MessageParser;
#[cfg(feature = "email")]
use rustls::{ClientConfig, RootCertStore};
#[cfg(feature = "email")]
use rustls_pki_types::DnsName;
#[cfg(feature = "email")]
use sha2::Digest;
#[cfg(feature = "email")]
use std::collections::HashSet;
#[cfg(feature = "email")]
use std::sync::Arc;
#[cfg(feature = "email")]
use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicU64, Ordering},
};
#[cfg(feature = "email")]
use std::time::Duration;
#[cfg(feature = "email")]
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(feature = "email")]
use tokio::io::{AsyncRead, AsyncWrite};
#[cfg(feature = "email")]
use tokio::net::TcpStream;
#[cfg(feature = "email")]
use tokio::sync::Mutex;
#[cfg(all(feature = "email", test))]
use tokio::sync::mpsc;
#[cfg(feature = "email")]
use tokio::sync::watch;
#[cfg(all(test, feature = "email"))]
use tokio::time::timeout;
#[cfg(feature = "email")]
#[cfg(feature = "email")]
use tokio_rustls::TlsConnector;
#[cfg(feature = "email")]
use tokio_rustls::client::TlsStream;
#[cfg(feature = "email")]
use tracing::{debug, error, warn};
#[cfg(feature = "email")]
#[cfg(feature = "email")]
mod capabilities;
mod config;
#[cfg(feature = "email")]
mod inbound;
#[cfg(feature = "email")]
mod native;
mod outbound;

#[cfg(feature = "email")]
use crate::channel::{ChannelInboundSink, ChannelSendError};
pub use crate::config::EmailConfig;
#[cfg(feature = "email")]
use crate::host::{EventSink, MediaRepository, Transcriber, VoiceMediaStore};
use crate::providers::mail::{ImapCredentials, MailSecret, SmtpCredentials, SmtpSecurity};
#[cfg(feature = "email")]
use crate::providers::native::HostPorts as NativeState;
#[cfg(all(feature = "email", test))]
use crate::traits::ChannelMessage;

#[cfg(feature = "email")]
type ImapSession = Session<TlsStream<TcpStream>>;

/// The bounds `async-imap` (runtime-tokio) needs of a connected stream, so the
/// IMAP exchange after the TLS handshake is testable over plain TCP.
#[cfg(feature = "email")]
trait ImapStream: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send {}
#[cfg(feature = "email")]
impl<S: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send> ImapStream for S {}

/// Budget for one [`EmailChannel::health_check_with`] IMAP login probe.
#[cfg(feature = "email")]
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Email channel — IMAP IDLE for instant push notifications, SMTP for outbound
pub struct EmailChannel {
    pub config: EmailConfig,
    /// Dedupe set for IMAP IDLE, which can re-report a message across
    /// re-establishes. A send-only build never opens a mailbox, so the field
    /// would be dead weight and a `never read` denial there.
    #[cfg(feature = "email")]
    seen_messages: Arc<Mutex<HashSet<String>>>,
    /// First delay before reconnecting a failed IMAP session; doubles up to
    /// [`Self::max_reconnect_backoff`].
    #[cfg(feature = "email")]
    reconnect_backoff: Duration,
    #[cfg(feature = "email")]
    max_reconnect_backoff: Duration,
    /// Host ports for the v2 adapter surface.
    #[cfg(feature = "email")]
    native: NativeState,
    /// Subjects of inbound mail by message id, so a reply keeps its thread subject.
    #[cfg(feature = "email")]
    subjects: native::Subjects,
    #[cfg(feature = "email")]
    listener_cancel: ListenerCancellationSlot,
    #[cfg(feature = "email")]
    listener_generation: Arc<AtomicU64>,
}

#[cfg(feature = "email")]
type ListenerCancellationSlot = Arc<StdMutex<Option<(u64, watch::Sender<bool>)>>>;

#[cfg(feature = "email")]
struct ListenerRegistration {
    slot: ListenerCancellationSlot,
    generation: u64,
}

#[cfg(feature = "email")]
impl Drop for ListenerRegistration {
    fn drop(&mut self) {
        let mut active = self.slot.lock().unwrap_or_else(|error| error.into_inner());
        if active
            .as_ref()
            .is_some_and(|(generation, _)| *generation == self.generation)
        {
            *active = None;
        }
    }
}

#[cfg(feature = "email")]
enum EmailDelivery<'a> {
    #[cfg(test)]
    Legacy(&'a mpsc::Sender<ChannelMessage>),
    Typed(&'a (dyn ChannelInboundSink + Send + Sync)),
}

#[cfg(feature = "email")]
#[derive(Debug, thiserror::Error)]
#[error("Email inbound sink rejected an event")]
struct InboundSinkRejected;

/// The IMAP server refused the credentials (`NO`/`BAD` to `LOGIN`).
///
/// Unlike a dropped connection this needs the user to fix their config, so the
/// reconnect loop keeps reporting it as an error.
#[cfg(feature = "email")]
#[derive(Debug, thiserror::Error)]
#[error("IMAP login failed: {0}")]
struct ImapLoginRejected(String);

/// Internal struct for parsed email data
#[cfg(feature = "email")]
struct ParsedEmail {
    _uid: u32,
    msg_id: String,
    sender: String,
    #[cfg(test)]
    content: String,
    timestamp: u64,
    /// The RFC 822 bytes, for the typed inbound path.
    raw: Vec<u8>,
}

/// Result from waiting on IDLE
#[cfg(feature = "email")]
#[derive(Debug, PartialEq, Eq)]
enum IdleWaitResult {
    NewMail,
    Timeout,
    Interrupted,
}

#[cfg(feature = "email")]
impl IdleWaitResult {
    fn from_response(response: &IdleResponse) -> Self {
        match response {
            IdleResponse::NewData(_) => Self::NewMail,
            IdleResponse::Timeout => Self::Timeout,
            IdleResponse::ManualInterrupt => Self::Interrupted,
        }
    }
}

#[cfg(feature = "email")]
crate::providers::native::native_v2_adapter!(EmailChannel);

#[cfg(all(test, feature = "email"))]
#[path = "fixtures_tests.rs"]
mod fixtures;

#[cfg(all(test, feature = "email"))]
#[path = "conformance_tests.rs"]
mod conformance;

#[cfg(all(test, feature = "email"))]
#[path = "email_channel_tests.rs"]
mod tests;

#[cfg(all(feature = "email", any(test, debug_assertions)))]
#[path = "test_support_tests.rs"]
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
