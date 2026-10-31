use super::*;
use crate::client::DiscordReply;

use std::sync::{Arc, Mutex};

use serde_json::json;
use tinybus::{
    Connection, InterfaceName, MemberName, ObjectPath, Proxy,
    broker::Broker,
    module::{
        AccessFuture, FailureReport, FailureReporter, FailureSink, ModuleAccess, ModuleCallError,
        ModuleCallFailure, ModuleClient, ProxyFailure, ReleaseAsset, ReleaseRecord,
    },
    service::Interface,
    transport::memory::MemoryBus,
};
use tinychannels_bus::{
    ChannelControllerRequest, ChannelControllerResult, ChannelsConfig, HttpProxyPolicy,
    controllers::DiscordGuild,
};

#[path = "lib_runtime_tests.rs"]
mod runtime_tests;

const MODULE_ID: &str = tinychannels_bus::MODULE_ID;
type Calls = Arc<Mutex<Vec<String>>>;

struct NoFailures;

impl FailureSink for NoFailures {
    fn capture(&self, _report: &FailureReport<'_>) {}
}

struct RecordingModule(Calls);

#[async_trait::async_trait]
impl Interface for RecordingModule {
    fn name(&self) -> InterfaceName {
        InterfaceName::new(tinychannels_bus::BUS_NAME).expect("valid bus interface")
    }

    fn members(&self) -> Vec<MemberName> {
        tinychannels_bus::METHODS
            .iter()
            .copied()
            .chain(["FixtureReady"])
            .map(|name| MemberName::new(name).expect("valid TinyChannels method"))
            .collect()
    }

    async fn call(
        &self,
        member: &MemberName,
        _args: serde_json::Value,
    ) -> tinybus::Result<serde_json::Value> {
        self.0
            .lock()
            .map_err(|_| tinybus::Error::failed("call log poisoned"))?
            .push(member.as_str().to_owned());
        if member.as_str() == "FixtureReady" {
            return Ok(json!(true));
        }
        Ok(json!({"result":"discord_guilds","value":{"raw":[]}}))
    }
}

struct TestAccess {
    record: &'static ReleaseRecord,
    _service: Connection,
    connection: Connection,
    reporter: FailureReporter,
}

impl ModuleAccess for TestAccess {
    fn find(&self, id: &str) -> Option<&'static ReleaseRecord> {
        (id == self.record.id).then_some(self.record)
    }

    fn enabled(&self) -> bool {
        true
    }

    fn reporter(&self) -> &FailureReporter {
        &self.reporter
    }

    fn ensure_loaded<'a>(&'a self, _id: &'a str) -> AccessFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_attested<'a>(&'a self, _id: &'a str) -> AccessFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn proxy<'a>(
        &'a self,
        bus_name: &'a str,
        object_path: &'a str,
    ) -> AccessFuture<'a, Result<tinybus::Proxy, ProxyFailure>> {
        Box::pin(async move {
            self.connection
                .proxy(bus_name, object_path, bus_name)
                .map_err(|_| ProxyFailure::Contract)
        })
    }
}

async fn unattested_client() -> (TinyChannelsClient, Calls, Proxy) {
    let host_key = Box::leak(
        tinybus::module::platform::host_candidates()
            .into_iter()
            .next()
            .expect("supported TinyBus host")
            .into_boxed_str(),
    );
    let archive = Box::leak("fixture.tar.gz".to_owned().into_boxed_str());
    let archive_sha = Box::leak("a".repeat(64).into_boxed_str());
    let library_sha = Box::leak("b".repeat(64).into_boxed_str());
    let assets = Box::leak(
        vec![ReleaseAsset {
            host_key,
            archive,
            sha256: archive_sha,
            library_sha256: Some(library_sha),
        }]
        .into_boxed_slice(),
    );
    let record = Box::leak(Box::new(ReleaseRecord {
        id: MODULE_ID,
        bus_name: tinychannels_bus::BUS_NAME,
        object_path: tinychannels_bus::OBJECT_PATH,
        version: "0.2.0",
        release_url: "https://example.invalid/tinychannels",
        assets,
    }));

    let calls = Arc::new(Mutex::new(Vec::new()));
    let bus = MemoryBus::new();
    let _broker = Broker::new().spawn(bus.clone());
    let service = Connection::connect(bus.connect().await.expect("service transport"))
        .await
        .expect("service connection");
    service
        .serve_at(
            ObjectPath::new(tinychannels_bus::OBJECT_PATH).expect("valid module path"),
            RecordingModule(Arc::clone(&calls)),
        )
        .await
        .expect("serve fixture");
    service
        .request_name(tinychannels_bus::BUS_NAME)
        .await
        .expect("own module name");
    let caller = Connection::connect(bus.connect().await.expect("caller transport"))
        .await
        .expect("caller connection");
    let control = caller
        .proxy(
            tinychannels_bus::BUS_NAME,
            tinychannels_bus::OBJECT_PATH,
            tinychannels_bus::BUS_NAME,
        )
        .expect("ordinary fixture control proxy");
    let module = ModuleClient::new(TestAccess {
        record,
        _service: service,
        connection: caller,
        reporter: FailureReporter::new(Arc::new(NoFailures)),
    });
    (TinyChannelsClient::new(module), calls, control)
}

#[test]
fn discord_replies_decode_only_the_matching_raw_projection() {
    let response: ChannelControllerResult = serde_json::from_value(json!({
        "result":"discord_guilds",
        "value":{"raw":[{"id":"guild-1","name":"Server","icon":null}]}
    }))
    .unwrap();
    let guilds: Vec<DiscordGuild> =
        TinyChannelsClient::decode_discord(response, DiscordReply::Guilds).unwrap();
    assert_eq!(guilds[0].id, "guild-1");

    let wrong_projection: ChannelControllerResult = serde_json::from_value(json!({
        "result":"discord_channels",
        "value":{"raw":[]}
    }))
    .unwrap();
    assert!(
        TinyChannelsClient::decode_discord::<Vec<DiscordGuild>>(
            wrong_projection,
            DiscordReply::Guilds
        )
        .is_none()
    );

    let malformed: ChannelControllerResult = serde_json::from_value(json!({
        "result":"discord_guilds",
        "value":{"raw":[{"id":"guild-1"}]}
    }))
    .unwrap();
    assert!(
        TinyChannelsClient::decode_discord::<Vec<DiscordGuild>>(malformed, DiscordReply::Guilds)
            .is_none()
    );
}

#[test]
fn discord_requests_keep_the_confidential_controller_wire_shape() {
    let config = ChannelsConfig::default();
    let proxy = HttpProxyPolicy::default();
    let token = "private-token";

    let requests = [
        (
            ChannelControllerRequest::DiscordListGuilds {
                config: config.clone(),
                bot_token: token.into(),
                proxy: proxy.clone(),
            },
            json!({
                "operation":"discord_list_guilds",
                "request":{"config":config,"bot_token":token,"proxy":proxy}
            }),
        ),
        (
            ChannelControllerRequest::DiscordListChannels {
                config: ChannelsConfig::default(),
                guild_id: "guild-1".into(),
                bot_token: token.into(),
                proxy: HttpProxyPolicy::default(),
            },
            json!({
                "operation":"discord_list_channels",
                "request":{"config":ChannelsConfig::default(),"guild_id":"guild-1","bot_token":token,"proxy":HttpProxyPolicy::default()}
            }),
        ),
        (
            ChannelControllerRequest::DiscordCheckPermissions {
                config: ChannelsConfig::default(),
                guild_id: "guild-1".into(),
                channel_id: "channel-1".into(),
                bot_token: token.into(),
                proxy: HttpProxyPolicy::default(),
            },
            json!({
                "operation":"discord_check_permissions",
                "request":{"config":ChannelsConfig::default(),"guild_id":"guild-1","channel_id":"channel-1","bot_token":token,"proxy":HttpProxyPolicy::default()}
            }),
        ),
    ];

    for (request, expected) in requests {
        assert_eq!(serde_json::to_value((request,)).unwrap(), json!([expected]));
    }
}

#[tokio::test]
async fn discord_credentials_never_fall_back_when_peer_proof_is_missing() {
    let (client, calls, _) = unattested_client().await;
    let guilds = client
        .discord_list_guilds(
            ChannelsConfig::default(),
            "fixture-private-token".into(),
            HttpProxyPolicy::default(),
        )
        .await;
    let channels = client
        .discord_list_channels(
            ChannelsConfig::default(),
            "fixture-guild".into(),
            "fixture-private-token".into(),
            HttpProxyPolicy::default(),
        )
        .await;
    let permissions = client
        .discord_check_permissions(
            ChannelsConfig::default(),
            "fixture-guild".into(),
            "fixture-channel".into(),
            "fixture-private-token".into(),
            HttpProxyPolicy::default(),
        )
        .await;
    let health = client
        .check_health(ChannelsConfig::default(), HttpProxyPolicy::default(), 1_000)
        .await;

    for error in [
        guilds.expect_err("unattested guild call must be rejected"),
        channels.expect_err("unattested channel call must be rejected"),
        permissions.expect_err("unattested permission call must be rejected"),
        health.expect_err("unattested health call must be rejected"),
    ] {
        assert_eq!(error.kind, ModuleCallError::Unavailable);
        assert!(!format!("{error:?}").contains("fixture-private-token"));
        assert!(!error.to_string().contains("fixture-private-token"));
    }
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn detailed_remote_invalid_input_is_available_only_in_explicit_fields() {
    let failure = ModuleCallFailure {
        kind: ModuleCallError::ModuleFault,
        wire_name: "ai.tinyhumans.tinychannels.Error.InvalidInput".into(),
        message: "invalid request containing fixture-private-token".into(),
    };

    assert_eq!(
        failure.wire_name,
        "ai.tinyhumans.tinychannels.Error.InvalidInput"
    );
    assert!(failure.message.contains("fixture-private-token"));
    assert!(!format!("{failure:?}").contains("fixture-private-token"));
    assert!(!failure.to_string().contains("fixture-private-token"));
}
