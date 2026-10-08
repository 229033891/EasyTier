use serde::de::DeserializeOwned;
use tauri::{
    AppHandle, Runtime,
    plugin::{PluginApi, PluginHandle},
};

use crate::models::*;

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "com.plugin.vpnservice";

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_vpnservice);

// initializes the Kotlin or Swift plugin classes
pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> crate::Result<Vpnservice<R>> {
    #[cfg(target_os = "android")]
    let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "VpnServicePlugin")?;
    #[cfg(target_os = "ios")]
    let handle = api.register_ios_plugin(init_plugin_vpnservice)?;
    Ok(Vpnservice(handle))
}

/// Access to the vpnservice APIs.
///
/// NOTE: the command string passed to `run_mobile_plugin` MUST be lowerCamelCase
/// — it is matched against the native method names verbatim:
///   * Android: `PluginHandle.kt` keys `@Command` methods by `method.name`, and
///     `@Command` has no name parameter, so only the Kotlin method name matches.
///   * iOS: `@objc` selector names are camelCase too.
///
/// Tauri only applies `heck::AsLowerCamelCase` on the **JS IPC** path
/// (`tauri/src/webview/mod.rs`), *not* on `run_mobile_plugin`, so the JS API
/// (`guest-js/index.ts`) keeps its snake_case command names while these Rust
/// wrappers must use camelCase. Passing snake_case here fails with
/// "No command <name> found for plugin ...".
pub struct Vpnservice<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> Vpnservice<R> {
    pub fn ping(&self, payload: PingRequest) -> crate::Result<PingResponse> {
        self.0
            .run_mobile_plugin("ping", payload)
            .map_err(Into::into)
    }

    pub fn prepare_vpn(&self, payload: VoidRequest) -> crate::Result<Status> {
        self.0
            .run_mobile_plugin("prepareVpn", payload)
            .map_err(Into::into)
    }

    pub fn start_vpn(&self, payload: StartVpnRequest) -> crate::Result<Status> {
        self.0
            .run_mobile_plugin("startVpn", payload)
            .map_err(Into::into)
    }

    pub fn stop_vpn(&self, payload: VoidRequest) -> crate::Result<Status> {
        self.0
            .run_mobile_plugin("stopVpn", payload)
            .map_err(Into::into)
    }

    pub fn get_vpn_status(&self, payload: VoidRequest) -> crate::Result<VpnStatus> {
        self.0
            .run_mobile_plugin("getVpnStatus", payload)
            .map_err(Into::into)
    }

    pub fn consume_vpn_tile_action(
        &self,
        payload: VoidRequest,
    ) -> crate::Result<VpnTileActionResponse> {
        self.0
            .run_mobile_plugin("consumeVpnTileAction", payload)
            .map_err(Into::into)
    }
}
