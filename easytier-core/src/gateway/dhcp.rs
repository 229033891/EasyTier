use std::{collections::HashSet, sync::Arc, time::Duration};

use async_trait::async_trait;
use cidr::Ipv4Inet;
use rand::RngExt;
use tokio_util::task::AbortOnDropHandle;

use crate::{
    config::IpPrefix, config::PeerId, config::runtime::CoreRuntimeConfigStore,
    peers::peer_manager::PeerManagerCore,
};

#[cfg(feature = "dhcp-ipv4")]
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DhcpIpv4Decision {
    WaitForPeers,
    Unchanged,
    Change {
        previous: Option<Ipv4Inet>,
        next: Option<Ipv4Inet>,
    },
}

#[derive(Debug, Default)]
pub struct DhcpIpv4Allocator {
    default_subnet: Option<Ipv4Inet>,
    current: Option<Ipv4Inet>,
}

/// Deterministic sort key that gives every peer its own candidate ordering.
///
/// CRITICAL (CR-14). This is the SplitMix64 finalizer over
/// `(peer_id, candidate address)`. Two properties are required:
///
/// * **Node-specific** — distinct peer ids yield unrelated orderings, so two
///   nodes racing for the same address diverge on the retry instead of marching
///   together in lockstep forever.
/// * **Stable forever** — it is plain integer arithmetic specified here, not a
///   library PRNG, so the order cannot silently change under us when `rand` is
///   upgraded. A restarting node must re-derive the same address, and an
///   EasyTier upgrade must not renumber a mesh.
fn candidate_order_key(my_peer_id: PeerId, candidate: &Ipv4Inet) -> u64 {
    let mut z = (my_peer_id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (u32::from(candidate.address()) as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93);
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl DhcpIpv4Allocator {
    pub fn new(default_subnet: Ipv4Inet) -> Self {
        Self {
            default_subnet: Some(default_subnet),
            current: None,
        }
    }

    pub fn current(&self) -> Option<Ipv4Inet> {
        self.current
    }

    pub fn reset(&mut self) {
        self.current = None;
    }

    pub fn commit(&mut self, next: Option<Ipv4Inet>) {
        self.current = next;
    }

    pub fn evaluate(
        &self,
        has_routes: bool,
        used_ipv4: &HashSet<Ipv4Inet>,
        my_peer_id: PeerId,
    ) -> DhcpIpv4Decision {
        if !has_routes {
            return DhcpIpv4Decision::WaitForPeers;
        }

        let Some(subnet) = self
            .default_subnet
            .as_ref()
            .or_else(|| used_ipv4.iter().next())
        else {
            return DhcpIpv4Decision::WaitForPeers;
        };
        if let Some(current) = self.current
            && current.network() == subnet.network()
            && !used_ipv4.contains(&current)
        {
            return DhcpIpv4Decision::Unchanged;
        }

        // CRITICAL (CR-14): the candidate order must be **node-specific**.
        //
        // `used_ipv4` comes from `list_routes`, which skips our own route, so
        // every node in the mesh observes the *same* set of in-use addresses.
        // Picking the first free address in a shared order therefore made every
        // concurrently-starting node choose the *same* VIP. Worse, the collision
        // was self-sustaining: both nodes then observed that address as taken by
        // a peer and both advanced to the next one, and the next one, in
        // lockstep — a permanent flapping loop that never converges.
        //
        // The order is a deterministic **permutation keyed by `my_peer_id`**, not
        // a rotation. Rotation is not enough: two peer ids congruent modulo the
        // host count share one cyclic order, so they would still agree on every
        // successive address and keep colliding (this is covered by
        // `peers_diverge_instead_of_marching_in_lockstep_after_a_collision`).
        // A per-node permutation makes any two distinct nodes order the remaining
        // candidates differently, so the retry after a collision diverges.
        //
        // It stays a pure function of `my_peer_id` (no RNG), so a restarting node
        // re-derives the same order and therefore the same address.
        let mut candidates: Vec<Ipv4Inet> = subnet
            .network()
            .iter()
            .filter(|candidate| {
                candidate.address() != subnet.first_address()
                    && candidate.address() != subnet.last_address()
            })
            .collect();
        candidates.sort_by_key(|candidate| candidate_order_key(my_peer_id, candidate));
        let next = candidates.into_iter().find(|c| !used_ipv4.contains(c));

        if self.current == next {
            return DhcpIpv4Decision::Unchanged;
        }

        DhcpIpv4Decision::Change {
            previous: self.current,
            next,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DhcpIpv4RouteSnapshot {
    pub has_routes: bool,
    pub used_ipv4: HashSet<Ipv4Inet>,
    /// Our own peer id.
    ///
    /// CRITICAL (CR-14): `used_ipv4` deliberately excludes our own address, so
    /// it is identical on every node. `DhcpIpv4Allocator::evaluate` needs the
    /// peer id to derive a *node-specific* candidate order — otherwise every
    /// node picks the same VIP and then flapping in lockstep forever.
    pub my_peer_id: PeerId,
}

#[async_trait]
pub trait DhcpIpv4RouteSource: Send + Sync + 'static {
    async fn dhcp_ipv4_route_snapshot(&self) -> DhcpIpv4RouteSnapshot;
}

#[async_trait]
impl DhcpIpv4RouteSource for PeerManagerCore {
    async fn dhcp_ipv4_route_snapshot(&self) -> DhcpIpv4RouteSnapshot {
        let routes = self.get_route().list_routes().await;
        let has_routes = !routes.is_empty();
        let used_ipv4 = routes
            .into_iter()
            .filter_map(|route| route.ipv4_addr.map(Into::into))
            .collect();
        DhcpIpv4RouteSnapshot {
            has_routes,
            used_ipv4,
            my_peer_id: self.my_peer_id(),
        }
    }
}

#[async_trait]
pub trait DhcpIpv4Host: Send + Sync + 'static {
    fn take_interface_closed(&self) -> bool;

    async fn apply_dhcp_ipv4(
        &self,
        previous: Option<Ipv4Inet>,
        next: Option<Ipv4Inet>,
    ) -> DhcpIpv4ApplyOutcome;

    fn publish_dhcp_ipv4(
        &self,
        _previous: Option<Ipv4Inet>,
        _requested: Option<Ipv4Inet>,
        _actual: Option<Ipv4Inet>,
    ) {
    }
}

#[cfg(feature = "dhcp-ipv4")]
pub(crate) struct DhcpIpv4Runtime {
    task: Mutex<Option<AbortOnDropHandle<()>>>,
}

#[cfg(feature = "dhcp-ipv4")]
impl DhcpIpv4Runtime {
    pub(crate) fn new() -> Self {
        Self {
            task: Mutex::new(None),
        }
    }

    pub(crate) async fn start(
        &self,
        route_source: Arc<dyn DhcpIpv4RouteSource>,
        runtime_config: CoreRuntimeConfigStore,
        host: Arc<dyn DhcpIpv4Host>,
    ) {
        let mut task = self.task.lock().await;
        if task.is_none() {
            task.replace(DhcpIpv4Service::new(route_source, runtime_config, host).start());
        }
    }

    pub(crate) async fn stop(&self) {
        self.task.lock().await.take();
    }
}

pub struct DhcpIpv4ApplyPermit {
    _guard: Box<dyn Send>,
}

impl DhcpIpv4ApplyPermit {
    pub fn new(guard: impl Send + 'static) -> Self {
        Self {
            _guard: Box::new(guard),
        }
    }
}

pub struct DhcpIpv4ApplyOutcome {
    pub actual: Option<Ipv4Inet>,
    pub result: anyhow::Result<()>,
    permit: Option<DhcpIpv4ApplyPermit>,
}

impl DhcpIpv4ApplyOutcome {
    pub fn applied(actual: Option<Ipv4Inet>) -> Self {
        Self {
            actual,
            result: Ok(()),
            permit: None,
        }
    }

    pub fn failed(actual: Option<Ipv4Inet>, error: impl Into<anyhow::Error>) -> Self {
        Self {
            actual,
            result: Err(error.into()),
            permit: None,
        }
    }

    pub fn with_permit(mut self, permit: DhcpIpv4ApplyPermit) -> Self {
        self.permit = Some(permit);
        self
    }
}

pub struct DhcpIpv4Service {
    operation: tokio::sync::Mutex<()>,
    allocator: std::sync::Mutex<DhcpIpv4Allocator>,
    route_source: Arc<dyn DhcpIpv4RouteSource>,
    runtime_config: CoreRuntimeConfigStore,
    host: Arc<dyn DhcpIpv4Host>,
}

impl DhcpIpv4Service {
    pub fn new(
        route_source: Arc<dyn DhcpIpv4RouteSource>,
        runtime_config: CoreRuntimeConfigStore,
        host: Arc<dyn DhcpIpv4Host>,
    ) -> Arc<Self> {
        Arc::new(Self {
            operation: tokio::sync::Mutex::new(()),
            allocator: std::sync::Mutex::new(DhcpIpv4Allocator::default()),
            route_source,
            runtime_config,
            host,
        })
    }

    pub fn current(&self) -> Option<Ipv4Inet> {
        self.allocator.lock().unwrap().current()
    }

    pub async fn reconcile_once(&self) -> bool {
        let _operation = self.operation.lock().await;
        if self.host.take_interface_closed() {
            self.allocator.lock().unwrap().reset();
        }
        let snapshot = self.route_source.dhcp_ipv4_route_snapshot().await;
        let decision = self.allocator.lock().unwrap().evaluate(
            snapshot.has_routes,
            &snapshot.used_ipv4,
            snapshot.my_peer_id,
        );

        let DhcpIpv4Decision::Change { previous, next } = decision else {
            return snapshot.has_routes;
        };
        tracing::debug!(?previous, ?next, "DHCP IPv4 reconciliation applying change");
        let outcome = self.host.apply_dhcp_ipv4(previous, next).await;
        let DhcpIpv4ApplyOutcome {
            actual,
            result,
            permit,
        } = outcome;
        self.runtime_config.update_peer_with(|peer| {
            peer.runtime.core.routes.ipv4 = actual.map(|actual| IpPrefix {
                address: actual.address().into(),
                prefix_len: actual.network_length(),
            });
        });
        match result {
            Ok(()) => {
                self.allocator.lock().unwrap().commit(actual);
                self.host.publish_dhcp_ipv4(previous, next, actual);
            }
            Err(err) => {
                tracing::error!(?previous, ?next, ?actual, ?err, "DHCP IPv4 apply failed");
            }
        }
        drop(permit);
        snapshot.has_routes
    }

    pub fn start(self: &Arc<Self>) -> AbortOnDropHandle<()> {
        let service = self.clone();
        AbortOnDropHandle::new(tokio::spawn(async move {
            let mut next_sleep = Duration::ZERO;
            loop {
                crate::foundation::time::sleep(next_sleep).await;
                next_sleep = if service.reconcile_once().await {
                    Duration::from_secs(rand::rng().random_range(5..10))
                } else {
                    Duration::from_secs(1)
                };
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    /// Peer id used by tests that only care about "some node in the mesh".
    ///
    /// `0` yields a scan offset of `0`, i.e. the plain "first free address"
    /// order, which is what the pre-CR-14 tests were written against. Tests
    /// about collision behaviour pass explicit, *differing* ids on purpose.
    const TEST_PEER_ID: PeerId = 0;

    struct StaticRouteSource {
        snapshot: Mutex<DhcpIpv4RouteSnapshot>,
    }

    #[async_trait]
    impl DhcpIpv4RouteSource for StaticRouteSource {
        async fn dhcp_ipv4_route_snapshot(&self) -> DhcpIpv4RouteSnapshot {
            self.snapshot.lock().unwrap().clone()
        }
    }

    type PublishedIpv4Route = (Option<Ipv4Inet>, Option<Ipv4Inet>, Option<Ipv4Inet>);

    #[derive(Default)]
    struct RecordingHost {
        interface_closed: AtomicBool,
        fail_apply: AtomicBool,
        hold_apply_permit: AtomicBool,
        permit_held: Arc<AtomicBool>,
        published_with_permit: AtomicBool,
        runtime_config: Mutex<Option<CoreRuntimeConfigStore>>,
        published_runtime_ipv4: Mutex<Vec<Option<IpPrefix>>>,
        changes: Mutex<Vec<(Option<Ipv4Inet>, Option<Ipv4Inet>)>>,
        published: Mutex<Vec<PublishedIpv4Route>>,
    }

    struct RecordingPermit(Arc<AtomicBool>);

    impl Drop for RecordingPermit {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }

    #[async_trait]
    impl DhcpIpv4Host for RecordingHost {
        fn take_interface_closed(&self) -> bool {
            self.interface_closed.swap(false, Ordering::AcqRel)
        }

        async fn apply_dhcp_ipv4(
            &self,
            previous: Option<Ipv4Inet>,
            next: Option<Ipv4Inet>,
        ) -> DhcpIpv4ApplyOutcome {
            self.changes.lock().unwrap().push((previous, next));
            let mut outcome = if self.fail_apply.load(Ordering::Acquire) {
                DhcpIpv4ApplyOutcome::failed(None, anyhow::anyhow!("apply failed"))
            } else {
                DhcpIpv4ApplyOutcome::applied(next)
            };
            if self.hold_apply_permit.load(Ordering::Acquire) {
                assert!(!self.permit_held.swap(true, Ordering::AcqRel));
                outcome = outcome.with_permit(DhcpIpv4ApplyPermit::new(RecordingPermit(
                    self.permit_held.clone(),
                )));
            }
            outcome
        }

        fn publish_dhcp_ipv4(
            &self,
            previous: Option<Ipv4Inet>,
            requested: Option<Ipv4Inet>,
            actual: Option<Ipv4Inet>,
        ) {
            self.published_with_permit
                .store(self.permit_held.load(Ordering::Acquire), Ordering::Release);
            if let Some(runtime_config) = self.runtime_config.lock().unwrap().as_ref() {
                self.published_runtime_ipv4.lock().unwrap().push(
                    runtime_config
                        .snapshot()
                        .peer
                        .runtime
                        .core
                        .routes
                        .ipv4
                        .clone(),
                );
            }
            self.published
                .lock()
                .unwrap()
                .push((previous, requested, actual));
        }
    }

    fn service(
        snapshot: DhcpIpv4RouteSnapshot,
        host: Arc<RecordingHost>,
    ) -> (Arc<DhcpIpv4Service>, CoreRuntimeConfigStore) {
        let runtime_config = CoreRuntimeConfigStore::new(
            crate::config::runtime::CoreRuntimeConfig::default(),
            Arc::new(crate::config::peers::PeerRuntimeSnapshot::default()),
        );
        *host.runtime_config.lock().unwrap() = Some(runtime_config.clone());
        let service = DhcpIpv4Service::new(
            Arc::new(StaticRouteSource {
                snapshot: Mutex::new(snapshot),
            }),
            runtime_config.clone(),
            host,
        );
        (service, runtime_config)
    }

    #[test]
    fn waits_until_at_least_one_route_exists() {
        let allocator = DhcpIpv4Allocator::default();

        assert_eq!(
            allocator.evaluate(false, &HashSet::new(), TEST_PEER_ID),
            DhcpIpv4Decision::WaitForPeers
        );
    }

    #[test]
    fn does_not_fall_back_to_a_builtin_subnet_without_assigned_ipv4() {
        let allocator = DhcpIpv4Allocator::default();

        assert_eq!(
            allocator.evaluate(true, &HashSet::new(), TEST_PEER_ID),
            DhcpIpv4Decision::WaitForPeers
        );
    }

    #[test]
    fn keeps_current_address_when_it_is_free_in_the_selected_subnet() {
        let mut allocator = DhcpIpv4Allocator::default();
        allocator.commit(Some("10.1.2.8/24".parse().unwrap()));
        let used = HashSet::from(["10.1.2.2/24".parse().unwrap()]);

        assert_eq!(
            allocator.evaluate(true, &used, TEST_PEER_ID),
            DhcpIpv4Decision::Unchanged
        );
    }

    #[test]
    fn selects_an_available_host_after_a_conflict() {
        let mut allocator = DhcpIpv4Allocator::default();
        allocator.commit(Some("10.1.2.1/24".parse().unwrap()));
        let used = HashSet::from([
            "10.1.2.1/24".parse::<Ipv4Inet>().unwrap(),
            "10.1.2.2/24".parse().unwrap(),
        ]);

        // CR-14: the specific address is now peer-id-derived, so assert the
        // contract rather than a literal.
        let decision = allocator.evaluate(true, &used, TEST_PEER_ID);
        let DhcpIpv4Decision::Change { previous, next } = decision else {
            panic!("expected a change, got {decision:?}");
        };
        assert_eq!(previous, Some("10.1.2.1/24".parse().unwrap()));
        let next = next.expect("a free host should be available");
        assert!(!used.contains(&next), "{next} is already in use");
        assert_ne!(next, previous.unwrap());
        assert_eq!(
            next.network(),
            "10.1.2.0/24".parse::<Ipv4Inet>().unwrap().network()
        );
    }

    #[test]
    fn reset_forgets_the_previous_interface_address() {
        let mut allocator = DhcpIpv4Allocator::default();
        allocator.commit(Some("10.1.2.8/24".parse().unwrap()));

        allocator.reset();

        assert_eq!(allocator.current(), None);
    }

    // -----------------------------------------------------------------------
    // CR-14: node-specific candidate order stops the duplicate-VIP flapping.
    // -----------------------------------------------------------------------

    fn subnet_allocator() -> DhcpIpv4Allocator {
        DhcpIpv4Allocator::new("10.1.2.0/24".parse().unwrap())
    }

    fn decision_next(decision: &DhcpIpv4Decision) -> Option<Ipv4Inet> {
        match decision {
            DhcpIpv4Decision::Change { next, .. } => *next,
            _ => None,
        }
    }

    /// The address a node with `peer_id` would pick given `used`.
    ///
    /// CR-14: the concrete address is peer-id-derived, so service-level tests
    /// must ask the allocator rather than hard-coding a literal.
    fn expected_pick(used: &[&str], peer_id: PeerId) -> Ipv4Inet {
        let used: HashSet<Ipv4Inet> = used.iter().map(|s| s.parse().unwrap()).collect();
        decision_next(&DhcpIpv4Allocator::default().evaluate(true, &used, peer_id))
            .expect("a free address should be available")
    }

    #[test]
    fn different_peers_pick_different_addresses_on_a_fresh_mesh() {
        // The core CR-14 regression. On a fresh mesh nobody has an address yet,
        // so `used_ipv4` is identical (empty) on every node. The old code always
        // returned the *first* free host, so every node claimed 10.1.2.1.
        let used: HashSet<Ipv4Inet> = HashSet::new();
        let mut picks = HashSet::new();
        for peer_id in 1..=8u32 {
            let decision = subnet_allocator().evaluate(true, &used, peer_id);
            let next = decision_next(&decision).expect("a free address should exist");
            assert!(
                picks.insert(next),
                "peer {peer_id} collided on {next}: {picks:?}"
            );
        }
        assert_eq!(picks.len(), 8);
    }

    #[test]
    fn peers_diverge_instead_of_marching_in_lockstep_after_a_collision() {
        // Model the old self-sustaining flapping: both nodes hold the same
        // address, both observe it as taken *by a peer*, so both must move —
        // but to *different* addresses, or they would simply collide again and
        // loop forever.
        let subnet = "10.1.2.0/24".parse::<Ipv4Inet>().unwrap();

        let first_pick = |peer_id: PeerId| {
            decision_next(&subnet_allocator().evaluate(true, &HashSet::new(), peer_id)).unwrap()
        };

        // Peer ids that share a scan offset share their first preference. Those
        // are the residual collision case — precisely the one that used to never
        // converge. The scan offset is `peer_id % <usable host count>`, so search
        // a range wide enough to wrap it rather than hard-coding the modulus.
        let (a, b) = (1..=600u32)
            .flat_map(|x| (1..=600u32).map(move |y| (x, y)))
            .find(|(x, y)| x != y && first_pick(*x) == first_pick(*y))
            .expect("some pair of peer ids must share a first preference");

        let shared = first_pick(a);
        // Both now see `shared` advertised by somebody else (not by themselves,
        // because `list_routes` skips our own route).
        let used: HashSet<Ipv4Inet> = HashSet::from([shared]);
        let next_a = decision_next(&subnet_allocator().evaluate(true, &used, a)).unwrap();
        let next_b = decision_next(&subnet_allocator().evaluate(true, &used, b)).unwrap();

        assert_ne!(
            next_a, next_b,
            "peers {a}/{b} advanced to the same address {next_a}; the flapping loop \
             would never converge"
        );
        // And they must both stay inside the subnet, skipping network/broadcast.
        for next in [next_a, next_b] {
            assert_eq!(next.network(), subnet.network());
            assert_ne!(next, subnet.first_address().into());
            assert_ne!(next, subnet.last_address().into());
            assert!(!used.contains(&next));
        }
    }

    #[test]
    fn candidate_order_is_stable_for_a_given_peer() {
        // No RNG: a restarting node must re-derive the same order, otherwise it
        // would pick a new address on every restart.
        let used: HashSet<Ipv4Inet> = HashSet::new();
        let first = decision_next(&subnet_allocator().evaluate(true, &used, 42));
        for _ in 0..5 {
            assert_eq!(
                decision_next(&subnet_allocator().evaluate(true, &used, 42)),
                first
            );
        }
    }

    #[test]
    fn a_free_current_address_is_never_churned() {
        // The rotation must not disturb stability: once we hold a free address we
        // keep it, even though it is not this node's first preference.
        for peer_id in 1..=16u32 {
            let mut allocator = subnet_allocator();
            allocator.commit(Some("10.1.2.7/24".parse().unwrap()));
            let used: HashSet<Ipv4Inet> = HashSet::from(["10.1.2.1/24".parse().unwrap()]);
            assert_eq!(
                allocator.evaluate(true, &used, peer_id),
                DhcpIpv4Decision::Unchanged,
                "peer {peer_id} would have churned its address"
            );
        }
    }

    #[tokio::test]
    async fn service_does_not_apply_ipv4_when_no_peer_has_one() {
        let host = Arc::new(RecordingHost::default());
        let (service, runtime_config) = service(
            DhcpIpv4RouteSnapshot {
                has_routes: true,
                used_ipv4: HashSet::new(),
                my_peer_id: TEST_PEER_ID,
            },
            host.clone(),
        );

        assert!(service.reconcile_once().await);

        assert_eq!(service.current(), None);
        assert!(host.changes.lock().unwrap().is_empty());
        assert!(host.published.lock().unwrap().is_empty());
        assert!(
            runtime_config
                .snapshot()
                .peer
                .runtime
                .core
                .routes
                .ipv4
                .is_none()
        );
    }

    #[tokio::test]
    async fn service_commits_only_after_host_apply_succeeds() {
        let host = Arc::new(RecordingHost::default());
        let (service, runtime_config) = service(
            DhcpIpv4RouteSnapshot {
                has_routes: true,
                my_peer_id: TEST_PEER_ID,
                used_ipv4: HashSet::from(["198.18.0.2/24".parse().unwrap()]),
            },
            host.clone(),
        );

        assert!(service.reconcile_once().await);

        let picked = expected_pick(&["198.18.0.2/24"], TEST_PEER_ID);
        assert_eq!(service.current(), Some(picked));
        assert_eq!(*host.changes.lock().unwrap(), [(None, Some(picked))]);
        assert_eq!(
            runtime_config.snapshot().peer.runtime.core.routes.ipv4,
            Some(IpPrefix::new(picked.address().into(), 24).unwrap())
        );
        assert_eq!(
            *host.published.lock().unwrap(),
            [(None, Some(picked), Some(picked))]
        );
    }

    #[tokio::test]
    async fn service_holds_host_permit_through_store_and_event_commit() {
        let host = Arc::new(RecordingHost::default());
        host.hold_apply_permit.store(true, Ordering::Release);
        let (service, _runtime_config) = service(
            DhcpIpv4RouteSnapshot {
                has_routes: true,
                my_peer_id: TEST_PEER_ID,
                used_ipv4: HashSet::from(["198.18.0.2/24".parse().unwrap()]),
            },
            host.clone(),
        );

        service.reconcile_once().await;

        let expected = Some(
            IpPrefix::new(
                expected_pick(&["198.18.0.2/24"], TEST_PEER_ID)
                    .address()
                    .into(),
                24,
            )
            .unwrap(),
        );
        assert!(host.published_with_permit.load(Ordering::Acquire));
        assert_eq!(
            host.published_runtime_ipv4.lock().unwrap().as_slice(),
            &[expected]
        );
        assert!(!host.permit_held.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn service_retries_change_when_host_apply_fails() {
        let host = Arc::new(RecordingHost::default());
        host.fail_apply.store(true, Ordering::Release);
        let (service, runtime_config) = service(
            DhcpIpv4RouteSnapshot {
                has_routes: true,
                my_peer_id: TEST_PEER_ID,
                used_ipv4: HashSet::from(["198.18.0.2/24".parse().unwrap()]),
            },
            host.clone(),
        );

        service.reconcile_once().await;
        service.reconcile_once().await;

        assert_eq!(service.current(), None);
        assert_eq!(host.changes.lock().unwrap().len(), 2);
        assert_eq!(
            runtime_config.snapshot().peer.runtime.core.routes.ipv4,
            None
        );
        assert!(host.published.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn interface_close_resets_previous_allocation_before_reapply() {
        let host = Arc::new(RecordingHost::default());
        let (service, _runtime_config) = service(
            DhcpIpv4RouteSnapshot {
                has_routes: true,
                my_peer_id: TEST_PEER_ID,
                used_ipv4: HashSet::from(["198.18.0.2/24".parse().unwrap()]),
            },
            host.clone(),
        );
        service.reconcile_once().await;
        host.interface_closed.store(true, Ordering::Release);

        service.reconcile_once().await;

        let picked = expected_pick(&["198.18.0.2/24"], TEST_PEER_ID);
        assert_eq!(
            host.changes.lock().unwrap().as_slice(),
            [(None, Some(picked)), (None, Some(picked))]
        );
    }
}
