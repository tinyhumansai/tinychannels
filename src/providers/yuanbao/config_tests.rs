use super::*;
use tinychannels_runtime::config::YuanbaoConfigExt as _;

#[test]
fn default_config_is_invalid() {
    let cfg = YuanbaoConfig::default();
    assert!(cfg.validate().is_err());
}

#[test]
fn env_defaults_fill_domains() {
    let mut cfg = YuanbaoConfig::default();
    cfg.apply_env_defaults();
    assert!(cfg.api_domain.contains("yuanbao.tencent.com"));
    assert!(cfg.ws_domain.contains("bot-wss.yuanbao.tencent.com"));
    assert!(cfg.validate().is_err());
}

#[test]
fn token_only_config_can_skip_api_domain() {
    let mut cfg = YuanbaoConfig {
        app_key: "key".into(),
        token: "token".into(),
        ..YuanbaoConfig::default()
    };
    cfg.apply_env_defaults();
    cfg.api_domain.clear();
    assert!(cfg.validate().is_ok());
}

#[test]
fn pre_env_uses_pre_domains() {
    let mut cfg = YuanbaoConfig {
        env: "pre".into(),
        ..YuanbaoConfig::default()
    };
    cfg.apply_env_defaults();
    assert!(cfg.api_domain.contains("bot-pre"));
    assert!(cfg.ws_domain.contains("bot-wss-pre"));
}

#[test]
fn explicit_domains_are_preserved() {
    let mut cfg = YuanbaoConfig {
        api_domain: "https://api.example.test".into(),
        ws_domain: "wss://ws.example.test".into(),
        ..YuanbaoConfig::default()
    };
    cfg.apply_env_defaults();
    assert_eq!(cfg.api_domain, "https://api.example.test");
    assert_eq!(cfg.ws_domain, "wss://ws.example.test");
}

#[test]
fn plugin_version_alias_deserializes() {
    let cfg: YuanbaoConfig = toml::from_str(
        r#"
            app_key = "key"
            app_secret = "secret"
            plugin_version = "openhuman/9.9.9"
            "#,
    )
    .unwrap();
    assert_eq!(strip_version_prefix(&cfg.bot_version), "9.9.9");
}
