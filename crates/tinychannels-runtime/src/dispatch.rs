//! Bounded-concurrency dispatch of inbound channel messages.

use std::future::Future;
use std::sync::Arc;

use tinychannels_bus::{ChannelInboundEnvelope, ChannelMessage};

use crate::log_worker_join_result;

/// An inbound message as the runtime dispatches it: the legacy
/// [`ChannelMessage`] plus, when it arrived through the relay transport, the
/// original [`ChannelInboundEnvelope`].
#[derive(Debug, Clone)]
pub struct RuntimeChannelMessage {
    pub message: ChannelMessage,
    pub inbound_envelope: Option<ChannelInboundEnvelope>,
}

impl RuntimeChannelMessage {
    /// A relay-delivered message that keeps its inbound envelope.
    pub fn with_inbound_envelope(
        message: ChannelMessage,
        inbound_envelope: ChannelInboundEnvelope,
    ) -> Self {
        Self {
            message,
            inbound_envelope: Some(inbound_envelope),
        }
    }
}

impl From<ChannelMessage> for RuntimeChannelMessage {
    fn from(message: ChannelMessage) -> Self {
        Self {
            message,
            inbound_envelope: None,
        }
    }
}

/// Feed messages from `rx` to `handler`, running at most
/// `max_in_flight_messages` handlers at once. Returns once `rx` closes and
/// every in-flight handler has finished. A panicking handler is logged and
/// does not stop the loop.
pub async fn run_dispatch_loop<M, F, Fut>(
    mut rx: tokio::sync::mpsc::Receiver<M>,
    max_in_flight_messages: usize,
    handler: F,
) where
    M: Send + 'static,
    F: Fn(M) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_in_flight_messages));
    let mut workers = tokio::task::JoinSet::new();

    while let Some(msg) = rx.recv().await {
        let permit = match Arc::clone(&semaphore).acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => break,
        };
        let work = handler(msg);
        workers.spawn(async move {
            let _permit = permit;
            work.await;
        });

        while let Some(result) = workers.try_join_next() {
            log_worker_join_result(result);
        }
    }

    while let Some(result) = workers.join_next().await {
        log_worker_join_result(result);
    }
}
