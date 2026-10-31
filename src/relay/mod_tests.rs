use crate::relay::{
    AuthenticatedRelayInboundEvent, CONTRACT_VERSION, CapabilityDescriptor,
    ConnectorToGatewayFrame, DELIVERY_SIG_HEADER, DELIVERY_TS_HEADER, GatewayToConnectorFrame,
    PassthroughForward, RelayDescriptorOptions, RelayFrameDialer, RelayFrameIo, RelayIdentity,
    RelayInboundHandler, RelayPlatformEntry, RelayReconnectPolicy, RelayTransport,
    RelayTransportError, RelayTransportTimeouts, delivery_payload, make_token_at,
    make_upgrade_token_at, relay_send_action_from_outbound_intent, sign,
    verify_delivery_signature_at, verify_signature, verify_token_at,
};
use crate::{
    ChannelOutboundIntent, DeliveryDurability, OutboundPayload, outbound_intent_from_legacy_message,
};
use async_trait::async_trait;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tinychannels_runtime::relay::ConnectorToGatewayFrameExt as _;
use tokio::sync::Notify;
use tokio::time::{Duration, sleep};

const SECRET: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const CONNECTOR_TOKEN: &str = "Z3ctaW5zdGFuY2UtMTowOjM3YWE3YjE0NWU4NzY0ZDQwM2JhOWM2MzlmMjMwZGQ2M2RlOGVkOTliODhmZWQzNmFhMDI2MjVhOGE3ZTM1NjQ";
const CONNECTOR_BODY: &str =
    r#"{"type":"message","event":{"text":"hi","source":{"chat_id":"c1"}}}"#;
const CONNECTOR_TS: i64 = 1_750_000_000;
const CONNECTOR_SIG: &str = "ac9509c8dae52b5590f06378260877334ff1adc4b1c96bafa4b514165fae6dc6";

fn telegram_descriptor() -> CapabilityDescriptor {
    CapabilityDescriptor {
        contract_version: CONTRACT_VERSION,
        platform: "telegram".into(),
        label: "Telegram".into(),
        max_message_length: 4096,
        supports_draft_streaming: false,
        supports_edit: true,
        supports_threads: true,
        markdown_dialect: "markdown_v2".into(),
        len_unit: "utf16".into(),
        emoji: "✈️".into(),
        platform_hint: "You are on Telegram.".into(),
        pii_safe: false,
    }
}

#[test]
fn descriptor_roundtrips_json_and_ignores_unknown_keys() {
    let descriptor = telegram_descriptor();
    let json = descriptor.to_json().expect("descriptor json");
    assert_eq!(
        CapabilityDescriptor::from_json(&json).expect("descriptor parse"),
        descriptor
    );

    let raw = format!("{},\"future_field\":\"ignored\"}}", &json[..json.len() - 1]);
    assert_eq!(
        CapabilityDescriptor::from_json(&raw).expect("descriptor parse"),
        descriptor
    );
}

#[test]
fn descriptor_json_is_compact_and_sorted() {
    let json = telegram_descriptor().to_json().expect("descriptor json");
    assert!(json.starts_with(r#"{"contract_version":1,"emoji":"✈️","label":"Telegram""#));
    assert!(!json.contains(": "));
}

#[test]
fn descriptor_fills_optional_defaults() {
    let minimal = concat!(
        r#"{"contract_version":1,"platform":"x","label":"X","#,
        r#""max_message_length":2000,"supports_draft_streaming":false,"#,
        r#""supports_edit":false,"supports_threads":false,"#,
        r#""markdown_dialect":"plain","len_unit":"chars"}"#
    );
    let descriptor = CapabilityDescriptor::from_json(minimal).expect("descriptor parse");
    assert_eq!(descriptor.emoji, "🔌");
    assert_eq!(descriptor.platform_hint, "");
    assert!(!descriptor.pii_safe);
}

#[test]
fn descriptor_projects_platform_entry_fields() {
    let entry = RelayPlatformEntry {
        name: "telegram".into(),
        label: "Telegram".into(),
        max_message_length: 0,
        emoji: Some("✈️".into()),
        platform_hint: Some("You are on Telegram.".into()),
        pii_safe: false,
    };
    let descriptor = CapabilityDescriptor::from_platform_entry(
        &entry,
        RelayDescriptorOptions {
            len_unit: "utf16".into(),
            supports_draft_streaming: true,
            supports_edit: false,
            supports_threads: true,
            markdown_dialect: "discord".into(),
        },
    );

    assert_eq!(descriptor.contract_version, CONTRACT_VERSION);
    assert_eq!(descriptor.max_message_length, 4096);
    assert_eq!(descriptor.emoji, "✈️");
    assert_eq!(descriptor.platform_hint, "You are on Telegram.");
    assert_eq!(descriptor.len_unit, "utf16");
    assert!(descriptor.supports_draft_streaming);
    assert!(!descriptor.supports_edit);
    assert_eq!(descriptor.markdown_dialect, "discord");
}

#[test]
fn token_roundtrips_and_payload_may_contain_colons() {
    let payload = "agent:main:discord:group:chanA";
    let token = make_token_at(payload, SECRET, 0, 1_700_000_000);
    assert_eq!(
        verify_token_at(&token, &[SECRET], 1_700_000_000),
        Some(payload.into())
    );
}

#[test]
fn upgrade_token_is_token_of_gateway_id() {
    assert_eq!(
        make_upgrade_token_at("gw-1", SECRET, 0, 1_700_000_000),
        make_token_at("gw-1", SECRET, 0, 1_700_000_000)
    );
}

#[test]
fn token_rejects_wrong_secret_expiry_and_garbage() {
    let token = make_token_at("p", SECRET, 0, 1_700_000_000);
    assert_eq!(verify_token_at(&token, &["deadbeef"], 1_700_000_000), None);

    let expired = make_token_at("p", SECRET, 1, 1);
    assert_eq!(verify_token_at(&expired, &[SECRET], 1_700_000_000), None);
    assert_eq!(
        verify_token_at("not-base64url!!!", &[SECRET], 1_700_000_000),
        None
    );
}

#[test]
fn token_rotation_verify_list_accepts_secondary_secret() {
    let old = SECRET;
    let new = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";
    let token = make_token_at("p", old, 0, 1_700_000_000);
    assert_eq!(
        verify_token_at(&token, &[new, old], 1_700_000_000),
        Some("p".into())
    );
    assert_eq!(verify_token_at(&token, &[new], 1_700_000_000), None);
}

#[test]
fn signature_verifies_against_rotation_list() {
    let payload = "1700000000.body";
    let signature = sign(payload, SECRET);
    assert!(verify_signature(payload, &signature, &["wrong", SECRET]));
    assert!(!verify_signature(payload, &signature, &["wrong"]));
    assert!(!verify_signature(payload, "zz", &[SECRET]));
}

#[test]
fn delivery_signature_accepts_valid_and_rejects_tamper_or_skew() {
    let body = r#"{"type":"message","event":{"text":"x"}}"#;
    let ts = 1_700_000_000;
    let signature = sign(&delivery_payload(ts, body), SECRET);

    assert!(verify_delivery_signature_at(
        body,
        Some(&ts.to_string()),
        Some(&signature),
        &[SECRET],
        300,
        ts
    ));
    assert!(!verify_delivery_signature_at(
        &format!("{body} "),
        Some(&ts.to_string()),
        Some(&signature),
        &[SECRET],
        300,
        ts
    ));
    assert!(!verify_delivery_signature_at(
        body,
        Some(&ts.to_string()),
        Some(&signature),
        &[SECRET],
        300,
        ts + 301
    ));
    assert!(verify_delivery_signature_at(
        body,
        Some(&ts.to_string()),
        Some(&signature),
        &[SECRET],
        300,
        ts + 299
    ));
}

#[test]
fn delivery_headers_match_connector_names() {
    assert_eq!(DELIVERY_TS_HEADER, "x-relay-timestamp");
    assert_eq!(DELIVERY_SIG_HEADER, "x-relay-signature");
}

#[test]
fn frozen_connector_vectors_match_hermes_tests() {
    assert_eq!(
        make_token_at("gw-instance-1", SECRET, 0, 1_700_000_000),
        CONNECTOR_TOKEN
    );
    assert_eq!(
        verify_token_at(CONNECTOR_TOKEN, &[SECRET], 1_700_000_000),
        Some("gw-instance-1".into())
    );
    assert_eq!(
        sign(&delivery_payload(CONNECTOR_TS, CONNECTOR_BODY), SECRET),
        CONNECTOR_SIG
    );
    assert!(verify_delivery_signature_at(
        CONNECTOR_BODY,
        Some(&CONNECTOR_TS.to_string()),
        Some(CONNECTOR_SIG),
        &[SECRET],
        300,
        CONNECTOR_TS
    ));
}

#[test]
fn gateway_frames_match_connector_wire_casing() {
    let hello = GatewayToConnectorFrame::Hello {
        platform: "discord".into(),
        bot_id: "appShared".into(),
    };
    assert_eq!(
        serde_json::to_value(&hello).expect("hello json"),
        json!({"type":"hello","platform":"discord","botId":"appShared"})
    );

    let outbound = GatewayToConnectorFrame::Outbound {
        request_id: "req-1".into(),
        action: json!({"op":"send","text":"hi"}),
        platform: Some("discord".into()),
        bot_id: Some("appShared".into()),
    };
    assert_eq!(
        serde_json::to_value(&outbound).expect("outbound json"),
        json!({
            "type":"outbound",
            "requestId":"req-1",
            "action":{"op":"send","text":"hi"},
            "platform":"discord",
            "botId":"appShared"
        })
    );

    let ack = GatewayToConnectorFrame::InboundAck {
        buffer_id: "buf-1".into(),
    };
    assert_eq!(
        ack.to_json().expect("ack json"),
        r#"{"type":"inbound_ack","bufferId":"buf-1"}"#
    );
}

#[test]
fn connector_frames_roundtrip_and_build_buffer_ack() {
    let raw = r#"{"type":"outbound_result","requestId":"req-1","result":{"success":true,"message_id":"srv-send"}}"#;
    let frame = ConnectorToGatewayFrame::from_json(raw).expect("outbound result");
    assert_eq!(
        frame,
        ConnectorToGatewayFrame::OutboundResult {
            request_id: "req-1".into(),
            result: json!({"success":true,"message_id":"srv-send"}),
        }
    );
    assert_eq!(
        frame.to_json().expect("outbound result json"),
        r#"{"type":"outbound_result","requestId":"req-1","result":{"message_id":"srv-send","success":true}}"#
    );

    let inbound = ConnectorToGatewayFrame::Inbound {
        event: json!({"text":"hello"}),
        buffer_id: Some("buf-1".into()),
    };
    assert_eq!(
        inbound.inbound_ack(),
        Some(GatewayToConnectorFrame::InboundAck {
            buffer_id: "buf-1".into()
        })
    );
}

#[test]
fn relay_send_action_projects_text_outbound_intent() {
    let intent = ChannelOutboundIntent {
        idempotency_key: "send-1".into(),
        channel_id: "discord".into(),
        conversation_id: "channel-123".into(),
        channel_ref: None,
        conversation_ref: None,
        reply_to_id: Some("message-1".into()),
        thread_id: Some("thread-2".into()),
        turn_id: None,
        durability: DeliveryDurability::Required,
        payload: OutboundPayload::Text { text: "hi".into() },
    };

    assert_eq!(
        relay_send_action_from_outbound_intent(&intent),
        json!({
            "op": "send",
            "chat_id": "channel-123",
            "content": "hi",
            "reply_to": "message-1",
            "metadata": {
                "idempotency_key": "send-1",
                "channel_id": "discord",
                "conversation_id": "channel-123",
                "thread_id": "thread-2",
                "durability": "required"
            }
        })
    );
}

#[test]
fn relay_send_action_preserves_legacy_native_payload_in_metadata() {
    let intent = outbound_intent_from_legacy_message(
        "telegram",
        json!({
            "chatId": "chat-1",
            "content": "hello",
            "photoUrl": "https://example.test/a.png",
            "idempotencyKey": "legacy-1"
        }),
    );

    assert_eq!(
        relay_send_action_from_outbound_intent(&intent),
        json!({
            "op": "send",
            "chat_id": "chat-1",
            "content": "hello",
            "reply_to": null,
            "metadata": {
                "idempotency_key": "legacy-1",
                "channel_id": "telegram",
                "conversation_id": "chat-1",
                "durability": "best_effort",
                "payload": {
                    "kind": "native_channel_data",
                    "data": {
                        "chatId": "chat-1",
                        "content": "hello",
                        "photoUrl": "https://example.test/a.png",
                        "idempotencyKey": "legacy-1"
                    }
                }
            }
        })
    );
}

#[test]
fn authenticated_inbound_event_strips_forged_wire_trust() {
    let frame = ConnectorToGatewayFrame::Inbound {
        event: json!({
            "text":"hello",
            "source":{
                "platform":"discord",
                "chat_id":"chan1",
                "delivered_via_upstream_relay":true,
                "deliveredViaUpstreamRelay":true
            },
            "access":{"deliveredViaUpstreamRelay":true}
        }),
        buffer_id: Some("buf-1".into()),
    };

    let inbound = frame
        .authenticated_inbound_event()
        .expect("authenticated inbound");
    assert!(inbound.delivered_via_authenticated_relay);
    assert_eq!(inbound.buffer_id.as_deref(), Some("buf-1"));
    assert_eq!(
        inbound.event,
        json!({
            "text":"hello",
            "source":{"platform":"discord","chat_id":"chan1"},
            "access":{}
        })
    );
}

#[test]
fn passthrough_forward_decodes_body_and_tolerates_malformed_base64() {
    let raw = concat!(
        r#"{"platform":"discord","botId":"appShared","method":"POST","#,
        r#""path":"/interactions/discord/appShared","#,
        r#""headers":[["content-type","application/json"]],"bodyB64":"eyJ0eXBlIjoyfQ=="}"#
    );
    let forward: PassthroughForward = serde_json::from_str(raw).expect("passthrough");
    assert_eq!(forward.platform, "discord");
    assert_eq!(forward.bot_id, "appShared");
    assert_eq!(
        forward.headers,
        vec![("content-type".into(), "application/json".into())]
    );
    assert_eq!(forward.body, br#"{"type":2}"#);
    assert_eq!(
        serde_json::to_value(&forward).expect("passthrough json")["bodyB64"],
        json!("eyJ0eXBlIjoyfQ==")
    );

    let malformed: PassthroughForward =
        serde_json::from_str(r#"{"platform":"x","bodyB64":"!!!not base64!!!"}"#)
            .expect("malformed passthrough");
    assert!(malformed.body.is_empty());
}

#[test]
fn passthrough_forward_buffer_ack_uses_same_inbound_ack_frame() {
    let frame = ConnectorToGatewayFrame::PassthroughForward {
        forward: PassthroughForward {
            platform: "discord".into(),
            bot_id: "appShared".into(),
            method: "POST".into(),
            path: "/interactions".into(),
            headers: vec![],
            body: vec![],
        },
        buffer_id: Some("buf-passthrough".into()),
    };

    assert_eq!(
        frame.inbound_ack(),
        Some(GatewayToConnectorFrame::InboundAck {
            buffer_id: "buf-passthrough".into()
        })
    );
}

#[derive(Default)]
struct MemoryRelayIo {
    sent: Mutex<Vec<GatewayToConnectorFrame>>,
    inbound: Mutex<VecDeque<ConnectorToGatewayFrame>>,
    sent_notify: Notify,
    inbound_notify: Notify,
    received: std::sync::atomic::AtomicUsize,
    received_notify: Notify,
    closed: AtomicBool,
}

impl MemoryRelayIo {
    async fn push(&self, frame: ConnectorToGatewayFrame) {
        self.inbound.lock().expect("inbound lock").push_back(frame);
        self.inbound_notify.notify_one();
    }

    fn sent(&self) -> Vec<GatewayToConnectorFrame> {
        self.sent.lock().expect("sent lock").clone()
    }

    async fn wait_for_sent_len(&self, len: usize) {
        loop {
            if self.sent.lock().expect("sent lock").len() >= len {
                return;
            }
            self.sent_notify.notified().await;
        }
    }

    async fn wait_for_received_len(&self, len: usize) {
        loop {
            let notified = self.received_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.received.load(Ordering::SeqCst) >= len {
                return;
            }
            notified.await;
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.inbound_notify.notify_waiters();
    }
}

#[async_trait]
impl RelayFrameIo for MemoryRelayIo {
    async fn send(&self, frame: GatewayToConnectorFrame) -> Result<(), RelayTransportError> {
        self.sent.lock().expect("sent lock").push(frame);
        self.sent_notify.notify_waiters();
        Ok(())
    }

    async fn recv(&self) -> Result<Option<ConnectorToGatewayFrame>, RelayTransportError> {
        loop {
            if let Some(frame) = self.inbound.lock().expect("inbound lock").pop_front() {
                self.received.fetch_add(1, Ordering::SeqCst);
                self.received_notify.notify_waiters();
                return Ok(Some(frame));
            }
            if self.closed.load(Ordering::SeqCst) {
                return Ok(None);
            }
            self.inbound_notify.notified().await;
        }
    }
}

#[derive(Default)]
struct RecordingInboundHandler {
    events: Mutex<Vec<AuthenticatedRelayInboundEvent>>,
}

#[async_trait]
impl RelayInboundHandler for RecordingInboundHandler {
    async fn handle(
        &self,
        event: AuthenticatedRelayInboundEvent,
    ) -> Result<(), RelayTransportError> {
        self.events.lock().expect("events lock").push(event);
        Ok(())
    }
}

#[derive(Default)]
struct QueueDialer {
    ios: Mutex<VecDeque<Arc<MemoryRelayIo>>>,
}

impl QueueDialer {
    fn push(&self, io: Arc<MemoryRelayIo>) {
        self.ios.lock().expect("ios lock").push_back(io);
    }
}

#[async_trait]
impl RelayFrameDialer for QueueDialer {
    async fn dial(&self) -> Result<Arc<dyn RelayFrameIo>, RelayTransportError> {
        self.ios
            .lock()
            .expect("ios lock")
            .pop_front()
            .map(|io| io as Arc<dyn RelayFrameIo>)
            .ok_or_else(|| RelayTransportError::Io("no relay io queued".into()))
    }
}

fn short_relay_timeouts() -> RelayTransportTimeouts {
    RelayTransportTimeouts {
        handshake_ms: 500,
        outbound_ms: 500,
        idle_ms: 500,
    }
}

fn discord_transport(io: Arc<MemoryRelayIo>) -> RelayTransport {
    RelayTransport::new(
        vec![RelayIdentity {
            platform: "discord".into(),
            bot_id: "appShared".into(),
        }],
        io,
        short_relay_timeouts(),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn relay_transport_connect_sends_hello_and_handshake_reads_descriptor() {
    let io = Arc::new(MemoryRelayIo::default());
    let transport = discord_transport(io.clone());

    transport.connect().await.expect("connect");
    assert_eq!(
        io.sent(),
        vec![GatewayToConnectorFrame::Hello {
            platform: "discord".into(),
            bot_id: "appShared".into()
        }]
    );

    io.push(ConnectorToGatewayFrame::Descriptor {
        descriptor: telegram_descriptor(),
    })
    .await;
    assert_eq!(
        transport.handshake().await.expect("handshake"),
        telegram_descriptor()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn relay_transport_negotiates_v2_and_returns_the_canonical_descriptor() {
    let io = Arc::new(MemoryRelayIo::default());
    let transport = Arc::new(RelayTransport::new(
        vec![
            RelayIdentity {
                platform: "discord".into(),
                bot_id: "a".into(),
            },
            RelayIdentity {
                platform: "discord".into(),
                bot_id: "b".into(),
            },
        ],
        io.clone(),
        short_relay_timeouts(),
    ));
    transport.connect_v2().await.expect("connect v2");
    let negotiation = {
        let transport = transport.clone();
        tokio::spawn(async move { transport.negotiate_v2().await })
    };
    io.wait_for_sent_len(2).await;
    assert!(matches!(
        io.sent().last(),
        Some(GatewayToConnectorFrame::HelloV2 {
            contract_version: 2,
            ..
        })
    ));
    let mut capabilities = tinychannels_bus::ChannelCapabilities::unknown();
    capabilities.progressive_edits = true;
    capabilities.text.text_limit = Some(4096);
    // Unrelated descriptors and the v1 descriptor cannot satisfy the v2 wait.
    let mut unrelated = tinychannels_bus::ChannelCapabilities::unknown();
    unrelated.progressive_edits = true;
    io.push(ConnectorToGatewayFrame::DescriptorV2 {
        platform: "discord".into(),
        bot_id: "other".into(),
        capabilities: Box::new(unrelated),
    })
    .await;
    io.push(ConnectorToGatewayFrame::Descriptor {
        descriptor: telegram_descriptor(),
    })
    .await;
    let mut second_capabilities = tinychannels_bus::ChannelCapabilities::unknown();
    second_capabilities.reactions = true;
    io.push(ConnectorToGatewayFrame::DescriptorV2 {
        platform: "discord".into(),
        bot_id: "b".into(),
        capabilities: Box::new(second_capabilities.clone()),
    })
    .await;
    io.push(ConnectorToGatewayFrame::DescriptorV2 {
        platform: "discord".into(),
        bot_id: "a".into(),
        capabilities: Box::new(capabilities.clone()),
    })
    .await;
    let negotiated = negotiation.await.unwrap().unwrap();
    assert_eq!(
        negotiated.get(&RelayIdentity {
            platform: "discord".into(),
            bot_id: "a".into()
        }),
        Some(&capabilities)
    );
    assert_eq!(
        negotiated.get(&RelayIdentity {
            platform: "discord".into(),
            bot_id: "b".into()
        }),
        Some(&second_capabilities)
    );
}

#[tokio::test(start_paused = true)]
async fn relay_v2_ignores_unsupported_versions_and_times_out_at_one_deadline() {
    let io = Arc::new(MemoryRelayIo::default());
    let transport = Arc::new(RelayTransport::new(
        vec![RelayIdentity {
            platform: "discord".into(),
            bot_id: "a".into(),
        }],
        io.clone(),
        RelayTransportTimeouts {
            handshake_ms: 60,
            outbound_ms: 500,
            idle_ms: 500,
        },
    ));
    transport.connect_v2().await.expect("connect v2");
    let started = tokio::time::Instant::now();
    let negotiation = {
        let transport = transport.clone();
        tokio::spawn(async move { transport.negotiate_v2().await })
    };
    io.wait_for_sent_len(1).await;
    let mut wrong_version = tinychannels_bus::ChannelCapabilities::unknown();
    wrong_version.version = 99;
    tokio::task::yield_now().await;
    let mut received = 0;
    for _ in 0..4 {
        io.push(ConnectorToGatewayFrame::DescriptorV2 {
            platform: "discord".into(),
            bot_id: "other".into(),
            capabilities: Box::new(wrong_version.clone()),
        })
        .await;
        io.push(ConnectorToGatewayFrame::Descriptor {
            descriptor: telegram_descriptor(),
        })
        .await;
        received += 2;
        io.wait_for_received_len(received).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(15)).await;
    }
    assert_eq!(
        negotiation.await.unwrap(),
        Err(RelayTransportError::Timeout {
            operation: "handshake_v2"
        })
    );
    assert_eq!(
        started.elapsed(),
        Duration::from_millis(60),
        "unrelated frames must not restart the handshake deadline"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn relay_v2_rejects_an_unsupported_version_for_a_configured_identity() {
    let io = Arc::new(MemoryRelayIo::default());
    let transport = Arc::new(discord_transport(io.clone()));
    transport.connect_v2().await.expect("connect v2");
    let negotiation = {
        let transport = transport.clone();
        tokio::spawn(async move { transport.negotiate_v2().await })
    };
    io.wait_for_sent_len(1).await;
    let mut unsupported = tinychannels_bus::ChannelCapabilities::unknown();
    unsupported.version = 7;
    io.push(ConnectorToGatewayFrame::DescriptorV2 {
        platform: "discord".into(),
        bot_id: "appShared".into(),
        capabilities: Box::new(unsupported),
    })
    .await;
    assert_eq!(
        negotiation.await.unwrap(),
        Err(RelayTransportError::UnsupportedCapabilitiesVersion {
            actual: 7,
            expected: tinychannels_bus::ChannelCapabilities::VERSION,
        })
    );
}

#[tokio::test]
async fn relay_v2_refuses_cached_capabilities_after_terminal_shutdown() {
    let io = Arc::new(MemoryRelayIo::default());
    let transport = Arc::new(discord_transport(io.clone()));
    transport.connect_v2().await.expect("connect v2");
    let negotiation = {
        let transport = transport.clone();
        tokio::spawn(async move { transport.negotiate_v2().await })
    };
    io.wait_for_sent_len(1).await;
    let mut capabilities = tinychannels_bus::ChannelCapabilities::unknown();
    capabilities.progressive_edits = true;
    io.push(ConnectorToGatewayFrame::DescriptorV2 {
        platform: "discord".into(),
        bot_id: "appShared".into(),
        capabilities: Box::new(capabilities),
    })
    .await;
    assert!(negotiation.await.unwrap().is_ok());
    transport.shutdown().await;
    assert_eq!(
        transport.negotiate_v2().await,
        Err(RelayTransportError::Closed)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn relay_transport_outbound_waits_for_matching_result() {
    let io = Arc::new(MemoryRelayIo::default());
    let transport = Arc::new(discord_transport(io.clone()));
    transport.connect().await.expect("connect");

    let task = {
        let transport = transport.clone();
        tokio::spawn(async move {
            transport
                .send_outbound(json!({"op":"send","text":"hi"}), Some("discord"))
                .await
        })
    };

    io.wait_for_sent_len(2).await;
    let sent = io.sent();
    let GatewayToConnectorFrame::Outbound {
        request_id,
        action,
        platform,
        bot_id,
    } = &sent[1]
    else {
        panic!("expected outbound frame");
    };
    assert_eq!(request_id, "req-1");
    assert_eq!(action, &json!({"op":"send","text":"hi"}));
    assert_eq!(platform.as_deref(), Some("discord"));
    assert_eq!(bot_id.as_deref(), Some("appShared"));

    io.push(ConnectorToGatewayFrame::OutboundResult {
        request_id: request_id.clone(),
        result: json!({"success":true,"message_id":"srv-send"}),
    })
    .await;
    assert_eq!(
        task.await.expect("task").expect("outbound result"),
        json!({"success":true,"message_id":"srv-send"})
    );
}

#[tokio::test(flavor = "current_thread")]
async fn relay_transport_inbound_handler_runs_before_buffer_ack() {
    let io = Arc::new(MemoryRelayIo::default());
    let transport = discord_transport(io.clone());
    let handler = Arc::new(RecordingInboundHandler::default());
    transport.set_inbound_handler(handler.clone()).await;
    transport.connect().await.expect("connect");

    io.push(ConnectorToGatewayFrame::Inbound {
        event: json!({
            "text":"hello",
            "source":{
                "platform":"discord",
                "delivered_via_upstream_relay":true
            }
        }),
        buffer_id: Some("buf-1".into()),
    })
    .await;

    io.wait_for_sent_len(2).await;
    assert_eq!(
        io.sent()[1],
        GatewayToConnectorFrame::InboundAck {
            buffer_id: "buf-1".into()
        }
    );
    let events = handler.events.lock().expect("events lock");
    assert_eq!(events.len(), 1);
    assert!(events[0].delivered_via_authenticated_relay);
    assert_eq!(
        events[0].event,
        json!({"text":"hello","source":{"platform":"discord"}})
    );
}

#[tokio::test(flavor = "current_thread")]
async fn relay_transport_does_not_ack_inbound_without_handler() {
    let io = Arc::new(MemoryRelayIo::default());
    let transport = discord_transport(io.clone());
    transport.connect().await.expect("connect");

    io.push(ConnectorToGatewayFrame::Inbound {
        event: json!({"text":"hello"}),
        buffer_id: Some("buf-1".into()),
    })
    .await;

    sleep(Duration::from_millis(20)).await;
    assert_eq!(
        io.sent(),
        vec![GatewayToConnectorFrame::Hello {
            platform: "discord".into(),
            bot_id: "appShared".into()
        }]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn relay_transport_go_idle_waits_for_connector_ack() {
    let io = Arc::new(MemoryRelayIo::default());
    let transport = Arc::new(discord_transport(io.clone()));
    transport.connect().await.expect("connect");

    let task = {
        let transport = transport.clone();
        tokio::spawn(async move { transport.go_idle().await })
    };
    io.wait_for_sent_len(2).await;
    assert_eq!(io.sent()[1], GatewayToConnectorFrame::GoingIdle);

    io.push(ConnectorToGatewayFrame::GoingIdleAck).await;
    assert!(task.await.expect("task").expect("go idle"));
}

#[tokio::test(flavor = "current_thread")]
async fn relay_transport_reconnect_supervisor_redials_and_rehandshakes() {
    let first = Arc::new(MemoryRelayIo::default());
    let second = Arc::new(MemoryRelayIo::default());
    let dialer = Arc::new(QueueDialer::default());
    dialer.push(second.clone());
    let transport = Arc::new(discord_transport(first.clone()));

    transport.connect().await.expect("connect");
    first
        .push(ConnectorToGatewayFrame::Descriptor {
            descriptor: telegram_descriptor(),
        })
        .await;
    assert_eq!(
        transport.handshake().await.expect("initial handshake"),
        telegram_descriptor()
    );

    let handle = transport.spawn_reconnect_supervisor(
        dialer,
        RelayReconnectPolicy {
            backoff_ms: 1,
            max_backoff_ms: 2,
        },
    );
    first.close();

    second.wait_for_sent_len(1).await;
    assert_eq!(
        second.sent(),
        vec![GatewayToConnectorFrame::Hello {
            platform: "discord".into(),
            bot_id: "appShared".into()
        }]
    );

    let mut descriptor = telegram_descriptor();
    descriptor.label = "Discord Relay".into();
    second
        .push(ConnectorToGatewayFrame::Descriptor {
            descriptor: descriptor.clone(),
        })
        .await;
    assert_eq!(
        transport.handshake().await.expect("reconnected handshake"),
        descriptor
    );
    handle.abort();
}

#[test]
fn websocket_dial_url_matches_hermes_normalization() {
    assert_eq!(
        crate::relay::websocket_dial_url("https://connector.example"),
        "wss://connector.example/relay"
    );
    assert_eq!(
        crate::relay::websocket_dial_url("http://connector.example/"),
        "ws://connector.example/relay"
    );
    assert_eq!(
        crate::relay::websocket_dial_url("wss://connector.example/relay"),
        "wss://connector.example/relay"
    );
}

#[test]
fn websocket_upgrade_authorization_uses_relay_hmac_token() {
    let config = crate::relay::WebSocketRelayConfig {
        url: "https://connector.example".into(),
        gateway_id: Some("gw-1".into()),
        upgrade_secret: Some(SECRET.into()),
        upgrade_ttl_seconds: 300,
    };
    let header = crate::relay::websocket_upgrade_authorization(&config).expect("auth header");
    let token = header.strip_prefix("Bearer ").expect("bearer token");
    assert_eq!(
        crate::relay::verify_token(token, &[SECRET]),
        Some("gw-1".into())
    );
}

#[test]
fn websocket_config_projects_from_relay_runtime_config() {
    let config = crate::config::RelayRuntimeConfig {
        url: "https://connector.example".into(),
        gateway_id: Some("gw-1".into()),
        upgrade_secret: Some(SECRET.into()),
        upgrade_ttl_seconds: 123,
        ..Default::default()
    };

    let websocket = crate::relay::WebSocketRelayConfig::from(&config);

    assert_eq!(websocket.url, "https://connector.example");
    assert_eq!(websocket.gateway_id.as_deref(), Some("gw-1"));
    assert_eq!(websocket.upgrade_secret.as_deref(), Some(SECRET));
    assert_eq!(websocket.upgrade_ttl_seconds, 123);
    let _dialer = crate::relay::WebSocketRelayDialer::new(websocket);
}

/// End-to-end loopback tests that dial the real WebSocket relay I/O against an
/// in-process `tokio-tungstenite` server, exercising newline-delimited frame
/// sequencing, upgrade-token authorization, and reconnect through the dialer.
mod websocket_loopback {
    use super::{SECRET, telegram_descriptor};
    use crate::relay::{
        ConnectorToGatewayFrame, GatewayToConnectorFrame, RelayFrameDialer, RelayFrameIo,
        WebSocketRelayConfig, WebSocketRelayDialer, connect_websocket_relay_io, verify_token,
    };
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
    use tokio_tungstenite::tungstenite::http::StatusCode;
    use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
    use tokio_tungstenite::tungstenite::protocol::Message;
    use tokio_tungstenite::{accept_async, accept_hdr_async};

    /// Accept only when the upgrade request carries a valid relay bearer token.
    // The `Result<Response, ErrorResponse>` shape is fixed by tungstenite's
    // handshake `Callback` contract, so the large error variant is unavoidable.
    #[allow(clippy::result_large_err)]
    fn authorize(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        let authorized = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .and_then(|token| verify_token(token, &[SECRET]))
            .is_some();
        if authorized {
            Ok(response)
        } else {
            let mut error = ErrorResponse::new(Some("unauthorized".to_string()));
            *error.status_mut() = StatusCode::UNAUTHORIZED;
            Err(error)
        }
    }

    fn hello() -> GatewayToConnectorFrame {
        GatewayToConnectorFrame::Hello {
            platform: "telegram".into(),
            bot_id: "bot-1".into(),
        }
    }

    #[tokio::test]
    async fn roundtrips_frames_in_order_over_a_real_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept tcp");
            let mut ws = accept_async(tcp).await.expect("server handshake");

            // Two connector frames delivered in a single text message; the
            // client must split on newlines and preserve order.
            let descriptor = ConnectorToGatewayFrame::Descriptor {
                descriptor: telegram_descriptor(),
            };
            let inbound = ConnectorToGatewayFrame::Inbound {
                event: json!({ "text": "hi" }),
                buffer_id: Some("b1".into()),
            };
            let batch = format!(
                "{}\n{}\n",
                descriptor.to_json().expect("descriptor json"),
                inbound.to_json().expect("inbound json"),
            );
            ws.send(Message::Text(batch.into()))
                .await
                .expect("send batch");

            // Read one gateway frame the client sends back.
            loop {
                match ws.next().await {
                    Some(Ok(Message::Text(text))) => {
                        break GatewayToConnectorFrame::from_json(text.to_string().trim())
                            .expect("decode gateway frame");
                    }
                    Some(Ok(_)) => continue,
                    other => panic!("expected a text frame, got {other:?}"),
                }
            }
        });

        let config = WebSocketRelayConfig::new(format!("http://{addr}"));
        let io = connect_websocket_relay_io(&config).await.expect("dial");

        match io.recv().await.expect("recv descriptor") {
            Some(ConnectorToGatewayFrame::Descriptor { descriptor }) => {
                assert_eq!(descriptor, telegram_descriptor());
            }
            other => panic!("expected descriptor first, got {other:?}"),
        }
        match io.recv().await.expect("recv inbound") {
            Some(ConnectorToGatewayFrame::Inbound { buffer_id, .. }) => {
                assert_eq!(buffer_id.as_deref(), Some("b1"));
            }
            other => panic!("expected inbound second, got {other:?}"),
        }

        io.send(hello()).await.expect("send hello");

        let received = server.await.expect("join server");
        assert_eq!(received, hello());
    }

    #[tokio::test]
    async fn rejects_dial_without_valid_upgrade_token() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept tcp");
            // No Authorization header is sent, so the handshake must be refused.
            assert!(
                accept_hdr_async(tcp, authorize).await.is_err(),
                "server should reject a tokenless upgrade"
            );
        });

        // Client without gateway_id/secret sends no Authorization header.
        let config = WebSocketRelayConfig::new(format!("http://{addr}"));
        assert!(
            connect_websocket_relay_io(&config).await.is_err(),
            "dial without a valid upgrade token must fail"
        );
        server.await.expect("join server");
    }

    #[tokio::test]
    async fn dialer_reconnects_with_valid_upgrade_token() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (tcp, _) = listener.accept().await.expect("accept tcp");
                let mut ws = accept_hdr_async(tcp, authorize)
                    .await
                    .expect("authorized handshake");
                let descriptor = ConnectorToGatewayFrame::Descriptor {
                    descriptor: telegram_descriptor(),
                };
                ws.send(Message::Text(
                    format!("{}\n", descriptor.to_json().expect("descriptor json")).into(),
                ))
                .await
                .expect("send descriptor");
                // Drain until the client drops the connection, then loop to the
                // next accept to serve the reconnect.
                while let Some(Ok(message)) = ws.next().await {
                    if message.is_close() {
                        break;
                    }
                }
            }
        });

        let mut config = WebSocketRelayConfig::new(format!("http://{addr}"));
        config.gateway_id = Some("gw-1".into());
        config.upgrade_secret = Some(SECRET.into());
        let dialer = WebSocketRelayDialer::new(config);

        for attempt in 0..2 {
            let io = dialer.dial().await.expect("redial");
            match io.recv().await.expect("recv descriptor") {
                Some(ConnectorToGatewayFrame::Descriptor { descriptor }) => {
                    assert_eq!(descriptor, telegram_descriptor(), "attempt {attempt}");
                }
                other => panic!("expected descriptor on attempt {attempt}, got {other:?}"),
            }
            // Dropping the I/O closes the socket so the server serves the next
            // connection.
            drop(io);
        }

        server.await.expect("join server");
    }

    #[tokio::test]
    async fn wss_dialer_rejects_a_self_signed_server_certificate() {
        use std::sync::Arc;
        use tokio::net::TcpListener;
        use tokio_rustls::TlsAcceptor;

        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        let certificate = include_bytes!("../../tests/fixtures/tls/cert.der");
        let private_key = include_bytes!("../../tests/fixtures/tls/key.der");
        let server_config = tokio_rustls::rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![tokio_rustls::rustls::pki_types::CertificateDer::from(
                    certificate.to_vec(),
                )],
                tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(private_key.to_vec())
                    .into(),
            )
            .expect("test certificate and key");
        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("local address").port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept TCP");
            assert!(
                acceptor.accept(socket).await.is_err(),
                "client should abort its handshake when the self-signed certificate is rejected"
            );
        });

        let dialer = WebSocketRelayDialer::new(WebSocketRelayConfig::new(format!(
            "https://localhost:{port}"
        )));
        let error = tokio::time::timeout(std::time::Duration::from_secs(3), dialer.dial())
            .await
            .expect("TLS certificate rejection is bounded")
            .err()
            .expect("the native-root WSS connector must reject the self-signed certificate");
        let message = error.to_string().to_ascii_lowercase();
        assert!(
            message.contains("certificate") || message.contains("unknownissuer"),
            "expected a certificate-verification error, got {message}"
        );
        tokio::time::timeout(std::time::Duration::from_secs(3), server)
            .await
            .expect("TLS fixture shuts down")
            .expect("TLS fixture task");
    }
}
// Pins the public paths previous releases published.
//
// The crate split moved these types to `tinychannels-bus`. Re-exporting them
// only from `tinychannels::relay` would silently break every consumer that
// names `tinychannels::relay::transport::…`, which compiles here and fails
// downstream — so name both spellings explicitly.
#[test]
fn the_relay_transport_module_path_still_resolves() {
    use std::any::TypeId;

    // The `transport::` spelling is the one a previous release published.
    assert_eq!(
        TypeId::of::<crate::relay::transport::RelayIdentity>(),
        TypeId::of::<crate::relay::RelayIdentity>(),
    );
    assert_eq!(
        TypeId::of::<crate::relay::transport::RelayTransportError>(),
        TypeId::of::<crate::relay::RelayTransportError>(),
    );
    assert_eq!(
        TypeId::of::<crate::relay::transport::RelayTransportTimeouts>(),
        TypeId::of::<crate::relay::RelayTransportTimeouts>(),
    );
    assert_eq!(
        TypeId::of::<crate::relay::transport::RelayReconnectPolicy>(),
        TypeId::of::<crate::relay::RelayReconnectPolicy>(),
    );

    // Traits have no `TypeId`; naming them in a path is the assertion.
    fn _accepts(_: &dyn crate::relay::transport::RelayFrameIo) {}
    fn _dialer(_: &dyn crate::relay::transport::RelayFrameDialer) {}
    fn _inbound(_: &dyn crate::relay::transport::RelayInboundHandler) {}
    fn _passthrough(_: &dyn crate::relay::transport::RelayPassthroughHandler) {}
    fn _interrupt(_: &dyn crate::relay::transport::RelayInterruptInboundHandler) {}
}

#[tokio::test]
async fn dropping_relay_transport_releases_owned_reader() {
    let io = Arc::new(MemoryRelayIo::default());
    let weak = Arc::downgrade(&io);
    let transport = RelayTransport::new(vec![], io.clone(), RelayTransportTimeouts::default());
    transport.connect().await.unwrap();
    tokio::task::yield_now().await;
    drop(transport);
    drop(io);
    tokio::time::timeout(Duration::from_secs(1), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("reader retained transport I/O after owner drop");
}

#[tokio::test]
async fn relay_shutdown_joins_reader_and_refuses_reconnect() {
    let io = Arc::new(MemoryRelayIo::default());
    let transport = Arc::new(RelayTransport::new(
        vec![],
        io.clone(),
        RelayTransportTimeouts::default(),
    ));
    transport.connect().await.unwrap();
    transport.shutdown().await;
    assert!(transport.is_closed());
    assert_eq!(Arc::strong_count(&io), 2);
    assert_eq!(transport.connect().await, Err(RelayTransportError::Closed));
    let supervisor = transport.spawn_reconnect_supervisor(
        Arc::new(QueueDialer::default()),
        RelayReconnectPolicy::default(),
    );
    supervisor.shutdown().await;
}

#[test]
fn relay_send_action_preserves_ordered_typed_parts_in_payload_metadata() {
    let parts = ["first", "second"]
        .into_iter()
        .map(|text| tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Text {
                text: text.into(),
                markdown_dialect: None,
            },
        })
        .collect::<Vec<_>>();
    let intent = ChannelOutboundIntent {
        idempotency_key: "typed-relay-key".into(),
        payload: OutboundPayload::Parts {
            parts: parts.clone(),
        },
        ..Default::default()
    };
    let action = relay_send_action_from_outbound_intent(&intent);
    assert_eq!(
        action["metadata"]["payload"]["parts"],
        serde_json::to_value(&parts).unwrap()
    );
    assert_eq!(action["metadata"]["idempotency_key"], "typed-relay-key");
    assert_eq!(action["content"], "");
}
