//! Relay frame transport loop.

use crate::relay::{
    AuthenticatedRelayInboundEvent, CapabilityDescriptor, ConnectorToGatewayFrame,
    GatewayToConnectorFrame, PassthroughForward,
};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use thiserror::Error;
use tokio::sync::{Mutex, Notify, RwLock, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep, timeout};

const DEFAULT_HANDSHAKE_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_OUTBOUND_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_IDLE_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_RECONNECT_BACKOFF_MS: u64 = 1_000;
const DEFAULT_RECONNECT_MAX_BACKOFF_MS: u64 = 30_000;

/// One platform/bot identity advertised to the relay connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RelayIdentity {
    pub platform: String,
    #[serde(rename = "botId")]
    pub bot_id: String,
}

/// Timeouts for transport operations that wait on connector frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct RelayTransportTimeouts {
    pub handshake_ms: u64,
    pub outbound_ms: u64,
    pub idle_ms: u64,
}

impl Default for RelayTransportTimeouts {
    fn default() -> Self {
        Self {
            handshake_ms: DEFAULT_HANDSHAKE_TIMEOUT_MS,
            outbound_ms: DEFAULT_OUTBOUND_TIMEOUT_MS,
            idle_ms: DEFAULT_IDLE_TIMEOUT_MS,
        }
    }
}

/// Reconnect backoff settings for relay runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct RelayReconnectPolicy {
    pub backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl Default for RelayReconnectPolicy {
    fn default() -> Self {
        Self {
            backoff_ms: DEFAULT_RECONNECT_BACKOFF_MS,
            max_backoff_ms: DEFAULT_RECONNECT_MAX_BACKOFF_MS,
        }
    }
}

/// Errors surfaced by the relay transport loop.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RelayTransportError {
    #[error("relay transport is closed")]
    Closed,
    #[error("relay transport timed out waiting for {operation}")]
    Timeout { operation: &'static str },
    #[error("relay transport io error: {0}")]
    Io(String),
    #[error("relay transport handler error: {0}")]
    Handler(String),
}

/// Minimal frame I/O boundary used by the transport loop.
#[async_trait]
pub trait RelayFrameIo: Send + Sync {
    async fn send(&self, frame: GatewayToConnectorFrame) -> Result<(), RelayTransportError>;
    async fn recv(&self) -> Result<Option<ConnectorToGatewayFrame>, RelayTransportError>;
}

/// Dialer used by reconnect supervisors to acquire a fresh frame I/O.
#[async_trait]
pub trait RelayFrameDialer: Send + Sync {
    async fn dial(&self) -> Result<Arc<dyn RelayFrameIo>, RelayTransportError>;
}

#[async_trait]
impl<T> RelayFrameIo for Arc<T>
where
    T: RelayFrameIo + ?Sized,
{
    async fn send(&self, frame: GatewayToConnectorFrame) -> Result<(), RelayTransportError> {
        (**self).send(frame).await
    }

    async fn recv(&self) -> Result<Option<ConnectorToGatewayFrame>, RelayTransportError> {
        (**self).recv().await
    }
}

/// Handler for authenticated connector-to-gateway inbound events.
#[async_trait]
pub trait RelayInboundHandler: Send + Sync {
    async fn handle(
        &self,
        event: AuthenticatedRelayInboundEvent,
    ) -> Result<(), RelayTransportError>;
}

/// Handler for connector-forwarded passthrough requests.
#[async_trait]
pub trait RelayPassthroughHandler: Send + Sync {
    async fn handle(
        &self,
        forward: PassthroughForward,
        buffer_id: Option<String>,
    ) -> Result<(), RelayTransportError>;
}

/// Handler for connector-to-gateway interrupt requests.
#[async_trait]
pub trait RelayInterruptInboundHandler: Send + Sync {
    async fn handle(&self, session_key: String, chat_id: String)
    -> Result<(), RelayTransportError>;
}

/// Hermes-compatible relay transport loop over a frame I/O implementation.
pub struct RelayTransport {
    identities: Vec<RelayIdentity>,
    io: RwLock<Arc<dyn RelayFrameIo>>,
    timeouts: RelayTransportTimeouts,
    state: Arc<RelayTransportState>,
}

impl RelayTransport {
    pub fn new(
        identities: Vec<RelayIdentity>,
        io: Arc<dyn RelayFrameIo>,
        timeouts: RelayTransportTimeouts,
    ) -> Self {
        Self {
            identities,
            io: RwLock::new(io),
            timeouts,
            state: Arc::new(RelayTransportState::default()),
        }
    }

    pub async fn connect(&self) -> Result<(), RelayTransportError> {
        self.prepare_connect().await;
        self.start_reader().await;
        self.send_hellos().await
    }

    pub async fn reconnect_with_io(
        &self,
        io: Arc<dyn RelayFrameIo>,
    ) -> Result<(), RelayTransportError> {
        *self.io.write().await = io;
        self.connect().await
    }

    pub fn spawn_reconnect_supervisor(
        self: &Arc<Self>,
        dialer: Arc<dyn RelayFrameDialer>,
        policy: RelayReconnectPolicy,
    ) -> RelayReconnectHandle {
        let shutdown = Arc::new(AtomicBool::new(false));
        let task_shutdown = shutdown.clone();
        let transport = self.clone();
        let task = tokio::spawn(async move {
            transport
                .reconnect_loop(dialer, policy, task_shutdown)
                .await;
        });
        RelayReconnectHandle { shutdown, task }
    }

    async fn prepare_connect(&self) {
        self.state.closed.store(false, Ordering::SeqCst);
        *self.state.descriptor.lock().await = None;
    }

    async fn send_hellos(&self) -> Result<(), RelayTransportError> {
        let io = self.current_io().await;
        for identity in &self.identities {
            io.send(GatewayToConnectorFrame::Hello {
                platform: identity.platform.clone(),
                bot_id: identity.bot_id.clone(),
            })
            .await?;
        }
        Ok(())
    }

    async fn current_io(&self) -> Arc<dyn RelayFrameIo> {
        self.io.read().await.clone()
    }

    pub async fn handshake(&self) -> Result<CapabilityDescriptor, RelayTransportError> {
        loop {
            if let Some(descriptor) = self.state.descriptor.lock().await.clone() {
                return Ok(descriptor);
            }
            if self.state.closed.load(Ordering::SeqCst) {
                return Err(RelayTransportError::Closed);
            }
            timeout(
                Duration::from_millis(self.timeouts.handshake_ms),
                self.state.descriptor_ready.notified(),
            )
            .await
            .map_err(|_| RelayTransportError::Timeout {
                operation: "handshake",
            })?;
        }
    }

    pub async fn send_outbound(
        &self,
        action: Value,
        platform: Option<&str>,
    ) -> Result<Value, RelayTransportError> {
        self.request_response(action, platform).await
    }

    pub async fn send_follow_up(
        &self,
        action: Value,
        platform: Option<&str>,
    ) -> Result<Value, RelayTransportError> {
        self.request_response(action, platform).await
    }

    pub async fn get_chat_info(&self, chat_id: &str) -> Result<Value, RelayTransportError> {
        let result = self
            .request_response(json!({"op": "get_chat_info", "chat_id": chat_id}), None)
            .await?;
        let info = result.get("chat_info").unwrap_or(&result);
        Ok(json!({
            "name": info.get("name").and_then(Value::as_str).unwrap_or(chat_id),
            "type": info.get("type").and_then(Value::as_str).unwrap_or("dm"),
        }))
    }

    pub async fn send_interrupt(
        &self,
        session_key: impl Into<String>,
        reason: Option<String>,
    ) -> Result<(), RelayTransportError> {
        self.current_io()
            .await
            .send(GatewayToConnectorFrame::Interrupt {
                session_key: session_key.into(),
                reason,
            })
            .await
    }

    pub async fn go_idle(&self) -> Result<bool, RelayTransportError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut waiter = self.state.going_idle.lock().await;
            *waiter = Some(tx);
        }
        if let Err(error) = self
            .current_io()
            .await
            .send(GatewayToConnectorFrame::GoingIdle)
            .await
        {
            self.state.going_idle.lock().await.take();
            return Err(error);
        }
        match timeout(Duration::from_millis(self.timeouts.idle_ms), rx).await {
            Ok(Ok(())) => Ok(true),
            Ok(Err(_)) => Err(RelayTransportError::Closed),
            Err(_) => {
                self.state.going_idle.lock().await.take();
                Ok(false)
            }
        }
    }

    pub async fn set_inbound_handler(&self, handler: Arc<dyn RelayInboundHandler>) {
        *self.state.inbound_handler.write().await = Some(handler);
    }

    pub async fn set_passthrough_handler(&self, handler: Arc<dyn RelayPassthroughHandler>) {
        *self.state.passthrough_handler.write().await = Some(handler);
    }

    pub async fn set_interrupt_inbound_handler(
        &self,
        handler: Arc<dyn RelayInterruptInboundHandler>,
    ) {
        *self.state.interrupt_handler.write().await = Some(handler);
    }

    async fn start_reader(&self) {
        if self
            .state
            .reader_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let io = self.current_io().await;
        let state = self.state.clone();
        tokio::spawn(async move {
            let result = read_loop(io.clone(), state.clone()).await;
            if let Err(error) = result {
                state.fail_waiters(error).await;
            }
        });
    }

    async fn reconnect_loop(
        &self,
        dialer: Arc<dyn RelayFrameDialer>,
        policy: RelayReconnectPolicy,
        shutdown: Arc<AtomicBool>,
    ) {
        let mut backoff_ms = policy.backoff_ms;
        loop {
            self.wait_closed().await;
            if shutdown.load(Ordering::SeqCst) {
                return;
            }
            sleep(Duration::from_millis(backoff_ms)).await;
            if shutdown.load(Ordering::SeqCst) {
                return;
            }
            match dialer.dial().await {
                Ok(io) => match self.reconnect_with_io(io).await {
                    Ok(()) => {
                        backoff_ms = policy.backoff_ms;
                    }
                    Err(_) => {
                        backoff_ms = next_backoff(backoff_ms, policy.max_backoff_ms);
                    }
                },
                Err(_) => {
                    backoff_ms = next_backoff(backoff_ms, policy.max_backoff_ms);
                }
            }
        }
    }

    async fn wait_closed(&self) {
        loop {
            if self.state.closed.load(Ordering::SeqCst) {
                return;
            }
            self.state.closed_ready.notified().await;
        }
    }

    async fn request_response(
        &self,
        action: Value,
        platform: Option<&str>,
    ) -> Result<Value, RelayTransportError> {
        let request_id = self.state.next_request_id();
        let (tx, rx) = oneshot::channel();
        self.state
            .pending
            .lock()
            .await
            .insert(request_id.clone(), tx);

        let (platform, bot_id) = platform
            .map(|platform| {
                (
                    Some(platform.to_string()),
                    self.bot_id_for(platform).map(str::to_string),
                )
            })
            .unwrap_or((None, None));

        let send_result = self
            .current_io()
            .await
            .send(GatewayToConnectorFrame::Outbound {
                request_id: request_id.clone(),
                action,
                platform,
                bot_id,
            })
            .await;
        if let Err(error) = send_result {
            self.state.pending.lock().await.remove(&request_id);
            return Err(error);
        }

        match timeout(Duration::from_millis(self.timeouts.outbound_ms), rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err(RelayTransportError::Closed),
            Err(_) => {
                self.state.pending.lock().await.remove(&request_id);
                Err(RelayTransportError::Timeout {
                    operation: "outbound_result",
                })
            }
        }
    }

    fn bot_id_for(&self, platform: &str) -> Option<&str> {
        self.identities
            .iter()
            .find(|identity| identity.platform == platform)
            .map(|identity| identity.bot_id.as_str())
    }
}

/// Handle for a spawned reconnect supervisor.
pub struct RelayReconnectHandle {
    shutdown: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl RelayReconnectHandle {
    pub fn abort(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.task.abort();
    }
}

#[derive(Default)]
struct RelayTransportState {
    reader_started: AtomicBool,
    closed: AtomicBool,
    request_counter: AtomicU64,
    descriptor: Mutex<Option<CapabilityDescriptor>>,
    descriptor_ready: Notify,
    closed_ready: Notify,
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    going_idle: Mutex<Option<oneshot::Sender<()>>>,
    inbound_handler: RwLock<Option<Arc<dyn RelayInboundHandler>>>,
    passthrough_handler: RwLock<Option<Arc<dyn RelayPassthroughHandler>>>,
    interrupt_handler: RwLock<Option<Arc<dyn RelayInterruptInboundHandler>>>,
}

impl RelayTransportState {
    fn next_request_id(&self) -> String {
        let id = self.request_counter.fetch_add(1, Ordering::SeqCst) + 1;
        format!("req-{id}")
    }

    async fn fail_waiters(&self, _error: RelayTransportError) {
        self.closed.store(true, Ordering::SeqCst);
        self.reader_started.store(false, Ordering::SeqCst);
        self.pending.lock().await.clear();
        self.going_idle.lock().await.take();
        self.descriptor_ready.notify_waiters();
        self.closed_ready.notify_waiters();
    }
}

fn next_backoff(current_ms: u64, max_ms: u64) -> u64 {
    current_ms.saturating_mul(2).min(max_ms)
}

async fn read_loop(
    io: Arc<dyn RelayFrameIo>,
    state: Arc<RelayTransportState>,
) -> Result<(), RelayTransportError> {
    loop {
        let Some(frame) = io.recv().await? else {
            return Err(RelayTransportError::Closed);
        };
        handle_frame(&io, &state, frame).await?;
    }
}

async fn handle_frame(
    io: &Arc<dyn RelayFrameIo>,
    state: &Arc<RelayTransportState>,
    frame: ConnectorToGatewayFrame,
) -> Result<(), RelayTransportError> {
    match frame {
        ConnectorToGatewayFrame::Descriptor { descriptor } => {
            *state.descriptor.lock().await = Some(descriptor);
            state.descriptor_ready.notify_waiters();
        }
        ConnectorToGatewayFrame::Inbound { event, buffer_id } => {
            let frame = ConnectorToGatewayFrame::Inbound { event, buffer_id };
            let Some(inbound) = frame.authenticated_inbound_event() else {
                return Ok(());
            };
            let ack = frame.inbound_ack();
            let handler = { state.inbound_handler.read().await.clone() };
            if let Some(handler) = handler {
                handler.handle(inbound).await?;
                if let Some(ack) = ack {
                    io.send(ack).await?;
                }
            }
        }
        ConnectorToGatewayFrame::OutboundResult { request_id, result } => {
            if let Some(sender) = state.pending.lock().await.remove(&request_id) {
                let _ = sender.send(result);
            }
        }
        ConnectorToGatewayFrame::InterruptInbound {
            session_key,
            chat_id,
        } => {
            let handler = { state.interrupt_handler.read().await.clone() };
            if let Some(handler) = handler {
                handler.handle(session_key, chat_id).await?;
            }
        }
        ConnectorToGatewayFrame::GoingIdleAck => {
            if let Some(sender) = state.going_idle.lock().await.take() {
                let _ = sender.send(());
            }
        }
        ConnectorToGatewayFrame::PassthroughForward { forward, buffer_id } => {
            let ack = buffer_id
                .as_ref()
                .map(|buffer_id| GatewayToConnectorFrame::InboundAck {
                    buffer_id: buffer_id.clone(),
                });
            let handler = { state.passthrough_handler.read().await.clone() };
            if let Some(handler) = handler {
                handler.handle(forward, buffer_id).await?;
                if let Some(ack) = ack {
                    io.send(ack).await?;
                }
            }
        }
    }
    Ok(())
}
