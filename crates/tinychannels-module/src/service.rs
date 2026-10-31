//! The TinyBus service boundary for the channel surface.
//!
//! One object, `/ai/tinyhumans/tinychannels/Channels`, preserving its original
//! five methods and adding typed intent, relay lifecycle, and shutdown members:
//!
//! ```text
//! StartChannel(name, ChannelsConfig) -> ()
//! StopChannel(name)                  -> ()
//! SendMessage(name, SendMessage)     -> ()
//! ListChannels()                     -> Vec<String>
//! ChannelStatus(name)                -> ChannelState
//! StartChannelConfigured(name, config, proxy) -> ()
//! StartChannelSupervised(name, config, proxy, policy) -> ()
//! ChannelController(request) -> ChannelControllerResult
//! SendChannelIntent(name, intent)     -> ChannelSendMessageResult
//! ```
//!
//! # This module is stateful, unlike the document one
//!
//! `tinydocs` holds nothing between calls: a document is generated and handed
//! back. A channel provider is the opposite — it owns a live connection and a
//! listen loop that outlives every call. So this object keeps a registry of
//! running providers, and `StopChannel` is not optional politeness: without it
//! a provider's task and socket leak until the process exits.
//!
//! # Config arrives per call, not at load
//!
//! `StartChannel` takes the whole [`ChannelsConfig`] rather than reading one
//! handed over at load time, because a desktop host changes it at runtime — a
//! user pastes a bot token and expects the channel to come up. Reading a
//! load-time config would mean a module restart per edit.
//!
//! Credentials in it are expected to be **already hydrated** by the host; see
//! `tinychannels::factory`. This module has no keyring and cannot resolve a
//! placeholder.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tinychannels::config::WhatsAppConfigExt as _;

use crate::delivery::HostDeliveryStore;
use crate::provider::{Hooks, Running, RunningSendError};
use crate::relay::Relays;
use std::sync::atomic::{AtomicBool, Ordering};
use tinybus::{BusName, Connection, Error as BusError, Result as BusResult};
use tinychannels::delivery::{
    DeliveryAttemptFailure, DeliveryAttemptResult, DeliveryAttemptSuccess, DeliveryQueueError,
    DeliveryQueueHandler, DeliveryQueueStore, DeliveryResultStore, PendingDeliveryDrainDecision,
    QueuedDelivery, UnknownSendReconciliation, ack_delivery, drain_selected_pending_deliveries,
    enqueue_delivery, recover_pending_deliveries,
};
use tinychannels::factory::{
    ConfiguredHttpClients, DefaultHttpClients, HttpClientFactory, build_channels,
};
use tinychannels::host::ChannelHost;
use tinychannels_bus::ChannelControllerRequest;
use tinychannels_bus::ChannelControllerResult;
use tinychannels_bus::ChannelOutboundIntent;
use tinychannels_bus::HttpProxyPolicy;
use tinychannels_bus::config::RelayRuntimeConfig;
use tinychannels_bus::controllers::ChannelSendMessageResult;
use tinychannels_bus::module_relay::{RelayReservation, RelayStatus};
use tinychannels_bus::{
    BUS_NAME, ChannelsConfig, EmailSendRequest, EmailUploadBegin, EmailUploadChunk,
    EmailUploadHandle, OBJECT_PATH, SendMessage,
};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::email::EmailSender;
use crate::host::HostChannels;

const UNKNOWN_CHANNEL_ERROR: &str = "ai.tinyhumans.tinychannels.Error.UnknownChannel";
const ALREADY_RUNNING_ERROR: &str = "ai.tinyhumans.tinychannels.Error.AlreadyRunning";
const NOT_CONFIGURED_ERROR: &str = "ai.tinyhumans.tinychannels.Error.NotConfigured";
const SEND_FAILED_ERROR: &str = "ai.tinyhumans.tinychannels.Error.SendFailed";
const NO_INGRESS_ERROR: &str = "ai.tinyhumans.tinychannels.Error.NoWebhookIngress";

/// Whether `name`, as configured, receives inbound traffic through a webhook
/// this module cannot serve. See the refusal in `start_channel` for why.
fn is_webhook_backed(name: &str, config: &ChannelsConfig) -> bool {
    name == "linq"
        || (name == "whatsapp"
            && config
                .whatsapp
                .as_ref()
                .is_some_and(|wa| wa.backend_type() == "cloud"))
}

/// Preserve the v21 text-send path for prepared legacy JSON messages.
///
/// `NativeChannelData` is the compatibility representation used by both the
/// legacy JSON and `SendMessage` preparation members. Providers that have not
/// migrated to typed sends still receive their original text through `send`;
/// opaque native payloads without a text field continue through typed dispatch.
fn legacy_text_message(intent: &ChannelOutboundIntent) -> Option<SendMessage> {
    let tinychannels_bus::OutboundPayload::NativeChannelData { data } = &intent.payload else {
        return None;
    };
    let allowed_fields = [
        "content",
        "text",
        "recipient",
        "conversationId",
        "conversation_id",
        "subject",
        "thread_ts",
        "threadId",
        "thread_id",
        "topicId",
        "topic_id",
        "replyToId",
        "reply_to_id",
        "idempotencyKey",
        "idempotency_key",
        "turnId",
        "turn_id",
        "originatingTurnId",
    ];
    if data
        .as_object()?
        .keys()
        .any(|field| !allowed_fields.contains(&field.as_str()))
    {
        return None;
    }
    let content = data
        .get("content")
        .or_else(|| data.get("text"))
        .and_then(serde_json::Value::as_str)?;
    let first_string = |keys: &[&str]| {
        keys.iter()
            .filter_map(|key| data.get(*key))
            .find_map(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    Some(SendMessage {
        content: content.to_owned(),
        recipient: first_string(&["recipient", "conversationId", "conversation_id"])
            .unwrap_or_else(|| intent.conversation_id.clone()),
        subject: first_string(&["subject"]),
        thread_ts: intent.thread_id.clone().or_else(|| {
            intent
                .conversation_ref
                .as_ref()
                .and_then(|conversation| conversation.thread_id.clone())
        }),
        topic_id: intent
            .conversation_ref
            .as_ref()
            .and_then(|conversation| conversation.topic_id.clone())
            .or_else(|| first_string(&["topicId", "topic_id"])),
        reply_to_id: intent
            .reply_to_id
            .clone()
            .or_else(|| first_string(&["replyToId", "reply_to_id"])),
        idempotency_key: (!intent.idempotency_key.is_empty())
            .then(|| intent.idempotency_key.clone()),
    })
}

async fn configured_capabilities(
    running: &Arc<Mutex<HashMap<String, Arc<Running>>>>,
    name: &str,
) -> tinychannels_bus::ChannelCapabilities {
    let running = running.lock().await;
    running
        .get(name)
        .or_else(|| running.get(tinychannels_bus::provider_id(name)))
        .map(crate::interaction::configured_adapter_capabilities)
        .unwrap_or_else(tinychannels_bus::ChannelCapabilities::unknown)
}

/// The served object.
pub(crate) struct Channels {
    pub(crate) host: HostChannels,
    email: EmailSender,
    media_uploads: Arc<crate::media::MediaUploads>,
    media_reads: Arc<crate::media::MediaReads>,
    progressive: Arc<crate::progressive::Replies>,
    web_turns: Mutex<crate::web_output::WebTurnRegistry>,
    /// Shared with each listener task so it can retire its own entry.
    pub(crate) running: Arc<Mutex<HashMap<String, Arc<Running>>>>,
    provider_activations: Mutex<HashMap<String, ProviderActivation>>,
    relays: Arc<Relays>,
    pub(crate) terminal: AtomicBool,
    runtimes: Option<Arc<crate::runtime::Runtimes>>,
    shutdown_task: Mutex<Option<JoinHandle<BusResult<()>>>>,
    delivery_admission: Arc<Mutex<()>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProviderActivation {
    generation: String,
    media_account_scope: Option<String>,
}

struct ModuleDeliveryHandler {
    running: Arc<Mutex<HashMap<String, Arc<Running>>>>,
    store: HostDeliveryStore,
}

impl Channels {
    pub(crate) fn new(host: HostChannels, runtimes: Option<Arc<crate::runtime::Runtimes>>) -> Self {
        let running = Arc::new(Mutex::new(HashMap::new()));
        Self {
            progressive: Arc::new(crate::progressive::Replies::new(
                host.clone(),
                running.clone(),
            )),
            web_turns: Mutex::new(crate::web_output::WebTurnRegistry::default()),
            relays: Arc::new(Relays::new(host.clone())),
            host,
            email: EmailSender::new(),
            media_uploads: Arc::new(crate::media::MediaUploads::default()),
            media_reads: Arc::new(crate::media::MediaReads::default()),
            running,
            provider_activations: Mutex::new(HashMap::new()),
            terminal: AtomicBool::new(false),
            runtimes,
            shutdown_task: Mutex::new(None),
            delivery_admission: Arc::new(Mutex::new(())),
        }
    }

    #[cfg(feature = "test-support")]
    async fn register_test_remote_channel(&self, name: String) -> BusResult<()> {
        let mut running = self.running.lock().await;
        if running.contains_key(&name) {
            return Err(BusError::failed("test channel is already registered"));
        }
        let channel = Arc::new(TestRemoteChannel { name: name.clone() });
        let provider =
            Running::start_outbound(channel, Arc::new(Hooks::default()), self.host.clone());
        running.insert(name, provider);
        Ok(())
    }

    #[cfg(feature = "test-support")]
    async fn unregister_test_remote_channel(&self, name: String) -> BusResult<()> {
        let provider = self.running.lock().await.remove(&name);
        if let Some(provider) = provider {
            provider.stop().await?;
        }
        Ok(())
    }

    async fn media_activation(&self, instance: &str) -> BusResult<ProviderActivation> {
        let running = self.running.lock().await;
        if running
            .get(instance)
            .is_none_or(|provider| provider.is_finished())
        {
            return Err(BusError::failed("media provider instance is not running"));
        }
        self.provider_activations
            .lock()
            .await
            .get(instance)
            .cloned()
            .ok_or_else(|| BusError::failed("media provider activation is unavailable"))
    }

    async fn start_channel_inner(
        &self,
        name: String,
        config: ChannelsConfig,
        http: &dyn HttpClientFactory,
        supervision: Option<tinychannels_bus::ChannelSupervisionPolicy>,
        outbound_only: bool,
    ) -> BusResult<()> {
        let mut running = self.running.lock().await;
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        if running
            .get(&name)
            .is_some_and(|running| running.is_finished())
        {
            running.remove(&name);
        }
        if running.contains_key(&name) {
            return Err(BusError::MethodFailed {
                name: ALREADY_RUNNING_ERROR.to_owned(),
                message: format!("channel {name} is already running"),
            });
        }

        let media_account_scope =
            crate::media_account::configured_media_account_scope(&name, &config);
        let hooks = Arc::new(Hooks::default());
        let owner_host: Arc<dyn ChannelHost> =
            hooks.host(self.host.clone(), name.clone(), media_account_scope.clone());
        let built = build_channels(&config, &owner_host, http);
        let channel = built
            .into_iter()
            .find(|candidate| candidate.name() == name)
            .ok_or_else(|| BusError::MethodFailed {
                name: NOT_CONFIGURED_ERROR.to_owned(),
                message: format!("channel {name} is not present in the supplied config"),
            })?;

        if is_webhook_backed(&name, &config) && !outbound_only {
            return Err(BusError::MethodFailed {
                name: NO_INGRESS_ERROR.to_owned(),
                message: format!(
                    "channel {name} receives inbound traffic through an external webhook, \
                     and this module has no ingress for it; run this provider in-process \
                     or serve its webhook host-side"
                ),
            });
        }

        if outbound_only && !is_webhook_backed(&name, &config) {
            return Err(BusError::failed(
                "outbound-only startup requires an external-webhook provider",
            ));
        }
        let provider = match supervision {
            Some(policy) => {
                Running::start_built_supervised(channel, hooks, self.host.clone(), policy)
            }
            None if outbound_only => {
                Running::start_built_outbound(channel, hooks, self.host.clone())
            }
            None => Running::start_built(channel, hooks, self.host.clone()),
        };
        running.insert(name.clone(), provider);
        self.provider_activations.lock().await.insert(
            name.clone(),
            ProviderActivation {
                generation: uuid::Uuid::new_v4().simple().to_string(),
                media_account_scope,
            },
        );
        drop(running);
        finish_start_admission(
            &self.host,
            &self.running,
            &self.delivery_admission,
            &self.terminal,
            &name,
        )
        .await
    }
}

#[cfg(feature = "test-support")]
#[allow(
    deprecated,
    reason = "test-only provider exercises the legacy listener seam"
)]
struct TestRemoteChannel {
    name: String,
}

#[cfg(feature = "test-support")]
#[allow(
    deprecated,
    reason = "test-only provider exercises the legacy listener seam"
)]
#[async_trait::async_trait]
impl tinychannels::Channel for TestRemoteChannel {
    fn name(&self) -> &str {
        &self.name
    }

    async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
        Ok(())
    }

    async fn listen(
        &self,
        _tx: tokio::sync::mpsc::Sender<tinychannels::ChannelMessage>,
    ) -> anyhow::Result<()> {
        std::future::pending().await
    }

    fn supports_remote_control(&self) -> bool {
        true
    }
}

#[async_trait::async_trait]
impl DeliveryQueueHandler for ModuleDeliveryHandler {
    async fn deliver(&self, entry: &QueuedDelivery) -> DeliveryAttemptResult {
        let Ok(intent_fingerprint) = intent_fingerprint(&entry.intent) else {
            return Err(DeliveryAttemptFailure {
                error: "delivery payload could not be encoded".into(),
                sent_before_error: false,
                permanent: Some(true),
                receipt: None,
            });
        };
        let running = self.running.lock().await;
        let Some(provider) = running
            .get(&entry.intent.channel_id)
            .filter(|provider| !provider.is_finished())
            .cloned()
        else {
            return Err(DeliveryAttemptFailure {
                error: "delivery target is unavailable".into(),
                sent_before_error: false,
                permanent: None,
                receipt: None,
            });
        };
        drop(running);

        let message = tinychannels_bus::ChannelOutboundMessage::from_intent(&entry.intent);
        let outcome = match provider.send_typed(message).await {
            Ok(outcome) => outcome,
            Err(error) => {
                return Err(DeliveryAttemptFailure {
                    error: "provider send failed".into(),
                    sent_before_error: matches!(&error, RunningSendError::AfterDispatch(_)),
                    permanent: matches!(&error, RunningSendError::ValidationFailed(_))
                        .then_some(true),
                    receipt: None,
                });
            }
        };
        if let Some(error) = outcome.error {
            return Err(DeliveryAttemptFailure {
                error: error.message,
                sent_before_error: error.sent_before_error
                    || !outcome.receipt.platform_message_ids.is_empty(),
                permanent: Some(!error.retryable),
                receipt: Some(outcome.receipt),
            });
        }
        let result = tinychannels_bus::DeliveryTerminalResult {
            id: entry.id.clone(),
            intent_fingerprint,
            completed_at_unix_ms: now_unix_ms(),
            receipt: Some(outcome.receipt.clone()),
        };
        if self.store.save_result(result).await.is_err() {
            return Err(DeliveryAttemptFailure {
                error: "terminal delivery result could not be persisted".into(),
                sent_before_error: true,
                permanent: None,
                receipt: Some(outcome.receipt),
            });
        }
        Ok(DeliveryAttemptSuccess {
            receipt: Some(outcome.receipt),
        })
    }

    async fn reconcile_unknown_send(
        &self,
        entry: &QueuedDelivery,
    ) -> Result<Option<UnknownSendReconciliation>, DeliveryQueueError> {
        let Some(result) = self.store.load_result(&entry.id).await? else {
            return Ok(None);
        };
        let fingerprint = intent_fingerprint(&entry.intent).map_err(|_| {
            DeliveryQueueError::Store("delivery intent could not be encoded".into())
        })?;
        if result.intent_fingerprint != fingerprint {
            // A definite verdict, not a store failure: quarantine this entry
            // in the failed collection instead of failing every drain.
            tracing::warn!("terminal delivery result conflicts with pending intent; quarantining");
            return Ok(Some(UnknownSendReconciliation::Unresolved {
                retryable: false,
                error: Some("terminal delivery result conflicts with pending intent".into()),
            }));
        }
        Ok(Some(UnknownSendReconciliation::Sent {
            receipt: Box::new(result.receipt.unwrap_or_default()),
            message_id: None,
        }))
    }
}

#[allow(
    clippy::unused_async,
    reason = "tinybus::interface requires every method to be `async fn`"
)]
#[tinybus::interface(name = "ai.tinyhumans.tinychannels.Channels")]
impl Channels {
    /// Register a network-free remote-control channel for host integration tests.
    #[tinybus(confidential)]
    async fn test_register_remote_channel(&self, name: String) -> BusResult<()> {
        #[cfg(feature = "test-support")]
        {
            self.register_test_remote_channel(name).await
        }
        #[cfg(not(feature = "test-support"))]
        {
            let _ = name;
            Err(BusError::failed("TinyChannels test support is disabled"))
        }
    }

    /// Remove a network-free remote-control channel registered by a test.
    #[tinybus(confidential)]
    async fn test_unregister_remote_channel(&self, name: String) -> BusResult<()> {
        #[cfg(feature = "test-support")]
        {
            self.unregister_test_remote_channel(name).await
        }
        #[cfg(not(feature = "test-support"))]
        {
            let _ = name;
            Err(BusError::failed("TinyChannels test support is disabled"))
        }
    }

    /// Reserve an inert identity-scoped runtime and immutable callback paths.
    #[tinybus(confidential)]
    async fn reserve_runtime(&self) -> BusResult<tinychannels_bus::ChannelRuntimeReservation> {
        self.runtimes
            .as_ref()
            .ok_or_else(|| BusError::failed("nested channel runtimes are unavailable"))?
            .reserve()
            .await
    }

    /// Activate a reserved runtime after its callback object is registered.
    /// The returned object's provider methods retain their existing arities.
    #[tinybus(confidential)]
    async fn start_runtime(&self, handle: String) -> BusResult<()> {
        self.runtimes
            .as_ref()
            .ok_or_else(|| BusError::failed("nested channel runtimes are unavailable"))?
            .start(handle)
            .await
    }

    /// Activate an immutable identity runtime with module-owned turn admission.
    #[tinybus(confidential)]
    async fn start_runtime_with_dispatch(
        &self,
        handle: String,
        policy: tinychannels_bus::ChannelRuntimeDispatchPolicy,
    ) -> BusResult<()> {
        self.runtimes
            .as_ref()
            .ok_or_else(|| BusError::failed("nested channel runtimes are unavailable"))?
            .start_with_dispatch(handle, policy)
            .await
    }

    /// Stop this identity's providers, relay and callbacks, joining pending startup.
    #[tinybus(confidential)]
    async fn stop_runtime(&self, handle: String) -> BusResult<()> {
        self.runtimes
            .as_ref()
            .ok_or_else(|| BusError::failed("nested channel runtimes are unavailable"))?
            .stop(handle)
            .await
    }

    /// Connect `name` from `config` and start receiving.
    async fn start_channel(&self, name: String, config: ChannelsConfig) -> BusResult<()> {
        self.start_channel_inner(name, config, &DefaultHttpClients, None, false)
            .await
    }

    /// Connect one provider with host-selected HTTP proxy configuration.
    ///
    /// This additive operation is confidential because proxy URLs may include
    /// credentials. `StartChannel` retains its original two-argument wire.
    #[tinybus(confidential)]
    async fn start_channel_configured(
        &self,
        name: String,
        config: ChannelsConfig,
        proxy: HttpProxyPolicy,
    ) -> BusResult<()> {
        let http = ConfiguredHttpClients::new(proxy)
            .map_err(|_| BusError::failed("invalid host HTTP proxy policy"))?;
        self.start_channel_inner(name, config, &http, None, false)
            .await
    }

    /// Start a provider with confidential configuration and module-owned reconnects.
    /// Legacy start operations keep their original arguments and single-listener behavior.
    #[tinybus(confidential)]
    async fn start_channel_supervised(
        &self,
        name: String,
        config: ChannelsConfig,
        proxy: HttpProxyPolicy,
        policy: tinychannels_bus::ChannelSupervisionPolicy,
    ) -> BusResult<()> {
        if policy.initial_backoff_secs > 86_400 || policy.max_backoff_secs > 86_400 {
            return Err(BusError::failed("provider retry delay exceeds one day"));
        }
        let http = ConfiguredHttpClients::new(proxy)
            .map_err(|_| BusError::failed("invalid host HTTP proxy policy"))?;
        self.start_channel_inner(name, config, &http, Some(policy), false)
            .await
    }

    /// Resolve instance-specific facts without starting native listeners or webhook ingress.
    #[tinybus(confidential)]
    async fn describe_configured_channels(
        &self,
        config: ChannelsConfig,
        proxy: HttpProxyPolicy,
    ) -> BusResult<Vec<tinychannels_bus::module_runtime::ConfiguredChannelInfo>> {
        use tinychannels_bus::module_runtime::{ChannelIngressKind, ConfiguredChannelInfo};
        if self.terminal.load(Ordering::Acquire) {
            return Err(BusError::failed("channel module is shutting down"));
        }
        let http = ConfiguredHttpClients::new(proxy)
            .map_err(|_| BusError::failed("invalid host HTTP proxy policy"))?;
        let host: Arc<dyn ChannelHost> = Arc::new(tinychannels::NoopHost);
        Ok(build_channels(&config, &host, &http)
            .into_iter()
            .map(|channel| ConfiguredChannelInfo {
                name: channel.name(),
                ingress: if is_webhook_backed(&channel.name(), &config) {
                    ChannelIngressKind::ExternalWebhook
                } else {
                    ChannelIngressKind::Listener
                },
                interaction: tinychannels_bus::ChannelInteractionInfoV2 {
                    capabilities: {
                        let mut capabilities = channel.capabilities();
                        capabilities.account.configured = Some(true);
                        capabilities.account.enabled = Some(true);
                        capabilities.account.proactive_target = channel.proactive_target();
                        capabilities
                    },
                },
            })
            .collect())
    }

    /// Keep webhook-backed providers sendable without claiming to serve their ingress.
    #[tinybus(confidential)]
    async fn start_outbound_channel(
        &self,
        name: String,
        config: ChannelsConfig,
        proxy: HttpProxyPolicy,
    ) -> BusResult<()> {
        let http = ConfiguredHttpClients::new(proxy)
            .map_err(|_| BusError::failed("invalid host HTTP proxy policy"))?;
        self.start_channel_inner(name, config, &http, None, true)
            .await
    }

    /// Prepare channel facts without executing host-owned policy.
    #[tinybus(confidential)]
    async fn prepare_channel_control(
        &self,
        request: tinychannels_bus::module_control::ChannelControlRequest,
    ) -> BusResult<tinychannels_bus::module_control::ChannelControlResult> {
        let _admission = self.delivery_admission.lock().await;
        if self.terminal.load(Ordering::Acquire) {
            return Err(BusError::failed("channel module is shutting down"));
        }
        let name = match &request {
            tinychannels_bus::module_control::ChannelControlRequest::Capabilities { channel }
            | tinychannels_bus::module_control::ChannelControlRequest::CapabilitiesV2 { channel } => {
                channel.as_str()
            }
            tinychannels_bus::module_control::ChannelControlRequest::Route { message, .. } => {
                message.channel.as_str()
            }
            _ => "",
        };
        let capabilities = match &request {
            tinychannels_bus::module_control::ChannelControlRequest::Route {
                capabilities: Some(capabilities),
                ..
            } => capabilities.as_ref().clone(),
            _ => configured_capabilities(&self.running, name).await,
        };
        crate::control::prepare(&self.host, request, capabilities).await
    }

    /// Parse and execute remote chat commands inside the channel module.
    #[tinybus(confidential)]
    async fn execute_remote(
        &self,
        request: tinychannels_bus::module_remote::RemoteOperation,
    ) -> BusResult<tinychannels_bus::module_remote::RemoteOperationResult> {
        let _admission = self.delivery_admission.lock().await;
        if self.terminal.load(Ordering::Acquire) {
            return Err(BusError::failed("channel module is shutting down"));
        }
        let name = match &request {
            tinychannels_bus::module_remote::RemoteOperation::Execute { message, .. } => {
                message.channel.as_str()
            }
            tinychannels_bus::module_remote::RemoteOperation::MarkTurn { channel, .. } => {
                channel.as_str()
            }
        };
        let capabilities = configured_capabilities(&self.running, name).await;
        crate::remote::execute(&self.host, request, capabilities).await
    }

    /// Drive progressive delivery without exposing its state to the host.
    #[tinybus(confidential)]
    async fn execute_progressive(
        &self,
        request: tinychannels_bus::module_progressive::ProgressiveCommand,
    ) -> BusResult<tinychannels_bus::module_progressive::ProgressiveResult> {
        if self.terminal.load(Ordering::Acquire) {
            return Err(BusError::failed("channel module is shutting down"));
        }
        self.progressive.execute(request).await
    }

    /// Begin one bounded, module-owned web turn projection.
    #[tinybus(confidential)]
    async fn begin_web_turn(
        &self,
        request: tinychannels_bus::BeginWebTurn,
    ) -> BusResult<tinychannels_bus::WebTurnHandle> {
        let mut web_turns = self.web_turns.lock().await;
        if self.terminal.load(Ordering::Acquire) {
            return Err(BusError::failed("channel module is shutting down"));
        }
        web_turns.begin(request)
    }

    /// Project one canonical provider output through the active web turn.
    #[tinybus(confidential)]
    async fn project_web_output(
        &self,
        request: tinychannels_bus::ProjectWebOutput,
    ) -> BusResult<tinychannels_bus::WebOutputEvents> {
        let mut web_turns = self.web_turns.lock().await;
        if self.terminal.load(Ordering::Acquire) {
            return Err(BusError::failed("channel module is shutting down"));
        }
        web_turns.project(request)
    }

    /// End one web turn and release its bounded projection state.
    #[tinybus(confidential)]
    async fn end_web_turn(&self, request: tinychannels_bus::EndWebTurn) -> BusResult<()> {
        self.web_turns.lock().await.end(request)
    }

    /// Parse runtime switching and render help from host-resolved catalog facts.
    #[tinybus(confidential)]
    async fn prepare_runtime_command(
        &self,
        request: tinychannels_bus::module_routes::PrepareRuntimeCommandRequest,
    ) -> BusResult<Option<tinychannels_bus::module_routes::PreparedRuntimeCommand>> {
        use tinychannels::routes::{self, ChannelRuntimeCommand};
        use tinychannels_bus::module_routes::PreparedRuntimeCommand;
        if self.terminal.load(Ordering::Acquire) {
            return Err(BusError::failed("channel module is shutting down"));
        }
        if serde_json::to_vec(&request)
            .map_err(|_| BusError::failed("invalid runtime command preparation"))?
            .len()
            > tinychannels_bus::module_runtime::PREPARATION_MESSAGE_BYTES
        {
            return Err(BusError::failed(
                "runtime command preparation exceeds its limit",
            ));
        }
        Ok(
            routes::parse_runtime_command(&request.channel, &request.content).map(|command| {
                match command {
                    ChannelRuntimeCommand::SetProvider(requested) => {
                        PreparedRuntimeCommand::SetProvider {
                            provider: routes::resolve_provider_alias(
                                &requested,
                                &request.providers,
                            ),
                            requested,
                        }
                    }
                    ChannelRuntimeCommand::SetModel(model) => {
                        PreparedRuntimeCommand::SetModel(model)
                    }
                    ChannelRuntimeCommand::ShowProviders => PreparedRuntimeCommand::ShowProviders(
                        routes::build_providers_help_response(&request.current, &request.providers),
                    ),
                    ChannelRuntimeCommand::ShowModel => PreparedRuntimeCommand::ShowModel(
                        routes::build_models_help_response_from_catalog(
                            &request.current,
                            &request.model_catalog,
                        ),
                    ),
                }
            }),
        )
    }

    /// Preserve provider data and compute persisted identities inside the module.
    #[tinybus(confidential)]
    async fn prepare_channel_inbound(
        &self,
        request: tinychannels_bus::module_inbound::PrepareChannelInboundRequest,
    ) -> BusResult<tinychannels_bus::module_inbound::PreparedChannelInbound> {
        use tinychannels_bus::module_inbound::{ChannelInboundSource, PreparedChannelInbound};
        if self.terminal.load(Ordering::Acquire) {
            return Err(BusError::failed("channel module is shutting down"));
        }
        if request.namespace.len() > 256
            || serde_json::to_vec(&request)
                .map_err(|_| BusError::failed("invalid inbound preparation"))?
                .len()
                > tinychannels_bus::module_runtime::PREPARATION_MESSAGE_BYTES
        {
            return Err(BusError::failed("inbound preparation exceeds its limit"));
        }
        let (message, envelope) = match request.source {
            ChannelInboundSource::Legacy { message, envelope } => {
                let envelope = envelope.unwrap_or_else(|| {
                    tinychannels::inbound_envelope_from_legacy_message(&message)
                });
                if envelope.channel.id != message.channel
                    || envelope.message_id != message.id
                    || envelope.sender.id != message.sender
                    || envelope.conversation.id != message.reply_target
                {
                    return Err(BusError::failed(
                        "inbound envelope identity does not match message",
                    ));
                }
                let message = match request.preparation_mode {
                    tinychannels_bus::module_inbound::ChannelInboundPreparationMode::LegacyText => {
                        *message
                    }
                    tinychannels_bus::module_inbound::ChannelInboundPreparationMode::Typed => {
                        tinychannels::typed_message_from_inbound_envelope(
                            &envelope,
                            message.timestamp,
                        )
                    }
                };
                (message, envelope)
            }
            ChannelInboundSource::Envelope {
                envelope,
                timestamp,
            } => (
                match request.preparation_mode {
                    tinychannels_bus::module_inbound::ChannelInboundPreparationMode::LegacyText => {
                        tinychannels::legacy_message_from_inbound_envelope(&envelope, timestamp)
                            .map_err(|error| BusError::failed(error.to_string()))?
                    }
                    tinychannels_bus::module_inbound::ChannelInboundPreparationMode::Typed => {
                        tinychannels::typed_message_from_inbound_envelope(&envelope, timestamp)
                    }
                },
                envelope,
            ),
        };
        let prepared = PreparedChannelInbound {
            history_key: tinychannels::context::conversation_history_key(&message),
            history_key_ignores_thread:
                tinychannels::context::legacy_v21_history_key_ignores_thread(&message.channel),
            thread_id: tinychannels::channel::derive_inbound_thread_id(
                &message.channel,
                Some(&message.sender),
                Some(&message.reply_target),
                message.thread_ts.as_deref(),
            ),
            client_id: tinychannels::channel::derive_inbound_client_id(
                &message.channel,
                Some(&message.sender),
            ),
            session_key: tinychannels::channel::build_session_key_for_inbound_envelope(
                &request.namespace,
                &envelope,
                request.session_policy,
            ),
            message,
            envelope,
        };
        if serde_json::to_vec(&prepared)
            .map_err(|_| BusError::failed("invalid inbound preparation"))?
            .len()
            > 2 * tinychannels_bus::module_runtime::PREPARATION_MESSAGE_BYTES
        {
            return Err(BusError::failed(
                "prepared inbound result exceeds its limit",
            ));
        }
        Ok(prepared)
    }

    /// Hosted ingress retains its optional-sender legacy identity behavior.
    #[tinybus(confidential)]
    async fn prepare_channel_identity(
        &self,
        request: tinychannels_bus::module_inbound::PrepareChannelIdentityRequest,
    ) -> BusResult<tinychannels_bus::module_inbound::PreparedChannelIdentity> {
        if self.terminal.load(Ordering::Acquire) {
            return Err(BusError::failed("channel module is shutting down"));
        }
        if serde_json::to_vec(&request)
            .map_err(|_| BusError::failed("invalid inbound identity"))?
            .len()
            > 4096
        {
            return Err(BusError::failed("inbound identity exceeds its limit"));
        }
        Ok(tinychannels_bus::module_inbound::PreparedChannelIdentity {
            thread_id: tinychannels::channel::derive_inbound_thread_id(
                &request.channel,
                request.sender.as_deref(),
                request.reply_target.as_deref(),
                request.thread_ts.as_deref(),
            ),
            client_id: tinychannels::channel::derive_inbound_client_id(
                &request.channel,
                request.sender.as_deref(),
            ),
        })
    }

    /// Preserve the existing legacy and typed message conversion and hashing.
    #[tinybus(confidential)]
    async fn prepare_outbound_intent(
        &self,
        channel: String,
        request: tinychannels_bus::module_runtime::ChannelMessagePreparation,
    ) -> BusResult<tinychannels_bus::module_runtime::PreparedChannelIntent> {
        use tinychannels_bus::module_runtime::{ChannelMessagePreparation, PreparedChannelIntent};
        if self.terminal.load(Ordering::Acquire) {
            return Err(BusError::failed("channel module is shutting down"));
        }
        if channel.is_empty()
            || channel.len() > 256
            || serde_json::to_vec(&request)
                .map_err(|_| BusError::failed("invalid channel message"))?
                .len()
                > tinychannels_bus::module_runtime::PREPARATION_MESSAGE_BYTES
        {
            return Err(BusError::failed("channel message exceeds its limit"));
        }
        let intent = match request {
            ChannelMessagePreparation::Legacy(message) => {
                tinychannels::outbound_intent_from_legacy_message(channel, message)
            }
            ChannelMessagePreparation::Typed(message) => {
                tinychannels::outbound_intent_from_send_message(channel, &message)
            }
            ChannelMessagePreparation::Intent(intent) => {
                if intent.channel_id != channel {
                    return Err(BusError::failed(
                        "intent destination does not match channel",
                    ));
                }
                *intent
            }
        };
        Ok(PreparedChannelIntent {
            legacy_message: tinychannels::legacy_message_value_from_outbound_intent(&intent),
            intent,
        })
    }

    /// Check configured providers using their native health methods without
    /// starting listen loops. Credentials and proxy policy require attestation.
    #[tinybus(confidential)]
    async fn check_channel_health(
        &self,
        config: ChannelsConfig,
        proxy: HttpProxyPolicy,
        timeout_ms: u64,
    ) -> BusResult<Vec<tinychannels_bus::ChannelHealthReport>> {
        if self.terminal.load(Ordering::Acquire) {
            return Err(BusError::failed("channel module is shutting down"));
        }
        let budget = crate::health::health_budget(timeout_ms)?;
        let http = ConfiguredHttpClients::new(proxy)
            .map_err(|_| BusError::failed("invalid host HTTP proxy policy"))?;
        let host: Arc<dyn ChannelHost> = Arc::new(tinychannels::NoopHost);
        let providers = build_channels(&config, &host, &http);
        Ok(crate::health::check_providers(&providers, budget).await)
    }

    /// Dispatch one typed operation through TinyChannels' ChannelManager.
    ///
    /// The request can contain channel credentials and therefore requires a
    /// confidential host-to-module call. TinyChannels owns validation and
    /// provider-native operations; its callback port is limited to host-owned
    /// configuration persistence and authenticated managed API calls.
    #[tinybus(confidential)]
    async fn channel_controller(
        &self,
        request: ChannelControllerRequest,
    ) -> BusResult<ChannelControllerResult> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        crate::controller::dispatch(&self.host, request).await
    }

    /// Disconnect `name` and drop its tasks.
    ///
    /// Stopping a channel that is not running is **not** an error: a host
    /// reconciling desired against actual state should be able to call this
    /// unconditionally.
    async fn stop_channel(&self, name: String) -> BusResult<()> {
        let entry = self.running.lock().await.get(&name).cloned();
        if let Some(entry) = entry {
            let _admission = self.delivery_admission.lock().await;
            if let Err(error) = recover_delivery_channel(&self.host, &self.running, &name).await
                && !matches!(error, DeliveryQueueError::Unsupported)
            {
                return Err(delivery_store_error(error));
            }
            entry.stop().await?;
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(10), self.host.flush())
                    .await
                    .map_err(|_| BusError::failed("callbacks are pending; retry StopChannel"))?;
            let mut running = self.running.lock().await;
            if running
                .get(&name)
                .is_some_and(|current| Arc::ptr_eq(current, &entry))
            {
                running.remove(&name);
            }
            self.provider_activations.lock().await.remove(&name);
            self.media_reads.invalidate_instance(&name).await;
            self.media_uploads.invalidate_instance(&name).await;
            result?;
        }
        Ok(())
    }

    /// Send one outbound message through a running provider.
    async fn send_message(&self, name: String, message: SendMessage) -> BusResult<()> {
        // Cloned out of the map so the send does not hold the registry lock for
        // the duration of a network round trip.
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        let entry = {
            let running = self.running.lock().await;
            running
                .get(&name)
                .filter(|entry| !entry.is_finished())
                .cloned()
                .ok_or_else(|| BusError::MethodFailed {
                    name: UNKNOWN_CHANNEL_ERROR.to_owned(),
                    message: format!("channel {name} is not running"),
                })?
        };

        entry
            .send(message)
            .await
            .map_err(|error| BusError::MethodFailed {
                name: SEND_FAILED_ERROR.to_owned(),
                message: error.to_string(),
            })
    }

    /// Run provider interactions in the same bounded actor queue as sends.
    /// Recipients and draft contents cross the bus confidentially.
    #[tinybus(confidential)]
    async fn channel_interaction(
        &self,
        request: tinychannels_bus::ChannelInteractionRequest,
    ) -> BusResult<tinychannels_bus::ChannelInteractionResult> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        let entry = self
            .running
            .lock()
            .await
            .get(&request.channel)
            .filter(|entry| !entry.is_finished())
            .cloned()
            .ok_or_else(|| BusError::MethodFailed {
                name: UNKNOWN_CHANNEL_ERROR.to_owned(),
                message: "requested channel is not running".into(),
            })?;
        entry
            .interact(request.interaction)
            .await
            .map_err(|_| BusError::MethodFailed {
                name: SEND_FAILED_ERROR.to_owned(),
                message: "provider interaction failed".into(),
            })
    }

    /// Send one intent through the provider actor or host-backed durable queue.
    /// Required durability fails closed when queue storage is unavailable.
    async fn send_channel_intent(
        &self,
        name: String,
        intent: ChannelOutboundIntent,
    ) -> BusResult<ChannelSendMessageResult> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        if intent.channel_id != name {
            return Err(BusError::failed("intent channel does not match provider"));
        }
        if tinychannels_bus::ChannelOutboundMessage::from_intent(&intent)
            .parts
            .is_empty()
        {
            return Err(BusError::failed(
                "outbound intent must contain at least one content part",
            ));
        }
        if intent.durability == tinychannels::DeliveryDurability::Required {
            return self.send_required_intent(intent).await;
        }
        if serde_json::to_vec(&intent).map_or(true, |bytes| {
            bytes.len() > tinychannels_bus::module_relay::RELAY_FRAME_BYTES
        }) {
            return Err(BusError::failed("channel intent byte limit exceeded"));
        }
        let entry = {
            let running = self.running.lock().await;
            running
                .get(&name)
                .filter(|entry| !entry.is_finished())
                .cloned()
                .ok_or_else(|| BusError::MethodFailed {
                    name: UNKNOWN_CHANNEL_ERROR.to_owned(),
                    message: format!("channel {name} is not running"),
                })?
        };
        if let Some(message) = legacy_text_message(&intent) {
            entry
                .send(message)
                .await
                .map_err(|error| BusError::MethodFailed {
                    name: SEND_FAILED_ERROR.to_owned(),
                    message: error.to_string(),
                })?;
            return Ok(ChannelSendMessageResult {
                message_id: None,
                receipt: None,
                error: None,
                raw: None,
            });
        }
        let outcome = entry
            .send_typed(tinychannels_bus::ChannelOutboundMessage::from_intent(
                &intent,
            ))
            .await
            .map_err(|error| BusError::MethodFailed {
                name: SEND_FAILED_ERROR.to_owned(),
                message: error.to_string(),
            })?;
        let has_provider_receipt = outcome.receipt.primary_platform_message_id.is_some()
            || !outcome.receipt.platform_message_ids.is_empty()
            || !outcome.receipt.parts.is_empty();
        Ok(ChannelSendMessageResult {
            message_id: outcome.receipt.primary_platform_message_id.clone(),
            receipt: has_provider_receipt.then_some(outcome.receipt),
            error: outcome.error,
            raw: None,
        })
    }

    async fn send_required_intent(
        &self,
        intent: ChannelOutboundIntent,
    ) -> BusResult<ChannelSendMessageResult> {
        if intent.idempotency_key.is_empty() || intent.idempotency_key.len() > 256 {
            return Err(BusError::failed(
                "required delivery idempotency key is invalid",
            ));
        }
        if intent.channel_id.is_empty() || intent.channel_id != intent.channel_id.trim() {
            return Err(BusError::failed("required delivery channel is invalid"));
        }
        if serde_json::to_vec(&intent).map_or(true, |bytes| {
            bytes.len() > tinychannels_bus::module_delivery::DELIVERY_RECORD_BYTES
        }) {
            return Err(BusError::failed(
                "required delivery exceeds the record limit",
            ));
        }
        let fingerprint = intent_fingerprint(&intent)
            .map_err(|_| BusError::failed("required delivery could not be encoded"))?;
        let _admission = self
            .delivery_admission
            .try_lock()
            .map_err(|_| BusError::failed("required delivery queue is busy"))?;
        let store = HostDeliveryStore::from_host(&self.host);
        let id = intent.idempotency_key.clone();

        if let Some(result) = store.load_result(&id).await.map_err(delivery_store_error)? {
            if result.intent_fingerprint != fingerprint {
                return Err(BusError::failed("required delivery key conflicts"));
            }
            if store
                .load_pending(&id)
                .await
                .map_err(delivery_store_error)?
                .is_some()
            {
                ack_delivery(&store, &id)
                    .await
                    .map_err(delivery_store_error)?;
            }
            return Ok(required_delivery_result(result.receipt));
        }
        if let Some(failed) = store.load_failed(&id).await.map_err(delivery_store_error)? {
            return if failed.intent == intent {
                Err(BusError::failed("required delivery is in failed storage"))
            } else {
                Err(BusError::failed("required delivery key conflicts"))
            };
        }

        let pending = store
            .load_pending(&id)
            .await
            .map_err(delivery_store_error)?;
        if let Some(existing) = &pending {
            if existing.intent != intent {
                return Err(BusError::failed("required delivery key conflicts"));
            }
        } else {
            let typed = tinychannels_bus::ChannelOutboundMessage::from_intent(&intent);
            // A live adapter can reject malformed/unsupported content before
            // persistence. An absent or finished provider is transient: keep
            // the intent durable so recovery can retry it when the provider
            // becomes available.
            let provider = self
                .running
                .lock()
                .await
                .get(&intent.channel_id)
                .filter(|provider| !provider.is_finished())
                .cloned();
            if let Some(provider) = provider
                && provider
                    .adapter_handle()
                    .validate_typed_send(&typed)
                    .is_err()
            {
                store
                    .save_failed(QueuedDelivery::new(id.clone(), intent, now_unix_ms()))
                    .await
                    .map_err(delivery_store_error)?;
                return Err(BusError::failed(
                    "provider rejected required delivery before send",
                ));
            }
            enqueue_delivery(
                &store,
                QueuedDelivery::new(id.clone(), intent.clone(), now_unix_ms()),
            )
            .await
            .map_err(delivery_store_error)?;
        }

        let handler = ModuleDeliveryHandler {
            running: self.running.clone(),
            store: store.clone(),
        };
        let recovery =
            drain_selected_pending_deliveries(&store, &handler, now_unix_ms(), |entry| {
                PendingDeliveryDrainDecision {
                    matches: entry.intent.channel_id == intent.channel_id,
                    bypass_backoff: false,
                }
            })
            .await
            .map_err(delivery_store_error)?;
        if let Some(result) = store.load_result(&id).await.map_err(delivery_store_error)? {
            if result.intent_fingerprint != fingerprint {
                return Err(BusError::failed("required delivery key conflicts"));
            }
            if store
                .load_pending(&id)
                .await
                .map_err(delivery_store_error)?
                .is_some()
            {
                ack_delivery(&store, &id)
                    .await
                    .map_err(delivery_store_error)?;
            }
            return Ok(required_delivery_result(result.receipt));
        }
        let still_pending = store
            .load_pending(&id)
            .await
            .map_err(delivery_store_error)?
            .is_some();
        let moved_to_failed = store
            .load_failed(&id)
            .await
            .map_err(delivery_store_error)?
            .is_some();
        if moved_to_failed || still_pending || recovery.failed > 0 || recovery.deferred_backoff > 0
        {
            return Err(BusError::failed(
                "required delivery remains pending or failed",
            ));
        }
        let result = store
            .load_result(&id)
            .await
            .map_err(delivery_store_error)?
            .ok_or_else(|| BusError::failed("required delivery result was not persisted"))?;
        Ok(required_delivery_result(result.receipt))
    }

    /// Send SMTP mail without requiring an active IMAP listener.
    /// The host hydrates credentials and sends this payload confidentially.
    #[tinybus(confidential)]
    async fn send_email(&self, request: EmailSendRequest) -> BusResult<ChannelSendMessageResult> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        self.email.send(request).await
    }

    /// Start a bounded confidential upload for an SMTP message.
    #[tinybus(confidential)]
    async fn begin_email_upload(&self, request: EmailUploadBegin) -> BusResult<EmailUploadHandle> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        self.email.begin_upload(request).await
    }

    /// Append an ordered, retry-safe confidential attachment chunk.
    #[tinybus(confidential)]
    async fn append_email_upload_chunk(&self, chunk: EmailUploadChunk) -> BusResult<u32> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        self.email.append_upload_chunk(chunk).await
    }

    /// Send a completed upload once; retries replay the handle's terminal result.
    #[tinybus(confidential)]
    async fn send_email_upload(
        &self,
        handle: EmailUploadHandle,
    ) -> BusResult<ChannelSendMessageResult> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        self.email.send_upload(handle).await
    }

    /// Discard an upload that has not entered SMTP delivery.
    #[tinybus(confidential)]
    async fn abort_email_upload(&self, handle: EmailUploadHandle) -> BusResult<()> {
        self.email.abort_upload(handle).await
    }

    /// Begin bounded media ingestion for one active configured provider.
    #[tinybus(confidential)]
    async fn begin_media_upload(
        &self,
        request: tinychannels_bus::MediaUploadBegin,
    ) -> BusResult<tinychannels_bus::MediaUploadHandle> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        let activation = self.media_activation(&request.instance).await?;
        if activation.media_account_scope.is_none() {
            return Err(BusError::failed(
                "configured account has no stable media identity",
            ));
        }
        self.media_uploads
            .begin(request, activation.generation)
            .await
    }

    /// Append one ordered, retry-safe media chunk over confidential TinyBus.
    #[tinybus(confidential)]
    async fn append_media_upload_chunk(
        &self,
        chunk: tinychannels_bus::MediaUploadChunk,
    ) -> BusResult<u32> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        let (instance, activation) = self.media_uploads.binding_handle(&chunk.handle).await?;
        if self.media_activation(&instance).await?.generation != activation {
            return Err(BusError::failed(
                "media upload belongs to another provider activation",
            ));
        }
        self.media_uploads.append(chunk).await
    }

    /// Persist a complete upload as an opaque scoped media reference.
    #[tinybus(confidential)]
    async fn finish_media_upload(
        &self,
        request: tinychannels_bus::MediaUploadFinish,
    ) -> BusResult<tinychannels_bus::MediaUploadResult> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        let (instance, activation) = self.media_uploads.binding(&request).await?;
        let current = self.media_activation(&instance).await?;
        if current.generation != activation {
            return Err(BusError::failed(
                "media upload belongs to another provider activation",
            ));
        }
        let repository = Arc::new(crate::provider::ModuleMediaRepository(
            self.host.clone(),
            instance,
            current.media_account_scope,
        ));
        self.media_uploads
            .clone()
            .finish(request, activation, repository)
            .await
    }

    /// Abort buffered media without writing a durable record.
    #[tinybus(confidential)]
    async fn abort_media_upload(
        &self,
        request: tinychannels_bus::MediaUploadFinish,
    ) -> BusResult<()> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        let (instance, activation) = self.media_uploads.binding(&request).await?;
        if self.media_activation(&instance).await?.generation != activation {
            return Err(BusError::failed(
                "media upload belongs to another provider activation",
            ));
        }
        self.media_uploads.abort(request).await
    }

    /// Resolve one scoped opaque reference into a short-lived chunk session.
    #[tinybus(confidential)]
    async fn begin_media_read(
        &self,
        request: tinychannels_bus::MediaReadBegin,
    ) -> BusResult<tinychannels_bus::MediaReadInfo> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        let activation = self.media_activation(&request.instance).await?;
        let Some(account_scope) = activation.media_account_scope.as_ref() else {
            return Err(BusError::failed(
                "configured account has no stable media identity",
            ));
        };
        if request.scope.conversation.id.trim().is_empty()
            || request.scope.turn_id.trim().is_empty()
            || request.media.opaque_id.as_deref().is_none_or(str::is_empty)
            || request.max_bytes == 0
            || request.max_bytes > tinychannels_bus::module_media::MEDIA_UPLOAD_MAX_BYTES as u64
        {
            return Err(BusError::failed("media resolution request is invalid"));
        }
        let encoded = serde_json::to_vec(&request)
            .map_err(|_| BusError::failed("media resolution request is invalid"))?;
        if encoded.len() > tinychannels_bus::module_media::MEDIA_UPLOAD_BEGIN_MAX_BYTES {
            return Err(BusError::failed(
                "media resolution request exceeds its limit",
            ));
        }
        let instance = request.instance.clone();
        let repository = crate::provider::ModuleMediaRepository(
            self.host.clone(),
            instance.clone(),
            Some(account_scope.clone()),
        );
        let media_id = request
            .media
            .opaque_id
            .clone()
            .ok_or_else(|| BusError::failed("media resolution request is invalid"))?;
        let mut scope = request.scope.clone();
        scope.configured_instance = instance.clone();
        let max_bytes = request.max_bytes;
        self.media_reads
            .resolve(
                request,
                activation.generation.clone(),
                || async move {
                    <crate::provider::ModuleMediaRepository as tinychannels::host::MediaRepository>::read_media(
                        &repository,
                        scope,
                        media_id,
                        max_bytes,
                    )
                    .await
                    .map_err(|_| BusError::failed("media could not be resolved"))
                },
                || async {
                    !self.terminal.load(Ordering::SeqCst)
                        && matches!(
                            self.media_activation(&instance).await,
                            Ok(current) if current == activation
                        )
                },
            )
            .await
    }

    /// Return one bounded chunk over confidential TinyBus method-reply framing.
    #[tinybus(confidential)]
    async fn read_media_chunk(
        &self,
        request: tinychannels_bus::MediaReadChunk,
    ) -> BusResult<tinychannels_bus::MediaReadChunkResult> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        let (instance, activation) = self.media_reads.binding(&request.handle).await?;
        if self.media_activation(&instance).await?.generation != activation {
            return Err(BusError::failed("media read belongs to another activation"));
        }
        self.media_reads.chunk(request).await
    }

    /// Release a resolved media read and its temporary byte buffer.
    #[tinybus(confidential)]
    async fn abort_media_read(&self, request: tinychannels_bus::MediaReadFinish) -> BusResult<()> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(BusError::failed("module is shutting down"));
        }
        let (instance, activation) = self.media_reads.binding(&request.0).await?;
        if self.media_activation(&instance).await?.generation != activation {
            return Err(BusError::failed("media read belongs to another activation"));
        }
        self.media_reads.abort(request).await
    }

    /// The providers currently running, in no particular order.
    async fn list_channels(&self) -> BusResult<Vec<String>> {
        Ok(self
            .running
            .lock()
            .await
            .iter()
            .filter(|(_, entry)| !entry.is_finished())
            .map(|(name, _)| name.clone())
            .collect())
    }

    /// Whether `name` is running, and whether the provider reports itself healthy.
    async fn channel_status(&self, name: String) -> BusResult<String> {
        let adapter = {
            let running = self.running.lock().await;
            running
                .get(&name)
                .filter(|entry| !entry.is_finished())
                .map(|entry| entry.adapter_handle().clone())
        };
        let Some(adapter) = adapter else {
            return Ok("stopped".to_owned());
        };
        let status = adapter
            .status()
            .await
            .map_err(|error| BusError::failed(error.message))?;
        Ok(if status.health_state.as_deref() != Some("unhealthy") {
            "connected".to_owned()
        } else {
            "unhealthy".to_owned()
        })
    }
    /// Mint a bounded, single-use relay reservation before async native startup.
    async fn reserve_relay(&self) -> BusResult<RelayReservation> {
        self.relays.reserve().await
    }
    /// Start this known handle with host-hydrated secrets over confidential delivery.
    #[tinybus(confidential)]
    async fn start_relay(
        &self,
        id: RelayReservation,
        config: RelayRuntimeConfig,
    ) -> BusResult<tinychannels::relay::CapabilityDescriptor> {
        self.relays.start(id, config).await
    }
    /// Start a relay through the canonical v2 per-identity capability handshake.
    #[tinybus(confidential)]
    async fn start_relay_v2(
        &self,
        id: RelayReservation,
        config: RelayRuntimeConfig,
    ) -> BusResult<tinychannels_bus::module_relay::RelayCapabilitiesV2> {
        self.relays.start_v2(id, config).await
    }
    /// Cancel pending startup and join relay/reconnect/native callback work.
    async fn stop_relay(&self, id: RelayReservation) -> BusResult<()> {
        self.relays.stop(id).await
    }
    /// Report safe lifecycle facts without URL, identities or credentials.
    async fn relay_status(&self, id: RelayReservation) -> BusResult<RelayStatus> {
        self.relays.status(id).await
    }
    /// Return identity-correlated v2 capabilities negotiated for this reservation.
    #[tinybus(confidential)]
    async fn relay_capabilities_v2(
        &self,
        id: RelayReservation,
    ) -> BusResult<Option<tinychannels_bus::module_relay::RelayCapabilitiesV2>> {
        self.relays.capabilities_v2(id).await
    }
    /// Enqueue one bounded, ordered relay intent.
    async fn send_relay_intent(
        &self,
        id: RelayReservation,
        intent: ChannelOutboundIntent,
    ) -> BusResult<ChannelSendMessageResult> {
        self.relays.send(id, intent).await
    }
    /// Freeze operations and join all native providers, relay and callbacks.
    pub(crate) async fn shutdown(&self) -> BusResult<()> {
        self.terminal.store(true, Ordering::SeqCst);
        self.web_turns.lock().await.clear();
        self.relays.freeze();
        // Fence and stop identity-scoped children before draining the legacy
        // service resources, but do not let one failed child skip cleanup of
        // root providers, relay, email, delivery state, or host callbacks.
        // Runtimes retains failed handles, so a later Shutdown retries them.
        let runtime_failure = if let Some(runtimes) = &self.runtimes {
            runtimes.shutdown().await.err()
        } else {
            None
        };
        let mut task = self.shutdown_task.lock().await;
        if task.is_none() {
            let progressive = self.progressive.clone();
            let providers = self.running.clone();
            let relay = self.relays.clone();
            let email = self.email.clone();
            let host = self.host.clone();
            let delivery_admission = self.delivery_admission.clone();
            *task = Some(tokio::spawn(async move {
                let progressive_result = progressive.shutdown().await;
                let progressive_pending = progressive_result.is_err();
                let mut failure = runtime_failure.or_else(|| progressive_result.err());
                let _delivery = delivery_admission.lock().await;
                let delivery_store = HostDeliveryStore::from_host(&host);
                let delivery_handler = ModuleDeliveryHandler {
                    running: providers.clone(),
                    store: delivery_store.clone(),
                };
                match recover_pending_deliveries(&delivery_store, &delivery_handler, now_unix_ms())
                    .await
                {
                    Err(DeliveryQueueError::Unsupported) => {}
                    Err(error) => failure = Some(delivery_store_error(error)),
                    Ok(_) => {}
                }
                drop(_delivery);
                let entries: Vec<_> = providers
                    .lock()
                    .await
                    .iter()
                    .map(|(name, entry)| (name.clone(), entry.clone()))
                    .collect();
                let mut pending = false;
                for (name, entry) in entries {
                    match entry.stop().await {
                        Ok(()) => {
                            providers.lock().await.remove(&name);
                        }
                        Err(error) => {
                            pending = true;
                            if failure.is_none() {
                                failure = Some(error);
                            }
                        }
                    }
                }
                if let Err(error) = relay.shutdown().await
                    && failure.is_none()
                {
                    failure = Some(error);
                }
                if let Err(error) = email.shutdown().await
                    && failure.is_none()
                {
                    failure = Some(error);
                }
                if !pending && !progressive_pending && relay.is_empty().await {
                    match host.shutdown().await {
                        Err(error) if failure.is_none() => failure = Some(error),
                        _ => {}
                    }
                }
                match failure {
                    Some(error) => Err(error),
                    None => Ok(()),
                }
            }));
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            task.as_mut()
                .ok_or_else(|| BusError::failed("missing shutdown owner"))?,
        )
        .await
        .map_err(|_| BusError::failed("module cleanup is pending; retry Shutdown"))?
        .map_err(|_| BusError::failed("module cleanup worker failed"))?;
        task.take();
        result
    }
}

async fn recover_delivery_channel(
    host: &HostChannels,
    running: &Arc<Mutex<HashMap<String, Arc<Running>>>>,
    channel_id: &str,
) -> Result<(), DeliveryQueueError> {
    let store = HostDeliveryStore::from_host(host);
    let handler = ModuleDeliveryHandler {
        running: running.clone(),
        store: store.clone(),
    };
    drain_selected_pending_deliveries(&store, &handler, now_unix_ms(), |entry| {
        PendingDeliveryDrainDecision {
            matches: entry.intent.channel_id == channel_id,
            bypass_backoff: false,
        }
    })
    .await?;
    Ok(())
}

async fn finish_start_admission(
    host: &HostChannels,
    running: &Arc<Mutex<HashMap<String, Arc<Running>>>>,
    delivery_admission: &Mutex<()>,
    terminal: &AtomicBool,
    name: &str,
) -> BusResult<()> {
    let _admission = delivery_admission.lock().await;
    if terminal.load(Ordering::SeqCst) {
        let entry = running.lock().await.get(name).cloned();
        if let Some(entry) = entry {
            entry.stop().await?;
            let mut running = running.lock().await;
            if running
                .get(name)
                .is_some_and(|current| Arc::ptr_eq(current, &entry))
            {
                running.remove(name);
            }
        }
        return Err(BusError::failed("module is shutting down"));
    }
    if let Err(error) = recover_delivery_channel(host, running, name).await
        && !matches!(error, DeliveryQueueError::Unsupported)
    {
        let entry = running.lock().await.get(name).cloned();
        if let Some(entry) = entry
            && entry.stop().await.is_ok()
        {
            let mut running = running.lock().await;
            if running
                .get(name)
                .is_some_and(|current| Arc::ptr_eq(current, &entry))
            {
                running.remove(name);
            }
        }
        return Err(delivery_store_error(error));
    }
    Ok(())
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StorageSetupOptions {
    storage_callback: Option<BusName>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ModuleSetupConfig {
    Null(()),
    Options(StorageSetupOptions),
}

impl ModuleSetupConfig {
    fn storage_callback(self) -> BusResult<Option<BusName>> {
        match self {
            Self::Null(()) => Ok(None),
            Self::Options(options) => match options.storage_callback {
                Some(destination) if !destination.is_unique() => Err(BusError::failed(
                    "storage callback destination must be unique",
                )),
                destination => Ok(destination),
            },
        }
    }
}

async fn setup_with_storage_callback(
    connection: Connection,
    storage_callback: Option<BusName>,
) -> BusResult<()> {
    // Storage callbacks identify the module by its well-known name.
    connection.request_name(BUS_NAME).await?;
    let host =
        HostChannels::new_with_storage_callback(connection.clone(), storage_callback.clone());
    // Storage belongs to the host's invocation scope, which may not exist at
    // module boot. HostChannels resolves this exact callback lazily and still
    // refuses storage operations if it is unavailable, without a fallback.
    let channels = Channels::new(
        host.clone(),
        Some(Arc::new(
            crate::runtime::Runtimes::new_with_storage_callback(
                connection.clone(),
                storage_callback,
            ),
        )),
    );
    let result = async {
        connection
            .serve_at(OBJECT_PATH.try_into()?, channels)
            .await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = host.shutdown().await;
    }
    result
}

async fn setup_with_context(
    context: tinybus_module::ModuleSetupContext,
    config: ModuleSetupConfig,
) -> BusResult<()> {
    setup_with_storage_callback(context.connection().clone(), config.storage_callback()?).await
}

fn intent_fingerprint(intent: &ChannelOutboundIntent) -> Result<String, serde_json::Error> {
    use sha2::Digest as _;
    let encoded = serde_json::to_vec(intent)?;
    Ok(hex::encode(sha2::Sha256::digest(encoded)))
}

fn required_delivery_result(
    receipt: Option<tinychannels_bus::channel::MessageReceipt>,
) -> ChannelSendMessageResult {
    let receipt = receipt.unwrap_or_default();
    ChannelSendMessageResult {
        message_id: receipt.primary_platform_message_id.clone(),
        receipt: Some(receipt),
        error: None,
        raw: None,
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn delivery_store_error(error: DeliveryQueueError) -> BusError {
    match error {
        DeliveryQueueError::Unsupported => {
            BusError::failed("required delivery host queue is unavailable")
        }
        DeliveryQueueError::Store(_) => {
            BusError::failed("required delivery persistence or recovery failed")
        }
    }
}

// Isolate the generated public C symbols so the lint exception cannot hide
// undocumented Rust API. Their contract is TinyBus ABI v1.
#[allow(
    missing_docs,
    unreachable_pub,
    reason = "generated C ABI symbols are documented by the TinyBus module SDK"
)]
pub(crate) mod exports {
    macro_rules! export_channels_module {
        ($($extra_method:literal),* $(,)?) => {
            tinybus_module::module_export_optional_static! {
        setup_with_context = super::setup_with_context,
        config = super::ModuleSetupConfig,
        worker_threads = 4,
        provides = [
            "ai.tinyhumans.tinychannels.Channels",
        ],
        methods = [
            "StartChannel",
            "StopChannel",
            "SendMessage",
            "ListChannels",
            "ChannelStatus",
            "ReserveRelay", "StartRelay", "StartRelayV2", "StopRelay", "RelayStatus", "RelayCapabilitiesV2", "SendRelayIntent", "Shutdown", "SendChannelIntent", "SendEmail",
            "BeginEmailUpload", "AppendEmailUploadChunk", "SendEmailUpload", "AbortEmailUpload",
            "StartChannelConfigured",
            "ChannelController",
            "ChannelInteraction",
            "CheckChannelHealth",
            "StartChannelSupervised",
            "ReserveRuntime", "StartRuntime", "StopRuntime",
            "DescribeConfiguredChannels", "StartOutboundChannel", "PrepareOutboundIntent",
            "PrepareChannelInbound", "PrepareChannelIdentity", "ExecuteProgressive", "ExecuteRemote", "PrepareChannelControl", "StartRuntimeWithDispatch", "PrepareRuntimeCommand",
            "BeginMediaUpload", "AppendMediaUploadChunk", "FinishMediaUpload", "AbortMediaUpload",
            "BeginMediaRead", "ReadMediaChunk", "AbortMediaRead",
            "BeginWebTurn", "ProjectWebOutput", "EndWebTurn",
            $($extra_method),*
        ],
        signals = [],
        // The host callback object is resolved per call, not required at load:
        // an outbound-only host that serves nothing is supported.
        requires = [],
        optional = ["ai.tinyhumans.tinychannels.ChannelsHost"],
        lazy = false,
    }
        };
    }

    #[cfg(feature = "test-support")]
    export_channels_module!("TestRegisterRemoteChannel", "TestUnregisterRemoteChannel");

    #[cfg(not(feature = "test-support"))]
    export_channels_module!();
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
