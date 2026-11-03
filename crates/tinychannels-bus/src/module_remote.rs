//! Remote chat commands and host-owned conversation state vocabulary.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A remote-control slash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCommand {
    Status,
    Sessions,
    New,
    Help,
}

/// The model route configured by the host for this sender.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteRoute {
    pub provider: String,
    pub model: String,
}
/// A host-owned conversation listed by the module's sessions command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteThreadSummary {
    pub id: String,
    pub title: String,
    pub message_count: usize,
    pub last_message_at: String,
}
/// A conversation the host persists after a module-owned new command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NewRemoteThread {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub labels: Vec<String>,
}
/// Trusted ingress identity carried back to the host for a remote conversation
/// reset. The host validates it against the callback's captured ingress before
/// applying any durable session operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteSessionScope {
    pub channel: crate::ChannelRef,
    pub conversation: crate::ConversationRef,
    /// Opaque deterministic identity produced by host channel preparation.
    pub session_key: String,
}
/// Host-validated state root and one incoming command context.
#[allow(
    clippy::large_enum_variant,
    reason = "remote operation variants retain their established wire representation"
)]
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", content = "request", rename_all = "snake_case")]
pub enum RemoteOperation {
    Execute {
        workspace_dir: String,
        message: Box<crate::ChannelMessage>,
        route: RemoteRoute,
        history_len: usize,
        session_scope: Option<RemoteSessionScope>,
    },
    MarkTurn {
        workspace_dir: String,
        channel: String,
        reply_target: String,
        busy: bool,
    },
}
/// The module identifies and renders commands; an ordinary message has no reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteOperationResult {
    pub reply: Option<String>,
}
/// Product state operations supplied by the host over its existing callback.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", content = "request", rename_all = "snake_case")]
pub enum RemoteHostRequest {
    ListThreads,
    CreateThread(NewRemoteThread),
    ClearHistory { session_scope: RemoteSessionScope },
    InvalidateThread { thread_id: String },
}
/// Results supplied by the host's conversation and session stores.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
pub enum RemoteHostResult {
    Threads(Vec<RemoteThreadSummary>),
    Completed,
}
