use easytier_proto::{
    api::config::{
        ConfigRpc, GetConfigRequest, GetConfigResponse, PatchConfigRequest, PatchConfigResponse,
    },
    rpc_types::{self, controller::BaseController},
};

use crate::{
    config::{api::network_config_from_toml, toml::ConfigLoader as _},
    management::apply_config_patch,
};

use super::{ReadOnlyInstanceResolver, ResolvedInstanceManagementRpc};

#[async_trait::async_trait]
impl<R> ConfigRpc for ResolvedInstanceManagementRpc<R>
where
    R: ReadOnlyInstanceResolver,
{
    type Controller = BaseController;

    async fn patch_config(
        &self,
        _: BaseController,
        request: PatchConfigRequest,
    ) -> rpc_types::error::Result<PatchConfigResponse> {
        let instance = self.instance(request.instance.as_ref())?;
        if let Some(patch) = request.patch {
            apply_config_patch(&instance, patch, self.config_patch_persistence.as_deref()).await?;
            // Refresh host-side caches (GUI localStorage) after a successful hot-patch.
            // Never fail the RPC for tip/writeback hooks — patch is already applied.
            if let Some(hooks) = self.mutation_hooks.as_ref()
                && let Some(toml) = instance.toml_config()
                && let Err(error) = hooks.on_managed_config_patched(&toml).await
            {
                tracing::info!(
                    instance_id = %instance.instance_id(),
                    %error,
                    "managed config patched tip/writeback hook failed"
                );
            }
        }
        Ok(PatchConfigResponse::default())
    }

    async fn get_config(
        &self,
        _: BaseController,
        request: GetConfigRequest,
    ) -> rpc_types::error::Result<GetConfigResponse> {
        let config = self
            .instance(request.instance.as_ref())?
            .toml_config()
            .ok_or_else(|| anyhow::anyhow!("shared TOML configuration is not available"))?;
        Ok(GetConfigResponse {
            config: Some(network_config_from_toml(&config)),
            toml_config: config.dump(),
        })
    }
}
