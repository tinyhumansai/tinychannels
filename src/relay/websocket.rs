//! Feature-gated WebSocket relay dialer.

use crate::relay::{
    DEFAULT_UPGRADE_TTL_SECONDS, GatewayToConnectorFrame, RelayFrameIo, RelayTransportError,
    make_upgrade_token,
};
use async_trait::async_trait;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Request};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type RelayWsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// WebSocket dialer config for a connector relay endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct WebSocketRelayConfig {
    pub url: String,
    pub gateway_id: Option<String>,
    pub upgrade_secret: Option<String>,
    pub upgrade_ttl_seconds: i64,
}

impl From<&crate::config::RelayRuntimeConfig> for WebSocketRelayConfig {
    fn from(config: &crate::config::RelayRuntimeConfig) -> Self {
        Self {
            url: config.url.clone(),
            gateway_id: config.gateway_id.clone(),
            upgrade_secret: config.upgrade_secret.clone(),
            upgrade_ttl_seconds: config.upgrade_ttl_seconds,
        }
    }
}

impl WebSocketRelayConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            upgrade_ttl_seconds: DEFAULT_UPGRADE_TTL_SECONDS,
            ..Default::default()
        }
    }
}

/// Dialer for reconnect supervisors that use WebSocket relay transport.
#[derive(Debug, Clone)]
pub struct WebSocketRelayDialer {
    config: WebSocketRelayConfig,
}

impl WebSocketRelayDialer {
    pub fn new(config: WebSocketRelayConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl crate::relay::RelayFrameDialer for WebSocketRelayDialer {
    async fn dial(&self) -> Result<Arc<dyn RelayFrameIo>, RelayTransportError> {
        Ok(Arc::new(connect_websocket_relay_io(&self.config).await?))
    }
}

/// Normalize connector base URLs to the `/relay` WebSocket upgrade target.
pub fn websocket_dial_url(url: &str) -> String {
    let mut raw = url.trim().to_string();
    if let Some(rest) = raw.strip_prefix("https://") {
        raw = format!("wss://{rest}");
    } else if let Some(rest) = raw.strip_prefix("http://") {
        raw = format!("ws://{rest}");
    }
    raw = raw.trim_end_matches('/').to_string();
    if !raw.ends_with("/relay") {
        raw.push_str("/relay");
    }
    raw
}

pub fn websocket_upgrade_authorization(config: &WebSocketRelayConfig) -> Option<String> {
    let gateway_id = config.gateway_id.as_deref()?;
    let secret = config.upgrade_secret.as_deref()?;
    let token = make_upgrade_token(gateway_id, secret, config.upgrade_ttl_seconds);
    Some(format!("Bearer {token}"))
}

pub async fn connect_websocket_relay_io(
    config: &WebSocketRelayConfig,
) -> Result<WebSocketRelayIo, RelayTransportError> {
    let request = websocket_request(config)?;
    let (stream, _) = connect_async(request)
        .await
        .map_err(|error| RelayTransportError::Io(error.to_string()))?;
    Ok(WebSocketRelayIo::new(stream))
}

fn websocket_request(config: &WebSocketRelayConfig) -> Result<Request<()>, RelayTransportError> {
    let mut request = websocket_dial_url(&config.url)
        .into_client_request()
        .map_err(|error| RelayTransportError::Io(error.to_string()))?;
    if let Some(authorization) = websocket_upgrade_authorization(config) {
        let value = HeaderValue::from_str(&authorization)
            .map_err(|error| RelayTransportError::Io(error.to_string()))?;
        request.headers_mut().insert(AUTHORIZATION, value);
    }
    Ok(request)
}

/// WebSocket-backed relay frame I/O.
pub struct WebSocketRelayIo {
    sink: Mutex<SplitSink<RelayWsStream, Message>>,
    stream: Mutex<SplitStream<RelayWsStream>>,
    read_buffer: Mutex<String>,
}

impl WebSocketRelayIo {
    fn new(stream: RelayWsStream) -> Self {
        let (sink, stream) = stream.split();
        Self {
            sink: Mutex::new(sink),
            stream: Mutex::new(stream),
            read_buffer: Mutex::new(String::new()),
        }
    }
}

#[async_trait]
impl RelayFrameIo for WebSocketRelayIo {
    async fn send(&self, frame: GatewayToConnectorFrame) -> Result<(), RelayTransportError> {
        let json = frame
            .to_json()
            .map_err(|error| RelayTransportError::Io(error.to_string()))?;
        self.sink
            .lock()
            .await
            .send(Message::Text(format!("{json}\n").into()))
            .await
            .map_err(|error| RelayTransportError::Io(error.to_string()))
    }

    async fn recv(
        &self,
    ) -> Result<Option<crate::relay::ConnectorToGatewayFrame>, RelayTransportError> {
        loop {
            if let Some(line) = self.next_buffered_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                return crate::relay::ConnectorToGatewayFrame::from_json(&line)
                    .map(Some)
                    .map_err(|error| RelayTransportError::Io(error.to_string()));
            }

            let message = self.stream.lock().await.next().await;
            match message {
                Some(Ok(Message::Text(text))) => {
                    self.read_buffer.lock().await.push_str(&text);
                }
                Some(Ok(Message::Binary(bytes))) => {
                    let chunk = String::from_utf8_lossy(&bytes);
                    self.read_buffer.lock().await.push_str(&chunk);
                }
                Some(Ok(Message::Close(_))) | None => return Ok(None),
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Frame(_))) => {}
                Some(Err(error)) => return Err(RelayTransportError::Io(error.to_string())),
            }
        }
    }
}

impl WebSocketRelayIo {
    async fn next_buffered_line(&self) -> Option<String> {
        let mut buffer = self.read_buffer.lock().await;
        let newline = buffer.find('\n')?;
        let line = buffer[..newline].to_string();
        buffer.drain(..=newline);
        Some(line)
    }
}
