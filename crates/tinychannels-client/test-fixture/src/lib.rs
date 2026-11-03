//! Native test-only module for checking the public TinyChannels client routes.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

use async_trait::async_trait;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tinybus::{
    CallContext, Connection, Error, Interface, InterfaceName, MemberName, ObjectPath, Result,
};
use tinychannels_bus::{
    ChannelControllerRequest, ChannelControllerResult, ChannelInteractionRequest,
    ChannelInteractionResult, ChannelOutboundIntent, ChannelRuntimeDispatchPolicy,
    ChannelRuntimeReservation, ChannelSupervisionPolicy, ChannelsConfig, HttpProxyPolicy,
    controllers::{
        ChannelSendMessageResult, DiscordChannelListResult, DiscordGuildListResult,
        DiscordPermissionCheckResult,
    },
    methods,
    module_control::{ChannelControlRequest, ChannelControlResult},
    module_progressive::{ProgressiveCommand, ProgressiveResult},
    module_relay::RelayReservation,
};

const BUS_NAME: &str = tinychannels_bus::BUS_NAME;
const OBJECT_PATH: &str = tinychannels_bus::OBJECT_PATH;
const TOKEN: &str = "fixture-call-secret";
const CONFIG_TOKEN: &str = "fixture-config-secret";
const INVALID_INPUT: &str = "ai.tinyhumans.tinychannels.Error.InvalidInput";
const READINESS_GATE_PATH: &str = "/ai/tinyhumans/tinychannels/ChannelsHost/test_fixture_ready_gate";

#[derive(Deserialize)]
struct FixtureConfig {
    #[serde(default)]
    hold_ready: bool,
}

struct FixtureService {
    connection: Connection,
    calls: Arc<AtomicUsize>,
    runtime_controls: Arc<RuntimeControls>,
}

struct RuntimeControls {
    fail_start: AtomicBool,
    fail_stop: AtomicBool,
    malformed_reservation: AtomicBool,
    malformed_status: AtomicBool,
    pause_next_reserve: AtomicBool,
    reserve_entered: tokio::sync::Semaphore,
    reserve_release: tokio::sync::Semaphore,
    delivery_flags: Mutex<Vec<(String, bool)>>,
    stop_calls: AtomicUsize,
    next_handle: AtomicU64,
}

impl Default for RuntimeControls {
    fn default() -> Self {
        Self {
            fail_start: AtomicBool::new(false),
            fail_stop: AtomicBool::new(false),
            malformed_reservation: AtomicBool::new(false),
            malformed_status: AtomicBool::new(false),
            pause_next_reserve: AtomicBool::new(false),
            reserve_entered: tokio::sync::Semaphore::new(0),
            reserve_release: tokio::sync::Semaphore::new(0),
            delivery_flags: Mutex::new(Vec::new()),
            stop_calls: AtomicUsize::new(0),
            next_handle: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl Interface for FixtureService {
    fn name(&self) -> InterfaceName {
        InterfaceName::new(BUS_NAME).expect("TinyChannels bus name is valid")
    }

    fn members(&self) -> Vec<MemberName> {
        tinychannels_bus::METHODS
            .into_iter()
            .chain([
                "FixtureCallCount",
                "FixtureReady",
                "FixtureReleaseAlias",
                "FixtureDisconnect",
                "FixtureFailStart",
                "FixtureFailStop",
                "FixtureStopCallCount",
                "FixtureMalformedReservation",
                "FixtureMalformedStatus",
                "FixturePauseReserve",
                "FixtureWaitReserve",
                "FixtureReleaseReserve",
                "FixtureTakeDeliveryFlags",
            ])
            .map(|method| MemberName::new(method).expect("TinyChannels member is valid"))
            .collect()
    }

    fn requires_confidential(&self, member: &MemberName) -> bool {
        matches!(
            member.as_str(),
            methods::CHANNEL_CONTROLLER
                | methods::CHECK_CHANNEL_HEALTH
                | methods::RESERVE_RUNTIME
                | methods::START_RUNTIME
                | methods::START_RUNTIME_WITH_DISPATCH
                | methods::STOP_RUNTIME
                | methods::DESCRIBE_CONFIGURED_CHANNELS
                | methods::START_CHANNEL_SUPERVISED
                | methods::START_OUTBOUND_CHANNEL
                | methods::PREPARE_CHANNEL_CONTROL
                | methods::EXECUTE_PROGRESSIVE
                | methods::PREPARE_OUTBOUND_INTENT
                | methods::CHANNEL_INTERACTION
                | methods::START_RELAY
        )
    }

    async fn call(&self, member: &MemberName, args: Value) -> Result<Value> {
        if member.as_str() == "FixtureCallCount" {
            return Ok(json!(self.calls.load(Ordering::SeqCst)));
        }
        if member.as_str() == "FixtureStopCallCount" {
            return Ok(json!(
                self.runtime_controls.stop_calls.load(Ordering::SeqCst)
            ));
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        match member.as_str() {
            "FixtureReady" => Ok(json!(true)),
            "FixtureFailStart" => {
                self.runtime_controls
                    .fail_start
                    .store(true, Ordering::SeqCst);
                Ok(Value::Null)
            }
            "FixtureFailStop" => {
                self.runtime_controls
                    .fail_stop
                    .store(true, Ordering::SeqCst);
                Ok(Value::Null)
            }
            "FixtureMalformedReservation" => {
                self.runtime_controls
                    .malformed_reservation
                    .store(true, Ordering::SeqCst);
                Ok(Value::Null)
            }
            "FixtureMalformedStatus" => {
                self.runtime_controls
                    .malformed_status
                    .store(true, Ordering::SeqCst);
                Ok(Value::Null)
            }
            "FixturePauseReserve" => {
                self.runtime_controls
                    .pause_next_reserve
                    .store(true, Ordering::SeqCst);
                Ok(Value::Null)
            }
            "FixtureWaitReserve" => {
                self.runtime_controls
                    .reserve_entered
                    .acquire()
                    .await
                    .map_err(|_| Error::failed("reservation gate closed"))?
                    .forget();
                Ok(Value::Null)
            }
            "FixtureReleaseReserve" => {
                self.runtime_controls.reserve_release.add_permits(1);
                Ok(Value::Null)
            }
            "FixtureTakeDeliveryFlags" => {
                let flags = std::mem::take(
                    &mut *self
                        .runtime_controls
                        .delivery_flags
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
                serde_json::to_value(flags)
                    .map_err(|_| Error::failed("delivery flags could not serialize"))
            }
            "FixtureReleaseAlias" => {
                self.connection.release_name(BUS_NAME).await?;
                Ok(Value::Null)
            }
            "FixtureDisconnect" => {
                self.connection.close().await?;
                Ok(Value::Null)
            }
            methods::RESERVE_RUNTIME => {
                arguments(&args, 0)?;
                self.reserve_runtime().await
            }
            methods::START_RUNTIME => self.start_runtime(args, false).await,
            methods::START_RUNTIME_WITH_DISPATCH => self.start_runtime(args, true).await,
            methods::STOP_RUNTIME => {
                self.runtime_controls
                    .stop_calls
                    .fetch_add(1, Ordering::SeqCst);
                let handle = first_string(&args)?;
                if handle.len() != 32 {
                    return Err(Error::failed("runtime stop handle mismatch"));
                }
                if self
                    .runtime_controls
                    .fail_stop
                    .swap(false, Ordering::SeqCst)
                {
                    return Err(Error::failed("cleanup pending"));
                }
                let (object_path, _) = runtime_paths(handle);
                self.connection
                    .unserve(&ObjectPath::new(&object_path)?)
                    .await;
                Ok(Value::Null)
            }
            methods::CHANNEL_CONTROLLER => self.channel_controller(args),
            methods::CHECK_CHANNEL_HEALTH => self.check_health(args),
            _ => Err(Error::failed("unknown test fixture method")),
        }
    }

    async fn call_with_context(
        &self,
        member: &MemberName,
        args: Value,
        context: &CallContext,
    ) -> Result<Value> {
        if audited_delivery_member(member.as_str()) {
            self.runtime_controls
                .delivery_flags
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((member.as_str().to_owned(), context.is_confidential()));
        }
        self.call(member, args).await
    }
}

impl FixtureService {
    async fn reserve_runtime(&self) -> Result<Value> {
        if self
            .runtime_controls
            .pause_next_reserve
            .swap(false, Ordering::SeqCst)
        {
            self.runtime_controls.reserve_entered.add_permits(1);
            self.runtime_controls
                .reserve_release
                .acquire()
                .await
                .map_err(|_| Error::failed("reservation gate closed"))?
                .forget();
        }
        if self
            .runtime_controls
            .malformed_reservation
            .swap(false, Ordering::SeqCst)
        {
            return Ok(json!({
                "handle": 17,
                "object_path": "invalid-runtime-object",
                "host_object_path": "invalid-host-object"
            }));
        }
        let handle = format!(
            "{:032x}",
            self.runtime_controls
                .next_handle
                .fetch_add(1, Ordering::SeqCst)
                + 1
        );
        let (object_path, host_object_path) = runtime_paths(&handle);
        let reservation = ChannelRuntimeReservation {
            handle,
            object_path: object_path.clone(),
            host_object_path,
        };
        self.connection
            .serve_at(
                ObjectPath::new(&object_path)?,
                FixtureObject {
                    calls: Arc::clone(&self.calls),
                    runtime_controls: Arc::clone(&self.runtime_controls),
                },
            )
            .await?;
        serde_json::to_value(reservation).map_err(|_| Error::failed("fixture serialization failed"))
    }

    async fn start_runtime(&self, args: Value, with_dispatch: bool) -> Result<Value> {
        let values = arguments(&args, if with_dispatch { 2 } else { 1 })?;
        let Some(handle) = values.first().and_then(Value::as_str) else {
            return Err(Error::failed("runtime start handle mismatch"));
        };
        if handle.len() != 32 {
            return Err(Error::failed("runtime start handle mismatch"));
        }
        if with_dispatch {
            let policy: ChannelRuntimeDispatchPolicy = decode(values, 1)?;
            if policy.listener_count != 2 {
                return Err(Error::failed("runtime dispatch policy mismatch"));
            }
        }
        if self
            .runtime_controls
            .fail_start
            .swap(false, Ordering::SeqCst)
        {
            return Err(Error::failed("cleanup pending"));
        }
        let (_, callback_path) = runtime_paths(handle);
        let callback = self
            .connection
            .proxy(
                tinychannels_bus::HOST_BUS_NAME,
                &callback_path,
                tinychannels_bus::HOST_BUS_NAME,
            )?
            .call::<String>("FixtureCallback", ())
            .await?;
        if callback != "callback-live" {
            return Err(Error::failed("runtime callback was not registered"));
        }
        Ok(Value::Null)
    }

    fn channel_controller(&self, args: Value) -> Result<Value> {
        let request = args
            .get(0)
            .cloned()
            .and_then(|value| serde_json::from_value::<ChannelControllerRequest>(value).ok())
            .ok_or_else(|| Error::failed("invalid test fixture request"))?;
        match request {
            ChannelControllerRequest::DiscordListGuilds {
                config,
                bot_token,
                proxy,
            } => {
                validate_request(&config, &bot_token, &proxy, None, None)?;
                if proxy.service_selectors == ["test:invalid-input"] {
                    return Err(invalid_input());
                }
                let result = match scenario(&proxy) {
                    "test:wrong-variant" => {
                        ChannelControllerResult::DiscordChannels(DiscordChannelListResult {
                            raw: Some(json!([])),
                            ..Default::default()
                        })
                    }
                    "test:missing-raw" => {
                        ChannelControllerResult::DiscordGuilds(DiscordGuildListResult::default())
                    }
                    "test:malformed-payload" => {
                        ChannelControllerResult::DiscordGuilds(DiscordGuildListResult {
                            raw: Some(json!({"private":"fixture"})),
                            ..Default::default()
                        })
                    }
                    _ => ChannelControllerResult::DiscordGuilds(DiscordGuildListResult {
                        raw: Some(json!([{"id":"guild-1","name":"Fixture guild","icon":null}])),
                        ..Default::default()
                    }),
                };
                serde_json::to_value(result)
                    .map_err(|_| Error::failed("fixture serialization failed"))
            }
            ChannelControllerRequest::DiscordListChannels {
                config,
                guild_id,
                bot_token,
                proxy,
            } => {
                validate_request(&config, &bot_token, &proxy, Some(&guild_id), None)?;
                let result = match scenario(&proxy) {
                    "test:wrong-variant" => {
                        ChannelControllerResult::DiscordGuilds(DiscordGuildListResult {
                            raw: Some(json!([])),
                            ..Default::default()
                        })
                    }
                    "test:missing-raw" => {
                        ChannelControllerResult::DiscordChannels(DiscordChannelListResult::default())
                    }
                    _ => ChannelControllerResult::DiscordChannels(DiscordChannelListResult {
                        raw: Some(
                            json!([{"id":"channel-1","name":"general","type":0,"position":11,"parent_id":"category-1"}]),
                        ),
                        ..Default::default()
                    }),
                };
                serde_json::to_value(result)
                    .map_err(|_| Error::failed("fixture serialization failed"))
            }
            ChannelControllerRequest::DiscordCheckPermissions {
                config,
                guild_id,
                channel_id,
                bot_token,
                proxy,
            } => {
                validate_request(
                    &config,
                    &bot_token,
                    &proxy,
                    Some(&guild_id),
                    Some(&channel_id),
                )?;
                let result = match scenario(&proxy) {
                    "test:wrong-variant" => {
                        ChannelControllerResult::DiscordGuilds(DiscordGuildListResult {
                            raw: Some(json!([])),
                            ..Default::default()
                        })
                    }
                    "test:missing-raw" => ChannelControllerResult::DiscordPermissions(
                        DiscordPermissionCheckResult::default(),
                    ),
                    "test:malformed-payload" => {
                        ChannelControllerResult::DiscordPermissions(DiscordPermissionCheckResult {
                            raw: Some(json!(false)),
                            ..Default::default()
                        })
                    }
                    _ => {
                        ChannelControllerResult::DiscordPermissions(DiscordPermissionCheckResult {
                            raw: Some(
                                json!({"can_view_channel":true,"can_send_messages":false,"can_read_message_history":true,"missing_permissions":["SEND_MESSAGES"]}),
                            ),
                            ..Default::default()
                        })
                    }
                };
                serde_json::to_value(result)
                    .map_err(|_| Error::failed("fixture serialization failed"))
            }
            _ => Err(Error::failed("unexpected controller operation")),
        }
    }

    fn check_health(&self, args: Value) -> Result<Value> {
        let Some(values) = args.as_array() else {
            return Err(Error::failed("invalid health fixture arguments"));
        };
        let config = values
            .first()
            .cloned()
            .and_then(|value| serde_json::from_value::<ChannelsConfig>(value).ok())
            .ok_or_else(|| Error::failed("invalid health config"))?;
        let proxy = values
            .get(1)
            .cloned()
            .and_then(|value| serde_json::from_value::<HttpProxyPolicy>(value).ok())
            .ok_or_else(|| Error::failed("invalid health proxy"))?;
        let timeout = values.get(2).and_then(Value::as_u64);
        validate_request(&config, TOKEN, &proxy, None, None)?;
        if timeout != Some(12_345) {
            return Err(invalid_input());
        }
        match scenario(&proxy) {
            "test:malformed-payload" => Ok(json!([{"channel":17,"state":"healthy"}])),
            "test:invalid-input" => Err(invalid_input()),
            _ => Ok(json!([
                {"channel":"discord","state":"healthy"},
                {"channel":"telegram","state":"timeout"}
            ])),
        }
    }
}

struct FixtureObject {
    calls: Arc<AtomicUsize>,
    runtime_controls: Arc<RuntimeControls>,
}

#[async_trait]
impl Interface for FixtureObject {
    fn name(&self) -> InterfaceName {
        InterfaceName::new(BUS_NAME).expect("TinyChannels bus name is valid")
    }

    fn members(&self) -> Vec<MemberName> {
        tinychannels_bus::METHODS
            .into_iter()
            .map(|method| MemberName::new(method).expect("TinyChannels member is valid"))
            .collect()
    }

    fn requires_confidential(&self, member: &MemberName) -> bool {
        matches!(
            member.as_str(),
            methods::DESCRIBE_CONFIGURED_CHANNELS
                | methods::START_CHANNEL_SUPERVISED
                | methods::START_OUTBOUND_CHANNEL
                | methods::PREPARE_CHANNEL_CONTROL
                | methods::EXECUTE_PROGRESSIVE
                | methods::PREPARE_OUTBOUND_INTENT
                | methods::CHANNEL_INTERACTION
                | methods::START_RELAY
        )
    }

    async fn call(&self, member: &MemberName, args: Value) -> Result<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match member.as_str() {
            methods::DESCRIBE_CONFIGURED_CHANNELS => {
                let values = arguments(&args, 2)?;
                let _: ChannelsConfig = decode(values, 0)?;
                let _: HttpProxyPolicy = decode(values, 1)?;
                Ok(json!([]))
            }
            methods::START_CHANNEL_SUPERVISED => {
                let values = arguments(&args, 4)?;
                let _: String = decode(values, 0)?;
                let _: ChannelsConfig = decode(values, 1)?;
                let _: HttpProxyPolicy = decode(values, 2)?;
                let _: ChannelSupervisionPolicy = decode(values, 3)?;
                Ok(Value::Null)
            }
            methods::START_OUTBOUND_CHANNEL => {
                let values = arguments(&args, 3)?;
                let _: String = decode(values, 0)?;
                let _: ChannelsConfig = decode(values, 1)?;
                let _: HttpProxyPolicy = decode(values, 2)?;
                Ok(Value::Null)
            }
            methods::PREPARE_CHANNEL_CONTROL => {
                let values = arguments(&args, 1)?;
                let _: ChannelControlRequest = decode(values, 0)?;
                serde_json::to_value(ChannelControlResult::RuntimeLimits {
                    timeout_secs: 47,
                    max_in_flight: 8,
                })
                .map_err(|_| Error::failed("fixture serialization failed"))
            }
            methods::EXECUTE_PROGRESSIVE => {
                let values = arguments(&args, 1)?;
                let _: ProgressiveCommand = decode(values, 0)?;
                serde_json::to_value(ProgressiveResult::Closed)
                    .map_err(|_| Error::failed("fixture serialization failed"))
            }
            methods::PREPARE_OUTBOUND_INTENT => {
                let values = arguments(&args, 2)?;
                let _: String = decode(values, 0)?;
                let _: tinychannels_bus::module_runtime::ChannelMessagePreparation =
                    decode(values, 1)?;
                serde_json::to_value(tinychannels_bus::module_runtime::PreparedChannelIntent {
                    intent: ChannelOutboundIntent::default(),
                    legacy_message: Value::Null,
                })
                .map_err(|_| Error::failed("fixture serialization failed"))
            }
            methods::CHANNEL_INTERACTION => {
                let values = arguments(&args, 1)?;
                let _: ChannelInteractionRequest = decode(values, 0)?;
                serde_json::to_value(ChannelInteractionResult::Completed)
                    .map_err(|_| Error::failed("fixture serialization failed"))
            }
            methods::START_RELAY => {
                let values = arguments(&args, 2)?;
                let _: RelayReservation = decode(values, 0)?;
                let _: tinychannels_bus::config::RelayRuntimeConfig = decode(values, 1)?;
                serde_json::to_value(tinychannels_bus::relay::CapabilityDescriptor::default())
                    .map_err(|_| Error::failed("fixture serialization failed"))
            }
            methods::RESERVE_RELAY => {
                arguments(&args, 0)?;
                serde_json::to_value(RelayReservation("fixture-relay".into()))
                    .map_err(|_| Error::failed("fixture serialization failed"))
            }
            methods::STOP_RELAY => {
                let values = arguments(&args, 1)?;
                let _: RelayReservation = decode(values, 0)?;
                Ok(Value::Null)
            }
            methods::CHANNEL_STATUS => {
                let values = arguments(&args, 1)?;
                let _: String = decode(values, 0)?;
                if self
                    .runtime_controls
                    .malformed_status
                    .swap(false, Ordering::SeqCst)
                {
                    return Ok(json!({"unexpected":"object reply"}));
                }
                Ok(json!("peer-a"))
            }
            methods::SEND_CHANNEL_INTENT => {
                let values = arguments(&args, 2)?;
                let _: String = decode(values, 0)?;
                let _: ChannelOutboundIntent = decode(values, 1)?;
                serde_json::to_value(ChannelSendMessageResult::default())
                    .map_err(|_| Error::failed("fixture serialization failed"))
            }
            methods::SEND_RELAY_INTENT => {
                let values = arguments(&args, 2)?;
                let _: RelayReservation = decode(values, 0)?;
                let _: ChannelOutboundIntent = decode(values, 1)?;
                serde_json::to_value(ChannelSendMessageResult::default())
                    .map_err(|_| Error::failed("fixture serialization failed"))
            }
            methods::SHUTDOWN => {
                arguments(&args, 0)?;
                Ok(Value::Null)
            }
            _ => Err(Error::failed("unexpected TinyChannels object method")),
        }
    }

    async fn call_with_context(
        &self,
        member: &MemberName,
        args: Value,
        context: &CallContext,
    ) -> Result<Value> {
        if audited_delivery_member(member.as_str()) {
            self.runtime_controls
                .delivery_flags
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((member.as_str().to_owned(), context.is_confidential()));
        }
        self.call(member, args).await
    }
}

fn arguments(value: &Value, count: usize) -> Result<&[Value]> {
    let values = value
        .as_array()
        .ok_or_else(|| Error::failed("fixture arguments are not positional"))?;
    if values.len() != count {
        return Err(Error::failed("fixture argument count mismatch"));
    }
    Ok(values)
}

fn decode<T: DeserializeOwned>(values: &[Value], index: usize) -> Result<T> {
    values
        .get(index)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| Error::failed("fixture argument has the wrong type"))
}

fn first_string(args: &Value) -> Result<&str> {
    args.as_array()
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or_else(|| Error::failed("fixture runtime handle is missing"))
}

fn runtime_paths(handle: &str) -> (String, String) {
    (
        format!("{OBJECT_PATH}/runtime_{handle}"),
        format!("{}/runtime_{handle}", tinychannels_bus::HOST_OBJECT_PATH),
    )
}

fn audited_delivery_member(member: &str) -> bool {
    tinychannels_bus::METHODS.contains(&member)
}

fn validate_request(
    config: &ChannelsConfig,
    bot_token: &str,
    proxy: &HttpProxyPolicy,
    guild_id: Option<&str>,
    channel_id: Option<&str>,
) -> Result<()> {
    if serde_json::to_value(config).ok() != serde_json::to_value(expected_config()).ok()
        || bot_token != TOKEN
        || guild_id.is_some_and(|actual| actual != "guild-1")
        || channel_id.is_some_and(|actual| actual != "channel-1")
        || !valid_proxy(proxy)
    {
        return Err(Error::MethodFailed {
            name: INVALID_INPUT.into(),
            message: "test fixture request did not match expected fields".into(),
        });
    }
    Ok(())
}

fn valid_proxy(proxy: &HttpProxyPolicy) -> bool {
    proxy.http_proxy.as_deref() == Some("http://proxy.example.test:8080")
        && proxy.https_proxy.as_deref() == Some("https://proxy.example.test:8443")
        && proxy.all_proxy.as_deref() == Some("socks5://proxy.example.test:1080")
        && proxy.no_proxy == ["localhost", ".internal"]
        && (proxy.service_selectors == ["channel.discord"] || scenario(proxy).starts_with("test:"))
}

fn scenario(proxy: &HttpProxyPolicy) -> &str {
    proxy.service_selectors.first().map_or("", String::as_str)
}

fn expected_config() -> ChannelsConfig {
    ChannelsConfig {
        cli: false,
        message_timeout_secs: 47,
        active_channel: Some("discord".into()),
        discord: Some(tinychannels_bus::config::DiscordConfig {
            bot_token: CONFIG_TOKEN.into(),
            guild_id: Some("config-guild".into()),
            channel_id: Some("config-channel".into()),
            allowed_users: vec!["member-1".into()],
            listen_to_bots: true,
            mention_only: true,
        }),
        ..Default::default()
    }
}

fn invalid_input() -> Error {
    Error::MethodFailed {
        name: INVALID_INPUT.into(),
        message: "request contained fixture-private-token; input is invalid".into(),
    }
}

async fn setup(connection: Connection, config: FixtureConfig) -> Result<()> {
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime_controls = Arc::new(RuntimeControls::default());
    connection
        .serve_at(
            ObjectPath::new(OBJECT_PATH)?,
            FixtureService {
                connection: connection.clone(),
                calls,
                runtime_controls,
            },
        )
        .await?;
    connection.request_name(BUS_NAME).await?;
    if config.hold_ready {
        connection
            .proxy(
                tinychannels_bus::HOST_BUS_NAME,
                READINESS_GATE_PATH,
                tinychannels_bus::HOST_BUS_NAME,
            )?
            .call::<()>("WaitBeforeReady", ())
            .await?;
    }
    Ok(())
}

tinybus_module::module_export! {
    setup = setup,
    config = FixtureConfig,
    worker_threads = 1,
    provides = ["ai.tinyhumans.tinychannels.Channels"],
        methods = [
            "ChannelController", "CheckChannelHealth", "FixtureCallCount",
            "FixtureReady", "FixtureReleaseAlias", "FixtureDisconnect",
            "FixtureFailStart", "FixtureFailStop", "FixtureStopCallCount",
            "FixtureMalformedReservation", "FixtureMalformedStatus",
            "FixturePauseReserve", "FixtureWaitReserve", "FixtureReleaseReserve",
            "FixtureTakeDeliveryFlags",
            "ReserveRuntime", "StartRuntime", "StartRuntimeWithDispatch", "StopRuntime",
            "DescribeConfiguredChannels", "StartChannelSupervised", "StartOutboundChannel",
            "PrepareChannelControl", "ExecuteProgressive", "PrepareOutboundIntent",
            "ChannelInteraction", "StartRelay", "ReserveRelay", "StopRelay", "ChannelStatus",
            "SendChannelIntent", "SendRelayIntent", "Shutdown",
        ],
    signals = [],
    requires = [],
    optional = [],
    lazy = false,
}
