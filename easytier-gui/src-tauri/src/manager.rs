//! GUI client manager, storage, and web-client hooks.

use super::*;
use async_trait::async_trait;
use dashmap::{DashMap, DashSet};
use easytier::common::config::{NetworkConfig, NetworkConfigExt};
use easytier::proto::api::logger::{LoggerRpc, LoggerRpcClientFactory, SetLoggerConfigRequest};
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
        cfg: NetworkConfig,
        source: PersistedConfigSource,
    ) -> anyhow::Result<()> {
        let source = self
            .network_configs
            .get(&inst_id)
            .map(|existing| existing.source.merge_persisted(source))
            .unwrap_or(source);
        let config = GUIConfig::new(inst_id.to_string(), cfg, source);
        self.network_configs.insert(inst_id, config);
        self.save_configs(app)
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
                                    ),
                                ) => {
                                    let _ = app_clone.emit("proxy_cidrs_updated", &instance_id_str);
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
    use super::{PersistedConfigSource, StoredGuiConfig};
    use easytier::proto::api::manage::NetworkConfig;

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
