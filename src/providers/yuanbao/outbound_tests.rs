use super::*;

#[test]
fn target_parse_dm() {
    match Target::parse("user_42") {
        Target::Dm(uid) => assert_eq!(uid, "user_42"),
        _ => panic!("should be DM"),
    }
}

#[test]
fn target_parse_group() {
    match Target::parse("g:room_99") {
        Target::Group(c) => assert_eq!(c, "room_99"),
        _ => panic!("should be group"),
    }
}

#[test]
fn target_parse_empty_dm() {
    assert!(matches!(Target::parse(""), Target::Dm("")));
}

#[test]
fn extract_filename_strips_query() {
    assert_eq!(extract_filename("https://x.com/a/b/cat.png"), "cat.png");
    assert_eq!(
        extract_filename("https://x.com/a/b/cat.png?sig=abc"),
        "cat.png"
    );
    assert_eq!(extract_filename("https://x.com/"), "file");
    assert_eq!(extract_filename(""), "file");
}

#[test]
fn extract_filename_from_bare_path() {
    // Not a valid URL → fall back to last non-empty `/`-segment.
    assert_eq!(extract_filename("/var/log/foo.bin"), "foo.bin");
    // Trailing slash gets skipped; last non-empty segment wins.
    assert_eq!(extract_filename("/var/log/"), "log");
    // Plain filename with no slashes.
    assert_eq!(extract_filename("plain.txt"), "plain.txt");
}

fn make_conn(cfg: super::super::config::YuanbaoConfig) -> Arc<YuanbaoConnection> {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    YuanbaoConnection::new(cfg, tx, None)
}

fn base_cfg() -> super::super::config::YuanbaoConfig {
    let mut c = super::super::config::YuanbaoConfig::default();
    c.app_key = "ak".into();
    c.ws_domain = "wss://x".into();
    c.token = "tok".into();
    c.bot_id = "cfg-bot".into();
    c
}

#[tokio::test]
async fn resolve_from_account_uses_config_bot_id_when_no_sign_manager() {
    let conn = make_conn(base_cfg());
    let sender = OutboundSender::new(conn, None, "ak".into(), "cfg-bot".into());
    assert_eq!(sender.resolve_from_account().await, "cfg-bot");
}

#[tokio::test]
async fn resolve_from_account_uses_sign_cache_when_bot_id_present() {
    let conn = make_conn(base_cfg());
    let mgr = super::super::sign::SignManager::new(reqwest::Client::new());
    // Seed the cache with a bot_id keyed on the same app_key.
    mgr.set_cached_for_test(
        "ak",
        super::super::sign::TokenEntry {
            token: "tok".into(),
            bot_id: "server-bot".into(),
            product: String::new(),
            source: "bot".into(),
            expire_ts: u64::MAX / 2,
        },
    )
    .await;
    let sender = OutboundSender::new(conn, Some(mgr), "ak".into(), "fallback-bot".into());
    // Sign cache hit → use server bot_id, not the fallback.
    assert_eq!(sender.resolve_from_account().await, "server-bot");
}

#[tokio::test]
async fn resolve_from_account_falls_back_when_sign_cache_bot_id_empty() {
    let conn = make_conn(base_cfg());
    let mgr = super::super::sign::SignManager::new(reqwest::Client::new());
    mgr.set_cached_for_test(
        "ak",
        super::super::sign::TokenEntry {
            token: "tok".into(),
            bot_id: String::new(),
            product: String::new(),
            source: String::new(),
            expire_ts: u64::MAX / 2,
        },
    )
    .await;
    let sender = OutboundSender::new(conn, Some(mgr), "ak".into(), "fallback-bot".into());
    assert_eq!(sender.resolve_from_account().await, "fallback-bot");
}
