//! Remote control against an in-memory host.

use std::collections::HashMap;
use std::sync::Mutex;

use super::session_store::{RemoteSessionStore, chat_key, with_store_read};
use super::*;

#[derive(Default)]
struct FakeHost {
    history: Mutex<HashMap<String, usize>>,
    threads: Mutex<Vec<RemoteThreadSummary>>,
    created: Mutex<Vec<NewRemoteThread>>,
    invalidated: Mutex<Vec<String>>,
    fail_list: bool,
    fail_create: bool,
}

#[async_trait]
impl RemoteControlHost for FakeHost {
    fn route(&self, _sender_key: &str) -> RemoteRoute {
        RemoteRoute {
            provider: "openai".into(),
            model: "gpt-5".into(),
        }
    }
    fn history_len(&self, sender_key: &str) -> usize {
        self.history
            .lock()
            .unwrap()
            .get(sender_key)
            .copied()
            .unwrap_or(0)
    }
    fn clear_history(&self, sender_key: &str) {
        self.history.lock().unwrap().remove(sender_key);
    }
    async fn list_threads(&self) -> anyhow::Result<Vec<RemoteThreadSummary>> {
        if self.fail_list {
            anyhow::bail!("disk on fire");
        }
        Ok(self.threads.lock().unwrap().clone())
    }
    async fn create_thread(&self, thread: NewRemoteThread) -> anyhow::Result<()> {
        if self.fail_create {
            anyhow::bail!("read-only");
        }
        self.created.lock().unwrap().push(thread);
        Ok(())
    }
    async fn invalidate_thread(&self, thread_id: &str) {
        self.invalidated.lock().unwrap().push(thread_id.into());
    }
}

fn ctx(channel: &str, dir: &std::path::Path) -> RemoteCommandContext {
    RemoteCommandContext {
        channel: channel.into(),
        reply_target: "chat-1".into(),
        sender_key: format!("{channel}_alice_chat-1"),
        workspace_dir: dir.to_path_buf(),
    }
}

fn thread(id: &str, title: &str, at: &str) -> RemoteThreadSummary {
    RemoteThreadSummary {
        id: id.into(),
        title: title.into(),
        message_count: 3,
        last_message_at: at.into(),
    }
}

#[test]
fn parses_commands_across_mentions_and_case() {
    assert_eq!(
        parse_remote_command(" /STATUS@OpenHumanBot now "),
        Some(RemoteCommand::Status)
    );
    assert_eq!(
        parse_remote_command("/sessions"),
        Some(RemoteCommand::Sessions)
    );
    assert_eq!(parse_remote_command("/new"), Some(RemoteCommand::New));
    assert_eq!(parse_remote_command("/help"), Some(RemoteCommand::Help));
    assert_eq!(parse_remote_command("/model"), None);
    assert_eq!(parse_remote_command("status"), None);
    assert_eq!(parse_remote_command(""), None);
}

#[test]
fn renderers_include_their_inputs() {
    assert!(build_remote_help_response().contains("`/status`"));
    assert!(format_session_line("", "thread-1", 2, true).starts_with("→ `thread-1`"));
    assert!(format_session_line("Title", "t", 2, false).starts_with("  `Title`"));
    assert!(build_new_session_response("Today", "thread-1").contains("thread-1"));
    assert!(
        build_status_response("Thread: none", "openai", "gpt-5", 3, true).contains("in progress")
    );
    assert!(build_status_response("Thread: none", "openai", "gpt-5", 3, false).contains("idle"));
}

#[tokio::test]
async fn new_then_status_on_any_chat_provider() {
    for channel in ["telegram", "discord", "slack"] {
        let dir = tempfile::tempdir().unwrap();
        let host = FakeHost::default();
        let c = ctx(channel, dir.path());
        host.history.lock().unwrap().insert(c.sender_key.clone(), 4);

        let before = execute_remote_command(&host, &c, RemoteCommand::Status).await;
        assert!(before.contains("none — send `/new`"), "{channel}: {before}");
        assert!(before.contains("In-memory turns: 4"));

        let reply = execute_remote_command(&host, &c, RemoteCommand::New).await;
        let created = host.created.lock().unwrap().clone();
        assert_eq!(created.len(), 1);
        assert_eq!(
            created[0].labels,
            vec![channel.to_string(), "remote".into()]
        );
        assert!(reply.contains(&created[0].id));
        assert_eq!(host.history_len(&c.sender_key), 0, "history cleared");
        assert_eq!(
            *host.invalidated.lock().unwrap(),
            vec![created[0].id.clone()]
        );

        let after = execute_remote_command(&host, &c, RemoteCommand::Status).await;
        assert!(after.contains(&created[0].id), "{channel}: {after}");
        assert!(after.contains("Provider: `openai`"));
    }
}

#[tokio::test]
async fn new_session_titles_use_the_channel_display_name() {
    let dir = tempfile::tempdir().unwrap();
    let host = FakeHost::default();
    let now = chrono::DateTime::parse_from_rfc3339("2026-09-29T15:04:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    new_session(&host, &ctx("telegram", dir.path()), now).await;
    new_session(&host, &ctx("slack", dir.path()), now).await;
    let titles: Vec<_> = host
        .created
        .lock()
        .unwrap()
        .iter()
        .map(|t| t.title.clone())
        .collect();
    assert_eq!(titles, ["Telegram Sep 29 3:04 PM", "Slack Sep 29 3:04 PM"]);
}

#[tokio::test]
async fn new_session_reports_create_failure_without_binding() {
    let dir = tempfile::tempdir().unwrap();
    let host = FakeHost {
        fail_create: true,
        ..Default::default()
    };
    let c = ctx("discord", dir.path());
    let reply = execute_remote_command(&host, &c, RemoteCommand::New).await;
    assert_eq!(reply, "Failed to create session: read-only");
    let bound =
        with_store_read(dir.path(), |s| Ok(s.binding("discord", "chat-1").cloned())).unwrap();
    assert!(bound.is_none());
}

#[tokio::test]
async fn sessions_lists_recent_first_and_marks_active() {
    let dir = tempfile::tempdir().unwrap();
    let host = FakeHost::default();
    let c = ctx("telegram", dir.path());
    assert!(
        execute_remote_command(&host, &c, RemoteCommand::Sessions)
            .await
            .contains("No conversation threads yet")
    );
    execute_remote_command(&host, &c, RemoteCommand::New).await;
    let active = host.created.lock().unwrap()[0].id.clone();
    {
        let mut threads = host.threads.lock().unwrap();
        threads.push(thread("old", "Old", "2026-01-01"));
        threads.push(thread(&active, "Active", "2026-03-01"));
        for i in 0..10 {
            threads.push(thread(&format!("filler-{i}"), "F", "2025-01-01"));
        }
    }
    let listing = execute_remote_command(&host, &c, RemoteCommand::Sessions).await;
    let lines: Vec<_> = listing.lines().collect();
    assert_eq!(lines.len(), 3 + SESSIONS_LIST_LIMIT);
    assert!(lines[3].starts_with("→ `Active`"));
    assert!(lines[4].contains("`Old`"));

    let failing = FakeHost {
        fail_list: true,
        ..Default::default()
    };
    assert_eq!(
        execute_remote_command(&failing, &c, RemoteCommand::Sessions).await,
        "Could not list sessions: disk on fire"
    );
}

#[tokio::test]
async fn help_needs_no_state() {
    let dir = tempfile::tempdir().unwrap();
    let reply = execute_remote_command(
        &FakeHost::default(),
        &ctx("irc", dir.path()),
        RemoteCommand::Help,
    )
    .await;
    assert_eq!(reply, build_remote_help_response());
}

#[tokio::test]
async fn busy_state_is_per_channel_and_chat() {
    let dir = tempfile::tempdir().unwrap();
    mark_turn(dir.path(), "discord", "chat-1", true).await;
    let (busy, other_channel) = with_store_read(dir.path(), |s| {
        Ok((
            s.is_busy("discord", "chat-1"),
            s.is_busy("telegram", "chat-1"),
        ))
    })
    .unwrap();
    assert!(busy && !other_channel);
    let status = execute_remote_command(
        &FakeHost::default(),
        &ctx("discord", dir.path()),
        RemoteCommand::Status,
    )
    .await;
    assert!(status.contains("in progress"));
    mark_turn(dir.path(), "discord", "chat-1", false).await;
    assert!(!with_store_read(dir.path(), |s| Ok(s.is_busy("discord", "chat-1"))).unwrap());
}

#[test]
fn legacy_telegram_bindings_are_adopted_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("state/telegram_remote_sessions.json");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let legacy_raw = r#"{"bindings":{"chat-9":{"thread_id":"thread-old","sender_key":"telegram_bob_chat-9","updated_at":"2026-01-01T00:00:00Z","title":"Old"}},"busy_reply_targets":{"chat-9":true}}"#;
    std::fs::write(&legacy, legacy_raw).unwrap();

    let mut store = RemoteSessionStore::load(dir.path()).unwrap();
    let binding = store.binding("telegram", "chat-9").cloned().unwrap();
    assert_eq!(binding.thread_id, "thread-old");
    assert!(!store.is_busy("telegram", "chat-9"), "busy is not adopted");
    assert!(store.binding("discord", "chat-9").is_none());

    store.set_busy("telegram", "chat-9", true);
    store.save().unwrap();
    assert_eq!(std::fs::read_to_string(&legacy).unwrap(), legacy_raw);

    // Once the shared store exists, it wins over the legacy file.
    std::fs::write(&legacy, r#"{"bindings":{}}"#).unwrap();
    let reloaded = RemoteSessionStore::load(dir.path()).unwrap();
    assert!(reloaded.binding("telegram", "chat-9").is_some());
}

#[test]
fn corrupt_store_resets_instead_of_failing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state/channel_remote_sessions.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{not json").unwrap();
    let store = RemoteSessionStore::load(dir.path()).unwrap();
    assert!(store.binding("telegram", "x").is_none());
    assert_eq!(chat_key("slack", "C1"), "slack:C1");
}
