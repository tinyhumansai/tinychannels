#![allow(
    deprecated,
    reason = "integration fixtures still exercise one-release v21 adapters"
)]

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use std::collections::BTreeMap;
use std::sync::Arc;
use tinybus::Result;
use tinybus::test_support::{admit_module, start_bus, wait_until_serving};
use tinychannels::relay::{CapabilityDescriptor, ConnectorToGatewayFrame, GatewayToConnectorFrame};
use tinychannels_bus::module_email::EmailSendRequest;
use tinychannels_bus::module_relay::RelayReservation;
use tinychannels_bus::{ChannelOutboundIntent, ChannelsConfig, OutboundPayload, SendMessage};
use tinystoragedrivers_api::{DocumentStoreExt, StorageBackend};
use tinystoragedrivers_busproxy::{PerCallerEndpoint, StorageServer, serve};
use tinystoragedrivers_testkit::MemoryBackend;
use tinystoragedrivers_types::{Query, Scope};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone)]
struct Callback(
    Arc<Mutex<Vec<serde_json::Value>>>,
    Arc<Mutex<DeliveryState>>,
);

#[derive(Default)]
struct DeliveryState {
    pending: BTreeMap<String, tinychannels_bus::DeliveryQueueEntry>,
    failed: BTreeMap<String, tinychannels_bus::DeliveryQueueEntry>,
    results: BTreeMap<String, tinychannels_bus::DeliveryTerminalResult>,
}

fn projected_text_context(
    events: &tinychannels_bus::WebOutputEvents,
) -> &tinychannels_bus::WebEventContext {
    let tinychannels_bus::WebChannelEvent::TextDelta { .. } = &events.events[0] else {
        panic!("expected text delta from loaded module: {events:?}");
    };
    events.events[0].context()
}

#[tinybus::interface(name = "ai.tinyhumans.tinychannels.ChannelsHost")]
impl Callback {
    async fn deliver_inbound(&self, channel: String, message: serde_json::Value) -> Result<()> {
        self.0
            .lock()
            .await
            .push(serde_json::json!({"kind":"inbound","channel":channel,"message":message}));
        Ok(())
    }

    async fn report_status(
        &self,
        channel: String,
        state: String,
        detail: Option<String>,
    ) -> Result<()> {
        self.0.lock().await.push(
            serde_json::json!({"kind":"status","channel":channel,"state":state,"detail":detail}),
        );
        Ok(())
    }

    async fn persist_allowed_identity(&self, channel: String, identity: String) -> Result<()> {
        self.0
            .lock()
            .await
            .push(serde_json::json!({"kind":"allowlist","channel":channel,"identity":identity}));
        Ok(())
    }

    async fn save_pending_delivery(
        &self,
        entry: tinychannels_bus::DeliveryQueueEntry,
    ) -> Result<()> {
        let mut state = self.1.lock().await;
        if state.pending.len() >= tinychannels_bus::module_delivery::DELIVERY_PENDING_CAPACITY
            && !state.pending.contains_key(&entry.id)
        {
            return Err(tinybus::Error::failed("artifact delivery queue is full"));
        }
        if state
            .pending
            .get(&entry.id)
            .is_some_and(|existing| existing.intent != entry.intent)
        {
            return Err(tinybus::Error::failed("artifact delivery key conflicts"));
        }
        state.pending.insert(entry.id.clone(), entry);
        Ok(())
    }

    async fn load_pending_delivery(
        &self,
        id: String,
    ) -> Result<Option<tinychannels_bus::DeliveryQueueEntry>> {
        Ok(self.1.lock().await.pending.get(&id).cloned())
    }

    async fn load_pending_deliveries(&self) -> Result<Vec<tinychannels_bus::DeliveryQueueEntry>> {
        Ok(self.1.lock().await.pending.values().cloned().collect())
    }

    async fn remove_pending_delivery(&self, id: String) -> Result<()> {
        self.1.lock().await.pending.remove(&id);
        Ok(())
    }

    async fn save_failed_delivery(
        &self,
        entry: tinychannels_bus::DeliveryQueueEntry,
    ) -> Result<()> {
        self.1.lock().await.failed.insert(entry.id.clone(), entry);
        Ok(())
    }

    async fn load_failed_delivery(
        &self,
        id: String,
    ) -> Result<Option<tinychannels_bus::DeliveryQueueEntry>> {
        Ok(self.1.lock().await.failed.get(&id).cloned())
    }

    async fn save_delivery_result(
        &self,
        result: tinychannels_bus::DeliveryTerminalResult,
    ) -> Result<()> {
        let mut state = self.1.lock().await;
        if state.results.len() >= tinychannels_bus::module_delivery::DELIVERY_RESULT_CAPACITY
            && !state.results.contains_key(&result.id)
        {
            return Err(tinybus::Error::failed("artifact result table is full"));
        }
        if state
            .results
            .get(&result.id)
            .is_some_and(|existing| existing != &result)
        {
            return Err(tinybus::Error::failed("artifact result key conflicts"));
        }
        state.results.insert(result.id.clone(), result);
        Ok(())
    }

    async fn load_delivery_result(
        &self,
        id: String,
    ) -> Result<Option<tinychannels_bus::DeliveryTerminalResult>> {
        Ok(self.1.lock().await.results.get(&id).cloned())
    }

    async fn dispatch_controller_backend(
        &self,
        request: tinychannels_bus::ChannelBackendRequest,
    ) -> Result<tinychannels_bus::ChannelBackendResult> {
        let tinychannels_bus::ChannelBackendRequest::SendReaction { channel, .. } = request else {
            return Err(tinybus::Error::failed("unexpected backend fixture request"));
        };
        self.0.lock().await.push(serde_json::json!({
            "kind":"controller",
            "channel":channel,
        }));
        Ok(tinychannels_bus::ChannelBackendResult::SendReaction(
            tinychannels_bus::controllers::ChannelReactionResult {
                success: true,
                message: None,
                raw: None,
            },
        ))
    }
}

#[tokio::test]
#[ignore = "run through scripts/verify-module-artifact.sh"]
async fn compiled_artifact_exposes_confidential_controller_proxy_and_websocket_lifecycle() {
    let (module_host, connection, _broker_task) = start_bus().await.unwrap();
    let info = admit_module(
        &module_host,
        "TINYCHANNELS_MODULE_ARTIFACT",
        "tinychannels-module",
    )
    .unwrap();
    assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(tinychannels_bus::CONTRACT_VERSION, 33);
    let interface = info
        .manifest
        .provides
        .iter()
        .find(|provided| {
            provided.version.interface.as_str() == "ai.tinyhumans.tinychannels.Channels"
        })
        .unwrap();
    let members: Vec<_> = interface.methods.iter().map(ToString::to_string).collect();
    let contract: Vec<_> = tinychannels_bus::METHODS
        .iter()
        .map(|method| method.to_string())
        .collect();
    assert_eq!(
        members, contract,
        "compiled artifact must preserve the bus registry membership and declaration order"
    );

    // Delivery records live in the host's storage, which the module reaches over
    // the storage bus; serve an in-memory backend for it.
    let storage = Arc::new(MemoryBackend::new());
    let served_backend = Arc::clone(&storage);
    let endpoint = PerCallerEndpoint::new(move |_sender| {
        let scoped = served_backend
            .for_scope(&Scope::new("tenant:module-artifact").ok()?)
            .ok()?;
        Some(StorageServer::new(scoped))
    });
    let _storage_host = serve(&connection, Arc::new(endpoint)).await.unwrap();

    let callbacks = Callback(
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(DeliveryState::default())),
    );
    connection
        .serve_at(
            tinychannels_bus::HOST_OBJECT_PATH.try_into().unwrap(),
            callbacks.clone(),
        )
        .await
        .unwrap();
    connection
        .request_name(tinychannels_bus::HOST_BUS_NAME)
        .await
        .unwrap();
    wait_until_serving(
        &connection,
        tinychannels_bus::BUS_NAME,
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap();
    let proxy = connection
        .proxy(
            tinychannels_bus::BUS_NAME,
            tinychannels_bus::OBJECT_PATH,
            tinychannels_bus::BUS_NAME,
        )
        .unwrap();
    let web_context = |client_id: &str| tinychannels_bus::WebEventContext {
        client_id: client_id.into(),
        thread_id: "same-thread".into(),
        request_id: "same-request".into(),
        turn_request_id: None,
        seq: None,
        ts: None,
    };
    let begin_web_turn = |client_id: &str, scope_key: &str| tinychannels_bus::BeginWebTurn {
        scope_key: scope_key.into(),
        context: web_context(client_id),
    };
    assert_eq!(
        proxy
            .call::<tinychannels_bus::WebTurnHandle>(
                "BeginWebTurn",
                begin_web_turn("same-client", "tenant-a"),
            )
            .await
            .unwrap_err()
            .wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED
    );
    let tenant_a: tinychannels_bus::WebTurnHandle = proxy
        .call_confidential("BeginWebTurn", begin_web_turn("same-client", "tenant-a"))
        .await
        .unwrap();
    let tenant_b: tinychannels_bus::WebTurnHandle = proxy
        .call_confidential("BeginWebTurn", begin_web_turn("same-client", "tenant-b"))
        .await
        .unwrap();
    let project =
        |handle: &tinychannels_bus::WebTurnHandle, text: &str| tinychannels_bus::ProjectWebOutput {
            handle: handle.handle.clone(),
            event: tinychannels_bus::module_progressive::ChannelOutputEvent::TextDelta {
                text: text.into(),
                iteration: Some(1),
            },
        };
    let tenant_a_first: tinychannels_bus::WebOutputEvents = proxy
        .call_confidential("ProjectWebOutput", project(&tenant_a, "first"))
        .await
        .unwrap();
    let tenant_a_second: tinychannels_bus::WebOutputEvents = proxy
        .call_confidential("ProjectWebOutput", project(&tenant_a, "second"))
        .await
        .unwrap();
    let tenant_b_first: tinychannels_bus::WebOutputEvents = proxy
        .call_confidential("ProjectWebOutput", project(&tenant_b, "isolated"))
        .await
        .unwrap();
    assert_eq!(projected_text_context(&tenant_a_first).seq, Some(1));
    assert_eq!(projected_text_context(&tenant_a_second).seq, Some(2));
    assert_eq!(projected_text_context(&tenant_b_first).seq, Some(1));
    assert_eq!(
        projected_text_context(&tenant_a_first).client_id,
        "same-client"
    );
    assert!(projected_text_context(&tenant_a_first).ts.is_some());
    let project_concurrently = |text: &'static str| {
        let connection = connection.clone();
        let handle = tenant_a.clone();
        async move {
            let proxy = connection
                .proxy(
                    tinychannels_bus::BUS_NAME,
                    tinychannels_bus::OBJECT_PATH,
                    tinychannels_bus::BUS_NAME,
                )
                .unwrap();
            let request = tinychannels_bus::ProjectWebOutput {
                handle: handle.handle,
                event: tinychannels_bus::module_progressive::ChannelOutputEvent::TextDelta {
                    text: text.into(),
                    iteration: Some(1),
                },
            };
            proxy
                .call_confidential::<tinychannels_bus::WebOutputEvents>("ProjectWebOutput", request)
                .await
        }
    };
    let (left, right) = tokio::join!(project_concurrently("left"), project_concurrently("right"));
    let left = left.unwrap();
    let right = right.unwrap();
    let mut concurrent_sequences = [
        projected_text_context(&left).seq.unwrap(),
        projected_text_context(&right).seq.unwrap(),
    ];
    concurrent_sequences.sort_unstable();
    assert_eq!(concurrent_sequences, [3, 4]);
    let end_proxy = connection
        .proxy(
            tinychannels_bus::BUS_NAME,
            tinychannels_bus::OBJECT_PATH,
            tinychannels_bus::BUS_NAME,
        )
        .unwrap();
    let end = end_proxy.call_confidential::<()>(
        "EndWebTurn",
        tinychannels_bus::EndWebTurn {
            handle: tenant_a.handle.clone(),
        },
    );
    let end_race_project = end_proxy.call_confidential::<tinychannels_bus::WebOutputEvents>(
        "ProjectWebOutput",
        project(&tenant_a, "end race"),
    );
    let (end_result, project_result) = tokio::join!(end, end_race_project);
    end_result.unwrap();
    let late_after_end = proxy
        .call_confidential::<tinychannels_bus::WebOutputEvents>(
            "ProjectWebOutput",
            project(&tenant_a, "late after end"),
        )
        .await
        .unwrap_err();
    match project_result {
        Ok(projected) => assert_eq!(projected_text_context(&projected).seq, Some(5)),
        Err(error) => assert_eq!(error.wire_name(), late_after_end.wire_name()),
    }
    proxy
        .call_confidential::<()>(
            "EndWebTurn",
            tinychannels_bus::EndWebTurn {
                handle: tenant_a.handle.clone(),
            },
        )
        .await
        .unwrap();
    assert!(
        proxy
            .call_confidential::<tinychannels_bus::WebOutputEvents>(
                "ProjectWebOutput",
                project(&tenant_a, "late"),
            )
            .await
            .is_err()
    );
    let terminal: tinychannels_bus::WebOutputEvents = proxy
        .call_confidential(
            "ProjectWebOutput",
            tinychannels_bus::ProjectWebOutput {
                handle: tenant_b.handle.clone(),
                event: tinychannels_bus::module_progressive::ChannelOutputEvent::FinalMessage {
                    text: "done".into(),
                    parts: Vec::new(),
                    receipt: None,
                },
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        terminal.events.as_slice(),
        [tinychannels_bus::WebChannelEvent::ChatDone { .. }]
    ));
    assert!(
        proxy
            .call_confidential::<tinychannels_bus::WebOutputEvents>(
                "ProjectWebOutput",
                project(&tenant_b, "late after terminal"),
            )
            .await
            .is_err()
    );
    proxy
        .call_confidential::<()>(
            "EndWebTurn",
            tinychannels_bus::EndWebTurn {
                handle: tenant_b.handle.clone(),
            },
        )
        .await
        .unwrap();
    // Identity scopes use separate objects and preserve all callback arities.
    assert_eq!(
        proxy
            .call::<tinychannels_bus::ChannelRuntimeReservation>("ReserveRuntime", ())
            .await
            .unwrap_err()
            .wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED
    );
    let first: tinychannels_bus::ChannelRuntimeReservation =
        proxy.call_confidential("ReserveRuntime", ()).await.unwrap();
    let second: tinychannels_bus::ChannelRuntimeReservation =
        proxy.call_confidential("ReserveRuntime", ()).await.unwrap();
    let first_callbacks = Callback(
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(DeliveryState::default())),
    );
    let second_callbacks = Callback(
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(DeliveryState::default())),
    );
    connection
        .serve_at(
            first.host_object_path.clone().try_into().unwrap(),
            first_callbacks.clone(),
        )
        .await
        .unwrap();
    connection
        .serve_at(
            second.host_object_path.clone().try_into().unwrap(),
            second_callbacks.clone(),
        )
        .await
        .unwrap();
    for scope in [&first, &second] {
        assert_eq!(
            proxy
                .call::<()>("StartRuntime", scope.handle.clone())
                .await
                .unwrap_err()
                .wire_name(),
            tinybus::Error::CONFIDENTIALITY_REQUIRED
        );
        proxy
            .call_confidential::<()>("StartRuntime", scope.handle.clone())
            .await
            .unwrap();
    }
    let first_proxy = connection
        .proxy(
            tinychannels_bus::BUS_NAME,
            &first.object_path,
            tinychannels_bus::BUS_NAME,
        )
        .unwrap();
    let second_proxy = connection
        .proxy(
            tinychannels_bus::BUS_NAME,
            &second.object_path,
            tinychannels_bus::BUS_NAME,
        )
        .unwrap();
    let nested = first_proxy
        .call_confidential::<tinychannels_bus::ChannelRuntimeReservation>("ReserveRuntime", ())
        .await
        .unwrap_err();
    assert_eq!(nested.wire_name(), tinybus::Error::FAILED);
    assert_eq!(
        nested.wire_message(),
        "nested channel runtimes are unavailable"
    );

    for (scoped, key) in [
        (&first_proxy, "first-scope"),
        (&second_proxy, "second-scope"),
    ] {
        let result = scoped
            .call_confidential::<tinychannels_bus::ChannelControllerResult>(
                "ChannelController",
                tinychannels_bus::ChannelControllerRequest::SendReaction {
                    config: ChannelsConfig::default(),
                    channel: "telegram".into(),
                    reaction: serde_json::json!({"message_id":key,"emoji":"ok"}),
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            result,
            tinychannels_bus::ChannelControllerResult::SendReaction(_)
        ));
        let intent = ChannelOutboundIntent {
            idempotency_key: key.into(),
            channel_id: "not-running".into(),
            turn_id: None,
            durability: tinychannels_bus::DeliveryDurability::Required,
            payload: OutboundPayload::Text {
                text: "scoped delivery".into(),
            },
            ..Default::default()
        };
        assert!(
            scoped
                .call::<tinychannels_bus::controllers::ChannelSendMessageResult>(
                    "SendChannelIntent",
                    ("not-running", intent)
                )
                .await
                .is_err()
        );
    }
    assert_eq!(first_callbacks.0.lock().await.len(), 1);
    assert_eq!(second_callbacks.0.lock().await.len(), 1);
    assert!(callbacks.0.lock().await.is_empty());
    // The module no longer calls per-record delivery callbacks on the host...
    for host in [&first_callbacks, &second_callbacks, &callbacks] {
        assert!(host.1.lock().await.pending.is_empty());
    }
    // ...it writes both scopes' required deliveries to the host's storage.
    let docs = storage
        .for_scope(&Scope::new("tenant:module-artifact").unwrap())
        .unwrap();
    let mut pending: Vec<String> = docs
        .documents()
        .query_all(
            tinychannels::storage::delivery::PENDING_COLLECTION,
            &Query::all(),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|found| found.id)
        .collect();
    pending.sort();
    assert_eq!(pending, ["first-scope", "second-scope"]);
    assert_eq!(
        proxy
            .call::<()>("StopRuntime", first.handle.clone())
            .await
            .unwrap_err()
            .wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED
    );
    proxy
        .call_confidential::<()>("StopRuntime", first.handle.clone())
        .await
        .unwrap();
    assert!(
        first_proxy
            .call::<Vec<String>>("ListChannels", ())
            .await
            .is_err()
    );
    let _: Vec<String> = second_proxy.call("ListChannels", ()).await.unwrap();
    proxy
        .call_confidential::<()>("StopRuntime", second.handle.clone())
        .await
        .unwrap();
    proxy
        .call_confidential::<()>("StopRuntime", first.handle)
        .await
        .unwrap();

    let interaction = tinychannels_bus::ChannelInteractionRequest {
        channel: "missing".into(),
        interaction: tinychannels_bus::ChannelInteraction::StartTyping {
            recipient: "room".into(),
        },
    };
    assert_eq!(
        proxy
            .call::<tinychannels_bus::ChannelInteractionResult>(
                "ChannelInteraction",
                interaction.clone()
            )
            .await
            .unwrap_err()
            .wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED
    );
    assert_eq!(
        proxy
            .call_confidential::<tinychannels_bus::ChannelInteractionResult>(
                "ChannelInteraction",
                interaction
            )
            .await
            .unwrap_err()
            .wire_name(),
        "ai.tinyhumans.tinychannels.Error.UnknownChannel"
    );
    assert_eq!(
        proxy
            .call::<Vec<tinychannels_bus::ChannelHealthReport>>(
                tinychannels_bus::methods::CHECK_CHANNEL_HEALTH,
                (
                    ChannelsConfig::default(),
                    tinychannels_bus::HttpProxyPolicy::default(),
                    10_000_u64
                ),
            )
            .await
            .unwrap_err()
            .wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED
    );
    let health: Vec<tinychannels_bus::ChannelHealthReport> = proxy
        .call_confidential(
            tinychannels_bus::methods::CHECK_CHANNEL_HEALTH,
            (
                ChannelsConfig::default(),
                tinychannels_bus::HttpProxyPolicy::default(),
                10_000_u64,
            ),
        )
        .await
        .unwrap();
    assert!(
        health.is_empty(),
        "empty configuration must not start providers"
    );
    assert!(
        proxy
            .call_confidential::<Vec<tinychannels_bus::ChannelHealthReport>>(
                tinychannels_bus::methods::CHECK_CHANNEL_HEALTH,
                (
                    ChannelsConfig::default(),
                    tinychannels_bus::HttpProxyPolicy::default(),
                    0_u64
                ),
            )
            .await
            .is_err()
    );

    // The configured member stays callable through the loaded artifact. An
    // absent channel gives the typed configuration error before any provider
    // or network resource is created.
    let configured_args = (
        "missing",
        ChannelsConfig::default(),
        tinychannels_bus::HttpProxyPolicy::default(),
    );
    assert_eq!(
        proxy
            .call::<()>("StartChannelConfigured", configured_args.clone())
            .await
            .unwrap_err()
            .wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED
    );
    let error = proxy
        .call_confidential::<()>("StartChannelConfigured", configured_args)
        .await
        .unwrap_err();
    assert_eq!(
        error.wire_name(),
        "ai.tinyhumans.tinychannels.Error.NotConfigured"
    );
    let prepared: tinychannels_bus::ChannelControllerResult = proxy
        .call_confidential("ChannelController", tinychannels_bus::ChannelControllerRequest::PrepareConnect {
            config: ChannelsConfig::default(), channel: "discord".into(),
            auth_mode: tinychannels_bus::ChannelAuthMode::BotToken,
            credentials: serde_json::json!({"bot_token":"fixture-token", "allowed_users":"@Alice, ALICE"}),
            proxy: Default::default(),
        }).await.unwrap();
    let tinychannels_bus::ChannelControllerResult::PreparedConnect(prepared) = prepared else {
        panic!("compiled artifact returned the wrong prepared connection result");
    };
    let Some(tinychannels_bus::module_controller::ChannelConfigUpdate::Discord(config)) =
        prepared.config_update
    else {
        panic!("compiled artifact did not prepare the provider configuration");
    };
    assert_eq!(config.allowed_users, ["alice"]);
    assert_eq!(prepared.result.status, "connected");
    let controller_request = tinychannels_bus::ChannelControllerRequest::TestChannel {
        config: ChannelsConfig::default(),
        channel: "telegram".into(),
        auth_mode: tinychannels_bus::controllers::ChannelAuthMode::BotToken,
        credentials: serde_json::json!({"bot_token":"fixture-secret"}),
    };
    let connect_request = tinychannels_bus::ChannelControllerRequest::Connect {
        config: ChannelsConfig::default(),
        channel: "artifact-missing-channel".into(),
        auth_mode: tinychannels_bus::controllers::ChannelAuthMode::ApiKey,
        credentials: serde_json::json!({}),
        proxy: tinychannels_bus::HttpProxyPolicy {
            https_proxy: Some("http://fixture-proxy.invalid:8080".into()),
            service_selectors: vec!["channel.yuanbao".into()],
            ..Default::default()
        },
    };
    assert_eq!(
        proxy
            .call::<tinychannels_bus::ChannelControllerResult>(
                "ChannelController",
                connect_request.clone(),
            )
            .await
            .err()
            .unwrap()
            .wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED,
        "connect credentials and proxy settings must use the confidential module operation"
    );
    let supervised_request = (
        "artifact-missing-channel".to_owned(),
        ChannelsConfig::default(),
        tinychannels_bus::HttpProxyPolicy::default(),
        tinychannels_bus::ChannelSupervisionPolicy {
            initial_backoff_secs: 86_401,
            max_backoff_secs: 86_401,
        },
    );
    assert_eq!(
        proxy
            .call::<()>("StartChannelSupervised", supervised_request.clone())
            .await
            .err()
            .unwrap()
            .wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED
    );
    let supervised_error = proxy
        .call_confidential::<()>("StartChannelSupervised", supervised_request)
        .await
        .err()
        .unwrap();
    assert_eq!(supervised_error.wire_name(), tinybus::Error::FAILED);
    assert_eq!(
        supervised_error.wire_message(),
        "provider retry delay exceeds one day"
    );
    let connect_error = match proxy
        .call_confidential::<tinychannels_bus::ChannelControllerResult>(
            "ChannelController",
            connect_request,
        )
        .await
    {
        Ok(_) => panic!("unknown channel must be rejected by controller dispatch"),
        Err(error) => error,
    };
    assert_eq!(connect_error.wire_name(), tinybus::Error::FAILED);
    assert_eq!(
        connect_error.wire_message(),
        "unknown channel: artifact-missing-channel",
        "the confidential call must reach module controller validation"
    );
    assert_eq!(
        proxy
            .call::<tinychannels_bus::ChannelControllerResult>(
                "ChannelController",
                controller_request.clone(),
            )
            .await
            .err()
            .unwrap()
            .wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED,
        "controller credentials must use the confidential module operation"
    );
    assert!(matches!(
        proxy
            .call_confidential::<tinychannels_bus::ChannelControllerResult>(
                "ChannelController",
                controller_request,
            )
            .await
            .unwrap(),
        tinychannels_bus::ChannelControllerResult::TestChannel(
            tinychannels_bus::controllers::ChannelTestResult { success: true, .. }
        )
    ));
    let discord_request = tinychannels_bus::ChannelControllerRequest::DiscordListGuilds {
        config: ChannelsConfig::default(),
        bot_token: "artifact-token".into(),
        proxy: tinychannels_bus::HttpProxyPolicy {
            https_proxy: Some("http://user:password@not a valid proxy".into()),
            service_selectors: vec!["channel.discord".into()],
            ..Default::default()
        },
    };
    let ordinary_discord_error = match proxy
        .call::<tinychannels_bus::ChannelControllerResult>(
            "ChannelController",
            discord_request.clone(),
        )
        .await
    {
        Ok(_) => panic!("confidential Discord request must reject ordinary calls"),
        Err(error) => error,
    };
    assert_eq!(
        ordinary_discord_error.wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED,
        "Discord credentials and host proxy settings must use the confidential module operation"
    );
    let discord_error = match proxy
        .call_confidential::<tinychannels_bus::ChannelControllerResult>(
            "ChannelController",
            discord_request,
        )
        .await
    {
        Ok(_) => panic!("invalid proxy URL must be rejected by controller dispatch"),
        Err(error) => error,
    };
    assert_eq!(discord_error.wire_name(), tinybus::Error::FAILED);
    assert_eq!(
        discord_error.wire_message(),
        "invalid HTTPS proxy URL",
        "the compiled module must dispatch the Discord request through configured proxy validation"
    );
    assert!(matches!(
        proxy
            .call_confidential::<tinychannels_bus::ChannelControllerResult>(
                "ChannelController",
                tinychannels_bus::ChannelControllerRequest::SendReaction {
                    config: ChannelsConfig::default(),
                    channel: "discord".into(),
                    reaction: serde_json::json!({"message_id":"artifact-message","emoji":"✅"}),
                },
            )
            .await
            .unwrap(),
        tinychannels_bus::ChannelControllerResult::SendReaction(
            tinychannels_bus::controllers::ChannelReactionResult { success: true, .. }
        )
    ));
    // Exercise every original argument shape on the loaded cdylib. The
    // expected NotConfigured/Unknown outcomes prove dispatch reached handlers.
    assert!(
        proxy
            .call::<()>("StartChannel", ("missing", ChannelsConfig::default()))
            .await
            .is_err()
    );
    proxy.call::<()>("StopChannel", ("missing",)).await.unwrap();
    assert!(
        proxy
            .call::<()>(
                "SendMessage",
                ("missing", SendMessage::new("message", "recipient")),
            )
            .await
            .is_err()
    );
    let _: Vec<String> = proxy.call("ListChannels", ()).await.unwrap();
    let status: String = proxy.call("ChannelStatus", ("missing",)).await.unwrap();
    assert_eq!(status, "stopped");
    let required = ChannelOutboundIntent {
        idempotency_key: "artifact-required".into(),
        channel_id: "not-running".into(),
        turn_id: None,
        durability: tinychannels::DeliveryDurability::Required,
        payload: OutboundPayload::Text {
            text: "artifact fixture".into(),
        },
        ..Default::default()
    };
    assert!(
        proxy
            .call::<tinychannels_bus::controllers::ChannelSendMessageResult>(
                "SendChannelIntent",
                ("not-running", required.clone()),
            )
            .await
            .is_err()
    );
    assert!(
        storage
            .for_scope(&Scope::new("tenant:module-artifact").unwrap())
            .unwrap()
            .documents()
            .get(
                tinychannels::storage::delivery::PENDING_COLLECTION,
                "artifact-required"
            )
            .await
            .unwrap()
            .is_some()
    );
    let mut conflict = required;
    conflict.payload = OutboundPayload::Text {
        text: "different artifact fixture".into(),
    };
    assert!(
        proxy
            .call::<tinychannels_bus::controllers::ChannelSendMessageResult>(
                "SendChannelIntent",
                ("not-running", conflict),
            )
            .await
            .is_err()
    );
    let (url, remote) = relay_server().await;
    assert!(
        proxy
            .call::<tinychannels_bus::controllers::ChannelSendMessageResult>(
                "SendEmail",
                EmailSendRequest {
                    config: Default::default(),
                    recipient: "recipient@example.test".into(),
                    subject: "test".into(),
                    body: "test".into(),
                    attachments: Vec::new(),
                },
            )
            .await
            .is_err(),
        "ordinary delivery must be refused for confidential email"
    );

    let (smtp_port, smtp_started, smtp_release, near_limit_smtp_server) = smtp_server().await;
    let attachment = vec![u8::MAX; tinychannels_bus::module_email::EMAIL_ATTACHMENT_BYTES];
    let upload: tinychannels_bus::EmailUploadHandle = proxy
        .call_confidential(
            "BeginEmailUpload",
            tinychannels_bus::EmailUploadBegin {
                config: tinychannels_bus::config::EmailConfig {
                    smtp_host: "127.0.0.1".into(),
                    smtp_port,
                    smtp_tls: false,
                    from_address: "from@example.test".into(),
                    ..Default::default()
                },
                recipient: "to@example.test".into(),
                subject: "near limit".into(),
                body: "fixture".into(),
                attachments: vec![tinychannels_bus::EmailAttachmentUpload {
                    filename: "near-limit.bin".into(),
                    content_type: "application/octet-stream".into(),
                    byte_count: attachment.len(),
                }],
            },
        )
        .await
        .unwrap();
    let mut first_chunk = None;
    for (sequence, bytes) in attachment
        .chunks(tinychannels_bus::module_email::EMAIL_UPLOAD_CHUNK_BYTES)
        .enumerate()
    {
        let chunk = tinychannels_bus::EmailUploadChunk {
            handle: upload.clone(),
            attachment_index: 0,
            sequence: sequence as u32,
            bytes: bytes.to_vec(),
        };
        let next: u32 = proxy
            .call_confidential("AppendEmailUploadChunk", chunk.clone())
            .await
            .unwrap();
        assert_eq!(next, sequence as u32 + 1);
        if sequence == 0 {
            first_chunk = Some(chunk);
        }
    }
    let first_chunk = first_chunk.unwrap();
    let replay: u32 = proxy
        .call_confidential("AppendEmailUploadChunk", first_chunk.clone())
        .await
        .unwrap();
    assert_eq!(
        replay,
        attachment
            .len()
            .div_ceil(tinychannels_bus::module_email::EMAIL_UPLOAD_CHUNK_BYTES) as u32,
        "old lost chunk acknowledgements replay safely"
    );
    let mut conflict = first_chunk;
    conflict.bytes[0] = 0;
    assert!(
        proxy
            .call_confidential::<u32>("AppendEmailUploadChunk", conflict)
            .await
            .is_err()
    );
    let send_waiter = tokio::spawn({
        let proxy = proxy.clone();
        let upload = upload.clone();
        async move {
            proxy
                .call_confidential::<tinychannels_bus::controllers::ChannelSendMessageResult>(
                    "SendEmailUpload",
                    upload,
                )
                .await
        }
    });
    smtp_started.await.unwrap();
    send_waiter.abort();
    let _ = send_waiter.await;
    let abort_waiter = tokio::spawn({
        let proxy = proxy.clone();
        let upload = upload.clone();
        async move {
            proxy
                .call_confidential::<()>("AbortEmailUpload", upload)
                .await
        }
    });
    tokio::task::yield_now().await;
    assert!(
        !abort_waiter.is_finished(),
        "abort waits for accepted SMTP work"
    );
    smtp_release.send(()).unwrap();
    assert!(abort_waiter.await.unwrap().is_err());
    let _: tinychannels_bus::controllers::ChannelSendMessageResult = proxy
        .call_confidential("SendEmailUpload", upload)
        .await
        .unwrap();
    let sent_mime = near_limit_smtp_server.await.unwrap();
    assert_eq!(
        sent_mime
            .iter()
            .filter(|line| line.starts_with("DATA"))
            .count(),
        1
    );
    assert_eq!(
        mime_attachment_bytes(&sent_mime, "near-limit.bin"),
        attachment
    );
    let reservation: RelayReservation = proxy.call("ReserveRelay", ()).await.unwrap();
    let config = tinychannels_bus::config::RelayRuntimeConfig {
        url,
        identities: vec![tinychannels_bus::config::RelayRuntimeIdentityConfig {
            platform: "telegram".into(),
            bot_id: "artifact-fixture".into(),
        }],
        ..Default::default()
    };
    assert!(
        proxy
            .call::<CapabilityDescriptor>("StartRelay", (reservation.clone(), config.clone()))
            .await
            .is_err(),
        "ordinary delivery must be refused for confidential startup"
    );
    let descriptor: CapabilityDescriptor = proxy
        .call_confidential("StartRelay", (reservation.clone(), config))
        .await
        .unwrap();
    assert_eq!(descriptor.platform, "telegram");
    let result: tinychannels_bus::controllers::ChannelSendMessageResult = proxy
        .call(
            "SendRelayIntent",
            (
                reservation.clone(),
                ChannelOutboundIntent {
                    channel_id: "telegram".into(),
                    conversation_id: "chat-1".into(),
                    channel_ref: None,
                    conversation_ref: None,
                    payload: OutboundPayload::Text {
                        text: "artifact send".into(),
                    },
                    ..Default::default()
                },
            ),
        )
        .await
        .unwrap();
    assert_eq!(result.message_id.as_deref(), Some("artifact-message"));
    proxy.call::<()>("StopRelay", (reservation,)).await.unwrap();

    // A v2-only peer responds to `hello_v2` without ever sending a v1
    // descriptor. The loaded module must expose the same canonical descriptor
    // over its TinyBus operation, correlated to the configured identity.
    let (v2_url, v2_peer) = relay_server_v2().await;
    let v2_reservation: RelayReservation = proxy.call("ReserveRelay", ()).await.unwrap();
    let v2_config = tinychannels_bus::config::RelayRuntimeConfig {
        url: v2_url,
        identities: vec![tinychannels_bus::config::RelayRuntimeIdentityConfig {
            platform: "telegram".into(),
            bot_id: "v2-fixture".into(),
        }],
        ..Default::default()
    };
    let v2: tinychannels_bus::module_relay::RelayCapabilitiesV2 = proxy
        .call_confidential("StartRelayV2", (v2_reservation.clone(), v2_config))
        .await
        .unwrap();
    assert_eq!(v2.contract_version, 2);
    assert_eq!(v2.instances.len(), 1);
    assert_eq!(v2.instances[0].platform, "telegram");
    assert_eq!(v2.instances[0].bot_id, "v2-fixture");
    assert!(v2.instances[0].capabilities.progressive_edits);
    let queried: Option<tinychannels_bus::module_relay::RelayCapabilitiesV2> = proxy
        .call_confidential("RelayCapabilitiesV2", (v2_reservation.clone(),))
        .await
        .unwrap();
    assert_eq!(queried, Some(v2));
    assert!(
        proxy
            .call::<Option<tinychannels_bus::module_relay::RelayCapabilitiesV2>>(
                "RelayCapabilitiesV2",
                (v2_reservation.clone(),),
            )
            .await
            .is_err()
    );
    proxy
        .call::<()>("StopRelay", (v2_reservation,))
        .await
        .unwrap();
    v2_peer.await.unwrap();
    let (smtp_port, smtp_started, smtp_release, smtp_server) = smtp_server().await;
    let shutdown_upload: tinychannels_bus::EmailUploadHandle = proxy
        .call_confidential(
            "BeginEmailUpload",
            tinychannels_bus::EmailUploadBegin {
                config: tinychannels_bus::config::EmailConfig {
                    smtp_host: "127.0.0.1".into(),
                    smtp_port,
                    smtp_tls: false,
                    from_address: "from@example.test".into(),
                    ..Default::default()
                },
                recipient: "to@example.test".into(),
                subject: "shutdown drain".into(),
                body: "fixture".into(),
                attachments: vec![tinychannels_bus::EmailAttachmentUpload {
                    filename: "shutdown.txt".into(),
                    content_type: "text/plain".into(),
                    byte_count: 8,
                }],
            },
        )
        .await
        .unwrap();
    proxy
        .call_confidential::<u32>(
            "AppendEmailUploadChunk",
            tinychannels_bus::EmailUploadChunk {
                handle: shutdown_upload.clone(),
                attachment_index: 0,
                sequence: 0,
                bytes: b"shutdown".to_vec(),
            },
        )
        .await
        .unwrap();
    let send_waiter = tokio::spawn({
        let proxy = proxy.clone();
        async move {
            proxy
                .call_confidential::<tinychannels_bus::controllers::ChannelSendMessageResult>(
                    "SendEmailUpload",
                    shutdown_upload,
                )
                .await
        }
    });
    smtp_started.await.unwrap();
    send_waiter.abort();
    let _ = send_waiter.await;
    let shutdown = tokio::spawn({
        let proxy = proxy.clone();
        async move { proxy.call::<()>("Shutdown", ()).await }
    });
    tokio::task::yield_now().await;
    assert!(!shutdown.is_finished(), "shutdown joins accepted SMTP work");
    smtp_release.send(()).unwrap();
    shutdown.await.unwrap().unwrap();
    let shutdown_mime = smtp_server.await.unwrap();
    assert_eq!(
        mime_attachment_bytes(&shutdown_mime, "shutdown.txt"),
        b"shutdown",
        "SMTP transcript: {shutdown_mime:#?}"
    );
    let _frames = remote.await.unwrap();
    let callback_events = callbacks.0.lock().await;
    assert!(
        callback_events
            .iter()
            .any(|event| event["kind"] == "inbound")
    );
    assert!(
        callback_events
            .iter()
            .any(|event| event["state"] == "connected")
    );
}

#[tokio::test]
#[ignore = "run through scripts/verify-module-artifact.sh"]
async fn compiled_artifact_ingests_scoped_media_and_sends_its_opaque_reference() {
    use tinychannels_bus::{
        ChannelRef, ConversationKind, ConversationRef, MediaKind, MediaReadBegin, MediaReadChunk,
        MediaReadFinish, MediaUploadBegin, MediaUploadChunk, MediaUploadFinish, MediaUploadHandle,
        OutboundContentPart,
        module_media::{MEDIA_UPLOAD_CHUNK_BYTES, MediaUploadResult},
    };
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let (module_host, connection, _broker_task) = start_bus().await.unwrap();
    admit_module(
        &module_host,
        "TINYCHANNELS_MODULE_ARTIFACT",
        "tinychannels-module",
    )
    .unwrap();
    let storage = MemoryBackend::new();
    let served_backend = Arc::new(storage);
    let endpoint = PerCallerEndpoint::new(move |_sender| {
        let scoped = served_backend.for_scope(&Scope::new("local").ok()?).ok()?;
        Some(StorageServer::new(scoped))
    });
    let _storage_host = serve(&connection, Arc::new(endpoint)).await.unwrap();
    let callbacks = Callback(
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(DeliveryState::default())),
    );
    connection
        .serve_at(
            tinychannels_bus::HOST_OBJECT_PATH.try_into().unwrap(),
            callbacks,
        )
        .await
        .unwrap();
    connection
        .request_name(tinychannels_bus::HOST_BUS_NAME)
        .await
        .unwrap();
    wait_until_serving(
        &connection,
        tinychannels_bus::BUS_NAME,
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap();
    let module_proxy = connection
        .proxy(
            tinychannels_bus::BUS_NAME,
            tinychannels_bus::OBJECT_PATH,
            tinychannels_bus::BUS_NAME,
        )
        .unwrap();
    let reservation: tinychannels_bus::ChannelRuntimeReservation = module_proxy
        .call_confidential("ReserveRuntime", ())
        .await
        .unwrap();
    let scoped_callbacks = Callback(
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(DeliveryState::default())),
    );
    connection
        .serve_at(
            reservation.host_object_path.clone().try_into().unwrap(),
            scoped_callbacks,
        )
        .await
        .unwrap();
    module_proxy
        .call_confidential::<()>("StartRuntime", reservation.handle.clone())
        .await
        .unwrap();
    let runtime_proxy = connection
        .proxy(
            tinychannels_bus::BUS_NAME,
            &reservation.object_path,
            tinychannels_bus::BUS_NAME,
        )
        .unwrap();
    let other_reservation: tinychannels_bus::ChannelRuntimeReservation = module_proxy
        .call_confidential("ReserveRuntime", ())
        .await
        .unwrap();
    connection
        .serve_at(
            other_reservation
                .host_object_path
                .clone()
                .try_into()
                .unwrap(),
            Callback(
                Arc::new(Mutex::new(Vec::new())),
                Arc::new(Mutex::new(DeliveryState::default())),
            ),
        )
        .await
        .unwrap();
    module_proxy
        .call_confidential::<()>("StartRuntime", other_reservation.handle.clone())
        .await
        .unwrap();
    let other_runtime_proxy = connection
        .proxy(
            tinychannels_bus::BUS_NAME,
            &other_reservation.object_path,
            tinychannels_bus::BUS_NAME,
        )
        .unwrap();

    let telegram_api = MockServer::start().await;
    // SAFETY: the artifact verifier runs this case in its own test process.
    unsafe {
        std::env::set_var("OPENHUMAN_TELEGRAM_BOT_API_BASE", telegram_api.uri());
    }
    Mock::given(method("POST"))
        .and(path_regex(".*/getUpdates"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": []
        })))
        .mount(&telegram_api)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(".*/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {"message_id": 92, "date": 1, "chat": {"id": 42, "type": "group"}}
        })))
        .mount(&telegram_api)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(".*/editMessageText"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {"message_id": 92, "date": 1, "chat": {"id": 42, "type": "group"}, "text": "progress"}
        })))
        .mount(&telegram_api)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(".*/sendPhoto"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 91,
                "date": 1,
                "chat": {"id": 42, "type": "group"},
                "photo": [{"file_id":"photo-fixture", "file_unique_id":"photo-unique", "width":1, "height":1}]
            }
        })))
        .mount(&telegram_api)
        .await;
    let config = ChannelsConfig {
        telegram: Some(tinychannels_bus::config::TelegramConfig {
            bot_token: "123:fixture".into(),
            chat_id: None,
            allowed_users: vec!["42".into()],
            stream_mode: tinychannels_bus::config::StreamMode::Partial,
            draft_update_interval_ms: 1_000,
            silent_streaming: true,
            mention_only: false,
        }),
        ..Default::default()
    };
    runtime_proxy
        .call_confidential::<()>(
            "StartChannelConfigured",
            (
                "telegram",
                config.clone(),
                tinychannels_bus::HttpProxyPolicy::default(),
            ),
        )
        .await
        .unwrap();
    let draft_scope_channel = ChannelRef {
        id: "telegram".into(),
        account_id: Some("telegram".into()),
    };
    let draft_scope_conversation = ConversationRef {
        kind: ConversationKind::Group,
        id: "42".into(),
        topic_id: Some("17".into()),
        ..Default::default()
    };
    let draft_intent = tinychannels_bus::ChannelOutboundIntent {
        idempotency_key: "artifact-draft-1".into(),
        channel_id: "telegram".into(),
        conversation_id: "42".into(),
        channel_ref: Some(draft_scope_channel.clone()),
        conversation_ref: Some(draft_scope_conversation.clone()),
        payload: tinychannels_bus::OutboundPayload::Text {
            text: "progress".into(),
        },
        ..Default::default()
    };
    let draft_result: tinychannels_bus::ChannelInteractionResult = runtime_proxy
        .call_confidential(
            "ChannelInteraction",
            tinychannels_bus::ChannelInteractionRequest {
                channel: "telegram".into(),
                interaction: tinychannels_bus::ChannelInteraction::UpsertDraft {
                    previous: None,
                    intent: Box::new(draft_intent.clone()),
                },
            },
        )
        .await
        .unwrap();
    let tinychannels_bus::ChannelInteractionResult::Sent(draft_outcome) = draft_result else {
        panic!("configured Telegram v2 draft returned no receipt")
    };
    assert!(
        draft_outcome.error.is_none(),
        "configured Telegram v2 draft should report a successful native send"
    );
    assert_eq!(
        draft_outcome.receipt.channel.as_ref(),
        Some(&draft_scope_channel)
    );
    assert_eq!(
        draft_outcome.receipt.conversation.as_ref(),
        Some(&draft_scope_conversation)
    );
    assert_eq!(
        draft_outcome.receipt.primary_platform_message_id.as_deref(),
        Some("92")
    );
    let draft_requests = telegram_api.received_requests().await.unwrap();
    let create_body: serde_json::Value = serde_json::from_slice(
        &draft_requests
            .iter()
            .find(|request| request.url.path().ends_with("/sendMessage"))
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(create_body["message_thread_id"], "17");
    assert_eq!(create_body["parse_mode"], "MarkdownV2");
    let mut update_intent = draft_intent.clone();
    update_intent.payload = tinychannels_bus::OutboundPayload::Text {
        text: "updated progress".into(),
    };
    let update_result: tinychannels_bus::ChannelInteractionResult = runtime_proxy
        .call_confidential(
            "ChannelInteraction",
            tinychannels_bus::ChannelInteractionRequest {
                channel: "telegram".into(),
                interaction: tinychannels_bus::ChannelInteraction::UpsertDraft {
                    previous: Some(draft_outcome.receipt.clone()),
                    intent: Box::new(update_intent),
                },
            },
        )
        .await
        .unwrap();
    let tinychannels_bus::ChannelInteractionResult::Sent(update_outcome) = update_result else {
        panic!("configured Telegram v2 draft update returned no receipt")
    };
    assert!(
        update_outcome.error.is_none(),
        "configured Telegram v2 draft update should report a successful native edit"
    );
    assert_eq!(
        update_outcome
            .receipt
            .primary_platform_message_id
            .as_deref(),
        Some("92")
    );
    let updated_requests = telegram_api.received_requests().await.unwrap();
    let edit_body: serde_json::Value = serde_json::from_slice(
        &updated_requests
            .iter()
            .find(|request| request.url.path().ends_with("/editMessageText"))
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(edit_body["parse_mode"], "MarkdownV2");
    let mut forged_receipt = draft_outcome.receipt.clone();
    forged_receipt.conversation.as_mut().unwrap().topic_id = Some("18".into());
    let before_reject = updated_requests
        .iter()
        .filter(|request| {
            request.url.path().ends_with("/sendMessage")
                || request.url.path().ends_with("/editMessageText")
        })
        .count();
    assert!(
        runtime_proxy
            .call_confidential::<tinychannels_bus::ChannelInteractionResult>(
                "ChannelInteraction",
                tinychannels_bus::ChannelInteractionRequest {
                    channel: "telegram".into(),
                    interaction: tinychannels_bus::ChannelInteraction::UpsertDraft {
                        previous: Some(forged_receipt),
                        intent: Box::new(draft_intent)
                    },
                }
            )
            .await
            .is_err()
    );
    let after_reject = telegram_api
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| {
            request.url.path().ends_with("/sendMessage")
                || request.url.path().ends_with("/editMessageText")
        })
        .count();
    assert_eq!(
        after_reject, before_reject,
        "forged prior draft scope must be rejected without Bot API write I/O"
    );
    other_runtime_proxy
        .call_confidential::<()>(
            "StartChannelConfigured",
            (
                "telegram",
                ChannelsConfig {
                    telegram: Some(tinychannels_bus::config::TelegramConfig {
                        bot_token: "123:other-fixture".into(),
                        chat_id: None,
                        allowed_users: vec!["42".into()],
                        stream_mode: tinychannels_bus::config::StreamMode::Partial,
                        draft_update_interval_ms: 1_000,
                        silent_streaming: true,
                        mention_only: false,
                    }),
                    ..Default::default()
                },
                tinychannels_bus::HttpProxyPolicy::default(),
            ),
        )
        .await
        .unwrap();

    let scope = tinychannels_bus::HostVoiceScope {
        configured_instance: "caller-cannot-select-this".into(),
        channel: ChannelRef {
            id: "caller-channel".into(),
            account_id: Some("caller-account".into()),
        },
        conversation: ConversationRef {
            kind: ConversationKind::Group,
            id: "42".into(),
            scope_id: Some("tenant-room".into()),
            parent_id: Some("parent".into()),
            thread_id: Some("17".into()),
            topic_id: Some("topic-17".into()),
        },
        turn_id: "turn-media-wire-1".into(),
    };
    let begin = MediaUploadBegin {
        instance: "telegram".into(),
        scope: scope.clone(),
        kind: MediaKind::Image,
        content_type: "image/png".into(),
        file_name: Some("tiny.png".into()),
        duration_ms: None,
        codec: None,
        byte_count: 68,
    };
    assert_eq!(
        runtime_proxy
            .call::<MediaUploadHandle>("BeginMediaUpload", begin.clone())
            .await
            .unwrap_err()
            .wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED
    );
    let handle: MediaUploadHandle = runtime_proxy
        .call_confidential("BeginMediaUpload", begin)
        .await
        .unwrap();
    let other_runtime_error = other_runtime_proxy
        .call_confidential::<MediaUploadResult>(
            "FinishMediaUpload",
            MediaUploadFinish(handle.clone()),
        )
        .await
        .unwrap_err();
    assert_eq!(other_runtime_error.wire_name(), tinybus::Error::FAILED);
    let png = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/f9sAAAAASUVORK5CYII=")
        .unwrap();
    assert_eq!(png.len(), 68);
    assert!(MEDIA_UPLOAD_CHUNK_BYTES > png.len());
    let png_for_wire_check = png.clone();
    runtime_proxy
        .call_confidential::<u32>(
            "AppendMediaUploadChunk",
            MediaUploadChunk {
                handle: handle.clone(),
                sequence: 0,
                bytes: png,
            },
        )
        .await
        .unwrap();
    let finish = MediaUploadFinish(handle);
    let reference: MediaUploadResult = runtime_proxy
        .call_confidential("FinishMediaUpload", finish.clone())
        .await
        .unwrap();
    assert!(reference.opaque_id.is_some());
    assert_eq!(reference.kind, MediaKind::Image);
    assert_eq!(reference.content_type.as_deref(), Some("image/png"));
    assert!(reference.path.is_none());
    assert!(reference.url.is_none());
    assert!(reference.local_cache_path.is_none());

    // Positive control: the other configured Telegram account can create and
    // resolve its own reference for the same conversation and turn.
    let other_handle: MediaUploadHandle = other_runtime_proxy
        .call_confidential(
            "BeginMediaUpload",
            MediaUploadBegin {
                instance: "telegram".into(),
                scope: scope.clone(),
                kind: MediaKind::Image,
                content_type: "image/png".into(),
                file_name: Some("other-account.png".into()),
                duration_ms: None,
                codec: None,
                byte_count: png_for_wire_check.len(),
            },
        )
        .await
        .unwrap();
    other_runtime_proxy
        .call_confidential::<u32>(
            "AppendMediaUploadChunk",
            MediaUploadChunk {
                handle: other_handle.clone(),
                sequence: 0,
                bytes: png_for_wire_check.clone(),
            },
        )
        .await
        .unwrap();
    let other_reference: MediaUploadResult = other_runtime_proxy
        .call_confidential("FinishMediaUpload", MediaUploadFinish(other_handle))
        .await
        .unwrap();
    let other_read = other_runtime_proxy
        .call_confidential::<tinychannels_bus::MediaReadInfo>(
            "BeginMediaRead",
            MediaReadBegin {
                instance: "telegram".into(),
                scope: scope.clone(),
                media: other_reference.clone(),
                max_bytes: 1024,
            },
        )
        .await
        .unwrap();
    let other_bytes = other_runtime_proxy
        .call_confidential::<tinychannels_bus::MediaReadChunkResult>(
            "ReadMediaChunk",
            MediaReadChunk {
                handle: other_read.handle.clone(),
                sequence: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(other_bytes.bytes, png_for_wire_check);
    other_runtime_proxy
        .call_confidential::<()>("AbortMediaRead", MediaReadFinish(other_read.handle))
        .await
        .unwrap();
    assert!(
        runtime_proxy
            .call_confidential::<tinychannels_bus::MediaReadInfo>(
                "BeginMediaRead",
                MediaReadBegin {
                    instance: "telegram".into(),
                    scope: scope.clone(),
                    media: other_reference,
                    max_bytes: 1024,
                },
            )
            .await
            .is_err()
    );

    let replay: MediaUploadResult = runtime_proxy
        .call_confidential("FinishMediaUpload", finish.clone())
        .await
        .unwrap();
    assert_eq!(replay, reference);

    // The compiled module resolves the durable opaque reference through the
    // same scoped store and returns bytes only in confidential bounded replies.
    let mut read_request = MediaReadBegin {
        instance: "telegram".into(),
        scope: scope.clone(),
        media: reference.clone(),
        max_bytes: 1024,
    };
    let cross_account_read = other_runtime_proxy
        .call_confidential::<tinychannels_bus::MediaReadInfo>(
            "BeginMediaRead",
            read_request.clone(),
        )
        .await;
    assert!(
        cross_account_read.is_err(),
        "a separately configured Telegram account must not resolve this account's durable media"
    );
    assert_eq!(
        runtime_proxy
            .call::<tinychannels_bus::MediaReadInfo>("BeginMediaRead", read_request.clone())
            .await
            .unwrap_err()
            .wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED
    );
    let mut wrong_conversation = read_request.clone();
    wrong_conversation.scope.conversation.topic_id = Some("different-topic".into());
    assert!(
        runtime_proxy
            .call_confidential::<tinychannels_bus::MediaReadInfo>(
                "BeginMediaRead",
                wrong_conversation
            )
            .await
            .is_err()
    );
    let mut wrong_turn = read_request.clone();
    wrong_turn.scope.turn_id = "different-turn".into();
    assert!(
        runtime_proxy
            .call_confidential::<tinychannels_bus::MediaReadInfo>("BeginMediaRead", wrong_turn)
            .await
            .is_err()
    );
    let mut foreign_account = read_request.clone();
    foreign_account.instance = "telegram:foreign".into();
    assert!(
        runtime_proxy
            .call_confidential::<tinychannels_bus::MediaReadInfo>("BeginMediaRead", foreign_account)
            .await
            .is_err()
    );
    let configured_foreign_account = runtime_proxy
        .call_confidential::<tinychannels_bus::MediaReadInfo>(
            "BeginMediaRead",
            MediaReadBegin {
                instance: "telegram:foreign".into(),
                scope: scope.clone(),
                media: reference.clone(),
                max_bytes: 1024,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        configured_foreign_account.wire_name(),
        tinybus::Error::FAILED
    );
    let mut claimed_foreign_account = read_request.clone();
    claimed_foreign_account.scope.channel.account_id = Some("telegram:foreign".into());
    let normalized_claim = runtime_proxy
        .call_confidential::<tinychannels_bus::MediaReadInfo>(
            "BeginMediaRead",
            claimed_foreign_account,
        )
        .await
        .unwrap();
    runtime_proxy
        .call_confidential::<()>(
            "AbortMediaRead",
            tinychannels_bus::MediaReadFinish(normalized_claim.handle),
        )
        .await
        .unwrap();
    let mut unknown_media = read_request.clone();
    unknown_media.media.opaque_id = Some("tinychannels_media_v1_unknown".into());
    assert!(
        runtime_proxy
            .call_confidential::<tinychannels_bus::MediaReadInfo>("BeginMediaRead", unknown_media)
            .await
            .is_err()
    );
    read_request.max_bytes = 67;
    assert!(
        runtime_proxy
            .call_confidential::<tinychannels_bus::MediaReadInfo>("BeginMediaRead", read_request)
            .await
            .is_err()
    );
    let read: tinychannels_bus::MediaReadInfo = runtime_proxy
        .call_confidential(
            "BeginMediaRead",
            MediaReadBegin {
                instance: "telegram".into(),
                scope: scope.clone(),
                media: reference.clone(),
                max_bytes: 1024,
            },
        )
        .await
        .unwrap();
    assert_eq!(read.content_type, "image/png");
    assert_eq!(read.byte_count, png_for_wire_check.len());
    assert_eq!(read.chunk_count, 1);
    assert!(read.media.local_cache_path.is_none());
    let chunk_request = MediaReadChunk {
        handle: read.handle.clone(),
        sequence: 0,
    };
    for (method_name, args) in [
        (
            "ReadMediaChunk",
            serde_json::to_value(chunk_request.clone()).unwrap(),
        ),
        (
            "AbortMediaRead",
            serde_json::to_value(MediaReadFinish(read.handle.clone())).unwrap(),
        ),
    ] {
        let error = runtime_proxy
            .call::<serde_json::Value>(method_name, args)
            .await
            .unwrap_err();
        assert_eq!(error.wire_name(), tinybus::Error::CONFIDENTIALITY_REQUIRED);
    }
    let chunk: tinychannels_bus::MediaReadChunkResult = runtime_proxy
        .call_confidential("ReadMediaChunk", chunk_request.clone())
        .await
        .unwrap();
    assert_eq!(chunk.bytes, png_for_wire_check);
    assert!(chunk.done);
    let retry: tinychannels_bus::MediaReadChunkResult = runtime_proxy
        .call_confidential("ReadMediaChunk", chunk_request)
        .await
        .unwrap();
    assert_eq!(retry, chunk, "a lost chunk reply must be safely retryable");
    let foreign_runtime_error = other_runtime_proxy
        .call_confidential::<tinychannels_bus::MediaReadChunk>(
            "ReadMediaChunk",
            MediaReadChunk {
                handle: read.handle.clone(),
                sequence: 0,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(foreign_runtime_error.wire_name(), tinybus::Error::FAILED);
    runtime_proxy
        .call_confidential::<()>("AbortMediaRead", MediaReadFinish(read.handle.clone()))
        .await
        .unwrap();
    assert!(
        runtime_proxy
            .call_confidential::<tinychannels_bus::MediaReadChunk>(
                "ReadMediaChunk",
                MediaReadChunk {
                    handle: read.handle,
                    sequence: 0,
                },
            )
            .await
            .is_err()
    );

    // Exercise the public framed request boundary at the advertised maximum
    // chunk, followed by a final partial chunk. The ordinary channel message
    // path below uses the independently returned opaque image reference.
    let mut pdf = vec![b'x'; MEDIA_UPLOAD_CHUNK_BYTES + 17];
    pdf[..5].copy_from_slice(b"%PDF-");
    let pdf_expected = pdf.clone();
    let pdf_begin = MediaUploadBegin {
        instance: "telegram".into(),
        scope: scope.clone(),
        kind: MediaKind::Document,
        content_type: "application/pdf".into(),
        file_name: Some("chunk-boundary.pdf".into()),
        duration_ms: None,
        codec: None,
        byte_count: pdf.len(),
    };
    let pdf_handle: MediaUploadHandle = runtime_proxy
        .call_confidential("BeginMediaUpload", pdf_begin)
        .await
        .unwrap();
    let remainder = pdf.split_off(MEDIA_UPLOAD_CHUNK_BYTES);
    runtime_proxy
        .call_confidential::<u32>(
            "AppendMediaUploadChunk",
            MediaUploadChunk {
                handle: pdf_handle.clone(),
                sequence: 0,
                bytes: pdf,
            },
        )
        .await
        .unwrap();
    runtime_proxy
        .call_confidential::<u32>(
            "AppendMediaUploadChunk",
            MediaUploadChunk {
                handle: pdf_handle.clone(),
                sequence: 1,
                bytes: remainder,
            },
        )
        .await
        .unwrap();
    let pdf_reference: MediaUploadResult = runtime_proxy
        .call_confidential("FinishMediaUpload", MediaUploadFinish(pdf_handle))
        .await
        .unwrap();
    assert_eq!(pdf_reference.kind, MediaKind::Document);
    assert_eq!(
        pdf_reference.size_bytes,
        Some((MEDIA_UPLOAD_CHUNK_BYTES + 17) as u64)
    );
    let pdf_read: tinychannels_bus::MediaReadInfo = runtime_proxy
        .call_confidential(
            "BeginMediaRead",
            MediaReadBegin {
                instance: "telegram".into(),
                scope: scope.clone(),
                media: pdf_reference,
                max_bytes: MEDIA_UPLOAD_CHUNK_BYTES as u64 + 17,
            },
        )
        .await
        .unwrap();
    assert_eq!(pdf_read.chunk_count, 2);
    let first_pdf_chunk_request = MediaReadChunk {
        handle: pdf_read.handle.clone(),
        sequence: 0,
    };
    let first_pdf_chunk: tinychannels_bus::MediaReadChunkResult = runtime_proxy
        .call_confidential("ReadMediaChunk", first_pdf_chunk_request.clone())
        .await
        .unwrap();
    assert_eq!(
        first_pdf_chunk.bytes,
        pdf_expected[..MEDIA_UPLOAD_CHUNK_BYTES]
    );
    assert!(!first_pdf_chunk.done);
    assert_eq!(
        runtime_proxy
            .call_confidential::<tinychannels_bus::MediaReadChunkResult>(
                "ReadMediaChunk",
                first_pdf_chunk_request,
            )
            .await
            .unwrap(),
        first_pdf_chunk
    );
    let last_pdf_chunk_request = MediaReadChunk {
        handle: pdf_read.handle.clone(),
        sequence: 1,
    };
    let last_pdf_chunk: tinychannels_bus::MediaReadChunkResult = runtime_proxy
        .call_confidential("ReadMediaChunk", last_pdf_chunk_request.clone())
        .await
        .unwrap();
    assert_eq!(
        last_pdf_chunk.bytes,
        pdf_expected[MEDIA_UPLOAD_CHUNK_BYTES..]
    );
    assert!(last_pdf_chunk.done);
    assert_eq!(
        runtime_proxy
            .call_confidential::<tinychannels_bus::MediaReadChunkResult>(
                "ReadMediaChunk",
                last_pdf_chunk_request,
            )
            .await
            .unwrap(),
        last_pdf_chunk,
        "the final confidential reply must be retryable"
    );
    runtime_proxy
        .call_confidential::<()>("AbortMediaRead", MediaReadFinish(pdf_read.handle))
        .await
        .unwrap();

    let send = ChannelOutboundIntent {
        idempotency_key: "media-wire-send".into(),
        channel_id: "telegram".into(),
        conversation_id: "42".into(),
        channel_ref: Some(ChannelRef {
            id: "telegram".into(),
            account_id: Some("telegram".into()),
        }),
        conversation_ref: Some(ConversationRef {
            kind: ConversationKind::Group,
            id: "42".into(),
            scope_id: Some("tenant-room".into()),
            parent_id: Some("parent".into()),
            thread_id: Some("17".into()),
            topic_id: Some("topic-17".into()),
        }),
        thread_id: Some("17".into()),
        turn_id: Some("turn-media-wire-1".into()),
        payload: OutboundPayload::Parts {
            parts: vec![OutboundContentPart::Content {
                part: tinychannels_bus::ContentPart::Image {
                    media: reference.clone(),
                    caption: Some("fixture".into()),
                    alt_text: None,
                },
            }],
        },
        ..Default::default()
    };
    let send_result = runtime_proxy
        .call_confidential::<tinychannels_bus::controllers::ChannelSendMessageResult>(
            "SendChannelIntent",
            ("telegram", send.clone()),
        )
        .await
        .unwrap();
    assert_eq!(send_result.message_id.as_deref(), Some("91"));
    assert!(send_result.error.is_none());

    let mut wrong_account = send.clone();
    wrong_account.channel_ref.as_mut().unwrap().account_id = Some("telegram:other".into());
    let mut wrong_conversation = send.clone();
    wrong_conversation.conversation_id = "43".into();
    wrong_conversation.conversation_ref.as_mut().unwrap().id = "43".into();
    let mut wrong_scope = send.clone();
    wrong_scope.conversation_ref.as_mut().unwrap().scope_id = Some("other-room".into());
    let mut wrong_parent = send.clone();
    wrong_parent.conversation_ref.as_mut().unwrap().parent_id = Some("other-parent".into());
    let mut wrong_thread = send.clone();
    wrong_thread.thread_id = Some("18".into());
    wrong_thread.conversation_ref.as_mut().unwrap().thread_id = Some("18".into());
    let mut wrong_topic = send.clone();
    wrong_topic.conversation_ref.as_mut().unwrap().topic_id = Some("other-topic".into());
    let mut wrong_turn = send.clone();
    wrong_turn.turn_id = Some("turn-from-another-message".into());
    let before_account_reject = telegram_api
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path().ends_with("/sendPhoto"))
        .count();
    assert!(
        runtime_proxy
            .call_confidential::<tinychannels_bus::controllers::ChannelSendMessageResult>(
                "SendChannelIntent",
                ("telegram", wrong_account),
            )
            .await
            .is_err(),
        "account mismatch should be rejected at the module boundary"
    );
    let after_account_reject = telegram_api
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path().ends_with("/sendPhoto"))
        .count();
    assert_eq!(after_account_reject, before_account_reject);
    let invalid_scopes = [
        wrong_conversation,
        wrong_scope,
        wrong_parent,
        wrong_thread,
        wrong_topic,
        wrong_turn,
    ];
    for invalid in invalid_scopes {
        let before = telegram_api
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path().ends_with("/sendPhoto"))
            .count();
        let rejected = runtime_proxy
            .call_confidential::<tinychannels_bus::controllers::ChannelSendMessageResult>(
                "SendChannelIntent",
                ("telegram", invalid),
            )
            .await
            .unwrap();
        let error = rejected
            .error
            .expect("out-of-scope media should return a typed dispatch error");
        assert_eq!(
            error.kind,
            tinychannels_bus::channel::SendErrorKind::Forbidden
        );
        assert!(rejected.message_id.is_none());
        assert!(rejected.receipt.is_none());
        let after = telegram_api
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path().ends_with("/sendPhoto"))
            .count();
        assert_eq!(after, before, "out-of-scope media must not reach Telegram");
    }
    let invalid_body = b"not a png".to_vec();
    let invalid_begin = MediaUploadBegin {
        instance: "telegram".into(),
        scope: scope.clone(),
        kind: MediaKind::Image,
        content_type: "image/png".into(),
        file_name: Some("invalid.png".into()),
        duration_ms: None,
        codec: None,
        byte_count: invalid_body.len(),
    };
    let invalid_handle: MediaUploadHandle = runtime_proxy
        .call_confidential("BeginMediaUpload", invalid_begin)
        .await
        .unwrap();
    runtime_proxy
        .call_confidential::<u32>(
            "AppendMediaUploadChunk",
            MediaUploadChunk {
                handle: invalid_handle.clone(),
                sequence: 0,
                bytes: invalid_body,
            },
        )
        .await
        .unwrap();
    let invalid_finish = MediaUploadFinish(invalid_handle);
    assert!(
        runtime_proxy
            .call_confidential::<MediaUploadResult>("FinishMediaUpload", invalid_finish.clone())
            .await
            .is_err()
    );
    runtime_proxy
        .call_confidential::<()>("AbortMediaUpload", invalid_finish.clone())
        .await
        .unwrap();
    assert!(
        runtime_proxy
            .call_confidential::<MediaUploadResult>("FinishMediaUpload", invalid_finish)
            .await
            .is_err()
    );
    let invalid_mime: MediaUploadHandle = runtime_proxy
        .call_confidential(
            "BeginMediaUpload",
            MediaUploadBegin {
                instance: "telegram".into(),
                scope: scope.clone(),
                kind: MediaKind::Image,
                content_type: "image/svg+xml".into(),
                file_name: Some("unsupported.svg".into()),
                duration_ms: None,
                codec: None,
                byte_count: png_for_wire_check.len(),
            },
        )
        .await
        .unwrap();
    runtime_proxy
        .call_confidential::<u32>(
            "AppendMediaUploadChunk",
            MediaUploadChunk {
                handle: invalid_mime.clone(),
                sequence: 0,
                bytes: png_for_wire_check.clone(),
            },
        )
        .await
        .unwrap();
    let invalid_mime_finish = MediaUploadFinish(invalid_mime);
    assert!(
        runtime_proxy
            .call_confidential::<MediaUploadResult>(
                "FinishMediaUpload",
                invalid_mime_finish.clone(),
            )
            .await
            .is_err()
    );
    runtime_proxy
        .call_confidential::<()>("AbortMediaUpload", invalid_mime_finish.clone())
        .await
        .unwrap();
    assert!(
        runtime_proxy
            .call_confidential::<MediaUploadResult>("FinishMediaUpload", invalid_mime_finish)
            .await
            .is_err()
    );
    let before_invalid_send = telegram_api
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path().ends_with("/sendPhoto"))
        .count();
    let invalid_send = ChannelOutboundIntent {
        idempotency_key: "invalid-media-no-reference".into(),
        channel_id: "telegram".into(),
        conversation_id: "42".into(),
        channel_ref: Some(ChannelRef {
            id: "telegram".into(),
            account_id: Some("telegram".into()),
        }),
        conversation_ref: send.conversation_ref.clone(),
        thread_id: send.thread_id.clone(),
        turn_id: send.turn_id.clone(),
        payload: OutboundPayload::Parts {
            parts: vec![OutboundContentPart::Content {
                part: tinychannels_bus::ContentPart::Image {
                    media: Default::default(),
                    caption: None,
                    alt_text: None,
                },
            }],
        },
        ..Default::default()
    };
    let invalid_result = runtime_proxy
        .call_confidential::<tinychannels_bus::controllers::ChannelSendMessageResult>(
            "SendChannelIntent",
            ("telegram", invalid_send),
        )
        .await
        .unwrap();
    let invalid_error = invalid_result
        .error
        .expect("missing media reference should be a typed rejection");
    assert_eq!(
        invalid_error.kind,
        tinychannels_bus::channel::SendErrorKind::Forbidden
    );
    assert!(!invalid_error.sent_before_error);
    assert!(invalid_result.message_id.is_none());
    assert!(invalid_result.receipt.is_none());
    let after_invalid_send = telegram_api
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path().ends_with("/sendPhoto"))
        .count();
    assert_eq!(after_invalid_send, before_invalid_send);

    let abandoned_handle: MediaUploadHandle = runtime_proxy
        .call_confidential(
            "BeginMediaUpload",
            MediaUploadBegin {
                instance: "telegram".into(),
                scope: scope.clone(),
                kind: MediaKind::Document,
                content_type: "application/pdf".into(),
                file_name: Some("abandoned.pdf".into()),
                duration_ms: None,
                codec: None,
                byte_count: 5,
            },
        )
        .await
        .unwrap();
    let pre_restart_read: tinychannels_bus::MediaReadInfo = runtime_proxy
        .call_confidential(
            "BeginMediaRead",
            MediaReadBegin {
                instance: "telegram".into(),
                scope: scope.clone(),
                media: reference.clone(),
                max_bytes: 1024,
            },
        )
        .await
        .unwrap();
    let requests = telegram_api.received_requests().await.unwrap();
    let upload = requests
        .iter()
        .find(|request| request.url.path().ends_with("/sendPhoto"))
        .expect("typed image send must reach Telegram's photo upload endpoint");
    let upload_text = String::from_utf8_lossy(&upload.body);
    assert!(upload_text.contains("name=\"chat_id\"\r\n\r\n42"));
    assert!(upload_text.contains("name=\"message_thread_id\"\r\n\r\n17"));
    assert!(upload_text.contains("name=\"caption\"\r\n\r\nfixture"));
    assert!(upload_text.contains("filename=\"tiny.png\""));
    assert!(
        upload
            .body
            .windows(png_for_wire_check.len())
            .any(|bytes| bytes == png_for_wire_check)
    );

    runtime_proxy
        .call_confidential::<()>("StopChannel", "telegram")
        .await
        .unwrap();
    runtime_proxy
        .call_confidential::<()>(
            "StartChannelConfigured",
            (
                "telegram",
                config,
                tinychannels_bus::HttpProxyPolicy::default(),
            ),
        )
        .await
        .unwrap();
    assert!(
        runtime_proxy
            .call_confidential::<u32>(
                "AppendMediaUploadChunk",
                MediaUploadChunk {
                    handle: abandoned_handle.clone(),
                    sequence: 0,
                    bytes: b"%PDF-".to_vec(),
                },
            )
            .await
            .is_err()
    );
    for stale in [finish, MediaUploadFinish(abandoned_handle)] {
        assert!(
            runtime_proxy
                .call_confidential::<MediaUploadResult>("FinishMediaUpload", stale.clone())
                .await
                .is_err()
        );
        assert!(
            runtime_proxy
                .call_confidential::<()>("AbortMediaUpload", stale)
                .await
                .is_err()
        );
    }
    assert!(
        runtime_proxy
            .call_confidential::<tinychannels_bus::MediaReadChunkResult>(
                "ReadMediaChunk",
                MediaReadChunk {
                    handle: pre_restart_read.handle.clone(),
                    sequence: 0,
                },
            )
            .await
            .is_err()
    );
    let after_restart_read: tinychannels_bus::MediaReadInfo = runtime_proxy
        .call_confidential(
            "BeginMediaRead",
            MediaReadBegin {
                instance: "telegram".into(),
                scope: scope.clone(),
                media: reference.clone(),
                max_bytes: 1024,
            },
        )
        .await
        .unwrap();
    let after_restart_bytes: tinychannels_bus::MediaReadChunkResult = runtime_proxy
        .call_confidential(
            "ReadMediaChunk",
            MediaReadChunk {
                handle: after_restart_read.handle.clone(),
                sequence: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(after_restart_bytes.bytes, png_for_wire_check);
    runtime_proxy
        .call_confidential::<()>("AbortMediaRead", MediaReadFinish(after_restart_read.handle))
        .await
        .unwrap();
    let after_restart = runtime_proxy
        .call_confidential::<tinychannels_bus::controllers::ChannelSendMessageResult>(
            "SendChannelIntent",
            ("telegram", send),
        )
        .await
        .unwrap();
    assert_eq!(after_restart.message_id.as_deref(), Some("91"));
    assert!(after_restart.error.is_none());
    runtime_proxy
        .call_confidential::<()>("StopChannel", "telegram")
        .await
        .unwrap();
    other_runtime_proxy
        .call_confidential::<()>("StopChannel", "telegram")
        .await
        .unwrap();
    module_proxy
        .call_confidential::<()>("StopRuntime", reservation.handle)
        .await
        .unwrap();
    module_proxy
        .call_confidential::<()>("StopRuntime", other_reservation.handle)
        .await
        .unwrap();
    // SAFETY: restore the process environment after the isolated artifact test.
    unsafe {
        std::env::remove_var("OPENHUMAN_TELEGRAM_BOT_API_BASE");
    }
}

#[tokio::test]
#[ignore = "run through scripts/verify-module-artifact.sh"]
async fn compiled_artifact_authenticates_whatsapp_cloud_webhooks_and_runs_native_v2() {
    use hmac::{Hmac, Mac, digest::KeyInit};
    use tinychannels_bus::{
        ChannelInteraction, ChannelInteractionRequest, ChannelInteractionResult, ChannelRef,
        ConversationKind, ConversationRef, WebhookDeliveryOutcome,
    };
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let graph = MockServer::start().await;
    // SAFETY: the artifact verifier runs this case in its own test process.
    unsafe {
        std::env::set_var("TINYCHANNELS_WHATSAPP_GRAPH_BASE", graph.uri());
    }
    Mock::given(method("POST"))
        .and(path_regex(".*/123/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"messages": [{"id": "wamid.artifact"}]})),
        )
        .mount(&graph)
        .await;

    let (module_host, connection, _broker_task) = start_bus().await.unwrap();
    let info = admit_module(
        &module_host,
        "TINYCHANNELS_MODULE_ARTIFACT",
        "tinychannels-module",
    )
    .unwrap();
    assert!(info.manifest.provides.iter().any(
        |provided| provided.version.interface.as_str() == "ai.tinyhumans.tinychannels.Channels"
    ));
    let storage = MemoryBackend::new();
    let served_backend = Arc::new(storage);
    let endpoint = PerCallerEndpoint::new(move |_sender| {
        let scoped = served_backend.for_scope(&Scope::new("local").ok()?).ok()?;
        Some(StorageServer::new(scoped))
    });
    let _storage_host = serve(&connection, Arc::new(endpoint)).await.unwrap();
    connection
        .serve_at(
            tinychannels_bus::HOST_OBJECT_PATH.try_into().unwrap(),
            Callback(
                Arc::new(Mutex::new(Vec::new())),
                Arc::new(Mutex::new(DeliveryState::default())),
            ),
        )
        .await
        .unwrap();
    connection
        .request_name(tinychannels_bus::HOST_BUS_NAME)
        .await
        .unwrap();
    wait_until_serving(
        &connection,
        tinychannels_bus::BUS_NAME,
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap();
    let module_proxy = connection
        .proxy(
            tinychannels_bus::BUS_NAME,
            tinychannels_bus::OBJECT_PATH,
            tinychannels_bus::BUS_NAME,
        )
        .unwrap();
    let reservation: tinychannels_bus::ChannelRuntimeReservation = module_proxy
        .call_confidential("ReserveRuntime", ())
        .await
        .unwrap();
    let scoped = Callback(
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(DeliveryState::default())),
    );
    connection
        .serve_at(
            reservation.host_object_path.clone().try_into().unwrap(),
            scoped.clone(),
        )
        .await
        .unwrap();
    module_proxy
        .call_confidential::<()>("StartRuntime", reservation.handle.clone())
        .await
        .unwrap();
    let runtime = connection
        .proxy(
            tinychannels_bus::BUS_NAME,
            &reservation.object_path,
            tinychannels_bus::BUS_NAME,
        )
        .unwrap();

    let config = ChannelsConfig {
        whatsapp: Some(tinychannels_bus::config::WhatsAppConfig {
            access_token: Some("access-token".into()),
            phone_number_id: Some("123".into()),
            verify_token: Some("verify-me".into()),
            app_secret: Some("artifact-secret".into()),
            session_path: None,
            pair_phone: None,
            pair_code: None,
            allowed_numbers: vec!["+15550001111".into()],
        }),
        ..Default::default()
    };
    let configured_start = runtime
        .call_confidential::<()>(
            "StartChannelConfigured",
            (
                "whatsapp",
                config.clone(),
                tinychannels_bus::HttpProxyPolicy::default(),
            ),
        )
        .await
        .expect_err("the module must not claim host-owned webhook ingress");
    assert_eq!(
        configured_start.wire_name(),
        "ai.tinyhumans.tinychannels.Error.NoWebhookIngress"
    );
    runtime
        .call_confidential::<()>(
            "StartOutboundChannel",
            (
                "whatsapp",
                config.clone(),
                tinychannels_bus::HttpProxyPolicy::default(),
            ),
        )
        .await
        .expect("the compiled module accepts host-forwarded WhatsApp Cloud webhooks");

    let interact = |interaction: ChannelInteraction| {
        let runtime = runtime.clone();
        async move {
            runtime
                .call_confidential::<ChannelInteractionResult>(
                    "ChannelInteraction",
                    ChannelInteractionRequest {
                        channel: "whatsapp".into(),
                        interaction,
                    },
                )
                .await
        }
    };
    // The subscription handshake is answered by the module, not the host.
    let challenge = interact(ChannelInteraction::VerifyWebhook {
        mode: "subscribe".into(),
        verify_token: "verify-me".into(),
        challenge: "challenge-42".into(),
    })
    .await
    .unwrap();
    assert!(matches!(
        challenge,
        ChannelInteractionResult::WebhookChallenge(value) if value == "challenge-42"
    ));
    assert!(
        interact(ChannelInteraction::VerifyWebhook {
            mode: "subscribe".into(),
            verify_token: "guess".into(),
            challenge: "x".into(),
        })
        .await
        .is_err()
    );

    let body = serde_json::to_vec(&serde_json::json!({
        "object": "whatsapp_business_account",
        "entry": [{"changes": [{"field": "messages", "value": {
            "metadata": {"phone_number_id": "123"},
            "contacts": [{"wa_id": "15550001111", "profile": {"name": "Ada"}}],
            "messages": [{"from": "15550001111", "id": "wamid.artifact.in", "timestamp": "1700000000",
                          "type": "text", "text": {"body": "hello compiled module"}}]
        }}]}]
    }))
    .unwrap();
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"artifact-secret").unwrap();
    mac.update(&body);
    let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    let deliver = |signature: Option<String>| {
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(&body);
        interact(ChannelInteraction::DeliverWebhook {
            signature,
            body_base64,
        })
    };
    // A forged or unsigned delivery is refused and reaches no one.
    assert!(deliver(None).await.is_err());
    assert!(deliver(Some("sha256=00".into())).await.is_err());
    assert!(
        scoped
            .0
            .lock()
            .await
            .iter()
            .all(|entry| entry["kind"] != "inbound")
    );
    let accepted = deliver(Some(signature.clone())).await.unwrap();
    assert!(matches!(
        accepted,
        ChannelInteractionResult::WebhookAccepted(WebhookDeliveryOutcome { messages: 1, .. })
    ));
    // Meta redelivers until it sees a 2xx; the second copy is a duplicate.
    let again = deliver(Some(signature)).await.unwrap();
    assert!(matches!(
        again,
        ChannelInteractionResult::WebhookAccepted(WebhookDeliveryOutcome {
            messages: 0,
            duplicates: 1,
            ..
        })
    ));
    let inbound: Vec<_> = scoped
        .0
        .lock()
        .await
        .iter()
        .filter(|entry| entry["kind"] == "inbound")
        .cloned()
        .collect();
    assert_eq!(inbound.len(), 1);
    let envelope = &inbound[0]["message"];
    assert_eq!(envelope["channel"]["id"], "whatsapp");
    assert_eq!(envelope["channel"]["accountId"], "whatsapp");
    assert_eq!(envelope["messageId"], "wamid.artifact.in");
    assert_eq!(envelope["text"], "hello compiled module");

    // The same configured account sends natively and reacts on the Graph wire.
    let scope_channel = ChannelRef {
        id: "whatsapp".into(),
        account_id: Some("whatsapp".into()),
    };
    let scope_conversation = ConversationRef {
        kind: ConversationKind::Dm,
        id: "+15550001111".into(),
        ..Default::default()
    };
    let sent: tinychannels_bus::controllers::ChannelSendMessageResult = runtime
        .call_confidential(
            "SendChannelIntent",
            (
                "whatsapp",
                ChannelOutboundIntent {
                    idempotency_key: "artifact-whatsapp-1".into(),
                    channel_id: "whatsapp".into(),
                    conversation_id: "+15550001111".into(),
                    channel_ref: Some(scope_channel.clone()),
                    conversation_ref: Some(scope_conversation.clone()),
                    payload: OutboundPayload::Text {
                        text: "native reply".into(),
                    },
                    ..Default::default()
                },
            ),
        )
        .await
        .unwrap();
    assert!(sent.error.is_none(), "{:?}", sent.error);
    let receipt = sent.receipt.expect("the provider's own receipt");
    assert_eq!(receipt.platform_message_ids, ["wamid.artifact"]);
    assert_eq!(receipt.channel.as_ref(), Some(&scope_channel));
    assert_eq!(receipt.conversation.as_ref(), Some(&scope_conversation));
    interact(ChannelInteraction::SendReaction {
        channel: scope_channel,
        conversation: scope_conversation,
        provider_message_id: "wamid.artifact.in".into(),
        reaction: tinychannels_bus::ChannelReaction {
            value: "👍".into(),
            custom_emoji_id: None,
        },
        remove: false,
    })
    .await
    .unwrap();
    let wire: Vec<serde_json::Value> = graph
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap_or_default())
        .collect();
    assert!(
        wire.iter()
            .any(|body| body["type"] == "text" && body["text"]["body"] == "native reply")
    );
    assert!(
        wire.iter()
            .any(|body| body["type"] == "reaction" && body["reaction"]["emoji"] == "👍")
    );
    assert!(
        graph
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| {
                request.headers.get("authorization").is_some_and(|value| {
                    value.to_str().unwrap_or_default() == "Bearer access-token"
                })
            })
    );
    runtime
        .call_confidential::<()>("StopChannel", "whatsapp")
        .await
        .unwrap();
}

fn mime_attachment_bytes(lines: &[String], filename: &str) -> Vec<u8> {
    let message = lines.join("\r\n");
    let marker = format!("filename=\"{filename}\"");
    let disposition = message.find(&marker).expect("MIME attachment disposition");
    let body_start = message[disposition..]
        .find("\r\n\r\n")
        .map(|offset| disposition + offset + 4)
        .expect("MIME attachment body");
    let encoded_end = message[body_start..]
        .find("\r\n--")
        .map(|offset| body_start + offset)
        .expect("next MIME boundary");
    let encoded = message[body_start..encoded_end].replace(['\r', '\n'], "");
    let headers = &message[disposition..body_start];
    if headers.contains("Content-Transfer-Encoding: base64") {
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("base64 MIME attachment")
    } else {
        message[body_start..encoded_end]
            .trim_end_matches("\r\n")
            .as_bytes()
            .to_vec()
    }
}

async fn smtp_server() -> (
    u16,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<Vec<String>>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (started, started_rx) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut started = Some(started);
        let mut released = Some(released);
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut received = Vec::new();
        write.write_all(b"220 fixture ESMTP\r\n").await.unwrap();
        let mut in_data = false;
        while let Ok(Some(line)) = lines.next_line().await {
            received.push(line.clone());
            if in_data {
                if line == "." {
                    in_data = false;
                    write.write_all(b"250 accepted\r\n").await.unwrap();
                }
                continue;
            }
            let upper = line.to_ascii_uppercase();
            let response = if upper.starts_with("EHLO") {
                b"250-fixture\r\n250 SIZE 30000000\r\n".as_slice()
            } else if upper.starts_with("MAIL") || upper.starts_with("RCPT") {
                b"250 accepted\r\n".as_slice()
            } else if upper.starts_with("DATA") {
                in_data = true;
                if let Some(started) = started.take() {
                    let _ = started.send(());
                }
                b"354 continue\r\n".as_slice()
            } else if upper.starts_with("QUIT") {
                write.write_all(b"221 bye\r\n").await.unwrap();
                break;
            } else {
                b"250 ok\r\n".as_slice()
            };
            write.write_all(response).await.unwrap();
            if upper.starts_with("DATA")
                && let Some(released) = released.take()
            {
                let _ = released.await;
            }
        }
        received
    });
    (port, started_rx, release, task)
}

async fn relay_server() -> (String, tokio::task::JoinHandle<Vec<serde_json::Value>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = tokio_tungstenite::accept_async(stream).await.unwrap();
        let hello = stream.next().await.unwrap().unwrap();
        assert!(matches!(
            serde_json::from_str::<GatewayToConnectorFrame>(hello.to_text().unwrap()).unwrap(),
            GatewayToConnectorFrame::Hello { .. }
        ));
        let descriptor = CapabilityDescriptor {
            platform: "telegram".into(),
            label: "artifact fixture".into(),
            ..Default::default()
        };
        stream
            .send(Message::Text(
                format!(
                    "{}\n",
                    serde_json::to_string(&ConnectorToGatewayFrame::Descriptor { descriptor })
                        .unwrap()
                )
                .into(),
            ))
            .await
            .unwrap();
        let mut envelope = tinychannels_bus::ChannelInboundEnvelope::default();
        envelope.channel.id = "telegram".into();
        envelope.text = "local inbound".into();
        stream
            .send(Message::Text(
                format!(
                    "{}\n",
                    serde_json::to_string(&ConnectorToGatewayFrame::Inbound {
                        event: serde_json::to_value(envelope).unwrap(),
                        buffer_id: Some("artifact-buffer".into()),
                    })
                    .unwrap()
                )
                .into(),
            ))
            .await
            .unwrap();
        let mut seen = Vec::new();
        while let Some(Ok(message)) = stream.next().await {
            let Ok(text) = message.to_text() else {
                continue;
            };
            let frame: GatewayToConnectorFrame = serde_json::from_str(text).unwrap();
            if let GatewayToConnectorFrame::Outbound { request_id, .. } = &frame {
                stream
                    .send(Message::Text(
                        format!(
                            "{}\n",
                            serde_json::to_string(&ConnectorToGatewayFrame::OutboundResult {
                                request_id: request_id.clone(),
                                result: serde_json::json!({"success":true,"message_id":"artifact-message"}),
                            })
                            .unwrap()
                        )
                        .into(),
                    ))
                    .await
                    .unwrap();
            }
            seen.push(serde_json::to_value(frame).unwrap());
        }
        seen
    });
    (url, task)
}

async fn relay_server_v2() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = tokio_tungstenite::accept_async(stream).await.unwrap();
        let hello = stream.next().await.unwrap().unwrap();
        let GatewayToConnectorFrame::HelloV2 {
            platform,
            bot_id,
            contract_version,
        } = serde_json::from_str(hello.to_text().unwrap()).unwrap()
        else {
            panic!("v2 relay must start with hello_v2")
        };
        assert_eq!(
            (platform.as_str(), bot_id.as_str(), contract_version),
            ("telegram", "v2-fixture", 2)
        );
        let mut capabilities = tinychannels_bus::ChannelCapabilities::unknown();
        capabilities.progressive_edits = true;
        stream
            .send(Message::Text(
                format!(
                    "{}\n",
                    serde_json::to_string(&ConnectorToGatewayFrame::DescriptorV2 {
                        platform,
                        bot_id,
                        capabilities: Box::new(capabilities),
                    })
                    .unwrap()
                )
                .into(),
            ))
            .await
            .unwrap();
        if let Some(Ok(message)) = stream.next().await {
            assert!(
                message.is_close(),
                "module sent an unexpected second v2 hello after the descriptor"
            );
        }
    });
    (url, task)
}
