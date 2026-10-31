#![allow(
    deprecated,
    reason = "integration fixtures still exercise one-release v21 adapters"
)]

use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, sync::Arc, time::Duration};
use tinybus::module::{ModuleRuntime, RuntimeNames};
use tinychannels_bus::{BUS_NAME, ChannelRuntimeReservation, HOST_BUS_NAME, OBJECT_PATH, methods};

#[derive(Debug, serde::Deserialize)]
struct PreviousChannelCapabilities {
    remote_control: bool,
    chat_approvals: bool,
    progressive_edits: bool,
    history_key_ignores_thread: bool,
}

#[derive(Debug, serde::Deserialize)]
struct PreviousInteractionInfo {
    capabilities: PreviousChannelCapabilities,
    proactive_target: Option<String>,
    supports_reactions: bool,
    supports_draft_updates: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
enum PreviousInteractionResult {
    Info(PreviousInteractionInfo),
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
enum PreviousControlResult {
    Capabilities(PreviousChannelCapabilities),
}

#[derive(Clone, Default)]
struct Callback {
    sends: Arc<std::sync::atomic::AtomicUsize>,
}

#[tinybus::interface(name = "ai.tinyhumans.tinychannels.ChannelsHost")]
impl Callback {
    async fn dispatch_controller_backend(
        &self,
        request: tinychannels_bus::ChannelBackendRequest,
    ) -> tinybus::Result<tinychannels_bus::ChannelBackendResult> {
        use tinychannels_bus::module_progressive::{
            ProgressiveTransportRequest, ProgressiveTransportResult,
        };
        let tinychannels_bus::ChannelBackendRequest::ProgressiveTransport(request) = request else {
            return Err(tinybus::Error::failed("unexpected backend callback"));
        };
        if matches!(*request, ProgressiveTransportRequest::Send { .. }) {
            self.sends.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(
            tinychannels_bus::ChannelBackendResult::ProgressiveTransport(Box::new(
                ProgressiveTransportResult::Success(serde_json::json!({"id": "artifact-message"})),
            )),
        )
    }

    async fn report_status(
        &self,
        _channel: String,
        _state: String,
        _detail: Option<String>,
    ) -> tinybus::Result<()> {
        Ok(())
    }

    async fn load_pending_deliveries(
        &self,
    ) -> tinybus::Result<Vec<tinychannels_bus::DeliveryQueueEntry>> {
        Ok(Vec::new())
    }
}

#[tokio::test]
#[ignore = "run through scripts/verify-module-artifact.sh"]
async fn runtime_only_host_closes_scoped_resources_without_a_legacy_callback_object() {
    let artifact = PathBuf::from(std::env::var_os("TINYCHANNELS_MODULE_ARTIFACT").unwrap());
    let directory = artifact
        .parent()
        .unwrap()
        .join(format!("runtime-only-host-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&directory).unwrap();
    let _staged = StagedArtifact(directory.clone());
    let staged = directory.join(artifact.file_name().unwrap());
    fs::copy(&artifact, &staged).unwrap();
    let hash = hex::encode(Sha256::digest(fs::read(&staged).unwrap()));
    fs::write(
        directory.join("modules.toml"),
        format!(
            "{:?} = {hash:?}\n",
            staged.file_name().unwrap().to_str().unwrap()
        ),
    )
    .unwrap();
    let runtime = ModuleRuntime::start(&RuntimeNames::default(), None).unwrap();
    let connection = runtime.connection();
    let modules = runtime.host();
    let loaded = modules.load_file(&staged).unwrap();
    modules
        .ensure_ready(loaded.manifest.bus_name.as_str())
        .await
        .unwrap();
    assert!(
        connection
            .attestation(loaded.manifest.bus_name.clone())
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(loaded.manifest.provides.len(), 1);
    assert_eq!(
        loaded.manifest.provides[0].version.interface.as_str(),
        BUS_NAME,
        "storage is supplied through the host callback instead of a migration service"
    );
    let provided = loaded.manifest.provides.first().unwrap();
    let actual: Vec<_> = provided
        .methods
        .iter()
        .map(|method| method.as_str())
        .collect();
    assert_eq!(actual, tinychannels_bus::METHODS);
    connection.request_name(HOST_BUS_NAME).await.unwrap();
    let proxy = connection.proxy(BUS_NAME, OBJECT_PATH, BUS_NAME).unwrap();
    let bounded_proxy = connection
        .proxy(BUS_NAME, OBJECT_PATH, BUS_NAME)
        .unwrap()
        .with_timeout(Duration::from_millis(350));
    let first: ChannelRuntimeReservation = proxy
        .call_confidential(methods::RESERVE_RUNTIME, ())
        .await
        .unwrap();
    let second: ChannelRuntimeReservation = proxy
        .call_confidential(methods::RESERVE_RUNTIME, ())
        .await
        .unwrap();
    let callback = Callback::default();
    for runtime in [&first, &second] {
        connection
            .serve_at(
                runtime.host_object_path.clone().try_into().unwrap(),
                callback.clone(),
            )
            .await
            .unwrap();
        if runtime.handle == first.handle {
            let request = (
                runtime.handle.clone(),
                tinychannels_bus::ChannelRuntimeDispatchPolicy { listener_count: 2 },
            );
            assert_eq!(
                proxy
                    .call::<()>(methods::START_RUNTIME_WITH_DISPATCH, request.clone())
                    .await
                    .unwrap_err()
                    .wire_name(),
                tinybus::Error::CONFIDENTIALITY_REQUIRED
            );
            bounded_proxy
                .call_confidential::<()>(methods::START_RUNTIME_WITH_DISPATCH, request)
                .await
                .unwrap();
            assert!(
                proxy
                    .call_confidential::<()>(methods::START_RUNTIME, runtime.handle.clone())
                    .await
                    .is_err()
            );
        } else {
            proxy
                .call_confidential::<()>(methods::START_RUNTIME, runtime.handle.clone())
                .await
                .unwrap();
        }
    }
    let first_client = connection
        .proxy(BUS_NAME, &first.object_path, BUS_NAME)
        .unwrap();
    let second_client = connection
        .proxy(BUS_NAME, &second.object_path, BUS_NAME)
        .unwrap();
    let _: Vec<String> = first_client.call(methods::LIST_CHANNELS, ()).await.unwrap();

    use tinychannels_bus::module_inbound::{
        ChannelInboundSource, PrepareChannelIdentityRequest, PrepareChannelInboundRequest,
        PreparedChannelIdentity, PreparedChannelInbound,
    };
    use tinychannels_bus::module_progressive::{
        ProgressiveCommand, ProgressiveEvent, ProgressiveResult,
    };
    let ordinary = first_client
        .call::<ProgressiveResult>(
            methods::EXECUTE_PROGRESSIVE,
            ProgressiveCommand::Open {
                channel: "discord:artifact".into(),
            },
        )
        .await;
    assert_eq!(
        ordinary.err().unwrap().wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED
    );
    let opened: ProgressiveResult = first_client
        .call_confidential(
            methods::EXECUTE_PROGRESSIVE,
            ProgressiveCommand::Open {
                channel: "discord:artifact".into(),
            },
        )
        .await
        .unwrap();
    let ProgressiveResult::Opened { handle } = opened else {
        panic!("missing progressive artifact handle");
    };
    let updated: ProgressiveResult = first_client
        .call_confidential(
            methods::EXECUTE_PROGRESSIVE,
            ProgressiveCommand::Feed {
                handle: handle.clone(),
                events: vec![ProgressiveEvent::TextDelta("artifact draft".into())],
            },
        )
        .await
        .unwrap();
    assert_eq!(updated, ProgressiveResult::Updated);
    for _ in 0..2 {
        let result: ProgressiveResult = first_client
            .call_confidential(
                methods::EXECUTE_PROGRESSIVE,
                ProgressiveCommand::Finish {
                    handle: handle.clone(),
                    text: "artifact final".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(result, ProgressiveResult::Closed);
    }
    assert_eq!(callback.sends.load(std::sync::atomic::Ordering::SeqCst), 1);
    use tinychannels_bus::module_remote::{RemoteOperation, RemoteOperationResult, RemoteRoute};
    let remote_workspace = tempfile::tempdir().unwrap();
    use tinychannels_bus::module_control::{ChannelControlRequest, ChannelControlResult};
    let control = ChannelControlRequest::RuntimeLimits {
        timeout_secs: 1,
        listener_count: 100,
    };
    assert!(
        first_client
            .call::<ChannelControlResult>(methods::PREPARE_CHANNEL_CONTROL, control.clone())
            .await
            .is_err()
    );
    let facts: ChannelControlResult = first_client
        .call_confidential(methods::PREPARE_CHANNEL_CONTROL, control)
        .await
        .unwrap();
    assert_eq!(
        facts,
        ChannelControlResult::RuntimeLimits {
            timeout_secs: 30,
            max_in_flight: 64
        }
    );

    let request = RemoteOperation::Execute {
        workspace_dir: remote_workspace.path().to_str().unwrap().into(),
        message: Box::new(tinychannels_bus::ChannelMessage {
            id: "fixture".into(),
            sender: "sender".into(),
            reply_target: "chat".into(),
            content: "/HELP@FixtureBot".into(),
            channel: "telegram".into(),
            timestamp: 1,
            thread_ts: None,
            sender_name: None,
            topic_id: None,
            reply_to_id: None,
        }),
        route: RemoteRoute {
            provider: "fixture".into(),
            model: "fixture".into(),
        },
        history_len: 0,
        session_scope: None,
    };
    assert!(
        first_client
            .call::<RemoteOperationResult>(methods::EXECUTE_REMOTE, request.clone())
            .await
            .is_err()
    );
    let remote: RemoteOperationResult = first_client
        .call_confidential(methods::EXECUTE_REMOTE, request)
        .await
        .unwrap();
    assert_eq!(
        remote.reply, None,
        "an unconfigured channel has no remote-control capability"
    );
    let _: RemoteOperationResult = first_client
        .call_confidential(
            methods::EXECUTE_REMOTE,
            RemoteOperation::MarkTurn {
                workspace_dir: remote_workspace.path().to_str().unwrap().into(),
                channel: "telegram".into(),
                reply_target: "chat".into(),
                busy: true,
            },
        )
        .await
        .unwrap();
    let identity = PrepareChannelIdentityRequest {
        channel: "telegram:bot".into(),
        sender: None,
        reply_target: Some("chat".into()),
        thread_ts: Some("topic".into()),
    };
    let ordinary = first_client
        .call::<PreparedChannelIdentity>(methods::PREPARE_CHANNEL_IDENTITY, identity.clone())
        .await;
    assert_eq!(
        ordinary.err().unwrap().wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED
    );
    let prepared: PreparedChannelIdentity = first_client
        .call_confidential(methods::PREPARE_CHANNEL_IDENTITY, identity)
        .await
        .unwrap();
    assert_eq!(prepared.thread_id, "channel:telegram:bot/chat");
    assert_eq!(prepared.client_id, "inbound");
    let inbound = PrepareChannelInboundRequest {
        source: ChannelInboundSource::Legacy {
            message: Box::new(tinychannels_bus::ChannelMessage {
                id: "msg".into(),
                channel: "discord".into(),
                sender: "user".into(),
                reply_target: "room".into(),
                content: "fixture".into(),
                ..Default::default()
            }),
            envelope: None,
        },
        namespace: "tenant".into(),
        session_policy: Default::default(),
        preparation_mode: Default::default(),
    };
    let ordinary = first_client
        .call::<PreparedChannelInbound>(methods::PREPARE_CHANNEL_INBOUND, inbound.clone())
        .await;
    assert_eq!(
        ordinary.err().unwrap().wire_name(),
        tinybus::Error::CONFIDENTIALITY_REQUIRED
    );
    let prepared: PreparedChannelInbound = first_client
        .call_confidential(methods::PREPARE_CHANNEL_INBOUND, inbound)
        .await
        .unwrap();
    assert_eq!(prepared.history_key, "discord_user_room");
    assert_eq!(prepared.thread_id, "channel:discord/user/room");
    assert_eq!(prepared.client_id, "inbound:discord:user");
    assert_eq!(
        prepared.session_key,
        "v22|namespace=p6:tenant|channel=p7:discord|account=a|kind=p7:unknown|scope=a|conversation=p4:room|parent=a|topic=a|thread=a|participant=p4:user"
    );
    assert_eq!(prepared.envelope.text, "fixture");

    use tinychannels_bus::module_runtime::{
        ChannelIngressKind, ChannelMessagePreparation, ConfiguredChannelInfo, PreparedChannelIntent,
    };
    let config = tinychannels_bus::ChannelsConfig {
        linq: Some(tinychannels_bus::config::LinqConfig {
            api_token: "fixture-token".into(),
            from_phone: "+15550000001".into(),
            allowed_senders: vec![],
            webhook_secret: None,
        }),
        yuanbao: Some(tinychannels_bus::config::YuanbaoConfig {
            app_key: "fixture-key".into(),
            token: "fixture-token".into(),
            ws_domain: "wss://example.invalid".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let ordinary_calls = [
        (
            methods::DESCRIBE_CONFIGURED_CHANNELS,
            serde_json::json!([config.clone(), tinychannels_bus::HttpProxyPolicy::default()]),
        ),
        (
            methods::START_OUTBOUND_CHANNEL,
            serde_json::json!([
                "linq",
                config.clone(),
                tinychannels_bus::HttpProxyPolicy::default()
            ]),
        ),
        (
            methods::PREPARE_OUTBOUND_INTENT,
            serde_json::json!([
                "linq",
                ChannelMessagePreparation::Legacy(serde_json::json!({"text":"fixture"}))
            ]),
        ),
    ];
    for (member, args) in ordinary_calls {
        let error = first_client
            .call::<serde_json::Value>(member, args)
            .await
            .unwrap_err();
        assert_eq!(
            error.wire_name(),
            tinybus::Error::CONFIDENTIALITY_REQUIRED,
            "valid provider configuration and message calls require confidential delivery"
        );
    }
    let catalog: Vec<ConfiguredChannelInfo> = first_client
        .call_confidential(
            methods::DESCRIBE_CONFIGURED_CHANNELS,
            (config.clone(), tinychannels_bus::HttpProxyPolicy::default()),
        )
        .await
        .unwrap();
    assert_eq!(catalog.len(), 2);
    let linq = catalog
        .iter()
        .find(|channel| channel.name == "linq")
        .unwrap();
    let yuanbao = catalog
        .iter()
        .find(|channel| channel.name == "yuanbao")
        .unwrap();
    assert_eq!(linq.ingress, ChannelIngressKind::ExternalWebhook);
    assert!(!linq.interaction.capabilities.progressive_edits);
    assert!(yuanbao.interaction.capabilities.streaming_reply_pipeline);
    assert!(!yuanbao.interaction.capabilities.progressive_edits);
    first_client
        .call_confidential::<()>(
            methods::START_OUTBOUND_CHANNEL,
            ("linq", config, tinychannels_bus::HttpProxyPolicy::default()),
        )
        .await
        .unwrap();
    let interaction: tinychannels_bus::ChannelInteractionResult = first_client
        .call_confidential(
            methods::CHANNEL_INTERACTION,
            (tinychannels_bus::ChannelInteractionRequest {
                channel: "linq".into(),
                interaction: tinychannels_bus::ChannelInteraction::DescribeV2,
            },),
        )
        .await
        .unwrap();
    let tinychannels_bus::ChannelInteractionResult::InfoV2(described) = interaction else {
        panic!("DescribeV2 should return its configured capabilities")
    };

    // The existing Describe operation keeps the previous four-flag payload
    // and its supplemental fields decodable by a host built against v21.
    let legacy_interaction: serde_json::Value = first_client
        .call_confidential(
            methods::CHANNEL_INTERACTION,
            (tinychannels_bus::ChannelInteractionRequest {
                channel: "linq".into(),
                interaction: tinychannels_bus::ChannelInteraction::Describe,
            },),
        )
        .await
        .unwrap();
    let legacy: PreviousInteractionResult = serde_json::from_value(legacy_interaction).unwrap();
    let PreviousInteractionResult::Info(legacy) = legacy;
    assert_eq!(
        legacy.capabilities.remote_control,
        described.capabilities.remote_control
    );
    assert_eq!(
        legacy.capabilities.chat_approvals,
        described.capabilities.chat_approvals
    );
    assert_eq!(
        legacy.capabilities.progressive_edits,
        described.capabilities.progressive_edits
    );
    assert_eq!(
        legacy.capabilities.history_key_ignores_thread,
        described.capabilities.history_key_ignores_thread
    );
    assert_eq!(
        legacy.proactive_target,
        described.capabilities.account.proactive_target
    );
    assert_eq!(legacy.supports_reactions, described.capabilities.reactions);
    assert_eq!(
        legacy.supports_draft_updates,
        described.capabilities.streaming_reply_pipeline
    );

    let controlled: tinychannels_bus::module_control::ChannelControlResult = first_client
        .call_confidential(
            methods::PREPARE_CHANNEL_CONTROL,
            (
                tinychannels_bus::module_control::ChannelControlRequest::CapabilitiesV2 {
                    channel: "linq".into(),
                },
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        controlled,
        tinychannels_bus::module_control::ChannelControlResult::CapabilitiesV2(Box::new(
            described.capabilities.clone()
        ))
    );
    let legacy_control: serde_json::Value = first_client
        .call_confidential(
            methods::PREPARE_CHANNEL_CONTROL,
            (
                tinychannels_bus::module_control::ChannelControlRequest::Capabilities {
                    channel: "linq".into(),
                },
            ),
        )
        .await
        .unwrap();
    let PreviousControlResult::Capabilities(legacy_control) =
        serde_json::from_value(legacy_control).unwrap();
    assert_eq!(
        legacy_control.remote_control,
        described.capabilities.remote_control
    );
    assert_eq!(
        legacy_control.chat_approvals,
        described.capabilities.chat_approvals
    );
    assert_eq!(
        legacy_control.progressive_edits,
        described.capabilities.progressive_edits
    );
    assert_eq!(
        legacy_control.history_key_ignores_thread,
        described.capabilities.history_key_ignores_thread
    );
    let prepared: PreparedChannelIntent = first_client
        .call_confidential(
            methods::PREPARE_OUTBOUND_INTENT,
            (
                "linq",
                ChannelMessagePreparation::Legacy(
                    serde_json::json!({"text":"fixture", "idempotencyKey":"original-key"}),
                ),
            ),
        )
        .await
        .unwrap();
    assert_eq!(prepared.intent.idempotency_key, "original-key");
    assert_eq!(prepared.legacy_message["idempotencyKey"], "original-key");
    let rendered: PreparedChannelIntent = first_client
        .call_confidential(
            methods::PREPARE_OUTBOUND_INTENT,
            (
                "linq",
                ChannelMessagePreparation::Intent(Box::new(prepared.intent)),
            ),
        )
        .await
        .unwrap();
    assert_eq!(rendered.intent.idempotency_key, "original-key");
    assert_eq!(rendered.legacy_message["idempotencyKey"], "original-key");
    proxy
        .call_confidential::<()>(methods::STOP_RUNTIME, first.handle)
        .await
        .unwrap();
    assert!(
        first_client
            .call::<Vec<String>>(methods::LIST_CHANNELS, ())
            .await
            .is_err()
    );
    let root_limits: tinychannels_bus::module_control::ChannelControlResult = bounded_proxy
        .call_confidential(
            methods::PREPARE_CHANNEL_CONTROL,
            tinychannels_bus::module_control::ChannelControlRequest::RuntimeLimits {
                timeout_secs: 1,
                listener_count: 100,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        root_limits,
        tinychannels_bus::module_control::ChannelControlResult::RuntimeLimits {
            timeout_secs: 30,
            max_in_flight: 64,
        }
    );
    let _: Vec<String> = second_client
        .call(methods::LIST_CHANNELS, ())
        .await
        .unwrap();
    proxy.call::<()>(methods::SHUTDOWN, ()).await.unwrap();
    assert!(
        second_client
            .call::<Vec<String>>(methods::LIST_CHANNELS, ())
            .await
            .is_err()
    );
}

struct StagedArtifact(PathBuf);
impl Drop for StagedArtifact {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
