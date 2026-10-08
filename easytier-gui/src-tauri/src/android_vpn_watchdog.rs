//! Android VPN lifecycle watchdog (audit A3 + A9).
//!
//! WebView `setInterval` is throttled/frozen in the background and under Doze,
//! but the Tauri/Rust runtime keeps running while the process is alive (especially
//! with a VpnService / MainForegroundService FGS).
//!
//! - **A3**: if VpnService is up but no enabled TUN instance remains, stop it via
//!   the plugin without waiting for JS.
//! - **A9**: when Kotlin reports a new `underlayNetworkGeneration` (Wi-Fi↔cellular),
//!   close direct peer conns so ManualConnectorManager's 1s loop redials on the
//!   new underlay — also without WebView.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_vpnservice::{VoidRequest, VpnserviceExt};
use tokio::time::{MissedTickBehavior, interval};

use crate::underlay_reconnect_grace::{
    in_underlay_reconnect_grace, swap_underlay_generation,
};
use crate::{CLIENT_MANAGER, INSTANCE_MANAGER};

const WATCHDOG_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) use crate::underlay_reconnect_grace::{
    arm_underlay_reconnect_grace, note_underlay_generation,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VpnWatchdogTick {
    vpn_running: bool,
    has_tun_instance: bool,
    /// First enabled TUN instance id, when any.
    instance_id: Option<String>,
    underlay_network_generation: Option<i64>,
    /// Native action taken this tick, if any.
    action: Option<&'static str>,
}

pub(crate) fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Let RPC / client manager finish initializing before the first probe.
        tokio::time::sleep(Duration::from_secs(5)).await;

        let mut ticker = interval(WATCHDOG_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(error) = tick_once(&app).await {
                // Keep this at warn: a silently swallowed tick error means the
                // whole A3/A14 orphan-VpnService cleanup and the A9 underlay
                // fallback stop working with zero signal (that is exactly how a
                // `run_mobile_plugin` name mismatch went unnoticed before).
                tracing::warn!(%error, "android vpn watchdog tick failed");
            }
        }
    });
}

/// Immediate orphan cleanup used by instance-stop hooks (do not wait for the
/// next 30s tick).
pub(crate) fn stop_vpn_if_no_tun(app: &AppHandle) -> Result<(), String> {
    // Fail-open on lock contention: a busy manager must not be mistaken for
    // "no TUN instance" (that would spuriously stop a live VPN).
    let guard = match CLIENT_MANAGER.try_read() {
        Ok(guard) => guard,
        Err(_) => return Ok(()),
    };
    let has_tun = guard
        .as_ref()
        .map(|cm| cm.get_enabled_instances_with_tun_ids().any(|_| true))
        .unwrap_or(false);
    if has_tun {
        return Ok(());
    }
    stop_vpn_plugin(app)
}

/// Force peer reconnect after underlay change (A9). Callable from the plugin
/// event fast-path and from the watchdog when generation drifts.
pub(crate) async fn reconnect_peers_after_underlay_change() -> Result<usize, String> {
    if in_underlay_reconnect_grace() {
        tracing::info!("skip underlay peer reconnect during startup grace");
        return Ok(0);
    }
    let Some(instance_manager) = INSTANCE_MANAGER.read().await.clone() else {
        return Ok(0);
    };
    let instance_ids: Vec<uuid::Uuid> = match CLIENT_MANAGER.try_read() {
        Ok(guard) => guard
            .as_ref()
            .map(|cm| cm.get_enabled_instances_with_tun_ids().collect())
            .unwrap_or_default(),
        Err(_) => return Ok(0),
    };
    if instance_ids.is_empty() {
        return Ok(0);
    }

    let mut closed = 0usize;
    for instance_id in instance_ids {
        let Some(instance) = instance_manager.instance(instance_id) else {
            continue;
        };
        if !instance.is_ready() {
            continue;
        }
        let snapshots = instance.peer_snapshots().await;
        for snapshot in snapshots {
            for conn in snapshot.conns {
                let Ok(conn_id) = conn.conn_id.parse::<uuid::Uuid>() else {
                    continue;
                };
                match instance.close_peer_conn(snapshot.peer_id, &conn_id).await {
                    Ok(()) => closed += 1,
                    Err(error) => {
                        tracing::debug!(
                            %instance_id,
                            peer_id = snapshot.peer_id,
                            %conn_id,
                            %error,
                            "close_peer_conn after underlay change failed"
                        );
                    }
                }
            }
        }
    }
    if closed > 0 {
        tracing::info!(closed, "closed peer conns after underlay network change");
    }
    Ok(closed)
}

async fn tick_once(app: &AppHandle) -> Result<(), String> {
    let (has_tun_instance, instance_id) = match CLIENT_MANAGER.try_read() {
        Ok(guard) => match guard.as_ref() {
            Some(cm) => {
                let mut ids = cm.get_enabled_instances_with_tun_ids();
                let first = ids.next().map(|id| id.to_string());
                (first.is_some(), first)
            }
            None => return Ok(()), // RPC not ready yet
        },
        Err(_) => return Ok(()),
    };

    let status = app
        .vpnservice()
        .get_vpn_status(VoidRequest {})
        .map_err(|e| e.to_string())?;
    let vpn_running = status.running;
    let underlay_gen = status.underlay_network_generation;

    let mut action = None;
    if vpn_running && !has_tun_instance {
        // Closed loop: tear down the orphan VpnService without waiting for WebView.
        tracing::warn!("android vpn watchdog: VpnService running with no TUN instance; stopping");
        stop_vpn_plugin(app)?;
        action = Some("stop_orphan");
    } else if let Some(generation) = underlay_gen {
        let previous = swap_underlay_generation(generation);
        // First observation only seeds the counter; a later bump means switch.
        if previous >= 0 && previous != generation {
            if in_underlay_reconnect_grace() {
                tracing::info!(
                    previous,
                    generation,
                    "android vpn watchdog: underlay generation changed during startup grace; seeding only"
                );
                action = Some("underlay_seed_grace");
            } else {
                tracing::warn!(
                    previous,
                    generation,
                    "android vpn watchdog: underlay network generation changed; reconnecting peers"
                );
                let _ = reconnect_peers_after_underlay_change().await?;
                action = Some("underlay_reconnect");
            }
        }
    }

    let tick = VpnWatchdogTick {
        vpn_running: if action == Some("stop_orphan") {
            false
        } else {
            vpn_running
        },
        has_tun_instance,
        instance_id,
        underlay_network_generation: underlay_gen,
        action,
    };
    app.emit("vpn_watchdog_tick", &tick)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn stop_vpn_plugin(app: &AppHandle) -> Result<(), String> {
    app.vpnservice()
        .stop_vpn(VoidRequest {})
        .map_err(|e| e.to_string())?;
    Ok(())
}
