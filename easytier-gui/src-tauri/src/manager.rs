//! GUI client manager, storage, and web-client hooks.

use super::*;
use async_trait::async_trait;
use dashmap::{DashMap, DashSet};
use easytier::common::config::{ConfigSource, NetworkConfig, NetworkConfigExt};
use easytier::proto::api::logger::{
    GetLoggerConfigRequest, LoggerRpc, LoggerRpcClientFactory, SetLoggerConfigRequest,
};
#[cfg(target_os = "android")]
use easytier::proto::api::manage::DeleteNetworkInstanceRequest;
use easytier::proto::api::manage::GetNetworkInstanceConfigRequest;
use easytier::proto::api::manage::RunNetworkInstanceRequest;
use easytier::proto::rpc::bidirect::BidirectRpcManager;
use easytier::proto::rpc_types::controller::BaseController;
#[cfg(target_os = "android")]
use easytier::proto::rpc_types::controller::Controller as _;
use easytier::web_client::WebClientHooks;
use easytier_core::management::config_source_from_rpc;
use easytier_core::management::remote_client::{
    PersistentConfig, RemoteClientError, merge_runtime_only_fields_into_stored,
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use tauri::Emitter;

pub(crate) struct GuiHooks {
    pub(crate) app: AppHandle,
}

#[async_trait]
impl WebClientHooks for GuiHooks {
    async fn pre_run_network_instance(
        &self,
        cfg: &easytier::common::config::TomlConfigLoader,
    ) -> Result<(), String> {
        let client_manager = get_client_manager!()?;
        client_manager
            .pre_run_network_instance_hook(
                &self.app,
                cfg,
                PersistedConfigSource::from_runtime_source(cfg.get_network_config_source()),
            )
            .await
    }

    async fn post_run_network_instance(&self, instance_id: &uuid::Uuid) -> Result<(), String> {
        let client_manager = get_client_manager!()?;
        client_manager
            .post_run_network_instance_hook(&self.app, instance_id)
            .await
    }

    async fn on_managed_config_overwrite(&self, instance_id: &uuid::Uuid) -> Result<(), String> {
        // Writeback + tip (§6.2); overwrite already applied — never fail the run path.
        // Prefer emit only after a successful writeback so the UI does not reload
        // a still-stale localStorage cache. `pre_run` usually wrote already; this
        // path is a second chance via live RPC.
        let client_manager = get_client_manager!()?;
        match client_manager
            .persist_synced_runtime_config(&self.app, *instance_id)
            .await
        {
            Ok(()) => {
                let _ = self
                    .app
                    .emit("managed_config_synced_overwrite", instance_id.to_string());
            }
            Err(error) => {
                tracing::warn!(
                    %instance_id,
                    %error,
                    "managed overwrite writeback failed; skip sync tip emit"
                );
            }
        }
        Ok(())
    }

    async fn on_managed_config_patched(
        &self,
        cfg: &easytier::common::config::TomlConfigLoader,
    ) -> Result<(), String> {
        // Hot-patch updates running TomlConfig without pre_run; refresh GUI storage
        // so storage-first get_config does not show a stale local cache.
        let client_manager = get_client_manager!()?;
        if let Err(error) = client_manager
            .persist_synced_toml_config(
                &self.app,
                cfg,
                PersistedConfigSource::from_runtime_source(cfg.get_network_config_source()),
            )
            .await
        {
            tracing::warn!(
                instance_id = %cfg.get_id(),
                %error,
                "managed hot-patch writeback failed"
            );
            return Err(error);
        }
        let _ = self
            .app
            .emit("managed_config_synced_overwrite", cfg.get_id().to_string());
        Ok(())
    }

    async fn post_remove_network_instances(
        &self,
        ids: &[uuid::Uuid],
        keep_local_config: bool,
    ) -> Result<(), String> {
        let client_manager = get_client_manager!()?;
        client_manager
            .post_remote_remove_network_instances_hook(&self.app, ids, keep_local_config)
            .await
    }
}

/// Matches the Windows auto-generated wintun name from `virtual_nic::create_tun`
/// (`et_<interface_count>_<4 alnum>`). Kept in sync with
/// `easytier-web::client_manager::runtime_reconcile::is_automatic_windows_dev_name`.
fn is_automatic_windows_dev_name(dev_name: &str) -> bool {
    let Some((interface_count, suffix)) = dev_name
        .strip_prefix("et_")
        .and_then(|value| value.split_once('_'))
    else {
        return false;
    };
    !interface_count.is_empty()
        && interface_count.bytes().all(|byte| byte.is_ascii_digit())
        && suffix.len() == 4
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub(crate) enum PersistedConfigSource {
    User,
    #[serde(alias = "webhook")]
    Web,
    #[serde(other)]
    #[default]
    Legacy,
}

impl PersistedConfigSource {
    pub(crate) fn from_runtime_source(source: ConfigSource) -> Self {
        match source {
            ConfigSource::User => Self::User,
            ConfigSource::Web => Self::Web,
        }
    }

    fn merge_persisted(self, incoming: Self) -> Self {
        match (self, incoming) {
            // Older runtimes report missing source as `user`. Keep the stronger persisted
            // ownership until web sync or an explicit user save repairs it.
            (Self::Web, Self::User) | (Self::Legacy, Self::User) => self,
            (_, next) => next,
        }
    }

    pub(crate) fn to_runtime_source(self) -> ConfigSource {
        match self {
            Self::User | Self::Legacy => ConfigSource::User,
            Self::Web => ConfigSource::Web,
        }
    }

    #[cfg(any(test, target_os = "android"))]
    fn is_web_like(self) -> bool {
        matches!(self, Self::Web)
    }
}

#[derive(Clone)]
pub(crate) struct GUIConfig {
    inst_id: String,
    pub(crate) config: NetworkConfig,
    source: PersistedConfigSource,
    /// Last observed running-instance config fingerprint (Service/Remote adopt).
    /// `None` until the first successful runtime read; user Save must not clear it.
    last_observed_runtime_fp: Option<String>,
    /// Mirrored from `enabled_networks` when listing; not persisted.
    disabled: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct StoredGuiConfig {
    config: NetworkConfig,
    #[serde(default)]
    source: PersistedConfigSource,
    /// See [`GUIConfig::last_observed_runtime_fp`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_observed_runtime_fp: Option<String>,
}

/// Stable fingerprint of a running NetworkConfig body (for console-sync detect).
pub(crate) fn network_config_runtime_fingerprint(cfg: &NetworkConfig) -> String {
    let Ok(bytes) = serde_json::to_vec(cfg) else {
        return String::new();
    };
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebRuntimeObserveAction {
    /// First sight (or only runtime-only drift): record fingerprint, keep storage body.
    LearnFingerprint,
    /// Console/device applied a different managed body: replace storage.
    AdoptRuntime,
    /// Fingerprint unchanged: storage-first.
    KeepStorage,
}

fn web_runtime_observe_action(
    stored: &NetworkConfig,
    runtime: &NetworkConfig,
    prev_fp: Option<&str>,
    runtime_fp: &str,
) -> WebRuntimeObserveAction {
    match prev_fp {
        None => WebRuntimeObserveAction::LearnFingerprint,
        Some(prev) if prev == runtime_fp => WebRuntimeObserveAction::KeepStorage,
        Some(_) => {
            // Ignore pure runtime-only drift (e.g. auto wintun `dev_name`) so
            // serialization jitter does not thrash localStorage / emit storms.
            let mut merged = stored.clone();
            merge_runtime_only_fields_into_stored(&mut merged, runtime);
            if network_config_runtime_fingerprint(&merged) == runtime_fp {
                WebRuntimeObserveAction::LearnFingerprint
            } else {
                WebRuntimeObserveAction::AdoptRuntime
            }
        }
    }
}

impl GUIConfig {
    fn new_with_runtime_fp(
        inst_id: String,
        config: NetworkConfig,
        source: PersistedConfigSource,
        last_observed_runtime_fp: Option<String>,
    ) -> Self {
        Self {
            inst_id,
            config,
            source,
            last_observed_runtime_fp,
            disabled: false,
        }
    }

    fn into_stored(self) -> StoredGuiConfig {
        StoredGuiConfig {
            config: self.config,
            source: self.source,
            last_observed_runtime_fp: self.last_observed_runtime_fp,
        }
    }
}

impl PersistentConfig<anyhow::Error> for GUIConfig {
    fn get_network_inst_id(&self) -> &str {
        &self.inst_id
    }
    fn get_network_config(&self) -> Result<NetworkConfig, anyhow::Error> {
        Ok(self.config.clone())
    }
    fn get_network_config_source(&self) -> ConfigSource {
        self.source.to_runtime_source()
    }
    fn is_disabled(&self) -> bool {
        self.disabled
    }
}

pub(crate) struct GUIStorage {
    network_configs: DashMap<Uuid, GUIConfig>,
    enabled_networks: DashSet<Uuid>,
}
impl GUIStorage {
    fn new() -> Self {
        Self {
            network_configs: DashMap::new(),
            enabled_networks: DashSet::new(),
        }
    }

    fn save_configs(&self, app: &AppHandle) -> anyhow::Result<()> {
        let configs = self
            .network_configs
            .iter()
            .map(|entry| entry.value().clone().into_stored())
            .collect::<Vec<_>>();
        app.emit("save_configs", configs)?;
        Ok(())
    }

    fn save_enabled_networks(&self, app: &AppHandle) -> anyhow::Result<()> {
        let payload: Vec<String> = self
            .enabled_networks
            .iter()
            .map(|entry| entry.key().to_string())
            .collect();
        app.emit("save_enabled_networks", payload)?;
        Ok(())
    }

    /// Batch-disable: mutate all ids then a single `save_enabled_networks`.
    pub(crate) fn disable_network_configs(
        &self,
        app: &AppHandle,
        network_inst_ids: &[Uuid],
    ) -> anyhow::Result<()> {
        for network_inst_id in network_inst_ids {
            self.enabled_networks.remove(network_inst_id);
        }
        self.save_enabled_networks(app)
    }

    fn save_config(
        &self,
        app: &AppHandle,
        inst_id: Uuid,
        mut cfg: NetworkConfig,
        source: PersistedConfigSource,
    ) -> anyhow::Result<()> {
        // Stale UI forms often still carry an empty `dev_name` after Windows auto-assigns
        // `et_<n>_<xxxx>` and `persist_runtime_dev_name` writes it back. Do not let a later
        // Save wipe that durable adapter identity — otherwise the next enable allocates a
        // brand-new wintun NIC again.
        if cfg.dev_name.as_deref().is_none_or(|name| name.is_empty())
            && let Some(existing) = self.network_configs.get(&inst_id)
            && let Some(stored_name) = existing
                .config
                .dev_name
                .as_deref()
                .filter(|name| is_automatic_windows_dev_name(name))
        {
            cfg.dev_name = Some(stored_name.to_owned());
        }

        let (source, last_observed_runtime_fp) = self
            .network_configs
            .get(&inst_id)
            .map(|existing| {
                (
                    existing.source.merge_persisted(source),
                    // User/local save must keep the last seen runtime fingerprint so a
                    // later console apply (runtime body change) is still detectable.
                    existing.last_observed_runtime_fp.clone(),
                )
            })
            .unwrap_or((source, None));
        let config = GUIConfig::new_with_runtime_fp(
            inst_id.to_string(),
            cfg,
            source,
            last_observed_runtime_fp,
        );
        self.network_configs.insert(inst_id, config);
        self.save_configs(app)
    }

    /// Persist config and record the runtime fingerprint (console sync / adopt).
    fn save_config_observing_runtime(
        &self,
        app: &AppHandle,
        inst_id: Uuid,
        cfg: NetworkConfig,
        source: PersistedConfigSource,
        runtime_fp: String,
    ) -> anyhow::Result<()> {
        let source = self
            .network_configs
            .get(&inst_id)
            .map(|existing| existing.source.merge_persisted(source))
            .unwrap_or(source);
        let config =
            GUIConfig::new_with_runtime_fp(inst_id.to_string(), cfg, source, Some(runtime_fp));
        self.network_configs.insert(inst_id, config);
        self.save_configs(app)
    }

    /// Learn the current runtime fingerprint without replacing the stored body.
    fn note_observed_runtime_fp(
        &self,
        app: &AppHandle,
        inst_id: Uuid,
        runtime_fp: String,
    ) -> anyhow::Result<()> {
        let Some(mut entry) = self.network_configs.get_mut(&inst_id) else {
            return Ok(());
        };
        if entry.last_observed_runtime_fp.as_deref() == Some(runtime_fp.as_str()) {
            return Ok(());
        }
        entry.last_observed_runtime_fp = Some(runtime_fp);
        drop(entry);
        self.save_configs(app)
    }

    pub(crate) fn persisted_source(&self, inst_id: Uuid) -> Option<PersistedConfigSource> {
        self.network_configs.get(&inst_id).map(|entry| entry.source)
    }
}
#[async_trait]
impl Storage<AppHandle, GUIConfig, anyhow::Error> for GUIStorage {
    async fn insert_or_update_user_network_config(
        &self,
        app: AppHandle,
        network_inst_id: Uuid,
        network_config: NetworkConfig,
        source: ConfigSource,
    ) -> Result<(), anyhow::Error> {
        self.save_config(
            &app,
            network_inst_id,
            network_config,
            PersistedConfigSource::from_runtime_source(source),
        )?;
        self.enabled_networks.insert(network_inst_id);
        self.save_enabled_networks(&app)?;
        Ok(())
    }

    async fn delete_network_configs(
        &self,
        app: AppHandle,
        network_inst_ids: &[Uuid],
    ) -> Result<(), anyhow::Error> {
        for network_inst_id in network_inst_ids {
            self.network_configs.remove(network_inst_id);
            self.enabled_networks.remove(network_inst_id);
        }
        self.save_configs(&app)?;
        self.save_enabled_networks(&app)?;
        Ok(())
    }

    async fn update_network_config_state(
        &self,
        app: AppHandle,
        network_inst_id: Uuid,
        disabled: bool,
    ) -> Result<(), anyhow::Error> {
        if disabled {
            self.enabled_networks.remove(&network_inst_id);
        } else {
            self.enabled_networks.insert(network_inst_id);
        }
        self.save_enabled_networks(&app)?;
        Ok(())
    }

    async fn list_network_configs(
        &self,
        _: AppHandle,
        props: ListNetworkProps,
    ) -> Result<Vec<GUIConfig>, anyhow::Error> {
        let mut ret = Vec::new();
        for entry in self.network_configs.iter() {
            let id: Uuid = entry.key().to_owned();
            let enabled = self.enabled_networks.contains(&id);
            let mut cfg = entry.value().clone();
            cfg.disabled = !enabled;
            match props {
                ListNetworkProps::All => {
                    ret.push(cfg);
                }
                ListNetworkProps::EnabledOnly => {
                    if enabled {
                        ret.push(cfg);
                    }
                }
                ListNetworkProps::DisabledOnly => {
                    if !enabled {
                        ret.push(cfg);
                    }
                }
            }
        }
        Ok(ret)
    }

    async fn get_network_config(
        &self,
        _: AppHandle,
        network_inst_id: &str,
    ) -> Result<Option<GUIConfig>, anyhow::Error> {
        let uuid = Uuid::parse_str(network_inst_id)?;
        Ok(self
            .network_configs
            .get(&uuid)
            .map(|entry| entry.value().clone()))
    }
}

pub(crate) struct GUIClientManager {
    pub(crate) storage: GUIStorage,
    pub(crate) rpc_manager: BidirectRpcManager,
}
impl GUIClientManager {
    pub async fn new(
        rpc_url: Option<String>,
        local_process_runtime: Option<Arc<CoreProcessRuntime>>,
    ) -> Result<Self, anyhow::Error> {
        let tunnel = if let Some(url) = rpc_url {
            runtime_rpc_dialer(url.parse()?).connect().await?
        } else {
            local_process_runtime
                .context("local RPC requires a core process runtime")?
                .connect_ring_tunnel(*RPC_RING_UUID.deref())?
        };

        let rpc_manager = BidirectRpcManager::new();
        rpc_manager.run_with_tunnel(tunnel);

        Ok(Self {
            storage: GUIStorage::new(),
            rpc_manager,
        })
    }

    pub fn get_enabled_instances_with_tun_ids(&self) -> impl Iterator<Item = uuid::Uuid> + '_ {
        self.storage
            .network_configs
            .iter()
            .filter(|v| self.storage.enabled_networks.contains(v.key()))
            .filter(|v| !v.config.no_tun())
            .filter_map(|c| c.config.instance_id().parse::<uuid::Uuid>().ok())
    }

    #[cfg(target_os = "android")]
    pub fn get_enabled_instances_with_web_like_tun_ids(
        &self,
    ) -> impl Iterator<Item = uuid::Uuid> + '_ {
        self.storage
            .network_configs
            .iter()
            .filter(|v| self.storage.enabled_networks.contains(v.key()))
            .filter(|v| !v.config.no_tun())
            .filter(|v| v.source.is_web_like())
            .filter_map(|c| c.config.instance_id().parse::<uuid::Uuid>().ok())
    }

    #[cfg(target_os = "android")]
    pub(crate) async fn disable_instances_with_tun(
        &self,
        app: &AppHandle,
        web_only: bool,
    ) -> Result<(), easytier_core::management::remote_client::RemoteClientError<anyhow::Error>>
    {
        use easytier_core::management::remote_client::RemoteClientError;

        let inst_ids: Vec<uuid::Uuid> = if web_only {
            self.get_enabled_instances_with_web_like_tun_ids().collect()
        } else {
            self.get_enabled_instances_with_tun_ids().collect()
        };
        if inst_ids.is_empty() {
            return Ok(());
        }
        // One Delete RPC for all ids (keep_local_config=true). Server awaits
        // every TUN stop before replying, so use the 60s mutator budget — the
        // default 5s would time out once several instances share one call.
        // GuiHooks also batch-disables on post_remove; we still disable locally
        // so storage stays consistent if the hook only warns.
        const BATCH_DELETE_TIMEOUT_MS: i32 = 60_000;
        let mut ctrl = BaseController::default();
        ctrl.set_timeout_ms(BATCH_DELETE_TIMEOUT_MS);
        let client = self
            .get_rpc_client(app.clone())
            .ok_or(RemoteClientError::ClientNotFound)?;
        client
            .delete_network_instance(
                ctrl,
                DeleteNetworkInstanceRequest {
                    inst_ids: inst_ids.iter().copied().map(Into::into).collect(),
                    keep_local_config: true,
                },
            )
            .await?;
        self.storage
            .disable_network_configs(app, &inst_ids)
            .map_err(RemoteClientError::PersistentError)?;
        self.notify_vpn_stop_if_no_tun(app)
            .map_err(|e| RemoteClientError::PersistentError(anyhow::anyhow!(e)))?;
        Ok(())
    }

    pub(crate) fn notify_vpn_stop_if_no_tun(&self, app: &AppHandle) -> Result<(), String> {
        let has_tun = self.get_enabled_instances_with_tun_ids().any(|_| true);
        if !has_tun {
            app.emit("vpn_service_stop", "")
                .map_err(|e| e.to_string())?;
            // A3: do not wait for WebView to process the emit — stop VpnService
            // from Rust so background/Doze cannot leave an orphan TUN.
            #[cfg(target_os = "android")]
            crate::android_vpn_watchdog::stop_vpn_if_no_tun(app)?;
        }
        Ok(())
    }

    /// Persist a console-synced TomlConfig into GUI storage (hot-patch / overwrite writeback).
    pub(crate) async fn persist_synced_toml_config(
        &self,
        app: &AppHandle,
        cfg: &easytier::common::config::TomlConfigLoader,
        source: PersistedConfigSource,
    ) -> Result<(), String> {
        let network = NetworkConfig::new_from_config(cfg).map_err(|e| e.to_string())?;
        let fp = network_config_runtime_fingerprint(&network);
        self.storage
            .save_config_observing_runtime(app, cfg.get_id(), network, source, fp)
            .map_err(|e| e.to_string())
    }

    /// Best-effort: read the running instance config and write it back to GUI storage.
    pub(crate) async fn persist_synced_runtime_config(
        &self,
        app: &AppHandle,
        instance_id: uuid::Uuid,
    ) -> Result<(), String> {
        // Must use the live process RPC (not storage-first get) so overwrite/hot-patch
        // writeback captures console-applied fields.
        let client = self
            .get_rpc_client(app.clone())
            .ok_or_else(|| "rpc client unavailable for synced writeback".to_string())?;
        let resp = client
            .get_network_instance_config(
                BaseController::default(),
                GetNetworkInstanceConfigRequest {
                    inst_id: Some(instance_id.into()),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        let Some(runtime_config) = resp.config else {
            return Err(format!(
                "running config unavailable for writeback: {instance_id}"
            ));
        };
        let source = config_source_from_rpc(resp.source)
            .map(PersistedConfigSource::from_runtime_source)
            .unwrap_or(PersistedConfigSource::Web);
        let fp = network_config_runtime_fingerprint(&runtime_config);
        self.storage
            .save_config_observing_runtime(app, instance_id, runtime_config, source, fp)
            .map_err(|e| e.to_string())
    }

    pub(crate) async fn pre_run_network_instance_hook(
        &self,
        app: &AppHandle,
        cfg: &easytier::common::config::TomlConfigLoader,
        source: PersistedConfigSource,
    ) -> Result<(), String> {
        let instance_id = cfg.get_id();
        app.emit("pre_run_network_instance", instance_id.to_string())
            .map_err(|e| e.to_string())?;

        #[cfg(target_os = "android")]
        if !cfg.get_flags().no_tun {
            match source {
                PersistedConfigSource::User | PersistedConfigSource::Legacy => {
                    self.disable_instances_with_tun(app, false)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                PersistedConfigSource::Web => {
                    self.disable_instances_with_tun(app, true)
                        .await
                        .map_err(|e| e.to_string())?;
                    if self.get_enabled_instances_with_tun_ids().next().is_some() {
                        return Err(
                            "Android only supports one active TUN network; user-managed VPN remains active"
                                .to_string(),
                        );
                    }
                }
            }
        }

        self.persist_synced_toml_config(app, cfg, source).await
    }

    pub(crate) async fn post_run_network_instance_hook(
        &self,
        app: &AppHandle,
        instance_id: &uuid::Uuid,
    ) -> Result<(), String> {
        #[cfg(target_os = "android")]
        if let Some(instance_manager) = crate::INSTANCE_MANAGER.read().await.as_ref() {
            let instance_uuid = *instance_id;
            if let Some(instance) = instance_manager.instance(instance_uuid) {
                if let Some(mut event_receiver) = subscribe_native_instance_event(&instance) {
                    let app_clone = app.clone();
                    let instance_id_clone = *instance_id;
                    tokio::spawn(async move {
                        let instance_id_str = instance_id_clone.to_string();
                        loop {
                            match event_receiver.recv().await {
                                Ok(
                                    easytier::common::global_ctx::GlobalCtxEvent::DhcpIpv4Changed(
                                        _,
                                        _,
                                    ),
                                ) => {
                                    let _ = app_clone.emit("dhcp_ip_changed", &instance_id_str);
                                }
                                Ok(
                                    easytier::common::global_ctx::GlobalCtxEvent::ProxyCidrsUpdated(
                                        _,
                                        _,
                                        _,
                                    ),
                                ) => {
                                    let _ = app_clone.emit("proxy_cidrs_updated", &instance_id_str);
                                }
                                // The core lost its TUN (read stream ended, sink fused,
                                // or the Android fd could not be attached). The native
                                // VpnService may still report "running", so the GUI has to
                                // tear it down and rebuild — `set_tun_device_error` alone
                                // never reaches `error_msg` (it is not `latest_error`).
                                Ok(
                                    easytier::common::global_ctx::GlobalCtxEvent::TunDeviceError(
                                        err,
                                    ),
                                ) => {
                                    tracing::warn!(
                                        instance = %instance_id_str,
                                        %err,
                                        "native TUN device failed; notifying GUI",
                                    );
                                    let _ = app_clone.emit("tun_device_error", &instance_id_str);
                                }
                                Ok(_) => {}
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    break;
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                    let _ = app_clone.emit("event_lagged", &instance_id_str);
                                    event_receiver = event_receiver.resubscribe();
                                }
                            }
                        }
                    });
                }
            }
        }

        self.storage.enabled_networks.insert(*instance_id);

        app.emit("post_run_network_instance", instance_id.to_string())
            .map_err(|e| e.to_string())?;

        // Windows only: the kernel may have assigned an auto-generated wintun adapter name.
        // Emitted first so the UI is not held up by the readback below. Best-effort on
        // purpose — the network is already running, so a persistence hiccup here must not
        // turn a successful enable into an error.
        #[cfg(target_os = "windows")]
        let _ = self.persist_runtime_dev_name(app, *instance_id).await;

        Ok(())
    }

    /// Persist the wintun adapter name the kernel assigned at startup.
    ///
    /// On Windows an empty `dev_name` makes the kernel invent `et_<ifcount>_<xxxx>` while
    /// creating the adapter. That name now reaches the management readback, but the instance
    /// is started asynchronously — `run_network_instance` returns before `create_tun` runs — so
    /// the config persisted by `pre_run_network_instance_hook` still carries an empty
    /// `dev_name`. Without this write-back the next launch (or the next enable, if the app was
    /// killed while enabled) picks a *different* name and leaves the previous `et_*` adapter
    /// behind, so every restart adds another NIC.
    ///
    /// Only user-owned configs are touched: web-owned configs are authoritative on the console
    /// side, whose reconciler deliberately ignores automatic Windows device names
    /// (`is_automatic_windows_dev_name`).
    #[cfg(target_os = "windows")]
    pub(crate) async fn persist_runtime_dev_name(
        &self,
        app: &AppHandle,
        instance_id: uuid::Uuid,
    ) -> Result<(), String> {
        const ATTEMPTS: usize = 15;
        const INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

        let Some(stored) = self
            .storage
            .get_network_config(app.clone(), &instance_id.to_string())
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(());
        };
        if stored.source == PersistedConfigSource::Web {
            return Ok(());
        }
        // Nothing to do once a concrete name is already stored; this is what keeps the
        // readback cost a one-off per config instead of a delay on every run.
        if stored
            .config
            .dev_name
            .as_deref()
            .is_some_and(|name| !name.is_empty())
        {
            return Ok(());
        }

        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(INTERVAL).await;
            }
            let Ok(runtime) = self
                .handle_get_network_config(app.clone(), instance_id)
                .await
            else {
                // Instance not up yet, or config readback is read-only for this instance.
                continue;
            };
            // A failed RPC falls back to the stored config, which is exactly the empty
            // `dev_name` we started from — require a non-empty name to make progress.
            let Some(dev_name) = runtime.dev_name.filter(|name| !name.is_empty()) else {
                continue;
            };

            let mut updated = stored.config.clone();
            updated.dev_name = Some(dev_name);
            self.storage
                .save_config(app, instance_id, updated, stored.source)
                .map_err(|e| e.to_string())?;
            return Ok(());
        }

        Ok(())
    }

    pub(crate) async fn post_remote_remove_network_instances_hook(
        &self,
        app: &AppHandle,
        ids: &[uuid::Uuid],
        keep_local_config: bool,
    ) -> Result<(), String> {
        // keep_local_config semantics: InstanceMutationHooks / proto field.
        if keep_local_config {
            self.storage
                .disable_network_configs(app, ids)
                .map_err(|e| e.to_string())?;
        } else {
            self.storage
                .delete_network_configs(app.clone(), ids)
                .await
                .map_err(|e| e.to_string())?;
        }
        self.notify_vpn_stop_if_no_tun(app)?;
        Ok(())
    }

    pub(crate) async fn post_stop_network_instances_hook(
        &self,
        app: &AppHandle,
    ) -> Result<(), String> {
        self.notify_vpn_stop_if_no_tun(app)?;
        Ok(())
    }

    fn get_logger_rpc_client(
        &self,
    ) -> Option<Box<dyn LoggerRpc<Controller = BaseController> + Send>> {
        Some(
            self.rpc_manager
                .rpc_client()
                .scoped_client::<LoggerRpcClientFactory<BaseController>>(1, 1, "".to_string()),
        )
    }

    pub(crate) async fn set_logging_level(&self, level: String) -> Result<(), anyhow::Error> {
        let logger_rpc = self
            .get_logger_rpc_client()
            .ok_or_else(|| anyhow::anyhow!("Logger RPC client not available"))?;
        logger_rpc
            .set_logger_config(
                BaseController::default(),
                SetLoggerConfigRequest {
                    level: easytier_core::management::parse_log_level(&level).into(),
                },
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn get_logging_level(&self) -> Result<String, anyhow::Error> {
        let logger_rpc = self
            .get_logger_rpc_client()
            .ok_or_else(|| anyhow::anyhow!("Logger RPC client not available"))?;
        let response = logger_rpc
            .get_logger_config(BaseController::default(), GetLoggerConfigRequest {})
            .await?;
        Ok(easytier_core::management::log_level_name(response.level()).to_string())
    }

    pub(crate) async fn load_configs(
        &self,
        app: AppHandle,
        configs: Vec<StoredGuiConfig>,
        enabled_networks: Vec<String>,
    ) -> anyhow::Result<()> {
        self.storage.network_configs.clear();
        for stored in configs {
            let instance_id = stored.config.instance_id();
            self.storage.network_configs.insert(
                instance_id.parse()?,
                GUIConfig::new_with_runtime_fp(
                    instance_id.to_string(),
                    stored.config,
                    stored.source,
                    stored.last_observed_runtime_fp,
                ),
            );
        }

        self.storage.enabled_networks.clear();
        let mut to_start = Vec::new();
        let mut queued = std::collections::HashSet::new();
        for id in enabled_networks {
            let Ok(uuid) = id.parse() else {
                continue;
            };
            if !queued.insert(uuid) {
                continue;
            }
            let config = self
                .storage
                .network_configs
                .get(&uuid)
                .map(|i| (i.value().config.clone(), i.value().source));
            let Some((config, source)) = config else {
                continue;
            };
            to_start.push((uuid, config, source));
        }

        // Android one-TUN pre_run must stay serial; desktop can start a few in parallel.
        #[cfg(target_os = "android")]
        {
            for (uuid, config, source) in to_start {
                self.start_enabled_network(&app, uuid, config, source)
                    .await?;
            }
        }
        #[cfg(not(target_os = "android"))]
        {
            const LOAD_CONFIG_CONCURRENCY: usize = 4;
            let mut rest = to_start.as_slice();
            while !rest.is_empty() {
                let n = rest.len().min(LOAD_CONFIG_CONCURRENCY);
                let (chunk, next) = rest.split_at(n);
                rest = next;
                match chunk {
                    [(u0, c0, s0)] => {
                        self.start_enabled_network(&app, *u0, c0.clone(), *s0)
                            .await?;
                    }
                    [(u0, c0, s0), (u1, c1, s1)] => {
                        let (r0, r1) = tokio::join!(
                            self.start_enabled_network(&app, *u0, c0.clone(), *s0),
                            self.start_enabled_network(&app, *u1, c1.clone(), *s1),
                        );
                        r0?;
                        r1?;
                    }
                    [(u0, c0, s0), (u1, c1, s1), (u2, c2, s2)] => {
                        let (r0, r1, r2) = tokio::join!(
                            self.start_enabled_network(&app, *u0, c0.clone(), *s0),
                            self.start_enabled_network(&app, *u1, c1.clone(), *s1),
                            self.start_enabled_network(&app, *u2, c2.clone(), *s2),
                        );
                        r0?;
                        r1?;
                        r2?;
                    }
                    [(u0, c0, s0), (u1, c1, s1), (u2, c2, s2), (u3, c3, s3)] => {
                        let (r0, r1, r2, r3) = tokio::join!(
                            self.start_enabled_network(&app, *u0, c0.clone(), *s0),
                            self.start_enabled_network(&app, *u1, c1.clone(), *s1),
                            self.start_enabled_network(&app, *u2, c2.clone(), *s2),
                            self.start_enabled_network(&app, *u3, c3.clone(), *s3),
                        );
                        r0?;
                        r1?;
                        r2?;
                        r3?;
                    }
                    _ => unreachable!("chunk size capped at LOAD_CONFIG_CONCURRENCY"),
                }
            }
        }
        Ok(())
    }

    async fn start_enabled_network(
        &self,
        app: &AppHandle,
        uuid: Uuid,
        config: NetworkConfig,
        source: PersistedConfigSource,
    ) -> anyhow::Result<()> {
        let client = self
            .get_rpc_client(app.clone())
            .ok_or_else(|| anyhow::anyhow!("RPC client not found"))?;
        let toml_config = config.gen_config_validated_for_run()?;
        self.pre_run_network_instance_hook(app, &toml_config, source)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        client
            .run_network_instance(
                BaseController::default(),
                RunNetworkInstanceRequest {
                    inst_id: None,
                    config: Some(config),
                    overwrite: false,
                    source: config_source_to_rpc(source.to_runtime_source()),
                },
            )
            .await?;
        self.post_run_network_instance_hook(app, &uuid)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }
}
#[async_trait]
impl RemoteClientManager<AppHandle, GUIConfig, anyhow::Error> for GUIClientManager {
    fn get_rpc_client(
        &self,
        _: AppHandle,
    ) -> Option<Box<dyn WebClientService<Controller = BaseController> + Send>> {
        Some(
            self.rpc_manager
                .rpc_client()
                .scoped_client::<WebClientServiceClientFactory<BaseController>>(
                    1,
                    1,
                    "".to_string(),
                ),
        )
    }

    fn get_storage(&self) -> &impl Storage<AppHandle, GUIConfig, anyhow::Error> {
        &self.storage
    }

    /// Storage-first body, with **Web-owned runtime adopt** for Service/Remote.
    ///
    /// When the running instance is Web-owned and its body fingerprint changes
    /// since the last observation, adopt the runtime config into GUI storage
    /// (console/device applied a newer managed config). Local Save keeps the
    /// previous fingerprint so storage can stay ahead of runtime until apply.
    async fn handle_get_network_config_with_source(
        &self,
        identify: AppHandle,
        inst_id: uuid::Uuid,
    ) -> Result<(NetworkConfig, ConfigSource), RemoteClientError<anyhow::Error>> {
        let inst_id_str = inst_id.to_string();
        let stored_row = self
            .storage
            .get_network_config(identify.clone(), &inst_id_str)
            .await
            .map_err(RemoteClientError::PersistentError)?;

        let runtime = if let Some(client) = self.get_rpc_client(identify.clone()) {
            match client
                .get_network_instance_config(
                    BaseController::default(),
                    GetNetworkInstanceConfigRequest {
                        inst_id: Some(inst_id.into()),
                    },
                )
                .await
            {
                Ok(resp) => resp.config.map(|config| (config, resp.source)),
                Err(error) => {
                    tracing::warn!(
                        %inst_id,
                        %error,
                        "get_network_instance_config failed; using storage body/source fallback"
                    );
                    None
                }
            }
        } else {
            None
        };

        if let Some(row) = stored_row {
            let source = runtime
                .as_ref()
                .and_then(|(_, rpc_source)| config_source_from_rpc(*rpc_source))
                .unwrap_or_else(|| row.get_runtime_network_config_source());

            if let Some((runtime_config, _)) = runtime.as_ref() {
                let runtime_fp = network_config_runtime_fingerprint(runtime_config);
                let observed = row.last_observed_runtime_fp.clone();

                if source == ConfigSource::Web {
                    match web_runtime_observe_action(
                        &row.config,
                        runtime_config,
                        observed.as_deref(),
                        &runtime_fp,
                    ) {
                        WebRuntimeObserveAction::LearnFingerprint => {
                            let _ = self
                                .storage
                                .note_observed_runtime_fp(&identify, inst_id, runtime_fp);
                        }
                        WebRuntimeObserveAction::AdoptRuntime => {
                            if let Err(error) = self.storage.save_config_observing_runtime(
                                &identify,
                                inst_id,
                                runtime_config.clone(),
                                PersistedConfigSource::Web,
                                runtime_fp,
                            ) {
                                tracing::warn!(
                                    %inst_id,
                                    %error,
                                    "failed to adopt web runtime config into GUI storage"
                                );
                            } else {
                                let _ = identify
                                    .emit("managed_config_synced_overwrite", inst_id.to_string());
                            }
                            return Ok((runtime_config.clone(), ConfigSource::Web));
                        }
                        WebRuntimeObserveAction::KeepStorage => {}
                    }
                } else if observed.as_deref() != Some(runtime_fp.as_str()) {
                    // Keep fingerprint current for User instances too so a later
                    // Web takeover still detects the next runtime change.
                    let _ = self
                        .storage
                        .note_observed_runtime_fp(&identify, inst_id, runtime_fp);
                }
            }

            let mut config = row
                .get_network_config()
                .map_err(RemoteClientError::PersistentError)?;
            if let Some((runtime_config, _)) = runtime.as_ref() {
                merge_runtime_only_fields_into_stored(&mut config, runtime_config);
            }
            return Ok((config, source));
        }

        if let Some((config, rpc_source)) = runtime {
            let source = config_source_from_rpc(rpc_source).unwrap_or(ConfigSource::User);
            return Ok((config, source));
        }

        Err(RemoteClientError::NotFound(format!(
            "No such network instance: {}",
            inst_id_str
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PersistedConfigSource, StoredGuiConfig, WebRuntimeObserveAction,
        is_automatic_windows_dev_name, network_config_runtime_fingerprint,
        web_runtime_observe_action,
    };
    use easytier::proto::api::manage::NetworkConfig;

    #[test]
    fn automatic_windows_dev_name_matches_et_count_suffix() {
        assert!(is_automatic_windows_dev_name("et_12_ab3d"));
        assert!(is_automatic_windows_dev_name("et_0_0000"));
        assert!(!is_automatic_windows_dev_name(""));
        assert!(!is_automatic_windows_dev_name("et0"));
        assert!(!is_automatic_windows_dev_name("et_12_ABCD")); // uppercase rejected
        assert!(!is_automatic_windows_dev_name("et_12_abcde")); // suffix too long
        assert!(!is_automatic_windows_dev_name("custom_tun"));
    }

    #[test]
    fn stored_gui_config_defaults_missing_source_to_legacy() {
        let stored: StoredGuiConfig = serde_json::from_value(serde_json::json!({
            "config": NetworkConfig::default(),
        }))
        .unwrap();
        assert_eq!(stored.source, PersistedConfigSource::Legacy);
    }

    #[test]
    fn stored_gui_config_deserializes_webhook_source_as_web() {
        let stored: StoredGuiConfig = serde_json::from_value(serde_json::json!({
            "config": NetworkConfig::default(),
            "source": "webhook",
        }))
        .unwrap();
        assert_eq!(stored.source, PersistedConfigSource::Web);
    }

    #[test]
    fn stored_gui_config_defaults_unknown_source_to_legacy() {
        let stored: StoredGuiConfig = serde_json::from_value(serde_json::json!({
            "config": NetworkConfig::default(),
            "source": "unknown",
        }))
        .unwrap();
        assert_eq!(stored.source, PersistedConfigSource::Legacy);
    }

    #[test]
    fn persisted_source_merge_keeps_legacy_and_web_over_ambiguous_user() {
        assert_eq!(
            PersistedConfigSource::Legacy.merge_persisted(PersistedConfigSource::User),
            PersistedConfigSource::Legacy
        );
        assert_eq!(
            PersistedConfigSource::Web.merge_persisted(PersistedConfigSource::User),
            PersistedConfigSource::Web
        );
        assert_eq!(
            PersistedConfigSource::Legacy.merge_persisted(PersistedConfigSource::Web),
            PersistedConfigSource::Web
        );
    }

    #[test]
    fn only_web_configs_are_web_like() {
        assert!(!PersistedConfigSource::Legacy.is_web_like());
        assert!(!PersistedConfigSource::User.is_web_like());
        assert!(PersistedConfigSource::Web.is_web_like());
    }

    fn named(name: &str, secret: &str, dev: Option<&str>) -> NetworkConfig {
        NetworkConfig {
            network_name: Some(name.to_owned()),
            network_secret: Some(secret.to_owned()),
            dev_name: dev.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn web_observe_learns_baseline_without_adopting_local_save() {
        let stored = named("saved", "s1", None);
        let runtime = named("running", "r0", Some("et_1_abcd"));
        let fp = network_config_runtime_fingerprint(&runtime);
        assert_eq!(
            web_runtime_observe_action(&stored, &runtime, None, &fp),
            WebRuntimeObserveAction::LearnFingerprint
        );
        assert_eq!(
            web_runtime_observe_action(&stored, &runtime, Some(&fp), &fp),
            WebRuntimeObserveAction::KeepStorage
        );
    }

    #[test]
    fn web_observe_adopts_when_runtime_body_changes() {
        let stored = named("saved", "s1", Some("et_1_abcd"));
        let old = named("running", "r0", Some("et_1_abcd"));
        let new = named("console", "c1", Some("et_1_abcd"));
        let old_fp = network_config_runtime_fingerprint(&old);
        let new_fp = network_config_runtime_fingerprint(&new);
        assert_ne!(old_fp, new_fp);
        assert_eq!(
            web_runtime_observe_action(&stored, &new, Some(&old_fp), &new_fp),
            WebRuntimeObserveAction::AdoptRuntime
        );
    }

    #[test]
    fn web_observe_learns_when_only_runtime_dev_name_fills_in() {
        let stored = named("same", "s", None);
        let runtime = named("same", "s", Some("et_1_abcd"));
        let prev = network_config_runtime_fingerprint(&named("same", "s", None));
        let fp = network_config_runtime_fingerprint(&runtime);
        assert_eq!(
            web_runtime_observe_action(&stored, &runtime, Some(&prev), &fp),
            WebRuntimeObserveAction::LearnFingerprint
        );
    }

    #[test]
    fn stored_gui_config_keeps_runtime_fingerprint() {
        let stored: StoredGuiConfig = serde_json::from_value(serde_json::json!({
            "config": NetworkConfig::default(),
            "source": "web",
            "last_observed_runtime_fp": "abc",
        }))
        .unwrap();
        assert_eq!(stored.last_observed_runtime_fp.as_deref(), Some("abc"));
    }
}
