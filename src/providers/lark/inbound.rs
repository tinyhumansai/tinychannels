//! Lark event intake over the webhook and WebSocket transports.

use super::*;
use tinychannels_bus::{
    ChannelInboundEnvelope, ChannelRef, ConversationKind, ConversationRef, SenderRef,
};

impl LarkChannel {
    pub(super) fn http_client(&self) -> reqwest::Client {
        self.http_client.clone()
    }

    pub(super) fn begin_listener(
        &self,
    ) -> Result<(watch::Receiver<bool>, Uuid), tinychannels_bus::channel::ChannelSendError> {
        let mut active = self
            .listener_cancel
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active.is_some() {
            return Err(tinychannels_bus::channel::ChannelSendError {
                kind: tinychannels_bus::channel::SendErrorKind::Forbidden,
                message: "Lark listener is already running".into(),
                retryable: false,
                ..Default::default()
            });
        }
        let (cancel, receiver) = watch::channel(false);
        let id = Uuid::new_v4();
        *active = Some((id, cancel));
        Ok((receiver, id))
    }

    pub(super) fn end_listener(&self, id: Uuid) {
        let mut active = self
            .listener_cancel
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active
            .as_ref()
            .is_some_and(|(active_id, _)| *active_id == id)
        {
            active.take();
        }
    }

    pub(super) fn cancel_listener(&self) {
        if let Some((_, cancel)) = self
            .listener_cancel
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
        {
            cancel.send_replace(true);
        }
    }

    pub(super) async fn mark_ws_message_seen(&self, message_id: &str) -> bool {
        let now = Instant::now();
        let mut seen = self.ws_seen_ids.write().await;
        seen.retain(|_, received| now.duration_since(*received) < Duration::from_secs(30 * 60));
        if seen.contains_key(message_id) {
            return false;
        }
        seen.insert(message_id.to_owned(), now);
        true
    }

    pub(super) async fn ws_message_was_seen(&self, message_id: &str) -> bool {
        let now = Instant::now();
        let mut seen = self.ws_seen_ids.write().await;
        seen.retain(|_, received| now.duration_since(*received) < Duration::from_secs(30 * 60));
        seen.contains_key(message_id)
    }

    pub(super) fn api_base(&self) -> &'static str {
        if self.use_feishu {
            FEISHU_BASE_URL
        } else {
            LARK_BASE_URL
        }
    }

    pub(super) fn ws_base(&self) -> &'static str {
        if self.use_feishu {
            FEISHU_WS_BASE_URL
        } else {
            LARK_WS_BASE_URL
        }
    }

    pub(super) fn tenant_access_token_url(&self) -> String {
        format!("{}/auth/v3/tenant_access_token/internal", self.api_base())
    }

    pub(super) fn send_message_url(&self) -> String {
        format!("{}/im/v1/messages?receive_id_type=chat_id", self.api_base())
    }

    /// POST /callback/ws/endpoint → (wss_url, client_config)
    pub(super) async fn get_ws_endpoint(&self) -> anyhow::Result<(String, WsClientConfig)> {
        let resp = self
            .http_client()
            .post(format!("{}/callback/ws/endpoint", self.ws_base()))
            .header("locale", if self.use_feishu { "zh" } else { "en" })
            .json(&serde_json::json!({
                "AppID": self.app_id,
                "AppSecret": self.app_secret,
            }))
            .send()
            .await?
            .json::<WsEndpointResp>()
            .await?;
        if resp.code != 0 {
            anyhow::bail!(
                "Lark WS endpoint failed: code={} msg={}",
                resp.code,
                resp.msg.as_deref().unwrap_or("(none)")
            );
        }
        let ep = resp
            .data
            .ok_or_else(|| anyhow::anyhow!("Lark WS endpoint: empty data"))?;
        Ok((ep.url, ep.client_config.unwrap_or_default()))
    }

    /// WS long-connection event loop.  Returns Ok(()) when the connection closes
    /// (the caller reconnects).
    #[allow(clippy::too_many_lines)]
    pub(super) async fn listen_ws(
        &self,
        sink: &(dyn crate::channel::ChannelInboundSink + Send + Sync),
    ) -> anyhow::Result<()> {
        let (wss_url, client_config) = self.get_ws_endpoint().await?;
        let service_id = wss_url
            .split('?')
            .nth(1)
            .and_then(|qs| {
                qs.split('&')
                    .find(|kv| kv.starts_with("service_id="))
                    .and_then(|kv| kv.split('=').nth(1))
                    .and_then(|v| v.parse::<i32>().ok())
            })
            .unwrap_or(0);
        tracing::info!("Lark: connecting to {wss_url}");

        let (ws_stream, _) = tokio_tungstenite::connect_async(&wss_url).await?;
        let (mut write, mut read) = ws_stream.split();
        tracing::info!("Lark: WS connected (service_id={service_id})");

        let mut ping_secs = client_config.ping_interval.unwrap_or(120).max(10);
        let mut hb_interval = tokio::time::interval(Duration::from_secs(ping_secs));
        let mut timeout_check = tokio::time::interval(Duration::from_secs(10));
        hb_interval.tick().await; // consume immediate tick

        let mut seq: u64 = 0;
        let mut last_recv = Instant::now();

        // Send initial ping immediately (like the official SDK) so the server
        // starts responding with pongs and we can calibrate the ping_interval.
        seq = seq.wrapping_add(1);
        let initial_ping = PbFrame {
            seq_id: seq,
            log_id: 0,
            service: service_id,
            method: 0,
            headers: vec![PbHeader {
                key: "type".into(),
                value: "ping".into(),
            }],
            payload: None,
        };
        if write
            .send(WsMsg::Binary(initial_ping.encode_to_vec().into()))
            .await
            .is_err()
        {
            anyhow::bail!("Lark: initial ping failed");
        }
        // message_id → (fragment_slots, created_at) for multi-part reassembly
        type FragEntry = (Vec<Option<Vec<u8>>>, Instant);
        let mut frag_cache: HashMap<String, FragEntry> = HashMap::new();

        loop {
            tokio::select! {
                biased;

                _ = hb_interval.tick() => {
                    seq = seq.wrapping_add(1);
                    let ping = PbFrame {
                        seq_id: seq, log_id: 0, service: service_id, method: 0,
                        headers: vec![PbHeader { key: "type".into(), value: "ping".into() }],
                        payload: None,
                    };
                    if write.send(WsMsg::Binary(ping.encode_to_vec().into())).await.is_err() {
                        tracing::warn!("Lark: ping failed, reconnecting");
                        break;
                    }
                    // GC stale fragments > 5 min
                    let cutoff = Instant::now().checked_sub(Duration::from_secs(300)).unwrap_or(Instant::now());
                    frag_cache.retain(|_, (_, ts)| *ts > cutoff);
                }

                _ = timeout_check.tick() => {
                    if last_recv.elapsed() > WS_HEARTBEAT_TIMEOUT {
                        tracing::warn!("Lark: heartbeat timeout, reconnecting");
                        break;
                    }
                }

                msg = read.next() => {
                    let raw = match msg {
                        Some(Ok(ws_msg)) => {
                            if should_refresh_last_recv(&ws_msg) {
                                last_recv = Instant::now();
                            }
                            match ws_msg {
                                WsMsg::Binary(b) => b,
                                WsMsg::Ping(d) => { let _ = write.send(WsMsg::Pong(d)).await; continue; }
                                WsMsg::Pong(_) => continue,
                                WsMsg::Close(_) => { tracing::info!("Lark: WS closed — reconnecting"); break; }
                                _ => continue,
                            }
                        }
                        None => { tracing::info!("Lark: WS closed — reconnecting"); break; }
                        Some(Err(e)) => { tracing::warn!("Lark: WS read error: {e}"); break; }
                    };

                    let frame = match PbFrame::decode(&raw[..]) {
                        Ok(f) => f,
                        Err(e) => { tracing::error!("Lark: proto decode: {e}"); continue; }
                    };

                    // CONTROL frame
                    if frame.method == 0 {
                        if frame.header_value("type") == "pong"
                            && let Some(p) = &frame.payload
                                && let Ok(cfg) = serde_json::from_slice::<WsClientConfig>(p)
                                    && let Some(secs) = cfg.ping_interval {
                                        let secs = secs.max(10);
                                        if secs != ping_secs {
                                            ping_secs = secs;
                                            hb_interval = tokio::time::interval(Duration::from_secs(ping_secs));
                                            tracing::info!("Lark: ping_interval → {ping_secs}s");
                                        }
                                    }
                        continue;
                    }

                    // DATA frame
                    let msg_type = frame.header_value("type").to_string();
                    let msg_id   = frame.header_value("message_id").to_string();
                    let sum      = frame.header_value("sum").parse::<usize>().unwrap_or(1);
                    let seq_num  = frame.header_value("seq").parse::<usize>().unwrap_or(0);

                    // ACK immediately (Feishu requires within 3 s)
                    {
                        let mut ack = frame.clone();
                        ack.payload = Some(br#"{"code":200,"headers":{},"data":[]}"#.to_vec());
                        ack.headers.push(PbHeader { key: "biz_rt".into(), value: "0".into() });
                        let _ = write.send(WsMsg::Binary(ack.encode_to_vec().into())).await;
                    }

                    // Fragment reassembly
                    let sum = if sum == 0 { 1 } else { sum };
                    let payload: Vec<u8> = if sum == 1 || msg_id.is_empty() || seq_num >= sum {
                        frame.payload.clone().unwrap_or_default()
                    } else {
                        let entry = frag_cache.entry(msg_id.clone())
                            .or_insert_with(|| (vec![None; sum], Instant::now()));
                        if entry.0.len() != sum { *entry = (vec![None; sum], Instant::now()); }
                        entry.0[seq_num] = frame.payload.clone();
                        if entry.0.iter().all(|s| s.is_some()) {
                            let full: Vec<u8> = entry.0.iter()
                                .flat_map(|s| s.as_deref().unwrap_or(&[]))
                                .copied().collect();
                            frag_cache.remove(&msg_id);
                            full
                        } else { continue; }
                    };

                    if msg_type != "event" { continue; }

                    let event: LarkEvent = match serde_json::from_slice(&payload) {
                        Ok(e) => e,
                        Err(e) => { tracing::error!("Lark: event JSON: {e}"); continue; }
                    };
                    if event.header.event_type != "im.message.receive_v1" { continue; }

                    let event_payload = event.event;
                    let message_id = event_payload
                        .pointer("/message/message_id")
                        .and_then(serde_json::Value::as_str)
                        .filter(|value| !value.is_empty());
                    let Some(message_id) = message_id else {
                        tracing::debug!("Lark: event has no message ID");
                        continue;
                    };
                    if !self.is_event_admitted(&event_payload) {
                        continue;
                    }

                    // Check dedup before downloading content. Typed events are
                    // recorded only after publication succeeds so a rejected
                    // sink can receive the provider retry after reconnect.
                    if self.ws_message_was_seen(message_id).await {
                        tracing::debug!("Lark WS: dup {message_id}");
                        continue;
                    }

                    let Some(envelope) = self
                        .inbound_envelope_for_listener(&event_payload, true)
                        .await
                    else {
                        continue;
                    };
                    if sink
                    .push_ingress(tinychannels_bus::ChannelIngress::Inbound(envelope))
                        .await
                        .is_err()
                    {
                        tracing::warn!("Lark WS: typed ingress sink rejected an event");
                        break;
                    }
                    self.mark_ws_message_seen(message_id).await;
                    continue;

                }
            }
        }
        Ok(())
    }

    /// Check if a user open_id is allowed
    pub(super) fn is_user_allowed(&self, open_id: &str) -> bool {
        self.allowed_users.iter().any(|u| u == "*" || u == open_id)
    }

    pub(super) fn is_event_admitted(&self, event: &serde_json::Value) -> bool {
        let sender_id = event
            .pointer("/sender/sender_id/open_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let sender_type = event
            .pointer("/sender/sender_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let chat_type = event
            .pointer("/message/chat_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        !sender_id.is_empty()
            && self.is_user_allowed(sender_id)
            && !matches!(sender_type, "app" | "bot")
            && (chat_type != "group"
                || should_respond_in_group(
                    event
                        .pointer("/message/mentions")
                        .and_then(serde_json::Value::as_array)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                ))
    }

    /// Get or refresh tenant access token
    pub(super) async fn get_tenant_access_token(&self) -> anyhow::Result<String> {
        // Check cache first
        {
            let cached = self.tenant_token.read().await;
            if let Some(ref token) = *cached {
                return Ok(token.clone());
            }
        }

        let url = self.tenant_access_token_url();
        let body = serde_json::json!({
            "app_id": self.app_id,
            "app_secret": self.app_secret,
        });

        let resp = self.http_client().post(&url).json(&body).send().await?;
        let data: serde_json::Value = resp.json().await?;

        let code = data.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = data
                .get("msg")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Lark tenant_access_token failed: {msg}");
        }

        let token = data
            .get("tenant_access_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing tenant_access_token in response"))?
            .to_string();

        // Cache it
        {
            let mut cached = self.tenant_token.write().await;
            *cached = Some(token.clone());
        }

        Ok(token)
    }

    /// Invalidate only the token that received the explicit auth rejection.
    /// Another request may already have refreshed the shared cache.
    pub(super) async fn invalidate_token_if_current(&self, rejected: &str) {
        let mut cached = self.tenant_token.write().await;
        if cached.as_deref() == Some(rejected) {
            *cached = None;
        }
    }

    /// Parse an event callback payload and extract text messages
    pub fn parse_event_payload(&self, payload: &serde_json::Value) -> Vec<ChannelMessage> {
        let mut messages = Vec::new();

        // Lark event v2 structure:
        // { "header": { "event_type": "im.message.receive_v1" }, "event": { "message": { ... }, "sender": { ... } } }
        let event_type = payload
            .pointer("/header/event_type")
            .and_then(|e| e.as_str())
            .unwrap_or("");

        if event_type != "im.message.receive_v1" {
            return messages;
        }

        let event = match payload.get("event") {
            Some(e) => e,
            None => return messages,
        };
        if event
            .pointer("/sender/sender_type")
            .and_then(|kind| kind.as_str())
            .is_some_and(|kind| kind == "app" || kind == "bot")
        {
            return messages;
        }

        // Extract sender open_id
        let open_id = event
            .pointer("/sender/sender_id/open_id")
            .and_then(|s| s.as_str())
            .unwrap_or("");

        if open_id.is_empty() {
            return messages;
        }

        // Check allowlist
        if !self.is_user_allowed(open_id) {
            tracing::warn!("Lark: ignoring message from unauthorized user: {open_id}");
            return messages;
        }
        if event
            .pointer("/message/chat_type")
            .and_then(|kind| kind.as_str())
            == Some("group")
            && !should_respond_in_group(
                event
                    .pointer("/message/mentions")
                    .and_then(serde_json::Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            )
        {
            return messages;
        }

        // Extract message content (text and post supported)
        let msg_type = event
            .pointer("/message/message_type")
            .and_then(|t| t.as_str())
            .unwrap_or("");

        let content_str = event
            .pointer("/message/content")
            .and_then(|c| c.as_str())
            .unwrap_or("");

        let text: String = match msg_type {
            "text" => {
                let extracted = serde_json::from_str::<serde_json::Value>(content_str)
                    .ok()
                    .and_then(|v| {
                        v.get("text")
                            .and_then(|t| t.as_str())
                            .filter(|s| !s.is_empty())
                            .map(String::from)
                    });
                match extracted {
                    Some(t) => t,
                    None => return messages,
                }
            }
            "post" => match parse_post_content(content_str) {
                Some(t) => t,
                None => return messages,
            },
            _ => {
                tracing::debug!("Lark: skipping unsupported message type: {msg_type}");
                return messages;
            }
        };

        let timestamp = event
            .pointer("/message/create_time")
            .and_then(|t| t.as_str())
            .and_then(|t| t.parse::<u64>().ok())
            // Lark timestamps are in milliseconds
            .map(|ms| ms / 1000)
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });

        let chat_id = event
            .pointer("/message/chat_id")
            .and_then(|c| c.as_str())
            .unwrap_or(open_id);

        messages.push(ChannelMessage {
            id: Uuid::new_v4().to_string(),
            sender: chat_id.to_string(),
            reply_target: chat_id.to_string(),
            content: text,
            channel: "lark".to_string(),
            timestamp,
            topic_id: None,
            reply_to_id: None,
            thread_ts: None,
            sender_name: None,
        });

        messages
    }
}

#[derive(Clone)]
pub(super) struct LarkWebhookState {
    pub(super) verification_token: String,
    pub(super) channel: Arc<LarkChannel>,
    pub(super) tx: Option<tokio::sync::mpsc::Sender<ChannelMessage>>,
    pub(super) typed_tx: Option<tokio::sync::mpsc::Sender<TypedWebhookEvent>>,
}

pub(super) async fn handle_lark_webhook(
    axum::extract::State(state): axum::extract::State<LarkWebhookState>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    // Both challenge and event callbacks must prove knowledge of the
    // verification token. A missing token is never authentication.
    let token_ok = !state.verification_token.is_empty()
        && payload
            .get("token")
            .or_else(|| payload.get("header").and_then(|header| header.get("token")))
            .and_then(|token| token.as_str())
            .is_some_and(|token| token == state.verification_token);
    if !token_ok {
        return (StatusCode::FORBIDDEN, "invalid token").into_response();
    }

    if let Some(challenge) = payload.get("challenge").and_then(|c| c.as_str()) {
        let resp = serde_json::json!({ "challenge": challenge });
        return (StatusCode::OK, axum::Json(resp)).into_response();
    }

    if let Some(typed_tx) = &state.typed_tx {
        if payload
            .pointer("/header/event_type")
            .and_then(serde_json::Value::as_str)
            != Some("im.message.receive_v1")
        {
            return (StatusCode::OK, "ok").into_response();
        }
        let Some(event) = payload.get("event") else {
            return (StatusCode::OK, "ok").into_response();
        };
        let (reply, response) = tokio::sync::oneshot::channel();
        if typed_tx
            .send(TypedWebhookEvent {
                event: event.clone(),
                reply,
            })
            .await
            .is_err()
        {
            return (StatusCode::SERVICE_UNAVAILABLE, "listener stopped").into_response();
        }
        return match response.await {
            Ok(TypedWebhookResult::Acknowledged) => (StatusCode::OK, "ok").into_response(),
            Ok(TypedWebhookResult::Retry) | Err(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "listener unavailable").into_response()
            }
        };
    }

    let messages = state.channel.parse_event_payload(&payload);
    if payload
        .pointer("/header/event_type")
        .and_then(serde_json::Value::as_str)
        == Some("im.message.receive_v1")
        && let Some(event) = payload.get("event")
        && let Some(envelope) = state.channel.inbound_envelope(event).await
        && crate::providers::native::inbound::publish(
            "lark",
            state.channel.native.events.as_ref(),
            envelope,
        )
        .await
    {
        return (StatusCode::OK, "ok").into_response();
    }
    for msg in messages {
        let Some(tx) = &state.tx else {
            break;
        };
        if tx.send(msg).await.is_err() {
            tracing::warn!("Lark: message channel closed");
            break;
        }
    }

    (StatusCode::OK, "ok").into_response()
}

impl LarkChannel {
    /// HTTP callback server (legacy — requires a public endpoint).
    /// Use `listen()` (WS long-connection) for new deployments.
    pub async fn listen_http(
        &self,
        tx: tokio::sync::mpsc::Sender<ChannelMessage>,
    ) -> anyhow::Result<()> {
        use axum::{Router, routing::post};

        let port = self.port.ok_or_else(|| {
            anyhow::anyhow!("Lark webhook mode requires `port` to be set in [channels_config.lark]")
        })?;
        if self.verification_token.is_empty() {
            anyhow::bail!("Lark webhook mode requires `verification_token`");
        }

        let state = LarkWebhookState {
            verification_token: self.verification_token.clone(),
            channel: Arc::new(self.clone()),
            tx: Some(tx),
            typed_tx: None,
        };

        let app = Router::new()
            .route("/lark", post(handle_lark_webhook))
            .with_state(state);

        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        tracing::info!("Lark event callback server listening on {addr}");

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }

    pub(super) async fn listen_http_typed(
        &self,
        sink: &(dyn crate::channel::ChannelInboundSink + Send + Sync),
        cancellation: &mut watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        use axum::{Router, routing::post};

        let port = self.port.ok_or_else(|| {
            anyhow::anyhow!("Lark webhook mode requires `port` to be set in [channels_config.lark]")
        })?;
        if self.verification_token.is_empty() {
            anyhow::bail!("Lark webhook mode requires `verification_token`");
        }

        let (typed_tx, mut events) = tokio::sync::mpsc::channel(128);
        let state = LarkWebhookState {
            verification_token: self.verification_token.clone(),
            channel: Arc::new(self.clone()),
            tx: None,
            typed_tx: Some(typed_tx),
        };
        let app = Router::new()
            .route("/lark", post(handle_lark_webhook))
            .with_state(state);
        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let server = axum::serve(listener, app).into_future();
        tokio::pin!(server);

        loop {
            tokio::select! {
                biased;
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow() {
                        return Ok(());
                    }
                }
                result = &mut server => return result.map_err(Into::into),
                Some(request) = events.recv() => {
                    let result = tokio::select! {
                        biased;
                        changed = cancellation.changed() => {
                            let _ = request.reply.send(TypedWebhookResult::Retry);
                            if changed.is_err() || *cancellation.borrow() {
                                return Ok(());
                            }
                            continue;
                        }
                        result = async {
                            let message_id = request
                                .event
                                .pointer("/message/message_id")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_owned();
                            if !self.is_event_admitted(&request.event)
                                || message_id.is_empty()
                                || self.ws_message_was_seen(&message_id).await
                            {
                                TypedWebhookResult::Acknowledged
                            } else if let Some(envelope) = self
                                .inbound_envelope_for_listener(&request.event, true)
                                .await
                            {
                                match sink
                                    .push_ingress(tinychannels_bus::ChannelIngress::Inbound(envelope))
                                    .await
                                {
                                    Ok(()) => {
                                        self.mark_ws_message_seen(&message_id).await;
                                        TypedWebhookResult::Acknowledged
                                    }
                                    Err(error) => {
                                        tracing::debug!(error = %error.message, "Lark typed webhook sink rejected an event");
                                        TypedWebhookResult::Retry
                                    }
                                }
                            } else {
                                TypedWebhookResult::Acknowledged
                            }
                        } => result,
                    };
                    let _ = request.reply.send(result);
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WS helper functions
// ─────────────────────────────────────────────────────────────────────────────

/// Flatten a Feishu `post` rich-text message to plain text.
///
/// Returns `None` when the content cannot be parsed or yields no usable text,
/// so callers can simply `continue` rather than forwarding a meaningless
/// placeholder string to the agent.
pub(super) fn parse_post_content(content: &str) -> Option<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let locale = parsed
        .get("zh_cn")
        .or_else(|| parsed.get("en_us"))
        .or_else(|| {
            parsed
                .as_object()
                .and_then(|m| m.values().find(|v| v.is_object()))
        })?;

    let mut text = String::new();

    if let Some(title) = locale
        .get("title")
        .and_then(|t| t.as_str())
        .filter(|s| !s.is_empty())
    {
        text.push_str(title);
        text.push_str("\n\n");
    }

    if let Some(paragraphs) = locale.get("content").and_then(|c| c.as_array()) {
        for para in paragraphs {
            if let Some(elements) = para.as_array() {
                for el in elements {
                    match el.get("tag").and_then(|t| t.as_str()).unwrap_or("") {
                        "text" => {
                            if let Some(t) = el.get("text").and_then(|t| t.as_str()) {
                                text.push_str(t);
                            }
                        }
                        "a" => {
                            text.push_str(
                                el.get("text")
                                    .and_then(|t| t.as_str())
                                    .filter(|s| !s.is_empty())
                                    .or_else(|| el.get("href").and_then(|h| h.as_str()))
                                    .unwrap_or(""),
                            );
                        }
                        "at" => {
                            let n = el
                                .get("user_name")
                                .and_then(|n| n.as_str())
                                .or_else(|| el.get("user_id").and_then(|i| i.as_str()))
                                .unwrap_or("user");
                            text.push('@');
                            text.push_str(n);
                        }
                        _ => {}
                    }
                }
                text.push('\n');
            }
        }
    }

    let result = text.trim().to_string();
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Remove `@_user_N` placeholder tokens injected by Feishu in group chats.
pub(super) fn strip_at_placeholders(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch == '@' {
            let rest: String = chars.clone().map(|(_, c)| c).collect();
            if let Some(after) = rest.strip_prefix("_user_") {
                let digit_count = after.chars().take_while(|c| c.is_ascii_digit()).count();
                if digit_count == 0 {
                    result.push(ch);
                    continue;
                }
                let skip = "_user_".len() + digit_count;
                for _ in 0..skip {
                    chars.next();
                }
                if chars.peek().map(|(_, c)| *c == ' ').unwrap_or(false) {
                    chars.next();
                }
                continue;
            }
        }
        result.push(ch);
    }
    result
}

/// In group chats, only respond when the bot is explicitly @-mentioned.
pub(super) fn should_respond_in_group(mentions: &[serde_json::Value]) -> bool {
    !mentions.is_empty()
}

impl LarkChannel {
    pub(crate) async fn inbound_envelope(
        &self,
        event: &serde_json::Value,
    ) -> Option<ChannelInboundEnvelope> {
        self.inbound_envelope_with_listener(event, false).await
    }

    pub(crate) async fn inbound_envelope_for_listener(
        &self,
        event: &serde_json::Value,
        typed_sink_available: bool,
    ) -> Option<ChannelInboundEnvelope> {
        self.inbound_envelope_with_listener(event, typed_sink_available)
            .await
    }

    async fn inbound_envelope_with_listener(
        &self,
        event: &serde_json::Value,
        typed_sink_available: bool,
    ) -> Option<ChannelInboundEnvelope> {
        use crate::providers::native::inbound::{
            InboundFile, Ingest, Ingested, assemble, media_of,
        };

        let message = event.get("message")?;
        let message_id = message.get("message_id")?.as_str()?.to_owned();
        if self.native.has_seen(&format!("lark:{message_id}")) {
            return None;
        }
        let chat_id = message.get("chat_id")?.as_str()?.to_owned();
        let chat_type = message
            .get("chat_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let message_type = message
            .get("message_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let sender_id = event
            .pointer("/sender/sender_id/open_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if sender_id.is_empty()
            || !self.is_user_allowed(sender_id)
            || event
                .pointer("/sender/sender_type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|kind| kind == "app" || kind == "bot")
            || (chat_type == "group"
                && !super::should_respond_in_group(
                    message
                        .get("mentions")
                        .and_then(serde_json::Value::as_array)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                ))
        {
            return None;
        }
        let content: serde_json::Value = serde_json::from_str(
            message
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("{}"),
        )
        .ok()?;
        let text = match message_type {
            "text" => content
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            "post" => super::parse_post_content(
                message
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            )
            .unwrap_or_default(),
            _ => String::new(),
        };
        // Feishu inserts these placeholders into plain text for mentioned
        // users. Keep the historical user-facing text on the typed path too.
        let text = super::strip_at_placeholders(&text).trim().to_owned();
        let thread_id = message
            .get("thread_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let conversation = ConversationRef {
            kind: if thread_id.is_some() {
                ConversationKind::Thread
            } else if chat_type == "p2p" {
                ConversationKind::Dm
            } else {
                ConversationKind::Group
            },
            id: chat_id,
            parent_id: message
                .get("root_id")
                .or_else(|| message.get("parent_id"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            thread_id,
            ..Default::default()
        };
        let sender = SenderRef {
            id: sender_id.to_owned(),
            ..Default::default()
        };
        let mut ingested = Ingested::default();
        let (key, resource_type, voice, default_mime) = match message_type {
            "image" => (
                content.get("image_key").and_then(serde_json::Value::as_str),
                "image",
                false,
                "image/jpeg",
            ),
            "audio" => (
                content.get("file_key").and_then(serde_json::Value::as_str),
                "file",
                true,
                "audio/ogg",
            ),
            "media" => (
                content.get("file_key").and_then(serde_json::Value::as_str),
                "file",
                false,
                "video/mp4",
            ),
            "file" => (
                content.get("file_key").and_then(serde_json::Value::as_str),
                "file",
                false,
                "application/octet-stream",
            ),
            _ => (None, "file", false, "application/octet-stream"),
        };
        if (self.native.events.is_some() || typed_sink_available)
            && (self.native.media_repository.is_some() || self.native.voice_media_store.is_some())
            && let Some(key) = key.filter(|key| !key.is_empty())
            && let Some(file) = self
                .download_resource(&message_id, key, resource_type, default_mime)
                .await
        {
            Ingest {
                provider: "lark",
                channel: ChannelRef {
                    id: "lark".into(),
                    account_id: None,
                },
                repository: self.native.media_repository.as_ref(),
                voice_store: self.native.voice_media_store.as_ref(),
                transcriber: self.native.transcriber.as_ref(),
                file_bytes: if resource_type == "image" {
                    IMAGE_LIMIT
                } else {
                    FILE_LIMIT
                },
                voice_bytes: FILE_LIMIT,
            }
            .add(
                &mut ingested,
                &conversation,
                &message_id,
                InboundFile {
                    bytes: file.0,
                    mime: file.1,
                    name: content
                        .get("file_name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .or(file.2),
                    duration_ms: content.get("duration").and_then(serde_json::Value::as_u64),
                    voice,
                    dedupe_key: format!("{message_id}:{key}"),
                    transcript: None,
                },
            )
            .await;
        }
        let parts = assemble(&text, None, ingested);
        if parts.is_empty() {
            return None;
        }
        let timestamp_unix_ms = message
            .get("create_time")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| value.parse::<u64>().ok());
        Some(ChannelInboundEnvelope {
            channel: ChannelRef {
                id: "lark".into(),
                account_id: None,
            },
            message_id: message_id.clone(),
            timestamp_unix_ms,
            conversation,
            sender,
            text,
            media: media_of(&parts),
            parts,
            raw: Some(serde_json::json!({"providerMessageId": message_id})),
            ..Default::default()
        })
    }

    async fn download_resource(
        &self,
        message_id: &str,
        key: &str,
        resource_type: &str,
        default_mime: &str,
    ) -> Option<(Vec<u8>, String, Option<String>)> {
        let token = self.get_tenant_access_token().await.ok()?;
        let url = format!(
            "{}/im/v1/messages/{}/resources/{}?type={}",
            self.api_base(),
            urlencoding::encode(message_id),
            urlencoding::encode(key),
            resource_type
        );
        let response = self
            .http_client()
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(crate::providers::native::inbound::normalize_mime)
            .filter(|mime| !mime.is_empty() && mime != "application/octet-stream")
            .unwrap_or_else(|| default_mime.into());
        let limit = if resource_type == "image" {
            IMAGE_LIMIT
        } else {
            FILE_LIMIT
        };
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if bytes.len().saturating_add(chunk.len()) as u64 > limit {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return None;
        }
        Some((bytes, mime, None))
    }
}
