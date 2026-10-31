//! The statically linked build is admitted by a `ModuleHost` and reaches `Ready`.

#![cfg(feature = "static-link")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use tinybus::module::{ModuleRuntime, ModuleState, RuntimeNames};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn linked_module_is_admitted_and_becomes_ready() {
    let runtime = ModuleRuntime::start(&RuntimeNames::default(), None).unwrap();
    let host = runtime.host();
    let info = host
        .attach_linked_with_config(
            tinychannels_module::linked_module().unwrap(),
            serde_json::json!({}),
        )
        .expect("linked module is admitted");
    assert_eq!(info.name, "tinychannels-module");
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if host
                .list()
                .iter()
                .any(|module| matches!(module.state, ModuleState::Ready))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("linked module reaches Ready");
    assert_eq!(info.manifest.provides.len(), 1);
    assert_eq!(
        info.manifest.provides[0].version.interface.as_str(),
        tinychannels_bus::BUS_NAME
    );
}
