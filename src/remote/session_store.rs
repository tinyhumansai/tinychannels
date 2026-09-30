//! Workspace-backed chat → thread bindings and busy state for remote control.
//!
//! One store serves every channel. Entries are keyed by `<channel>:<reply
//! target>`, so the same chat id on two providers never collides.
//!
//! Hosts that ran Telegram-only remote control wrote
//! `state/telegram_remote_sessions.json`, keyed by bare reply target. The first
//! load of a workspace with no shared store adopts those bindings under the
//! `telegram:` prefix. The legacy file is read, never modified.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

const STORE_FILE: &str = "state/channel_remote_sessions.json";
const LEGACY_TELEGRAM_STORE_FILE: &str = "state/telegram_remote_sessions.json";
const LOG_PREFIX: &str = "[channel-remote]";

/// A chat's binding to a host conversation thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatBinding {
    pub thread_id: String,
    pub sender_key: String,
    pub updated_at: String,
    /// Title captured at `/new` time so `/status` can show it without listing
    /// every thread.
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SessionStoreFile {
    #[serde(default)]
    bindings: HashMap<String, ChatBinding>,
    #[serde(default)]
    busy_reply_targets: HashMap<String, bool>,
}

/// Store key for one chat on one channel.
pub fn chat_key(channel: &str, reply_target: &str) -> String {
    format!("{channel}:{reply_target}")
}

/// The remote-control session store for one workspace.
pub struct RemoteSessionStore {
    file: SessionStoreFile,
    path: PathBuf,
}

fn read_store_file(path: &Path) -> anyhow::Result<Option<SessionStoreFile>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&raw).unwrap_or_else(|error| {
        tracing::warn!(
            "{LOG_PREFIX} corrupt session store at {}: {error}; resetting",
            path.display()
        );
        SessionStoreFile::default()
    })))
}

impl RemoteSessionStore {
    /// Load the workspace's store, adopting legacy Telegram bindings when the
    /// shared store does not exist yet.
    pub fn load(workspace_dir: &Path) -> anyhow::Result<Self> {
        let path = workspace_dir.join(STORE_FILE);
        let file = match read_store_file(&path)? {
            Some(file) => file,
            None => Self::adopt_legacy_telegram(workspace_dir)?,
        };
        tracing::debug!(
            "{LOG_PREFIX} loaded session store bindings={} busy={}",
            file.bindings.len(),
            file.busy_reply_targets.len()
        );
        Ok(Self { file, path })
    }

    fn adopt_legacy_telegram(workspace_dir: &Path) -> anyhow::Result<SessionStoreFile> {
        let legacy_path = workspace_dir.join(LEGACY_TELEGRAM_STORE_FILE);
        let Some(legacy) = read_store_file(&legacy_path)? else {
            return Ok(SessionStoreFile::default());
        };
        // Busy flags are per-process turn state; only bindings carry over.
        let bindings: HashMap<_, _> = legacy
            .bindings
            .into_iter()
            .map(|(reply_target, binding)| (chat_key("telegram", &reply_target), binding))
            .collect();
        tracing::info!(
            "{LOG_PREFIX} adopted {} legacy telegram binding(s) from {}",
            bindings.len(),
            legacy_path.display()
        );
        Ok(SessionStoreFile {
            bindings,
            busy_reply_targets: HashMap::new(),
        })
    }

    /// Write the store to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(&self.file)?)?;
        Ok(())
    }

    pub fn binding(&self, channel: &str, reply_target: &str) -> Option<&ChatBinding> {
        self.file.bindings.get(&chat_key(channel, reply_target))
    }

    pub fn set_binding(
        &mut self,
        channel: &str,
        reply_target: &str,
        thread_id: String,
        sender_key: String,
        title: Option<String>,
    ) {
        self.file.bindings.insert(
            chat_key(channel, reply_target),
            ChatBinding {
                thread_id,
                sender_key,
                updated_at: chrono::Utc::now().to_rfc3339(),
                title,
            },
        );
    }

    pub fn set_busy(&mut self, channel: &str, reply_target: &str, busy: bool) {
        let key = chat_key(channel, reply_target);
        if busy {
            self.file.busy_reply_targets.insert(key, true);
        } else {
            self.file.busy_reply_targets.remove(&key);
        }
    }

    pub fn is_busy(&self, channel: &str, reply_target: &str) -> bool {
        self.file
            .busy_reply_targets
            .get(&chat_key(channel, reply_target))
            .copied()
            .unwrap_or(false)
    }
}

static STORE: OnceLock<Mutex<Option<RemoteSessionStore>>> = OnceLock::new();

fn with_cached<R>(
    workspace_dir: &Path,
    f: impl FnOnce(&mut RemoteSessionStore) -> anyhow::Result<R>,
) -> anyhow::Result<R> {
    let lock = STORE.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let expected_path = workspace_dir.join(STORE_FILE);
    if guard
        .as_ref()
        .is_none_or(|store| store.path != expected_path)
    {
        *guard = Some(RemoteSessionStore::load(workspace_dir)?);
    }
    let store = guard
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("session store not initialized"))?;
    f(store)
}

/// Run `f` against the cached store and flush it to disk.
pub fn with_store<F, R>(workspace_dir: &Path, f: F) -> anyhow::Result<R>
where
    F: FnOnce(&mut RemoteSessionStore) -> anyhow::Result<R>,
{
    with_cached(workspace_dir, |store| {
        let result = f(store)?;
        store.save()?;
        Ok(result)
    })
}

/// Run `f` against the cached store without writing to disk.
pub fn with_store_read<F, R>(workspace_dir: &Path, f: F) -> anyhow::Result<R>
where
    F: FnOnce(&RemoteSessionStore) -> anyhow::Result<R>,
{
    with_cached(workspace_dir, |store| f(store))
}

/// Record whether a turn is running for a chat, for `/status`. Blocking I/O
/// runs off the async executor. Failures are logged, never returned: busy
/// state is advisory.
pub async fn mark_turn(workspace_dir: &Path, channel: &str, reply_target: &str, busy: bool) {
    let workspace_dir = workspace_dir.to_path_buf();
    let (channel_owned, target_owned) = (channel.to_string(), reply_target.to_string());
    let result = tokio::task::spawn_blocking(move || {
        with_store(&workspace_dir, |store| {
            store.set_busy(&channel_owned, &target_owned, busy);
            Ok(())
        })
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(
            "{LOG_PREFIX} failed to persist busy={busy} channel={channel} reply_target={reply_target}: {error}"
        ),
        Err(error) => tracing::warn!(
            "{LOG_PREFIX} join error persisting busy={busy} channel={channel} reply_target={reply_target}: {error}"
        ),
    }
}
