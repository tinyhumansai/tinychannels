//! Agent turn dispatch — the core runtime capability.
//!
//! This is what separates a *rich* provider (web, telegram, presentation)
//! from a *lean* one (irc, signal). A lean provider only needs to receive
//! inbound messages and send outbound text; it never touches the dispatcher.
//! A rich provider hands an assembled [`ChannelTurn`] to the host, then
//! streams the resulting [`ChannelOutputEvent`]s back to the user (partial
//! deltas, tool progress, approval asks, the final message, media, etc.).
//!
//! The host owns the actual agent harness; TinyChannels only defines the
//! portable request/response shapes so a provider never links the harness.

use crate::harness::types::{ChannelOutputEvent, ChannelTurn};
use async_trait::async_trait;
use std::sync::Arc;

/// Knobs that tune how a single turn is executed. All fields have sane
/// defaults so a caller can `DispatchOptions::default()` and only override
/// what a given channel supports.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchOptions {
    /// Stream partial text/tool-progress events as they are produced. When
    /// `false` the host only emits a single `FinalMessage`.
    pub stream_partial: bool,
    /// Allow the agent to invoke tools during this turn.
    pub allow_tools: bool,
    /// Force a specific model/route (`"provider:model"`); `None` uses the
    /// host's configured default for the channel role.
    pub model_override: Option<String>,
    /// BCP-47 locale hint for the reply (`"en"`, `"es"`); `None` = auto.
    pub locale: Option<String>,
    /// Upper bound on wall-clock time for the turn; `None` = host default.
    pub timeout_secs: Option<u64>,
}

impl Default for DispatchOptions {
    fn default() -> Self {
        Self {
            stream_partial: true,
            allow_tools: true,
            model_override: None,
            locale: None,
            timeout_secs: None,
        }
    }
}

/// A dispatch request: the assembled turn plus its execution options.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchRequest {
    pub turn: ChannelTurn,
    pub options: DispatchOptions,
}

impl DispatchRequest {
    /// Build a request with default options from an assembled turn.
    pub fn new(turn: ChannelTurn) -> Self {
        Self {
            turn,
            options: DispatchOptions::default(),
        }
    }

    /// Override the execution options.
    pub fn with_options(mut self, options: DispatchOptions) -> Self {
        self.options = options;
        self
    }
}

/// A live, cancellable agent run. Poll [`TurnHandle::events`] for streamed
/// output; call [`TurnHandle::cancel`] to abort mid-flight.
pub struct TurnHandle {
    /// Host-assigned run identifier (used for telemetry / ledger correlation).
    pub run_id: String,
    /// Ordered stream of output events. Closes when the turn completes.
    pub events: tokio::sync::mpsc::Receiver<ChannelOutputEvent>,
    cancel: Arc<dyn Fn() + Send + Sync>,
}

impl TurnHandle {
    /// Construct a handle. Hosts build this after spawning the run.
    pub fn new(
        run_id: impl Into<String>,
        events: tokio::sync::mpsc::Receiver<ChannelOutputEvent>,
        cancel: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            events,
            cancel,
        }
    }

    /// Request cancellation of the in-flight run. Idempotent; the event
    /// stream should then emit a `Cancellation` and close.
    pub fn cancel(&self) {
        (self.cancel)();
    }

    /// Await the next output event, or `None` when the run is complete.
    pub async fn next(&mut self) -> Option<ChannelOutputEvent> {
        self.events.recv().await
    }

    /// Drain the stream to completion, returning every event in order.
    /// Convenience for non-streaming callers and tests.
    pub async fn collect(mut self) -> Vec<ChannelOutputEvent> {
        let mut out = Vec::new();
        while let Some(event) = self.events.recv().await {
            out.push(event);
        }
        out
    }
}

/// Runs a [`ChannelTurn`] through the agent harness and streams results.
///
/// Needed by: **web, telegram, presentation**. Lean providers never call it.
#[async_trait]
pub trait TurnDispatcher: Send + Sync {
    /// Dispatch a turn. The returned handle streams [`ChannelOutputEvent`]s
    /// until the run finishes. Errors are for *admission* failures (bad turn,
    /// runtime unavailable); once a handle is returned, per-turn failures are
    /// surfaced as `Cancellation`/`Native` events on the stream.
    async fn dispatch(&self, request: DispatchRequest) -> anyhow::Result<TurnHandle>;

    /// Cancel any in-flight run(s) for a session key (e.g. user typed "stop").
    /// Default no-op for hosts without cooperative cancellation.
    async fn cancel_session(&self, _session_key: &str) -> anyhow::Result<()> {
        Ok(())
    }
}
