use async_trait::async_trait;
use uuid::Uuid;

use easytier_proto::{
    api::manage::{
        CollectNetworkInfoRequest, CollectNetworkInfoResponse, DeleteNetworkInstanceRequest,
        GetNetworkInstanceConfigRequest, ListNetworkInstanceMetaRequest,
        ListNetworkInstanceRequest, NetworkConfig, NetworkMeta, RunNetworkInstanceRequest,
        ValidateConfigRequest, ValidateConfigResponse, WebClientService,
    },
    rpc_types::controller::BaseController,
};

use crate::config::toml::ConfigSource;

use super::{config_source_from_rpc, config_source_to_rpc};

#[async_trait]
pub trait RemoteClientManager<T, C, E>
where
    T: Clone + Send + 'static,
    C: PersistentConfig<E> + Send + 'static,
    E: Send + 'static,
{
    fn get_rpc_client(
        &self,
        identify: T,
    ) -> Option<Box<dyn WebClientService<Controller = BaseController> + Send>>;

    fn get_storage(&self) -> &impl Storage<T, C, E>;

    async fn handle_validate_config(
        &self,
        identify: T,
        config: NetworkConfig,
    ) -> Result<ValidateConfigResponse, RemoteClientError<E>> {
        let client = self
            .get_rpc_client(identify)
            .ok_or(RemoteClientError::ClientNotFound)?;
        client
            .validate_config(
                BaseController::default(),
                ValidateConfigRequest {
                    config: Some(config),
                },
            )
            .await
            .map_err(RemoteClientError::RpcError)
    }

    async fn handle_run_network_instance(
        &self,
        identify: T,
        config: NetworkConfig,
        save: bool,
    ) -> Result<(), RemoteClientError<E>> {
        self.handle_run_network_instance_with_source(identify, config, save, ConfigSource::User)
            .await
    }

    async fn handle_run_network_instance_with_source(
        &self,
        identify: T,
        config: NetworkConfig,
        save: bool,
        source: ConfigSource,
    ) -> Result<(), RemoteClientError<E>> {
        let client = self
            .get_rpc_client(identify.clone())
            .ok_or(RemoteClientError::ClientNotFound)?;
        let resp = client
            .run_network_instance(
                BaseController::default(),
                RunNetworkInstanceRequest {
                    inst_id: None,
                    config: Some(config.clone()),
                    overwrite: true,
                    source: config_source_to_rpc(source),
                },
            )
            .await?;

        if save {
            self.get_storage()
                .insert_or_update_user_network_config(
                    identify,
                    resp.inst_id.unwrap_or_default().into(),
                    config,
                    source,
                )
                .await
                .map_err(RemoteClientError::PersistentError)?;
        }

        Ok(())
    }

    async fn handle_collect_network_info(
        &self,
        identify: T,
        inst_ids: Option<Vec<uuid::Uuid>>,
    ) -> Result<CollectNetworkInfoResponse, RemoteClientError<E>> {
        let client = self
            .get_rpc_client(identify)
            .ok_or(RemoteClientError::ClientNotFound)?;
        let resp = client
            .collect_network_info(
                BaseController::default(),
                CollectNetworkInfoRequest {
                    inst_ids: inst_ids
                        .unwrap_or_default()
                        .into_iter()
                        .map(|id| id.into())
                        .collect(),
                },
            )
            .await?;

        Ok(resp)
    }

    async fn handle_list_network_instance_ids(
        &self,
        identify: T,
    ) -> Result<ListNetworkInstanceIdsJsonResp, RemoteClientError<E>> {
        let client = self
            .get_rpc_client(identify.clone())
            .ok_or(RemoteClientError::ClientNotFound)?;
        let ret = client
            .list_network_instance(BaseController::default(), ListNetworkInstanceRequest {})
            .await?;

        let running_inst_ids = ret.inst_ids.clone().into_iter().collect();

        let to_proto_uuid = |id: &str| Into::<easytier_proto::common::Uuid>::into(id.to_string());

        // Single All list: derive db_inst_ids + db_disabled_inst_ids in memory
        // (avoids a second DisabledOnly round-trip).
        let all_configs = self
            .get_storage()
            .list_network_configs(identify, ListNetworkProps::All)
            .await
            .map_err(RemoteClientError::PersistentError)?;

        let mut db_inst_ids = Vec::with_capacity(all_configs.len());
        let mut db_disabled_inst_ids = Vec::new();
        for cfg in &all_configs {
            let id = to_proto_uuid(cfg.get_network_inst_id());
            if cfg.is_disabled() {
                db_disabled_inst_ids.push(id);
            }
            db_inst_ids.push(id);
        }

        // Stopped-in-list view: storage rows not currently running on the device.
        let disabled_inst_ids = db_inst_ids
            .iter()
            .filter(|id| !ret.inst_ids.contains(id))
            .cloned()
            .collect::<Vec<_>>();

        Ok(ListNetworkInstanceIdsJsonResp {
            running_inst_ids,
            disabled_inst_ids,
            db_inst_ids,
            db_disabled_inst_ids,
        })
    }

    async fn handle_remove_network_instances(
        &self,
        identify: T,
        inst_ids: Vec<uuid::Uuid>,
    ) -> Result<(), RemoteClientError<E>> {
        if inst_ids.is_empty() {
            return Ok(());
        }
        let client = self
            .get_rpc_client(identify.clone())
            .ok_or(RemoteClientError::ClientNotFound)?;
        self.get_storage()
            .delete_network_configs(identify, &inst_ids)
            .await
            .map_err(RemoteClientError::PersistentError)?;

        client
            .delete_network_instance(
                BaseController::default(),
                DeleteNetworkInstanceRequest {
                    inst_ids: inst_ids.into_iter().map(|id| id.into()).collect(),
                    keep_local_config: false,
                },
            )
            .await?;

        Ok(())
    }

    async fn handle_update_network_state(
        &self,
        identify: T,
        inst_id: uuid::Uuid,
        disabled: bool,
    ) -> Result<(), RemoteClientError<E>> {
        let client = self
            .get_rpc_client(identify.clone())
            .ok_or(RemoteClientError::ClientNotFound)?;

        let (cfg, source) = self
            .handle_get_network_config_with_source(identify.clone(), inst_id)
            .await?;

        if disabled {
            self.get_storage()
                .insert_or_update_user_network_config(
                    identify.clone(),
                    inst_id,
                    cfg.clone(),
                    source,
                )
                .await
                .map_err(RemoteClientError::PersistentError)?;

            client
                .delete_network_instance(
                    BaseController::default(),
                    DeleteNetworkInstanceRequest {
                        inst_ids: vec![inst_id.into()],
                        keep_local_config: true,
                    },
                )
                .await?;
        } else {
            client
                .run_network_instance(
                    BaseController::default(),
                    RunNetworkInstanceRequest {
                        inst_id: Some(inst_id.into()),
                        config: Some(cfg),
                        overwrite: true,
                        source: config_source_to_rpc(source),
                    },
                )
                .await?;
        }

        self.get_storage()
            .update_network_config_state(identify, inst_id, disabled)
            .await
            .map_err(RemoteClientError::PersistentError)?;

        Ok(())
    }

    async fn handle_get_network_metas(
        &self,
        identify: T,
        inst_ids: Vec<uuid::Uuid>,
    ) -> Result<GetNetworkMetasResponse, RemoteClientError<E>> {
        let mut metas = std::collections::HashMap::new();

        if let Some(client) = self.get_rpc_client(identify.clone())
            && let Ok(resp) = client
                .list_network_instance_meta(
                    BaseController::default(),
                    ListNetworkInstanceMetaRequest {
                        inst_ids: inst_ids.iter().cloned().map(|id| id.into()).collect(),
                    },
                )
                .await
        {
            for meta in resp.metas {
                if let Some(inst_id) = meta.inst_id.as_ref() {
                    let inst_id: uuid::Uuid = (*inst_id).into();
                    metas.insert(inst_id, meta);
                }
            }
        }

        let missing: Vec<uuid::Uuid> = inst_ids
            .iter()
            .copied()
            .filter(|id| !metas.contains_key(id))
            .collect();
        // One storage list for all misses instead of N get_network_config calls (S1).
        if !missing.is_empty()
            && let Ok(configs) = self
                .get_storage()
                .list_network_configs(identify.clone(), ListNetworkProps::All)
                .await
        {
            let missing_set: std::collections::HashSet<uuid::Uuid> =
                missing.iter().copied().collect();
            for cfg in configs {
                let Ok(instance_id) = uuid::Uuid::parse_str(cfg.get_network_inst_id()) else {
                    continue;
                };
                if !missing_set.contains(&instance_id) || metas.contains_key(&instance_id) {
                    continue;
                }
                let network_name = cfg
                    .get_network_config()
                    .ok()
                    .and_then(|c| c.network_name)
                    .unwrap_or_default();
                let source = cfg.get_runtime_network_config_source();
                metas.insert(
                    instance_id,
                    NetworkMeta {
                        inst_id: Some(instance_id.into()),
                        network_name: network_name.clone(),
                        config_permission: 0,
                        instance_name: network_name,
                        source: config_source_to_rpc(source),
                    },
                );
            }
        }

        for instance_id in missing {
            if metas.contains_key(&instance_id) {
                continue;
            }
            let (config, source) = self
                .handle_get_network_config_with_source(identify.clone(), instance_id)
                .await?;
            let network_name = config.network_name.unwrap_or_default();
            metas.insert(
                instance_id,
                NetworkMeta {
                    inst_id: Some(instance_id.into()),
                    network_name: network_name.clone(),
                    config_permission: 0,
                    instance_name: network_name,
                    source: config_source_to_rpc(source),
                },
            );
        }

        Ok(GetNetworkMetasResponse { metas })
    }

    async fn handle_save_network_config(
        &self,
        identify: T,
        inst_id: uuid::Uuid,
        config: NetworkConfig,
    ) -> Result<(), RemoteClientError<E>> {
        self.handle_save_network_config_with_source(identify, inst_id, config, ConfigSource::User)
            .await
    }

    async fn handle_save_network_config_with_source(
        &self,
        identify: T,
        inst_id: uuid::Uuid,
        config: NetworkConfig,
        source: ConfigSource,
    ) -> Result<(), RemoteClientError<E>> {
        self.get_storage()
            .insert_or_update_user_network_config(identify.clone(), inst_id, config, source)
            .await
            .map_err(RemoteClientError::PersistentError)?;
        self.get_storage()
            .update_network_config_state(identify, inst_id, true)
            .await
            .map_err(RemoteClientError::PersistentError)?;
        Ok(())
    }

    async fn handle_get_network_config(
        &self,
        identify: T,
        inst_id: uuid::Uuid,
    ) -> Result<NetworkConfig, RemoteClientError<E>> {
        self.handle_get_network_config_with_source(identify, inst_id)
            .await
            .map(|(config, _)| config)
    }

    async fn handle_get_network_config_with_source(
        &self,
        identify: T,
        inst_id: uuid::Uuid,
    ) -> Result<(NetworkConfig, ConfigSource), RemoteClientError<E>> {
        // Default: RPC first (live runtime config), storage fallback.
        // GUI Windows `persist_runtime_dev_name` depends on this order: storage often
        // still has an empty `dev_name` while the running instance already has an
        // auto-assigned wintun name. Web console overrides this method to prefer
        // SQLite (authoritative for managed edits) — see ClientManager in easytier-web.
        if let Some(client) = self.get_rpc_client(identify.clone())
            && let Ok(resp) = client
                .get_network_instance_config(
                    BaseController::default(),
                    GetNetworkInstanceConfigRequest {
                        inst_id: Some(inst_id.into()),
                    },
                )
                .await
            && let Some(config) = resp.config
        {
            let source = if let Some(source) = config_source_from_rpc(resp.source) {
                source
            } else {
                self.get_storage()
                    .get_network_config(identify.clone(), &inst_id.to_string())
                    .await
                    .map_err(RemoteClientError::PersistentError)?
                    .map(|cfg| cfg.get_runtime_network_config_source())
                    .unwrap_or(ConfigSource::User)
            };
            return Ok((config, source));
        }

        let inst_id = inst_id.to_string();

        let db_row = self
            .get_storage()
            .get_network_config(identify, &inst_id)
            .await
            .map_err(RemoteClientError::PersistentError)?
            .ok_or(RemoteClientError::NotFound(format!(
                "No such network instance: {}",
                inst_id
            )))?;

        Ok((
            db_row
                .get_network_config()
                .map_err(RemoteClientError::PersistentError)?,
            db_row.get_runtime_network_config_source(),
        ))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteClientError<E> {
    #[error("Client not found")]
    ClientNotFound,
    #[error("Not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    RpcError(#[from] easytier_proto::rpc_types::error::Error),
    #[error(transparent)]
    PersistentError(E),
    #[error("Other error: {0}")]
    Other(String),
}

pub enum ListNetworkProps {
    All,
    EnabledOnly,
    DisabledOnly,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct ListNetworkInstanceIdsJsonResp {
    running_inst_ids: Vec<easytier_proto::common::Uuid>,
    disabled_inst_ids: Vec<easytier_proto::common::Uuid>,
    /// All storage-backed instance ids for this machine (console SQLite / GUIStorage).
    /// Used with `running_inst_ids` to detect device-only residuals (`∈ R ∧ ∉ DB`).
    #[serde(default)]
    db_inst_ids: Vec<easytier_proto::common::Uuid>,
    /// Storage rows with `disabled=true` (may still be running on the device).
    #[serde(default)]
    db_disabled_inst_ids: Vec<easytier_proto::common::Uuid>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct GetNetworkMetasResponse {
    metas: std::collections::HashMap<uuid::Uuid, NetworkMeta>,
}

pub trait PersistentConfig<E> {
    fn get_network_inst_id(&self) -> &str;
    fn get_network_config(&self) -> Result<NetworkConfig, E>;
    fn get_network_config_source(&self) -> ConfigSource;
    fn get_runtime_network_config_source(&self) -> ConfigSource {
        self.get_network_config_source()
    }
    /// Storage `disabled=true` (may still be running on the device).
    /// Default `false` for storages that track enablement elsewhere (e.g. GUI).
    fn is_disabled(&self) -> bool {
        false
    }
}

#[async_trait]
pub trait Storage<T, C, E>: Send + Sync
where
    C: PersistentConfig<E>,
{
    async fn insert_or_update_user_network_config(
        &self,
        identify: T,
        network_inst_id: Uuid,
        network_config: NetworkConfig,
        source: ConfigSource,
    ) -> Result<(), E>;

    async fn delete_network_configs(&self, identify: T, network_inst_ids: &[Uuid])
    -> Result<(), E>;

    async fn update_network_config_state(
        &self,
        identify: T,
        network_inst_id: Uuid,
        disabled: bool,
    ) -> Result<(), E>;

    async fn list_network_configs(&self, identify: T, props: ListNetworkProps)
    -> Result<Vec<C>, E>;

    async fn get_network_config(&self, identify: T, network_inst_id: &str) -> Result<Option<C>, E>;
}
