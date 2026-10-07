//! GUI client manager, storage, and web-client hooks.

use super::*;
use async_trait::async_trait;
use dashmap::{DashMap, DashSet};
use easytier::common::config::{ConfigSource, NetworkConfig, NetworkConfigExt};
use easytier::proto::api::logger::{
    GetLoggerConfigRequest, LoggerRpc, LoggerRpcClientFactory, SetLoggerConfigRequest,
};
use easytier::proto::api::manage::RunNetworkInstanceRequest;
use easytier::proto::rpc::bidirect::BidirectRpcManager;
use easytier::proto::rpc_types::controller::BaseController;
use easytier::web_client::WebClientHooks;
use easytier_core::management::remote_client::PersistentConfig;

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

    async fn post_remove_network_instances(&self, ids: &[uuid::Uuid]) -> Result<(), String> {
        let client_manager = get_client_manager!()?;
        client_manager
            .post_remote_remove_network_instances_hook(&self.app, ids)
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

    fn to_runtime_source(self) -> ConfigSource {
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
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct StoredGuiConfig {
    config: NetworkConfig,
    #[serde(default)]
    source: PersistedConfigSource,
}

impl GUIConfig {
    fn new(inst_id: String, config: NetworkConfig, source: PersistedConfigSource) -> Self {
        Self {
            inst_id,
            config,
            source,
        }
    }

    fn into_stored(self) -> StoredGuiConfig {
        StoredGuiConfig {
            config: self.config,
            source: self.source,
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

        let source = self
            .network_configs
            .get(&inst_id)
            .map(|existing| existing.source.merge_persisted(source))
            .unwrap_or(source);
        let config = GUIConfig::new(inst_id.to_string(), cfg, source);
        self.network_configs.insert(inst_id, config);
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
            match props {
                ListNetworkProps::All => {
                    ret.push(entry.value().clone());
                }
                ListNetworkProps::EnabledOnly => {
                    if self.enabled_networks.contains(&id) {
                        ret.push(entry.value().clone());
                    }
                }
                ListNetworkProps::DisabledOnly => {
                    if !self.enabled_networks.contains(&id) {
                        ret.push(entry.value().clone());
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
        let inst_ids: Vec<uuid::Uuid> = if web_only {
            self.get_enabled_instances_with_web_like_tun_ids().collect()
        } else {
            self.get_enabled_instances_with_tun_ids().collect()
        };
        for inst_id in inst_ids {
            self.handle_update_network_state(app.clone(), inst_id, true)
                .await?;
        }
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

        self.storage
            .save_config(
                app,
                instance_id,
                NetworkConfig::new_from_config(cfg).map_err(|e| e.to_string())?,
                source,
            )
            .map_err(|e| e.to_string())?;

        Ok(())
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
    ) -> Result<(), String> {
        self.storage
            .delete_network_configs(app.clone(), ids)
            .await
            .map_err(|e| e.to_string())?;
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
                GUIConfig::new(instance_id.to_string(), stored.config, stored.source),
            );
        }

        self.storage.enabled_networks.clear();
        let client = self
            .get_rpc_client(app.clone())
            .ok_or_else(|| anyhow::anyhow!("RPC client not found"))?;
        for id in enabled_networks {
            if let Ok(uuid) = id.parse()
                && !self.storage.enabled_networks.contains(&uuid)
            {
                let config = self
                    .storage
                    .network_configs
                    .get(&uuid)
                    .map(|i| (i.value().config.clone(), i.value().source));
                let Some((config, source)) = config else {
                    continue;
                };
                let toml_config = config.gen_config()?;
                self.pre_run_network_instance_hook(&app, &toml_config, source)
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
                self.post_run_network_instance_hook(&app, &uuid)
                    .await
                    .map_err(|e| anyhow::anyhow!(e))?;
            }
        }
        Ok(())
    }
}
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
}

#[cfg(test)]
mod tests {
    use super::{PersistedConfigSource, StoredGuiConfig, is_automatic_windows_dev_name};
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
}
