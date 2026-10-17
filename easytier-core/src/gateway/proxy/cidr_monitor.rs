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
    /// Install the local default route. Set only after an exit VIP has a next
    /// hop. Honoured in append mode as well — see `resolve_proxy_cidrs`.
    pub has_exit_nodes: bool,
    /// Opt-in escape hatch: keep peer-advertised `/0` in the L2 desired set
    /// even when this node has no reachable exit.
    pub allow_peer_default_without_exit: bool,
    /// CIDRs contributed by `Stable` / `Failover` policy rules. Filled by the
    /// L2 collector from the unified routing table, so both planes share one
    /// snapshot (no L2-installed-but-L3-unknown window).
    pub local_routes: BTreeSet<Ipv4Cidr>,
    /// Opt-in (Q5): **append** `manual_routes` to the computed set instead of
    /// replacing it. Default `false` keeps the legacy override semantics for the
    /// *learned* set only; a local `/0` (exit or policy rule) is still installed.
    pub manual_routes_append: bool,
}

impl From<&CoreInstanceRuntimeConfig> for ProxyCidrConfigSnapshot {
    fn from(config: &CoreInstanceRuntimeConfig) -> Self {
        Self {
            manual_routes: config.services.manual_routes.clone(),
            no_tun: config.services.proxy.no_tun,
            has_exit_nodes: false,
            allow_peer_default_without_exit: config.services.proxy.allow_peer_default_without_exit,
            // The runtime-config snapshot has no access to the routing table;
            // `collect_proxy_cidr_state` fills this in from the peer manager.
            local_routes: BTreeSet::new(),
            manual_routes_append: config.services.manual_routes_append,
        }
    }
}

/// Desktop L2 sync observability: desired vs installed proxy CIDRs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyCidrRouteSyncStatus {
    pub desired: Vec<String>,
    pub installed: Vec<String>,
    pub local_exit_default: bool,
    pub last_error: Option<String>,
}

impl ProxyCidrRouteSyncStatus {
    pub fn summary(&self) -> String {
        let desired = if self.desired.is_empty() {
            "-".to_string()
        } else {
            self.desired.join(",")
        };
        let installed = if self.installed.is_empty() {
            "-".to_string()
        } else {
            self.installed.join(",")
        };
        match &self.last_error {
            Some(err) => format!(
                "desired=[{desired}] installed=[{installed}] exit={} last_error={err}",
                self.local_exit_default
            ),
            None => format!(
                "desired=[{desired}] installed=[{installed}] exit={}",
                self.local_exit_default
            ),
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

/// Build the L2 desired IPv4 proxy CIDR set.
///
/// Composition (unified routing §9.3):
/// 1. peer-advertised routes,
/// 2. CIDRs contributed by `Stable` / `Failover` policy rules (`local_routes`),
/// 3. `manual_routes` — **replaces** the learned set by default, or is appended
///    when `manual_routes_append` is set (Q5).
///
/// Peer-advertised `0.0.0.0/0` is filtered out unless this node has a reachable
/// exit (`has_exit_nodes`), a live local `/0` rule, or the escape hatch
/// `allow_peer_default_without_exit`. Filtering must happen here (desired set),
/// not only in OS apply — otherwise the 1s reconciler retries forever.
///
/// **`manual_routes` only ever replaces the *learned* set** (peer-advertised
/// routes plus policy `local_routes`). It does **not** disable the node's own
/// default-route intent:
///
/// - a `/0` written in `manual_routes` itself is the user's explicit full-tunnel
///   request and always survives;
/// - `exit_nodes` with a reachable VIP + next hop still installs the local `/0`
///   under `manual_routes_append = true` (see `collect_proxy_cidr_state`, which
///   sets `has_exit_nodes` for append mode);
/// - a policy rule carrying an explicit `/0` (`local_routes`) still installs the
///   local `/0` under **either** mode — that is a local declaration of
///   "full tunnel", not a learned route.
///
/// The escape hatch `allow_peer_default_without_exit` is the one exception: it
/// keeps the **peer-advertised** `/0`, which is part of the learned set, so
/// override mode still suppresses it.
pub(crate) fn resolve_proxy_cidrs(
    peer_routes: BTreeSet<Ipv4Cidr>,
    config: ProxyCidrConfigSnapshot,
) -> BTreeSet<Ipv4Cidr> {
    let default = ipv4_default_cidr();
    let manual_override = config.manual_routes.is_some() && !config.manual_routes_append;
    // A `/0` the user wrote into `manual_routes` is a local declaration, exactly
    // like a policy `/0` rule — it must not be filtered back out below.
    let manual_declares_default = config
        .manual_routes
        .as_ref()
        .is_some_and(|routes| routes.contains(&default));

    let mut routes = if manual_override {
        // Legacy override (default): `routes = [...]` is the whole truth for the
        // *learned* set — peer-advertised routes and policy CIDRs are dropped.
        config.manual_routes.clone().unwrap_or_default()
    } else {
        let mut routes = peer_routes;
        routes.extend(config.local_routes.iter().copied());
        if let Some(manual_routes) = &config.manual_routes {
            routes.extend(manual_routes.iter().copied());
        }
        routes
    };

    // A local declaration of `/0` (explicit route, policy rule, or — in append
    // mode only — a reachable `exit_nodes`) installs the TUN default.
    let exit_declares_default = config.has_exit_nodes && !manual_override;
    let local_rule_declares_default = config.local_routes.contains(&default);
    let install_local_default =
        (exit_declares_default || local_rule_declares_default || manual_declares_default)
            && !config.no_tun;

    // The escape hatch keeps the *peer-advertised* `/0`, which lives in the
    // learned set — override mode deliberately drops it.
    let keep_peer_default =
        (!manual_override && config.allow_peer_default_without_exit && !config.no_tun)
            || install_local_default;
    if !keep_peer_default {
        routes.remove(&default);
    }
    if install_local_default {
        routes.insert(default);
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
    // Override mode (`manual_routes` without append) owns the learned set, so an
    // exit `/0` would be dropped by `resolve_proxy_cidrs` anyway — do not report
    // `local_exit_default` for it. Append mode keeps it: the manual list is
    // *additional*, not a replacement for this node's default-route intent.
    let manual_override =
        config.services.manual_routes.is_some() && !config.services.manual_routes_append;
    let local_exit_default = !manual_override
        && !config.services.proxy.no_tun
        && peer_manager
            .get_peer_map()
            .resolve_exit_node_peer(&exit_nodes)
            .await
            .is_some();
    let mut snapshot = ProxyCidrConfigSnapshot::from(config);
    snapshot.has_exit_nodes = local_exit_default;
    // Same snapshot the L3 hot path reads → no L2-installed-but-L3-unknown window.
    snapshot.local_routes = peer_manager.local_route_table().l2_contributing_cidrs();
    let current = resolve_proxy_cidrs(peer_routes, snapshot);
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

/// Safety-net reconcile tick for the L2 proxy-CIDR monitor.
///
/// The primary trigger is `PeerManagerCore::proxy_cidr_wake` (peer info moved,
/// i.e. a peer just (re)advertised its routes). This tick stays as the backstop
/// so that (a) a missed or never-fired signal can never stall L2 route
/// installation, and (b) config-only changes — which do not bump peer info —
/// still land within ~1s.
const RECONCILE_TICK: Duration = Duration::from_secs(1);

/// Wait for the next peer-info wake-up or `RECONCILE_TICK`, whichever comes
/// first. `None` means this instance has no OSPF service to observe
/// (`route_algo = none`), so only the tick applies.
async fn wait_for_wake(wake: Option<&tokio::sync::Notify>) {
    match wake {
        Some(wake) => {
            tokio::select! {
                _ = wake.notified() => {}
                _ = crate::foundation::time::sleep(RECONCILE_TICK) => {}
            }
        }
        None => crate::foundation::time::sleep(RECONCILE_TICK).await,
    }
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
            // Domain policy routes learn `/32`s via MagicDNS `observe_ips` without
            // bumping peer-info time. Track LocalRouteTable.revision so L2 install
            // still runs within the ~1s reconcile window (§7.1 / §9.8.4).
            let mut last_local_route_revision = None;

            loop {
                // Grab the wake handle without holding the peer manager across
                // the wait, so instance shutdown is not delayed by up to a tick.
                let Some(peer_manager) = self.peer_manager.upgrade() else {
                    break;
                };
                let wake = peer_manager.proxy_cidr_wake();
                drop(peer_manager);

                wait_for_wake(wake.as_deref()).await;

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
                let local_route_revision = peer_manager.local_route_table().revision();
                let local_routes_changed = last_local_route_revision != Some(local_route_revision);

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
                    && !local_routes_changed
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
                last_local_route_revision = Some(local_route_revision);
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

    /// A wake-up that lands *before* the reconciler starts waiting must still be
    /// observed. This is why the producer uses `notify_one` (which stores a
    /// permit) rather than `notify_waiters` (which drops it when nobody waits):
    /// peer info routinely changes while the reconciler is busy diffing, and a
    /// dropped wake-up would push route installation back to the 1s tick.
    #[tokio::test]
    async fn wake_permit_is_observed_before_the_reconcile_tick() {
        let notify = tokio::sync::Notify::new();
        notify.notify_one();

        let started = std::time::Instant::now();
        wait_for_wake(Some(&notify)).await;

        assert!(
            started.elapsed() < RECONCILE_TICK,
            "a stored permit must short-circuit the tick, took {:?}",
            started.elapsed()
        );
    }

    /// Without a signal the helper still returns within one tick — the backstop
    /// that keeps a missed signal from stalling L2 installation forever.
    #[tokio::test]
    async fn tick_still_fires_without_a_wake_signal() {
        let started = std::time::Instant::now();
        wait_for_wake(None).await;

        assert!(started.elapsed() >= RECONCILE_TICK);
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

    /// A `/0` written directly in `manual_routes` is the user asking for a full
    /// tunnel — it is a *local* declaration, so the peer-`/0` filter must not
    /// delete it. Regression: an intermediate rewrite dropped the override
    /// early-return and silently removed this route.
    #[test]
    fn manual_routes_keep_explicit_default() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["0.0.0.0/0", "192.0.2.0/24"])),
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["0.0.0.0/0", "192.0.2.0/24"]));
    }

    /// Same in append mode.
    #[test]
    fn manual_routes_append_keeps_explicit_default() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["0.0.0.0/0"])),
                manual_routes_append: true,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["0.0.0.0/0", "10.0.0.0/8"]));
    }

    /// `no_tun` still wins over every local `/0` declaration.
    #[test]
    fn manual_routes_explicit_default_is_suppressed_by_no_tun() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["0.0.0.0/0", "192.0.2.0/24"])),
                no_tun: true,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["192.0.2.0/24"]));
    }

    /// Append mode is *additive*: the user's list no longer cancels the node's
    /// own default-route intent, so a reachable exit still installs the local
    /// `/0` (the caller sets `has_exit_nodes` for append mode).
    #[test]
    fn manual_routes_append_keeps_exit_default() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["192.0.2.0/24"])),
                manual_routes_append: true,
                has_exit_nodes: true,
                ..Default::default()
            },
        );
        assert_eq!(
            resolved,
            cidrs(&["0.0.0.0/0", "10.0.0.0/8", "192.0.2.0/24"])
        );
    }

    /// …but append mode without a reachable exit must not invent one.
    #[test]
    fn manual_routes_append_without_exit_has_no_default() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["192.0.2.0/24"])),
                manual_routes_append: true,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["10.0.0.0/8", "192.0.2.0/24"]));
    }

    /// The escape hatch keeps the *peer-advertised* `/0`, which belongs to the
    /// learned set — override mode deliberately discards it.
    #[test]
    fn escape_hatch_is_suppressed_by_manual_routes_override() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["0.0.0.0/0", "10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["192.0.2.0/24"])),
                allow_peer_default_without_exit: true,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["192.0.2.0/24"]));
    }

    /// …and is honoured again in append mode.
    #[test]
    fn escape_hatch_survives_manual_routes_append() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["0.0.0.0/0", "10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["192.0.2.0/24"])),
                manual_routes_append: true,
                allow_peer_default_without_exit: true,
                ..Default::default()
            },
        );
        assert_eq!(
            resolved,
            cidrs(&["0.0.0.0/0", "10.0.0.0/8", "192.0.2.0/24"])
        );
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

    #[test]
    fn peer_default_is_filtered_without_exit_or_escape_hatch() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["0.0.0.0/0", "10.0.0.0/8"]),
            ProxyCidrConfigSnapshot::default(),
        );
        assert_eq!(resolved, cidrs(&["10.0.0.0/8"]));
    }

    #[test]
    fn peer_default_kept_with_escape_hatch_without_exit() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["0.0.0.0/0", "10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                allow_peer_default_without_exit: true,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["0.0.0.0/0", "10.0.0.0/8"]));
    }

    #[test]
    fn peer_default_survives_when_local_exit_is_active() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["0.0.0.0/0", "10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                has_exit_nodes: true,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["0.0.0.0/0", "10.0.0.0/8"]));
    }

    #[test]
    fn peer_default_filtered_when_exit_configured_but_no_tun() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["0.0.0.0/0", "10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                has_exit_nodes: true,
                no_tun: true,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["10.0.0.0/8"]));
    }

    #[test]
    fn escape_hatch_ignored_when_no_tun() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["0.0.0.0/0", "10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                allow_peer_default_without_exit: true,
                no_tun: true,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["10.0.0.0/8"]));
    }

    #[test]
    fn local_routes_are_appended_to_peer_routes() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                local_routes: cidrs(&["198.51.100.0/24"]),
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["10.0.0.0/8", "198.51.100.0/24"]));
    }

    #[test]
    fn local_default_rule_installs_full_tunnel() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                local_routes: cidrs(&["0.0.0.0/0"]),
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["0.0.0.0/0", "10.0.0.0/8"]));
    }

    #[test]
    fn manual_routes_append_keeps_the_computed_set() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["192.0.2.0/24"])),
                manual_routes_append: true,
                local_routes: cidrs(&["198.51.100.0/24"]),
                ..Default::default()
            },
        );
        assert_eq!(
            resolved,
            cidrs(&["10.0.0.0/8", "192.0.2.0/24", "198.51.100.0/24"])
        );
    }

    /// A policy CIDR (non-`/0`) is part of the learned set → override drops it.
    #[test]
    fn manual_routes_override_still_replaces_local_routes() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["192.0.2.0/24"])),
                local_routes: cidrs(&["198.51.100.0/24"]),
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["192.0.2.0/24"]));
    }

    /// An explicit `[[policy_routes]] cidr = "0.0.0.0/0"` is a *local* full-tunnel
    /// declaration, not a learned route, so it survives override mode too.
    #[test]
    fn local_default_rule_survives_manual_routes_override() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["192.0.2.0/24"])),
                local_routes: cidrs(&["0.0.0.0/0"]),
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["0.0.0.0/0", "192.0.2.0/24"]));
    }

    /// `no_tun` wins over the local `/0` declaration in both modes.
    #[test]
    fn local_default_rule_is_suppressed_by_no_tun() {
        let resolved = resolve_proxy_cidrs(
            cidrs(&["10.0.0.0/8"]),
            ProxyCidrConfigSnapshot {
                manual_routes: Some(cidrs(&["192.0.2.0/24"])),
                local_routes: cidrs(&["0.0.0.0/0"]),
                no_tun: true,
                ..Default::default()
            },
        );
        assert_eq!(resolved, cidrs(&["192.0.2.0/24"]));
    }
}
