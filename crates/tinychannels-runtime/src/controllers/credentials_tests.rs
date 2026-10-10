use super::*;

#[test]
fn parse_allowed_users_handles_string_csv() {
    let value = serde_json::json!("alice,bob,@carol");
    assert_eq!(
        parse_allowed_users(Some(&value)),
        vec!["alice", "bob", "carol"]
    );
}

#[test]
fn parse_allowed_users_handles_newline_separated_string() {
    let value = serde_json::json!("alice\nbob\r\ncarol");
    assert_eq!(
        parse_allowed_users(Some(&value)),
        vec!["alice", "bob", "carol"]
    );
}

#[test]
fn parse_allowed_users_dedups_case_insensitively() {
    let value = serde_json::json!("Alice,ALICE,alice,@Alice");
    assert_eq!(parse_allowed_users(Some(&value)), vec!["alice"]);
}

#[test]
fn parse_allowed_users_normalizes_at_prefix_and_whitespace() {
    let value = serde_json::json!("  @Alice  ");
    assert_eq!(parse_allowed_users(Some(&value)), vec!["alice"]);
}

#[test]
fn parse_allowed_users_rejects_empty_and_at_only() {
    let value = serde_json::json!(",  ,@,@ ,@@@, ,");
    let expected: Vec<String> = Vec::new();
    assert_eq!(parse_allowed_users(Some(&value)), expected);
}

#[test]
fn parse_allowed_users_accepts_array_of_strings() {
    let value = serde_json::json!(["a", "b,c", "@d\ne"]);
    let out = parse_allowed_users(Some(&value));
    for expected in ["a", "b", "c", "d", "e"] {
        assert!(out.contains(&expected.to_string()));
    }
}

#[test]
fn parse_allowed_users_returns_empty_for_none_or_non_string_value() {
    assert!(parse_allowed_users(None).is_empty());
    assert!(parse_allowed_users(Some(&serde_json::json!(42))).is_empty());
    assert!(parse_allowed_users(Some(&serde_json::json!({}))).is_empty());
    assert!(parse_allowed_users(Some(&serde_json::Value::Null)).is_empty());
}

#[test]
fn channel_credential_provider_combines_channel_id_and_mode() {
    assert_eq!(
        channel_credential_provider("telegram", ChannelAuthMode::BotToken),
        "channel:telegram:bot_token"
    );
    assert_eq!(
        channel_credential_provider("discord", ChannelAuthMode::OAuth),
        "channel:discord:oauth"
    );
}
