use super::*;
use crate::media::MediaPersistence;
use std::sync::atomic::{AtomicUsize, Ordering};
use tinychannels::host::MediaRepository;

async fn memory_host() -> HostChannels {
    let bus = tinybus::transport::memory::MemoryBus::new();
    tinybus::broker::Broker::new().spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    HostChannels::new(connection)
}

#[tokio::test]
async fn module_media_scope_is_validated_and_bound_to_the_configured_account() {
    let repository = ModuleMediaRepository(
        memory_host().await,
        "telegram:main".into(),
        Some("acct-7".into()),
    );
    let mut scope = tinychannels::host::MediaScope::default();
    assert!(repository.bind_scope(&mut scope).is_err());

    scope.conversation.id = "room".into();
    assert!(repository.bind_scope(&mut scope).is_err());

    scope.turn_id = "turn-1".into();
    repository.bind_scope(&mut scope).unwrap();
    assert_eq!(scope.configured_instance, "telegram:main");
    assert_eq!(scope.channel.id, "telegram");
    assert_eq!(scope.channel.account_id.as_deref(), Some("acct-7"));

    let unbound = ModuleMediaRepository(memory_host().await, "telegram:main".into(), None);
    assert!(unbound.bind_scope(&mut scope).is_err());

    let request = || tinychannels::host::MediaStoreRequest {
        metadata: tinychannels::host::MediaMetadata {
            scope: scope.clone(),
            kind: tinychannels_bus::MediaKind::Image,
            content_type: "image/png".into(),
            file_name: Some("image.png".into()),
            duration_ms: None,
            codec: None,
            voice_direction: None,
            dedupe_key: None,
            max_bytes: 1024,
        },
        bytes: vec![1, 2, 3],
    };
    assert!(repository.store_media(request()).await.is_err());
    assert!(repository.store_public_media(request()).await.is_err());
    assert!(
        repository
            .read_media(scope.clone(), "opaque-id".into(), 1024)
            .await
            .is_err()
    );
    assert!(
        repository
            .delete_media(scope, "opaque-id".into())
            .await
            .is_err()
    );
}

#[test]
fn channel_event_scoping_preserves_each_event_kind_and_overwrites_provider_identity() {
    use tinychannels_bus::ChannelEventKind as Kind;
    let kinds = [
        (
            Kind::Typing {
                actor: Default::default(),
                active: true,
                expires_at: None,
            },
            "typing",
        ),
        (
            Kind::ReadReceipt {
                message_id: "m".into(),
                actor: None,
                timestamp: None,
            },
            "read_receipt",
        ),
        (
            Kind::DeliveredReceipt {
                message_id: "m".into(),
                actor: None,
                timestamp: None,
            },
            "delivered_receipt",
        ),
        (
            Kind::ReactionAdded {
                message_id: "m".into(),
                provider_message_id: None,
                actor: Default::default(),
                reaction: Default::default(),
            },
            "reaction_added",
        ),
        (
            Kind::ReactionRemoved {
                message_id: "m".into(),
                provider_message_id: None,
                actor: Default::default(),
                reaction: Default::default(),
            },
            "reaction_removed",
        ),
        (
            Kind::OutboundReaction {
                message_id: "m".into(),
                provider_message_id: None,
                reaction: Default::default(),
                success: true,
            },
            "outbound_reaction",
        ),
        (
            Kind::MessageUpdated {
                message_id: "m".into(),
                provider_message_id: None,
                parts: vec![],
            },
            "message_updated",
        ),
        (
            Kind::MessageDeleted {
                message_id: "m".into(),
                provider_message_id: None,
            },
            "message_deleted",
        ),
        (
            Kind::Presence {
                actor: Default::default(),
                state: "online".into(),
                expires_at: None,
            },
            "presence",
        ),
        (
            Kind::Lifecycle {
                state: "ready".into(),
                reason: None,
            },
            "lifecycle",
        ),
        (
            Kind::ButtonCallback {
                query_id: "cb".into(),
                action_id: "go".into(),
                provider_message_id: None,
                actor: Default::default(),
            },
            "button_callback",
        ),
    ];

    for (kind, expected_kind) in kinds {
        let scoped = scope_channel_event(
            "telegram:main",
            tinychannels_bus::ChannelEvent {
                version: 2,
                event_id: "event-1".into(),
                source_event_id: None,
                source_timestamp: None,
                channel: tinychannels_bus::ChannelRef {
                    id: "attacker".into(),
                    account_id: Some("spoofed".into()),
                },
                conversation: None,
                event: kind,
            },
        );
        assert_eq!(scoped.channel.id, "telegram");
        assert_eq!(scoped.channel.account_id.as_deref(), Some("telegram:main"));
        assert_eq!(
            scoped.event_id,
            format!("telegram:main:event-1:{expected_kind}")
        );
    }
}

struct SlowChannel {
    started: StdMutex<Option<oneshot::Sender<()>>>,
    release: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
}

#[async_trait::async_trait]
impl Channel for SlowChannel {
    fn name(&self) -> &str {
        "fixture"
    }

    async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
        if let Some(started) = self.started.lock().unwrap().take() {
            let _ = started.send(());
        }
        if let Some(release) = self.release.lock().await.take() {
            let _ = release.await;
        }
        Ok(())
    }

    async fn listen(
        &self,
        _tx: tokio::sync::mpsc::Sender<tinychannels::ChannelMessage>,
    ) -> anyhow::Result<()> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn canceled_send_waiter_does_not_cancel_owned_send_or_shutdown_join() {
    let (started, started_rx) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let channel = Arc::new(SlowChannel {
        started: StdMutex::new(Some(started)),
        release: tokio::sync::Mutex::new(Some(release_rx)),
    });
    let connection = tinybus::Connection::connect({
        let bus = tinybus::transport::memory::MemoryBus::new();
        tinybus::broker::Broker::new().spawn(bus.clone());
        bus.connect().await.unwrap()
    })
    .await
    .unwrap();
    let host = HostChannels::new(connection);
    let running = Running::start(channel, Arc::new(Hooks::default()), host);
    let send_waiter = tokio::spawn({
        let running = running.clone();
        async move { running.send(SendMessage::new("body", "recipient")).await }
    });
    started_rx.await.unwrap();
    send_waiter.abort();
    let _ = send_waiter.await;

    let stop = tokio::spawn({
        let running = running.clone();
        async move { running.stop().await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), async {
            while !stop.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_err()
    );
    release.send(()).unwrap();
    stop.await.unwrap().unwrap();
}

#[tokio::test]
async fn provider_shutdown_replies_to_queued_sends_as_not_dispatched() {
    let (started, started_rx) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let channel = Arc::new(SlowChannel {
        started: StdMutex::new(Some(started)),
        release: tokio::sync::Mutex::new(Some(release_rx)),
    });
    let host = memory_host().await;
    let running = Running::start(channel, Arc::new(Hooks::default()), host);
    let first = tokio::spawn({
        let running = running.clone();
        async move { running.send(SendMessage::new("in flight", "room")).await }
    });
    started_rx.await.unwrap();

    let queued = tokio::spawn({
        let running = running.clone();
        async move { running.send(SendMessage::new("queued", "room")).await }
    });
    while running.send.capacity() != 255 {
        tokio::task::yield_now().await;
    }

    let stop = tokio::spawn({
        let running = running.clone();
        async move { running.stop().await }
    });
    release.send(()).unwrap();

    first.await.unwrap().unwrap();
    let queued_error = queued.await.unwrap().unwrap_err();
    assert!(matches!(queued_error, RunningSendError::NotDispatched(_)));
    assert!(queued_error.to_string().contains("provider is stopping"));
    stop.await.unwrap().unwrap();
}

#[derive(Default)]
struct InteractionChannel(StdMutex<Vec<String>>);
#[async_trait::async_trait]
impl Channel for InteractionChannel {
    fn name(&self) -> &str {
        "interaction-fixture"
    }
    async fn listen(&self, _: mpsc::Sender<tinychannels::ChannelMessage>) -> anyhow::Result<()> {
        std::future::pending().await
    }
    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(format!("send:{}", message.content));
        Ok(())
    }
    async fn start_typing(&self, recipient: &str) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(format!("typing:{recipient}"));
        Ok(())
    }
    async fn send_draft(&self, message: &SendMessage) -> anyhow::Result<Option<String>> {
        self.0
            .lock()
            .unwrap()
            .push(format!("draft:{}", message.content));
        Ok(Some("module-draft".into()))
    }
    async fn stop_typing(&self, recipient: &str) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(format!("stop:{recipient}"));
        Ok(())
    }
}

#[tokio::test]
async fn provider_actor_orders_interactions_with_sends_and_rejects_after_stop() {
    use tinychannels_bus::{ChannelInteraction as Op, ChannelInteractionResult};
    let bus = tinybus::transport::memory::MemoryBus::new();
    tinybus::broker::Broker::new().spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    let channel = Arc::new(InteractionChannel::default());
    let running = Running::start(
        channel.clone(),
        Arc::new(Hooks::default()),
        HostChannels::new(connection),
    );
    running
        .interact(Op::StartTyping {
            recipient: "room".into(),
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    running
        .send(SendMessage::new("ordinary", "room"))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let draft = running
        .interact(Op::SendDraft {
            message: SendMessage::new("partial", "room"),
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(matches!(draft, ChannelInteractionResult::Draft(Some(id)) if id == "module-draft"));
    running
        .interact(Op::StopTyping {
            recipient: "room".into(),
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    running.stop().await.unwrap();
    assert_eq!(
        *channel.0.lock().unwrap(),
        ["typing:room", "send:ordinary", "draft:partial", "stop:room"]
    );
    assert!(matches!(
        running.interact(Op::Describe).await,
        Err(RunningSendError::NotDispatched(_))
    ));
}

struct WebhookVerificationAdapter;

#[async_trait::async_trait]
impl ChannelAdapter for WebhookVerificationAdapter {
    fn descriptor(&self) -> tinychannels_bus::channel::ChannelDescriptor {
        tinychannels_bus::channel::ChannelDescriptor {
            id: "webhook-fixture".into(),
            display_name: "Webhook fixture".into(),
            ..Default::default()
        }
    }

    fn supports_webhook_verification(&self) -> bool {
        true
    }

    fn verify_webhook(&self, mode: &str, verify_token: &str, challenge: &str) -> Option<String> {
        (mode == "subscribe" && verify_token == "fixture-token").then(|| challenge.to_owned())
    }

    async fn start(
        &self,
        _sink: &(dyn ChannelInboundSink + Send + Sync),
    ) -> std::result::Result<(), ChannelSendError> {
        Ok(())
    }

    async fn stop(&self) -> std::result::Result<(), ChannelSendError> {
        Ok(())
    }

    async fn send(
        &self,
        _intent: tinychannels::channel::ChannelOutboundIntent,
    ) -> std::result::Result<tinychannels::channel::MessageReceipt, ChannelSendError> {
        Err(ChannelSendError::unsupported("send is not used"))
    }

    async fn status(
        &self,
    ) -> std::result::Result<tinychannels::controllers::ChannelAccountSnapshot, ChannelSendError>
    {
        Ok(Default::default())
    }
}

#[async_trait::async_trait]
impl tinychannels::channel::LegacyChannelReaction for WebhookVerificationAdapter {
    async fn send_reaction(
        &self,
        _channel: &tinychannels_bus::ChannelRef,
        _conversation: &tinychannels_bus::ConversationRef,
        _provider_message_id: &str,
        reaction: &tinychannels_bus::ChannelReaction,
        _remove: bool,
    ) -> std::result::Result<(), ChannelSendError> {
        if reaction.value == "reject" {
            Err(ChannelSendError::new("reaction rejected"))
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn outbound_only_provider_routes_webhook_verification_to_native_adapter() {
    let running = Running::start_built_outbound(
        BuiltChannel::from_adapter(Arc::new(WebhookVerificationAdapter)),
        Arc::new(Hooks::default()),
        memory_host().await,
    );

    let result = running
        .interact(tinychannels_bus::ChannelInteraction::VerifyWebhook {
            mode: "subscribe".into(),
            verify_token: "fixture-token".into(),
            challenge: "challenge-42".into(),
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(matches!(
        result,
        tinychannels_bus::ChannelInteractionResult::WebhookChallenge(value)
            if value == "challenge-42"
    ));
    running.stop().await.unwrap();
}

#[tokio::test]
async fn outbound_only_provider_routes_scoped_reactions_to_native_extension() {
    let adapter = Arc::new(WebhookVerificationAdapter);
    let running = Running::start_built_outbound(
        BuiltChannel::from_adapter(adapter.clone()).with_reaction_port(adapter),
        Arc::new(Hooks::default()),
        memory_host().await,
    );

    let result = running
        .interact(tinychannels_bus::ChannelInteraction::SendReaction {
            channel: tinychannels_bus::ChannelRef {
                id: "whatsapp".into(),
                account_id: Some("whatsapp".into()),
            },
            conversation: tinychannels_bus::ConversationRef {
                kind: tinychannels_bus::ConversationKind::Dm,
                id: "+15550001111".into(),
                ..Default::default()
            },
            provider_message_id: "wamid.fixture".into(),
            reaction: tinychannels_bus::ChannelReaction {
                value: "👍".into(),
                custom_emoji_id: None,
            },
            remove: false,
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(matches!(
        result,
        tinychannels_bus::ChannelInteractionResult::Completed
    ));
    running.stop().await.unwrap();
}

#[tokio::test]
async fn outbound_only_provider_preserves_native_reaction_failures() {
    let adapter = Arc::new(WebhookVerificationAdapter);
    let running = Running::start_built_outbound(
        BuiltChannel::from_adapter(adapter.clone()).with_reaction_port(adapter),
        Arc::new(Hooks::default()),
        memory_host().await,
    );

    let error = running
        .interact(tinychannels_bus::ChannelInteraction::SendReaction {
            channel: tinychannels_bus::ChannelRef {
                id: "whatsapp".into(),
                account_id: Some("whatsapp".into()),
            },
            conversation: tinychannels_bus::ConversationRef {
                id: "room".into(),
                ..Default::default()
            },
            provider_message_id: "message".into(),
            reaction: tinychannels_bus::ChannelReaction {
                value: "reject".into(),
                custom_emoji_id: None,
            },
            remove: false,
        })
        .await
        .unwrap_err();

    assert!(error.to_string().contains("reaction rejected"));
    running.stop().await.unwrap();
}

#[tokio::test]
async fn outbound_only_provider_rejects_reactions_without_a_native_extension() {
    let running = Running::start_built_outbound(
        BuiltChannel::from_adapter(Arc::new(WebhookVerificationAdapter)),
        Arc::new(Hooks::default()),
        memory_host().await,
    );

    let error = running
        .interact(tinychannels_bus::ChannelInteraction::SendReaction {
            channel: tinychannels_bus::ChannelRef::default(),
            conversation: tinychannels_bus::ConversationRef::default(),
            provider_message_id: "message".into(),
            reaction: tinychannels_bus::ChannelReaction {
                value: "👍".into(),
                custom_emoji_id: None,
            },
            remove: false,
        })
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("provider reaction is not supported")
    );
    running.stop().await.unwrap();
}

#[tokio::test]
async fn outbound_only_provider_describes_native_adapter_capabilities() {
    let running = Running::start_built_outbound(
        BuiltChannel::from_adapter(Arc::new(WebhookVerificationAdapter)),
        Arc::new(Hooks::default()),
        memory_host().await,
    );

    let result = running
        .interact(tinychannels_bus::ChannelInteraction::DescribeV2)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let tinychannels_bus::ChannelInteractionResult::InfoV2(info) = result else {
        panic!("DescribeV2 should return the configured adapter's capabilities")
    };
    assert_eq!(info.capabilities.account.configured, Some(true));
    assert_eq!(info.capabilities.account.enabled, Some(true));
    running.stop().await.unwrap();
}

#[tokio::test]
async fn outbound_only_provider_returns_legacy_description_from_native_adapter() {
    let running = Running::start_built_outbound(
        BuiltChannel::from_adapter(Arc::new(WebhookVerificationAdapter)),
        Arc::new(Hooks::default()),
        memory_host().await,
    );

    let result = running
        .interact(tinychannels_bus::ChannelInteraction::Describe)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(matches!(
        result,
        tinychannels_bus::ChannelInteractionResult::Info(info)
            if !info.supports_reactions && !info.supports_draft_updates
    ));
    running.stop().await.unwrap();
}

#[tokio::test]
async fn legacy_provider_keeps_v2_description_dispatch() {
    let bus = tinybus::transport::memory::MemoryBus::new();
    tinybus::broker::Broker::new().spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    let running = Running::start(
        Arc::new(InteractionChannel::default()),
        Arc::new(Hooks::default()),
        HostChannels::new(connection),
    );

    let result = running
        .interact(tinychannels_bus::ChannelInteraction::DescribeV2)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(matches!(
        result,
        tinychannels_bus::ChannelInteractionResult::InfoV2(_)
    ));
    running.stop().await.unwrap();
}

#[tokio::test]
async fn outbound_only_provider_dispatches_draft_upsert_through_retained_draft_port() {
    let adapter = Arc::new(WebhookVerificationAdapter);
    let drafts = Arc::new(CapturingDraftUpsert(std::sync::Mutex::new(None)));
    let running = Running::start_built_outbound(
        BuiltChannel::with_output_ports(adapter, None, Some(drafts.clone())),
        Arc::new(Hooks::default()),
        memory_host().await,
    );

    let result = running
        .interact(tinychannels_bus::ChannelInteraction::UpsertDraft {
            previous: None,
            intent: Box::new(tinychannels_bus::ChannelOutboundIntent {
                idempotency_key: "draft-worker".into(),
                conversation_id: "room".into(),
                payload: tinychannels_bus::OutboundPayload::Text {
                    text: "draft".into(),
                },
                ..Default::default()
            }),
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(matches!(
        result,
        tinychannels_bus::ChannelInteractionResult::Sent(_)
    ));
    assert!(drafts.0.lock().unwrap().is_some());
    running.stop().await.unwrap();
}

#[tokio::test]
async fn outbound_only_provider_rejects_draft_upsert_without_a_draft_port() {
    let running = Running::start_built_outbound(
        BuiltChannel::from_adapter(Arc::new(WebhookVerificationAdapter)),
        Arc::new(Hooks::default()),
        memory_host().await,
    );

    let error = running
        .interact(tinychannels_bus::ChannelInteraction::UpsertDraft {
            previous: None,
            intent: Box::default(),
        })
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("provider draft upsert is not supported")
    );
    running.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn typing_refresh_is_bounded_and_joined_by_provider_owner() {
    use tinychannels_bus::ChannelInteraction as Op;
    let bus = tinybus::transport::memory::MemoryBus::new();
    tinybus::broker::Broker::new().spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    let channel = Arc::new(InteractionChannel::default());
    let running = Running::start(
        channel.clone(),
        Arc::new(Hooks::default()),
        HostChannels::new(connection.clone()),
    );

    running
        .interact(Op::StartTyping {
            recipient: "room".into(),
        })
        .await
        .unwrap();
    let first_handle = running
        .interact(Op::StartTypingRefresh {
            recipient: "room".into(),
            interval_secs: 4,
        })
        .await
        .unwrap();
    let tinychannels_bus::ChannelInteractionResult::TypingRefresh {
        handle: first_handle,
    } = first_handle
    else {
        panic!("provider did not return a refresh handle")
    };
    let second_handle = running
        .interact(Op::StartTypingRefresh {
            recipient: "room".into(),
            interval_secs: 4,
        })
        .await
        .unwrap();
    let tinychannels_bus::ChannelInteractionResult::TypingRefresh {
        handle: second_handle,
    } = second_handle
    else {
        panic!("provider did not return a refresh handle")
    };
    assert_ne!(first_handle, second_handle);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(4)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        channel.0.lock().unwrap().len(),
        3,
        "both refresh handles remain independently active"
    );

    assert!(
        running
            .interact(Op::StartTypingRefresh {
                recipient: "other".into(),
                interval_secs: 0,
            })
            .await
            .is_err()
    );
    running
        .interact(Op::StopTypingRefresh {
            handle: first_handle,
        })
        .await
        .unwrap();
    tokio::time::advance(Duration::from_secs(4)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(channel.0.lock().unwrap().len(), 4);
    running
        .interact(Op::StopTypingRefresh {
            handle: second_handle.clone(),
        })
        .await
        .unwrap();
    running
        .interact(Op::StopTyping {
            recipient: "room".into(),
        })
        .await
        .unwrap();
    running.stop().await.unwrap();
    assert!(matches!(
        running
            .interact(Op::StopTypingRefresh {
                handle: second_handle,
            })
            .await,
        Err(RunningSendError::NotDispatched(_))
    ));
    assert_eq!(
        *channel.0.lock().unwrap(),
        [
            "typing:room",
            "typing:room",
            "typing:room",
            "typing:room",
            "stop:room"
        ]
    );
    connection.close().await.unwrap();
}

struct PendingTypingChannel {
    entered: Arc<tokio::sync::Notify>,
    dropped: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl Channel for PendingTypingChannel {
    fn name(&self) -> &str {
        "pending-typing"
    }

    async fn send(&self, _: &SendMessage) -> anyhow::Result<()> {
        Ok(())
    }

    async fn listen(&self, _: mpsc::Sender<tinychannels::ChannelMessage>) -> anyhow::Result<()> {
        std::future::pending().await
    }

    async fn start_typing(&self, _: &str) -> anyhow::Result<()> {
        struct DropMark(Arc<std::sync::atomic::AtomicUsize>);
        impl Drop for DropMark {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let _drop_mark = DropMark(Arc::clone(&self.dropped));
        self.entered.notify_one();
        std::future::pending().await
    }
}

#[tokio::test(start_paused = true)]
async fn typing_refresh_stop_and_shutdown_cancel_inflight_provider_calls() {
    use tinychannels_bus::ChannelInteraction as Op;
    let bus = tinybus::transport::memory::MemoryBus::new();
    tinybus::broker::Broker::new().spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let channel = Arc::new(PendingTypingChannel {
        entered: Arc::clone(&entered),
        dropped: Arc::clone(&dropped),
    });
    let running = Running::start(
        channel,
        Arc::new(Hooks::default()),
        HostChannels::new(connection.clone()),
    );

    let stop_handle = running
        .interact(Op::StartTypingRefresh {
            recipient: "stop-case".into(),
            interval_secs: 1,
        })
        .await
        .unwrap();
    let tinychannels_bus::ChannelInteractionResult::TypingRefresh {
        handle: stop_handle,
    } = stop_handle
    else {
        panic!("provider did not return a refresh handle")
    };
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    entered.notified().await;
    running
        .interact(Op::StopTypingRefresh {
            handle: stop_handle,
        })
        .await
        .unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);

    let shutdown_handle = running
        .interact(Op::StartTypingRefresh {
            recipient: "shutdown-case".into(),
            interval_secs: 1,
        })
        .await
        .unwrap();
    let tinychannels_bus::ChannelInteractionResult::TypingRefresh { handle: _ } = shutdown_handle
    else {
        panic!("provider did not return a refresh handle")
    };
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    entered.notified().await;
    running.stop().await.unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
    connection.close().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn typing_refresh_capacity_counts_handles_not_recipients() {
    use tinychannels_bus::ChannelInteraction as Op;
    let bus = tinybus::transport::memory::MemoryBus::new();
    tinybus::broker::Broker::new().spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    let channel = Arc::new(InteractionChannel::default());
    let running = Running::start(
        channel,
        Arc::new(Hooks::default()),
        HostChannels::new(connection.clone()),
    );
    let mut handles = Vec::new();
    for _ in 0..MAX_TYPING_REFRESH_RECIPIENTS {
        let result = running
            .interact(Op::StartTypingRefresh {
                recipient: "same-room".into(),
                interval_secs: MAX_TYPING_REFRESH_INTERVAL_SECS,
            })
            .await
            .unwrap();
        let tinychannels_bus::ChannelInteractionResult::TypingRefresh { handle } = result else {
            panic!("provider did not return a refresh handle")
        };
        handles.push(handle);
    }
    assert!(
        running
            .interact(Op::StartTypingRefresh {
                recipient: "same-room".into(),
                interval_secs: MAX_TYPING_REFRESH_INTERVAL_SECS,
            })
            .await
            .is_err()
    );
    for handle in handles {
        running
            .interact(Op::StopTypingRefresh { handle })
            .await
            .unwrap();
    }
    running.stop().await.unwrap();
    connection.close().await.unwrap();
}

struct ReconnectingChannel {
    attempts: StdMutex<Vec<oneshot::Sender<()>>>,
}
#[async_trait::async_trait]
impl Channel for ReconnectingChannel {
    fn name(&self) -> &str {
        "reconnect-fixture"
    }
    async fn send(&self, _: &SendMessage) -> anyhow::Result<()> {
        Ok(())
    }
    async fn listen(&self, _: mpsc::Sender<tinychannels::ChannelMessage>) -> anyhow::Result<()> {
        let last = {
            let mut attempts = self.attempts.lock().unwrap();
            let receipt = attempts.remove(0);
            let _ = receipt.send(());
            attempts.is_empty()
        };
        if last {
            std::future::pending::<()>().await;
        }
        anyhow::bail!("fixture disconnected")
    }
}

#[tokio::test(start_paused = true)]
async fn disconnected_module_provider_reconnects_and_stop_joins_the_new_listener() {
    let (first, first_rx) = oneshot::channel();
    let (second, mut second_rx) = oneshot::channel();
    let channel = Arc::new(ReconnectingChannel {
        attempts: StdMutex::new(vec![first, second]),
    });
    let bus = tinybus::transport::memory::MemoryBus::new();
    tinybus::broker::Broker::new().spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    let host = HostChannels::new(connection);
    let running = Running::start_supervised(
        channel,
        Arc::new(Hooks::default()),
        host.clone(),
        ChannelSupervisionPolicy::default(),
    );
    first_rx.await.unwrap();
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(10)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(
        second_rx.try_recv().is_ok(),
        "the module must own provider reconnects"
    );
    assert!(!running.is_finished());
    running.stop().await.unwrap();
    assert!(running.is_finished());
    host.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn stopping_during_selected_backoff_prevents_another_provider_attempt() {
    let (first, first_rx) = oneshot::channel();
    let (second, mut second_rx) = oneshot::channel();
    let channel = Arc::new(ReconnectingChannel {
        attempts: StdMutex::new(vec![first, second]),
    });
    let bus = tinybus::transport::memory::MemoryBus::new();
    tinybus::broker::Broker::new().spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    let host = HostChannels::new(connection);
    let running = Running::start_supervised(
        channel.clone(),
        Arc::new(Hooks::default()),
        host.clone(),
        ChannelSupervisionPolicy {
            initial_backoff_secs: 30,
            max_backoff_secs: 60,
        },
    );
    first_rx.await.unwrap();
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(5)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(matches!(
        second_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    running.stop().await.unwrap();
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(channel.attempts.lock().unwrap().len(), 1);
    assert!(running.is_finished());
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn lifecycle_hooks_are_drained_once_in_registration_order() {
    let hooks = Hooks::default();
    let calls = Arc::new(StdMutex::new(Vec::new()));
    for index in 0..3 {
        let calls = calls.clone();
        hooks.register_shutdown(
            "fixture",
            Box::new(move || {
                Box::pin(async move {
                    calls.lock().unwrap().push(index);
                })
            }),
        );
    }
    hooks.drain().await;
    hooks.drain().await;
    assert_eq!(*calls.lock().unwrap(), [0, 1, 2]);
}

#[tokio::test]
async fn host_adapters_propagate_callback_failures() {
    use tinychannels::host::{AllowlistStore, EventSink, Transcriber};
    let bus = tinybus::transport::memory::MemoryBus::new();
    tinybus::broker::Broker::new().spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    let host = HostChannels::new(connection);
    // Once host callbacks stop, adapters must surface admission failures.
    host.shutdown().await.unwrap();
    assert!(
        ModuleAllowlist(host.clone())
            .persist_allowed_identity("fixture", "member")
            .await
            .is_err()
    );
    assert!(
        ModuleEvents(host.clone(), "fixture".into())
            .publish("fixture", "event", serde_json::json!({}))
            .await
            .is_err()
    );
    let transcriber = ModuleTranscriber(host.clone(), None);
    assert_eq!(transcriber.name(), "module-host");
    assert!(
        transcriber
            .transcribe(tinychannels::host::TranscriptionRequest {
                audio_base64: "YQ==".into(),
                mime_type: Some("audio/wav".into()),
                file_name: None,
                language: None,
            })
            .await
            .is_err()
    );
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn lost_send_worker_reply_distinguishes_dispatch_from_queueing() {
    for dispatched in [false, true] {
        let (send, mut commands) = mpsc::channel::<SendCommand>(1);
        let (cancel, _) = watch::channel(false);
        let running = Arc::new(Running {
            channel: tinychannels::factory::BuiltChannel::from_adapter(Arc::new(
                super::LegacyChannelAdapter(Arc::new(InteractionChannel::default())),
            )),
            spoken: None,
            send,
            cancel,
            worker: Mutex::new(None),
            finished: Arc::new(AtomicBool::new(false)),
        });
        let waiter = tokio::spawn({
            let running = running.clone();
            async move { running.send(SendMessage::new("body", "room")).await }
        });
        let command = commands.recv().await.unwrap();
        if dispatched {
            command.started.send(()).unwrap();
        } else {
            drop(command.started);
        }
        drop(command.reply);
        let error = waiter.await.unwrap().unwrap_err();
        if dispatched {
            assert!(matches!(error, RunningSendError::AfterDispatch(_)));
            assert_eq!(
                error.to_string(),
                "provider send worker stopped after dispatch"
            );
        } else {
            assert!(matches!(error, RunningSendError::NotDispatched(_)));
            assert_eq!(
                error.to_string(),
                "provider send worker stopped before dispatch"
            );
        }
    }
}

struct LegacyTextOnlyChannel(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl Channel for LegacyTextOnlyChannel {
    fn name(&self) -> &str {
        "legacy-text-only"
    }

    async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn listen(
        &self,
        _tx: tokio::sync::mpsc::Sender<tinychannels::ChannelMessage>,
    ) -> anyhow::Result<()> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn typed_legacy_projection_failure_is_not_reported_as_dispatched() {
    let sends = Arc::new(AtomicUsize::new(0));
    let bus = tinybus::transport::memory::MemoryBus::new();
    tinybus::broker::Broker::new().spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    let running = Running::start(
        Arc::new(LegacyTextOnlyChannel(sends.clone())),
        Arc::new(Hooks::default()),
        HostChannels::new(connection),
    );
    let message = tinychannels_bus::ChannelOutboundMessage {
        channel_id: "legacy-text-only".into(),
        conversation_id: "room".into(),
        channel_ref: None,
        conversation_ref: None,
        reply_to_id: None,
        thread_id: None,
        turn_id: None,
        idempotency_key: "typed-non-text".into(),
        parts: vec![tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Image {
                media: Default::default(),
                caption: None,
                alt_text: None,
            },
        }],
    };

    let error = running.send_typed(message).await.unwrap_err();

    assert!(matches!(error, RunningSendError::ValidationFailed(_)));
    assert!(!error.to_string().is_empty());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    running.stop().await.unwrap();
}

#[tokio::test]
async fn legacy_adapter_rejects_unsupported_typed_content_before_send() {
    let sends = Arc::new(AtomicUsize::new(0));
    let adapter = LegacyChannelAdapter(Arc::new(LegacyTextOnlyChannel(sends.clone())));
    let message = tinychannels_bus::ChannelOutboundMessage {
        channel_id: "legacy-text-only".into(),
        conversation_id: "room".into(),
        channel_ref: None,
        conversation_ref: None,
        reply_to_id: None,
        thread_id: None,
        turn_id: None,
        idempotency_key: "typed-image".into(),
        parts: vec![tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Image {
                media: Default::default(),
                caption: None,
                alt_text: None,
            },
        }],
    };

    let error = adapter.send_typed(&message).await.unwrap_err();

    assert!(error.is_unsupported());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn legacy_adapter_rejects_native_payload_without_a_text_projection() {
    let sends = Arc::new(AtomicUsize::new(0));
    let adapter = LegacyChannelAdapter(Arc::new(LegacyTextOnlyChannel(sends.clone())));
    let intent = tinychannels::channel::ChannelOutboundIntent {
        idempotency_key: "opaque-native".into(),
        channel_id: "legacy-text-only".into(),
        conversation_id: "room".into(),
        payload: tinychannels_bus::OutboundPayload::NativeChannelData {
            data: serde_json::json!({"opaque": true}),
        },
        ..Default::default()
    };

    let error = adapter.send(intent).await.unwrap_err();

    assert!(error.is_unsupported());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
}

struct CapturingDraftUpsert(
    std::sync::Mutex<
        Option<(
            Option<tinychannels_bus::channel::MessageReceipt>,
            tinychannels::channel::ChannelOutboundIntent,
        )>,
    >,
);

#[async_trait::async_trait]
impl tinychannels::channel::ChannelStreamingDraft for CapturingDraftUpsert {
    async fn upsert_draft(
        &self,
        previous: Option<&tinychannels_bus::channel::MessageReceipt>,
        intent: tinychannels::channel::ChannelOutboundIntent,
    ) -> std::result::Result<tinychannels_bus::channel::MessageReceipt, ChannelSendError> {
        *self.0.lock().unwrap() = Some((previous.cloned(), intent));
        Ok(tinychannels_bus::channel::MessageReceipt {
            primary_platform_message_id: Some("draft-message-4".into()),
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn draft_upsert_fills_missing_receipt_scope_and_preserves_previous_receipt() {
    let drafts = CapturingDraftUpsert(std::sync::Mutex::new(None));
    let previous = tinychannels_bus::channel::MessageReceipt {
        primary_platform_message_id: Some("draft-message-3".into()),
        ..Default::default()
    };
    let intent = tinychannels::channel::ChannelOutboundIntent {
        idempotency_key: "draft-4".into(),
        channel_id: "telegram:primary".into(),
        conversation_id: "chat-8".into(),
        thread_id: Some("thread-2".into()),
        payload: tinychannels_bus::OutboundPayload::Text {
            text: "updated draft".into(),
        },
        ..Default::default()
    };

    let tinychannels_bus::ChannelInteractionResult::Sent(outcome) = dispatch_draft_upsert(
        &drafts,
        "telegram:primary",
        Some(previous.clone()),
        intent.clone(),
    )
    .await
    .unwrap() else {
        panic!("draft upsert returns a send outcome");
    };

    assert_eq!(
        drafts.0.lock().unwrap().as_ref().unwrap(),
        &(Some(previous), intent)
    );
    assert_eq!(
        outcome.receipt.primary_platform_message_id.as_deref(),
        Some("draft-message-4")
    );
    assert_eq!(outcome.receipt.idempotency_key.as_deref(), Some("draft-4"));
    assert_eq!(outcome.receipt.channel.as_ref().unwrap().id, "telegram");
    assert_eq!(
        outcome
            .receipt
            .channel
            .as_ref()
            .unwrap()
            .account_id
            .as_deref(),
        Some("telegram:primary")
    );
    assert_eq!(outcome.receipt.conversation.as_ref().unwrap().id, "chat-8");
    assert_eq!(outcome.receipt.thread_id.as_deref(), Some("thread-2"));
    assert!(outcome.error.is_none());
}

struct RejectingDraftUpsert;

#[async_trait::async_trait]
impl tinychannels::channel::ChannelStreamingDraft for RejectingDraftUpsert {
    async fn upsert_draft(
        &self,
        _previous: Option<&tinychannels_bus::channel::MessageReceipt>,
        _intent: tinychannels::channel::ChannelOutboundIntent,
    ) -> std::result::Result<tinychannels_bus::channel::MessageReceipt, ChannelSendError> {
        Err(ChannelSendError::new("draft update rejected"))
    }
}

#[tokio::test]
async fn draft_upsert_maps_provider_errors_to_bus_failures() {
    let error = dispatch_draft_upsert(
        &RejectingDraftUpsert,
        "telegram:primary",
        None,
        tinychannels::channel::ChannelOutboundIntent::default(),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("draft update rejected"));
}

struct PreScopedDraftUpsert;

#[async_trait::async_trait]
impl tinychannels::channel::ChannelStreamingDraft for PreScopedDraftUpsert {
    async fn upsert_draft(
        &self,
        _previous: Option<&tinychannels_bus::channel::MessageReceipt>,
        _intent: tinychannels::channel::ChannelOutboundIntent,
    ) -> std::result::Result<tinychannels_bus::channel::MessageReceipt, ChannelSendError> {
        Ok(tinychannels_bus::channel::MessageReceipt {
            conversation: Some(tinychannels_bus::ConversationRef {
                id: "provider-room".into(),
                ..Default::default()
            }),
            thread_id: Some("provider-thread".into()),
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn draft_upsert_preserves_provider_receipt_conversation_and_thread() {
    let tinychannels_bus::ChannelInteractionResult::Sent(outcome) = dispatch_draft_upsert(
        &PreScopedDraftUpsert,
        "telegram:primary",
        None,
        tinychannels::channel::ChannelOutboundIntent {
            conversation_id: "requested-room".into(),
            thread_id: Some("requested-thread".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap() else {
        panic!("draft upsert returns a send outcome");
    };

    assert_eq!(
        outcome.receipt.conversation.as_ref().unwrap().id,
        "provider-room"
    );
    assert_eq!(
        outcome.receipt.thread_id.as_deref(),
        Some("provider-thread")
    );
}

struct EmptyReceiptProvider;

#[async_trait::async_trait]
impl TestProviderBehavior for EmptyReceiptProvider {
    fn test_provider_id(&self) -> &'static str {
        "telegram"
    }

    async fn test_send(
        &self,
        _message: &tinychannels_bus::ChannelOutboundMessage,
    ) -> std::result::Result<tinychannels_bus::channel::ChannelSendOutcome, ChannelSendError> {
        Ok(Default::default())
    }
}

#[tokio::test]
async fn provider_fills_missing_receipt_scope_from_the_submitted_message() {
    let host = memory_host().await;
    let hooks = Arc::new(Hooks::default());
    let _provider_host = hooks.host(host.clone(), "telegram:primary".into(), None);
    let running = Running::start_test_outbound(Arc::new(EmptyReceiptProvider), hooks, host);

    let mut message = SendMessage::new("hello", "chat-7");
    message.thread_ts = Some("thread-3".into());
    message.idempotency_key = Some("send-42".into());
    let tinychannels_bus::ChannelInteractionResult::Sent(outcome) = running
        .submit(ProviderOperation::Send(message))
        .await
        .unwrap()
    else {
        panic!("provider should return a send outcome");
    };
    let receipt = outcome.receipt;

    assert_eq!(receipt.idempotency_key.as_deref(), Some("send-42"));
    assert_eq!(
        receipt.channel,
        Some(tinychannels_bus::ChannelRef {
            id: "telegram".into(),
            account_id: Some("telegram:primary".into()),
        })
    );
    assert_eq!(
        receipt.conversation.as_ref().map(|r| r.id.as_str()),
        Some("chat-7")
    );
    assert_eq!(receipt.thread_id.as_deref(), Some("thread-3"));
    running.stop().await.unwrap();
}

struct CountingTypedProvider(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl TestProviderBehavior for CountingTypedProvider {
    fn test_provider_id(&self) -> &'static str {
        "telegram"
    }

    async fn test_send(
        &self,
        _message: &tinychannels_bus::ChannelOutboundMessage,
    ) -> std::result::Result<tinychannels_bus::channel::ChannelSendOutcome, ChannelSendError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Default::default())
    }
}

#[tokio::test]
async fn typed_send_rejects_a_foreign_account_before_provider_io() {
    let sends = Arc::new(AtomicUsize::new(0));
    let host = memory_host().await;
    let hooks = Arc::new(Hooks::default());
    let _provider_host = hooks.host(host.clone(), "telegram:primary".into(), None);
    let running =
        Running::start_test_outbound(Arc::new(CountingTypedProvider(sends.clone())), hooks, host);
    let message = tinychannels_bus::ChannelOutboundMessage {
        channel_id: "telegram:primary".into(),
        channel_ref: Some(tinychannels_bus::ChannelRef {
            id: "telegram".into(),
            account_id: Some("telegram:other".into()),
        }),
        conversation_id: "chat-7".into(),
        conversation_ref: None,
        reply_to_id: None,
        thread_id: None,
        turn_id: None,
        idempotency_key: "foreign-account".into(),
        parts: vec![tinychannels_bus::OutboundContentPart::Content {
            part: tinychannels_bus::ContentPart::Text {
                text: "hello".into(),
                markdown_dialect: None,
            },
        }],
    };

    let error = running.send_typed(message).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("does not match configured provider")
    );
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    running.stop().await.unwrap();
}

struct FailingStartAdapter;

#[async_trait::async_trait]
impl ChannelAdapter for FailingStartAdapter {
    fn descriptor(&self) -> tinychannels::channel::ChannelDescriptor {
        tinychannels::channel::ChannelDescriptor {
            id: "fixture".into(),
            display_name: "fixture".into(),
            ..Default::default()
        }
    }

    async fn start(
        &self,
        _sink: &(dyn ChannelInboundSink + Send + Sync),
    ) -> std::result::Result<(), ChannelSendError> {
        Err(ChannelSendError::new("listener unavailable"))
    }

    async fn stop(&self) -> std::result::Result<(), ChannelSendError> {
        Ok(())
    }

    async fn send(
        &self,
        _intent: tinychannels::channel::ChannelOutboundIntent,
    ) -> std::result::Result<tinychannels::channel::MessageReceipt, ChannelSendError> {
        Err(ChannelSendError::unsupported("send is not used"))
    }

    async fn status(
        &self,
    ) -> std::result::Result<tinychannels::controllers::ChannelAccountSnapshot, ChannelSendError>
    {
        Ok(Default::default())
    }
}

struct FailedIngressSink(Arc<tokio::sync::Notify>);

#[async_trait::async_trait]
impl ChannelInboundSink for FailedIngressSink {
    async fn push(
        &self,
        _envelope: tinychannels_bus::ChannelInboundEnvelope,
    ) -> std::result::Result<(), ChannelSendError> {
        self.0.notify_one();
        Err(ChannelSendError::new("host callback unavailable"))
    }
}

struct LegacyIngressChannel(Arc<tokio::sync::Notify>);

#[async_trait::async_trait]
impl Channel for LegacyIngressChannel {
    fn name(&self) -> &str {
        "legacy-ingress"
    }

    async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
        Ok(())
    }

    async fn listen(&self, tx: mpsc::Sender<tinychannels::ChannelMessage>) -> anyhow::Result<()> {
        tx.send(tinychannels::ChannelMessage {
            id: "message-1".into(),
            sender: "sender-1".into(),
            reply_target: "room-1".into(),
            content: "hello".into(),
            channel: "legacy-ingress".into(),
            ..Default::default()
        })
        .await?;
        self.0.notified().await;
        Ok(())
    }
}

#[tokio::test]
async fn adapter_listener_preserves_start_failure_for_supervision() {
    let adapter: Arc<dyn ChannelAdapter> = Arc::new(FailingStartAdapter);
    let sink: Arc<dyn ChannelInboundSink> =
        Arc::new(FailedIngressSink(Arc::new(tokio::sync::Notify::new())));

    let error = start_adapter_listener(&adapter, None, sink)
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "listener unavailable");
}

#[tokio::test]
async fn legacy_listener_logs_host_ingress_failure_and_keeps_provider_result() {
    let notify = Arc::new(tokio::sync::Notify::new());
    let channel: Arc<dyn Channel> = Arc::new(LegacyIngressChannel(notify.clone()));
    let adapter: Arc<dyn ChannelAdapter> = Arc::new(LegacyChannelAdapter(channel.clone()));
    let sink: Arc<dyn ChannelInboundSink> = Arc::new(FailedIngressSink(notify));

    let result = start_adapter_listener(&adapter, Some(&channel), sink).await;

    assert!(result.is_ok());
}

#[derive(Default)]
struct CountingTyping {
    starts: std::sync::atomic::AtomicUsize,
    stops: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl tinychannels::channel::ChannelTyping for CountingTyping {
    async fn start_typing(&self, _: &str) -> std::result::Result<(), ChannelSendError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn stop_typing(&self, _: &str) -> std::result::Result<(), ChannelSendError> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn adapter_typing_port_serves_start_stop_and_refresh_interactions() {
    use tinychannels_bus::{ChannelInteraction as Op, ChannelInteractionResult as Res};
    let port = Arc::new(CountingTyping::default());
    let dyn_port: Arc<dyn tinychannels::channel::ChannelTyping> = port.clone();
    let mut typing = TypingRefreshes::new();
    let started = dispatch_adapter_typing(
        &dyn_port,
        "telegram",
        &Op::StartTyping {
            recipient: "chat".into(),
        },
        &mut typing,
    )
    .await
    .expect("typing interaction is handled")
    .unwrap();
    assert!(matches!(started, Res::Completed));
    assert_eq!(port.starts.load(Ordering::SeqCst), 1);

    let handle = match dispatch_adapter_typing(
        &dyn_port,
        "telegram",
        &Op::StartTypingRefresh {
            recipient: "chat".into(),
            interval_secs: 4,
        },
        &mut typing,
    )
    .await
    .unwrap()
    .unwrap()
    {
        Res::TypingRefresh { handle } => handle,
        other => panic!("expected a refresh handle, got {other:?}"),
    };
    tokio::time::sleep(Duration::from_secs(9)).await;
    assert!(
        port.starts.load(Ordering::SeqCst) >= 3,
        "refresh re-issues typing"
    );
    dispatch_adapter_typing(
        &dyn_port,
        "telegram",
        &Op::StopTypingRefresh { handle },
        &mut typing,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(typing.is_empty());
    let stopped = dispatch_adapter_typing(
        &dyn_port,
        "telegram",
        &Op::StopTyping {
            recipient: "chat".into(),
        },
        &mut typing,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(stopped, Res::Completed));
    assert_eq!(port.stops.load(Ordering::SeqCst), 1);
    assert!(
        dispatch_adapter_typing(&dyn_port, "telegram", &Op::Describe, &mut typing)
            .await
            .is_none()
    );
}
