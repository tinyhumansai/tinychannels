use super::*;

#[test]
fn telegram_and_its_alias_share_capabilities() {
    let tg = capabilities_for("telegram");
    assert_eq!(capabilities_for("tg"), tg);
    assert_eq!(capabilities_for("tg:12345"), tg);
    assert_eq!(capabilities_for("telegram:-100"), tg);
    assert!(tg.remote_control && tg.chat_approvals && tg.progressive_edits);
    assert!(tg.history_key_ignores_thread);
}

#[test]
fn chat_providers_get_remote_control_and_approvals_but_not_edits() {
    for provider in [
        "discord",
        "slack",
        "mattermost",
        "imessage",
        "signal",
        "whatsapp",
        "irc",
    ] {
        let caps = capabilities_for(provider);
        assert!(caps.remote_control, "{provider} remote_control");
        assert!(caps.chat_approvals, "{provider} chat_approvals");
        assert!(!caps.progressive_edits, "{provider} progressive_edits");
        assert!(!caps.history_key_ignores_thread, "{provider} thread key");
    }
    assert_eq!(
        capabilities_for("discord:guild/chan"),
        ChannelCapabilities::CHAT
    );
}

#[test]
fn unknown_and_non_conversational_providers_fail_safe() {
    for provider in ["email", "cli", "webhook", "brand-new", ""] {
        assert_eq!(
            capabilities_for(provider),
            ChannelCapabilities::NONE,
            "{provider}"
        );
    }
}

#[test]
fn provider_id_strips_prefix_and_resolves_alias() {
    assert_eq!(provider_id("tg:1"), "telegram");
    assert_eq!(provider_id("slack:C1"), "slack");
    assert_eq!(provider_id("email"), "email");
}
