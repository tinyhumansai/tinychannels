//! The **host boundary**: one generic capability surface every channel
//! provider can lean on, standardized across lean and rich providers alike.
//!
//! ## Why
//!
//! [`crate::traits::Channel`] covers the *transport* side of a provider —
//! receive inbound messages, send outbound ones. That is all a **lean**
//! provider (irc, signal, slack) needs. **Rich** providers (web, telegram,
//! presentation, whatsapp_web) additionally need to reach back into the host
//! runtime: run an agent turn, transcribe a voice note, park an approval,
//! recall conversation history, publish domain events, register shutdown, or
//! record telemetry.
//!
//! Historically those providers called OpenHuman internals directly, which is
//! exactly why they could not be ported into TinyChannels. [`ChannelHost`]
//! inverts that dependency: the **host** implements a set of small, portable
//! capability traits, and providers consume them through one aggregator that
//! is handed to them at construction via [`ProviderContext`].
//!
//! ## Shape
//!
//! - Each capability is an independent object-safe trait ([`TurnDispatcher`],
//!   [`Transcriber`], [`SpeechSynthesizer`], [`ApprovalGate`], [`ReactionGate`],
//!   [`ConversationStore`], [`EventSink`], [`LifecycleRegistry`], [`RunLedger`],
//!   plus the pre-existing [`crate::context::Memory`]).
//! - [`ChannelHost`] exposes each as an `Option<Arc<dyn …>>` accessor that
//!   defaults to `None`. A host implements only what it can back; a provider
//!   asks only for what it needs and degrades gracefully when a capability is
//!   absent.
//! - [`HostCapabilities`] is a cheap, copyable descriptor a provider can check
//!   up front (e.g. "only advertise voice replies if `host.capabilities().tts`").
//! - [`ChannelHostBuilder`] composes a concrete host from whatever capabilities
//!   you have; [`NoopHost`] is the empty host for lean providers and tests.
//!
//! A provider that uses **none** of this keeps working unchanged — it just
//! ignores the `host` on its [`ProviderContext`].

mod dispatch;
mod noop;
mod services;

pub use dispatch::{DispatchOptions, DispatchRequest, TurnDispatcher, TurnHandle};
pub use noop::{ChannelHostBuilder, NoopHost};
pub use services::{
    AllowlistStore, ApprovalAsk, ApprovalDecision, ApprovalGate, ConversationMessage,
    ConversationStore, EventSink, LifecycleRegistry, ReactionDecision, ReactionGate, ReactionQuery,
    RunEventAppend, RunLedger, RunTelemetry, RunUpsert, ShutdownHook, SpeechRequest, SpeechResult,
    SpeechSynthesizer, Transcriber, TranscriptionRequest, TranscriptionResult,
};

use crate::config::ChannelsConfig;
use crate::context::Memory;
use std::sync::Arc;

#[cfg(test)]
mod tests;

/// A cheap, copyable snapshot of which host capabilities are available.
///
/// Providers read this once (e.g. in their constructor) to branch behavior
/// without repeatedly probing `Option` accessors, and to advertise honest
/// [`crate::channel::ChannelStaticCapabilities`] downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HostCapabilities {
    pub turn_dispatch: bool,
    pub stt: bool,
    pub tts: bool,
    pub approvals: bool,
    pub reaction_gate: bool,
    pub conversation_store: bool,
    pub memory_recall: bool,
    pub event_sink: bool,
    pub lifecycle: bool,
    pub run_ledger: bool,
    pub allowlist_store: bool,
}

impl HostCapabilities {
    /// The empty capability set (a lean host).
    pub const NONE: Self = Self {
        turn_dispatch: false,
        stt: false,
        tts: false,
        approvals: false,
        reaction_gate: false,
        conversation_store: false,
        memory_recall: false,
        event_sink: false,
        lifecycle: false,
        run_ledger: false,
        allowlist_store: false,
    };

    /// Derive the descriptor from a live host by probing each accessor.
    pub fn probe(host: &dyn ChannelHost) -> Self {
        Self {
            turn_dispatch: host.dispatcher().is_some(),
            stt: host.transcriber().is_some(),
            tts: host.synthesizer().is_some(),
            approvals: host.approvals().is_some(),
            reaction_gate: host.reactions().is_some(),
            conversation_store: host.conversations().is_some(),
            memory_recall: host.memory().is_some(),
            event_sink: host.events().is_some(),
            lifecycle: host.lifecycle().is_some(),
            run_ledger: host.ledger().is_some(),
            allowlist_store: host.allowlist().is_some(),
        }
    }

    /// Whether any capability at all is present.
    pub fn is_lean(&self) -> bool {
        *self == Self::NONE
    }
}

/// The single, generic capability surface handed to every provider.
///
/// All accessors default to `None`, so implementing this trait is
/// incremental: a host overrides exactly the capabilities it can provide.
/// Provider code should treat every capability as optional and degrade
/// gracefully (a channel with no [`TurnDispatcher`] simply cannot run agent
/// turns; a channel with no [`Transcriber`] drops voice notes).
pub trait ChannelHost: Send + Sync {
    /// Core: run agent turns and stream output events.
    fn dispatcher(&self) -> Option<Arc<dyn TurnDispatcher>> {
        None
    }
    /// Speech-to-text for inbound voice.
    fn transcriber(&self) -> Option<Arc<dyn Transcriber>> {
        None
    }
    /// Text-to-speech for spoken replies.
    fn synthesizer(&self) -> Option<Arc<dyn SpeechSynthesizer>> {
        None
    }
    /// Human-in-the-loop approval gate.
    fn approvals(&self) -> Option<Arc<dyn ApprovalGate>> {
        None
    }
    /// Inference-driven "should I respond?" gate.
    fn reactions(&self) -> Option<Arc<dyn ReactionGate>> {
        None
    }
    /// Durable per-session conversation history.
    fn conversations(&self) -> Option<Arc<dyn ConversationStore>> {
        None
    }
    /// Semantic memory recall (vector store); see [`crate::context::Memory`].
    fn memory(&self) -> Option<Arc<dyn Memory>> {
        None
    }
    /// Domain event bus publish.
    fn events(&self) -> Option<Arc<dyn EventSink>> {
        None
    }
    /// Graceful-shutdown hook registration.
    fn lifecycle(&self) -> Option<Arc<dyn LifecycleRegistry>> {
        None
    }
    /// Run/telemetry ledger for observability.
    fn ledger(&self) -> Option<Arc<dyn RunLedger>> {
        None
    }
    /// Persisted allowlist store (promote paired/authorized identities).
    fn allowlist(&self) -> Option<Arc<dyn AllowlistStore>> {
        None
    }

    /// Snapshot of available capabilities. Defaults to probing the accessors;
    /// hosts may override with a cached descriptor.
    fn capabilities(&self) -> HostCapabilities {
        HostCapabilities {
            turn_dispatch: self.dispatcher().is_some(),
            stt: self.transcriber().is_some(),
            tts: self.synthesizer().is_some(),
            approvals: self.approvals().is_some(),
            reaction_gate: self.reactions().is_some(),
            conversation_store: self.conversations().is_some(),
            memory_recall: self.memory().is_some(),
            event_sink: self.events().is_some(),
            lifecycle: self.lifecycle().is_some(),
            run_ledger: self.ledger().is_some(),
            allowlist_store: self.allowlist().is_some(),
        }
    }
}

/// Everything a provider is constructed with. Lean providers use only
/// [`ProviderContext::http_client`] and [`ProviderContext::channels_config`];
/// rich providers additionally reach through [`ProviderContext::host`].
///
/// This is the standardized construction seam: `startup` builds one context
/// (wiring the real host) and every provider — lean or rich — is created from
/// it, so adding a capability never changes a provider's constructor arity.
#[derive(Clone)]
pub struct ProviderContext {
    /// The host capability surface. [`NoopHost`] for pure transports/tests.
    pub host: Arc<dyn ChannelHost>,
    /// The resolved channels configuration.
    pub channels_config: ChannelsConfig,
    /// A pre-built (optionally proxied) HTTP client for outbound REST calls.
    pub http_client: reqwest::Client,
}

impl ProviderContext {
    /// Construct a context. Pass [`NoopHost::arc`] for a host-less provider.
    pub fn new(
        host: Arc<dyn ChannelHost>,
        channels_config: ChannelsConfig,
        http_client: reqwest::Client,
    ) -> Self {
        Self {
            host,
            channels_config,
            http_client,
        }
    }

    /// Shorthand for the current host capability descriptor.
    pub fn capabilities(&self) -> HostCapabilities {
        self.host.capabilities()
    }
}
