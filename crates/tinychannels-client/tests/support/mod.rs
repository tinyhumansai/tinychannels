//! Cached native module fixtures for the public client integration tests.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#![allow(
    dead_code,
    reason = "the native fixtures are shared by separate integration targets"
)]

use std::{
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::Duration,
};

use tinybus::{
    Connection, Error, Interface, InterfaceName, MemberName, ObjectPath, Proxy,
    broker::Broker,
    module::{
        AccessFuture, CachedRelease, FailureReport, FailureReporter, FailureSink, HostObjectGuard,
        ModuleAccess, ModuleCallError, ModuleCallFailure, ModuleClient, ModuleHost, ModuleRuntime,
        ModuleState, ProxyFailure, ReleaseAsset, ReleaseRecord, RuntimeNames,
    },
    transport::memory::MemoryBus,
};
use tinychannels_client::TinyChannelsClient;
use tokio::sync::OnceCell;

const MODULE_ID: &str = tinychannels_bus::MODULE_ID;
const RELEASE_URL: &str = "https://github.com/tinyhumansai/tinychannels/releases/tag/v0.2.0";
const READINESS_GATE_PATH: &str =
    "/ai/tinyhumans/tinychannels/ChannelsHost/test_fixture_ready_gate";

static RECORD: OnceLock<ReleaseRecord> = OnceLock::new();
static NATIVE_MODULE_RECORD: OnceLock<ReleaseRecord> = OnceLock::new();
static WRONG_ARCHIVE_RECORD: OnceLock<ReleaseRecord> = OnceLock::new();
static WRONG_LIBRARY_RECORD: OnceLock<ReleaseRecord> = OnceLock::new();

#[derive(Default)]
pub(crate) struct Reports(Mutex<Vec<(String, String, String)>>);

impl FailureSink for Reports {
    fn capture(&self, report: &FailureReport<'_>) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((
                report.module.to_owned(),
                report.version.to_owned(),
                report.reason.code().to_owned(),
            ));
    }
}

impl Reports {
    pub(crate) fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub(crate) fn entries(&self) -> Vec<(String, String, String)> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

pub(crate) struct Access {
    host: Arc<ModuleHost>,
    connection: Connection,
    loaded: Arc<OnceCell<()>>,
    record: &'static ReleaseRecord,
    release: CachedRelease<'static>,
    reporter: FailureReporter,
}

impl ModuleAccess for Access {
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
        Box::pin(async move {
            self.loaded
                .get_or_try_init(|| async {
                    self.host
                        .load_github_release_cached(&self.release, serde_json::json!({}))
                        .map(|_| ())
                        .map_err(|error| {
                            format!("the cached test module could not be admitted: {error}")
                        })
                })
                .await
                .map(|_| ())
        })
    }

    fn prepare_attested<'a>(&'a self, id: &'a str) -> AccessFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.ensure_loaded(id).await?;
            self.host
                .ensure_ready(self.record.bus_name)
                .await
                .map_err(|_| "the cached test module did not become ready".to_owned())
        })
    }

    fn proxy<'a>(
        &'a self,
        bus_name: &'a str,
        object_path: &'a str,
    ) -> AccessFuture<'a, Result<Proxy, ProxyFailure>> {
        Box::pin(async move {
            self.connection
                .proxy(bus_name, object_path, bus_name)
                .map_err(|_| ProxyFailure::Contract)
        })
    }
}

struct SharedAccess(Arc<Access>);

impl ModuleAccess for SharedAccess {
    fn find(&self, id: &str) -> Option<&'static ReleaseRecord> {
        self.0.find(id)
    }

    fn enabled(&self) -> bool {
        self.0.enabled()
    }

    fn reporter(&self) -> &FailureReporter {
        self.0.reporter()
    }

    fn ensure_loaded<'a>(&'a self, id: &'a str) -> AccessFuture<'a, Result<(), String>> {
        self.0.ensure_loaded(id)
    }

    fn prepare_attested<'a>(&'a self, id: &'a str) -> AccessFuture<'a, Result<(), String>> {
        self.0.prepare_attested(id)
    }

    fn proxy<'a>(
        &'a self,
        bus_name: &'a str,
        object_path: &'a str,
    ) -> AccessFuture<'a, Result<Proxy, ProxyFailure>> {
        self.0.proxy(bus_name, object_path)
    }
}

struct WrongPinAccess {
    access: Arc<Access>,
    record: &'static ReleaseRecord,
}

impl ModuleAccess for WrongPinAccess {
    fn find(&self, id: &str) -> Option<&'static ReleaseRecord> {
        (id == self.record.id).then_some(self.record)
    }

    fn enabled(&self) -> bool {
        self.access.enabled()
    }

    fn reporter(&self) -> &FailureReporter {
        self.access.reporter()
    }

    fn ensure_loaded<'a>(&'a self, id: &'a str) -> AccessFuture<'a, Result<(), String>> {
        self.access.ensure_loaded(id)
    }

    fn prepare_attested<'a>(&'a self, id: &'a str) -> AccessFuture<'a, Result<(), String>> {
        self.access.prepare_attested(id)
    }

    fn proxy<'a>(
        &'a self,
        bus_name: &'a str,
        object_path: &'a str,
    ) -> AccessFuture<'a, Result<Proxy, ProxyFailure>> {
        self.access.proxy(bus_name, object_path)
    }
}

pub(crate) struct Fixture {
    pub(crate) client: TinyChannelsClient,
    pub(crate) wrong_archive_client: TinyChannelsClient,
    pub(crate) wrong_library_client: TinyChannelsClient,
    pub(crate) probe: Connection,
    pub(crate) reports: Arc<Reports>,
    _host: Arc<ModuleHost>,
    _broker_task: tokio::task::JoinHandle<tinybus::Result<()>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self._broker_task.abort();
    }
}

pub(crate) async fn fixture() -> Fixture {
    let bus = MemoryBus::new();
    let broker = Broker::new();
    let broker_task = broker.clone().spawn(bus.clone());
    let host = Arc::new(ModuleHost::new(broker));
    let probe = Connection::connect(bus.connect().await.expect("probe transport"))
        .await
        .expect("probe connection");
    let release_record = record(Pin::Correct);
    let release = cached_release();
    let reports = Arc::new(Reports::default());
    let access = Arc::new(Access {
        host: Arc::clone(&host),
        connection: probe.clone(),
        loaded: Arc::new(OnceCell::new()),
        record: release_record,
        release,
        reporter: FailureReporter::new(reports.clone()),
    });
    access
        .ensure_loaded(MODULE_ID)
        .await
        .expect("cached pinned module should load without network access");
    host.ensure_ready(release_record.bus_name)
        .await
        .expect("cached pinned module should become ready");

    let wrong_archive_client =
        TinyChannelsClient::new(tinybus::module::ModuleClient::new(WrongPinAccess {
            access: Arc::clone(&access),
            record: record(Pin::WrongArchive),
        }));
    let wrong_library_client =
        TinyChannelsClient::new(tinybus::module::ModuleClient::new(WrongPinAccess {
            access: Arc::clone(&access),
            record: record(Pin::WrongLibrary),
        }));
    Fixture {
        client: TinyChannelsClient::new(tinybus::module::ModuleClient::new(SharedAccess(access))),
        wrong_archive_client,
        wrong_library_client,
        probe,
        reports,
        _host: host,
        _broker_task: broker_task,
    }
}

/// A locally cached native module loaded on the same broker used by SDK leases.
pub(crate) struct RuntimeFixture {
    pub(crate) client: TinyChannelsClient,
    pub(crate) startup_client: TinyChannelsClient,
    pub(crate) control: ModuleClient,
    pub(crate) runtime: &'static ModuleRuntime,
    pub(crate) reports: Arc<Reports>,
    bind_stage: Arc<AtomicU8>,
    readiness_attempts: Arc<AtomicUsize>,
    access: Arc<RuntimeAccess>,
    readiness_gate: Option<Arc<ReadinessGate>>,
    _readiness_gate_guard: Option<HostObjectGuard>,
}

#[derive(Clone, Copy)]
#[repr(u8)]
enum BindStage {
    PreparationNotStarted,
    CachedLoadInProgress,
    CachedLoadFailed,
    CachedLoadComplete,
    ReadinessInProgress,
    ReadinessFailed,
    ReadyBeforePeerResolution,
}

impl BindStage {
    fn from_byte(stage: u8) -> Self {
        match stage {
            1 => Self::CachedLoadInProgress,
            2 => Self::CachedLoadFailed,
            3 => Self::CachedLoadComplete,
            4 => Self::ReadinessInProgress,
            5 => Self::ReadinessFailed,
            6 => Self::ReadyBeforePeerResolution,
            _ => Self::PreparationNotStarted,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::PreparationNotStarted => "preparation_not_started",
            Self::CachedLoadInProgress => "cached_load_in_progress",
            Self::CachedLoadFailed => "cached_load_failed",
            Self::CachedLoadComplete => "cached_load_complete",
            Self::ReadinessInProgress => "readiness_in_progress",
            Self::ReadinessFailed => "readiness_failed",
            Self::ReadyBeforePeerResolution => "ready_before_peer_resolution",
        }
    }
}

impl RuntimeFixture {
    pub(crate) fn bind_stage(&self) -> &'static str {
        BindStage::from_byte(self.bind_stage.load(Ordering::SeqCst)).label()
    }

    pub(crate) fn readiness_attempts(&self) -> usize {
        self.readiness_attempts.load(Ordering::SeqCst)
    }

    pub(crate) fn module_ready(&self) -> bool {
        self.runtime.host().list().iter().any(|module| {
            module.manifest.bus_name.as_str() == tinychannels_bus::BUS_NAME
                && matches!(module.state, ModuleState::Ready | ModuleState::Serving)
        })
    }

    pub(crate) async fn wait_until_setup_is_held_before_ready(&self) {
        self.readiness_gate
            .as_ref()
            .expect("gated fixture has a readiness gate")
            .entered
            .acquire()
            .await
            .expect("readiness gate stays open")
            .forget();
    }

    pub(crate) fn release_ready(&self) {
        self.readiness_gate
            .as_ref()
            .expect("gated fixture has a readiness gate")
            .release
            .add_permits(1);
    }

    pub(crate) async fn wait_until_gate_callback_returns(&self) -> bool {
        let Some(gate) = self.readiness_gate.as_ref() else {
            return false;
        };
        match tokio::time::timeout(Duration::from_secs(5), gate.returned.acquire()).await {
            Ok(Ok(permit)) => {
                permit.forget();
                true
            }
            Ok(Err(_)) | Err(_) => false,
        }
    }

    pub(crate) async fn readiness_attempt_started(&self, previous: usize) -> bool {
        for _ in 0..10_000 {
            if self.readiness_attempts() > previous {
                return self.bind_stage() == "readiness_in_progress";
            }
            tokio::task::yield_now().await;
        }
        false
    }
}

struct ReadinessGate {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    returned: tokio::sync::Semaphore,
}

struct ReadinessGateHost(Arc<ReadinessGate>);

#[async_trait::async_trait]
impl Interface for ReadinessGateHost {
    fn name(&self) -> InterfaceName {
        InterfaceName::new(tinychannels_bus::HOST_BUS_NAME).expect("TinyChannels host bus name")
    }

    fn members(&self) -> Vec<MemberName> {
        vec![MemberName::new("WaitBeforeReady").expect("fixture readiness gate method")]
    }

    async fn call(
        &self,
        _: &MemberName,
        _: serde_json::Value,
    ) -> tinybus::Result<serde_json::Value> {
        self.0.entered.add_permits(1);
        self.0
            .release
            .acquire()
            .await
            .map_err(|_| Error::failed("fixture readiness gate closed"))?
            .forget();
        self.0.returned.add_permits(1);
        Ok(serde_json::Value::Null)
    }
}

struct RuntimeAccess {
    runtime: &'static ModuleRuntime,
    loaded: OnceCell<()>,
    record: &'static ReleaseRecord,
    release: CachedRelease<'static>,
    reporter: FailureReporter,
    bind_stage: Arc<AtomicU8>,
    readiness_attempts: Arc<AtomicUsize>,
    hold_ready: bool,
}

struct SharedRuntimeAccess(Arc<RuntimeAccess>);

impl ModuleAccess for SharedRuntimeAccess {
    fn find(&self, id: &str) -> Option<&'static ReleaseRecord> {
        self.0.find(id)
    }

    fn enabled(&self) -> bool {
        self.0.enabled()
    }

    fn reporter(&self) -> &FailureReporter {
        self.0.reporter()
    }

    fn ensure_loaded<'a>(&'a self, id: &'a str) -> AccessFuture<'a, Result<(), String>> {
        self.0.ensure_loaded(id)
    }

    fn prepare_attested<'a>(&'a self, id: &'a str) -> AccessFuture<'a, Result<(), String>> {
        self.0.prepare_attested(id)
    }

    fn proxy<'a>(
        &'a self,
        bus_name: &'a str,
        object_path: &'a str,
    ) -> AccessFuture<'a, Result<Proxy, ProxyFailure>> {
        self.0.proxy(bus_name, object_path)
    }
}

impl ModuleAccess for RuntimeAccess {
    fn find(&self, id: &str) -> Option<&'static ReleaseRecord> {
        (id == self.record.id).then_some(self.record)
    }

    fn enabled(&self) -> bool {
        true
    }

    fn reporter(&self) -> &FailureReporter {
        &self.reporter
    }

    fn ensure_loaded<'a>(&'a self, _: &'a str) -> AccessFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let loaded = self
                .loaded
                .get_or_try_init(|| async {
                    self.bind_stage
                        .store(BindStage::CachedLoadInProgress as u8, Ordering::SeqCst);
                    let result = self
                        .runtime
                        .host()
                        .load_github_release_cached(
                            &self.release,
                            if self.hold_ready {
                                serde_json::json!({ "hold_ready": true })
                            } else {
                                serde_json::json!({})
                            },
                        )
                        .map(|_| ());
                    if result.is_err() {
                        self.bind_stage
                            .store(BindStage::CachedLoadFailed as u8, Ordering::SeqCst);
                    }
                    result.map_err(|error| {
                        format!("the locally pinned native fixture could not load: {error}")
                    })
                })
                .await
                .map(|_| ());
            if loaded.is_ok() {
                self.bind_stage
                    .store(BindStage::CachedLoadComplete as u8, Ordering::SeqCst);
            }
            loaded
        })
    }

    fn prepare_attested<'a>(&'a self, id: &'a str) -> AccessFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.ensure_loaded(id).await?;
            self.readiness_attempts.fetch_add(1, Ordering::SeqCst);
            self.bind_stage
                .store(BindStage::ReadinessInProgress as u8, Ordering::SeqCst);
            self.runtime
                .host()
                .ensure_ready(self.record.bus_name)
                .await
                .map(|()| {
                    self.bind_stage
                        .store(BindStage::ReadyBeforePeerResolution as u8, Ordering::SeqCst)
                })
                .map_err(|_| {
                    self.bind_stage
                        .store(BindStage::ReadinessFailed as u8, Ordering::SeqCst);
                    "the locally pinned native fixture did not become ready".to_owned()
                })
        })
    }

    fn proxy<'a>(
        &'a self,
        bus_name: &'a str,
        object_path: &'a str,
    ) -> AccessFuture<'a, Result<Proxy, ProxyFailure>> {
        Box::pin(async move {
            self.runtime
                .connection()
                .proxy(bus_name, object_path, bus_name)
                .map_err(|_| ProxyFailure::Contract)
        })
    }
}

pub(crate) async fn gated_runtime_fixture() -> RuntimeFixture {
    runtime_fixture_with_readiness().await
}

async fn runtime_fixture_with_readiness() -> RuntimeFixture {
    let runtime = Box::leak(Box::new(
        ModuleRuntime::start(&RuntimeNames::default(), None)
            .expect("start one runtime for module and host callbacks"),
    ));
    let reports = Arc::new(Reports::default());
    let bind_stage = Arc::new(AtomicU8::new(0));
    let readiness_attempts = Arc::new(AtomicUsize::new(0));
    let readiness_gate = Arc::new(ReadinessGate {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
        returned: tokio::sync::Semaphore::new(0),
    });
    let readiness_gate_guard = runtime
        .serve_host_object(
            tinybus::BusName::new(tinychannels_bus::HOST_BUS_NAME)
                .expect("TinyChannels host bus name"),
            ObjectPath::new(READINESS_GATE_PATH).expect("fixture readiness gate path"),
            ReadinessGateHost(Arc::clone(&readiness_gate)),
        )
        .await
        .expect("serve fixture readiness gate before loading the module");
    let access = Arc::new(RuntimeAccess {
        runtime,
        loaded: OnceCell::new(),
        record: record(Pin::Correct),
        release: cached_release(),
        reporter: FailureReporter::new(reports.clone()),
        bind_stage: Arc::clone(&bind_stage),
        readiness_attempts: Arc::clone(&readiness_attempts),
        hold_ready: true,
    });
    let startup_client =
        TinyChannelsClient::new(ModuleClient::new(SharedRuntimeAccess(Arc::clone(&access))));
    let control = ModuleClient::new(SharedRuntimeAccess(Arc::clone(&access)))
        .with_timeout(Duration::from_secs(2));
    let client = TinyChannelsClient::new(control.clone());
    let fixture = RuntimeFixture {
        client,
        startup_client,
        control,
        runtime,
        reports,
        bind_stage,
        readiness_attempts,
        access,
        readiness_gate: Some(readiness_gate),
        _readiness_gate_guard: Some(readiness_gate_guard),
    };
    fixture
        .access
        .ensure_loaded(MODULE_ID)
        .await
        .expect("gated native fixture loads before waiting at the READY barrier");
    fixture.wait_until_setup_is_held_before_ready().await;
    fixture
}

/// The actual TinyChannels module cdylib, staged locally with both exact hashes.
pub(crate) async fn native_module_runtime_fixture() -> RuntimeFixture {
    let runtime = Box::leak(Box::new(
        ModuleRuntime::start(&RuntimeNames::default(), None)
            .expect("start one runtime for native module and host callbacks"),
    ));
    let reports = Arc::new(Reports::default());
    let bind_stage = Arc::new(AtomicU8::new(0));
    let readiness_attempts = Arc::new(AtomicUsize::new(0));
    let access = Arc::new(RuntimeAccess {
        runtime,
        loaded: OnceCell::new(),
        record: native_module_record(),
        release: cached_release_with_prefix(
            "TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST",
            env_prefixed("TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST", "RELEASE_URL"),
        ),
        reporter: FailureReporter::new(reports.clone()),
        bind_stage: Arc::clone(&bind_stage),
        readiness_attempts: Arc::clone(&readiness_attempts),
        hold_ready: false,
    });
    let control = ModuleClient::new(SharedRuntimeAccess(Arc::clone(&access)));
    RuntimeFixture {
        client: TinyChannelsClient::new(control.clone()),
        startup_client: TinyChannelsClient::new(control.clone()),
        control,
        runtime,
        reports,
        bind_stage,
        readiness_attempts,
        access,
        readiness_gate: None,
        _readiness_gate_guard: None,
    }
}

pub(crate) struct UnattestedFixture {
    pub(crate) client: TinyChannelsClient,
    pub(crate) calls: Arc<std::sync::atomic::AtomicUsize>,
    pub(crate) probe: Connection,
    _service: Connection,
    _broker_task: tokio::task::JoinHandle<tinybus::Result<()>>,
}

pub(crate) async fn unattested_fixture() -> UnattestedFixture {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Unattested(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl Interface for Unattested {
        fn name(&self) -> InterfaceName {
            InterfaceName::new(tinychannels_bus::BUS_NAME).expect("valid module name")
        }

        fn members(&self) -> Vec<MemberName> {
            ["ChannelController", "CheckChannelHealth", "FixtureReady"]
                .into_iter()
                .map(|method| MemberName::new(method).expect("valid method"))
                .collect()
        }

        fn requires_confidential(&self, member: &MemberName) -> bool {
            matches!(member.as_str(), "ChannelController" | "CheckChannelHealth")
        }

        async fn call(
            &self,
            member: &MemberName,
            _args: serde_json::Value,
        ) -> tinybus::Result<serde_json::Value> {
            if member.as_str() == "FixtureReady" {
                return Ok(serde_json::json!(true));
            }
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({"result":"discord_guilds","value":{"raw":[]}}))
        }
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let bus = MemoryBus::new();
    let broker = Broker::new();
    let broker_task = broker.spawn(bus.clone());
    let service = Connection::connect(bus.connect().await.expect("service transport"))
        .await
        .expect("service connection");
    service
        .serve_at(
            ObjectPath::new(tinychannels_bus::OBJECT_PATH).expect("valid object path"),
            Unattested(Arc::clone(&calls)),
        )
        .await
        .expect("serve unattested peer");
    service
        .request_name(tinychannels_bus::BUS_NAME)
        .await
        .expect("own untrusted well-known name");
    let client_connection = Connection::connect(bus.connect().await.expect("client transport"))
        .await
        .expect("client connection");
    let owner = client_connection
        .name_owner(tinychannels_bus::BUS_NAME)
        .await
        .expect("query peer owner")
        .expect("peer owns the module name");
    assert!(owner.is_unique());

    let access = UnattestedAccess {
        connection: client_connection.clone(),
        record: record(Pin::Correct),
        reporter: FailureReporter::new(Arc::new(Reports::default())),
    };
    UnattestedFixture {
        client: TinyChannelsClient::new(tinybus::module::ModuleClient::new(access)),
        calls,
        probe: client_connection,
        _service: service,
        _broker_task: broker_task,
    }
}

pub(crate) async fn assert_unattested_peer_live(probe: &Connection) -> tinybus::BusName {
    let owner = probe
        .name_owner(tinychannels_bus::BUS_NAME)
        .await
        .expect("query unattested peer owner")
        .expect("unattested peer remains connected");
    assert!(owner.is_unique());
    assert!(
        probe
            .attestation(owner.clone())
            .await
            .expect("public peer proof lookup")
            .is_none()
    );
    let readiness = probe
        .proxy(
            tinychannels_bus::BUS_NAME,
            tinychannels_bus::OBJECT_PATH,
            tinychannels_bus::BUS_NAME,
        )
        .expect("unattested readiness proxy")
        .call::<bool>("FixtureReady", ())
        .await
        .expect("content-free readiness probe");
    assert!(readiness);
    owner
}

struct UnattestedAccess {
    connection: Connection,
    record: &'static ReleaseRecord,
    reporter: FailureReporter,
}

impl ModuleAccess for UnattestedAccess {
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
    ) -> AccessFuture<'a, Result<Proxy, ProxyFailure>> {
        Box::pin(async move {
            self.connection
                .proxy(bus_name, object_path, bus_name)
                .map_err(|_| ProxyFailure::Contract)
        })
    }
}

pub(crate) async fn calls(probe: &Connection) -> usize {
    let proxy = probe
        .proxy(
            tinychannels_bus::BUS_NAME,
            tinychannels_bus::OBJECT_PATH,
            tinychannels_bus::BUS_NAME,
        )
        .expect("fixture probe proxy");
    proxy
        .call("FixtureCallCount", ())
        .await
        .expect("read fixture call count")
}

pub(crate) fn record(pin: Pin) -> &'static ReleaseRecord {
    let slot = match pin {
        Pin::Correct => &RECORD,
        Pin::WrongArchive => &WRONG_ARCHIVE_RECORD,
        Pin::WrongLibrary => &WRONG_LIBRARY_RECORD,
    };
    slot.get_or_init(|| {
        let host_key = Box::leak(
            tinybus::module::platform::host_candidates()
                .into_iter()
                .next()
                .expect("test platform has a TinyBus module key")
                .into_boxed_str(),
        );
        let archive = env("TINYCHANNELS_CLIENT_TEST_ARCHIVE_NAME");
        let archive_sha = match pin {
            Pin::WrongArchive => Box::leak("0".repeat(64).into_boxed_str()),
            _ => env("TINYCHANNELS_CLIENT_TEST_ARCHIVE_SHA256"),
        };
        let library_sha = match pin {
            Pin::WrongLibrary => Box::leak("f".repeat(64).into_boxed_str()),
            _ => env("TINYCHANNELS_CLIENT_TEST_LIBRARY_SHA256"),
        };
        let assets = Box::leak(
            vec![ReleaseAsset {
                host_key,
                archive,
                sha256: archive_sha,
                library_sha256: Some(library_sha),
            }]
            .into_boxed_slice(),
        );
        ReleaseRecord {
            id: MODULE_ID,
            bus_name: tinychannels_bus::BUS_NAME,
            object_path: tinychannels_bus::OBJECT_PATH,
            version: "0.2.0",
            release_url: RELEASE_URL,
            assets,
        }
    })
}

#[derive(Clone, Copy)]
pub(crate) enum Pin {
    Correct,
    WrongArchive,
    WrongLibrary,
}

fn cached_release() -> CachedRelease<'static> {
    cached_release_with_prefix("TINYCHANNELS_CLIENT_TEST", RELEASE_URL)
}

fn cached_release_with_prefix(prefix: &str, release_url: &'static str) -> CachedRelease<'static> {
    CachedRelease {
        release_url,
        asset_name: env_prefixed(prefix, "ARCHIVE_NAME"),
        expected_sha256: Some(env_prefixed(prefix, "ARCHIVE_SHA256")),
        expected_library_sha256: Some(env_prefixed(prefix, "LIBRARY_SHA256")),
        cache_dir: Path::new(env_prefixed(prefix, "CACHE")),
        allow_download: false,
    }
}

fn native_module_record() -> &'static ReleaseRecord {
    NATIVE_MODULE_RECORD.get_or_init(|| {
        let host_key = Box::leak(
            tinybus::module::platform::host_candidates()
                .into_iter()
                .next()
                .expect("test platform has a TinyBus module key")
                .into_boxed_str(),
        );
        let archive = env_prefixed("TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST", "ARCHIVE_NAME");
        let archive_sha = env_prefixed("TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST", "ARCHIVE_SHA256");
        let library_sha = env_prefixed("TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST", "LIBRARY_SHA256");
        let assets = Box::leak(
            vec![ReleaseAsset {
                host_key,
                archive,
                sha256: archive_sha,
                library_sha256: Some(library_sha),
            }]
            .into_boxed_slice(),
        );
        ReleaseRecord {
            id: MODULE_ID,
            bus_name: tinychannels_bus::BUS_NAME,
            object_path: tinychannels_bus::OBJECT_PATH,
            version: "0.2.0",
            release_url: env_prefixed("TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST", "RELEASE_URL"),
            assets,
        }
    })
}

fn env(name: &str) -> &'static str {
    Box::leak(
        std::env::var(name)
            .unwrap_or_else(|_| panic!("{name} must identify the staged local fixture"))
            .into_boxed_str(),
    )
}

fn env_prefixed(prefix: &str, suffix: &str) -> &'static str {
    let name = format!("{prefix}_{suffix}");
    Box::leak(
        std::env::var(&name)
            .unwrap_or_else(|_| panic!("{name} must identify the staged local fixture"))
            .into_boxed_str(),
    )
}

pub(crate) fn assert_unavailable(error: ModuleCallFailure) {
    assert_eq!(error.kind, ModuleCallError::Unavailable);
}
