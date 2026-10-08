use std::collections::HashSet;
use std::sync::{
    Arc, Mutex as StdMutex, MutexGuard, PoisonError, Weak,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use easytier_proto::{
    api::manage::NetworkConfig,
    rpc_types::controller::BaseController,
    web::{
        DeviceOsInfo, GetFeatureRequest, GetFeatureResponse, HeartbeatRequest, HeartbeatResponse,
        ReportNetworkConfigRequest, WebServerServiceClientFactory,
    },
};
use tokio::{sync::Mutex, task::JoinSet};
use tokio_util::task::AbortOnDropHandle;
use url::Url;

use crate::{
    connectivity::protocol::raw::TunnelDialer,
    foundation::time,
    instance::{CoreInstance, CoreInstanceHost, manager::InstanceFactory},
    rpc::{bidirect::BidirectRpcManager, service_registry::ServiceRegistry},
    tunnel::{Tunnel, web_security},
};

#[cfg(not(feature = "management"))]
use super::register_web_client_rpc;
use super::{
    ConfigFileStorage, DaemonGuard, InstanceManager, InstanceMutationHooks, config_server_client,
    config_server_status,
};
#[cfg(feature = "management")]
use super::{LoggerControl, register_management_rpc};

const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const MAX_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
// Keep retry ownership in this loop when transport or protocol handshakes stall.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const FEATURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const DEFAULT_HEARTBEAT_INTERVAL_MS: u32 = 3_500;
const DEFAULT_HEARTBEAT_TIMEOUT_MS: u32 = 15_000;
const MIN_HEARTBEAT_INTERVAL_MS: u32 = 1_000;
const MAX_HEARTBEAT_INTERVAL_MS: u32 = 60_000;
const MIN_HEARTBEAT_TIMEOUT_MS: u32 = 5_000;
const MAX_HEARTBEAT_TIMEOUT_MS: u32 = 120_000;
const MIN_HEARTBEAT_TIMEOUT_MARGIN_MS: u32 = 5_000;

/// Recover from a poisoned `StdMutex` instead of silently dropping updates.
/// Poison means a prior holder paniced while holding the lock; the inner state
/// is still the best available process truth for revision / RPC bookkeeping.
fn lock_mutex<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned: PoisonError<MutexGuard<'_, T>>| {
            tracing::error!(
                "config-server StdMutex poisoned; recovering inner state so reports keep working"
            );
            poisoned.into_inner()
        })
}

fn next_backoff(current: std::time::Duration) -> std::time::Duration {
    current
        .checked_mul(2)
        .unwrap_or(MAX_RETRY_INTERVAL)
        .min(MAX_RETRY_INTERVAL)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeartbeatPolicy {
    interval: std::time::Duration,
    timeout_ms: i32,
}

impl Default for HeartbeatPolicy {
    fn default() -> Self {
        Self {
            interval: std::time::Duration::from_millis(DEFAULT_HEARTBEAT_INTERVAL_MS.into()),
            timeout_ms: DEFAULT_HEARTBEAT_TIMEOUT_MS as i32,
        }
    }
}

impl HeartbeatPolicy {
    fn from_response(response: &HeartbeatResponse) -> (Self, bool) {
        let requested_interval = response
            .heartbeat_interval_ms
            .unwrap_or(DEFAULT_HEARTBEAT_INTERVAL_MS);
        let requested_timeout = response
            .heartbeat_timeout_ms
            .unwrap_or(DEFAULT_HEARTBEAT_TIMEOUT_MS);
        let interval_ms =
            requested_interval.clamp(MIN_HEARTBEAT_INTERVAL_MS, MAX_HEARTBEAT_INTERVAL_MS);
        let timeout_ms = requested_timeout
            .clamp(MIN_HEARTBEAT_TIMEOUT_MS, MAX_HEARTBEAT_TIMEOUT_MS)
            .max(interval_ms.saturating_add(MIN_HEARTBEAT_TIMEOUT_MARGIN_MS));
        (
            Self {
                interval: std::time::Duration::from_millis(interval_ms.into()),
                timeout_ms: timeout_ms as i32,
            },
            interval_ms != requested_interval || timeout_ms != requested_timeout,
        )
    }

    fn controller(self) -> BaseController {
        BaseController {
            timeout_ms: self.timeout_ms,
            ..Default::default()
        }
    }

    fn remaining_interval(self, elapsed: std::time::Duration) -> Option<std::time::Duration> {
        self.interval
            .checked_sub(elapsed)
            .filter(|delay| !delay.is_zero())
    }
}

async fn connect_config_server(
    connector: &dyn TunnelDialer,
    timeout: std::time::Duration,
) -> anyhow::Result<Box<dyn Tunnel>> {
    time::timeout(timeout, connector.connect())
        .await
        .map_err(|_| anyhow::anyhow!("config-server connection timed out after {timeout:?}"))?
}

/// Normalized config-server endpoint and authentication token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigServerEndpoint {
    connect_url: Url,
    token: String,
}

impl ConfigServerEndpoint {
    pub fn parse(input: &str, supports_scheme: impl FnOnce(&Url) -> bool) -> anyhow::Result<Self> {
        let endpoint = Url::parse(input)
            .map_err(|error| anyhow::anyhow!("failed to parse config server URL: {error}"))?;
        if !supports_scheme(&endpoint) {
            anyhow::bail!("unsupported config server scheme: {}", endpoint.scheme());
        }

        let token = endpoint
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .map(|segment| percent_encoding::percent_decode_str(segment).decode_utf8())
            .transpose()
            .map_err(|error| anyhow::anyhow!("failed to decode config server token: {error}"))?
            .map(|token| token.to_string())
            .unwrap_or_default();
        if token.is_empty() {
            anyhow::bail!("empty token");
        }

        let mut connect_url = endpoint;
        if !matches!(connect_url.scheme(), "ws" | "wss") {
            connect_url.set_path("");
        }
        Ok(Self { connect_url, token })
    }

    pub fn connect_url(&self) -> &Url {
        &self.connect_url
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

pub struct WebClientConfig {
    pub token: String,
    pub machine_id: uuid::Uuid,
    pub hostname: String,
    pub device_os: DeviceOsInfo,
    pub easytier_version: String,
    pub secure_mode: bool,
}

#[async_trait]
pub(crate) trait WebClientBackend: Send + Sync + 'static {
    fn register(&self, registry: &ServiceRegistry);

    async fn instance_ids(&self) -> anyhow::Result<Vec<uuid::Uuid>>;

    fn failed_instance_ids(&self) -> Vec<uuid::Uuid>;

    fn user_disabled_web_instance_ids(&self) -> Vec<uuid::Uuid> {
        Vec::new()
    }

    /// Whether this backend can accurately report intentional local stops.
    ///
    /// Fail-safe default: a backend that cannot enumerate user-disabled web-owned
    /// instances keeps returning `false`, otherwise the config server reads the
    /// always-empty list as "nothing is disabled" and auto-runs instances the
    /// user stopped on purpose. Backends that do implement
    /// [`Self::user_disabled_web_instance_ids`] must opt in explicitly.
    fn support_user_disabled_instances(&self) -> bool {
        false
    }

    fn instance_state_generation(&self) -> usize {
        0
    }

    async fn wait_for_instance_state_change(&self, _generation: usize) -> usize {
        std::future::pending().await
    }
}

struct NativeWebClientBackend<F>
where
    F: InstanceFactory,
{
    instances: Arc<InstanceManager<F>>,
    hooks: Arc<dyn InstanceMutationHooks>,
    storage: Arc<dyn ConfigFileStorage>,
    #[cfg(feature = "management")]
    logger: Arc<dyn LoggerControl>,
}

#[async_trait]
impl<F, H> WebClientBackend for NativeWebClientBackend<F>
where
    F: InstanceFactory<Instance = CoreInstance<H>, CreateContext = ()>,
    F::Error: std::fmt::Debug + std::fmt::Display + Send + Sync + 'static,
    H: CoreInstanceHost,
{
    fn register(&self, registry: &ServiceRegistry) {
        #[cfg(feature = "management")]
        register_management_rpc(
            self.instances.clone(),
            registry,
            self.hooks.clone(),
            self.storage.clone(),
            self.logger.clone(),
        );
        #[cfg(not(feature = "management"))]
        register_web_client_rpc(
            self.instances.clone(),
            registry,
            self.hooks.clone(),
            self.storage.clone(),
        );
    }

    async fn instance_ids(&self) -> anyhow::Result<Vec<uuid::Uuid>> {
        Ok(self.instances.instance_ids())
    }

    fn failed_instance_ids(&self) -> Vec<uuid::Uuid> {
        self.instances.failed_instance_ids()
    }

    fn user_disabled_web_instance_ids(&self) -> Vec<uuid::Uuid> {
        self.instances.user_disabled_web_instance_ids()
    }

    fn support_user_disabled_instances(&self) -> bool {
        // The instance manager tracks intentional local stops, so the heartbeat
        // may report the disabled set as authoritative.
        true
    }

    fn instance_state_generation(&self) -> usize {
        self.instances.instance_state_generation()
    }

    async fn wait_for_instance_state_change(&self, generation: usize) -> usize {
        self.instances
            .wait_for_instance_state_change(generation)
            .await
    }
}

struct WebClientController {
    config: WebClientConfig,
    backend: Arc<dyn WebClientBackend>,
    runtime_id: uuid::Uuid,
    managed_config_revision: StdMutex<Option<String>>,
    active_rpc: StdMutex<Option<Arc<BidirectRpcManager>>>,
    /// Bumped when a local report updates the revision cache so in-flight
    /// heartbeats cannot clobber a fresher CAS result.
    revision_generation: std::sync::atomic::AtomicU64,
    /// Client-minted revision of the last unconfirmed local report per instance,
    /// so retrying the same edit reuses it and stays idempotent on the console.
    /// Keyed by instance id so concurrent multi-instance reports do not clobber
    /// each other's pending revision (which previously caused false
    /// `RevisionConflict`).
    pending_report: StdMutex<std::collections::HashMap<String, PendingReport>>,
}

/// A `config_revision` minted for a local report that has no definitive answer yet.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingReport {
    instance_id: String,
    expected_revision: String,
    config_revision: String,
}

impl WebClientController {
    fn cached_revision(&self) -> Option<String> {
        lock_mutex(&self.managed_config_revision).clone()
    }

    /// Cache a revision observed by a local report (applied or conflict) and bump
    /// the generation so an in-flight heartbeat cannot write a staler value.
    fn store_local_revision(&self, revision: Option<String>) {
        *lock_mutex(&self.managed_config_revision) = revision;
        self.revision_generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Cache the revision carried by a heartbeat, unless a local report landed
    /// after that heartbeat sampled the generation.
    fn store_heartbeat_revision(&self, sampled_generation: u64, revision: Option<String>) {
        let mut guard = lock_mutex(&self.managed_config_revision);
        if self.revision_generation.load(Ordering::Acquire) == sampled_generation {
            *guard = revision;
        }
    }

    /// Reuse the revision of an unresolved report for the same instance and
    /// expected revision; otherwise mint a new one. The console treats an
    /// identical target revision as already applied, so a retry after a lost
    /// response must not look like a concurrent edit.
    fn revision_for_report(&self, instance_id: &str, expected_revision: &str) -> String {
        let mut guard = lock_mutex(&self.pending_report);
        if let Some(pending) = guard.get(instance_id)
            && pending.expected_revision == expected_revision
        {
            return pending.config_revision.clone();
        }
        let config_revision = uuid::Uuid::new_v4().to_string();
        guard.insert(
            instance_id.to_owned(),
            PendingReport {
                instance_id: instance_id.to_owned(),
                expected_revision: expected_revision.to_owned(),
                config_revision: config_revision.clone(),
            },
        );
        config_revision
    }

    /// Drop the pending report after a definitive answer. Transient failures keep
    /// it so the next attempt reuses the same revision.
    fn clear_pending_report(&self, instance_id: &str) {
        lock_mutex(&self.pending_report).remove(instance_id);
    }
}

/// Error from reporting a client-edited web-owned config to the console.
pub use config_server_client::ReportNetworkConfigError;

/// Portable config-server client. Hosts only supply identity and adapters.
pub struct WebClient<F> {
    controller: Arc<WebClientController>,
    report_client: Arc<dyn config_server_client::ConfigServerReportClient>,
    _tasks: AbortOnDropHandle<()>,
    _manager_guard: Option<DaemonGuard>,
    connected: Arc<AtomicBool>,
    _factory: std::marker::PhantomData<F>,
    // Struct fields drop in declaration order: clear observational status
    // first, then the report registry, so teardown never advertises
    // enabled-without-report.
    _clear_status_on_drop: ClearConfigServerStatusOnDrop,
    _clear_report_client_on_drop: ClearReportClientOnDrop,
}

impl<F, H> WebClient<F>
where
    F: InstanceFactory<Instance = CoreInstance<H>, CreateContext = ()>,
    F::Error: std::fmt::Debug + std::fmt::Display + Send + Sync + 'static,
    H: CoreInstanceHost,
{
    pub fn new<T: TunnelDialer + 'static>(
        connector: T,
        config: WebClientConfig,
        instances: Arc<InstanceManager<F>>,
        hooks: Arc<dyn InstanceMutationHooks>,
        storage: Arc<dyn ConfigFileStorage>,
        #[cfg(feature = "management")] logger: Arc<dyn LoggerControl>,
    ) -> Self {
        let manager_guard = instances.register_daemon();
        let backend = Arc::new(NativeWebClientBackend {
            instances,
            hooks,
            storage,
            #[cfg(feature = "management")]
            logger,
        });
        Self::start(connector, config, backend, Some(manager_guard))
    }
}

#[cfg(target_os = "wasi")]
impl WebClient<()> {
    pub(crate) fn with_backend<T: TunnelDialer + 'static>(
        connector: T,
        config: WebClientConfig,
        backend: Arc<dyn WebClientBackend>,
    ) -> Self {
        Self::start(connector, config, backend, None)
    }
}

impl<F> WebClient<F> {
    fn start<T: TunnelDialer + 'static>(
        connector: T,
        config: WebClientConfig,
        backend: Arc<dyn WebClientBackend>,
        manager_guard: Option<DaemonGuard>,
    ) -> Self {
        let controller = Arc::new(WebClientController {
            config,
            backend,
            runtime_id: uuid::Uuid::new_v4(),
            managed_config_revision: StdMutex::new(None),
            active_rpc: StdMutex::new(None),
            revision_generation: std::sync::atomic::AtomicU64::new(0),
            pending_report: StdMutex::new(std::collections::HashMap::new()),
        });
        let connected = Arc::new(AtomicBool::new(false));
        // Install the report client before advertising enabled status so
        // GetConfigServerStatus / ReportManagedNetworkConfig never observe
        // "enabled but not reportable".
        let report_client: Arc<dyn config_server_client::ConfigServerReportClient> =
            Arc::new(WebClientReportFacade {
                controller: controller.clone(),
                connected: connected.clone(),
            });
        config_server_client::install_config_server_report_client(Some(report_client.clone()));
        config_server_status::mark_enabled();
        config_server_status::set_endpoint_url(&connector.remote_url());
        // Resolve the management endpoint early so underlay excludes can pin it
        // before a TUN default route is installed. Desktop DNS binds to the
        // physical default iface once a TUN exclude is registered; this eager
        // resolve still shrinks the pre-TUN race.
        #[cfg(not(any(target_os = "wasi", target_arch = "wasm32")))]
        {
            drop(tokio::spawn(async {
                let _ = config_server_status::underlay_exclude_candidate_ips().await;
            }));
        }
        let tasks = AbortOnDropHandle::new(tokio::spawn(web_client_routine(
            controller.clone(),
            connected.clone(),
            Box::new(connector),
        )));

        Self {
            controller,
            report_client: report_client.clone(),
            _tasks: tasks,
            _manager_guard: manager_guard,
            connected,
            _factory: std::marker::PhantomData,
            _clear_status_on_drop: ClearConfigServerStatusOnDrop,
            _clear_report_client_on_drop: ClearReportClientOnDrop { report_client },
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub fn managed_config_revision(&self) -> Option<String> {
        self.controller.cached_revision()
    }

    /// Push a client-edited web-owned NetworkConfig to the console with CAS.
    pub async fn report_network_config(
        &self,
        config: NetworkConfig,
    ) -> Result<(), ReportNetworkConfigError> {
        self.report_client.report_network_config(config).await
    }
}

struct WebClientReportFacade {
    controller: Arc<WebClientController>,
    connected: Arc<AtomicBool>,
}

#[async_trait]
impl config_server_client::ConfigServerReportClient for WebClientReportFacade {
    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    async fn report_network_config(
        &self,
        config: NetworkConfig,
    ) -> Result<(), ReportNetworkConfigError> {
        if !self.is_connected() {
            return Err(ReportNetworkConfigError::NotConnected);
        }
        let rpc = lock_mutex(&self.controller.active_rpc)
            .clone()
            .ok_or(ReportNetworkConfigError::NotConnected)?;
        let instance_id = config.instance_id().to_owned();
        let expected = self.controller.cached_revision().unwrap_or_default();
        // Retrying the same edit must reuse the same target revision: the console
        // answers an already-applied revision with `AlreadyApplied`, while a fresh
        // revision after a lost response would surface as a bogus conflict.
        let config_revision = self.controller.revision_for_report(&instance_id, &expected);
        let client = rpc
            .rpc_client()
            .scoped_client::<WebServerServiceClientFactory<BaseController>>(1, 1, String::new());
        let response = client
            .report_network_config(
                BaseController::default(),
                ReportNetworkConfigRequest {
                    machine_id: Some(self.controller.config.machine_id.into()),
                    user_token: self.controller.config.token.clone(),
                    config: Some(config),
                    expected_config_revision: expected,
                    config_revision,
                },
            )
            .await?;
        if response.ok {
            self.controller.clear_pending_report(&instance_id);
            self.controller.store_local_revision(
                Some(response.applied_config_revision).filter(|value| !value.is_empty()),
            );
            return Ok(());
        }
        let current = (!response.current_config_revision.is_empty())
            .then(|| response.current_config_revision.clone());
        match response.error_code.as_str() {
            // The console owns the revision; refresh our cache so the caller can
            // re-read the current config and retry against it.
            "managed_config_revision_conflict" => {
                self.controller.clear_pending_report(&instance_id);
                self.controller.store_local_revision(current.clone());
                Err(ReportNetworkConfigError::RevisionConflict { current })
            }
            "managed_config_ownership_conflict" => {
                self.controller.clear_pending_report(&instance_id);
                self.controller.store_local_revision(current);
                Err(ReportNetworkConfigError::OwnershipConflict)
            }
            "not_authorized" => {
                self.controller.clear_pending_report(&instance_id);
                Err(ReportNetworkConfigError::NotAuthorized)
            }
            other => {
                self.controller.clear_pending_report(&instance_id);
                Err(ReportNetworkConfigError::Invalid(if other.is_empty() {
                    "unknown report failure".to_owned()
                } else {
                    other.to_owned()
                }))
            }
        }
    }
}

struct ClearConfigServerStatusOnDrop;

impl Drop for ClearConfigServerStatusOnDrop {
    fn drop(&mut self) {
        config_server_status::clear();
    }
}

struct ClearReportClientOnDrop {
    report_client: Arc<dyn config_server_client::ConfigServerReportClient>,
}

impl Drop for ClearReportClientOnDrop {
    fn drop(&mut self) {
        config_server_client::clear_config_server_report_client(&self.report_client);
    }
}

async fn web_client_routine(
    controller: Arc<WebClientController>,
    connected: Arc<AtomicBool>,
    connector: Box<dyn TunnelDialer>,
) {
    let mut backoff = RETRY_INTERVAL;
    loop {
        let connection = match connect_config_server(connector.as_ref(), CONNECT_TIMEOUT).await {
            // Do NOT reset `backoff` here. A plain dial can keep succeeding while
            // the secure upgrade / session negotiation fails, and resetting on
            // dial pinned those retries at RETRY_INTERVAL (1s) forever. Only a
            // genuinely established session resets the backoff (see below).
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(
                    %error,
                    retry_in_ms = backoff.as_millis(),
                    "failed to connect to config server; retrying"
                );
                config_server_status::mark_error(error.to_string());
                connected.store(false, Ordering::Release);
                time::sleep(backoff).await;
                backoff = next_backoff(backoff);
                continue;
            }
        };

        // Tunnel is up, but the session is not ready until feature negotiation /
        // optional secure upgrade succeed. Do not report connected yet — otherwise
        // the UI flashes "已连接" before GetFeature or the secure handshake fails.
        config_server_status::record_tunnel_remote(connection.info().as_ref());
        config_server_status::clear_last_error();
        connected.store(false, Ordering::Release);
        tracing::info!(?connection, "dialed config server; negotiating session");
        let mut session = WebClientSession::new(connection, controller.clone());
        let support_encryption = match time::timeout(FEATURE_TIMEOUT, session.get_feature()).await {
            Ok(Ok(feature)) => feature.support_encryption,
            Ok(Err(error)) => {
                tracing::warn!(%error, "GetFeature RPC failed; using legacy tunnel");
                false
            }
            Err(_) => {
                tracing::warn!("GetFeature RPC timed out; using legacy tunnel");
                false
            }
        };

        if support_encryption && web_security::web_secure_tunnel_supported() {
            drop(session);
            let connection = match connect_config_server(connector.as_ref(), CONNECT_TIMEOUT).await
            {
                Ok(connection) => connection,
                Err(error) => {
                    connected.store(false, Ordering::Release);
                    config_server_status::mark_error(error.to_string());
                    tracing::warn!(
                        %error,
                        retry_in_ms = backoff.as_millis(),
                        "failed to reconnect secure config-server tunnel"
                    );
                    time::sleep(backoff).await;
                    backoff = next_backoff(backoff);
                    continue;
                }
            };
            config_server_status::record_tunnel_remote(connection.info().as_ref());
            let connection = match web_security::upgrade_client_tunnel(connection).await {
                Ok(connection) => {
                    backoff = RETRY_INTERVAL;
                    connection
                }
                Err(error) => {
                    connected.store(false, Ordering::Release);
                    config_server_status::mark_error(error.to_string());
                    tracing::warn!(
                        %error,
                        retry_in_ms = backoff.as_millis(),
                        "config-server secure handshake failed"
                    );
                    time::sleep(backoff).await;
                    backoff = next_backoff(backoff);
                    continue;
                }
            };
            config_server_status::record_tunnel_remote(connection.info().as_ref());
            let mut session = WebClientSession::new(connection, controller.clone());
            connected.store(true, Ordering::Release);
            config_server_status::mark_connected();
            tracing::info!("connected to config server (secure tunnel)");
            session.start_heartbeat().await;
            session.wait().await;
            connected.store(false, Ordering::Release);
            config_server_status::mark_disconnected();
            // Successful session ended (server close / network drop). Back off
            // before hot-reconnecting to avoid a reconnect storm.
            tracing::info!(
                retry_in_ms = RETRY_INTERVAL.as_millis(),
                "config-server session ended; reconnecting after backoff"
            );
            time::sleep(RETRY_INTERVAL).await;
            continue;
        }

        if support_encryption {
            if controller.config.secure_mode {
                connected.store(false, Ordering::Release);
                config_server_status::mark_error(
                    "secure mode requires web secure-tunnel support in the local build",
                );
                tracing::warn!("secure mode requires web secure-tunnel support in the local build");
                time::sleep(backoff).await;
                backoff = next_backoff(backoff);
                continue;
            }
            tracing::warn!(
                "server supports encryption but the local build is using a legacy tunnel"
            );
        }
        if controller.config.secure_mode {
            connected.store(false, Ordering::Release);
            config_server_status::mark_error(
                "secure mode requires config-server encryption support",
            );
            tracing::warn!("secure mode requires config-server encryption support");
            time::sleep(backoff).await;
            backoff = next_backoff(backoff);
            continue;
        }

        connected.store(true, Ordering::Release);
        config_server_status::mark_connected();
        // Session is up: this is the point where exponential backoff resets.
        backoff = RETRY_INTERVAL;
        tracing::info!("connected to config server");
        session.start_heartbeat().await;
        session.wait().await;
        connected.store(false, Ordering::Release);
        config_server_status::mark_disconnected();
        tracing::info!(
            retry_in_ms = RETRY_INTERVAL.as_millis(),
            "config-server session ended; reconnecting after backoff"
        );
        time::sleep(RETRY_INTERVAL).await;
    }
}

struct WebClientSession {
    rpc: Arc<BidirectRpcManager>,
    controller: Arc<WebClientController>,
    heartbeat_started: AtomicBool,
    tasks: Mutex<JoinSet<()>>,
}

fn running_instances_for_heartbeat(
    instance_ids: Vec<uuid::Uuid>,
    failed_instance_ids: &[uuid::Uuid],
) -> Vec<uuid::Uuid> {
    let failed_instance_ids: HashSet<_> = failed_instance_ids.iter().copied().collect();
    instance_ids
        .into_iter()
        .filter(|instance_id| !failed_instance_ids.contains(instance_id))
        .collect()
}

fn build_heartbeat_request(
    config: &WebClientConfig,
    runtime_id: uuid::Uuid,
    running_network_instances: Vec<uuid::Uuid>,
    failed_network_instances: Vec<uuid::Uuid>,
    disabled_network_instances: Vec<uuid::Uuid>,
    support_user_disabled_instances: bool,
) -> HeartbeatRequest {
    HeartbeatRequest {
        machine_id: Some(config.machine_id.into()),
        inst_id: Some(runtime_id.into()),
        user_token: config.token.clone(),
        easytier_version: config.easytier_version.clone(),
        hostname: config.hostname.clone(),
        report_time: chrono::Local::now().to_rfc3339(),
        device_os: Some(config.device_os.clone()),
        support_config_source: true,
        running_network_instances: running_network_instances
            .into_iter()
            .map(Into::into)
            .collect(),
        failed_network_instances: failed_network_instances
            .into_iter()
            .map(Into::into)
            .collect(),
        support_heartbeat_policy: true,
        support_user_disabled_instances,
        disabled_network_instances: disabled_network_instances
            .into_iter()
            .map(Into::into)
            .collect(),
        magic_dns_os_wired: crate::gateway::magic_dns::get_magic_dns_os_wired(),
    }
}

async fn wait_for_next_heartbeat(
    backend: &dyn WebClientBackend,
    observed_generation: usize,
    policy: HeartbeatPolicy,
    elapsed: std::time::Duration,
) {
    let Some(delay) = policy.remaining_interval(elapsed) else {
        return;
    };
    tokio::select! {
        _ = time::sleep(delay) => {}
        _ = backend.wait_for_instance_state_change(observed_generation) => {}
    }
}

impl WebClientSession {
    fn new(tunnel: Box<dyn Tunnel>, controller: Arc<WebClientController>) -> Self {
        let rpc = Arc::new(BidirectRpcManager::new());
        rpc.run_with_tunnel(tunnel);
        controller.backend.register(rpc.rpc_server().registry());
        *lock_mutex(&controller.active_rpc) = Some(rpc.clone());
        Self {
            rpc,
            controller,
            heartbeat_started: AtomicBool::new(false),
            tasks: Mutex::new(JoinSet::new()),
        }
    }

    fn clear_active_rpc_if_current(&self) {
        let mut guard = lock_mutex(&self.controller.active_rpc);
        if guard
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, &self.rpc))
        {
            *guard = None;
        }
    }

    pub async fn start_heartbeat(&self) {
        if self.heartbeat_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut tasks = self.tasks.lock().await;
        Self::heartbeat_routine(&self.rpc, Arc::downgrade(&self.controller), &mut tasks);
    }

    fn heartbeat_routine(
        rpc: &BidirectRpcManager,
        controller: Weak<WebClientController>,
        tasks: &mut JoinSet<()>,
    ) {
        let controller = controller.upgrade().expect("web client controller");
        let controller = Arc::downgrade(&controller);
        let client = rpc
            .rpc_client()
            .scoped_client::<WebServerServiceClientFactory<BaseController>>(1, 1, String::new());

        tasks.spawn(async move {
            let mut heartbeat_policy = HeartbeatPolicy::default();
            loop {
                let heartbeat_started_at = std::time::Instant::now();
                let Some(controller) = controller.upgrade() else {
                    break;
                };
                let revision_generation = controller.revision_generation.load(Ordering::Acquire);
                let observed_generation = controller.backend.instance_state_generation();
                let failed_network_instances = controller.backend.failed_instance_ids();
                let disabled_network_instances =
                    controller.backend.user_disabled_web_instance_ids();
                let support_user_disabled_instances =
                    controller.backend.support_user_disabled_instances();
                let running_network_instances = match controller.backend.instance_ids().await {
                    Ok(instance_ids) => {
                        running_instances_for_heartbeat(instance_ids, &failed_network_instances)
                    }
                    Err(error) => {
                        tracing::error!(%error, "failed to list config-server instances");
                        break;
                    }
                };
                let request = build_heartbeat_request(
                    &controller.config,
                    controller.runtime_id,
                    running_network_instances,
                    failed_network_instances,
                    disabled_network_instances,
                    support_user_disabled_instances,
                );

                match client
                    .heartbeat(heartbeat_policy.controller(), request)
                    .await
                {
                    Ok(response) => {
                        tracing::debug!(?response, "config-server heartbeat response");
                        // Skip stale heartbeats that raced a successful local report.
                        controller.store_heartbeat_revision(
                            revision_generation,
                            response.managed_config_revision.clone(),
                        );
                        let (next_policy, adjusted) = HeartbeatPolicy::from_response(&response);
                        if adjusted {
                            tracing::warn!(
                                requested_interval_ms = ?response.heartbeat_interval_ms,
                                requested_timeout_ms = ?response.heartbeat_timeout_ms,
                                applied_interval_ms = next_policy.interval.as_millis(),
                                applied_timeout_ms = next_policy.timeout_ms,
                                "config-server heartbeat policy was outside safe bounds"
                            );
                        }
                        heartbeat_policy = next_policy;
                        wait_for_next_heartbeat(
                            controller.backend.as_ref(),
                            observed_generation,
                            heartbeat_policy,
                            heartbeat_started_at.elapsed(),
                        )
                        .await;
                    }
                    Err(error) => {
                        tracing::error!(?error, "config-server heartbeat failed");
                        break;
                    }
                }
            }
        });
    }

    async fn wait_routines(&self) {
        self.tasks.lock().await.join_next().await;
        self.tasks.lock().await.abort_all();
    }

    async fn wait(&mut self) {
        tokio::select! {
            _ = self.rpc.wait() => {}
            _ = self.wait_routines() => {}
        }
        self.clear_active_rpc_if_current();
    }

    async fn get_feature(
        &self,
    ) -> Result<GetFeatureResponse, easytier_proto::rpc_types::error::Error> {
        let client = self
            .rpc
            .rpc_client()
            .scoped_client::<WebServerServiceClientFactory<BaseController>>(1, 1, String::new());
        client
            .get_feature(BaseController::default(), GetFeatureRequest {})
            .await
    }
}

impl Drop for WebClientSession {
    fn drop(&mut self) {
        self.clear_active_rpc_if_current();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::pending,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;

    use super::*;
    use crate::tunnel::ring::create_ring_tunnel_pair;

    struct StalledThenReadyDialer {
        attempts: AtomicUsize,
    }

    struct ImmediateStateChangeBackend;

    #[async_trait]
    impl WebClientBackend for ImmediateStateChangeBackend {
        fn register(&self, _registry: &ServiceRegistry) {}

        async fn instance_ids(&self) -> anyhow::Result<Vec<uuid::Uuid>> {
            Ok(Vec::new())
        }

        fn failed_instance_ids(&self) -> Vec<uuid::Uuid> {
            Vec::new()
        }

        async fn wait_for_instance_state_change(&self, generation: usize) -> usize {
            generation.wrapping_add(1)
        }
    }

    #[async_trait]
    impl TunnelDialer for StalledThenReadyDialer {
        async fn connect(&self) -> anyhow::Result<Box<dyn Tunnel>> {
            if self.attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                return pending().await;
            }

            let (tunnel, _peer) = create_ring_tunnel_pair();
            Ok(tunnel)
        }

        fn remote_url(&self) -> Url {
            "ring://config-server".parse().unwrap()
        }
    }

    #[test]
    fn heartbeat_hides_failed_instances_from_the_running_list() {
        let running = uuid::Uuid::new_v4();
        let failed = uuid::Uuid::new_v4();
        let stopped_clean = uuid::Uuid::new_v4();
        let instance_ids = vec![running, failed, stopped_clean];
        let failed_instance_ids = vec![failed];

        let reported = running_instances_for_heartbeat(instance_ids.clone(), &failed_instance_ids);

        assert_eq!(reported, vec![running, stopped_clean]);
        assert!(running_instances_for_heartbeat(instance_ids, &[]).len() == 3);
    }

    #[tokio::test]
    async fn stalled_connection_attempt_times_out_and_allows_redial() {
        let connector = StalledThenReadyDialer {
            attempts: AtomicUsize::new(0),
        };

        let error = connect_config_server(&connector, std::time::Duration::from_millis(10))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("connection timed out"));

        connect_config_server(&connector, std::time::Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(connector.attempts.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn instance_state_change_interrupts_a_long_heartbeat_interval() {
        let policy = HeartbeatPolicy {
            interval: std::time::Duration::from_secs(60),
            timeout_ms: 65_000,
        };

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_for_next_heartbeat(
                &ImmediateStateChangeBackend,
                0,
                policy,
                std::time::Duration::ZERO,
            ),
        )
        .await
        .expect("instance state change must wake heartbeat before its interval");
    }

    #[test]
    fn endpoint_normalizes_non_websocket_paths() {
        let endpoint =
            ConfigServerEndpoint::parse("udp://example.com/team%2Ftoken", |_| true).unwrap();
        assert_eq!(endpoint.token(), "team/token");
        assert_eq!(endpoint.connect_url().as_str(), "udp://example.com");
    }

    #[test]
    fn endpoint_rejects_token_shorthand() {
        let error = ConfigServerEndpoint::parse("team%2Ftoken", |_| true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to parse config server URL")
        );
    }

    #[test]
    fn endpoint_preserves_websocket_path_and_validates_scheme() {
        let endpoint =
            ConfigServerEndpoint::parse("wss://example.com/team", |url| url.scheme() == "wss")
                .unwrap();
        assert_eq!(endpoint.token(), "team");
        assert_eq!(endpoint.connect_url().as_str(), "wss://example.com/team");

        let error =
            ConfigServerEndpoint::parse("unknown://example.com/team", |_| false).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported config server scheme")
        );
    }

    #[test]
    fn endpoint_rejects_an_empty_token() {
        assert!(ConfigServerEndpoint::parse("udp://example.com", |_| true).is_err());
    }

    #[test]
    fn heartbeat_request_carries_registered_and_failed_instance_ids() {
        let runtime_id = uuid::Uuid::new_v4();
        let registered = uuid::Uuid::new_v4();
        let failed = uuid::Uuid::new_v4();
        let disabled = uuid::Uuid::new_v4();
        let request = build_heartbeat_request(
            &WebClientConfig {
                token: "token".to_owned(),
                machine_id: uuid::Uuid::new_v4(),
                hostname: "host".to_owned(),
                device_os: DeviceOsInfo::default(),
                easytier_version: "test-version".to_owned(),
                secure_mode: false,
            },
            runtime_id,
            vec![registered],
            vec![failed],
            vec![disabled],
            true,
        );

        assert_eq!(request.inst_id.map(uuid::Uuid::from), Some(runtime_id));
        assert_eq!(
            request
                .running_network_instances
                .into_iter()
                .map(uuid::Uuid::from)
                .collect::<Vec<_>>(),
            vec![registered]
        );
        assert_eq!(
            request
                .failed_network_instances
                .into_iter()
                .map(uuid::Uuid::from)
                .collect::<Vec<_>>(),
            vec![failed]
        );
        assert!(request.support_heartbeat_policy);
        assert!(request.support_user_disabled_instances);
        assert_eq!(
            request
                .disabled_network_instances
                .into_iter()
                .map(uuid::Uuid::from)
                .collect::<Vec<_>>(),
            vec![disabled]
        );
    }

    #[test]
    fn heartbeat_policy_uses_safe_defaults_for_legacy_servers() {
        let (policy, adjusted) = HeartbeatPolicy::from_response(&HeartbeatResponse::default());

        assert!(!adjusted);
        assert_eq!(
            policy.interval,
            std::time::Duration::from_millis(DEFAULT_HEARTBEAT_INTERVAL_MS.into())
        );
        assert_eq!(policy.timeout_ms, DEFAULT_HEARTBEAT_TIMEOUT_MS as i32);
    }

    #[test]
    fn heartbeat_policy_clamps_server_values_and_preserves_timeout_margin() {
        let (minimum, adjusted) = HeartbeatPolicy::from_response(&HeartbeatResponse {
            heartbeat_interval_ms: Some(1),
            heartbeat_timeout_ms: Some(1),
            managed_config_revision: None,
        });
        assert!(adjusted);
        assert_eq!(
            minimum.interval,
            std::time::Duration::from_millis(MIN_HEARTBEAT_INTERVAL_MS.into())
        );
        assert_eq!(minimum.timeout_ms, 6_000);

        let (maximum, adjusted) = HeartbeatPolicy::from_response(&HeartbeatResponse {
            heartbeat_interval_ms: Some(u32::MAX),
            heartbeat_timeout_ms: Some(u32::MAX),
            managed_config_revision: None,
        });
        assert!(adjusted);
        assert_eq!(
            maximum.interval,
            std::time::Duration::from_millis(MAX_HEARTBEAT_INTERVAL_MS.into())
        );
        assert_eq!(maximum.timeout_ms, MAX_HEARTBEAT_TIMEOUT_MS as i32);

        let (margin, adjusted) = HeartbeatPolicy::from_response(&HeartbeatResponse {
            heartbeat_interval_ms: Some(60_000),
            heartbeat_timeout_ms: Some(5_000),
            managed_config_revision: None,
        });
        assert!(adjusted);
        assert_eq!(margin.timeout_ms, 65_000);
    }

    fn test_controller() -> WebClientController {
        WebClientController {
            config: WebClientConfig {
                token: "token".to_owned(),
                machine_id: uuid::Uuid::new_v4(),
                hostname: "host".to_owned(),
                device_os: DeviceOsInfo::default(),
                easytier_version: "test".to_owned(),
                secure_mode: false,
            },
            backend: Arc::new(ImmediateStateChangeBackend),
            runtime_id: uuid::Uuid::new_v4(),
            managed_config_revision: StdMutex::new(None),
            active_rpc: StdMutex::new(None),
            revision_generation: std::sync::atomic::AtomicU64::new(0),
            pending_report: StdMutex::new(std::collections::HashMap::new()),
        }
    }

    #[test]
    fn backends_that_cannot_report_disabled_instances_default_to_unsupported() {
        // Fail-safe default: claiming support with an always-empty disabled list
        // would let the console auto-run intentionally stopped instances.
        assert!(!ImmediateStateChangeBackend.support_user_disabled_instances());
    }

    #[test]
    fn heartbeat_revision_is_cached_when_no_local_report_raced() {
        let controller = test_controller();
        let sampled = controller.revision_generation.load(Ordering::Acquire);

        controller.store_heartbeat_revision(sampled, Some("rev-from-heartbeat".to_owned()));

        assert_eq!(
            controller.cached_revision().as_deref(),
            Some("rev-from-heartbeat")
        );
    }

    #[test]
    fn stale_heartbeat_cannot_clobber_a_fresher_local_report() {
        let controller = test_controller();
        // The heartbeat samples the generation before its RPC round trip...
        let sampled = controller.revision_generation.load(Ordering::Acquire);
        // ...and a local report lands while that heartbeat is in flight.
        controller.store_local_revision(Some("rev-from-local-report".to_owned()));

        controller.store_heartbeat_revision(sampled, Some("stale-heartbeat-rev".to_owned()));

        assert_eq!(
            controller.cached_revision().as_deref(),
            Some("rev-from-local-report")
        );
    }

    #[test]
    fn retrying_the_same_edit_reuses_the_pending_revision() {
        let controller = test_controller();

        let first = controller.revision_for_report("instance-a", "expected-1");
        assert_eq!(
            controller.revision_for_report("instance-a", "expected-1"),
            first,
            "a retry of the same edit must stay idempotent on the console"
        );
        assert_ne!(
            controller.revision_for_report("instance-a", "expected-2"),
            first,
            "a different expected revision is a different edit"
        );

        controller.clear_pending_report("instance-a");
        assert_ne!(
            controller.revision_for_report("instance-a", "expected-1"),
            first,
            "a confirmed report starts a new revision"
        );
    }

    #[test]
    fn concurrent_instance_reports_keep_independent_pending_revisions() {
        let controller = test_controller();

        let a = controller.revision_for_report("instance-a", "expected-1");
        let b = controller.revision_for_report("instance-b", "expected-1");
        assert_ne!(a, b);
        assert_eq!(
            controller.revision_for_report("instance-a", "expected-1"),
            a,
            "instance-a pending must survive a concurrent instance-b report"
        );
        assert_eq!(
            controller.revision_for_report("instance-b", "expected-1"),
            b
        );

        controller.clear_pending_report("instance-a");
        assert_eq!(
            controller.revision_for_report("instance-b", "expected-1"),
            b,
            "clearing instance-a must not drop instance-b pending"
        );
    }
}
