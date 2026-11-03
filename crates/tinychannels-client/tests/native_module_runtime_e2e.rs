//! Provider-free composition with the actual TinyChannels module cdylib.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

mod support;

use tinybus::{Interface, InterfaceName, MemberName, module::RetryPolicy};
use tinychannels_bus::{ChannelRuntimeReservation, ChannelsConfig, HttpProxyPolicy};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_native_module_runs_provider_free_runtime_and_releases_callback_on_ack() {
    // This test process owns one ModuleRuntime. The fixture artifact is the
    // locally built tinychannels-module cdylib with exact archive/library
    // hashes; no provider configuration or external network is used.
    let fixture = support::native_module_runtime_fixture().await;
    let driver = fixture
        .client
        .runtime_driver()
        .await
        .expect("strictly bind the locally built TinyChannels module artifact");
    let callback_path = Arc::new(Mutex::new(None::<String>));
    let captured_callback_path = Arc::clone(&callback_path);
    let lease = fixture
        .runtime
        .lease(
            driver,
            move |reservation: &ChannelRuntimeReservation, _| {
                *captured_callback_path
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(reservation.host_object_path.clone());
                Ok(FixtureCallback)
            },
            RetryPolicy::new(Duration::from_secs(2), vec![]).expect("valid stop policy"),
            |_| {},
        )
        .await
        .expect("real module reserves, creates its private object, and starts");

    let _configured = lease
        .object()
        .describe_configured_channels(ChannelsConfig::default(), HttpProxyPolicy::default())
        .await
        .expect("real module's typed provider description with empty configuration");
    let path = callback_path
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .expect("callback factory received the module-returned path");
    assert_eq!(
        fixture
            .runtime
            .connection()
            .proxy(
                tinychannels_bus::HOST_BUS_NAME,
                &path,
                tinychannels_bus::HOST_BUS_NAME,
            )
            .expect("registered host callback proxy")
            .call::<String>("FixtureCallback", ())
            .await
            .expect("registered callback is reachable at the returned path"),
        "callback-live"
    );
    assert!(lease.close().await.is_ok(), "real STOP unit reply is ACK");
    assert!(
        fixture
            .runtime
            .connection()
            .proxy(
                tinychannels_bus::HOST_BUS_NAME,
                &path,
                tinychannels_bus::HOST_BUS_NAME,
            )
            .expect("callback probe after STOP")
            .call::<String>("FixtureCallback", ())
            .await
            .is_err(),
        "the real STOP ACK releases the callback registration"
    );
}
