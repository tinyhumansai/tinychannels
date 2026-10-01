use tinychannels_bus::ChannelsConfig;
use tinychannels_bus::config::WhatsAppConfig;

/// Mirrors the predicate in `start_channel`.
///
/// Extracted rather than duplicated in the test so the two cannot disagree:
/// the point of these cases is the *classification*, and a copy of the rule
/// would keep passing after the real one changed.
use super::is_webhook_backed as webhook_backed;

fn whatsapp(cloud: bool) -> ChannelsConfig {
    ChannelsConfig {
        whatsapp: Some(WhatsAppConfig {
            access_token: None,
            phone_number_id: cloud.then(|| "1".to_owned()),
            verify_token: None,
            app_secret: None,
            session_path: (!cloud).then(|| "/tmp/session".to_owned()),
            pair_phone: None,
            pair_code: None,
            allowed_numbers: Vec::new(),
        }),
        ..ChannelsConfig::default()
    }
}

#[test]
fn linq_is_refused_because_its_inbound_path_is_a_webhook() {
    assert!(webhook_backed("linq", &ChannelsConfig::default()));
}

#[test]
fn whatsapp_cloud_is_refused_but_whatsapp_web_is_not() {
    // Both report `name() == "whatsapp"`, so only the config shape separates
    // them. Getting this backwards would either refuse a working provider or
    // silently accept a dead one.
    assert!(webhook_backed("whatsapp", &whatsapp(true)));
    assert!(!webhook_backed("whatsapp", &whatsapp(false)));
}

#[test]
fn providers_with_a_real_listen_loop_are_unaffected() {
    for name in ["telegram", "discord", "slack", "irc", "signal"] {
        assert!(
            !webhook_backed(name, &ChannelsConfig::default()),
            "{name} should not be refused"
        );
    }
}
