//! Shared channel runtime helpers.

use crate::traits::ChannelMessage;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Per-sender conversation history for channel messages.
pub type ConversationHistoryMap = Arc<Mutex<HashMap<String, Vec<ChatMessage>>>>;
/// Maximum history messages to keep per sender.
pub const MAX_CHANNEL_HISTORY: usize = 50;

pub const DEFAULT_CHANNEL_INITIAL_BACKOFF_SECS: u64 = 2;
pub const DEFAULT_CHANNEL_MAX_BACKOFF_SECS: u64 = 60;
pub const MIN_CHANNEL_MESSAGE_TIMEOUT_SECS: u64 = 30;
pub const CHANNEL_MESSAGE_TIMEOUT_SECS: u64 = 300;
pub const CHANNEL_PARALLELISM_PER_CHANNEL: usize = 4;
pub const CHANNEL_MIN_IN_FLIGHT_MESSAGES: usize = 8;
pub const CHANNEL_MAX_IN_FLIGHT_MESSAGES: usize = 64;
pub const CHANNEL_TYPING_REFRESH_INTERVAL_SECS: u64 = 4;
pub const MEMORY_CONTEXT_MAX_ENTRIES: usize = 4;
pub const MEMORY_CONTEXT_ENTRY_MAX_CHARS: usize = 800;
pub const MEMORY_CONTEXT_MAX_CHARS: usize = 4_000;
pub const CHANNEL_HISTORY_COMPACT_KEEP_MESSAGES: usize = 12;
pub const CHANNEL_HISTORY_COMPACT_CONTENT_CHARS: usize = 600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRouteSelection {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryEntry {
    pub key: String,
    pub content: String,
    pub score: Option<f64>,
}

#[async_trait::async_trait]
pub trait Memory: Send + Sync {
    async fn recall(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>>;
}

pub fn effective_channel_message_timeout_secs(configured: u64) -> u64 {
    configured.max(MIN_CHANNEL_MESSAGE_TIMEOUT_SECS)
}

pub fn conversation_memory_key(msg: &ChannelMessage) -> String {
    format!("{}_{}_{}", msg.channel, msg.sender, msg.id)
}

pub fn conversation_history_key(msg: &ChannelMessage) -> String {
    let base_key = format!("{}_{}_{}", msg.channel, msg.sender, msg.reply_target);
    if msg.channel == "telegram" {
        return base_key;
    }
    if let Some(thread_ts) = msg.thread_ts.as_deref() {
        let thread_ts = thread_ts.trim();
        if !thread_ts.is_empty() {
            return format!("{base_key}_thread:{thread_ts}");
        }
    }
    base_key
}

pub fn compact_history(turns: &mut Vec<ChatMessage>) -> bool {
    if turns.is_empty() {
        return false;
    }

    let keep_from = turns
        .len()
        .saturating_sub(CHANNEL_HISTORY_COMPACT_KEEP_MESSAGES);
    let mut compacted = turns[keep_from..].to_vec();

    for turn in &mut compacted {
        if turn.content.chars().count() > CHANNEL_HISTORY_COMPACT_CONTENT_CHARS {
            turn.content =
                truncate_with_ellipsis(&turn.content, CHANNEL_HISTORY_COMPACT_CONTENT_CHARS);
        }
    }

    *turns = compacted;
    true
}

pub fn clear_sender_history(histories: &ConversationHistoryMap, sender_key: &str) {
    histories
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(sender_key);
}

pub fn compact_sender_history(histories: &ConversationHistoryMap, sender_key: &str) -> bool {
    let mut histories = histories.lock().unwrap_or_else(|e| e.into_inner());
    let Some(turns) = histories.get_mut(sender_key) else {
        return false;
    };
    compact_history(turns)
}

pub fn should_skip_memory_context_entry(key: &str, content: &str) -> bool {
    if key.trim().to_ascii_lowercase().ends_with("_history") {
        return true;
    }

    content.chars().count() > MEMORY_CONTEXT_MAX_CHARS
}

pub fn is_context_window_overflow_message(err: &str) -> bool {
    let lower = err.to_lowercase();
    [
        "exceeds the context window",
        "context window of this model",
        "maximum context length",
        "context length exceeded",
        "too many tokens",
        "token limit exceeded",
        "prompt is too long",
        "input is too long",
    ]
    .iter()
    .any(|hint| lower.contains(hint))
}

pub async fn build_memory_context(
    mem: &dyn Memory,
    user_msg: &str,
    min_relevance_score: f64,
) -> String {
    let mut context = String::new();

    if let Ok(entries) = mem.recall(user_msg, 5).await {
        let mut included = 0usize;
        let mut used_chars = 0usize;

        for entry in entries.iter().filter(|e| match e.score {
            Some(score) => score >= min_relevance_score,
            None => true,
        }) {
            if included >= MEMORY_CONTEXT_MAX_ENTRIES {
                break;
            }

            if should_skip_memory_context_entry(&entry.key, &entry.content) {
                continue;
            }

            let content = if entry.content.chars().count() > MEMORY_CONTEXT_ENTRY_MAX_CHARS {
                truncate_with_ellipsis(&entry.content, MEMORY_CONTEXT_ENTRY_MAX_CHARS)
            } else {
                entry.content.clone()
            };

            let line = format!("- {}: {}\n", entry.key, content);
            let line_chars = line.chars().count();
            if used_chars + line_chars > MEMORY_CONTEXT_MAX_CHARS {
                break;
            }

            if included == 0 {
                context.push_str("[Memory context]\n");
            }

            context.push_str(&line);
            used_chars += line_chars;
            included += 1;
        }

        if included > 0 {
            context.push('\n');
        }
    }

    context
}

fn truncate_with_ellipsis(input: &str, max_chars: usize) -> String {
    let mut iter = input.chars();
    let mut out = String::new();
    for _ in 0..max_chars {
        let Some(ch) = iter.next() else {
            return input.to_string();
        };
        out.push(ch);
    }
    if iter.next().is_some() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockMemory {
        entries: Vec<MemoryEntry>,
    }

    #[async_trait::async_trait]
    impl Memory for MockMemory {
        async fn recall(&self, _query: &str, _limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(self.entries.clone())
        }
    }

    fn message(channel: &str, sender: &str, reply_target: &str) -> ChannelMessage {
        ChannelMessage {
            id: "m1".into(),
            sender: sender.into(),
            reply_target: reply_target.into(),
            content: "hello".into(),
            channel: channel.into(),
            timestamp: 123,
            thread_ts: None,
        }
    }

    #[test]
    fn conversation_keys_match_openhuman_channel_behavior() {
        let telegram = message("telegram", "alice", "chat1");
        assert_eq!(conversation_memory_key(&telegram), "telegram_alice_m1");
        assert_eq!(conversation_history_key(&telegram), "telegram_alice_chat1");

        let mut discord = message("discord", "bob", "channel1");
        discord.thread_ts = Some("thread1".into());
        assert_eq!(
            conversation_history_key(&discord),
            "discord_bob_channel1_thread:thread1"
        );
    }

    #[test]
    fn compact_history_keeps_recent_turns_and_truncates_content() {
        let mut history = (0..20)
            .map(|idx| {
                if idx == 19 {
                    ChatMessage::assistant("x".repeat(700))
                } else {
                    ChatMessage::user(format!("turn {idx}"))
                }
            })
            .collect::<Vec<_>>();

        assert!(compact_history(&mut history));
        assert_eq!(history.len(), CHANNEL_HISTORY_COMPACT_KEEP_MESSAGES);
        assert!(history.last().unwrap().content.chars().count() <= 601);
    }

    #[test]
    fn memory_context_skip_and_overflow_detection_match_openhuman_hints() {
        assert!(should_skip_memory_context_entry("note_history", "short"));
        assert!(should_skip_memory_context_entry(
            "note",
            &"x".repeat(MEMORY_CONTEXT_MAX_CHARS + 1)
        ));
        assert!(!should_skip_memory_context_entry("note", "short"));

        assert!(is_context_window_overflow_message(
            "maximum context length exceeded"
        ));
        assert!(!is_context_window_overflow_message("network unavailable"));
    }

    #[tokio::test]
    async fn build_memory_context_filters_entries_and_truncates_content() {
        let memory = MockMemory {
            entries: vec![
                MemoryEntry {
                    key: "keep".into(),
                    content: "v".into(),
                    score: Some(0.9),
                },
                MemoryEntry {
                    key: "drop_history".into(),
                    content: "ignored".into(),
                    score: Some(0.9),
                },
                MemoryEntry {
                    key: "low".into(),
                    content: "too low".into(),
                    score: Some(0.1),
                },
                MemoryEntry {
                    key: "long".into(),
                    content: "x".repeat(MEMORY_CONTEXT_ENTRY_MAX_CHARS + 50),
                    score: None,
                },
            ],
        };

        let rendered = build_memory_context(&memory, "hello", 0.4).await;
        assert!(rendered.contains("[Memory context]"));
        assert!(rendered.contains("- keep: v"));
        assert!(!rendered.contains("drop_history"));
        assert!(!rendered.contains("too low"));
        assert!(rendered.contains("- long: "));
    }
}
