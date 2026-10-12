use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use easytier_proto::{
    api::manage::{
        CollectNetworkInfoRequest, CollectNetworkInfoResponse, DeleteNetworkInstanceRequest,
        DeleteNetworkInstanceResponse, GetConfigServerStatusRequest, GetConfigServerStatusResponse,
        GetNetworkInstanceConfigRequest, GetNetworkInstanceConfigResponse,
        ListNetworkInstanceMetaRequest, ListNetworkInstanceMetaResponse,
        ListNetworkInstanceRequest, ListNetworkInstanceResponse, NetworkInstanceRunningInfoMap,
        NetworkMeta, ReportManagedNetworkConfigRequest, ReportManagedNetworkConfigResponse,
        RetainNetworkInstanceRequest, RetainNetworkInstanceResponse, RunNetworkInstanceRequest,
        RunNetworkInstanceResponse, ValidateConfigRequest, ValidateConfigResponse,
        WebClientService,
    },
    rpc_types::{self, controller::BaseController},
};

use crate::{
    config::{
        api::network_config_from_toml,
        api_input::NetworkConfigExt as _,
        toml::{ConfigLoader as _, ConfigSource, TomlConfig},
    },
    instance::{CoreInstance, CoreInstanceHost, manager::InstanceFactory},
};

use super::{
    ConfigFileControl, ConfigFilePermission, InstanceManager, config_server_client,
    config_server_status, config_source_from_rpc, config_source_to_rpc,
    network_instance_running_info,
};

#[async_trait::async_trait]
pub trait InstanceMutationHooks: Send + Sync + 'static {
    fn manages_remote_config_instances(&self) -> bool {
        false
    }

    async fn pre_run_network_instance(&self, _config: &TomlConfig) -> Result<(), String> {
        Ok(())
    }

    async fn post_run_network_instance(&self, _instance_id: &uuid::Uuid) -> Result<(), String> {
        Ok(())
    }

    /// Console/managed overwrite replaced a running (or prior) instance.
    /// Hosts may show a "synced newer config" tip; failures must not roll back.
    async fn on_managed_config_overwrite(&self, _instance_id: &uuid::Uuid) -> Result<(), String> {
        Ok(())
    }

    /// Called after runtime instances are removed.
    ///
    /// `keep_local_config` mirrors `DeleteNetworkInstanceRequest.keep_local_config`
    /// in `api_manage.proto` (canonical Stop vs Delete contract):
    /// - `true`: console/app Stop — retain local GUI/app config; hosts should
    ///   emit stop/disable semantics (not purge).
    /// - `false`: console Delete / reconcile — purge local config.
    ///
    /// Breaking for implementors that override this method: the second parameter
    /// was added so Stop and Delete can diverge in Host adapters (FFI/GUI/ohrs).
    async fn post_remove_network_instances(
        &self,
        _instance_ids: &[uuid::Uuid],
        _keep_local_config: bool,
    ) -> Result<(), String> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl InstanceMutationHooks for () {}

/// Host Adapter for configuration-file effects used by process management.
#[async_trait::async_trait]
pub trait ConfigFileStorage: Send + Sync + 'static {
    async fn inspect(&self, path: &Path) -> ConfigFileControl;

    async fn read(&self, path: &Path) -> anyhow::Result<Option<Vec<u8>>>;

    /// Atomically replaces the file and restricts newly created files to the
    /// current user when the Host supports file permissions.
    async fn write(&self, path: &Path, contents: &[u8]) -> anyhow::Result<()>;

    async fn remove(&self, path: &Path) -> anyhow::Result<()>;
}

#[derive(Default)]
pub struct UnsupportedConfigFileStorage;

#[async_trait::async_trait]
impl ConfigFileStorage for UnsupportedConfigFileStorage {
    async fn inspect(&self, path: &Path) -> ConfigFileControl {
        ConfigFileControl::new(
            Some(path.to_owned()),
            ConfigFilePermission::from(ConfigFilePermission::READ_ONLY),
        )
    }

    async fn read(&self, _path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
        anyhow::bail!("configuration-file storage is unsupported by this Host")
    }

    async fn write(&self, _path: &Path, _contents: &[u8]) -> anyhow::Result<()> {
        anyhow::bail!("configuration-file storage is unsupported by this Host")
    }

    async fn remove(&self, _path: &Path) -> anyhow::Result<()> {
        anyhow::bail!("configuration-file storage is unsupported by this Host")
    }
}

/// Result of one process-level removal transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceMutationResult {
    pub remaining_instance_ids: Vec<uuid::Uuid>,
    pub removed_instance_ids: Vec<uuid::Uuid>,
}

/// Transport-independent process-level Instance management.
pub struct ProcessManagement<F>
where
    F: InstanceFactory,
{
    instances: Arc<InstanceManager<F>>,
    hooks: Arc<dyn InstanceMutationHooks>,
    storage: Arc<dyn ConfigFileStorage>,
    mutation_lock: Arc<tokio::sync::Mutex<()>>,
}

impl<F> Clone for ProcessManagement<F>
where
    F: InstanceFactory,
{
    fn clone(&self) -> Self {
        Self {
            instances: self.instances.clone(),
            hooks: self.hooks.clone(),
            storage: self.storage.clone(),
            mutation_lock: self.mutation_lock.clone(),
        }
    }
}

impl<F, H> ProcessManagement<F>
where
    F: InstanceFactory<Instance = CoreInstance<H>, CreateContext = ()>,
    F::Error: std::fmt::Debug + std::fmt::Display + Send + Sync + 'static,
    H: CoreInstanceHost,
{
    pub fn new(
        instances: Arc<InstanceManager<F>>,
        hooks: Arc<dyn InstanceMutationHooks>,
        storage: Arc<dyn ConfigFileStorage>,
    ) -> Self {
        let mutation_lock = instances.mutation_lock();
        Self {
            instances,
            hooks,
            storage,
            mutation_lock,
        }
    }

    async fn mark_user_disabled_web_instances(&self, instance_ids: &[uuid::Uuid]) {
        for instance_id in instance_ids {
            if self.instances.config_source(*instance_id) == Some(ConfigSource::Web) {
                self.instances
                    .mark_user_disabled_web_instance(*instance_id)
                    .await;
            }
        }
    }

    async fn clear_user_disabled_web_instances(&self, instance_ids: &[uuid::Uuid]) {
        for instance_id in instance_ids {
            self.instances
                .clear_user_disabled_web_instance(*instance_id)
                .await;
        }
    }

    async fn is_remote_removable(&self, control: &ConfigFileControl) -> bool {
        if control.is_read_only() || !control.is_deletable() {
            return false;
        }
        let Some(path) = control.path.as_deref() else {
            return true;
        };
        !self.storage.inspect(path).await.is_read_only()
    }

    async fn ensure_overwritable(
        &self,
        instance_id: uuid::Uuid,
        control: &ConfigFileControl,
        require_deletable: bool,
    ) -> anyhow::Result<()> {
        if control.is_read_only() {
            anyhow::bail!("instance {instance_id} is read-only, cannot be overwritten");
        }
        if require_deletable && !control.is_deletable() {
            anyhow::bail!("instance {instance_id} is no-delete, cannot be overwritten");
        }
        if let Some(path) = control.path.as_deref()
            && self.storage.inspect(path).await.is_read_only()
        {
            anyhow::bail!(
                "config file {} is read-only, cannot be overwritten",
                path.display()
            );
        }
        Ok(())
    }

    async fn apply_file_cleanup(&self, cleanup: ConfigFileCleanup) {
        let result = match cleanup {
            ConfigFileCleanup::Remove(path) => self.storage.remove(&path).await,
            ConfigFileCleanup::Restore { path, contents } => {
                self.storage.write(&path, &contents).await
            }
        };
        if let Err(error) = result {
            tracing::warn!(%error, "failed to roll back configuration file");
        }
    }

    async fn rollback_started_instance(
        &self,
        started_instance_id: Option<uuid::Uuid>,
        file_cleanup: Option<ConfigFileCleanup>,
        restore_instance: Option<(TomlConfig, ConfigFileControl)>,
    ) {
        if let Some(instance_id) = started_instance_id
            && let Err(error) = self.instances.delete_network_instances([instance_id]).await
        {
            tracing::warn!(%error, "failed to remove rolled-back instance");
            return;
        }
        if let Some(cleanup) = file_cleanup {
            self.apply_file_cleanup(cleanup).await;
        }
        if let Some((config, control)) = restore_instance
            && let Err(error) = self.instances.run_network_instance(config, control)
        {
            tracing::warn!(%error, "failed to restore overwritten instance");
        }
    }

    pub async fn run_network_instance(
        &self,
        config: TomlConfig,
        requested_id: Option<uuid::Uuid>,
        overwrite: bool,
        requested_source: Option<ConfigSource>,
    ) -> anyhow::Result<uuid::Uuid> {
        let mut instance_id = config.get_id();
        if let Some(requested_id) = requested_id {
            instance_id = requested_id;
            config.set_id(instance_id);
        }
        let _mutation = self.mutation_lock.lock().await;
        if overwrite {
            self.instances
                .clear_user_disabled_web_instance(instance_id)
                .await;
        } else if self.instances.is_user_disabled_web_instance(instance_id) {
            tracing::info!(
                %instance_id,
                "skip auto-run of user-disabled web-managed instance"
            );
            return Ok(instance_id);
        }
        let remote_managed = self.hooks.manages_remote_config_instances();

        let mut replacing = false;
        let mut restore_instance = None;
        let mut control = if let Some(control) = self.instances.config_control(instance_id) {
            let existing_source = self.instances.config_source(instance_id);
            let error_message = self
                .instances
                .network_info(instance_id)
                .await
                .and_then(|info| info.error_msg)
                .unwrap_or_default();
            if !overwrite && error_message.is_empty() {
                return Ok(instance_id);
            }
            self.ensure_overwritable(instance_id, &control, remote_managed)
                .await?;
            config.set_network_config_source(requested_source.or(existing_source));
            replacing = true;
            restore_instance = self
                .instances
                .config(instance_id)
                .map(|config| (config, control.clone()));
            control
        } else if let Some(config_dir) = self.instances.config_dir() {
            config.set_network_config_source(requested_source);
            ConfigFileControl::new(
                Some(config_dir.join(format!("{instance_id}.toml"))),
                ConfigFilePermission::default(),
            )
        } else {
            config.set_network_config_source(requested_source);
            ConfigFileControl::new(None, ConfigFilePermission::default())
        };

        self.hooks
            .pre_run_network_instance(&config)
            .await
            .map_err(|error| anyhow::anyhow!("pre-run hook failed: {error}"))?;

        if replacing {
            self.ensure_overwritable(instance_id, &control, remote_managed)
                .await?;
        }

        let mut file_cleanup = None;
        if !control.is_read_only()
            && let Some(path) = control.path.as_deref()
        {
            let cleanup = match self.storage.read(path).await {
                Ok(Some(contents)) => Some(ConfigFileCleanup::Restore {
                    path: path.to_owned(),
                    contents,
                }),
                Ok(None) => Some(ConfigFileCleanup::Remove(path.to_owned())),
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "failed to back up config file {} before overwrite: {error}",
                        path.display()
                    ));
                }
            };
            if let Err(error) = self.storage.write(path, config.dump().as_bytes()).await {
                tracing::warn!(%error, path = %path.display(), "failed to write config file");
                control.set_read_only(true);
            } else {
                file_cleanup = cleanup;
            }
        }

        // Overwrite: detach under lock, await TUN stop outside, then re-lock to run.
        // New starts stay under one lock hold (no stop to await).
        let pending_replace = if replacing {
            match self.instances.detach_network_instances([instance_id]) {
                Ok((_, pending)) => pending,
                Err(error) => {
                    self.rollback_started_instance(None, file_cleanup, restore_instance)
                        .await;
                    return Err(error);
                }
            }
        } else {
            crate::instance::manager::PendingInstanceStops::empty()
        };

        if !replacing {
            let started = self
                .start_network_instance_locked(
                    instance_id,
                    config,
                    control,
                    file_cleanup,
                    restore_instance,
                    remote_managed,
                    overwrite,
                )
                .await?;
            // Host hooks / revision IO must not hold mutation_lock (same as
            // detach-join-outside for overwrite stops).
            drop(_mutation);
            return self.after_run_network_instance(started).await;
        }

        // Drop mutation_lock before awaiting TUN teardown.
        drop(_mutation);
        if let Err(error) = pending_replace.join().await {
            self.rollback_started_instance(None, file_cleanup, restore_instance)
                .await;
            return Err(error);
        }

        let _mutation = self.mutation_lock.lock().await;
        // Rare race: another mutator may have revived this id after our stop
        // completed. Do not return Ok — that would claim overwrite success while
        // leaving the concurrent instance running. Re-delete under this lock
        // (await is OK on this rare path) then start ours.
        if self.instances.instance(instance_id).is_some()
            && let Err(error) = self.instances.delete_network_instances([instance_id]).await
        {
            self.rollback_started_instance(None, file_cleanup, restore_instance)
                .await;
            return Err(error);
        }
        let started = self
            .start_network_instance_locked(
                instance_id,
                config,
                control,
                file_cleanup,
                restore_instance,
                remote_managed,
                overwrite,
            )
            .await?;
        drop(_mutation);
        self.after_run_network_instance(started).await
    }

    /// Start under `mutation_lock`. Hooks / revision persist happen in
    /// [`Self::after_run_network_instance`] after the lock is released.
    #[allow(clippy::too_many_arguments)]
    async fn start_network_instance_locked(
        &self,
        instance_id: uuid::Uuid,
        config: TomlConfig,
        control: ConfigFileControl,
        file_cleanup: Option<ConfigFileCleanup>,
        restore_instance: Option<(TomlConfig, ConfigFileControl)>,
        remote_managed: bool,
        overwrite: bool,
    ) -> anyhow::Result<StartedNetworkRun> {
        let is_web_source = config.get_network_config_source() == ConfigSource::Web;
        if let Err(error) = self.instances.run_network_instance(config, control) {
            self.rollback_started_instance(None, file_cleanup, restore_instance)
                .await;
            return Err(error);
        }
        Ok(StartedNetworkRun {
            instance_id,
            is_web_source,
            overwrite,
            remote_managed,
            file_cleanup,
            restore_instance,
        })
    }

    async fn after_run_network_instance(
        &self,
        started: StartedNetworkRun,
    ) -> anyhow::Result<uuid::Uuid> {
        let StartedNetworkRun {
            instance_id,
            is_web_source,
            overwrite,
            remote_managed,
            file_cleanup,
            restore_instance,
        } = started;

        if let Err(error) = self.hooks.post_run_network_instance(&instance_id).await {
            if remote_managed {
                let _mutation = self.mutation_lock.lock().await;
                self.rollback_started_instance(Some(instance_id), file_cleanup, restore_instance)
                    .await;
                return Err(anyhow::anyhow!("post-run hook failed: {error}"));
            }
            tracing::warn!(%error, "post-run hook failed");
        }
        // Tip only: never roll back a successful overwrite for host UI hooks.
        if overwrite && let Err(error) = self.hooks.on_managed_config_overwrite(&instance_id).await
        {
            tracing::info!(
                %instance_id,
                %error,
                "managed config overwrite tip hook failed"
            );
        }
        // Boot-gate milestone: persist only for Web-source applies. User
        // overwrite must not advance disk revision (next boot would skip defer
        // while local web tomls may still be stale).
        if is_web_source {
            config_server_client::persist_active_managed_config_revision();
        }
        Ok(instance_id)
    }

    pub async fn retain_network_instances(
        &self,
        retained: Vec<uuid::Uuid>,
    ) -> anyhow::Result<InstanceMutationResult> {
        let (result, pending_stops) = {
            let _mutation = self.mutation_lock.lock().await;
            self.retain_network_instances_locked(retained).await?
        };
        pending_stops.join().await?;
        Ok(result)
    }

    /// Resolves retained names and mutates the collection in one transaction.
    pub async fn retain_owned_network_instances_by_name(
        &self,
        retained_names: Vec<String>,
    ) -> anyhow::Result<InstanceMutationResult> {
        self.retain_owned_network_instances_by_name_gated(retained_names, |_| Ok::<(), String>(()))
            .await
    }

    /// Like [`Self::retain_owned_network_instances_by_name`], but runs `gate` on
    /// the ids that would be dropped **under** `mutation_lock`.
    pub async fn retain_owned_network_instances_by_name_gated<E>(
        &self,
        retained_names: Vec<String>,
        gate: impl FnOnce(&[uuid::Uuid]) -> Result<(), E>,
    ) -> anyhow::Result<InstanceMutationResult>
    where
        E: std::fmt::Display,
    {
        let (result, pending_stops) = {
            let _mutation = self.mutation_lock.lock().await;
            let retained = self.resolve_instance_ids_by_name(&retained_names)?;
            let retained_set = retained.iter().copied().collect::<HashSet<_>>();
            let to_drop = self
                .instances
                .instance_ids()
                .into_iter()
                .filter(|id| !retained_set.contains(id))
                .collect::<Vec<_>>();
            gate(&to_drop).map_err(|e| anyhow::anyhow!("{e}"))?;
            self.retain_network_instances_locked(retained).await?
        };
        pending_stops.join().await?;
        Ok(result)
    }

    async fn retain_network_instances_locked(
        &self,
        retained: Vec<uuid::Uuid>,
    ) -> anyhow::Result<(
        InstanceMutationResult,
        crate::instance::manager::PendingInstanceStops,
    )> {
        let before = self.instances.instance_ids();
        if !self.hooks.manages_remote_config_instances() {
            let removed = before
                .iter()
                .copied()
                .filter(|id| !retained.contains(id))
                .collect::<Vec<_>>();
            // Retain drops non-kept instances (keep_local_config=false) — clear
            // disabled marks, same as hard Delete.
            self.clear_user_disabled_web_instances(&removed).await;
            let (remaining, pending_stops) = self.instances.detach_network_instances(removed)?;
            let remaining_set = remaining.iter().copied().collect::<HashSet<_>>();
            let removed = before
                .into_iter()
                .filter(|id| !remaining_set.contains(id))
                .collect::<Vec<_>>();
            self.notify_removed_instances(&removed, false).await?;
            return Ok((
                InstanceMutationResult {
                    removed_instance_ids: removed,
                    remaining_instance_ids: remaining,
                },
                pending_stops,
            ));
        }

        let mut retained = retained.into_iter().collect::<HashSet<_>>();
        let mut removed = Vec::new();
        for instance in self.instances.instances() {
            let instance_id = instance.instance_id();
            if retained.contains(&instance_id) {
                continue;
            }
            let Some(control) = self.instances.config_control(instance_id) else {
                continue;
            };
            if self.is_remote_removable(&control).await {
                removed.push(instance_id);
            } else {
                retained.insert(instance_id);
            }
        }
        self.clear_user_disabled_web_instances(&removed).await;
        let (remaining, pending_stops) =
            self.instances.detach_network_instances(removed.clone())?;
        self.notify_removed_instances(&removed, false).await?;
        Ok((
            InstanceMutationResult {
                remaining_instance_ids: remaining,
                removed_instance_ids: removed,
            },
            pending_stops,
        ))
    }

    pub async fn delete_network_instances(
        &self,
        requested: Vec<uuid::Uuid>,
        keep_local_config: bool,
    ) -> anyhow::Result<InstanceMutationResult> {
        let remote_managed = self.hooks.manages_remote_config_instances();
        let (removed, remaining, pending_stops, files) = {
            let _mutation = self.mutation_lock.lock().await;
            let requested = requested.into_iter().collect::<HashSet<_>>();
            let mut removed = Vec::new();
            // (instance_id, path) so purge can skip ids revived by a concurrent Run.
            let mut files = Vec::<(uuid::Uuid, PathBuf)>::new();
            for instance in self.instances.instances() {
                let instance_id = instance.instance_id();
                if !requested.contains(&instance_id) {
                    continue;
                }
                let Some(control) = self.instances.config_control(instance_id) else {
                    continue;
                };
                let removable = if remote_managed {
                    self.is_remote_removable(&control).await
                } else {
                    control.is_deletable()
                };
                if removable {
                    removed.push(instance_id);
                    if let Some(path) = control.path {
                        files.push((instance_id, path));
                    }
                }
            }
            // Stop (keep=true): mark web instances intentionally disabled for heartbeat.
            // Delete (keep=false): drop disabled marks so the set does not retain purged ids.
            // Semantics: see `InstanceMutationHooks::post_remove_network_instances` / proto field.
            if keep_local_config {
                self.mark_user_disabled_web_instances(&removed).await;
            } else {
                self.clear_user_disabled_web_instances(&removed).await;
            }
            // Detach under lock (hooks see removed set); await TUN stop outside lock.
            let (remaining, pending_stops) =
                self.instances.detach_network_instances(removed.clone())?;
            self.notify_removed_instances(&removed, keep_local_config)
                .await?;
            (removed, remaining, pending_stops, files)
        };
        pending_stops.join().await?;
        // File purge after TUN stop. Re-take the lock so a concurrent overwrite/run
        // that revived the same id (and rewrote its toml) cannot have its file
        // unlinked. Unlink itself is fast; the expensive wait already finished.
        if !keep_local_config && !files.is_empty() {
            let _mutation = self.mutation_lock.lock().await;
            for (instance_id, path) in files {
                if self.instances.instance(instance_id).is_some() {
                    continue;
                }
                if remote_managed && self.storage.inspect(&path).await.is_read_only() {
                    continue;
                }
                if let Err(error) = self.storage.remove(&path).await {
                    tracing::warn!(%error, path = %path.display(), "failed to remove config file");
                }
            }
        }
        Ok(InstanceMutationResult {
            remaining_instance_ids: remaining,
            removed_instance_ids: removed,
        })
    }

    /// Starts one caller-owned Instance while preserving its static control.
    pub async fn run_owned_network_instance(
        &self,
        config: TomlConfig,
        control: ConfigFileControl,
    ) -> anyhow::Result<uuid::Uuid> {
        let _mutation = self.mutation_lock.lock().await;
        let instance_id = config.get_id();
        if self.instances.instance(instance_id).is_some() {
            anyhow::bail!("instance {instance_id} already exists");
        }
        let instance_name = config.get_inst_name();
        if super::resolve_optional_instance_by_name(self.instances.as_ref(), &instance_name)?
            .is_some()
        {
            anyhow::bail!("instance name {instance_name} already exists");
        }
        // Local intentional start clears any prior intentional-stop mark so an
        // unexpected later exit can be auto-recovered by the config server.
        self.instances
            .clear_user_disabled_web_instance(instance_id)
            .await;
        self.instances.run_network_instance(config, control)
    }

    /// Removes caller-owned Instances without applying remote config-file policy.
    pub async fn delete_owned_network_instances(
        &self,
        requested: Vec<uuid::Uuid>,
    ) -> anyhow::Result<InstanceMutationResult> {
        self.delete_owned_network_instances_selected_by(|| requested)
            .await
    }

    /// Selects caller-owned Instances and removes them under one lock.
    pub async fn delete_owned_network_instances_selected_by(
        &self,
        select: impl FnOnce() -> Vec<uuid::Uuid>,
    ) -> anyhow::Result<InstanceMutationResult> {
        let (result, pending_stops) = {
            let _mutation = self.mutation_lock.lock().await;
            let requested = select();
            self.delete_owned_network_instances_locked(requested)
                .await?
        };
        pending_stops.join().await?;
        Ok(result)
    }

    /// Resolves requested names and removes them in one transaction.
    pub async fn delete_owned_network_instances_by_name(
        &self,
        requested_names: Vec<String>,
    ) -> anyhow::Result<InstanceMutationResult> {
        self.delete_owned_network_instances_by_name_gated(requested_names, |_| Ok::<(), String>(()))
            .await
    }

    /// Like [`Self::delete_owned_network_instances_by_name`], but runs `gate` on
    /// the resolved ids **under** `mutation_lock` (closes the check-then-act
    /// window used by FFI managed-delete rejection).
    pub async fn delete_owned_network_instances_by_name_gated<E>(
        &self,
        requested_names: Vec<String>,
        gate: impl FnOnce(&[uuid::Uuid]) -> Result<(), E>,
    ) -> anyhow::Result<InstanceMutationResult>
    where
        E: std::fmt::Display,
    {
        let (result, pending_stops) = {
            let _mutation = self.mutation_lock.lock().await;
            let requested = self.resolve_instance_ids_by_name(&requested_names)?;
            gate(&requested).map_err(|e| anyhow::anyhow!("{e}"))?;
            self.delete_owned_network_instances_locked(requested)
                .await?
        };
        pending_stops.join().await?;
        Ok(result)
    }

    async fn delete_owned_network_instances_locked(
        &self,
        requested: Vec<uuid::Uuid>,
    ) -> anyhow::Result<(
        InstanceMutationResult,
        crate::instance::manager::PendingInstanceStops,
    )> {
        let before = self.instances.instance_ids();
        let before_set = before.iter().copied().collect::<HashSet<_>>();
        let to_remove = requested
            .into_iter()
            .filter(|id| before_set.contains(id))
            .collect::<Vec<_>>();
        // Intentional local stop (FFI/Android/iOS): mark web-managed instances
        // before detach so heartbeat can sync `disabled` and skip auto-run.
        // Only mark ids that are actually present (config_source still resolvable).
        self.mark_user_disabled_web_instances(&to_remove).await;
        let (remaining, pending_stops) = self.instances.detach_network_instances(to_remove)?;
        let remaining_set = remaining.iter().copied().collect::<HashSet<_>>();
        let removed = before
            .into_iter()
            .filter(|id| !remaining_set.contains(id))
            .collect::<Vec<_>>();
        // Local intentional stop → keep_local_config=true (see trait/proto).
        self.notify_removed_instances(&removed, true).await?;
        Ok((
            InstanceMutationResult {
                removed_instance_ids: removed,
                remaining_instance_ids: remaining,
            },
            pending_stops,
        ))
    }

    fn resolve_instance_ids_by_name(&self, names: &[String]) -> anyhow::Result<Vec<uuid::Uuid>> {
        names
            .iter()
            .map(|name| {
                super::resolve_optional_instance_by_name(self.instances.as_ref(), name)
                    .map(|instance| instance.map(|instance| instance.instance_id()))
            })
            .filter_map(Result::transpose)
            .collect()
    }

    async fn notify_removed_instances(
        &self,
        removed: &[uuid::Uuid],
        keep_local_config: bool,
    ) -> anyhow::Result<()> {
        if let Err(error) = self
            .hooks
            .post_remove_network_instances(removed, keep_local_config)
            .await
        {
            if self.hooks.manages_remote_config_instances() {
                anyhow::bail!("post-remove hook failed: {error}");
            }
            tracing::warn!(%error, "post-remove hook failed");
        }
        Ok(())
    }
}

enum ConfigFileCleanup {
    Remove(PathBuf),
    Restore { path: PathBuf, contents: Vec<u8> },
}

struct StartedNetworkRun {
    instance_id: uuid::Uuid,
    is_web_source: bool,
    overwrite: bool,
    remote_managed: bool,
    file_cleanup: Option<ConfigFileCleanup>,
    restore_instance: Option<(TomlConfig, ConfigFileControl)>,
}

/// Protobuf projection over transport-independent process management.
pub struct ProcessManagementRpc<F>
where
    F: InstanceFactory,
{
    management: ProcessManagement<F>,
}

impl<F> Clone for ProcessManagementRpc<F>
where
    F: InstanceFactory,
{
    fn clone(&self) -> Self {
        Self {
            management: self.management.clone(),
        }
    }
}

impl<F, H> ProcessManagementRpc<F>
where
    F: InstanceFactory<Instance = CoreInstance<H>, CreateContext = ()>,
    F::Error: std::fmt::Debug + std::fmt::Display + Send + Sync + 'static,
    H: CoreInstanceHost,
{
    pub fn new(
        instances: Arc<InstanceManager<F>>,
        hooks: Arc<dyn InstanceMutationHooks>,
        storage: Arc<dyn ConfigFileStorage>,
    ) -> Self {
        Self {
            management: ProcessManagement::new(instances, hooks, storage),
        }
    }
}

#[async_trait::async_trait]
impl<F, H> WebClientService for ProcessManagementRpc<F>
where
    F: InstanceFactory<Instance = CoreInstance<H>, CreateContext = ()>,
    F::Error: std::fmt::Debug + std::fmt::Display + Send + Sync + 'static,
    H: CoreInstanceHost,
{
    type Controller = BaseController;

    async fn validate_config(
        &self,
        _: BaseController,
        request: ValidateConfigRequest,
    ) -> rpc_types::error::Result<ValidateConfigResponse> {
        Ok(ValidateConfigResponse {
            toml_config: request.config.unwrap_or_default().gen_config()?.dump(),
        })
    }

    async fn run_network_instance(
        &self,
        _: BaseController,
        request: RunNetworkInstanceRequest,
    ) -> rpc_types::error::Result<RunNetworkInstanceResponse> {
        let config = request
            .config
            .ok_or_else(|| anyhow::anyhow!("config is required"))?
            .gen_config()?;
        let requested_id = request.inst_id.map(Into::into);
        let requested_source = config_source_from_rpc(request.source);
        let management = self.management.clone();
        let instance_id = tokio::spawn(async move {
            management
                .run_network_instance(config, requested_id, request.overwrite, requested_source)
                .await
        })
        .await
        .map_err(|error| anyhow::anyhow!("instance mutation task failed: {error}"))??;
        Ok(RunNetworkInstanceResponse {
            inst_id: Some(instance_id.into()),
        })
    }

    async fn retain_network_instance(
        &self,
        _: BaseController,
        request: RetainNetworkInstanceRequest,
    ) -> rpc_types::error::Result<RetainNetworkInstanceResponse> {
        let retained = request.inst_ids.into_iter().map(Into::into).collect();
        let management = self.management.clone();
        let result =
            tokio::spawn(async move { management.retain_network_instances(retained).await })
                .await
                .map_err(|error| anyhow::anyhow!("instance mutation task failed: {error}"))??;
        Ok(RetainNetworkInstanceResponse {
            remain_inst_ids: result
                .remaining_instance_ids
                .into_iter()
                .map(Into::into)
                .collect(),
        })
    }

    async fn collect_network_info(
        &self,
        _: BaseController,
        request: CollectNetworkInfoRequest,
    ) -> rpc_types::error::Result<CollectNetworkInfoResponse> {
        let included = request
            .inst_ids
            .into_iter()
            .map(uuid::Uuid::from)
            .collect::<HashSet<_>>();
        let map = if included.is_empty() {
            self.management
                .instances
                .collect_network_infos()
                .await?
                .into_iter()
                .map(|(id, info)| (id.to_string(), info))
                .collect()
        } else {
            let mut map = std::collections::BTreeMap::new();
            for instance_id in included {
                let Some(instance) = self.management.instances.instance(instance_id) else {
                    continue;
                };
                map.insert(
                    instance_id.to_string(),
                    network_instance_running_info(instance.as_ref()).await?,
                );
            }
            map
        };
        Ok(CollectNetworkInfoResponse {
            info: Some(NetworkInstanceRunningInfoMap { map }),
        })
    }

    async fn list_network_instance(
        &self,
        _: BaseController,
        _: ListNetworkInstanceRequest,
    ) -> rpc_types::error::Result<ListNetworkInstanceResponse> {
        Ok(ListNetworkInstanceResponse {
            inst_ids: self
                .management
                .instances
                .instance_ids()
                .into_iter()
                .map(Into::into)
                .collect(),
        })
    }

    async fn delete_network_instance(
        &self,
        _: BaseController,
        request: DeleteNetworkInstanceRequest,
    ) -> rpc_types::error::Result<DeleteNetworkInstanceResponse> {
        let requested = request.inst_ids.into_iter().map(Into::into).collect();
        let keep_local_config = request.keep_local_config;
        let management = self.management.clone();
        let result = tokio::spawn(async move {
            management
                .delete_network_instances(requested, keep_local_config)
                .await
        })
        .await
        .map_err(|error| anyhow::anyhow!("instance mutation task failed: {error}"))??;
        Ok(DeleteNetworkInstanceResponse {
            remain_inst_ids: result
                .remaining_instance_ids
                .into_iter()
                .map(Into::into)
                .collect(),
        })
    }

    async fn get_network_instance_config(
        &self,
        _: BaseController,
        request: GetNetworkInstanceConfigRequest,
    ) -> rpc_types::error::Result<GetNetworkInstanceConfigResponse> {
        let instance_id = request
            .inst_id
            .ok_or_else(|| anyhow::anyhow!("instance id is required"))?
            .into();
        let control = self
            .management
            .instances
            .config_control(instance_id)
            .ok_or_else(|| anyhow::anyhow!("instance config control not found"))?;
        if control.is_read_only() {
            return Err(
                anyhow::anyhow!("configuration for instance {instance_id} is read-only").into(),
            );
        }
        Ok(GetNetworkInstanceConfigResponse {
            config: self
                .management
                .instances
                .config(instance_id)
                .map(|config| network_config_from_toml(&config)),
            source: config_source_to_rpc(
                self.management
                    .instances
                    .config_source(instance_id)
                    .unwrap_or(ConfigSource::User),
            ),
        })
    }

    async fn list_network_instance_meta(
        &self,
        _: BaseController,
        request: ListNetworkInstanceMetaRequest,
    ) -> rpc_types::error::Result<ListNetworkInstanceMetaResponse> {
        let mut metas = Vec::with_capacity(request.inst_ids.len());
        for instance_id in request.inst_ids.into_iter().map(uuid::Uuid::from) {
            let Some(instance) = self.management.instances.instance(instance_id) else {
                continue;
            };
            let Some(config) = instance.toml_config() else {
                continue;
            };
            let Some(control) = self.management.instances.config_control(instance_id) else {
                continue;
            };
            metas.push(NetworkMeta {
                inst_id: Some(instance_id.into()),
                network_name: config.get_network_identity().network_name,
                config_permission: control.permission.into(),
                instance_name: instance.instance_name().to_owned(),
                source: config_source_to_rpc(config.get_network_config_source()),
            });
        }
        Ok(ListNetworkInstanceMetaResponse { metas })
    }

    async fn get_config_server_status(
        &self,
        _: BaseController,
        _: GetConfigServerStatusRequest,
    ) -> rpc_types::error::Result<GetConfigServerStatusResponse> {
        // Prefer the actionable registry when present so status and
        // ReportManagedNetworkConfig share one source of truth.
        let report = config_server_client::config_server_report_client();
        let status = config_server_status::snapshot();
        let connected = report
            .as_ref()
            .map(|client| client.is_connected())
            .unwrap_or(status.connected);
        // When live-connected, never surface a stale dial error from a prior attempt.
        let last_error = if connected {
            String::new()
        } else {
            status.last_error.unwrap_or_default()
        };
        let connecting_detail = if connected {
            String::new()
        } else {
            status.connecting_detail.unwrap_or_default()
        };
        Ok(GetConfigServerStatusResponse {
            enabled: report.is_some() || status.enabled,
            connected,
            last_error,
            connecting_detail,
        })
    }

    async fn report_managed_network_config(
        &self,
        _: BaseController,
        request: ReportManagedNetworkConfigRequest,
    ) -> rpc_types::error::Result<ReportManagedNetworkConfigResponse> {
        let Some(config) = request.config else {
            return Ok(config_server_client::ReportNetworkConfigError::Invalid(
                "missing NetworkConfig".into(),
            )
            .into_managed_response());
        };
        match config_server_client::report_via_process_client(config).await {
            Ok(()) => Ok(config_server_client::managed_report_ok()),
            Err(error) => Ok(error.into_managed_response()),
        }
    }
}
