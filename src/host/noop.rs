//! An empty host and a composable builder.
//!
//! [`NoopHost`] advertises no capabilities — the right default for lean
//! providers and for standalone tests of transport behavior. [`ChannelHostBuilder`]
//! assembles a concrete host from whatever capability implementations you
//! have, so the OpenHuman side can wire real capabilities incrementally
//! (e.g. dispatcher + STT this week, ledger next) without a bespoke host type.

use super::{
    AllowlistStore, ApprovalGate, ChannelHost, ConversationStore, EventSink, LifecycleRegistry,
    Memory, ReactionGate, RunLedger, SpeechSynthesizer, Transcriber, TurnDispatcher,
};
use std::sync::Arc;

/// A host that provides nothing. Every accessor returns `None`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopHost;

impl NoopHost {
    /// A ready-to-share `Arc<dyn ChannelHost>` for host-less construction.
    pub fn arc() -> Arc<dyn ChannelHost> {
        Arc::new(NoopHost)
    }
}

impl ChannelHost for NoopHost {}

/// Assembles a [`ChannelHost`] from individually-provided capabilities.
///
/// ```ignore
/// let host = ChannelHostBuilder::new()
///     .dispatcher(my_dispatcher)
///     .transcriber(my_stt)
///     .build();
/// ```
#[derive(Default, Clone)]
pub struct ChannelHostBuilder {
    dispatcher: Option<Arc<dyn TurnDispatcher>>,
    transcriber: Option<Arc<dyn Transcriber>>,
    synthesizer: Option<Arc<dyn SpeechSynthesizer>>,
    approvals: Option<Arc<dyn ApprovalGate>>,
    reactions: Option<Arc<dyn ReactionGate>>,
    conversations: Option<Arc<dyn ConversationStore>>,
    memory: Option<Arc<dyn Memory>>,
    events: Option<Arc<dyn EventSink>>,
    lifecycle: Option<Arc<dyn LifecycleRegistry>>,
    ledger: Option<Arc<dyn RunLedger>>,
    allowlist: Option<Arc<dyn AllowlistStore>>,
}

impl ChannelHostBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn dispatcher(mut self, value: Arc<dyn TurnDispatcher>) -> Self {
        self.dispatcher = Some(value);
        self
    }
    pub fn transcriber(mut self, value: Arc<dyn Transcriber>) -> Self {
        self.transcriber = Some(value);
        self
    }
    pub fn synthesizer(mut self, value: Arc<dyn SpeechSynthesizer>) -> Self {
        self.synthesizer = Some(value);
        self
    }
    pub fn approvals(mut self, value: Arc<dyn ApprovalGate>) -> Self {
        self.approvals = Some(value);
        self
    }
    pub fn reactions(mut self, value: Arc<dyn ReactionGate>) -> Self {
        self.reactions = Some(value);
        self
    }
    pub fn conversations(mut self, value: Arc<dyn ConversationStore>) -> Self {
        self.conversations = Some(value);
        self
    }
    pub fn memory(mut self, value: Arc<dyn Memory>) -> Self {
        self.memory = Some(value);
        self
    }
    pub fn events(mut self, value: Arc<dyn EventSink>) -> Self {
        self.events = Some(value);
        self
    }
    pub fn lifecycle(mut self, value: Arc<dyn LifecycleRegistry>) -> Self {
        self.lifecycle = Some(value);
        self
    }
    pub fn ledger(mut self, value: Arc<dyn RunLedger>) -> Self {
        self.ledger = Some(value);
        self
    }
    pub fn allowlist(mut self, value: Arc<dyn AllowlistStore>) -> Self {
        self.allowlist = Some(value);
        self
    }

    /// Finalize into a shareable host.
    pub fn build(self) -> Arc<dyn ChannelHost> {
        Arc::new(CompositeHost {
            dispatcher: self.dispatcher,
            transcriber: self.transcriber,
            synthesizer: self.synthesizer,
            approvals: self.approvals,
            reactions: self.reactions,
            conversations: self.conversations,
            memory: self.memory,
            events: self.events,
            lifecycle: self.lifecycle,
            ledger: self.ledger,
            allowlist: self.allowlist,
        })
    }
}

/// The concrete host produced by [`ChannelHostBuilder::build`].
struct CompositeHost {
    dispatcher: Option<Arc<dyn TurnDispatcher>>,
    transcriber: Option<Arc<dyn Transcriber>>,
    synthesizer: Option<Arc<dyn SpeechSynthesizer>>,
    approvals: Option<Arc<dyn ApprovalGate>>,
    reactions: Option<Arc<dyn ReactionGate>>,
    conversations: Option<Arc<dyn ConversationStore>>,
    memory: Option<Arc<dyn Memory>>,
    events: Option<Arc<dyn EventSink>>,
    lifecycle: Option<Arc<dyn LifecycleRegistry>>,
    ledger: Option<Arc<dyn RunLedger>>,
    allowlist: Option<Arc<dyn AllowlistStore>>,
}

impl ChannelHost for CompositeHost {
    fn dispatcher(&self) -> Option<Arc<dyn TurnDispatcher>> {
        self.dispatcher.clone()
    }
    fn transcriber(&self) -> Option<Arc<dyn Transcriber>> {
        self.transcriber.clone()
    }
    fn synthesizer(&self) -> Option<Arc<dyn SpeechSynthesizer>> {
        self.synthesizer.clone()
    }
    fn approvals(&self) -> Option<Arc<dyn ApprovalGate>> {
        self.approvals.clone()
    }
    fn reactions(&self) -> Option<Arc<dyn ReactionGate>> {
        self.reactions.clone()
    }
    fn conversations(&self) -> Option<Arc<dyn ConversationStore>> {
        self.conversations.clone()
    }
    fn memory(&self) -> Option<Arc<dyn Memory>> {
        self.memory.clone()
    }
    fn events(&self) -> Option<Arc<dyn EventSink>> {
        self.events.clone()
    }
    fn lifecycle(&self) -> Option<Arc<dyn LifecycleRegistry>> {
        self.lifecycle.clone()
    }
    fn ledger(&self) -> Option<Arc<dyn RunLedger>> {
        self.ledger.clone()
    }
    fn allowlist(&self) -> Option<Arc<dyn AllowlistStore>> {
        self.allowlist.clone()
    }
}
