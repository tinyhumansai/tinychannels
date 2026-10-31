//! Owned provider listener, lifecycle hooks and ordered inbound forwarding.
use crate::host::HostChannels;
use crate::voice_reply::{ModuleSynthesizer, ModuleVoiceCodec, SpokenReply, inbound_voice};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tinybus::{Error, Result};
use tinychannels::channel::{ChannelAdapter, ChannelInboundSink};
#[cfg(any(test, feature = "test-support"))]
use tinychannels::channel::{ChannelSendError, ChannelSendErrorExt};
use tinychannels::factory::BuiltChannel;
use tinychannels::host::{LifecycleRegistry, ShutdownHook};
use tinychannels::{Channel, ChannelHostBuilder, SendMessage};
use tinychannels_bus::{ChannelSupervisionPolicy, HostEventPublication, HostTranscriptionRequest};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

struct AbortOnDrop<T>(JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

enum ProviderOperation {
    Send(SendMessage),
    SendTyped(tinychannels_bus::ChannelOutboundMessage),
    Interaction(Box<tinychannels_bus::ChannelInteraction>),
}

struct TypingRefreshTask {
    recipient: String,
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
}

type TypingRefreshes = HashMap<String, TypingRefreshTask>;

const MAX_TYPING_REFRESH_RECIPIENTS: usize = 64;
const MAX_TYPING_REFRESH_INTERVAL_SECS: u64 = 60;

async fn dispatch_interaction(
    channel: Arc<dyn Channel>,
    interaction: tinychannels_bus::ChannelInteraction,
    typing: &mut TypingRefreshes,
) -> Result<tinychannels_bus::ChannelInteractionResult> {
    use tinychannels_bus::ChannelInteraction as Op;
    match interaction {
        Op::StartTypingRefresh {
            recipient,
            interval_secs,
        } => {
            let refresh_channel = Arc::clone(&channel);
            let name = channel.name().to_string();
            start_typing_refresh(typing, recipient, interval_secs, name, move |recipient| {
                let channel = Arc::clone(&refresh_channel);
                Box::pin(async move {
                    channel
                        .start_typing(&recipient)
                        .await
                        .map_err(|error| error.to_string())
                })
            })
        }
        Op::StopTypingRefresh { handle } => {
            stop_typing_refresh(&handle, typing).await;
            Ok(tinychannels_bus::ChannelInteractionResult::Completed)
        }
        interaction => crate::interaction::dispatch(channel.as_ref(), interaction).await,
    }
}

type TypingStartFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<(), String>> + Send>>;

/// Spawn the module-owned refresh task for one recipient. `start` re-issues the
/// provider typing action; the task is cancelled and joined by
/// `StopTypingRefresh`.
fn start_typing_refresh(
    typing: &mut TypingRefreshes,
    recipient: String,
    interval_secs: u64,
    name: String,
    start: impl Fn(String) -> TypingStartFuture + Send + 'static,
) -> Result<tinychannels_bus::ChannelInteractionResult> {
    if interval_secs == 0 || interval_secs > MAX_TYPING_REFRESH_INTERVAL_SECS {
        return Err(Error::failed(
            "typing refresh interval is outside its limit",
        ));
    }
    if typing.len() >= MAX_TYPING_REFRESH_RECIPIENTS {
        return Err(Error::failed("typing refresh capacity is full"));
    }
    let (cancel, mut canceled) = watch::channel(false);
    let refresh_recipient = recipient.clone();
    let interval = Duration::from_secs(interval_secs);
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = async { let _ = canceled.wait_for(|value| *value).await; } => break,
                _ = tokio::time::sleep(interval) => {
                    let refresh = start(refresh_recipient.clone());
                    tokio::select! {
                        biased;
                        _ = async { let _ = canceled.wait_for(|value| *value).await; } => break,
                        result = refresh => if let Err(error) = result {
                            tracing::debug!(channel = %name, "typing refresh failed: {error}");
                        }
                    }
                }
            }
        }
    });
    let handle = loop {
        let candidate = uuid::Uuid::new_v4().simple().to_string();
        if !typing.contains_key(&candidate) {
            break candidate;
        }
    };
    typing.insert(
        handle.clone(),
        TypingRefreshTask {
            recipient,
            cancel,
            task,
        },
    );
    Ok(tinychannels_bus::ChannelInteractionResult::TypingRefresh { handle })
}

/// Typing interactions for a factory-built adapter, through its typing port.
/// `None` when the interaction is not a typing operation.
async fn dispatch_adapter_typing(
    port: &Arc<dyn tinychannels::channel::ChannelTyping>,
    name: &str,
    interaction: &tinychannels_bus::ChannelInteraction,
    typing: &mut TypingRefreshes,
) -> Option<Result<tinychannels_bus::ChannelInteractionResult>> {
    use tinychannels_bus::ChannelInteraction as Op;
    match interaction {
        Op::StartTyping { recipient } => Some(
            port.start_typing(recipient)
                .await
                .map(|()| tinychannels_bus::ChannelInteractionResult::Completed)
                .map_err(|_| Error::failed("provider typing start failed")),
        ),
        Op::StopTyping { recipient } => Some(
            port.stop_typing(recipient)
                .await
                .map(|()| tinychannels_bus::ChannelInteractionResult::Completed)
                .map_err(|_| Error::failed("provider typing stop failed")),
        ),
        Op::StartTypingRefresh {
            recipient,
            interval_secs,
        } => {
            let port = Arc::clone(port);
            Some(start_typing_refresh(
                typing,
                recipient.clone(),
                *interval_secs,
                name.to_owned(),
                move |recipient| {
                    let port = Arc::clone(&port);
                    Box::pin(async move {
                        port.start_typing(&recipient)
                            .await
                            .map_err(|error| error.message)
                    })
                },
            ))
        }
        Op::StopTypingRefresh { handle } => {
            stop_typing_refresh(handle, typing).await;
            Some(Ok(tinychannels_bus::ChannelInteractionResult::Completed))
        }
        _ => None,
    }
}

async fn dispatch_draft_upsert(
    drafts: &dyn tinychannels::channel::ChannelStreamingDraft,
    configured_instance: &str,
    previous: Option<tinychannels_bus::channel::MessageReceipt>,
    intent: tinychannels_bus::ChannelOutboundIntent,
) -> Result<tinychannels_bus::ChannelInteractionResult> {
    let channel = intent
        .channel_ref
        .clone()
        .unwrap_or_else(|| tinychannels_bus::ChannelRef {
            id: tinychannels_bus::provider_id(configured_instance).to_owned(),
            account_id: Some(configured_instance.to_owned()),
        });
    let conversation =
        intent
            .conversation_ref
            .clone()
            .unwrap_or_else(|| tinychannels_bus::ConversationRef {
                id: intent.conversation_id.clone(),
                thread_id: intent.thread_id.clone(),
                ..Default::default()
            });
    let thread_id = intent
        .thread_id
        .clone()
        .or_else(|| conversation.thread_id.clone());
    let idempotency_key = intent.idempotency_key.clone();
    let mut receipt = drafts
        .upsert_draft(previous.as_ref(), intent)
        .await
        .map_err(|error| Error::failed(error.message))?;
    receipt.idempotency_key = Some(idempotency_key);
    receipt.channel = Some(channel);
    if receipt.conversation.is_none() {
        receipt.conversation = Some(conversation);
    }
    if receipt.thread_id.is_none() {
        receipt.thread_id = thread_id;
    }
    Ok(tinychannels_bus::ChannelInteractionResult::Sent(Box::new(
        tinychannels_bus::channel::ChannelSendOutcome {
            receipt,
            error: None,
        },
    )))
}

async fn stop_typing_refresh(handle: &str, typing: &mut TypingRefreshes) {
    if let Some(refresh) = typing.remove(handle) {
        refresh.cancel.send_replace(true);
        if let Err(error) = refresh.task.await {
            tracing::debug!(%handle, recipient = %refresh.recipient, "typing refresh task join failed: {error}");
        }
    }
}

async fn stop_all_typing_refreshes(typing: &mut TypingRefreshes) {
    let handles = typing.keys().cloned().collect::<Vec<_>>();
    for handle in handles {
        stop_typing_refresh(&handle, typing).await;
    }
}

struct SendCommand {
    operation: ProviderOperation,
    reply: oneshot::Sender<Result<tinychannels_bus::ChannelInteractionResult>>,
    started: oneshot::Sender<()>,
}

#[derive(Debug)]
pub(crate) enum RunningSendError {
    ValidationFailed(String),
    NotDispatched(String),
    AfterDispatch(String),
}

impl std::fmt::Display for RunningSendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ValidationFailed(message)
            | Self::NotDispatched(message)
            | Self::AfterDispatch(message) => formatter.write_str(message),
        }
    }
}

/// Native hooks registered while the selected provider starts.
#[derive(Default)]
pub(crate) struct Hooks {
    hooks: StdMutex<Vec<ShutdownHook>>,
    configured_instance: StdMutex<Option<String>>,
    media_account_scope: StdMutex<Option<String>>,
    spoken: StdMutex<Option<Arc<SpokenReply>>>,
}
impl LifecycleRegistry for Hooks {
    fn register_shutdown(&self, _name: &str, hook: ShutdownHook) {
        self.hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(hook);
    }
}
impl Hooks {
    /// The outbound voice pipeline bound to the host this provider started with.
    pub(crate) fn spoken(&self) -> Option<Arc<SpokenReply>> {
        self.spoken
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub(crate) fn host(
        self: &Arc<Self>,
        callbacks: HostChannels,
        configured_instance: String,
        media_account_scope: Option<String>,
    ) -> Arc<dyn tinychannels::host::ChannelHost> {
        *self
            .configured_instance
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(configured_instance.clone());
        *self
            .media_account_scope
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = media_account_scope.clone();
        let voice_store = ModuleVoiceMediaStore(ModuleMediaRepository(
            callbacks.clone(),
            configured_instance.clone(),
            media_account_scope.clone(),
        ));
        let synthesizer = Arc::new(ModuleSynthesizer(
            callbacks.clone(),
            configured_instance.clone(),
        ));
        let codec = Arc::new(ModuleVoiceCodec(callbacks.clone()));
        let voice_store = Arc::new(voice_store);
        *self.spoken.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(SpokenReply {
            instance: configured_instance.clone(),
            policy: Arc::new(callbacks.clone()),
            synthesizer: synthesizer.clone(),
            codec: codec.clone(),
            store: voice_store.clone(),
        }));
        ChannelHostBuilder::new()
            .synthesizer(synthesizer)
            .voice_codec(codec)
            .lifecycle(self.clone())
            .transcriber(Arc::new(ModuleTranscriber(
                callbacks.clone(),
                Some(configured_instance.clone()),
            )))
            .media_repository(Arc::new(ModuleMediaRepository(
                callbacks.clone(),
                configured_instance.clone(),
                media_account_scope.clone(),
            )))
            .voice_media_store(voice_store)
            .events(Arc::new(ModuleEvents(
                callbacks.clone(),
                configured_instance,
            )))
            .storage(Arc::new(ModuleStorage(callbacks.clone())))
            .allowlist(Arc::new(ModuleAllowlist(callbacks)))
            .build()
    }
    async fn drain(&self) {
        let hooks = std::mem::take(&mut *self.hooks.lock().unwrap_or_else(|e| e.into_inner()));
        for hook in hooks {
            hook().await;
        }
    }
}

/// Persists inbound/outbound voice through the module's own scoped media
/// repository; the host holds no voice store.
const MAX_VOICE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct ModuleVoiceMediaStore(pub(crate) ModuleMediaRepository);

#[async_trait::async_trait]
impl tinychannels::host::VoiceMediaStore for ModuleVoiceMediaStore {
    async fn store_voice(
        &self,
        request: tinychannels::host::VoiceMediaRequest,
    ) -> anyhow::Result<tinychannels_bus::HostStoreVoiceMediaResult> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(request.audio_base64.as_bytes())
            .map_err(|_| anyhow::anyhow!("voice media payload is invalid"))?;
        if bytes.is_empty() || bytes.len() > MAX_VOICE_BYTES {
            anyhow::bail!("voice media exceeds the configured size limit");
        }
        let media = <ModuleMediaRepository as tinychannels::host::MediaRepository>::store_media(
            &self.0,
            tinychannels::host::MediaStoreRequest {
                metadata: tinychannels::host::MediaMetadata {
                    scope: request.scope,
                    kind: tinychannels_bus::MediaKind::Audio,
                    content_type: request.mime_type,
                    file_name: request.file_name,
                    duration_ms: request.duration_ms,
                    codec: request.codec,
                    voice_direction: Some(request.direction),
                    dedupe_key: None,
                    max_bytes: MAX_VOICE_BYTES as u64,
                },
                bytes,
            },
        )
        .await?;
        Ok(tinychannels_bus::HostStoreVoiceMediaResult {
            media,
            accessible_url: None,
        })
    }

    async fn read_voice(
        &self,
        scope: tinychannels_bus::HostVoiceScope,
        opaque_id: String,
    ) -> anyhow::Result<tinychannels_bus::HostReadVoiceMediaResult> {
        use base64::Engine;
        let stored = <ModuleMediaRepository as tinychannels::host::MediaRepository>::read_media(
            &self.0,
            scope,
            opaque_id,
            MAX_VOICE_BYTES as u64,
        )
        .await?;
        Ok(tinychannels_bus::HostReadVoiceMediaResult {
            audio_base64: base64::engine::general_purpose::STANDARD.encode(&stored.bytes),
            mime_type: stored.content_type,
            file_name: stored.file_name,
        })
    }
}

#[derive(Clone)]
pub(crate) struct ModuleMediaRepository(
    pub(crate) HostChannels,
    pub(crate) String,
    pub(crate) Option<String>,
);

impl ModuleMediaRepository {
    fn bind_scope(&self, scope: &mut tinychannels::host::MediaScope) -> anyhow::Result<()> {
        if scope.conversation.id.trim().is_empty() || scope.turn_id.trim().is_empty() {
            anyhow::bail!("media scope requires a conversation and originating turn");
        }
        let account = self
            .2
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("configured account has no stable media identity"))?;
        scope.configured_instance = self.1.clone();
        scope.channel.id = tinychannels_bus::provider_id(&self.1).to_owned();
        scope.channel.account_id = Some(account.to_owned());
        Ok(())
    }

    async fn scoped_store(&self) -> anyhow::Result<tinychannels::host::ScopedMediaStore> {
        let storage = self.0.storage().await.map_err(|error| {
            anyhow::anyhow!("module scoped storage is unavailable: {:?}", error.kind())
        })?;
        Ok(tinychannels::host::ScopedMediaStore::new(
            Arc::clone(storage.documents()),
            Arc::clone(storage.blobs()),
        ))
    }
}

#[async_trait::async_trait]
impl crate::media::MediaPersistence for ModuleMediaRepository {
    async fn store_public_media(
        &self,
        request: tinychannels::host::MediaStoreRequest,
    ) -> tinybus::Result<tinychannels_bus::MediaReference> {
        <Self as tinychannels::host::MediaRepository>::store_media(self, request)
            .await
            .map_err(|_| Error::failed("media could not be stored"))
    }
}

#[async_trait::async_trait]
impl tinychannels::host::MediaRepository for ModuleMediaRepository {
    async fn store_media(
        &self,
        mut request: tinychannels::host::MediaStoreRequest,
    ) -> anyhow::Result<tinychannels_bus::MediaReference> {
        self.bind_scope(&mut request.metadata.scope)?;
        self.scoped_store().await?.store(request).await
    }

    async fn store_media_stream(
        &self,
        mut request: tinychannels::host::MediaStreamRequest,
    ) -> anyhow::Result<tinychannels_bus::MediaReference> {
        self.bind_scope(&mut request.metadata.scope)?;
        self.scoped_store().await?.store_stream(request).await
    }

    async fn read_media(
        &self,
        mut scope: tinychannels::host::MediaScope,
        opaque_id: String,
        max_bytes: u64,
    ) -> anyhow::Result<tinychannels::host::StoredMedia> {
        if scope.configured_instance != self.1 {
            anyhow::bail!("media scope does not match the configured provider instance");
        }
        self.bind_scope(&mut scope)?;
        self.scoped_store()
            .await?
            .read(scope, &opaque_id, max_bytes)
            .await
    }

    async fn delete_media(
        &self,
        mut scope: tinychannels::host::MediaScope,
        opaque_id: String,
    ) -> anyhow::Result<bool> {
        self.bind_scope(&mut scope)?;
        self.scoped_store().await?.delete(scope, &opaque_id).await
    }
}

struct ModuleStorage(HostChannels);

#[async_trait::async_trait]
impl tinychannels::host::StorageProvider for ModuleStorage {
    async fn documents(&self) -> anyhow::Result<Arc<dyn tinystoragedrivers_api::DocumentStore>> {
        self.0
            .documents()
            .await
            .map_err(|error| anyhow::anyhow!("host storage: {:?}", error.kind()))
    }
}

struct ModuleAllowlist(HostChannels);

#[async_trait::async_trait]
impl tinychannels::host::AllowlistStore for ModuleAllowlist {
    async fn persist_allowed_identity(&self, channel: &str, identity: &str) -> anyhow::Result<()> {
        self.0
            .persist_allowed_identity(channel, identity)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }
}

/// Host STT bridge. With a configured instance the module is the authenticated
/// source of the scope's provider identity, as for media, so a provider that
/// knows only conversation and turn still produces a scope the host accepts.
struct ModuleTranscriber(HostChannels, Option<String>);

#[async_trait::async_trait]
impl tinychannels::host::Transcriber for ModuleTranscriber {
    fn name(&self) -> &str {
        "module-host"
    }

    async fn transcribe(
        &self,
        request: tinychannels::host::TranscriptionRequest,
    ) -> anyhow::Result<tinychannels::host::TranscriptionResult> {
        self.transcribe_with_scope(None, request).await
    }

    async fn transcribe_scoped(
        &self,
        scope: tinychannels_bus::HostVoiceScope,
        request: tinychannels::host::TranscriptionRequest,
    ) -> anyhow::Result<tinychannels::host::TranscriptionResult> {
        self.transcribe_with_scope(Some(scope), request).await
    }
}

impl ModuleTranscriber {
    async fn transcribe_with_scope(
        &self,
        mut scope: Option<tinychannels_bus::HostVoiceScope>,
        request: tinychannels::host::TranscriptionRequest,
    ) -> anyhow::Result<tinychannels::host::TranscriptionResult> {
        if let (Some(scope), Some(instance)) = (scope.as_mut(), self.1.as_deref()) {
            scope.configured_instance = instance.to_owned();
            scope.channel.id = tinychannels_bus::provider_id(instance).to_owned();
            scope.channel.account_id = Some(instance.to_owned());
        }
        let result = self
            .0
            .transcribe(HostTranscriptionRequest {
                scope,
                audio_base64: request.audio_base64,
                mime_type: request.mime_type,
                file_name: request.file_name,
                language: request.language,
            })
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        Ok(tinychannels::host::TranscriptionResult {
            text: result.text,
            language: result.language,
            duration_secs: result.duration_secs,
        })
    }
}

pub(crate) struct ModuleEvents(pub(crate) HostChannels, pub(crate) String);

fn scope_channel_event(
    account_id: &str,
    mut event: tinychannels_bus::ChannelEvent,
) -> tinychannels_bus::ChannelEvent {
    // The module start call is the authenticated source of instance scope;
    // provider payloads cannot choose or spoof their account identity.
    event.channel.id = tinychannels_bus::provider_id(account_id).to_owned();
    event.channel.account_id = Some(account_id.to_owned());
    let kind = match &event.event {
        tinychannels_bus::ChannelEventKind::Typing { .. } => "typing",
        tinychannels_bus::ChannelEventKind::ReadReceipt { .. } => "read_receipt",
        tinychannels_bus::ChannelEventKind::DeliveredReceipt { .. } => "delivered_receipt",
        tinychannels_bus::ChannelEventKind::ReactionAdded { .. } => "reaction_added",
        tinychannels_bus::ChannelEventKind::ReactionRemoved { .. } => "reaction_removed",
        tinychannels_bus::ChannelEventKind::OutboundReaction { .. } => "outbound_reaction",
        tinychannels_bus::ChannelEventKind::MessageUpdated { .. } => "message_updated",
        tinychannels_bus::ChannelEventKind::MessageDeleted { .. } => "message_deleted",
        tinychannels_bus::ChannelEventKind::Presence { .. } => "presence",
        tinychannels_bus::ChannelEventKind::Lifecycle { .. } => "lifecycle",
        tinychannels_bus::ChannelEventKind::ButtonCallback { .. } => "button_callback",
    };
    event.event_id = format!("{account_id}:{}:{kind}", event.event_id);
    event
}

#[async_trait::async_trait]
impl tinychannels::host::EventSink for ModuleEvents {
    async fn publish(
        &self,
        domain: &str,
        kind: &str,
        payload: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.0
            .publish_event(HostEventPublication {
                domain: domain.to_owned(),
                kind: kind.to_owned(),
                payload,
            })
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    async fn publish_channel_event(
        &self,
        event: tinychannels_bus::ChannelEvent,
    ) -> anyhow::Result<()> {
        let event = scope_channel_event(&self.1, event);
        self.0.deliver_event(event).await.map_err(|error| {
            tracing::warn!(%error, "channel host ingress callback failed");
            anyhow::anyhow!(error.to_string())
        })
    }

    async fn publish_ingress(
        &self,
        ingress: tinychannels_bus::ChannelIngress,
    ) -> anyhow::Result<bool> {
        match ingress {
            tinychannels_bus::ChannelIngress::Inbound(mut envelope) => {
                envelope.channel.id = tinychannels_bus::provider_id(&self.1).to_owned();
                envelope.channel.account_id = Some(self.1.clone());
                self.0
                    .try_deliver_inbound(&self.1, envelope)
                    .await
                    .map_err(|error| anyhow::anyhow!(error.message))?;
                Ok(true)
            }
            tinychannels_bus::ChannelIngress::Event(event) => {
                self.publish_channel_event(event)
                    .await
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                Ok(true)
            }
        }
    }

    async fn publish_inbound(
        &self,
        message: tinychannels_bus::ChannelMessage,
    ) -> anyhow::Result<bool> {
        let mut envelope = tinychannels::inbound_envelope_from_legacy_message(&message);
        if tinychannels_bus::provider_id(&self.1) == "telegram" {
            if let Some((chat, topic)) = message.reply_target.split_once(':') {
                envelope.conversation.id = chat.to_owned();
                envelope.conversation.topic_id = Some(topic.to_owned());
                envelope.conversation.thread_id = None;
            }
            let raw_message_id = message.id.strip_prefix("telegram_").and_then(|id| {
                id.rsplit_once('_')
                    .map(|(_, message_id)| message_id.to_owned())
            });
            envelope.raw = Some(serde_json::json!({"providerMessageId": raw_message_id}));
        }
        self.publish_ingress(tinychannels_bus::ChannelIngress::Inbound(envelope))
            .await
    }
}

/// Bridge the runtime's unified provider ingress into the existing ordered
/// TinyBus host callback path. Envelope-only adapters continue to work through
/// `push`; adapters that emit lifecycle or message events opt into
/// `push_event` and preserve ordering with inbound messages.
#[async_trait::async_trait]
impl tinychannels::channel::ChannelInboundSink for ModuleEvents {
    async fn push(
        &self,
        mut envelope: tinychannels_bus::ChannelInboundEnvelope,
    ) -> std::result::Result<(), tinychannels::channel::ChannelSendError> {
        envelope.channel.id = tinychannels_bus::provider_id(&self.1).to_owned();
        envelope.channel.account_id = Some(self.1.clone());
        inbound_voice::record(
            &envelope.conversation.id,
            envelope
                .parts
                .iter()
                .any(|part| matches!(part, tinychannels_bus::ContentPart::VoiceNote { .. })),
        );
        self.0
            .try_deliver_inbound(&self.1, envelope)
            .await
            .inspect_err(|error| {
                tracing::warn!(message = %error.message, "channel host ingress callback failed");
            })
    }

    async fn push_event(
        &self,
        event: tinychannels_bus::ChannelEvent,
    ) -> std::result::Result<(), tinychannels::channel::ChannelSendError> {
        self.0
            .try_deliver_event(scope_channel_event(&self.1, event))
            .await
            .inspect_err(|error| {
                tracing::warn!(message = %error.message, "channel host ingress callback failed");
            })
    }
}

/// Reconnect execution stays in the module; raw provider errors never cross callbacks.
async fn listen_with_supervision(
    adapter: Arc<dyn ChannelAdapter>,
    legacy: Option<Arc<dyn Channel>>,
    sink: Arc<dyn ChannelInboundSink>,
    host: HostChannels,
    policy: Option<ChannelSupervisionPolicy>,
) -> anyhow::Result<()> {
    let Some(policy) = policy else {
        return start_adapter_listener(&adapter, legacy.as_ref(), sink.clone()).await;
    };
    let name = adapter.descriptor().id;
    let mut backoff = policy.initial_backoff_secs.max(1);
    let maximum = policy.max_backoff_secs.max(backoff);
    loop {
        if let Err(error) = start_adapter_listener(&adapter, legacy.as_ref(), sink.clone()).await {
            tracing::warn!(provider = %name, %error, "provider listener stopped; restarting");
        }
        host.report_status(&name, "restarting", Some("provider disconnected".into()))
            .await;
        tokio::time::sleep(
            Duration::from_secs(backoff)
                + Duration::from_millis(tinychannels::runtime::jitter_millis(backoff)),
        )
        .await;
        backoff = backoff.saturating_mul(2).min(maximum);
        host.report_status(&name, "connecting", None).await;
    }
}

async fn start_adapter_listener(
    adapter: &Arc<dyn ChannelAdapter>,
    legacy: Option<&Arc<dyn Channel>>,
    sink: Arc<dyn ChannelInboundSink>,
) -> anyhow::Result<()> {
    if let Some(channel) = legacy {
        let (tx, mut rx) = mpsc::channel(256);
        let sink = sink.clone();
        let forward = tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                let envelope = tinychannels::inbound_envelope_from_legacy_message(&message);
                if let Err(error) = sink
                    .push_ingress(tinychannels_bus::ChannelIngress::Inbound(envelope))
                    .await
                {
                    tracing::warn!(message = %error.message, "legacy channel ingress callback failed");
                }
            }
        });
        let result = channel.listen(tx).await;
        forward.abort();
        let _ = forward.await;
        result
    } else {
        adapter
            .start(sink.as_ref())
            .await
            .map_err(|error| anyhow::anyhow!(error.message))
    }
}

/// V21-only wrapper used by compatibility tests and old module call sites.
/// Production factory results enter `Running` as their native v2 adapter.
#[cfg(any(test, feature = "test-support"))]
struct LegacyChannelAdapter(Arc<dyn Channel>);

#[cfg(any(test, feature = "test-support"))]
#[async_trait::async_trait]
impl ChannelAdapter for LegacyChannelAdapter {
    fn descriptor(&self) -> tinychannels::channel::ChannelDescriptor {
        let id = self.0.name().to_owned();
        tinychannels::channel::ChannelDescriptor {
            display_name: id.clone(),
            id,
            ..Default::default()
        }
    }

    fn proactive_target(&self) -> Option<String> {
        self.0.proactive_target()
    }

    fn declared_capabilities(&self) -> tinychannels_bus::ChannelCapabilities {
        // Preserve the v21 provider's explicit capability projection at this
        // compatibility boundary; the rest of the module consumes v2 facts.
        self.0.capabilities()
    }

    async fn start(
        &self,
        _sink: &(dyn ChannelInboundSink + Send + Sync),
    ) -> std::result::Result<(), ChannelSendError> {
        Err(ChannelSendError::unsupported(
            "legacy listeners require the v21 compatibility bridge",
        ))
    }

    async fn stop(&self) -> std::result::Result<(), ChannelSendError> {
        Ok(())
    }

    async fn send(
        &self,
        intent: tinychannels::channel::ChannelOutboundIntent,
    ) -> std::result::Result<tinychannels::channel::MessageReceipt, ChannelSendError> {
        let value = tinychannels::channel::legacy_message_value_from_outbound_intent(&intent);
        let text = value
            .get("content")
            .or_else(|| value.get("text"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ChannelSendError::unsupported("v21 channel accepts text sends only"))?;
        let message = SendMessage {
            content: text.to_owned(),
            recipient: value
                .get("recipient")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&intent.conversation_id)
                .to_owned(),
            subject: value
                .get("subject")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            thread_ts: intent.thread_id.clone(),
            topic_id: intent
                .conversation_ref
                .as_ref()
                .and_then(|conversation| conversation.topic_id.clone()),
            reply_to_id: intent.reply_to_id.clone(),
            idempotency_key: Some(intent.idempotency_key),
        };
        self.0
            .send(&message)
            .await
            .map_err(|error| ChannelSendError::new(error.to_string()))?;
        Ok(tinychannels::channel::MessageReceipt::default())
    }

    async fn send_typed(
        &self,
        message: &tinychannels_bus::ChannelOutboundMessage,
    ) -> std::result::Result<tinychannels_bus::channel::ChannelSendOutcome, ChannelSendError> {
        self.0
            .validate_typed_send(message)
            .map_err(|error| ChannelSendError::unsupported(error.to_string()))?;
        self.0
            .send_typed_with_receipt(message)
            .await
            .map_err(|error| {
                let mut error = ChannelSendError::new(error.to_string());
                // The v21 error has no delivery-state field. Once its send
                // method returns an error, the compatibility bridge cannot
                // prove that the provider did not accept the message, so
                // durable delivery must reconcile instead of replaying it.
                error.sent_before_error = true;
                error
            })
    }

    fn validate_typed_send(
        &self,
        message: &tinychannels_bus::ChannelOutboundMessage,
    ) -> std::result::Result<(), ChannelSendError> {
        self.0
            .validate_typed_send(message)
            .map_err(|error| ChannelSendError::unsupported(error.to_string()))
    }

    async fn status(
        &self,
    ) -> std::result::Result<tinychannels::controllers::ChannelAccountSnapshot, ChannelSendError>
    {
        Ok(tinychannels::controllers::ChannelAccountSnapshot {
            channel_id: self.0.name().to_owned(),
            health_state: Some(
                if self.0.health_check().await {
                    "healthy"
                } else {
                    "unhealthy"
                }
                .into(),
            ),
            ..Default::default()
        })
    }
}

/// A resource is retained until its owning task acknowledges cleanup.
pub(crate) struct Running {
    pub(crate) channel: BuiltChannel,
    /// Outbound voice pipeline for module-delivered replies, when the host
    /// callbacks are bound.
    pub(crate) spoken: Option<Arc<SpokenReply>>,
    send: mpsc::Sender<SendCommand>,
    cancel: watch::Sender<bool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    finished: Arc<AtomicBool>,
}
impl Running {
    #[cfg(test)]
    pub(crate) fn start(
        channel: Arc<dyn Channel>,
        hooks: Arc<Hooks>,
        host: HostChannels,
    ) -> Arc<Self> {
        Self::start_inner(
            BuiltChannel::from_adapter(Arc::new(LegacyChannelAdapter(channel.clone()))),
            Some(channel),
            hooks,
            host,
            None,
            false,
        )
    }
    pub(crate) fn start_built(
        channel: BuiltChannel,
        hooks: Arc<Hooks>,
        host: HostChannels,
    ) -> Arc<Self> {
        Self::start_inner(channel, None, hooks, host, None, false)
    }
    #[cfg(test)]
    pub(crate) fn start_supervised(
        channel: Arc<dyn Channel>,
        hooks: Arc<Hooks>,
        host: HostChannels,
        policy: ChannelSupervisionPolicy,
    ) -> Arc<Self> {
        Self::start_inner(
            BuiltChannel::from_adapter(Arc::new(LegacyChannelAdapter(channel.clone()))),
            Some(channel),
            hooks,
            host,
            Some(policy),
            false,
        )
    }
    pub(crate) fn start_built_supervised(
        channel: BuiltChannel,
        hooks: Arc<Hooks>,
        host: HostChannels,
        policy: ChannelSupervisionPolicy,
    ) -> Arc<Self> {
        Self::start_inner(channel, None, hooks, host, Some(policy), false)
    }
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn start_outbound(
        channel: Arc<dyn Channel>,
        hooks: Arc<Hooks>,
        host: HostChannels,
    ) -> Arc<Self> {
        Self::start_inner(
            BuiltChannel::from_adapter(Arc::new(LegacyChannelAdapter(channel.clone()))),
            Some(channel),
            hooks,
            host,
            None,
            true,
        )
    }
    pub(crate) fn start_built_outbound(
        channel: BuiltChannel,
        hooks: Arc<Hooks>,
        host: HostChannels,
    ) -> Arc<Self> {
        Self::start_inner(channel, None, hooks, host, None, true)
    }
    #[cfg(test)]
    pub(crate) fn start_test(
        provider: Arc<dyn TestProviderBehavior>,
        hooks: Arc<Hooks>,
        host: HostChannels,
    ) -> Arc<Self> {
        Self::start_test_outbound(provider, hooks, host)
    }
    #[cfg(test)]
    pub(crate) fn start_test_outbound(
        provider: Arc<dyn TestProviderBehavior>,
        hooks: Arc<Hooks>,
        host: HostChannels,
    ) -> Arc<Self> {
        let adapter = Arc::new(TestProviderAdapter(provider));
        Self::start_built_outbound(
            BuiltChannel::with_output_ports(adapter.clone(), Some(adapter.clone()), Some(adapter)),
            hooks,
            host,
        )
    }
    fn start_inner(
        channel: BuiltChannel,
        legacy: Option<Arc<dyn Channel>>,
        hooks: Arc<Hooks>,
        host: HostChannels,
        supervision: Option<ChannelSupervisionPolicy>,
        outbound_only: bool,
    ) -> Arc<Self> {
        let (cancel, mut canceled) = watch::channel(false);
        let send_cancel = cancel.subscribe();
        let (send, mut sends) = mpsc::channel::<SendCommand>(256);
        let finished = Arc::new(AtomicBool::new(false));
        let task_finished = finished.clone();
        let adapter = channel.adapter().clone();
        let send_drafts = channel.drafts().cloned();
        let send_reactions = channel.reactions().cloned();
        let send_typing = channel.typing().cloned();
        let spoken = hooks.spoken();
        let listen_adapter = adapter.clone();
        let send_adapter = adapter.clone();
        let stop_adapter = adapter.clone();
        let listen_legacy = legacy.clone();
        let send_legacy = legacy.clone();
        let configured_instance = hooks
            .configured_instance
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| channel.name());
        let worker_cancel = cancel.clone();
        let worker = tokio::spawn(async move {
            let name = listen_adapter.descriptor().id;
            host.report_status(&name, "connecting", None).await;
            let listener_host = host.clone();
            let ingress: Arc<dyn ChannelInboundSink> =
                Arc::new(ModuleEvents(host.clone(), configured_instance.clone()));
            let mut listener = AbortOnDrop(tokio::spawn(async move {
                if outbound_only {
                    std::future::pending::<anyhow::Result<()>>().await
                } else {
                    listen_with_supervision(
                        listen_adapter,
                        listen_legacy,
                        ingress,
                        listener_host,
                        supervision,
                    )
                    .await
                }
            }));
            let mut send_cancel = send_cancel;
            let mut sender = AbortOnDrop(tokio::spawn(async move {
                let mut typing = TypingRefreshes::new();
                loop {
                    let command = tokio::select! {biased;
                        _=async {let _=send_cancel.wait_for(|value|*value).await;}=>break,
                        command=sends.recv()=>match command {Some(command)=>command,None=>break},
                    };
                    let _ = command.started.send(());
                    let result = match command.operation {
                        ProviderOperation::Send(message) => {
                            let intent = tinychannels::outbound_intent_from_send_message(
                                &configured_instance,
                                &message,
                            );
                            let idempotency_key = intent.idempotency_key.clone();
                            let conversation =
                                intent.conversation_ref.clone().unwrap_or_else(|| {
                                    tinychannels_bus::ConversationRef {
                                        id: intent.conversation_id.clone(),
                                        ..Default::default()
                                    }
                                });
                            let thread_id = intent.thread_id.clone();
                            send_adapter
                                .send_outcome(intent)
                                .await
                                .map(|mut outcome| {
                                    outcome.receipt.idempotency_key = Some(idempotency_key);
                                    outcome.receipt.channel = Some(tinychannels_bus::ChannelRef {
                                        id: tinychannels_bus::provider_id(&configured_instance)
                                            .to_owned(),
                                        account_id: Some(configured_instance.clone()),
                                    });
                                    if outcome.receipt.conversation.is_none() {
                                        outcome.receipt.conversation = Some(conversation);
                                    }
                                    if outcome.receipt.thread_id.is_none() {
                                        outcome.receipt.thread_id = thread_id;
                                    }
                                    tinychannels_bus::ChannelInteractionResult::Sent(Box::new(
                                        outcome,
                                    ))
                                })
                                .map_err(|error| Error::failed(error.message))
                        }
                        ProviderOperation::SendTyped(message) => {
                            let expected_channel_id =
                                tinychannels_bus::provider_id(&configured_instance);
                            if message.channel_ref.as_ref().is_some_and(|channel| {
                                channel.id != expected_channel_id
                                    || channel
                                        .account_id
                                        .as_deref()
                                        .is_some_and(|account| account != configured_instance)
                            }) {
                                let _ = command.reply.send(Err(Error::failed(
                                    "typed send channel does not match configured provider",
                                )));
                                continue;
                            }
                            let mut outcome = match send_adapter.send_typed(&message).await {
                                Ok(outcome) => outcome,
                                Err(error) => tinychannels_bus::channel::ChannelSendOutcome {
                                    error: Some(error),
                                    ..Default::default()
                                },
                            };
                            outcome.receipt.idempotency_key = Some(message.idempotency_key.clone());
                            outcome.receipt.channel =
                                Some(message.channel_ref.clone().unwrap_or_else(|| {
                                    tinychannels_bus::ChannelRef {
                                        id: tinychannels_bus::provider_id(&configured_instance)
                                            .to_owned(),
                                        account_id: Some(configured_instance.clone()),
                                    }
                                }));
                            if outcome.receipt.conversation.is_none() {
                                outcome.receipt.conversation =
                                    Some(message.conversation_ref.clone().unwrap_or_else(|| {
                                        tinychannels_bus::ConversationRef {
                                            id: message.conversation_id.clone(),
                                            ..Default::default()
                                        }
                                    }));
                            }
                            if outcome.receipt.thread_id.is_none() {
                                outcome.receipt.thread_id =
                                    message.thread_id.clone().or_else(|| {
                                        message
                                            .conversation_ref
                                            .as_ref()
                                            .and_then(|r| r.thread_id.clone())
                                    });
                            }
                            Ok(tinychannels_bus::ChannelInteractionResult::Sent(Box::new(
                                outcome,
                            )))
                        }
                        ProviderOperation::Interaction(interaction) => match *interaction {
                            tinychannels_bus::ChannelInteraction::UpsertDraft {
                                previous,
                                intent,
                            } => match send_drafts.as_ref() {
                                Some(drafts) => {
                                    dispatch_draft_upsert(
                                        drafts.as_ref(),
                                        &configured_instance,
                                        previous,
                                        *intent,
                                    )
                                    .await
                                }
                                None => Err(Error::failed(
                                    "provider draft upsert is not supported by this adapter",
                                )),
                            },
                            tinychannels_bus::ChannelInteraction::SendReaction {
                                channel,
                                conversation,
                                provider_message_id,
                                reaction,
                                remove,
                            } => match send_reactions.as_ref() {
                                Some(reactions) => reactions
                                    .send_reaction(
                                        &channel,
                                        &conversation,
                                        &provider_message_id,
                                        &reaction,
                                        remove,
                                    )
                                    .await
                                    .map(|()| tinychannels_bus::ChannelInteractionResult::Completed)
                                    .map_err(|error| Error::failed(error.message)),
                                None => Err(Error::failed(
                                    "provider reaction is not supported by this adapter",
                                )),
                            },
                            tinychannels_bus::ChannelInteraction::Describe => {
                                match send_legacy.as_ref() {
                                    Some(legacy) => {
                                        dispatch_interaction(
                                            legacy.clone(),
                                            tinychannels_bus::ChannelInteraction::Describe,
                                            &mut typing,
                                        )
                                        .await
                                    }
                                    None => Ok(crate::interaction::describe_adapter(
                                        send_adapter.as_ref(),
                                        true,
                                    )),
                                }
                            }
                            tinychannels_bus::ChannelInteraction::DescribeV2 => {
                                match send_legacy.as_ref() {
                                    Some(legacy) => {
                                        dispatch_interaction(
                                            legacy.clone(),
                                            tinychannels_bus::ChannelInteraction::DescribeV2,
                                            &mut typing,
                                        )
                                        .await
                                    }
                                    None => Ok(crate::interaction::describe_adapter(
                                        send_adapter.as_ref(),
                                        false,
                                    )),
                                }
                            }
                            interaction => {
                                if let Some(port) = send_typing.as_ref()
                                    && let Some(result) = dispatch_adapter_typing(
                                        port,
                                        &configured_instance,
                                        &interaction,
                                        &mut typing,
                                    )
                                    .await
                                {
                                    result
                                } else {
                                    match crate::interaction::dispatch_native_webhook(
                                        send_adapter.as_ref(),
                                        interaction.clone(),
                                    )
                                    .await
                                    {
                                        Some(result) => result,
                                        None => match send_legacy.as_ref() {
                                            Some(legacy) => {
                                                dispatch_interaction(
                                                    legacy.clone(),
                                                    interaction,
                                                    &mut typing,
                                                )
                                                .await
                                            }
                                            None => Err(Error::failed(
                                                "provider interaction is not supported by this adapter",
                                            )),
                                        },
                                    }
                                }
                            }
                        },
                    };
                    let _ = command.reply.send(result);
                }
                stop_all_typing_refreshes(&mut typing).await;
                while let Ok(command) = sends.try_recv() {
                    let _ = command
                        .reply
                        .send(Err(Error::failed("provider is stopping")));
                }
            }));
            let outcome = tokio::select! {biased;
                _=async {let _=canceled.wait_for(|value|*value).await;}=>{listener.0.abort(); let _=(&mut listener.0).await;None},
                outcome=&mut listener.0=>Some(outcome),
            };
            worker_cancel.send_replace(true);
            // Join listener first: it cannot register another native hook later.
            hooks.drain().await;
            let _ = (&mut sender.0).await;
            let _ = stop_adapter.stop().await;
            host.report_status(
                &name,
                if outcome.is_some_and(|outcome| !matches!(outcome, Ok(Ok(())))) {
                    "failed"
                } else {
                    "stopped"
                },
                None,
            )
            .await;
            task_finished.store(true, Ordering::SeqCst);
        });
        Arc::new(Self {
            channel,
            spoken,
            send,
            cancel,
            worker: Mutex::new(Some(worker)),
            finished,
        })
    }
    pub(crate) fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }
    pub(crate) fn adapter_handle(&self) -> &Arc<dyn ChannelAdapter> {
        self.channel.adapter()
    }
    pub(crate) fn provider_name(&self) -> String {
        self.channel.name()
    }
    #[cfg(test)]
    pub(crate) fn declared_capabilities(&self) -> tinychannels_bus::ChannelCapabilities {
        self.channel.capabilities()
    }
    pub(crate) async fn send(
        &self,
        message: SendMessage,
    ) -> std::result::Result<(), RunningSendError> {
        self.submit(ProviderOperation::Send(message))
            .await
            .map(|_| ())
    }

    pub(crate) async fn send_typed(
        &self,
        message: tinychannels_bus::ChannelOutboundMessage,
    ) -> std::result::Result<tinychannels_bus::channel::ChannelSendOutcome, RunningSendError> {
        self.channel
            .adapter()
            .validate_typed_send(&message)
            .map_err(|error| RunningSendError::ValidationFailed(error.message))?;
        match self.submit(ProviderOperation::SendTyped(message)).await? {
            tinychannels_bus::ChannelInteractionResult::Sent(receipt) => Ok(*receipt),
            _ => Err(RunningSendError::AfterDispatch(
                "typed send returned no receipt".into(),
            )),
        }
    }

    pub(crate) async fn interact(
        &self,
        interaction: tinychannels_bus::ChannelInteraction,
    ) -> std::result::Result<tinychannels_bus::ChannelInteractionResult, RunningSendError> {
        self.submit(ProviderOperation::Interaction(Box::new(interaction)))
            .await
    }

    async fn submit(
        &self,
        operation: ProviderOperation,
    ) -> std::result::Result<tinychannels_bus::ChannelInteractionResult, RunningSendError> {
        let (reply, wait) = oneshot::channel();
        let (started, dispatch) = oneshot::channel();
        self.send
            .try_send(SendCommand {
                operation,
                reply,
                started,
            })
            .map_err(|_| {
                RunningSendError::NotDispatched("provider send queue is full or closed".into())
            })?;
        match wait.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => match dispatch.await {
                Ok(()) => Err(RunningSendError::AfterDispatch(error.to_string())),
                Err(_) => Err(RunningSendError::NotDispatched(error.to_string())),
            },
            Err(_) => match dispatch.await {
                Ok(()) => Err(RunningSendError::AfterDispatch(
                    "provider send worker stopped after dispatch".into(),
                )),
                Err(_) => Err(RunningSendError::NotDispatched(
                    "provider send worker stopped before dispatch".into(),
                )),
            },
        }
    }
    pub(crate) async fn stop(&self) -> Result<()> {
        self.cancel.send_replace(true);
        let mut worker = self.worker.lock().await;
        if let Some(task) = worker.as_mut() {
            tokio::time::timeout(Duration::from_secs(10), task)
                .await
                .map_err(|_| Error::failed("provider cleanup is pending; retry StopChannel"))?
                .map_err(|_| Error::failed("provider cleanup worker failed"))?;
        }
        worker.take();
        Ok(())
    }
}

#[cfg(test)]
struct TestProviderAdapter(Arc<dyn TestProviderBehavior>);

#[cfg(test)]
#[async_trait::async_trait]
impl ChannelAdapter for TestProviderAdapter {
    fn descriptor(&self) -> tinychannels::channel::ChannelDescriptor {
        tinychannels::channel::ChannelDescriptor {
            id: self.0.test_provider_id().into(),
            display_name: self.0.test_provider_id().into(),
            ..Default::default()
        }
    }

    fn declared_capabilities(&self) -> tinychannels_bus::ChannelCapabilities {
        self.0.test_capabilities()
    }

    async fn start(
        &self,
        _: &(dyn ChannelInboundSink + Send + Sync),
    ) -> std::result::Result<(), ChannelSendError> {
        Ok(())
    }

    async fn stop(&self) -> std::result::Result<(), ChannelSendError> {
        Ok(())
    }

    async fn send(
        &self,
        intent: tinychannels::channel::ChannelOutboundIntent,
    ) -> std::result::Result<tinychannels::channel::MessageReceipt, ChannelSendError> {
        let mut outcome = self
            .0
            .test_send(&tinychannels_bus::ChannelOutboundMessage::from_intent(
                &intent,
            ))
            .await?;
        if let Some(error) = outcome.error.take() {
            return Err(error);
        }
        Ok(outcome.receipt)
    }

    async fn send_typed(
        &self,
        message: &tinychannels_bus::ChannelOutboundMessage,
    ) -> std::result::Result<tinychannels_bus::channel::ChannelSendOutcome, ChannelSendError> {
        self.0.test_send(message).await
    }

    async fn status(
        &self,
    ) -> std::result::Result<tinychannels::controllers::ChannelAccountSnapshot, ChannelSendError>
    {
        Ok(Default::default())
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl tinychannels::channel::ChannelEdit for TestProviderAdapter {
    async fn edit(
        &self,
        receipt: &tinychannels_bus::channel::MessageReceipt,
        intent: tinychannels_bus::ChannelOutboundIntent,
    ) -> std::result::Result<tinychannels_bus::channel::MessageReceipt, ChannelSendError> {
        self.0.test_edit(receipt, intent).await
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl tinychannels::channel::ChannelStreamingDraft for TestProviderAdapter {}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.get_mut().take() {
            worker.abort();
        }
    }
}

#[cfg(test)]
#[async_trait::async_trait]
pub(crate) trait TestProviderBehavior: Send + Sync {
    fn test_provider_id(&self) -> &'static str;

    fn test_capabilities(&self) -> tinychannels_bus::ChannelCapabilities {
        tinychannels_bus::ChannelCapabilities::unknown()
    }

    async fn test_send(
        &self,
        _message: &tinychannels_bus::ChannelOutboundMessage,
    ) -> std::result::Result<tinychannels_bus::channel::ChannelSendOutcome, ChannelSendError> {
        Err(ChannelSendError::unsupported(
            "test provider send is not configured",
        ))
    }

    async fn test_edit(
        &self,
        receipt: &tinychannels_bus::channel::MessageReceipt,
        _intent: tinychannels_bus::ChannelOutboundIntent,
    ) -> std::result::Result<tinychannels_bus::channel::MessageReceipt, ChannelSendError> {
        Ok(receipt.clone())
    }
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
