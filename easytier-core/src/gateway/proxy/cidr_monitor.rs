use std::{collections::BTreeSet, sync::Arc, time::Duration};

use cidr::Ipv4Cidr;

fn ipv4_default_cidr() -> Ipv4Cidr {
    Ipv4Cidr::new(std::net::Ipv4Addr::UNSPECIFIED, 0).expect("0.0.0.0/0")
}
#[cfg(feature = "proxy-cidr-monitor")]
use tokio::sync::Mutex;
use tokio_util::task::AbortOnDropHandle;

use crate::{
    config::runtime::{CoreInstanceRuntimeConfig, CoreRuntimeConfigStore},
    events::{CoreEvent, CoreEventSink},
    peers::peer_manager::PeerManagerCore,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProxyCidrConfigSnapshot {
    pub manual_routes: Option<BTreeSet<Ipv4Cidr>>,
    pub no_tun: bool,
    /// Install the local default route. Set only after an exit VIP has a next hop.
    pub has_exit_nodes: bool,
}

impl From<&CoreInstanceRuntimeConfig> for ProxyCidrConfigSnapshot {
    fn from(config: &CoreInstanceRuntimeConfig) -> Self {
        Self {
            manual_routes: config.services.manual_routes.clone(),
            no_tun: config.services.proxy.no_tun,
            has_exit_nodes: false,
        }
    }
}

#[cfg(feature = "proxy-cidr-monitor")]
pub(crate) struct ProxyCidrMonitorRuntime {
    enabled: bool,
    events: Arc<dyn CoreEventSink>,
    task: Mutex<Option<AbortOnDropHandle<()>>>,
}

#[cfg(feature = "proxy-cidr-monitor")]
impl ProxyCidrMonitorRuntime {
    pub(crate) fn new(enabled: bool, events: Arc<dyn CoreEventSink>) -> Self {
        Self {
            enabled,
            events,
            task: Mutex::new(None),
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) async fn start(
        &self,
        peer_manager: &Arc<PeerManagerCore>,
        runtime_config: CoreRuntimeConfigStore,
    ) {
        if !self.enabled {
            return;
        }
        let mut task = self.task.lock().await;
        if task.is_none() {
            task.replace(
                ProxyCidrMonitor::new(peer_manager, runtime_config, self.events.clone()).start(),
            );
        }
    }

    pub(crate) async fn stop(&self) {
        self.task.lock().await.take();
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyCidrDiff {
    pub current: BTreeSet<Ipv4Cidr>,
    pub added: Vec<Ipv4Cidr>,
    pub removed: Vec<Ipv4Cidr>,
    pub local_exit_default: bool,
}

pub(crate) fn resolve_proxy_cidrs(
    peer_routes: BTreeSet<Ipv4Cidr>,
    config: ProxyCidrConfigSnapshot,
) -> BTreeSet<Ipv4Cidr> {
    if let Some(manual_routes) = config.manual_routes {
        return manual_routes;
    }
    let mut routes = peer_routes;
    if config.has_exit_nodes && !config.no_tun {
        routes.insert(ipv4_default_cidr());
    }
    routes
}

pub(crate) fn diff_proxy_cidrs(
    previous: &BTreeSet<Ipv4Cidr>,
    current: BTreeSet<Ipv4Cidr>,
    local_exit_default: bool,
) -> ProxyCidrDiff {
    let added = current.difference(previous).copied().collect();
    let removed = previous.difference(&current).copied().collect();
    ProxyCidrDiff {
        current,
        added,
        removed,
        local_exit_default,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn collect_proxy_cidrs(
    peer_manager: &PeerManagerCore,
    config: &CoreInstanceRuntimeConfig,
) -> BTreeSet<Ipv4Cidr> {
    collect_proxy_cidr_state(peer_manager, config).await.0
}

async fn collect_proxy_cidr_state(
    peer_manager: &PeerManagerCore,
    config: &CoreInstanceRuntimeConfig,
) -> (BTreeSet<Ipv4Cidr>, bool) {
    let peer_routes = peer_manager.get_route().list_proxy_cidrs().await;
    let exit_nodes = peer_manager.configured_exit_nodes().await;
    let local_exit_default = !config.services.proxy.no_tun
        && config.services.manual_routes.is_none()
        && peer_manager
            .get_peer_map()
            .resolve_exit_node_peer(&exit_nodes)
            .await
            .is_some();
    let current = resolve_proxy_cidrs(
        peer_routes,
        ProxyCidrConfigSnapshot {
            manual_routes: config.services.manual_routes.clone(),
            no_tun: config.services.proxy.no_tun,
            has_exit_nodes: local_exit_default,
        },
    );
    (current, local_exit_default)
}

#[cfg(test)]
fn resolve_proxy_cidrs_from_runtime(
    peer_routes: BTreeSet<Ipv4Cidr>,
    config: &CoreInstanceRuntimeConfig,
) -> BTreeSet<Ipv4Cidr> {
    resolve_proxy_cidrs(peer_routes, config.into())
}

pub(crate) async fn collect_proxy_cidr_diff(
    peer_manager: &PeerManagerCore,
    runtime_config: &CoreRuntimeConfigStore,
    previous: &BTreeSet<Ipv4Cidr>,
) -> ProxyCidrDiff {
    let config = runtime_config.snapshot();
    collect_proxy_cidr_diff_from_snapshot(peer_manager, config.as_ref(), previous).await
}

async fn collect_proxy_cidr_diff_from_snapshot(
    peer_manager: &PeerManagerCore,
    config: &CoreInstanceRuntimeConfig,
    previous: &BTreeSet<Ipv4Cidr>,
) -> ProxyCidrDiff {
    let (current, local_exit_default) = collect_proxy_cidr_state(peer_manager, config).await;
    diff_proxy_cidrs(previous, current, local_exit_default)
}

#[cfg_attr(not(feature = "proxy-cidr-monitor"), allow(dead_code))]
pub(crate) struct ProxyCidrMonitor {
    peer_manager: std::sync::Weak<PeerManagerCore>,
    runtime_config: CoreRuntimeConfigStore,
    events: Arc<dyn CoreEventSink>,
}

#[cfg_attr(not(feature = "proxy-cidr-monitor"), allow(dead_code))]
impl ProxyCidrMonitor {
    pub(crate) fn new(
        peer_manager: &Arc<PeerManagerCore>,
        runtime_config: CoreRuntimeConfigStore,
        events: Arc<dyn CoreEventSink>,
    ) -> Self {
        Self {
            peer_manager: Arc::downgrade(peer_manager),
            runtime_config,
            events,
        }
    }

    pub(crate) fn start(self) -> AbortOnDropHandle<()> {
        AbortOnDropHandle::new(tokio::spawn(async move {
            let mut current = BTreeSet::new();
            let mut last_update = None;
            let mut last_runtime_config: Option<Arc<CoreInstanceRuntimeConfig>> = None;
            let mut last_local_exit_default = false;
            // Cache whether the exit peer was reachable in the previous iteration.
            // Only invalidate the stable-state skip when reachability toggles.
            let mut last_exit_node_reachable = false;

            loop {
                crate::foundation::time::sleep(Duration::from_secs(1)).await;
                let Some(peer_manager) = self.peer_manager.upgrade() else {
                    break;
                };
                let update = peer_manager
                    .get_route()
                    .get_peer_info_last_update_time()
                    .await;
                let runtime_config = self.runtime_config.snapshot();
                let runtime_config_changed = last_runtime_config
                    .as_ref()
                    .map(|previous| !Arc::ptr_eq(previous, &runtime_config))
                    .unwrap_or(true);

                // Check exit-node reachability only when exit nodes are configured,
                // and only skip the stable-state fast-path when reachability changes.
                let exit_node_reachable = if runtime_config.services.exit_nodes.is_empty() {
                    false
                } else {
                    let exit_nodes = peer_manager.configured_exit_nodes().await;
                    peer_manager
                        .get_peer_map()
                        .resolve_exit_node_peer(&exit_nodes)
                        .await
                        .is_some()
                };
                let exit_reachability_changed = exit_node_reachable != last_exit_node_reachable;

                if last_update == Some(update)
                    && !runtime_config_changed
                    && !exit_reachability_changed
                {
                    continue;
                }
                last_exit_node_reachable = exit_node_reachable;

                let diff = collect_proxy_cidr_diff_from_snapshot(
                    peer_manager.as_ref(),
                    runtime_config.as_ref(),
                    &current,
                )
                .await;
                let cidrs_changed = !diff.added.is_empty() || !diff.removed.is_empty();
                let local_exit_changed = diff.local_exit_default != last_local_exit_default;
                last_update = Some(update);
                last_runtime_config = Some(runtime_config.clone());
                last_local_exit_default = diff.local_exit_default;
                current = diff.current;
                if cidrs_changed || local_exit_changed {
                    self.events.emit(CoreEvent::ProxyCidrsUpdated {
                        added: diff.added,
                        removed: diff.removed,
                        local_exit_default: diff.local_exit_default,
                    });
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::peers::PeerRuntimeSnapshot,
        config::runtime::{CoreInstanceRuntimeConfig, CoreRuntimeConfig},
    };

    fn cidrs(values: &[&str]) -> BTreeSet<Ipv4Cidr> {
        values.iter().map(|value| value.parse().unwrap()).collect()
    }

    #[test]
    fn manual_routes_override_peer_routes() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["192.0.2.0/24"])),
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["192.0.2.0/24"]));
    }

    #[test]
    fn dynamic_routes_report_ordered_diff() {
        let current = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: None,
                ..Default::default()
            },
        );
        let diff = diff_proxy_cidrs(&cidrs(&["10.0.0.0/8", "172.16.0.0/12"]), current, false);

        assert_eq!(diff.current, cidrs(&["10.0.0.0/8"]));
        assert!(diff.added.is_empty());
        assert_eq!(diff.removed, vec!["172.16.0.0/12".parse().unwrap()]);
    }

    #[test]
    fn runtime_store_update_changes_the_monitor_config_snapshot() {
        let initial_peer = PeerRuntimeSnapshot::default();
        let store = CoreRuntimeConfigStore::new(
            CoreRuntimeConfig {
                manual_routes: Some(cidrs(&["192.0.2.0/24"])),
                ..Default::default()
            },
            Arc::new(initial_peer),
        );
        let initial = store.snapshot();

        let updated_peer = PeerRuntimeSnapshot::default();
        store.replace(CoreInstanceRuntimeConfig {
            services: CoreRuntimeConfig::default(),
            peer: Arc::new(updated_peer),
        });
        let updated = store.snapshot();

        assert_eq!(
            resolve_proxy_cidrs_from_runtime(cidrs(&["10.0.0.0/8"]), initial.as_ref()),
            cidrs(&["192.0.2.0/24"])
        );
        assert_eq!(
            resolve_proxy_cidrs_from_runtime(cidrs(&["10.0.0.0/8"]), updated.as_ref()),
            cidrs(&["10.0.0.0/8"])
        );
    }

    #[test]
    fn exit_nodes_install_local_default_without_manual_routes() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                has_exit_nodes: true,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["0.0.0.0/0", "10.0.0.0/8"]));
    }

    #[test]
    fn exit_nodes_do_not_install_default_when_no_tun() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                has_exit_nodes: true,
                no_tun: true,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["10.0.0.0/8"]));
    }

    #[test]
    fn manual_routes_suppress_exit_default() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["192.0.2.0/24"])),
                has_exit_nodes: true,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["192.0.2.0/24"]));
    }

    #[test]
    fn unresolved_exit_snapshot_does_not_install_default() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                has_exit_nodes: false,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["10.0.0.0/8"]));
    }

    #[test]
    fn clearing_exit_nodes_drops_only_the_local_default() {
        let with_exit = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                has_exit_nodes: true,
                ..Default::default()
            },
        );
        let without_exit =
            resolve_proxy_cidrs(cidrs(&["10.0.0.0/8"]), ProxyCidrConfigSnapshot::default());
        let diff = diff_proxy_cidrs(&with_exit, without_exit, false);
        assert_eq!(diff.removed, vec![ipv4_default_cidr()]);
        assert!(diff.added.is_empty());
        assert_eq!(diff.current, cidrs(&["10.0.0.0/8"]));
    }
}
