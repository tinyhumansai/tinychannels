//! Identity-scoped client and SDK lease lifecycle over one pinned native fixture.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

mod support;

use tinybus::{
    BusName, Error, Interface, InterfaceName, MemberName, ObjectPath,
    module::{ModuleCallError, RetryPolicy, RuntimeLeaseDriver},
};
use tinychannels_bus::{
    ChannelInteraction, ChannelInteractionRequest, ChannelInteractionResult, ChannelOutboundIntent,
    ChannelRuntimeDispatchPolicy, ChannelSupervisionPolicy, ChannelsConfig, HttpProxyPolicy,
    module_control::{ChannelControlRequest, ChannelControlResult},
    module_progressive::{ProgressiveCommand, ProgressiveResult},
    module_relay::RelayReservation,
    module_runtime::ChannelMessagePreparation,
};
use tinychannels_client::{ChannelRuntimeClient, ChannelRuntimeErrorKind};

const FIRST_RUNTIME_HANDLE: u64 = 1;
const LAST_RUNTIME_HANDLE: u64 = 4;

#[derive(Clone)]
struct FixtureCallback;

#[async_trait::async_trait]
impl Interface for FixtureCallback {
    fn name(&self) -> InterfaceName {
        InterfaceName::new(tinychannels_bus::HOST_BUS_NAME)
            .expect("TinyChannels host callback interface")
    }

    fn members(&self) -> Vec<MemberName> {
        vec![MemberName::new("FixtureCallback").expect("fixture callback method")]
    }

    async fn call(
        &self,
        _: &MemberName,
        _: serde_json::Value,
    ) -> tinybus::Result<serde_json::Value> {
        Ok(serde_json::json!("callback-live"))
    }
}

struct UnattestedReplacementRoot(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl Interface for UnattestedReplacementRoot {
    fn name(&self) -> InterfaceName {
        InterfaceName::new(tinychannels_bus::BUS_NAME).expect("TinyChannels module interface")
    }

    fn members(&self) -> Vec<MemberName> {
        vec![MemberName::new("FixtureReady").expect("readiness method")]
    }

    async fn call(
        &self,
        member: &MemberName,
        _: serde_json::Value,
    ) -> tinybus::Result<serde_json::Value> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if member.as_str() == "FixtureReady" {
            Ok(serde_json::json!(true))
        } else {
            Err(Error::failed("unexpected replacement root call"))
        }
    }
}

struct UnattestedReplacementObject(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl Interface for UnattestedReplacementObject {
    fn name(&self) -> InterfaceName {
        InterfaceName::new(tinychannels_bus::BUS_NAME).expect("TinyChannels module interface")
    }

    fn members(&self) -> Vec<MemberName> {
        vec![
            MemberName::new(tinychannels_bus::methods::CHANNEL_STATUS)
                .expect("channel status method"),
        ]
    }

    async fn call(
        &self,
        member: &MemberName,
        _: serde_json::Value,
    ) -> tinybus::Result<serde_json::Value> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if member.as_str() == tinychannels_bus::methods::CHANNEL_STATUS {
            Ok(serde_json::json!("peer-b"))
        } else {
            Err(Error::failed("unexpected replacement object call"))
        }
    }
}

fn runtime_handle(index: u64) -> String {
    format!("{index:032x}")
}

fn runtime_object_path(handle: u64) -> String {
    format!(
        "{}/runtime_{}",
        tinychannels_bus::OBJECT_PATH,
        runtime_handle(handle)
    )
}

async fn callback_probe(
    runtime: &'static tinybus::module::ModuleRuntime,
    path: &str,
) -> tinybus::Result<String> {
    runtime
        .connection()
        .proxy(
            tinychannels_bus::HOST_BUS_NAME,
            path,
            tinychannels_bus::HOST_BUS_NAME,
        )?
        .call("FixtureCallback", ())
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typed_driver_preserves_peer_through_dispatch_uncertainty_and_alias_handover() {
    // This process starts exactly one ModuleRuntime. It loads the digest-pinned
    // native A artifact and also owns the lease callback and replacement B.
    let fixture = support::gated_runtime_fixture().await;
    let startup_client = fixture.startup_client.clone();
    let before_startup_bind = fixture.readiness_attempts();
    let startup_bind = tokio::spawn(async move { startup_client.runtime_driver().await });
    let readiness_started = fixture.readiness_attempt_started(before_startup_bind).await;
    let bind_pending_at_gate = !startup_bind.is_finished();
    fixture.release_ready();
    let startup_result = startup_bind.await;
    let module_ready_after_release = fixture.module_ready();
    let startup_driver = startup_result
        .expect("default-deadline cold bind task completes")
        .expect("cold attested bind succeeds after setup publishes READY");
    assert!(
        readiness_started,
        "cold bind entered host readiness before the setup barrier was released"
    );
    assert!(
        bind_pending_at_gate,
        "cold bind waits for host readiness while the fixture setup barrier is held"
    );
    assert!(
        module_ready_after_release,
        "host observes READY for fixture A"
    );
    drop(startup_driver);
    assert!(
        fixture.module_ready(),
        "fixture startup readiness must complete before the two-second lease-operation deadline is applied"
    );
    let serial_driver = fixture.client.runtime_driver().await.unwrap_or_else(|_| {
        panic!(
            "bind A before any runtime reservation; native fixture stage: {}",
            fixture.bind_stage()
        )
    });
    let dispatch_driver = fixture
        .client
        .runtime_driver_with_dispatch(ChannelRuntimeDispatchPolicy { listener_count: 2 })
        .await
        .expect("bind A for an explicit dispatch opt-in");
    let start_uncertain_driver = fixture
        .client
        .runtime_driver()
        .await
        .expect("bind A for uncertain startup coverage");
    let stop_uncertain_driver = fixture
        .client
        .runtime_driver()
        .await
        .expect("bind A for uncertain shutdown coverage");
    let control = fixture
        .control
        .bind_attested("tinychannels")
        .await
        .expect("hold a test control reference to attested A");

    let ready: bool = control
        .call("FixtureReady", ())
        .await
        .expect("A is live before alias handover");
    assert!(ready);
    assert!(
        control
            .call::<()>(
                tinychannels_bus::methods::START_RUNTIME,
                (runtime_handle(FIRST_RUNTIME_HANDLE),),
            )
            .await
            .is_err(),
        "the native fixture refuses an ordinary lifecycle call"
    );
    fixture.reports.clear();
    control
        .call::<()>("FixtureMalformedReservation", ())
        .await
        .expect("configure one malformed reservation reply");
    let malformed_reservation = serial_driver
        .reserve()
        .await
        .expect_err("malformed reservation fails closed");
    assert_eq!(
        malformed_reservation.kind(),
        ChannelRuntimeErrorKind::Module(ModuleCallError::IncompatibleContract)
    );
    assert_eq!(
        fixture.reports.entries(),
        [(
            "tinychannels".into(),
            support::record(support::Pin::Correct).version.into(),
            "incompatible_contract".into(),
        )],
        "the SDK reports the malformed reservation once"
    );
    control
        .call::<()>("FixturePauseReserve", ())
        .await
        .expect("pause the next reservation reply from A");
    let serial_callback_path = Arc::new(Mutex::new(None::<String>));
    let serial_callback_capture = Arc::clone(&serial_callback_path);
    let serial_activation = fixture.runtime.lease(
        serial_driver,
        move |reservation, _| {
            *serial_callback_capture
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(reservation.host_object_path.clone());
            Ok(FixtureCallback)
        },
        RetryPolicy::new(Duration::from_secs(2), vec![]).expect("valid stop policy"),
        |_| {},
    );
    let serial_activation = tokio::spawn(serial_activation);
    control
        .call::<()>("FixtureWaitReserve", ())
        .await
        .expect("A is paused inside confidential ReserveRuntime");
    control
        .call::<()>("FixtureReleaseAlias", ())
        .await
        .expect("A releases its well-known name while Reserve is paused");

    // B is intentionally an ordinary, unattested host object. It is only an
    // availability/collision control and cannot receive confidential calls.
    let replacement_calls = Arc::new(AtomicUsize::new(0));
    let _replacement = fixture
        .runtime
        .serve_host_object(
            BusName::new(tinychannels_bus::BUS_NAME).expect("module bus name"),
            ObjectPath::new(tinychannels_bus::OBJECT_PATH).expect("module root path"),
            UnattestedReplacementRoot(Arc::clone(&replacement_calls)),
        )
        .await
        .expect("ordinary B can claim A's released alias");
    for handle in FIRST_RUNTIME_HANDLE..=LAST_RUNTIME_HANDLE {
        fixture
            .runtime
            .connection()
            .serve_at(
                ObjectPath::new(runtime_object_path(handle)).expect("B collision path"),
                UnattestedReplacementObject(Arc::clone(&replacement_calls)),
            )
            .await
            .expect("B serves a colliding private object path");
    }
    let b_owner = fixture
        .runtime
        .connection()
        .name_owner(tinychannels_bus::BUS_NAME)
        .await
        .expect("read B's owner")
        .expect("B owns the alias");
    assert!(b_owner.is_unique());
    assert!(
        fixture
            .runtime
            .connection()
            .attestation(b_owner)
            .await
            .expect("public B attestation lookup")
            .is_none()
    );
    let b_ready: bool = fixture
        .runtime
        .connection()
        .proxy(
            tinychannels_bus::BUS_NAME,
            tinychannels_bus::OBJECT_PATH,
            tinychannels_bus::BUS_NAME,
        )
        .expect("ordinary B root proxy")
        .call("FixtureReady", ())
        .await
        .expect("ordinary readiness proves B is available");
    assert!(b_ready);
    for handle in FIRST_RUNTIME_HANDLE..=LAST_RUNTIME_HANDLE {
        let b_status: String = fixture
            .runtime
            .connection()
            .proxy(
                tinychannels_bus::BUS_NAME,
                runtime_object_path(handle),
                tinychannels_bus::BUS_NAME,
            )
            .expect("ordinary B object proxy")
            .call(tinychannels_bus::methods::CHANNEL_STATUS, ("discord",))
            .await
            .expect("ordinary status proves B's colliding object is live");
        assert_eq!(b_status, "peer-b");
    }
    let b_calls_before_leases = replacement_calls.load(Ordering::SeqCst);
    control
        .call::<()>("FixtureReleaseReserve", ())
        .await
        .expect("release the paused A reservation after B takes the alias");
    let serial_lease = serial_activation
        .await
        .expect("activation task")
        .expect("legacy reserve/start/callback transaction stays on A");
    fixture.reports.clear();
    control
        .call::<()>("FixtureMalformedStatus", ())
        .await
        .expect("configure one malformed object response");
    let malformed_status = serial_lease
        .object()
        .channel_status("discord".into())
        .await
        .expect_err("malformed typed object response fails closed");
    assert_eq!(
        malformed_status.kind(),
        ChannelRuntimeErrorKind::Module(ModuleCallError::IncompatibleContract)
    );
    assert_eq!(
        fixture.reports.entries(),
        [(
            "tinychannels".into(),
            support::record(support::Pin::Correct).version.into(),
            "incompatible_contract".into(),
        )],
        "the SDK reports the malformed object reply once"
    );
    let serial_path = serial_callback_path
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .expect("callback factory received A's returned path");
    assert_eq!(
        serial_lease
            .object()
            .channel_status("discord".into())
            .await
            .expect("positive typed object result from A after alias handover"),
        "peer-a"
    );

    // Exact typed object request/response projections. The fixture decodes
    // positional arguments and marks confidential members as such.
    assert!(
        serial_lease
            .object()
            .describe_configured_channels(ChannelsConfig::default(), HttpProxyPolicy::default())
            .await
            .expect("confidential DescribeConfiguredChannels")
            .is_empty()
    );
    serial_lease
        .object()
        .start_channel_supervised(
            "discord".into(),
            ChannelsConfig::default(),
            HttpProxyPolicy::default(),
            ChannelSupervisionPolicy::default(),
        )
        .await
        .expect("confidential StartChannelSupervised");
    serial_lease
        .object()
        .start_outbound_channel(
            "discord".into(),
            ChannelsConfig::default(),
            HttpProxyPolicy::default(),
        )
        .await
        .expect("confidential StartOutboundChannel");
    assert_eq!(
        serial_lease
            .object()
            .prepare_channel_control(ChannelControlRequest::RuntimeLimits {
                timeout_secs: 47,
                listener_count: 2,
            })
            .await
            .expect("confidential PrepareChannelControl"),
        ChannelControlResult::RuntimeLimits {
            timeout_secs: 47,
            max_in_flight: 8,
        }
    );
    assert_eq!(
        serial_lease
            .object()
            .execute_progressive(ProgressiveCommand::Close {
                handle: "progressive-fixture".into(),
            })
            .await
            .expect("confidential ExecuteProgressive"),
        ProgressiveResult::Closed
    );
    let prepared = serial_lease
        .object()
        .prepare_outbound_intent(
            "discord".into(),
            ChannelMessagePreparation::Legacy(serde_json::json!({"text":"fixture"})),
        )
        .await
        .expect("confidential PrepareOutboundIntent");
    assert!(prepared.legacy_message.is_null());
    assert!(matches!(
        serial_lease
            .object()
            .channel_interaction(ChannelInteractionRequest {
                channel: "discord".into(),
                interaction: ChannelInteraction::Describe,
            })
            .await
            .expect("confidential ChannelInteraction"),
        ChannelInteractionResult::Completed
    ));
    let relay = serial_lease
        .object()
        .reserve_relay()
        .await
        .expect("ordinary ReserveRelay");
    assert_eq!(relay, RelayReservation("fixture-relay".into()));
    assert_eq!(
        serial_lease
            .object()
            .start_relay(
                relay.clone(),
                tinychannels_bus::config::RelayRuntimeConfig::default()
            )
            .await
            .expect("confidential StartRelay response"),
        tinychannels_bus::relay::CapabilityDescriptor::default()
    );
    serial_lease
        .object()
        .stop_relay(relay.clone())
        .await
        .expect("ordinary StopRelay");
    serial_lease
        .object()
        .send_channel_intent("discord".into(), ChannelOutboundIntent::default())
        .await
        .expect("ordinary SendChannelIntent");
    serial_lease
        .object()
        .send_relay_intent(relay, ChannelOutboundIntent::default())
        .await
        .expect("ordinary SendRelayIntent");
    serial_lease
        .object()
        .shutdown()
        .await
        .expect("ordinary Shutdown");

    let ordinary_a_object = control
        .object_client(&runtime_object_path(FIRST_RUNTIME_HANDLE))
        .expect("test-only held object on A");
    assert!(
        ordinary_a_object
            .call::<Vec<tinychannels_bus::module_runtime::ConfiguredChannelInfo>>(
                tinychannels_bus::methods::DESCRIBE_CONFIGURED_CHANNELS,
                (ChannelsConfig::default(), HttpProxyPolicy::default()),
            )
            .await
            .is_err()
    );
    assert!(
        control
            .call::<()>(
                tinychannels_bus::methods::STOP_RUNTIME,
                (runtime_handle(FIRST_RUNTIME_HANDLE),),
            )
            .await
            .is_err()
    );
    assert!(
        serial_lease.close().await.is_ok(),
        "decoded STOP unit is ACK"
    );
    assert!(callback_probe(fixture.runtime, &serial_path).await.is_err());

    let dispatch_lease = fixture
        .runtime
        .lease(
            dispatch_driver,
            |_, _| Ok(FixtureCallback),
            RetryPolicy::new(Duration::from_secs(2), vec![]).expect("valid dispatch stop policy"),
            |_| {},
        )
        .await
        .expect("explicit StartRuntimeWithDispatch path");
    assert_eq!(
        dispatch_lease
            .object()
            .channel_status("discord".into())
            .await
            .expect("dispatch object remains on A"),
        "peer-a"
    );
    assert!(dispatch_lease.close().await.is_ok());
    assert_eq!(
        replacement_calls.load(Ordering::SeqCst),
        b_calls_before_leases,
        "positive A results and callback traffic show no lease call reached B"
    );

    control
        .call::<()>("FixtureFailStart", ())
        .await
        .expect("configure one opaque START failure");
    let uncertain_start_object = Arc::new(Mutex::new(None::<ChannelRuntimeClient>));
    let uncertain_start_capture = Arc::clone(&uncertain_start_object);
    let uncertain_start_path = Arc::new(Mutex::new(None::<String>));
    let uncertain_start_path_capture = Arc::clone(&uncertain_start_path);
    let start_result = fixture
        .runtime
        .lease(
            start_uncertain_driver,
            move |reservation, object| {
                *uncertain_start_capture
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(object.clone());
                *uncertain_start_path_capture
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(reservation.host_object_path.clone());
                Ok(FixtureCallback)
            },
            RetryPolicy::new(Duration::from_secs(2), vec![]).expect("valid start policy"),
            |_| {},
        )
        .await;
    assert!(start_result.is_err(), "opaque START failure is not success");
    let start_path = uncertain_start_path
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .expect("callback factory received the returned start path");
    assert_eq!(
        callback_probe(fixture.runtime, &start_path)
            .await
            .expect("unacknowledged START retains its callback"),
        "callback-live"
    );
    let uncertain_object = uncertain_start_object
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .expect("SDK retains the typed object during uncertain START")
        .clone();
    assert_eq!(
        uncertain_object
            .channel_status("discord".into())
            .await
            .expect("the captured object remains bound to A"),
        "peer-a"
    );
    let stops_after_uncertain_start: usize = control
        .call("FixtureStopCallCount", ())
        .await
        .expect("read stop count after uncertain start");
    assert_eq!(
        stops_after_uncertain_start, 2,
        "Unacknowledged START sends no STOP"
    );

    control
        .call::<()>("FixtureFailStop", ())
        .await
        .expect("configure one opaque cleanup-pending STOP failure");
    let uncertain_stop_path = Arc::new(Mutex::new(None::<String>));
    let uncertain_stop_capture = Arc::clone(&uncertain_stop_path);
    let stop_lease = fixture
        .runtime
        .lease(
            stop_uncertain_driver,
            move |reservation, _| {
                *uncertain_stop_capture
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(reservation.host_object_path.clone());
                Ok(FixtureCallback)
            },
            RetryPolicy::new(Duration::from_millis(100), vec![Duration::from_millis(1)])
                .expect("finite retry policy cannot retry opaque STOP"),
            |_| {},
        )
        .await
        .expect("start the final lease before uncertain STOP");
    assert!(stop_lease.close().await.is_err());
    assert!(stop_lease.close().await.is_err());
    let stops_after_uncertain_stop: usize = control
        .call("FixtureStopCallCount", ())
        .await
        .expect("read stop count after uncertain close");
    assert_eq!(
        stops_after_uncertain_stop, 3,
        "opaque cleanup-pending error does not authorize a second STOP"
    );
    let stop_path = uncertain_stop_path
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .expect("callback factory received the returned stop path");
    assert_eq!(
        callback_probe(fixture.runtime, &stop_path)
            .await
            .expect("Unacknowledged STOP retains the callback"),
        "callback-live"
    );
    assert_eq!(
        stop_lease
            .object()
            .channel_status("discord".into())
            .await
            .expect("uncertain STOP retains the bound A object"),
        "peer-a"
    );
    assert_eq!(
        replacement_calls.load(Ordering::SeqCst),
        b_calls_before_leases,
        "all positive A calls bypass the live unattested B"
    );

    let delivery_flags: Vec<(String, bool)> = control
        .call("FixtureTakeDeliveryFlags", ())
        .await
        .expect("read broker-authenticated call classifications");
    let confidential_methods = [
        tinychannels_bus::methods::RESERVE_RUNTIME,
        tinychannels_bus::methods::START_RUNTIME,
        tinychannels_bus::methods::START_RUNTIME_WITH_DISPATCH,
        tinychannels_bus::methods::STOP_RUNTIME,
        tinychannels_bus::methods::DESCRIBE_CONFIGURED_CHANNELS,
        tinychannels_bus::methods::START_CHANNEL_SUPERVISED,
        tinychannels_bus::methods::START_OUTBOUND_CHANNEL,
        tinychannels_bus::methods::PREPARE_CHANNEL_CONTROL,
        tinychannels_bus::methods::EXECUTE_PROGRESSIVE,
        tinychannels_bus::methods::PREPARE_OUTBOUND_INTENT,
        tinychannels_bus::methods::CHANNEL_INTERACTION,
        tinychannels_bus::methods::START_RELAY,
    ];
    let tested_methods = [
        tinychannels_bus::methods::RESERVE_RUNTIME,
        tinychannels_bus::methods::START_RUNTIME,
        tinychannels_bus::methods::START_RUNTIME_WITH_DISPATCH,
        tinychannels_bus::methods::STOP_RUNTIME,
        tinychannels_bus::methods::DESCRIBE_CONFIGURED_CHANNELS,
        tinychannels_bus::methods::START_CHANNEL_SUPERVISED,
        tinychannels_bus::methods::START_OUTBOUND_CHANNEL,
        tinychannels_bus::methods::PREPARE_CHANNEL_CONTROL,
        tinychannels_bus::methods::EXECUTE_PROGRESSIVE,
        tinychannels_bus::methods::PREPARE_OUTBOUND_INTENT,
        tinychannels_bus::methods::CHANNEL_INTERACTION,
        tinychannels_bus::methods::START_RELAY,
        tinychannels_bus::methods::RESERVE_RELAY,
        tinychannels_bus::methods::STOP_RELAY,
        tinychannels_bus::methods::CHANNEL_STATUS,
        tinychannels_bus::methods::SEND_CHANNEL_INTENT,
        tinychannels_bus::methods::SEND_RELAY_INTENT,
        tinychannels_bus::methods::SHUTDOWN,
    ];
    for method in tested_methods {
        assert!(
            delivery_flags.iter().any(|(seen, _)| seen == method),
            "the fixture observed the typed wrapper call for {method}"
        );
    }
    for (method, confidential) in &delivery_flags {
        assert_eq!(
            *confidential,
            confidential_methods.contains(&method.as_str()),
            "delivery flag for {method}"
        );
    }

    // The test-only control closes A's native service connection. Its held
    // object fails while the ordinary B controls remain available.
    let _ = control.call::<()>("FixtureDisconnect", ()).await;
    assert!(control.call::<bool>("FixtureReady", ()).await.is_err());
    assert!(
        stop_lease
            .object()
            .channel_status("discord".into())
            .await
            .is_err()
    );
    let b_ready_after_disconnect: bool = fixture
        .control
        .call("tinychannels", "FixtureReady", ())
        .await
        .expect("ordinary B remains available after A disconnects");
    assert!(b_ready_after_disconnect);
    let b_status_after_disconnect: String = fixture
        .runtime
        .connection()
        .proxy(
            tinychannels_bus::BUS_NAME,
            runtime_object_path(LAST_RUNTIME_HANDLE),
            tinychannels_bus::BUS_NAME,
        )
        .expect("ordinary B object after A disconnect")
        .call(tinychannels_bus::methods::CHANNEL_STATUS, ("discord",))
        .await
        .expect("positive B object response after A disconnect");
    assert_eq!(b_status_after_disconnect, "peer-b");
    assert_eq!(
        replacement_calls.load(Ordering::SeqCst),
        b_calls_before_leases + 2,
        "only the explicit post-disconnect ordinary B probes reached B"
    );
}
