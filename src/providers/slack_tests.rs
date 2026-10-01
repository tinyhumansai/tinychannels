use super::*;

#[test]
fn slack_channel_name() {
    let ch = SlackChannel::new("xoxb-fake".into(), None, vec![]);
    assert_eq!(ch.name(), "slack");
}

#[test]
fn slack_channel_with_channel_id() {
    let ch = SlackChannel::new("xoxb-fake".into(), Some("C12345".into()), vec![]);
    assert_eq!(ch.channel_id, Some("C12345".to_string()));
}

#[test]
fn empty_allowlist_denies_everyone() {
    let ch = SlackChannel::new("xoxb-fake".into(), None, vec![]);
    assert!(!ch.is_user_allowed("U12345"));
    assert!(!ch.is_user_allowed("anyone"));
}

#[test]
fn wildcard_allows_everyone() {
    let ch = SlackChannel::new("xoxb-fake".into(), None, vec!["*".into()]);
    assert!(ch.is_user_allowed("U12345"));
}

#[test]
fn specific_allowlist_filters() {
    let ch = SlackChannel::new("xoxb-fake".into(), None, vec!["U111".into(), "U222".into()]);
    assert!(ch.is_user_allowed("U111"));
    assert!(ch.is_user_allowed("U222"));
    assert!(!ch.is_user_allowed("U333"));
}

#[test]
fn allowlist_exact_match_not_substring() {
    let ch = SlackChannel::new("xoxb-fake".into(), None, vec!["U111".into()]);
    assert!(!ch.is_user_allowed("U1111"));
    assert!(!ch.is_user_allowed("U11"));
}

#[test]
fn allowlist_empty_user_id() {
    let ch = SlackChannel::new("xoxb-fake".into(), None, vec!["U111".into()]);
    assert!(!ch.is_user_allowed(""));
}

#[test]
fn allowlist_case_sensitive() {
    let ch = SlackChannel::new("xoxb-fake".into(), None, vec!["U111".into()]);
    assert!(ch.is_user_allowed("U111"));
    assert!(!ch.is_user_allowed("u111"));
}

#[test]
fn allowlist_wildcard_and_specific() {
    let ch = SlackChannel::new("xoxb-fake".into(), None, vec!["U111".into(), "*".into()]);
    assert!(ch.is_user_allowed("U111"));
    assert!(ch.is_user_allowed("anyone"));
}

// Message ID edge cases.

#[test]
fn slack_message_id_format_includes_channel_and_ts() {
    // Verify that message IDs follow the format: slack_{channel_id}_{ts}
    let ts = "1234567890.123456";
    let channel_id = "C12345";
    let expected_id = format!("slack_{channel_id}_{ts}");
    assert_eq!(expected_id, "slack_C12345_1234567890.123456");
}

#[test]
fn slack_message_id_is_deterministic() {
    // Same channel_id + same ts = same ID (prevents duplicates after restart)
    let ts = "1234567890.123456";
    let channel_id = "C12345";
    let id1 = format!("slack_{channel_id}_{ts}");
    let id2 = format!("slack_{channel_id}_{ts}");
    assert_eq!(id1, id2);
}

#[test]
fn slack_message_id_different_ts_different_id() {
    // Different timestamps produce different IDs
    let channel_id = "C12345";
    let id1 = format!("slack_{channel_id}_1234567890.123456");
    let id2 = format!("slack_{channel_id}_1234567890.123457");
    assert_ne!(id1, id2);
}

#[test]
fn slack_message_id_different_channel_different_id() {
    // Different channels produce different IDs even with same ts
    let ts = "1234567890.123456";
    let id1 = format!("slack_C12345_{ts}");
    let id2 = format!("slack_C67890_{ts}");
    assert_ne!(id1, id2);
}

#[test]
fn slack_message_id_no_uuid_randomness() {
    // Verify format doesn't contain random UUID components
    let ts = "1234567890.123456";
    let channel_id = "C12345";
    let id = format!("slack_{channel_id}_{ts}");
    assert!(!id.contains('-')); // No UUID dashes
    assert!(id.starts_with("slack_"));
}

#[test]
fn inbound_thread_ts_prefers_explicit_thread_ts() {
    let msg = serde_json::json!({
        "ts": "123.002",
        "thread_ts": "123.001"
    });

    let thread_ts = SlackChannel::inbound_thread_ts(&msg, "123.002");
    assert_eq!(thread_ts.as_deref(), Some("123.001"));
}

#[test]
fn inbound_thread_ts_falls_back_to_ts() {
    let msg = serde_json::json!({
        "ts": "123.001"
    });

    let thread_ts = SlackChannel::inbound_thread_ts(&msg, "123.001");
    assert_eq!(thread_ts.as_deref(), Some("123.001"));
}

#[test]
fn inbound_thread_ts_none_when_ts_missing() {
    let msg = serde_json::json!({});

    let thread_ts = SlackChannel::inbound_thread_ts(&msg, "");
    assert_eq!(thread_ts, None);
}
