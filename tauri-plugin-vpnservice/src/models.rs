#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingRequest {
    pub value: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingResponse {
    pub value: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoidRequest {}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartVpnRequest {
    pub ipv4_addr: Option<String>,
    /// Instance virtual IPv6 with prefix, e.g. `fd00::1/64`.
    ///
    /// Must stay in sync with Kotlin `StartVpnArgs.ipv6Addr` and the JS
    /// `StartVpnRequest.ipv6Addr` — dropping it here silently starts the VPN
    /// without IPv6.
    pub ipv6_addr: Option<String>,
    pub routes: Option<Vec<String>>,
    pub dns: Option<String>,
    pub disallowed_applications: Option<Vec<String>>,
    pub mtu: Option<u32>,
}

/// Result of `prepare_vpn` / `start_vpn` / `stop_vpn`.
///
/// Mirrors the JS `InvokeResponse`: `prepareVpn` fills `granted`, `startVpn`
/// may fill `errorMsg` (e.g. `need_prepare`), `stopVpn` returns it empty.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub error_msg: Option<String>,
    pub granted: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnStatus {
    pub running: bool,
    pub ipv4_addr: Option<String>,
    pub routes: Option<Vec<String>>,
    pub dns: Option<String>,
    /// Incremented by Kotlin NetworkCallback on underlay switch (A9).
    pub underlay_network_generation: Option<i64>,
    pub underlay_network_id: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnTileActionResponse {
    pub action: Option<String>,
}
