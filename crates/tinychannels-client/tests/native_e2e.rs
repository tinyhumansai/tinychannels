//! Public client routes over a strictly admitted cached native fixture.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod support;

use tinybus::module::{ModuleCallError, ModuleCallFailure};
use tinychannels_bus::{
    ChannelHealthState, ChannelsConfig, HttpProxyPolicy, config::DiscordConfig,
};

const TOKEN: &str = "fixture-call-secret";

fn config() -> ChannelsConfig {
    ChannelsConfig {
        cli: false,
        message_timeout_secs: 47,
        active_channel: Some("discord".into()),
        discord: Some(DiscordConfig {
            bot_token: "fixture-config-secret".into(),
            guild_id: Some("config-guild".into()),
            channel_id: Some("config-channel".into()),
            allowed_users: vec!["member-1".into()],
            listen_to_bots: true,
            mention_only: true,
        }),
        ..Default::default()
    }
}

fn proxy(selector: &str) -> HttpProxyPolicy {
    HttpProxyPolicy {
        http_proxy: Some("http://proxy.example.test:8080".into()),
        https_proxy: Some("https://proxy.example.test:8443".into()),
        all_proxy: Some("socks5://proxy.example.test:1080".into()),
        no_proxy: vec!["localhost".into(), ".internal".into()],
        service_selectors: vec![selector.into()],
    }
}

fn assert_one_module_fault(reports: &support::Reports) {
    assert_eq!(
        reports.entries(),
        [("tinychannels".into(), "0.2.0".into(), "module_fault".into())]
    );
}

fn assert_sanitized_failure(failure: &ModuleCallFailure) {
    assert_eq!(failure.kind, ModuleCallError::ModuleFault);
    assert!(!format!("{failure:?}").contains("fixture-private-token"));
    assert!(!failure.to_string().contains("fixture-private-token"));
}

#[tokio::test]
async fn all_public_wrappers_route_to_the_pinned_peer_and_decode_typed_results() {
    let fixture = support::fixture().await;
    let record = support::record(support::Pin::Correct);
    let before = support::calls(&fixture.probe).await;
    // Observe the admitted peer's public proof metadata; each actual operation
    // below still traverses TinyChannelsClient's strict ModuleClient call.
    let peer = fixture
        .probe
        .name_owner(record.bus_name)
        .await
        .expect("module owner lookup")
        .expect("fixture is serving");
    assert!(peer.is_unique());
    let proof = fixture
        .probe
        .attestation(peer)
        .await
        .expect("public peer proof lookup")
        .expect("cached release is attested");
    assert_eq!(proof.name.as_str(), record.bus_name);
    assert_eq!(proof.sha256, record.assets[0].sha256);
    assert_eq!(
        proof.library_sha256.as_deref(),
        record.assets[0].library_sha256
    );

    let guilds = fixture
        .client
        .discord_list_guilds(config(), TOKEN.into(), proxy("channel.discord"))
        .await
        .expect("guild list from the pinned native fixture");
    assert_eq!(guilds.len(), 1);
    assert_eq!(guilds[0].id, "guild-1");
    assert_eq!(guilds[0].name, "Fixture guild");
    assert_eq!(guilds[0].icon, None);

    let channels = fixture
        .client
        .discord_list_channels(
            config(),
            "guild-1".into(),
            TOKEN.into(),
            proxy("channel.discord"),
        )
        .await
        .expect("channel list from the pinned native fixture");
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].id, "channel-1");
    assert_eq!(channels[0].name, "general");
    assert_eq!(channels[0].channel_type, 0);
    assert_eq!(channels[0].position, 11);
    assert_eq!(channels[0].parent_id.as_deref(), Some("category-1"));

    let permissions = fixture
        .client
        .discord_check_permissions(
            config(),
            "guild-1".into(),
            "channel-1".into(),
            TOKEN.into(),
            proxy("channel.discord"),
        )
        .await
        .expect("permission check from the pinned native fixture");
    assert!(permissions.can_view_channel);
    assert!(!permissions.can_send_messages);
    assert!(permissions.can_read_message_history);
    assert_eq!(permissions.missing_permissions, ["SEND_MESSAGES"]);

    let health = fixture
        .client
        .check_health(config(), proxy("channel.discord"), 12_345)
        .await
        .expect("health response from the pinned native fixture");
    assert_eq!(health[0].channel, "discord");
    assert_eq!(health[0].state, ChannelHealthState::Healthy);
    assert_eq!(health[1].channel, "telegram");
    assert_eq!(health[1].state, ChannelHealthState::Timeout);
    assert_eq!(support::calls(&fixture.probe).await, before + 4);

    // The fixture validates full config, bot token, IDs, every proxy field,
    // and the non-default timeout before returning these typed results.
    assert_eq!(record.version, "0.2.0");
}

#[tokio::test]
async fn malformed_public_responses_report_one_sanitized_module_fault() {
    let fixture = support::fixture().await;

    fixture.reports.clear();
    let guilds = fixture
        .client
        .discord_list_guilds(config(), TOKEN.into(), proxy("test:wrong-variant"))
        .await
        .expect_err("wrong result variant is malformed");
    assert_sanitized_failure(&guilds);
    assert_one_module_fault(&fixture.reports);

    fixture.reports.clear();
    let channels = fixture
        .client
        .discord_list_channels(
            config(),
            "guild-1".into(),
            TOKEN.into(),
            proxy("test:missing-raw"),
        )
        .await
        .expect_err("missing raw projection is malformed");
    assert_sanitized_failure(&channels);
    assert_one_module_fault(&fixture.reports);

    fixture.reports.clear();
    let permissions = fixture
        .client
        .discord_check_permissions(
            config(),
            "guild-1".into(),
            "channel-1".into(),
            TOKEN.into(),
            proxy("test:malformed-payload"),
        )
        .await
        .expect_err("wrong permission payload shape is malformed");
    assert_sanitized_failure(&permissions);
    assert_one_module_fault(&fixture.reports);

    fixture.reports.clear();
    let health = fixture
        .client
        .check_health(config(), proxy("test:malformed-payload"), 12_345)
        .await
        .expect_err("wrong health payload shape is malformed");
    assert_eq!(health.kind, ModuleCallError::IncompatibleContract);
    assert!(!format!("{health:?}").contains("fixture-private-token"));
    assert!(!health.to_string().contains("fixture-private-token"));
    assert_eq!(
        fixture.reports.entries(),
        [(
            "tinychannels".into(),
            "0.2.0".into(),
            "incompatible_contract".into()
        )]
    );
}

#[tokio::test]
async fn detailed_invalid_input_keeps_its_wire_classification_and_redacts_rendering() {
    let fixture = support::fixture().await;
    fixture.reports.clear();
    let failure = fixture
        .client
        .discord_list_guilds(config(), TOKEN.into(), proxy("test:invalid-input"))
        .await
        .expect_err("fixture emits an InvalidInput remote failure");

    assert_eq!(failure.kind, ModuleCallError::ModuleFault);
    assert_eq!(
        failure.wire_name,
        "ai.tinyhumans.tinychannels.Error.InvalidInput"
    );
    assert!(failure.message.contains("fixture-private-token"));
    assert!(!format!("{failure:?}").contains("fixture-private-token"));
    assert!(!failure.to_string().contains("fixture-private-token"));
    assert_one_module_fault(&fixture.reports);
}

#[tokio::test]
async fn wrong_release_pins_and_unattested_peers_are_refused_before_business_calls() {
    let fixture = support::fixture().await;
    let before = support::calls(&fixture.probe).await;

    for client in [&fixture.wrong_archive_client, &fixture.wrong_library_client] {
        let error = client
            .discord_list_guilds(config(), TOKEN.into(), proxy("channel.discord"))
            .await
            .expect_err("a mismatched half of the paired pin must be refused");
        support::assert_unavailable(error);
        assert_eq!(support::calls(&fixture.probe).await, before);

        let error = client
            .discord_list_channels(
                config(),
                "guild-1".into(),
                TOKEN.into(),
                proxy("channel.discord"),
            )
            .await
            .expect_err("a mismatched pin must refuse channel listing");
        support::assert_unavailable(error);
        assert_eq!(support::calls(&fixture.probe).await, before);

        let error = client
            .discord_check_permissions(
                config(),
                "guild-1".into(),
                "channel-1".into(),
                TOKEN.into(),
                proxy("channel.discord"),
            )
            .await
            .expect_err("a mismatched pin must refuse permission checks");
        support::assert_unavailable(error);
        assert_eq!(support::calls(&fixture.probe).await, before);

        let error = client
            .check_health(config(), proxy("channel.discord"), 12_345)
            .await
            .expect_err("a mismatched pin must refuse health checks");
        support::assert_unavailable(error);
        assert_eq!(support::calls(&fixture.probe).await, before);
    }

    let unattested = support::unattested_fixture().await;
    let owner = support::assert_unattested_peer_live(&unattested.probe).await;
    let error = unattested
        .client
        .discord_list_guilds(config(), TOKEN.into(), proxy("channel.discord"))
        .await
        .expect_err("unique but unattested peer must be refused");
    assert_eq!(error.kind, ModuleCallError::Unavailable);
    assert_eq!(
        unattested.calls.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        support::assert_unattested_peer_live(&unattested.probe).await,
        owner
    );

    let error = unattested
        .client
        .discord_list_channels(
            config(),
            "guild-1".into(),
            TOKEN.into(),
            proxy("channel.discord"),
        )
        .await
        .expect_err("unique but unattested peer must refuse channel listing");
    assert_eq!(error.kind, ModuleCallError::Unavailable);
    assert_eq!(
        unattested.calls.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        support::assert_unattested_peer_live(&unattested.probe).await,
        owner
    );

    let error = unattested
        .client
        .discord_check_permissions(
            config(),
            "guild-1".into(),
            "channel-1".into(),
            TOKEN.into(),
            proxy("channel.discord"),
        )
        .await
        .expect_err("unique but unattested peer must refuse permission checks");
    assert_eq!(error.kind, ModuleCallError::Unavailable);
    assert_eq!(
        unattested.calls.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        support::assert_unattested_peer_live(&unattested.probe).await,
        owner
    );

    let error = unattested
        .client
        .check_health(config(), proxy("channel.discord"), 12_345)
        .await
        .expect_err("unique but unattested peer must refuse health checks");
    assert_eq!(error.kind, ModuleCallError::Unavailable);
    assert_eq!(
        unattested.calls.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        support::assert_unattested_peer_live(&unattested.probe).await,
        owner
    );
}
