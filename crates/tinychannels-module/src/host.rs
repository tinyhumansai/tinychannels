//! The host-side object this module calls, and the client that reaches it.
//!
//! # Why inbound needs its own object
//!
//! `Interface::call` receives a member name and a JSON body — no caller
//! identity, no connection — so a served object cannot open a stream or send a
//! reply back to whoever called it. That is fine for a codec, where every
//! answer belongs to the call that asked for it. A channel is not a codec: the
//! interesting traffic is unsolicited, arriving from Telegram or Slack at a
//! moment no host call is outstanding.
//!
//! So the direction is inverted for that half. The **host** serves
//! [`HOST_BUS_NAME`], and this module calls it. That is the same shape
//! `tinymemory`'s `RuntimeHost` uses, and it is why `tinychannels-bus` declares
//! two objects rather than one.
//!
//! # One ordered consumer, not a task per message
//!
//! Notifications go through a bounded queue drained by a **single** task that
//! awaits each call before starting the next. Two properties fall out, and both
//! are load-bearing:
//!
//! - **Order is preserved.** Spawning a task per notification would let two host
//!   calls race, and the host would observe conversation turns in an order the
//!   provider never produced them in. For a chat surface that is corruption, not
//!   a performance detail.
//! - **Memory is bounded.** A task per notification also means unbounded
//!   outstanding calls and cloned payloads whenever the host is slower than the
//!   traffic, which is exactly when you can least afford to grow.
//!
//! Backpressure therefore propagates the way it already did: a full outbox
//! blocks the forwarder, which stops draining the provider's bounded inbound
//! queue, which blocks the provider — the same path a slow consumer always took.
//! Nothing is silently dropped.
//!
//! Identity runtimes may explicitly opt into a separate bounded turn dispatcher.
//! It invokes `ProcessInbound` concurrently while the notification consumer
//! still owns status/persistence/event ordering. Its barriers drain the accepted
//! turn frontier and shutdown joins the dispatcher. Legacy callers retain the
//! serial v22 `DeliverInbound` envelope contract described above.
//!
//! # Failures are logged, not retried
//!
//! A failed delivery is logged and the queue moves on. Retrying here would be
//! wrong: this module has no durable spool, so a retry loop would either stall
//! the ordered queue behind one unreachable host or grow in memory. Durable
//! redelivery belongs to whoever owns storage, which is the host.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tinybus::{BusName, Connection, Error, Result};
use tinychannels_bus::ChannelEvent;
use tinychannels_bus::names::{HOST_BUS_NAME, HOST_OBJECT_PATH, host_methods};
use tinychannels_bus::{ChannelBackendRequest, ChannelBackendResult};
use tinychannels_bus::{HostEventPublication, HostTranscriptionRequest, HostTranscriptionResult};
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::task::JoinHandle;

const OUTBOX_CAPACITY: usize = 256;
const MAX_CALLBACK_BYTES: usize = 128 * 1024;
const MAX_TRANSCRIPTION_REQUEST_BYTES: usize = 12 * 1024 * 1024;
// Base64 expands audio by 4/3. A 12 MiB serialized callback remains below
// TinyBus's 16 MiB frame limit in both directions, including JSON overhead.
const MAX_VOICE_CALLBACK_BYTES: usize = 12 * 1024 * 1024;
const MAX_ACTIVE_HOST_CALLBACKS: usize = 16;
const MAX_CONTROLLER_PAYLOAD_BYTES: usize = 1024 * 1024;
const HOST_VOICE_CALLBACK_TIMEOUT: Duration = Duration::from_secs(60);

#[path = "host_dispatch.rs"]
mod dispatch;

enum Notification {
    DeliverInbound {
        channel: String,
        message: serde_json::Value,
    },
    ReportStatus {
        channel: String,
        state: String,
        detail: Option<String>,
    },
    PersistAllowedIdentity {
        channel: String,
        identity: String,
        reply: oneshot::Sender<Result<()>>,
    },
    PublishEvent {
        publication: HostEventPublication,
        reply: oneshot::Sender<Result<()>>,
    },
    DeliverEvent {
        event: Box<ChannelEvent>,
        reply: oneshot::Sender<Result<()>>,
    },
    Barrier {
        stop: bool,
        reply: oneshot::Sender<()>,
    },
}

struct Owner {
    worker: Mutex<Option<JoinHandle<()>>>,
    terminal: AtomicBool,
    publication: Mutex<()>,
    active_callbacks: AtomicUsize,
    callbacks_drained: Notify,
    failure: Arc<StdMutex<Option<String>>>,
}

struct ActiveHostCallback(Arc<Owner>);

impl Drop for ActiveHostCallback {
    fn drop(&mut self) {
        self.0.active_callbacks.fetch_sub(1, Ordering::SeqCst);
        self.0.callbacks_drained.notify_waiters();
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.get_mut().take() {
            worker.abort();
        }
    }
}

/// An ordered, bounded client for the optional host callback object.
#[derive(Clone)]
pub struct HostChannels {
    outbox: mpsc::Sender<Notification>,
    owner: Arc<Owner>,
    connection: Connection,
    object_path: Arc<str>,
    /// Optional unique host callback destination supplied by the loader.
    storage_callback: Option<BusName>,
    /// The host's storage objects, connected on first use and then shared by
    /// every clone.
    storage: Arc<Mutex<Option<tinystoragedrivers_api::Storage>>>,
}
impl std::fmt::Debug for HostChannels {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostChannels")
            .finish_non_exhaustive()
    }
}
impl HostChannels {
    #[cfg(test)]
    pub(crate) fn new_at_with_dispatch(
        connection: Connection,
        object_path: String,
        max_in_flight: usize,
    ) -> Self {
        Self::new_at_inner(connection, object_path, Some(max_in_flight), None)
    }

    pub(crate) fn new_at_with_dispatch_and_storage_callback(
        connection: Connection,
        object_path: String,
        max_in_flight: usize,
        storage_callback: Option<BusName>,
    ) -> Self {
        Self::new_at_inner(
            connection,
            object_path,
            Some(max_in_flight),
            storage_callback,
        )
    }

    /// Start one owned callback consumer. Explicit shutdown drains and joins it.
    #[must_use]
    pub fn new(connection: Connection) -> Self {
        Self::new_at(connection, HOST_OBJECT_PATH.to_owned())
    }

    pub(crate) fn new_with_storage_callback(
        connection: Connection,
        storage_callback: Option<BusName>,
    ) -> Self {
        Self::new_at_inner(
            connection,
            HOST_OBJECT_PATH.to_owned(),
            None,
            storage_callback,
        )
    }

    /// Bind all callbacks to an immutable runtime-specific host object.
    pub(crate) fn new_at(connection: Connection, object_path: String) -> Self {
        Self::new_at_inner(connection, object_path, None, None)
    }

    pub(crate) fn new_at_with_storage_callback(
        connection: Connection,
        object_path: String,
        storage_callback: Option<BusName>,
    ) -> Self {
        Self::new_at_inner(connection, object_path, None, storage_callback)
    }

    fn new_at_inner(
        connection: Connection,
        object_path: String,
        dispatch_limit: Option<usize>,
        storage_callback: Option<BusName>,
    ) -> Self {
        let object_path: Arc<str> = object_path.into();
        let callback_path = object_path.clone();
        let (outbox, mut inbox) = mpsc::channel(OUTBOX_CAPACITY);
        let failure = Arc::new(StdMutex::new(None));
        let task_failure = failure.clone();
        let callback_connection = connection.clone();
        let worker = tokio::spawn(async move {
            let mut dispatcher = dispatch_limit.map(|limit| {
                dispatch::InboundDispatcher::new(
                    callback_connection.clone(),
                    callback_path.clone(),
                    task_failure.clone(),
                    limit,
                )
            });
            while let Some(notification) = inbox.recv().await {
                if let Notification::Barrier { stop, reply } = notification {
                    if let Some(dispatcher) = dispatcher.as_ref() {
                        dispatcher.flush().await;
                    }
                    let _ = reply.send(());
                    if stop {
                        break;
                    }
                    continue;
                }
                if let (Some(dispatcher), Notification::DeliverInbound { channel, message }) =
                    (dispatcher.as_mut(), &notification)
                {
                    dispatcher.enqueue(channel.clone(), message.clone()).await;
                    continue;
                }
                if let Notification::PersistAllowedIdentity {
                    channel,
                    identity,
                    reply,
                } = notification
                {
                    let result = async {
                        let proxy = callback_connection.proxy(
                            HOST_BUS_NAME,
                            callback_path.as_ref(),
                            HOST_BUS_NAME,
                        )?;
                        proxy
                            .call::<()>(host_methods::PERSIST_ALLOWED_IDENTITY, (channel, identity))
                            .await
                    }
                    .await;
                    let _ = reply.send(result);
                    continue;
                }
                if let Notification::PublishEvent { publication, reply } = notification {
                    let result = async {
                        let proxy = callback_connection.proxy(
                            HOST_BUS_NAME,
                            callback_path.as_ref(),
                            HOST_BUS_NAME,
                        )?;
                        proxy
                            .call::<()>(host_methods::PUBLISH_EVENT, publication)
                            .await
                    }
                    .await;
                    let result = match result {
                        Err(error)
                            if error.wire_name()
                                == "ai.tinyhumans.tinybus.Error.NameHasNoOwner"
                                || error.wire_name()
                                    == "ai.tinyhumans.tinybus.Error.MemberNotFound" =>
                        {
                            // Event sinks are optional capabilities. A host on
                            // an older callback surface must not make provider
                            // startup or shutdown fail because an event is
                            // unavailable.
                            Ok(())
                        }
                        result => result,
                    };
                    let _ = reply.send(result);
                    continue;
                }
                if let Notification::DeliverEvent { event, reply } = notification {
                    // Typed events share provider ingress order with messages.
                    // Scoped runtimes may still be processing an earlier
                    // message in the parallel dispatcher after its notification
                    // left this queue, so drain that accepted frontier first.
                    if let Some(dispatcher) = dispatcher.as_ref() {
                        dispatcher.flush().await;
                    }
                    let result = async {
                        let proxy = callback_connection.proxy(
                            HOST_BUS_NAME,
                            callback_path.as_ref(),
                            HOST_BUS_NAME,
                        )?;
                        proxy.call::<()>(host_methods::DELIVER_EVENT, *event).await
                    }
                    .await;
                    let _ = reply.send(result);
                    continue;
                }
                let inbound = matches!(notification, Notification::DeliverInbound { .. });
                let result = async {
                    let proxy = callback_connection.proxy(
                        HOST_BUS_NAME,
                        callback_path.as_ref(),
                        HOST_BUS_NAME,
                    )?;
                    match notification {
                        Notification::DeliverInbound { channel, message } => {
                            proxy
                                .call::<()>(host_methods::DELIVER_INBOUND, (channel, message))
                                .await
                        }
                        Notification::ReportStatus {
                            channel,
                            state,
                            detail,
                        } => {
                            proxy
                                .call::<()>(host_methods::REPORT_STATUS, (channel, state, detail))
                                .await
                        }
                        Notification::PersistAllowedIdentity { .. } => unreachable!(),
                        Notification::PublishEvent { .. } => unreachable!(),
                        Notification::DeliverEvent { .. } => unreachable!(),
                        Notification::Barrier { .. } => Ok(()),
                    }
                }
                .await;
                if let Err(error) = result {
                    // No host object is a supported outbound-only configuration.
                    if inbound || error.wire_name() != "ai.tinyhumans.tinybus.Error.NameHasNoOwner"
                    {
                        record_failure(&task_failure, error.to_string());
                    }
                }
            }
            if let Some(dispatcher) = dispatcher.as_mut() {
                dispatcher.shutdown().await;
            }
        });
        Self {
            outbox,
            connection,
            object_path,
            storage_callback,
            storage: Arc::new(Mutex::new(None)),
            owner: Arc::new(Owner {
                worker: Mutex::new(Some(worker)),
                terminal: AtomicBool::new(false),
                publication: Mutex::new(()),
                active_callbacks: AtomicUsize::new(0),
                callbacks_drained: Notify::new(),
                failure,
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn connection(&self) -> Connection {
        self.connection.clone()
    }

    /// The module's scoped handle on the host's storage, over the storage bus.
    ///
    /// The host binds the scope from the authenticated caller; nothing here
    /// names one. A connect failure is not cached, so a host that starts
    /// serving storage later is picked up by the next call.
    pub(crate) async fn storage(
        &self,
    ) -> tinystoragedrivers_types::Result<tinystoragedrivers_api::Storage> {
        let mut slot = self.storage.lock().await;
        if let Some(scoped) = slot.as_ref() {
            return Ok(scoped.clone());
        }
        let storage = match &self.storage_callback {
            Some(destination) => {
                tinystoragedrivers_busproxy::connect_storage_at(
                    self.connection.clone(),
                    destination.clone(),
                )
                .await?
            }
            None => tinystoragedrivers_busproxy::connect_storage(self.connection.clone()).await?,
        };
        self.prepare_storage_schema(storage.documents()).await?;
        *slot = Some(storage.clone());
        Ok(storage)
    }

    async fn prepare_storage_schema(
        &self,
        documents: &Arc<dyn tinystoragedrivers_api::DocumentStore>,
    ) -> tinystoragedrivers_types::Result<()> {
        // A callback host installs the owner's declarations before granting
        // access. Its module-facing port deliberately denies schema mutation.
        // The legacy standalone storage route still owns its preparation.
        if self.storage_callback.is_some() {
            Ok(())
        } else {
            tinychannels::storage::prepare(documents).await
        }
    }

    /// The scoped document store, for the delivery, remote-session and
    /// WhatsApp session records.
    pub(crate) async fn documents(
        &self,
    ) -> tinystoragedrivers_types::Result<Arc<dyn tinystoragedrivers_api::DocumentStore>> {
        Ok(Arc::clone(self.storage().await?.documents()))
    }

    /// Request transcription through the host's configured speech provider.
    pub async fn transcribe(
        &self,
        request: HostTranscriptionRequest,
    ) -> Result<HostTranscriptionResult> {
        if serde_json::to_vec(&request)
            .map_or(true, |bytes| bytes.len() > MAX_TRANSCRIPTION_REQUEST_BYTES)
        {
            return Err(Error::failed("transcription request exceeds byte limit"));
        }
        let publication = self.owner.publication.lock().await;
        if self.owner.terminal.load(Ordering::SeqCst) {
            return Err(Error::failed("callback consumer is closed"));
        }
        // Admission is serialized with shutdown by `publication`; callbacks
        // can only decrement this count while an admission is in progress.
        // Reserve first, then roll back if the existing count was already full.
        if self.owner.active_callbacks.fetch_add(1, Ordering::SeqCst) >= MAX_ACTIVE_HOST_CALLBACKS {
            self.owner.active_callbacks.fetch_sub(1, Ordering::SeqCst);
            return Err(Error::failed("too many active transcription callbacks"));
        }
        let owner = self.owner.clone();
        let connection = self.connection.clone();
        let object_path = self.object_path.clone();
        let (reply, wait) = oneshot::channel();
        drop(publication);
        tokio::spawn(async move {
            let _active = ActiveHostCallback(owner);
            let result = async {
                let proxy = connection.proxy(HOST_BUS_NAME, object_path.as_ref(), HOST_BUS_NAME)?;
                proxy.call(host_methods::TRANSCRIBE, request).await
            }
            .await;
            let _ = reply.send(result);
        });
        wait.await
            .map_err(|_| Error::failed("transcription callback was interrupted"))?
    }

    /// Run a scoped voice callback with symmetric frame bounds, bounded
    /// lifetime, and shutdown ownership. The spawned callback remains tracked
    /// if the caller cancels its waiter.
    async fn voice_callback<Request, Response>(
        &self,
        method: &'static str,
        request: Request,
    ) -> Result<Response>
    where
        Request: serde::Serialize + Send + 'static,
        Response: serde::de::DeserializeOwned + serde::Serialize + Send + 'static,
    {
        if serde_json::to_vec(&request).map_or(true, |bytes| bytes.len() > MAX_VOICE_CALLBACK_BYTES)
        {
            return Err(Error::failed("voice callback request exceeds byte limit"));
        }
        let publication = self.owner.publication.lock().await;
        if self.owner.terminal.load(Ordering::SeqCst) {
            return Err(Error::failed("callback consumer is closed"));
        }
        if self.owner.active_callbacks.fetch_add(1, Ordering::SeqCst) >= MAX_ACTIVE_HOST_CALLBACKS {
            self.owner.active_callbacks.fetch_sub(1, Ordering::SeqCst);
            return Err(Error::failed("too many active voice callbacks"));
        }
        let owner = self.owner.clone();
        let connection = self.connection.clone();
        let object_path = self.object_path.clone();
        let (reply, wait) = oneshot::channel();
        drop(publication);
        tokio::spawn(async move {
            let _active = ActiveHostCallback(owner);
            let result = tokio::time::timeout(HOST_VOICE_CALLBACK_TIMEOUT, async {
                let proxy = connection.proxy(HOST_BUS_NAME, object_path.as_ref(), HOST_BUS_NAME)?;
                let response: Response = proxy.call(method, request).await?;
                if serde_json::to_vec(&response)
                    .map_or(true, |bytes| bytes.len() > MAX_VOICE_CALLBACK_BYTES)
                {
                    return Err(Error::failed("voice callback response exceeds byte limit"));
                }
                Ok(response)
            })
            .await
            .unwrap_or_else(|_| Err(Error::failed("voice callback timed out")));
            let _ = reply.send(result);
        });
        wait.await
            .map_err(|_| Error::failed("voice callback was interrupted"))?
    }

    /// Request scoped speech transcription from the host.
    pub async fn transcribe_voice(
        &self,
        request: tinychannels_bus::HostVoiceTranscriptionRequest,
    ) -> Result<HostTranscriptionResult> {
        self.voice_callback(host_methods::TRANSCRIBE_VOICE, request)
            .await
    }

    /// Request scoped speech synthesis from the host.
    pub async fn synthesize_speech(
        &self,
        request: tinychannels_bus::HostSpeechSynthesisRequest,
    ) -> Result<tinychannels_bus::HostSpeechSynthesisResult> {
        let result: tinychannels_bus::HostSpeechSynthesisResult = self
            .voice_callback(host_methods::SYNTHESIZE_SPEECH, request)
            .await?;
        if result.audio_base64.trim().is_empty() || result.mime_type.trim().is_empty() {
            return Err(Error::failed(
                "voice callback returned unusable speech audio",
            ));
        }
        Ok(result)
    }

    /// Persist audio through the host and receive its opaque durable reference.
    pub async fn store_voice_media(
        &self,
        request: tinychannels_bus::HostStoreVoiceMediaRequest,
    ) -> Result<tinychannels_bus::HostStoreVoiceMediaResult> {
        self.voice_callback(host_methods::STORE_VOICE_MEDIA, request)
            .await
    }

    /// Resolve scoped media bytes for a provider-native upload.
    pub async fn read_voice_media(
        &self,
        request: tinychannels_bus::HostReadVoiceMediaRequest,
    ) -> Result<tinychannels_bus::HostReadVoiceMediaResult> {
        let maximum = usize::try_from(request.max_bytes)
            .map_err(|_| Error::failed("voice media byte limit is invalid"))?;
        let result: tinychannels_bus::HostReadVoiceMediaResult = self
            .voice_callback(host_methods::READ_VOICE_MEDIA, request)
            .await?;
        let decoded = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &result.audio_base64,
        )
        .map_err(|_| Error::failed("host returned invalid base64 voice media"))?;
        if decoded.len() > maximum {
            return Err(Error::failed(
                "host returned voice media above requested limit",
            ));
        }
        Ok(result)
    }

    /// Resolve host-authoritative voice policy for the active scoped turn.
    pub async fn voice_reply_policy(
        &self,
        request: tinychannels_bus::HostVoiceReplyPolicyRequest,
    ) -> Result<tinychannels_bus::HostVoiceReplyPolicy> {
        self.voice_callback(host_methods::GET_VOICE_REPLY_POLICY, request)
            .await
    }

    /// Call the loaded TinyVoice codec module over its minimal bus contract.
    pub async fn transcode_voice(
        &self,
        request: tinyvoice_bus::codec::CodecTranscodeRequest,
    ) -> Result<tinyvoice_bus::codec::CodecOutput> {
        const LIMIT: usize = 12 * 1024 * 1024;
        if serde_json::to_vec(&request).map_or(true, |bytes| bytes.len() > LIMIT) {
            return Err(Error::failed("voice codec request exceeds byte limit"));
        }
        let proxy = self.connection.proxy(
            tinyvoice_bus::BUS_NAME,
            tinyvoice_bus::OBJECT_PATH,
            tinyvoice_bus::BUS_NAME,
        )?;
        let response: tinyvoice_bus::codec::CodecOutput = tokio::time::timeout(
            HOST_VOICE_CALLBACK_TIMEOUT,
            proxy.call(tinyvoice_bus::names::methods::CODEC_TRANSCODE, (request,)),
        )
        .await
        .map_err(|_| Error::failed("TinyVoice codec call timed out"))??;
        if serde_json::to_vec(&response).map_or(true, |bytes| bytes.len() > LIMIT) {
            return Err(Error::failed("voice codec response exceeds byte limit"));
        }
        Ok(response)
    }

    /// Probe media through the loaded TinyVoice codec module.
    pub async fn probe_voice(
        &self,
        request: tinyvoice_bus::codec::CodecInput,
    ) -> Result<tinyvoice_bus::codec::CodecInfo> {
        const LIMIT: usize = 12 * 1024 * 1024;
        if serde_json::to_vec(&request).map_or(true, |bytes| bytes.len() > LIMIT) {
            return Err(Error::failed("voice codec probe exceeds byte limit"));
        }
        let proxy = self.connection.proxy(
            tinyvoice_bus::BUS_NAME,
            tinyvoice_bus::OBJECT_PATH,
            tinyvoice_bus::BUS_NAME,
        )?;
        tokio::time::timeout(
            HOST_VOICE_CALLBACK_TIMEOUT,
            proxy.call(tinyvoice_bus::names::methods::CODEC_PROBE, (request,)),
        )
        .await
        .map_err(|_| Error::failed("TinyVoice codec probe timed out"))?
    }

    /// Extract a bounded normalized waveform through TinyVoice.
    pub async fn waveform_voice(
        &self,
        request: tinyvoice_bus::codec::WaveformRequest,
    ) -> Result<tinyvoice_bus::codec::WaveformResult> {
        const LIMIT: usize = 12 * 1024 * 1024;
        if serde_json::to_vec(&request).map_or(true, |bytes| bytes.len() > LIMIT) {
            return Err(Error::failed("voice waveform request exceeds byte limit"));
        }
        let proxy = self.connection.proxy(
            tinyvoice_bus::BUS_NAME,
            tinyvoice_bus::OBJECT_PATH,
            tinyvoice_bus::BUS_NAME,
        )?;
        tokio::time::timeout(
            HOST_VOICE_CALLBACK_TIMEOUT,
            proxy.call(tinyvoice_bus::names::methods::CODEC_WAVEFORM, (request,)),
        )
        .await
        .map_err(|_| Error::failed("TinyVoice waveform call timed out"))?
    }

    /// Invoke one bounded host-owned credential, persistence, or managed API
    /// capability required by the module's controller adapter.
    pub async fn dispatch_controller_backend(
        &self,
        request: ChannelBackendRequest,
    ) -> Result<ChannelBackendResult> {
        if serde_json::to_vec(&request)
            .map_or(true, |bytes| bytes.len() > MAX_CONTROLLER_PAYLOAD_BYTES)
        {
            return Err(Error::failed("controller request exceeds byte limit"));
        }
        let publication = self.owner.publication.lock().await;
        if self.owner.terminal.load(Ordering::SeqCst) {
            return Err(Error::failed("callback consumer is closed"));
        }
        if self.owner.active_callbacks.fetch_add(1, Ordering::SeqCst) >= MAX_ACTIVE_HOST_CALLBACKS {
            self.owner.active_callbacks.fetch_sub(1, Ordering::SeqCst);
            return Err(Error::failed("too many active host callbacks"));
        }
        let owner = self.owner.clone();
        let connection = self.connection.clone();
        let object_path = self.object_path.clone();
        let (reply, wait) = oneshot::channel();
        drop(publication);
        tokio::spawn(async move {
            let _active = ActiveHostCallback(owner);
            let result = async {
                let proxy = connection.proxy(HOST_BUS_NAME, object_path.as_ref(), HOST_BUS_NAME)?;
                let result: ChannelBackendResult = proxy
                    .call(host_methods::DISPATCH_CONTROLLER_BACKEND, request)
                    .await?;
                if serde_json::to_vec(&result)
                    .map_or(true, |bytes| bytes.len() > MAX_CONTROLLER_PAYLOAD_BYTES)
                {
                    return Err(Error::failed("controller result exceeds byte limit"));
                }
                Ok(result)
            }
            .await;
            let _ = reply.send(result);
        });
        wait.await
            .map_err(|_| Error::failed("host controller callback was interrupted"))?
    }

    /// Publish one bounded provider event through the host event sink.
    pub async fn publish_event(&self, publication: HostEventPublication) -> Result<()> {
        if serde_json::to_vec(&publication).map_or(true, |bytes| bytes.len() > MAX_CALLBACK_BYTES) {
            return Err(Error::failed("event callback byte limit exceeded"));
        }
        let (reply, wait) = oneshot::channel();
        let _publication = self.owner.publication.lock().await;
        if self.owner.terminal.load(Ordering::SeqCst) {
            return Err(Error::failed("callback consumer is closed"));
        }
        self.outbox
            .send(Notification::PublishEvent { publication, reply })
            .await
            .map_err(|_| Error::failed("callback consumer is closed"))?;
        drop(_publication);
        wait.await
            .map_err(|_| Error::failed("event publication was interrupted"))?
    }

    /// Deliver one typed provider event over the scoped host callback.
    pub async fn deliver_event(&self, event: ChannelEvent) -> Result<()> {
        if serde_json::to_vec(&event).map_or(true, |bytes| bytes.len() > MAX_CALLBACK_BYTES) {
            return Err(Error::failed("channel event callback byte limit exceeded"));
        }
        let (reply, wait) = oneshot::channel();
        let _publication = self.owner.publication.lock().await;
        if self.owner.terminal.load(Ordering::SeqCst) {
            return Err(Error::failed("callback consumer is closed"));
        }
        self.outbox
            .send(Notification::DeliverEvent {
                event: Box::new(event),
                reply,
            })
            .await
            .map_err(|_| Error::failed("callback consumer is closed"))?;
        drop(_publication);
        match wait
            .await
            .map_err(|_| Error::failed("event delivery was interrupted"))?
        {
            Err(error)
                if error.wire_name() == "ai.tinyhumans.tinybus.Error.NameHasNoOwner"
                    || error.wire_name() == "ai.tinyhumans.tinybus.Error.MemberNotFound" =>
            {
                Ok(())
            }
            result => result,
        }
    }

    /// Deliver one typed provider event and preserve whether failure came
    /// from invalid ingress, a closed consumer, or the host callback itself.
    pub async fn try_deliver_event(
        &self,
        event: ChannelEvent,
    ) -> std::result::Result<(), tinychannels::channel::ChannelSendError> {
        if serde_json::to_vec(&event).map_or(true, |bytes| bytes.len() > MAX_CALLBACK_BYTES) {
            return Err(ingress_rejected_error());
        }
        if self.owner.terminal.load(Ordering::SeqCst) {
            return Err(inbound_host_closed_error());
        }
        match self.deliver_event(event).await {
            Ok(()) => Ok(()),
            Err(error) if error.to_string() == "callback consumer is closed" => {
                Err(inbound_host_closed_error())
            }
            Err(error) => {
                tracing::warn!(%error, "channel host ingress callback failed");
                Err(host_ingress_transient_error())
            }
        }
    }

    /// Enqueue one inbound message; a flush observes delivery failures.
    pub async fn deliver_inbound(
        &self,
        channel: &str,
        envelope: tinychannels_bus::ChannelInboundEnvelope,
    ) {
        if let Err(error) = self.try_deliver_inbound(channel, envelope).await {
            record_failure(&self.owner.failure, error.message);
        }
    }

    /// Enqueue one inbound message and report rejection to an adapter that can
    /// retry it. The legacy [`Self::deliver_inbound`] method retains its
    /// fire-and-forget behavior for existing callers.
    pub async fn try_deliver_inbound(
        &self,
        channel: &str,
        envelope: tinychannels_bus::ChannelInboundEnvelope,
    ) -> std::result::Result<(), tinychannels::channel::ChannelSendError> {
        let message = serde_json::to_value(envelope).map_err(|_| ingress_rejected_error())?;
        self.try_deliver_inbound_value(channel, message).await
    }

    /// Enqueue authenticated relay provenance with its normalized envelope.
    pub async fn deliver_relay_inbound(
        &self,
        channel: &str,
        delivery: tinychannels_bus::module_relay::RelayInboundDelivery,
    ) {
        let Ok(message) = serde_json::to_value(delivery) else {
            record_failure(
                &self.owner.failure,
                "relay envelope could not be encoded".into(),
            );
            return;
        };
        self.deliver_inbound_value(channel, message).await;
    }

    async fn deliver_inbound_value(&self, channel: &str, message: serde_json::Value) {
        if let Err(error) = self.try_deliver_inbound_value(channel, message).await {
            record_failure(&self.owner.failure, error.message);
        }
    }

    async fn try_deliver_inbound_value(
        &self,
        channel: &str,
        message: serde_json::Value,
    ) -> std::result::Result<(), tinychannels::channel::ChannelSendError> {
        if serde_json::to_vec(&message).map_or(true, |bytes| bytes.len() > MAX_CALLBACK_BYTES) {
            return Err(ingress_rejected_error());
        }
        self.enqueue_result(Notification::DeliverInbound {
            channel: channel.to_owned(),
            message,
        })
        .await
        .map_err(|_| inbound_host_closed_error())?;
        Ok(())
    }

    /// Enqueue a status fact after preceding inbound messages.
    pub async fn report_status(&self, channel: &str, state: &str, detail: Option<String>) {
        if channel.len() > 1024
            || state.len() > 128
            || detail.as_ref().is_some_and(|detail| detail.len() > 4096)
        {
            record_failure(
                &self.owner.failure,
                "status callback byte limit exceeded".into(),
            );
            return;
        }
        self.enqueue(Notification::ReportStatus {
            channel: channel.to_owned(),
            state: state.to_owned(),
            detail,
        })
        .await;
    }

    /// Persist a provider-authorized identity through the host-owned policy store.
    pub async fn persist_allowed_identity(&self, channel: &str, identity: &str) -> Result<()> {
        let (reply, wait) = oneshot::channel();
        let _publication = self.owner.publication.lock().await;
        if self.owner.terminal.load(Ordering::SeqCst) {
            return Err(Error::failed("callback consumer is closed"));
        }
        self.outbox
            .send(Notification::PersistAllowedIdentity {
                channel: channel.to_owned(),
                identity: identity.to_owned(),
                reply,
            })
            .await
            .map_err(|_| Error::failed("callback consumer is closed"))?;
        drop(_publication);
        wait.await
            .map_err(|_| Error::failed("allowlist persistence was interrupted"))?
    }

    async fn enqueue(&self, notification: Notification) {
        if let Err(error) = self.enqueue_result(notification).await {
            record_failure(&self.owner.failure, error.to_string());
        }
    }

    async fn enqueue_result(&self, notification: Notification) -> Result<()> {
        let _publication = self.owner.publication.lock().await;
        if self.owner.terminal.load(Ordering::SeqCst) {
            return Err(Error::failed("callback consumer is closed"));
        }
        self.outbox
            .send(notification)
            .await
            .map_err(|_| Error::failed("callback consumer is closed"))
    }

    /// Await all preceding callbacks, returning an observed terminal delivery error.
    ///
    /// # Errors
    /// Returns a callback failure once, or a closed-consumer error. Dropping this
    /// waiter does not cancel the owned consumer or discard its failure facts.
    pub async fn flush(&self) -> Result<()> {
        let (reply, wait) = oneshot::channel();
        self.outbox
            .send(Notification::Barrier { stop: false, reply })
            .await
            .map_err(|_| Error::failed("callback consumer is closed"))?;
        wait.await
            .map_err(|_| Error::failed("callback barrier was interrupted"))?;
        self.take_failure()
    }

    /// Freeze publication, drain all callbacks and join the consumer.
    ///
    /// # Errors
    /// Reports a terminal delivery error once. A canceled waiter leaves the
    /// join handle retained for a later shutdown retry.
    pub async fn shutdown(&self) -> Result<()> {
        let publication = self.owner.publication.lock().await;
        self.owner.terminal.store(true, Ordering::SeqCst);
        drop(publication);
        loop {
            let notified = self.owner.callbacks_drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.owner.active_callbacks.load(Ordering::SeqCst) == 0 {
                break;
            }
            notified.await;
        }
        let (reply, wait) = oneshot::channel();
        if self
            .outbox
            .send(Notification::Barrier { stop: true, reply })
            .await
            .is_ok()
        {
            let _ = wait.await;
        }
        let mut worker = self.owner.worker.lock().await;
        if let Some(task) = worker.as_mut() {
            let _ = task.await;
        }
        worker.take();
        self.take_failure()
    }

    fn take_failure(&self) -> Result<()> {
        match self
            .owner
            .failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            Some(error) => Err(Error::failed(error)),
            None => Ok(()),
        }
    }
}

fn ingress_rejected_error() -> tinychannels::channel::ChannelSendError {
    tinychannels::channel::ChannelSendError {
        kind: tinychannels::channel::SendErrorKind::BadFormat,
        message: "channel host rejected ingress payload".into(),
        ..Default::default()
    }
}

fn inbound_host_closed_error() -> tinychannels::channel::ChannelSendError {
    tinychannels::channel::ChannelSendError {
        kind: tinychannels::channel::SendErrorKind::Unknown,
        message: "channel host ingress consumer is closed".into(),
        ..Default::default()
    }
}

fn host_ingress_transient_error() -> tinychannels::channel::ChannelSendError {
    tinychannels::channel::ChannelSendError {
        kind: tinychannels::channel::SendErrorKind::Transient,
        message: "channel host ingress callback failed".into(),
        retryable: true,
        ..Default::default()
    }
}

fn record_failure(failure: &StdMutex<Option<String>>, error: String) {
    let mut failure = failure.lock().unwrap_or_else(|error| error.into_inner());
    if failure.is_none() {
        *failure = Some(error);
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "host_dispatch_tests.rs"]
mod dispatch_tests;
