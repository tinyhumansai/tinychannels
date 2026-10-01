//! Remote control of a host session from inside a chat.
//!
//! Every provider whose [`crate::capabilities::ChannelCapabilities`] sets
//! `remote_control` understands four slash commands:
//!
//! - `/status`: the bound thread, model route, history size and turn state
//! - `/sessions`: the host's recent conversation threads
//! - `/new`: start a fresh thread and bind this chat to it
//! - `/help`: the command list
//!
//! Parsing, rendering, the chat → thread bindings ([`session_store`]) and the
//! command flow live here. The conversation threads, model routes and
//! in-memory history belong to the host, which exposes them through
//! [`RemoteControlHost`].

pub mod session_store;

use std::path::PathBuf;

use async_trait::async_trait;

pub use session_store::{ChatBinding, RemoteSessionStore, chat_key, mark_turn};
use session_store::{with_store, with_store_read};

const LOG_PREFIX: &str = "[channel-remote]";

/// Maximum number of sessions listed by `/sessions`.
pub const SESSIONS_LIST_LIMIT: usize = 8;

/// A remote-control command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteCommand {
    Status,
    Sessions,
    New,
    Help,
}

/// Parse a remote-control command. Accepts bot mentions (`/status@MyBot`) and
/// any letter case. Returns `None` for anything else, including other slash
/// commands such as `/model`.
pub fn parse_remote_command(content: &str) -> Option<RemoteCommand> {
    let command = content.split_whitespace().next()?;
    let command = command
        .strip_prefix('/')?
        .split('@')
        .next()
        .unwrap_or(command)
        .to_ascii_lowercase();
    match command.as_str() {
        "status" => Some(RemoteCommand::Status),
        "sessions" => Some(RemoteCommand::Sessions),
        "new" => Some(RemoteCommand::New),
        "help" => Some(RemoteCommand::Help),
        _ => None,
    }
}

/// Render the `/help` text.
pub fn build_remote_help_response() -> String {
    [
        "Remote control:",
        "",
        "• `/status` — active thread, model, and turn state",
        "• `/sessions` — recent conversation threads",
        "• `/new` — start a fresh thread for this chat",
        "• `/help` — this message",
        "",
        "Model routing: `/model`, `/models` (where supported).",
    ]
    .join("\n")
}

/// Render one row of `/sessions`.
pub fn format_session_line(title: &str, id: &str, message_count: usize, active: bool) -> String {
    let marker = if active { "→ " } else { "  " };
    let title = if title.trim().is_empty() { id } else { title };
    format!("{marker}`{title}` — {message_count} msgs (id: `{id}`)")
}

/// Render the `/new` confirmation.
pub fn build_new_session_response(title: &str, thread_id: &str) -> String {
    format!(
        "Started new session **{title}**.\nThread id: `{thread_id}`\nIn-memory channel history cleared for this chat."
    )
}

/// Render `/status` from host-supplied state.
pub fn build_status_response(
    thread_line: &str,
    provider: &str,
    model: &str,
    history_len: usize,
    busy: bool,
) -> String {
    let turn_state = if busy { "in progress ⏳" } else { "idle" };
    format!(
        "**Status**\n{thread_line}\nProvider: `{provider}`\nModel: `{model}`\nIn-memory turns: {history_len}\nTurn: {turn_state}"
    )
}

/// The model route a sender's turns currently use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRoute {
    pub provider: String,
    pub model: String,
}

/// A host conversation thread, as `/sessions` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteThreadSummary {
    pub id: String,
    pub title: String,
    pub message_count: usize,
    /// Sort key, most recent first. Any consistently ordered string (RFC 3339
    /// timestamps sort correctly).
    pub last_message_at: String,
}

/// A thread for the host to create on `/new`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRemoteThread {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub labels: Vec<String>,
}

/// Host side of remote control: the parts only the host owns.
#[async_trait]
pub trait RemoteControlHost: Send + Sync {
    /// The model route for `sender_key` (a conversation history key).
    fn route(&self, sender_key: &str) -> RemoteRoute;
    /// How many turns of in-memory history the host holds for `sender_key`.
    fn history_len(&self, sender_key: &str) -> usize;
    /// Drop the in-memory history for `sender_key`.
    fn clear_history(&self, sender_key: &str);
    /// List the host's conversation threads.
    async fn list_threads(&self) -> anyhow::Result<Vec<RemoteThreadSummary>>;
    /// Create a conversation thread.
    async fn create_thread(&self, thread: NewRemoteThread) -> anyhow::Result<()>;
    /// Drop any cached agent session for `thread_id`, so the next turn starts
    /// from the new binding.
    async fn invalidate_thread(&self, thread_id: &str);
}

/// Who issued a remote-control command, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCommandContext {
    /// Provider id of the channel (`telegram`, `discord`, ...).
    pub channel: String,
    /// Chat the command came from; bindings are per chat.
    pub reply_target: String,
    /// Conversation history key of the sender.
    pub sender_key: String,
    /// Workspace holding the session store.
    pub workspace_dir: PathBuf,
}

/// Run a remote-control command and return the reply text.
pub async fn execute_remote_command(
    host: &dyn RemoteControlHost,
    ctx: &RemoteCommandContext,
    command: RemoteCommand,
) -> String {
    tracing::debug!(
        "{LOG_PREFIX} command={command:?} channel={} reply_target={}",
        ctx.channel,
        ctx.reply_target
    );
    match command {
        RemoteCommand::Status => status(host, ctx).await,
        RemoteCommand::Sessions => sessions(host, ctx).await,
        RemoteCommand::New => new_session(host, ctx, chrono::Utc::now()).await,
        RemoteCommand::Help => build_remote_help_response(),
    }
}

async fn read_chat_state(ctx: &RemoteCommandContext) -> (Option<ChatBinding>, bool) {
    let workspace = ctx.workspace_dir.clone();
    let (channel, reply_target) = (ctx.channel.clone(), ctx.reply_target.clone());
    tokio::task::spawn_blocking(move || {
        with_store_read(&workspace, |store| {
            Ok((
                store.binding(&channel, &reply_target).cloned(),
                store.is_busy(&channel, &reply_target),
            ))
        })
    })
    .await
    .unwrap_or_else(|join_err| {
        tracing::warn!("{LOG_PREFIX} join error reading session store: {join_err}");
        Ok((None, false))
    })
    .unwrap_or_else(|store_err| {
        tracing::warn!("{LOG_PREFIX} session store error: {store_err}");
        (None, false)
    })
}

async fn status(host: &dyn RemoteControlHost, ctx: &RemoteCommandContext) -> String {
    let route = host.route(&ctx.sender_key);
    let history_len = host.history_len(&ctx.sender_key);
    let (binding, busy) = read_chat_state(ctx).await;
    let thread_line = match binding {
        Some(ChatBinding {
            ref thread_id,
            ref title,
            ..
        }) => {
            let display_title = title
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or(thread_id);
            format!("Thread: `{display_title}` (`{thread_id}`)")
        }
        None => "Thread: _(none — send `/new` to bind a thread)_".to_string(),
    };
    build_status_response(
        &thread_line,
        &route.provider,
        &route.model,
        history_len,
        busy,
    )
}

async fn sessions(host: &dyn RemoteControlHost, ctx: &RemoteCommandContext) -> String {
    let active_thread_id = read_chat_state(ctx).await.0.map(|b| b.thread_id);
    let mut threads = match host.list_threads().await {
        Ok(list) => list,
        Err(error) => {
            tracing::warn!("{LOG_PREFIX} sessions: list_threads failed: {error}");
            return format!("Could not list sessions: {error}");
        }
    };
    if threads.is_empty() {
        return "No conversation threads yet. Send `/new` to create one.".to_string();
    }
    threads.sort_by(|a, b| b.last_message_at.cmp(&a.last_message_at));

    let mut lines = vec![
        "**Recent sessions**".to_string(),
        format!("Showing up to {SESSIONS_LIST_LIMIT} threads:"),
        String::new(),
    ];
    for thread in threads.into_iter().take(SESSIONS_LIST_LIMIT) {
        lines.push(format_session_line(
            &thread.title,
            &thread.id,
            thread.message_count,
            active_thread_id.as_deref() == Some(thread.id.as_str()),
        ));
    }
    lines.join("\n")
}

/// Human-readable channel name for thread titles (`telegram` → `Telegram`).
fn channel_display_name(channel: &str) -> String {
    crate::controllers::find_channel_definition(channel)
        .map(|definition| definition.display_name.to_string())
        .unwrap_or_else(|| {
            let mut chars = channel.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        })
}

async fn new_session(
    host: &dyn RemoteControlHost,
    ctx: &RemoteCommandContext,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let thread_id = format!("thread-{}", uuid::Uuid::new_v4());
    let title = format!(
        "{} {} {}",
        channel_display_name(&ctx.channel),
        now.format("%b %-d"),
        now.format("%-I:%M %p")
    );
    let thread = NewRemoteThread {
        id: thread_id.clone(),
        title: title.clone(),
        created_at: now.to_rfc3339(),
        labels: vec![ctx.channel.clone(), "remote".to_string()],
    };
    if let Err(error) = host.create_thread(thread).await {
        tracing::warn!("{LOG_PREFIX} new: create_thread failed: {error}");
        return format!("Failed to create session: {error}");
    }

    host.clear_history(&ctx.sender_key);

    let workspace_dir = ctx.workspace_dir.clone();
    let (channel, reply_target) = (ctx.channel.clone(), ctx.reply_target.clone());
    let (thread_owned, sender_owned, title_owned) =
        (thread_id.clone(), ctx.sender_key.clone(), title.clone());
    let bind_result = tokio::task::spawn_blocking(move || {
        with_store(&workspace_dir, |store| {
            store.set_binding(
                &channel,
                &reply_target,
                thread_owned,
                sender_owned,
                Some(title_owned),
            );
            Ok(())
        })
    })
    .await
    .unwrap_or_else(|e| Err(anyhow::anyhow!("join error: {e}")));

    if let Err(error) = bind_result {
        tracing::warn!("{LOG_PREFIX} new: persist binding failed: {error}");
        return format!(
            "Created thread `{thread_id}` but failed to persist the chat binding: {error}"
        );
    }

    host.invalidate_thread(&thread_id).await;
    tracing::info!(
        "{LOG_PREFIX} new session thread_id={thread_id} channel={} reply_target={}",
        ctx.channel,
        ctx.reply_target
    );
    build_new_session_response(&title, &thread_id)
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;
