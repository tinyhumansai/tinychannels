//! Cold fixture readiness is separate from bounded runtime operations.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

mod support;

use tinybus::module::ModuleCallError;
use tinychannels_client::ChannelRuntimeErrorKind;

#[tokio::test]
async fn withheld_ready_fails_closed_within_the_client_deadline() {
    // One ModuleRuntime owns the fixture and its gate for this process. The
    // module serves its object and claims its name before entering the host
    // callback gate, but TinyBus cannot publish READY until setup returns.
    let fixture = support::gated_runtime_fixture().await;
    let module_ready_before_bind = fixture.module_ready();

    let limited_client = fixture.client.clone();
    let before_limited_bind = fixture.readiness_attempts();
    let limited_bind = tokio::spawn(async move { limited_client.runtime_driver().await });
    let readiness_started = fixture.readiness_attempt_started(before_limited_bind).await;
    let bind_pending_at_gate = !limited_bind.is_finished();

    // Native module startup and callback-gate entry above use real time. Pause
    // only after preparation is blocked at READY, so advancing the host clock
    // exercises the existing caller deadline without advancing the DLL clock.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(2)).await;
    let bind_outcome = limited_bind.await;
    let module_ready_after_timeout = fixture.module_ready();
    tokio::time::resume();

    // Always release and drain the foreign setup callback before asserting the
    // captured result, so a failed expectation cannot strand the DLL worker.
    fixture.release_ready();
    let gate_callback_returned = fixture.wait_until_gate_callback_returns().await;
    let _cleanup_result = fixture
        .runtime
        .host()
        .ensure_ready(tinychannels_bus::BUS_NAME)
        .await;

    let error = match bind_outcome {
        Ok(Err(error)) => Some(error),
        _ => None,
    };
    assert!(
        !module_ready_before_bind,
        "host must not report gated A as READY"
    );
    assert!(
        readiness_started,
        "client entered host readiness while native setup remained gated"
    );
    assert!(
        bind_pending_at_gate,
        "a held READY barrier must keep the attested bind pending"
    );
    assert!(gate_callback_returned, "released host callback must drain");
    assert!(
        !module_ready_after_timeout,
        "timed-out bind must not accept the withheld READY module"
    );
    assert_eq!(
        error.map(|error| error.kind()),
        Some(ChannelRuntimeErrorKind::Module(
            ModuleCallError::Unavailable
        )),
        "withheld READY must fail closed at the existing two-second client deadline"
    );
}
