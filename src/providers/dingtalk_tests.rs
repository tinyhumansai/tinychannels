use super::*;

#[test]
fn test_name() {
    let ch = DingTalkChannel::new("id".into(), "secret".into(), vec![]);
    assert_eq!(ch.name(), "dingtalk");
}

#[test]
fn test_user_allowed_wildcard() {
    let ch = DingTalkChannel::new("id".into(), "secret".into(), vec!["*".into()]);
    assert!(ch.is_user_allowed("anyone"));
}

#[test]
fn test_user_allowed_specific() {
    let ch = DingTalkChannel::new("id".into(), "secret".into(), vec!["user123".into()]);
    assert!(ch.is_user_allowed("user123"));
    assert!(!ch.is_user_allowed("other"));
}

#[test]
fn test_user_denied_empty() {
    let ch = DingTalkChannel::new("id".into(), "secret".into(), vec![]);
    assert!(!ch.is_user_allowed("anyone"));
}

#[test]
fn test_config_serde() {
    let toml_str = r#"
client_id = "app_id_123"
client_secret = "secret_456"
allowed_users = ["user1", "*"]
"#;
    let config: crate::config::DingTalkConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.client_id, "app_id_123");
    assert_eq!(config.client_secret, "secret_456");
    assert_eq!(config.allowed_users, vec!["user1", "*"]);
}

#[test]
fn test_config_serde_defaults() {
    let toml_str = r#"
client_id = "id"
client_secret = "secret"
"#;
    let config: crate::config::DingTalkConfig = toml::from_str(toml_str).unwrap();
    assert!(config.allowed_users.is_empty());
}

#[test]
fn parse_stream_data_supports_string_payload() {
    let frame = serde_json::json!({
        "data": "{\"text\":{\"content\":\"hello\"}}"
    });
    let parsed = DingTalkChannel::parse_stream_data(&frame).unwrap();
    assert_eq!(
        parsed.get("text").and_then(|v| v.get("content")),
        Some(&serde_json::json!("hello"))
    );
}

#[test]
fn parse_stream_data_supports_object_payload() {
    let frame = serde_json::json!({
        "data": {"text": {"content": "hello"}}
    });
    let parsed = DingTalkChannel::parse_stream_data(&frame).unwrap();
    assert_eq!(
        parsed.get("text").and_then(|v| v.get("content")),
        Some(&serde_json::json!("hello"))
    );
}

#[test]
fn resolve_chat_id_handles_numeric_group_conversation_type() {
    let data = serde_json::json!({
        "conversationType": 2,
        "conversationId": "cid-group",
    });
    let chat_id = DingTalkChannel::resolve_chat_id(&data, "staff-1");
    assert_eq!(chat_id, "cid-group");
}
