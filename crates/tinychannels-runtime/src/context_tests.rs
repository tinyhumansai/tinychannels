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
        sender_name: None,
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

    let mut telegram_topic = message("telegram", "alice", "-100123");
    telegram_topic.thread_ts = Some("topic-99".into());
    assert_eq!(
        conversation_history_key(&telegram_topic),
        "telegram_alice_-100123"
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

#[test]
fn context_overflow_matches_established_phrasings() {
    for body in [
        "This model's maximum context length is 8192 tokens",
        "request exceeds the context window of this model",
        "context length exceeded",
        "{\"error\":{\"code\":500,\"message\":\"Context size has been exceeded.\"}}",
        "too many tokens in the prompt",
        "token limit exceeded",
        "prompt is too long for the selected model",
        "input is too long",
    ] {
        assert!(is_context_window_overflow_message(body), "{body}");
    }
}

#[test]
fn context_overflow_matches_lmstudio_n_keep_body() {
    assert!(is_context_window_overflow_message(
        "lmstudio API error (400): The number of tokens to keep from the initial prompt is greater than the context length (n_keep: 10978 >= n_ctx: 8192)."
    ));
    assert!(is_context_window_overflow_message(
        "prompt is greater than the context length of the loaded model"
    ));
    assert!(is_context_window_overflow_message(
        "n_keep: 9000 >= n_ctx: 4096"
    ));
}

#[test]
fn context_overflow_matches_dashscope_input_length_range() {
    let body = "Provider returned error: {\"error\":{\"code\":\"invalid_parameter_error\",\
                \"message\":\"Range of input length should be [1, 98304]\"}}";
    assert!(is_context_window_overflow_message(body));
}

#[test]
fn context_overflow_ignores_other_dashscope_parameter_errors() {
    // Same DashScope error envelope, but not the input-length range.
    let body = "Provider returned error: {\"error\":{\"code\":\"invalid_parameter_error\",\
                \"message\":\"Range of temperature should be [0, 2)\"}}";
    assert!(!is_context_window_overflow_message(body));
}

#[test]
fn context_overflow_ignores_unrelated_and_rate_limit_bodies() {
    for body in [
        "network unavailable",
        "rate limit exceeded, retry after 30s",
        "tool call exceeded the allowed budget",
        // Only one of the paired n_keep/n_ctx tokens.
        "loaded model with n_ctx: 8192 and 32 layers",
        "Rate limit reached: too many tokens per minute (TPM) for this org",
        "rate_limit_exceeded: token limit exceeded, retry after 12s",
        "You have hit too many tokens per min; try again in 30s",
    ] {
        assert!(!is_context_window_overflow_message(body), "{body}");
    }
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

#[test]
fn sender_history_cleanup_and_empty_compaction_preserve_other_senders() {
    let histories: ConversationHistoryMap = Arc::new(Mutex::new(HashMap::from([
        ("alice".into(), vec![ChatMessage::user("hi")]),
        ("bob".into(), vec![ChatMessage::assistant("hello")]),
    ])));
    assert!(!compact_sender_history(&histories, "missing"));
    assert!(compact_sender_history(&histories, "alice"));
    clear_sender_history(&histories, "alice");
    assert!(histories.lock().unwrap().contains_key("bob"));
    assert!(!histories.lock().unwrap().contains_key("alice"));
    assert!(!compact_history(&mut Vec::new()));
    assert_eq!(effective_channel_message_timeout_secs(1), 30);
    assert_eq!(effective_channel_message_timeout_secs(300), 300);
}

#[tokio::test]
async fn memory_context_bounds_entries_and_total_text_without_empty_headers() {
    let entries = (0..6)
        .map(|i| MemoryEntry {
            key: format!("fixture-{i}"),
            content: "value".into(),
            score: None,
        })
        .collect();
    let rendered = build_memory_context(&MockMemory { entries }, "query", 0.0).await;
    assert!(rendered.contains("fixture-3"));
    assert!(!rendered.contains("fixture-4"));
    let entries = vec![MemoryEntry {
        key: "k".repeat(MEMORY_CONTEXT_MAX_CHARS),
        content: "value".into(),
        score: None,
    }];
    assert!(
        build_memory_context(&MockMemory { entries }, "query", 0.0)
            .await
            .is_empty()
    );
}
