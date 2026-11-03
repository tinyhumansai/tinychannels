use super::*;

use tinybus::module::RuntimeLeaseDriver;

#[test]
fn runtime_driver_public_type_implements_the_sdk_lease_contract() {
    fn assert_driver<T: RuntimeLeaseDriver>() {}

    assert_driver::<ChannelRuntimeDriver>();
}

#[tokio::test]
async fn runtime_driver_builders_bind_strictly_before_any_runtime_call() {
    let (client, calls, control) = unattested_client().await;

    // The peer is alive and ordinary calls work, so refusing the runtime API
    // proves strict binding rather than a missing module or bus connection.
    let ready: bool = control
        .call("FixtureReady", ())
        .await
        .expect("ordinary readiness control succeeds on the live peer");
    assert!(ready);
    let before = calls.lock().expect("call log").clone();
    assert_eq!(before, ["FixtureReady"]);

    assert!(client.runtime_driver().await.is_err());
    assert!(
        client
            .runtime_driver_with_dispatch(tinychannels_bus::ChannelRuntimeDispatchPolicy {
                listener_count: 1
            },)
            .await
            .is_err()
    );

    // Failed proof must occur before ReserveRuntime or any business method is
    // delivered. In particular, availability must never trigger fallback.
    assert_eq!(*calls.lock().expect("call log after refusal"), before);
}
