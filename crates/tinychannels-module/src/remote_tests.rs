use super::*;
async fn execute(
    callback: &HostChannels,
    request: RemoteOperation,
) -> tinybus::Result<RemoteOperationResult> {
    let remote_control = match &request {
        RemoteOperation::Execute { message, .. } => message.channel != "email",
        RemoteOperation::MarkTurn { channel, .. } => channel != "email",
    };
    super::execute(
        callback,
        request,
        tinychannels_bus::ChannelCapabilities {
            remote_control,
            ..Default::default()
        },
    )
    .await
}
use std::sync::Arc;
use tinybus::{broker::Broker, transport::memory::MemoryBus};
use tokio::sync::Mutex;

#[derive(Clone)]
struct Callback {
    sessions: tinychannels::remote::session_store::RemoteSessionStore,
    calls: Arc<Mutex<Vec<String>>>,
}
#[tinybus::interface(name = "ai.tinyhumans.tinychannels.ChannelsHost")]
impl Callback {
    async fn dispatch_controller_backend(
        &self,
        request: ChannelBackendRequest,
    ) -> Result<ChannelBackendResult> {
        let ChannelBackendRequest::RemoteControl(request) = request else {
            return Err(Error::failed("unexpected callback"));
        };
        let mut calls = self.calls.lock().await;
        let result = match request {
            RemoteHostRequest::CreateThread(thread) => {
                assert!(thread.id.starts_with("thread-"));
                assert_eq!(thread.labels, vec!["telegram", "remote"]);
                calls.push("create".into());
                RemoteHostResult::Completed
            }
            RemoteHostRequest::ClearHistory { session_scope } => {
                assert_eq!(session_scope, scope());
                assert!(
                    self.sessions
                        .binding("telegram", "chat")
                        .await
                        .unwrap()
                        .is_none()
                );
                calls.push("clear".into());
                RemoteHostResult::Completed
            }
            RemoteHostRequest::InvalidateThread { thread_id } => {
                assert_eq!(
                    self.sessions
                        .binding("telegram", "chat")
                        .await
                        .unwrap()
                        .unwrap()
                        .thread_id,
                    thread_id
                );
                calls.push("invalidate".into());
                RemoteHostResult::Completed
            }
            RemoteHostRequest::ListThreads => {
                calls.push("list".into());
                RemoteHostResult::Threads(vec![
                    RemoteThreadSummary {
                        id: "old".into(),
                        title: "Older".into(),
                        message_count: 1,
                        last_message_at: "2025".into(),
                    },
                    RemoteThreadSummary {
                        id: "new".into(),
                        title: "Newer".into(),
                        message_count: 2,
                        last_message_at: "2026".into(),
                    },
                ])
            }
        };
        Ok(ChannelBackendResult::RemoteControl(result))
    }
}
async fn fixture(
    _workspace: &std::path::Path,
) -> (HostChannels, Callback, crate::storage_fixture::StorageHost) {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    connection
        .request_name(tinychannels_bus::HOST_BUS_NAME)
        .await
        .unwrap();
    let storage = crate::storage_fixture::StorageHost::serve(&connection, "remote").await;
    let callback = Callback {
        sessions: tinychannels::remote::session_store::RemoteSessionStore::new(
            storage.docs("remote"),
        ),
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    connection
        .serve_at(
            tinychannels_bus::HOST_OBJECT_PATH.try_into().unwrap(),
            callback.clone(),
        )
        .await
        .unwrap();
    let host = HostChannels::new(connection);
    host.storage()
        .await
        .unwrap_or_else(|error| panic!("remote fixture storage must connect: {error:?}"));
    (host, callback, storage)
}
fn command(workspace: &std::path::Path, content: &str, channel: &str) -> RemoteOperation {
    RemoteOperation::Execute {
        workspace_dir: workspace.to_str().unwrap().into(),
        message: Box::new(tinychannels_bus::ChannelMessage {
            id: "fixture".into(),
            sender: "sender".into(),
            reply_target: "chat".into(),
            content: content.into(),
            channel: channel.into(),
            timestamp: 1,
            thread_ts: None,
            sender_name: None,
            topic_id: None,
            reply_to_id: None,
        }),
        route: RemoteRoute {
            provider: "fixture-provider".into(),
            model: "fixture-model".into(),
        },
        history_len: 7,
        session_scope: Some(scope()),
    }
}
fn scope() -> tinychannels_bus::module_remote::RemoteSessionScope {
    tinychannels_bus::module_remote::RemoteSessionScope {
        channel: tinychannels_bus::ChannelRef {
            id: "telegram".into(),
            account_id: Some("telegram:fixture".into()),
        },
        conversation: tinychannels_bus::ConversationRef {
            kind: tinychannels_bus::ConversationKind::Dm,
            id: "chat".into(),
            ..Default::default()
        },
        session_key: "canonical-telegram-session".into(),
    }
}
#[tokio::test]
async fn new_command_awaits_host_history_clear_before_persisting_binding() {
    let workspace = tempfile::tempdir().unwrap();
    let (host, callback, _storage) = fixture(workspace.path()).await;
    let result = execute(&host, command(workspace.path(), "/new", "telegram"))
        .await
        .unwrap();
    let reply = result.reply.unwrap();
    assert!(
        reply.contains("Started new session"),
        "unexpected `/new` reply: {reply}"
    );
    assert_eq!(
        *callback.calls.lock().await,
        vec!["create", "clear", "invalidate"]
    );
    let result = execute(&host, command(workspace.path(), "/status", "telegram"))
        .await
        .unwrap();
    let text = result.reply.unwrap();
    assert!(text.contains("thread-"));
    assert!(text.contains("fixture-provider"));
    assert!(text.contains("Session turns: 7"));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn new_without_prepared_session_scope_never_calls_host_clear() {
    let workspace = tempfile::tempdir().unwrap();
    let (host, callback, _storage) = fixture(workspace.path()).await;
    let mut request = command(workspace.path(), "/new", "telegram");
    let RemoteOperation::Execute { session_scope, .. } = &mut request else {
        unreachable!()
    };
    *session_scope = None;

    let result = execute(&host, request).await.unwrap();

    assert!(
        result
            .reply
            .unwrap()
            .contains("trusted channel scope is unavailable")
    );
    assert!(callback.calls.lock().await.is_empty());
}
#[tokio::test]
async fn sessions_are_sorted_and_turn_state_is_owned_by_the_module() {
    let workspace = tempfile::tempdir().unwrap();
    let (host, callback, _storage) = fixture(workspace.path()).await;
    execute(
        &host,
        RemoteOperation::MarkTurn {
            workspace_dir: workspace.path().to_str().unwrap().into(),
            channel: "telegram".into(),
            reply_target: "chat".into(),
            busy: true,
        },
    )
    .await
    .unwrap();
    let status = execute(&host, command(workspace.path(), "/status", "telegram"))
        .await
        .unwrap()
        .reply
        .unwrap();
    assert!(
        status.contains("in progress"),
        "busy turn was not rendered: {status}"
    );
    let text = execute(&host, command(workspace.path(), "/sessions", "telegram"))
        .await
        .unwrap()
        .reply
        .unwrap();
    assert!(text.find("Newer").unwrap() < text.find("Older").unwrap());
    assert_eq!(*callback.calls.lock().await, vec!["list"]);
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn ordinary_messages_and_unsupported_channels_do_not_call_host_state() {
    let workspace = tempfile::tempdir().unwrap();
    let (host, callback, _storage) = fixture(workspace.path()).await;
    for (text, channel) in [("ordinary message", "telegram"), ("/new", "email")] {
        assert_eq!(
            execute(&host, command(workspace.path(), text, channel))
                .await
                .unwrap()
                .reply,
            None
        );
    }
    assert!(callback.calls.lock().await.is_empty());
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn legacy_workspace_bindings_are_imported_once_and_left_untouched() {
    let workspace = tempfile::tempdir().unwrap();
    let state = workspace.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let legacy = state.join("telegram_remote_sessions.json");
    let body = serde_json::json!({
        "bindings": { "chat": {
            "thread_id": "thread-legacy", "sender_key": "telegram_sender_chat",
            "updated_at": "2025-01-01T00:00:00Z", "title": "Legacy chat"
        }}
    })
    .to_string();
    std::fs::write(&legacy, &body).unwrap();
    let (host, _callback, _storage) = fixture(workspace.path()).await;
    let status = execute(&host, command(workspace.path(), "/status", "telegram"))
        .await
        .unwrap()
        .reply
        .unwrap();
    assert!(status.contains("thread-legacy"), "{status}");
    assert_eq!(std::fs::read_to_string(&legacy).unwrap(), body);
    assert!(!state.join("channel_remote_sessions.json").exists());
    host.shutdown().await.unwrap();
}
