//! Unit tests for OSPF peer routing.

use super::*;
use crate::packet::ZCPacket;
use crate::peers::peer_rpc::PeerRpcManagerTransport;
use crate::peers::route::{DefaultRouteCostCalculator, RouteInterface};
use crate::peers::test_support::NoopPeerContext;
use parking_lot::Mutex;
use tokio::sync::Notify;

impl PeerRouteServiceImpl {
    pub(crate) async fn list_peers_from_interface<T: FromIterator<PeerId>>(&self) -> T {
        self.interface_peer_snapshot()
            .await
            .peers
            .iter()
            .copied()
            .collect()
    }
}

impl PeerRoute {
    pub(crate) fn task_count(&self) -> usize {
        let route_tasks = self.tasks.lock().unwrap().len();
        let session_tasks = self
            .service_impl
            .sessions
            .iter()
            .filter(|session| session.task.is_running())
            .count();
        route_tasks + session_tasks
    }
}

struct CountingInterface {
    my_peer_id: PeerId,
    peers: Arc<Mutex<Vec<PeerId>>>,
    peer_identity_types: Arc<Mutex<HashMap<PeerId, Option<PeerIdentityType>>>>,
    list_peers_calls: Arc<AtomicU32>,
    get_peer_identity_type_calls: Arc<AtomicU32>,
}

struct PeriodicRequeryInterface {
    peers: Vec<PeerId>,
    list_peers_calls: Arc<AtomicU32>,
}

struct BlockingInterface {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[derive(Default)]
struct TogglePeerRelayContext {
    enabled: AtomicBool,
}

impl PeerContext for TogglePeerRelayContext {
    fn network_identity(&self) -> CoreNetworkIdentity {
        CoreNetworkIdentity::default()
    }

    fn flags(&self) -> crate::proto::common::FlagsInConfig {
        crate::proto::common::FlagsInConfig {
            prefer_peer_relay: self.enabled.load(Ordering::Relaxed),
            ..Default::default()
        }
    }
}

#[async_trait::async_trait]
impl RouteInterface for BlockingInterface {
    async fn list_peers(&self) -> Vec<PeerId> {
        self.entered.notify_one();
        self.release.notified().await;
        vec![2]
    }

    async fn get_peer_identity_type(&self, _peer_id: PeerId) -> Option<PeerIdentityType> {
        Some(PeerIdentityType::Admin)
    }

    fn my_peer_id(&self) -> PeerId {
        1
    }
}

struct TestPeerRpcTransport;

#[async_trait::async_trait]
impl PeerRpcManagerTransport for TestPeerRpcTransport {
    fn my_peer_id(&self) -> PeerId {
        1
    }

    async fn send(&self, _msg: ZCPacket, _dst_peer_id: PeerId) -> anyhow::Result<()> {
        Ok(())
    }

    async fn recv(&self) -> anyhow::Result<ZCPacket> {
        std::future::pending().await
    }
}

struct TestPublicIpv6Runtime;

#[async_trait::async_trait]
impl PublicIpv6Runtime for TestPublicIpv6Runtime {
    fn ipv6_public_addr_auto(&self) -> bool {
        false
    }

    fn ipv6_public_addr_provider(&self) -> bool {
        false
    }

    fn instance_id(&self) -> uuid::Uuid {
        uuid::Uuid::nil()
    }

    fn network_name(&self) -> String {
        "default".to_owned()
    }

    async fn collect_reserved_public_ipv6_addrs(&self, _prefix: Ipv6Cidr) -> HashSet<Ipv6Addr> {
        HashSet::new()
    }

    fn public_ipv6_lease_changed(&self, _old: Option<Ipv6Inet>, _new: Option<Ipv6Inet>) {}

    fn public_ipv6_routes_changed(&self, _added: Vec<Ipv6Inet>, _removed: Vec<Ipv6Inet>) {}
}

#[async_trait::async_trait]
impl RouteInterface for CountingInterface {
    async fn list_peers(&self) -> Vec<PeerId> {
        self.list_peers_calls.fetch_add(1, Ordering::Relaxed);
        self.peers.lock().clone()
    }

    async fn get_peer_identity_type(&self, peer_id: PeerId) -> Option<PeerIdentityType> {
        self.get_peer_identity_type_calls
            .fetch_add(1, Ordering::Relaxed);
        self.peer_identity_types
            .lock()
            .get(&peer_id)
            .copied()
            .flatten()
    }

    async fn get_peer_public_key(&self, peer_id: PeerId) -> Option<Vec<u8>> {
        Some(vec![peer_id as u8; 32])
    }

    fn my_peer_id(&self) -> PeerId {
        self.my_peer_id
    }
}

#[async_trait::async_trait]
impl RouteInterface for PeriodicRequeryInterface {
    async fn list_peers(&self) -> Vec<PeerId> {
        self.list_peers_calls.fetch_add(1, Ordering::Relaxed);
        self.peers.clone()
    }

    fn my_peer_id(&self) -> PeerId {
        1
    }

    fn need_periodic_requery_peers(&self) -> bool {
        true
    }

    async fn get_peer_identity_type(&self, _peer_id: PeerId) -> Option<PeerIdentityType> {
        Some(PeerIdentityType::Admin)
    }

    async fn get_peer_public_key(&self, peer_id: PeerId) -> Option<Vec<u8>> {
        Some(vec![peer_id as u8; 32])
    }
}

fn test_service_impl(my_peer_id: PeerId) -> PeerRouteServiceImpl {
    PeerRouteServiceImpl::new(my_peer_id, Arc::new(NoopPeerContext::default()))
}

fn test_peer_relay_service_impl(my_peer_id: PeerId) -> PeerRouteServiceImpl {
    let flags = crate::proto::common::FlagsInConfig {
        prefer_peer_relay: true,
        ..Default::default()
    };
    PeerRouteServiceImpl::new(
        my_peer_id,
        Arc::new(NoopPeerContext::default().with_flags(flags)),
    )
}

fn interface_peer_snapshot(
    peers: impl IntoIterator<Item = (PeerId, PeerIdentityType, Option<Vec<u8>>)>,
) -> InterfacePeerSnapshot {
    let peers: Vec<_> = peers.into_iter().collect();
    InterfacePeerSnapshot {
        generation: 1,
        peers: peers.iter().map(|(peer_id, _, _)| *peer_id).collect(),
        identity_types: peers
            .iter()
            .map(|(peer_id, identity, _)| (*peer_id, Some(*identity)))
            .collect(),
        public_keys: peers
            .into_iter()
            .map(|(peer_id, _, public_key)| (peer_id, public_key))
            .collect(),
    }
}

fn install_peer_info(
    service_impl: &PeerRouteServiceImpl,
    peer_id: PeerId,
    advertised_public_key: Vec<u8>,
    avoid_relay_data: bool,
) {
    let feature_flag = crate::proto::common::PeerFeatureFlag {
        avoid_relay_data,
        ..Default::default()
    };
    service_impl.synced_route_info.peer_infos.write().insert(
        peer_id,
        RoutePeerInfo {
            peer_id,
            version: 1,
            feature_flag: Some(feature_flag),
            noise_static_pubkey: advertised_public_key,
            ..Default::default()
        },
    );
}

fn install_credential_grant(
    service_impl: &PeerRouteServiceImpl,
    public_key: Vec<u8>,
    allow_relay: bool,
) {
    service_impl
        .synced_route_info
        .trusted_credential_pubkeys
        .write()
        .insert(
            public_key,
            TrustedCredentialPubkey {
                allow_relay,
                ..Default::default()
            },
        );
}

fn install_conn_row(
    service_impl: &PeerRouteServiceImpl,
    peer_id: PeerId,
    connected_peers: impl IntoIterator<Item = PeerId>,
) {
    service_impl.synced_route_info.conn_map.write().insert(
        peer_id,
        RouteConnInfo {
            connected_peers: connected_peers.into_iter().collect(),
            version: 1.into(),
            last_update: SystemTime::now(),
        },
    );
}

async fn test_route_with_admin_peer(
    context: ArcPeerContext,
) -> (Arc<PeerRoute>, Arc<PeerRpcManager>) {
    let peer_rpc = Arc::new(PeerRpcManager::new(TestPeerRpcTransport));
    let route = PeerRoute::new(
        1,
        context,
        Arc::new(TestPublicIpv6Runtime),
        peer_rpc.clone(),
    );
    *route.service_impl.interface.lock().await = Some(Box::new(CountingInterface {
        my_peer_id: 1,
        peers: Arc::new(Mutex::new(vec![2])),
        peer_identity_types: Arc::new(Mutex::new(HashMap::from([(
            2,
            Some(PeerIdentityType::Admin),
        )]))),
        list_peers_calls: Arc::new(AtomicU32::new(0)),
        get_peer_identity_type_calls: Arc::new(AtomicU32::new(0)),
    }));
    (route, peer_rpc)
}

fn peer(peer_id: PeerId) -> OspfPeerInfo {
    OspfPeerInfo {
        peer_id,
        info: RoutePeerInfo {
            peer_id,
            version: 1,
            ..Default::default()
        },
    }
}

fn connected(
    peer_id: PeerId,
    connected_peers: impl IntoIterator<Item = PeerId>,
) -> OspfPeerConnInfo {
    OspfPeerConnInfo {
        peer_id,
        connected_peers: connected_peers.into_iter().collect(),
    }
}

#[test]
fn trusted_credential_replacement_is_atomic_for_readers() {
    const CREDENTIAL_COUNT: u32 = 16_384;
    const REPLACEMENT_COUNT: usize = 16;

    let service_impl = test_service_impl(1);
    let credentials: HashMap<_, _> = (0..CREDENTIAL_COUNT)
        .map(|id| {
            (
                id.to_le_bytes().to_vec(),
                TrustedCredentialPubkey {
                    allow_relay: true,
                    ..Default::default()
                },
            )
        })
        .collect();
    service_impl
        .synced_route_info
        .replace_trusted_credential_pubkeys(&credentials);

    let keys: Vec<_> = credentials.keys().cloned().collect();
    let start = std::sync::Barrier::new(2);

    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            start.wait();
            for _ in 0..REPLACEMENT_COUNT {
                service_impl
                    .synced_route_info
                    .replace_trusted_credential_pubkeys(&credentials);
            }
        });

        start.wait();
        let mut snapshots_read = 0;
        while !writer.is_finished() {
            assert!(keys.iter().all(|key| {
                service_impl
                    .synced_route_info
                    .get_credential_info_by_pubkey(key)
                    .is_some()
            }));
            snapshots_read += 1;
        }
        writer.join().unwrap();
        assert!(snapshots_read > 0);
    });
}

#[test]
fn peer_relay_projection_suppresses_only_covered_credential_leaves() {
    let service_impl = test_peer_relay_service_impl(1);
    let relay_key = vec![2; 32];
    let leaf_a_key = vec![3; 32];
    let leaf_b_key = vec![4; 32];
    let snapshot = interface_peer_snapshot([
        (2, PeerIdentityType::Credential, Some(relay_key.clone())),
        (3, PeerIdentityType::Credential, Some(leaf_a_key.clone())),
        (4, PeerIdentityType::Credential, Some(leaf_b_key.clone())),
        (5, PeerIdentityType::Admin, None),
    ]);

    for peer_id in 1..=5 {
        install_peer_info(&service_impl, peer_id, vec![peer_id as u8; 32], false);
    }
    install_credential_grant(&service_impl, relay_key, true);
    install_credential_grant(&service_impl, leaf_a_key, false);
    install_credential_grant(&service_impl, leaf_b_key, false);
    install_conn_row(&service_impl, 2, [3, 4]);

    assert!(service_impl.reconcile_my_conn_info(&snapshot, false));
    assert_eq!(
        service_impl
            .synced_route_info
            .get_connected_peers::<BTreeSet<_>>(1),
        Some(BTreeSet::from([2, 5]))
    );

    *service_impl.cached_interface_peer_snapshot.lock().unwrap() = Arc::new(snapshot.clone());
    let local_snapshot = service_impl.local_route_snapshot();
    assert_eq!(
        local_snapshot
            .conn_map
            .iter()
            .find(|row| row.peer_id == 1)
            .unwrap()
            .connected_peers,
        snapshot.peers
    );

    service_impl.update_route_table();
    assert_eq!(
        service_impl
            .route_table
            .get_next_hop(3)
            .unwrap()
            .next_hop_peer_id,
        3
    );
}

#[test]
fn peer_relay_projection_uses_authenticated_public_key() {
    let service_impl = test_peer_relay_service_impl(1);
    let authenticated_key = vec![2; 32];
    let forged_relay_key = vec![9; 32];
    let snapshot = interface_peer_snapshot([
        (
            2,
            PeerIdentityType::Credential,
            Some(authenticated_key.clone()),
        ),
        (4, PeerIdentityType::Credential, Some(vec![4; 32])),
    ]);
    install_peer_info(&service_impl, 2, forged_relay_key.clone(), false);
    install_credential_grant(&service_impl, authenticated_key, false);
    install_credential_grant(&service_impl, forged_relay_key, true);
    install_conn_row(&service_impl, 2, [4]);

    assert_eq!(
        service_impl.derive_advertised_connected_peers(&snapshot),
        snapshot.peers
    );
}

#[test]
fn peer_relay_projection_is_disabled_by_default() {
    let service_impl = test_service_impl(1);
    let snapshot = interface_peer_snapshot([
        (2, PeerIdentityType::Credential, Some(vec![2; 32])),
        (4, PeerIdentityType::Credential, Some(vec![4; 32])),
    ]);
    install_peer_info(&service_impl, 2, vec![2; 32], false);
    install_credential_grant(&service_impl, vec![2; 32], true);
    install_conn_row(&service_impl, 2, [4]);

    assert_eq!(
        service_impl.derive_advertised_connected_peers(&snapshot),
        snapshot.peers
    );
}

#[test]
fn peer_relay_projection_restores_edges_after_last_coverage_disappears() {
    let service_impl = test_peer_relay_service_impl(1);
    let snapshot = interface_peer_snapshot([
        (2, PeerIdentityType::Credential, Some(vec![2; 32])),
        (3, PeerIdentityType::Credential, Some(vec![3; 32])),
        (4, PeerIdentityType::Credential, Some(vec![4; 32])),
        (5, PeerIdentityType::Credential, Some(vec![5; 32])),
    ]);
    for relay_peer_id in [2, 3] {
        install_peer_info(
            &service_impl,
            relay_peer_id,
            vec![relay_peer_id as u8; 32],
            false,
        );
        install_credential_grant(&service_impl, vec![relay_peer_id as u8; 32], true);
    }
    install_conn_row(&service_impl, 2, [4]);
    install_conn_row(&service_impl, 3, [4, 5]);

    assert!(service_impl.reconcile_my_conn_info(&snapshot, false));
    let first_version = service_impl
        .synced_route_info
        .conn_map
        .read()
        .get(&1)
        .unwrap()
        .version
        .get();
    assert_eq!(
        service_impl
            .synced_route_info
            .get_connected_peers::<BTreeSet<_>>(1),
        Some(BTreeSet::from([2, 3]))
    );

    service_impl.synced_route_info.conn_map.write().remove(&2);
    assert!(!service_impl.reconcile_my_conn_info(&snapshot, false));

    service_impl.synced_route_info.conn_map.write().remove(&3);
    assert!(service_impl.reconcile_my_conn_info(&snapshot, false));
    let self_row = service_impl.synced_route_info.conn_map.read();
    let self_row = self_row.get(&1).unwrap();
    assert_eq!(self_row.connected_peers, snapshot.peers);
    assert_eq!(self_row.version.get(), first_version + 1);
}

#[test]
fn peer_relay_projection_ignores_ineligible_relays() {
    let service_impl = test_peer_relay_service_impl(1);
    let snapshot = interface_peer_snapshot([
        (2, PeerIdentityType::Credential, Some(vec![2; 32])),
        (3, PeerIdentityType::Credential, Some(vec![3; 32])),
        (4, PeerIdentityType::Admin, Some(vec![4; 32])),
        (5, PeerIdentityType::Credential, Some(vec![5; 32])),
        (6, PeerIdentityType::Credential, Some(vec![6; 32])),
    ]);
    install_peer_info(&service_impl, 2, vec![2; 32], false);
    install_peer_info(&service_impl, 3, vec![3; 32], true);
    install_peer_info(&service_impl, 4, vec![4; 32], false);
    install_peer_info(&service_impl, 5, vec![5; 32], false);
    for peer_id in [2, 3, 4, 5] {
        install_credential_grant(&service_impl, vec![peer_id as u8; 32], peer_id != 2);
        install_conn_row(&service_impl, peer_id, [6]);
    }
    service_impl
        .synced_route_info
        .suppressed_non_reusable_credential_peers
        .insert(5, ());

    assert_eq!(
        service_impl.derive_advertised_connected_peers(&snapshot),
        snapshot.peers
    );
}

#[test]
fn peer_relay_projection_refreshes_local_topology_when_advertisement_is_unchanged() {
    let service_impl = test_peer_relay_service_impl(1);
    install_peer_info(&service_impl, 2, vec![2; 32], false);
    install_credential_grant(&service_impl, vec![2; 32], true);
    install_conn_row(&service_impl, 2, [3, 4]);
    let first = interface_peer_snapshot([
        (2, PeerIdentityType::Credential, Some(vec![2; 32])),
        (3, PeerIdentityType::Credential, Some(vec![3; 32])),
    ]);
    assert!(service_impl.reconcile_my_conn_info(&first, false));
    let self_version = service_impl
        .synced_route_info
        .conn_map
        .read()
        .get(&1)
        .unwrap()
        .version
        .get();
    let route_version = service_impl.synced_route_info.version.get();

    let second = interface_peer_snapshot([
        (2, PeerIdentityType::Credential, Some(vec![2; 32])),
        (4, PeerIdentityType::Credential, Some(vec![4; 32])),
    ]);
    assert!(service_impl.reconcile_my_conn_info(&second, true));
    assert_eq!(
        service_impl
            .synced_route_info
            .conn_map
            .read()
            .get(&1)
            .unwrap()
            .version
            .get(),
        self_version
    );
    assert_eq!(
        service_impl.synced_route_info.version.get(),
        route_version + 1
    );
}

#[tokio::test]
async fn peer_relay_projection_reconciles_forwarded_row_without_interface_change() {
    let service_impl = test_peer_relay_service_impl(1);
    let peers = Arc::new(Mutex::new(vec![2, 4]));
    let list_peers_calls = Arc::new(AtomicU32::new(0));
    *service_impl.interface.lock().await = Some(Box::new(CountingInterface {
        my_peer_id: 1,
        peers,
        peer_identity_types: Arc::new(Mutex::new(HashMap::from([
            (2, Some(PeerIdentityType::Credential)),
            (4, Some(PeerIdentityType::Credential)),
        ]))),
        list_peers_calls: list_peers_calls.clone(),
        get_peer_identity_type_calls: Arc::new(AtomicU32::new(0)),
    }));
    install_peer_info(&service_impl, 2, vec![2; 32], false);
    install_credential_grant(&service_impl, vec![2; 32], true);

    assert!(service_impl.update_my_conn_info().await);
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        service_impl
            .synced_route_info
            .get_connected_peers::<BTreeSet<_>>(1),
        Some(BTreeSet::from([2, 4]))
    );

    install_conn_row(&service_impl, 2, [4]);
    assert!(service_impl.update_my_conn_info().await);
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        service_impl
            .synced_route_info
            .get_connected_peers::<BTreeSet<_>>(1),
        Some(BTreeSet::from([2]))
    );
}

#[tokio::test]
async fn interface_peer_cache_refreshes_only_when_marked_dirty() {
    let service_impl = test_service_impl(1);
    let peers = Arc::new(Mutex::new(vec![2, 3]));
    let peer_identity_types = Arc::new(Mutex::new(HashMap::new()));
    let list_peers_calls = Arc::new(AtomicU32::new(0));
    let get_peer_identity_type_calls = Arc::new(AtomicU32::new(0));
    *service_impl.interface.lock().await = Some(Box::new(CountingInterface {
        my_peer_id: 1,
        peers: peers.clone(),
        peer_identity_types,
        list_peers_calls: list_peers_calls.clone(),
        get_peer_identity_type_calls,
    }));

    let first: BTreeSet<_> = service_impl.list_peers_from_interface().await;
    let second: BTreeSet<_> = service_impl.list_peers_from_interface().await;

    assert_eq!(first, BTreeSet::from([2, 3]));
    assert_eq!(second, BTreeSet::from([2, 3]));
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 1);

    *peers.lock() = vec![2, 4];
    service_impl.handle_peer_context_event(&PeerContextEvent::PeerConnAdded);

    let third: BTreeSet<_> = service_impl.list_peers_from_interface().await;
    assert_eq!(third, BTreeSet::from([2, 4]));
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn update_my_conn_info_skips_interface_scan_when_topology_is_unchanged() {
    let service_impl = test_service_impl(1);
    let peers = Arc::new(Mutex::new(vec![2, 3]));
    let peer_identity_types = Arc::new(Mutex::new(HashMap::new()));
    let list_peers_calls = Arc::new(AtomicU32::new(0));
    let get_peer_identity_type_calls = Arc::new(AtomicU32::new(0));
    *service_impl.interface.lock().await = Some(Box::new(CountingInterface {
        my_peer_id: 1,
        peers: peers.clone(),
        peer_identity_types,
        list_peers_calls: list_peers_calls.clone(),
        get_peer_identity_type_calls: get_peer_identity_type_calls.clone(),
    }));

    assert!(service_impl.update_my_conn_info().await);
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 1);
    assert_eq!(get_peer_identity_type_calls.load(Ordering::Relaxed), 2);

    assert!(!service_impl.update_my_conn_info().await);
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 1);
    assert_eq!(get_peer_identity_type_calls.load(Ordering::Relaxed), 2);

    *peers.lock() = vec![2, 4];
    service_impl.handle_peer_context_event(&PeerContextEvent::PeerConnRemoved);

    assert!(service_impl.update_my_conn_info().await);
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 2);
    assert_eq!(get_peer_identity_type_calls.load(Ordering::Relaxed), 4);

    assert!(!service_impl.update_my_conn_info().await);
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 2);
    assert_eq!(get_peer_identity_type_calls.load(Ordering::Relaxed), 4);
}

#[tokio::test]
async fn shutdown_withdrawal_preserves_local_physical_routes() {
    let (route, _peer_rpc) = test_route_with_admin_peer(Arc::new(NoopPeerContext::default())).await;

    assert!(route.service_impl.update_my_infos().await);
    assert_eq!(
        route
            .service_impl
            .synced_route_info
            .get_connected_peers::<BTreeSet<_>>(1),
        Some(BTreeSet::from([2]))
    );
    let previous_version = route
        .service_impl
        .synced_route_info
        .conn_map
        .read()
        .get(&1)
        .unwrap()
        .version
        .get();

    assert!(route.withdraw_self_conn_info().await);
    {
        let conn_map = route.service_impl.synced_route_info.conn_map.read();
        let withdrawn = conn_map.get(&1).unwrap();
        assert!(withdrawn.connected_peers.is_empty());
        assert!(withdrawn.version.get() > previous_version);
    }

    let local_snapshot = route.service_impl.local_route_snapshot();
    assert_eq!(
        local_snapshot
            .conn_map
            .iter()
            .find(|row| row.peer_id == 1)
            .unwrap()
            .connected_peers,
        BTreeSet::from([2])
    );

    route.service_impl.mark_interface_peers_dirty();
    assert!(!route.service_impl.update_my_conn_info().await);
    assert!(
        route
            .service_impl
            .synced_route_info
            .get_connected_peers::<BTreeSet<_>>(1)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn empty_self_conn_info_round_trips_through_list_and_bitmap() {
    let source = test_service_impl(1);
    *source.interface.lock().await = Some(Box::new(CountingInterface {
        my_peer_id: 1,
        peers: Arc::new(Mutex::new(vec![2])),
        peer_identity_types: Arc::new(Mutex::new(HashMap::from([(
            2,
            Some(PeerIdentityType::Admin),
        )]))),
        list_peers_calls: Arc::new(AtomicU32::new(0)),
        get_peer_identity_type_calls: Arc::new(AtomicU32::new(0)),
    }));
    assert!(source.update_my_infos().await);
    source
        .self_conn_info_withdrawn
        .store(true, Ordering::Release);
    source.mark_interface_peers_dirty();
    assert!(source.update_my_infos().await);

    let session = SyncRouteSession::new(1, 2);
    let mut estimated_size = 0;
    let peer_list = source
        .build_conn_peer_list(&session, &mut estimated_size)
        .expect("withdrawn row should remain in the peer list");
    let listed = peer_list
        .peer_conn_infos
        .iter()
        .find(|info| info.peer_id.is_some_and(|id| id.peer_id == 1))
        .expect("peer list should carry the withdrawn self row");
    assert!(listed.connected_peer_ids.is_empty());

    let list_receiver = test_service_impl(2);
    install_conn_row(&list_receiver, 1, [2]);
    list_receiver
        .synced_route_info
        .update_conn_info_with_list(&peer_list);
    assert!(
        list_receiver
            .synced_route_info
            .get_connected_peers::<BTreeSet<_>>(1)
            .unwrap()
            .is_empty()
    );

    let bitmap = source.build_conn_bitmap();
    let self_index = bitmap
        .peer_ids
        .iter()
        .position(|id| id.peer_id == 1)
        .expect("bitmap should carry the withdrawn self row");
    assert!(bitmap.get_connected_peers(self_index).is_empty());

    let bitmap_receiver = test_service_impl(2);
    install_conn_row(&bitmap_receiver, 1, [2]);
    bitmap_receiver
        .synced_route_info
        .update_conn_info_with_bitmap(&bitmap);
    assert!(
        bitmap_receiver
            .synced_route_info
            .get_connected_peers::<BTreeSet<_>>(1)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn periodic_requery_without_peer_change_keeps_route_version_stable() {
    let service_impl = test_peer_relay_service_impl(1);
    let list_peers_calls = Arc::new(AtomicU32::new(0));
    *service_impl.interface.lock().await = Some(Box::new(PeriodicRequeryInterface {
        peers: vec![2],
        list_peers_calls: list_peers_calls.clone(),
    }));

    assert!(service_impl.update_my_conn_info().await);
    let route_version = service_impl.synced_route_info.version.get();

    assert!(!service_impl.update_my_conn_info().await);
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 2);
    assert_eq!(service_impl.synced_route_info.version.get(), route_version);
}

#[tokio::test]
async fn enabling_peer_relay_refreshes_authenticated_interface_metadata() {
    let context = Arc::new(TogglePeerRelayContext::default());
    let service_impl = PeerRouteServiceImpl::new(1, context.clone());
    let peer_identity_types = Arc::new(Mutex::new(HashMap::from([
        (2, Some(PeerIdentityType::Credential)),
        (3, Some(PeerIdentityType::Credential)),
    ])));
    let list_peers_calls = Arc::new(AtomicU32::new(0));
    *service_impl.interface.lock().await = Some(Box::new(CountingInterface {
        my_peer_id: 1,
        peers: Arc::new(Mutex::new(vec![2, 3])),
        peer_identity_types,
        list_peers_calls: list_peers_calls.clone(),
        get_peer_identity_type_calls: Arc::new(AtomicU32::new(0)),
    }));
    install_peer_info(&service_impl, 2, vec![2; 32], false);
    install_credential_grant(&service_impl, vec![2; 32], true);
    install_conn_row(&service_impl, 2, [3]);

    assert!(service_impl.update_my_conn_info().await);
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        service_impl
            .synced_route_info
            .get_connected_peers::<BTreeSet<_>>(1),
        Some(BTreeSet::from([2, 3]))
    );

    context.enabled.store(true, Ordering::Relaxed);

    assert!(service_impl.update_my_conn_info().await);
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        service_impl
            .synced_route_info
            .get_connected_peers::<BTreeSet<_>>(1),
        Some(BTreeSet::from([2]))
    );
}

#[tokio::test]
async fn get_peer_identity_type_reuses_snapshot_until_topology_changes() {
    let service_impl = test_service_impl(1);
    let peers = Arc::new(Mutex::new(vec![2, 3]));
    let peer_identity_types = Arc::new(Mutex::new(HashMap::from([
        (2, Some(PeerIdentityType::Credential)),
        (3, Some(PeerIdentityType::Admin)),
        (4, Some(PeerIdentityType::Admin)),
    ])));
    let list_peers_calls = Arc::new(AtomicU32::new(0));
    let get_peer_identity_type_calls = Arc::new(AtomicU32::new(0));
    *service_impl.interface.lock().await = Some(Box::new(CountingInterface {
        my_peer_id: 1,
        peers: peers.clone(),
        peer_identity_types: peer_identity_types.clone(),
        list_peers_calls: list_peers_calls.clone(),
        get_peer_identity_type_calls: get_peer_identity_type_calls.clone(),
    }));

    assert_eq!(
        service_impl.get_peer_identity_type_from_interface(2).await,
        Some(PeerIdentityType::Credential)
    );
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 1);
    assert_eq!(get_peer_identity_type_calls.load(Ordering::Relaxed), 2);

    assert_eq!(
        service_impl.get_peer_identity_type_from_interface(2).await,
        Some(PeerIdentityType::Credential)
    );
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 1);
    assert_eq!(get_peer_identity_type_calls.load(Ordering::Relaxed), 2);

    *peers.lock() = vec![2, 4];
    service_impl.handle_peer_context_event(&PeerContextEvent::PeerConnRemoved);

    assert_eq!(
        service_impl.get_peer_identity_type_from_interface(4).await,
        Some(PeerIdentityType::Admin)
    );
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 2);
    assert_eq!(get_peer_identity_type_calls.load(Ordering::Relaxed), 4);

    assert_eq!(
        service_impl.get_peer_identity_type_from_interface(4).await,
        Some(PeerIdentityType::Admin)
    );
    assert_eq!(list_peers_calls.load(Ordering::Relaxed), 2);
    assert_eq!(get_peer_identity_type_calls.load(Ordering::Relaxed), 4);
}

#[tokio::test]
async fn missing_peer_resync_recovers_expired_multihop_route() {
    for use_bitmap in [false, true] {
        // B(1) -- C(2) -- A(3) -- D(4). Only B expires A's old metadata.
        let (receiver, _peer_rpc) =
            test_route_with_admin_peer(Arc::new(NoopPeerContext::default())).await;
        let service = &receiver.service_impl;
        service.update_my_infos().await;
        service.get_or_create_session(2);
        let sender = test_service_impl(2);
        let old = SystemTime::now() - Duration::from_secs(120);
        for peer_id in 1..=4 {
            install_peer_info(&sender, peer_id, Vec::new(), false);
            if peer_id != 1 {
                install_peer_info(service, peer_id, Vec::new(), false);
            }
        }
        for (_, info) in sender.synced_route_info.peer_infos.write().iter_mut() {
            info.last_update = Some(old.into());
            info.feature_flag.as_mut().unwrap().support_conn_list_sync = true;
        }
        for (peer_id, info) in service.synced_route_info.peer_infos.write().iter_mut() {
            info.last_update = Some(
                if *peer_id == 3 {
                    old
                } else {
                    SystemTime::now()
                }
                .into(),
            );
        }
        for (peer_id, neighbors) in [(1, vec![2]), (2, vec![1, 3]), (3, vec![2, 4]), (4, vec![3])] {
            install_conn_row(&sender, peer_id, neighbors.clone());
            install_conn_row(service, peer_id, neighbors);
        }
        install_conn_row(service, 2, [1]);
        sender
            .synced_route_info
            .conn_map
            .read()
            .get(&2)
            .unwrap()
            .version
            .set_if_larger(2);
        service.update_route_table_and_cached_local_conn_bitmap();
        assert!(!service.route_table.topology_peer_reachable(3));
        service.clear_expired_peer().await;
        assert!(!service.synced_route_info.peer_infos.read().contains_key(&3));

        sender.synced_route_info.version.inc();
        sender.update_route_table_and_cached_local_conn_bitmap();
        let session = sender.get_or_create_session(1);
        let infos: Vec<_> = sender
            .synced_route_info
            .peer_infos
            .read()
            .values()
            .cloned()
            .collect();
        session.update_dst_saved_peer_info_version(&infos, 1);
        session.update_dst_saved_conn_info_version(&sender.build_conn_bitmap().into(), 1);
        let cursor = SystemTime::now();
        session.update_last_sync_succ_timestamp(cursor);
        assert!(sender.build_route_info(&session).is_none());

        let conn_info = if use_bitmap {
            sender.build_conn_bitmap().into()
        } else {
            RouteConnPeerList {
                peer_conn_infos: vec![PeerConnInfo {
                    peer_id: Some(PeerIdVersion {
                        peer_id: 2,
                        version: 2,
                    }),
                    connected_peer_ids: vec![1, 3],
                }],
            }
            .into()
        };
        let response = receiver
            .session_mgr
            .do_sync_route_info(2, 10, true, None, None, Some(conn_info), None)
            .await
            .unwrap();
        assert_eq!(response.missing_peer_ids, vec![3]);
        assert!(!service.route_table.topology_peer_reachable(4));

        // Lose the first response. A retry/keepalive without topology still
        // reports the missing peer; no periodic full topology scan is needed.
        let response = receiver
            .session_mgr
            .do_sync_route_info(2, 10, true, None, None, None, None)
            .await
            .unwrap();
        assert_eq!(response.missing_peer_ids, vec![3]);
        sender.request_missing_peer_infos(&session, &response);
        assert_eq!(session.last_sync_succ_timestamp.load(), Some(cursor));
        assert!(session.check_saved_peer_info_update_to_date(4, 1));

        let (infos, conn_info, foreign) = sender.build_sync_request(&session, 1);
        let infos = infos.unwrap();
        assert_eq!(infos.iter().map(|p| p.peer_id).collect::<Vec<_>>(), vec![3]);
        assert!(foreign.is_none());
        if use_bitmap {
            assert!(conn_info.is_none());
        } else {
            let Some(ConnInfo::ConnPeerList(list)) = &conn_info else {
                panic!("a single missing row should use the smaller incremental list");
            };
            assert_eq!(list.peer_conn_infos.len(), 1);
            assert_eq!(list.peer_conn_infos[0].peer_id.unwrap().peer_id, 3);
            assert_eq!(list.peer_conn_infos[0].connected_peer_ids, vec![2, 4]);
        }

        // A failed supplement must remain eligible despite the old timestamp.
        assert_eq!(sender.build_route_info(&session).unwrap(), infos);
        assert_eq!(sender.build_conn_info(&session, 1), conn_info);

        // Metadata alone is insufficient when the connection row was also lost.
        let response = receiver
            .session_mgr
            .do_sync_route_info(
                2,
                10,
                true,
                Some(infos.clone()),
                Some(infos.iter().map(raw_route_peer_info).collect()),
                None,
                None,
            )
            .await
            .unwrap();
        assert!(response.missing_peer_ids.is_empty());
        assert_eq!(
            response.missing_conn_peer_ids,
            if use_bitmap { vec![] } else { vec![3] }
        );
        session.update_dst_saved_peer_info_version(&infos, 1);
        sender.request_missing_peer_infos(&session, &response);
        assert!(sender.build_route_info(&session).is_none());
        if !use_bitmap {
            assert!(!service.route_table.topology_peer_reachable(4));
        }
        let response = receiver
            .session_mgr
            .do_sync_route_info(2, 10, true, None, None, conn_info.clone(), None)
            .await
            .unwrap();
        assert!(response.missing_peer_ids.is_empty());
        assert!(response.missing_conn_peer_ids.is_empty());
        assert!(service.route_table.topology_peer_reachable(4));
        assert_eq!(
            service
                .synced_route_info
                .get_peer_info_version_with_default(3),
            1
        );

        session.update_dst_saved_peer_info_version(&infos, 1);
        if let Some(conn_info) = &conn_info {
            session.update_dst_saved_conn_info_version(conn_info, 1);
        }
        sender.request_missing_peer_infos(&session, &response);
        assert!(sender.build_route_info(&session).is_none());
        assert!(sender.build_conn_info(&session, 1).is_none());
        receiver.stop().await;
    }
}

#[test]
fn missing_peer_resync_discards_collected_and_rejected_peers() {
    let service = test_service_impl(1);
    let session = SyncRouteSession::new(1, 2);
    let conn_info: ConnInfo = RouteConnPeerList {
        peer_conn_infos: vec![PeerConnInfo {
            peer_id: Some(PeerIdVersion {
                peer_id: 2,
                version: 1,
            }),
            connected_peer_ids: vec![1, 3, 4],
        }],
    }
    .into();
    service.synced_route_info.update_conn_info(&conn_info);
    assert_eq!(
        service.collect_missing_peer_ids(&session, Some(&conn_info), false, &[4]),
        (vec![2, 3], vec![3])
    );
    service.synced_route_info.remove_peer(3);
    assert_eq!(
        service.collect_missing_peer_ids(&session, None, false, &[]),
        (vec![2], vec![])
    );
    // A credential peer cannot supply metadata for other nodes.
    assert_eq!(
        service.collect_missing_peer_ids(&session, Some(&conn_info), true, &[]),
        (vec![2], vec![])
    );
    session.update_remote_state_locked(123, true);
    assert!(session.missing_peer_ids.lock().is_empty());
}

#[test]
fn missing_peer_resync_does_not_repeat_metadata_for_absent_connection_rows() {
    let service = test_service_impl(1);
    for peer_id in 1..=3 {
        install_peer_info(&service, peer_id, Vec::new(), false);
    }
    install_conn_row(&service, 1, [2]);
    let conn_info: ConnInfo = RouteConnPeerList {
        peer_conn_infos: vec![PeerConnInfo {
            peer_id: Some(PeerIdVersion {
                peer_id: 2,
                version: 1,
            }),
            connected_peer_ids: vec![1, 3],
        }],
    }
    .into();
    service.synced_route_info.update_conn_info(&conn_info);
    service.update_route_table();
    let session = SyncRouteSession::new(1, 2);
    let (missing_peer_ids, missing_conn_peer_ids) =
        service.collect_missing_peer_ids(&session, Some(&conn_info), false, &[]);
    assert!(missing_peer_ids.is_empty());
    assert_eq!(missing_conn_peer_ids, vec![3]);

    let sender_session = SyncRouteSession::new(1, 2);
    let infos = service.build_route_info(&sender_session).unwrap();
    sender_session.update_dst_saved_peer_info_version(&infos, 2);
    service.request_missing_peer_infos(
        &sender_session,
        &SyncRouteInfoResponse {
            missing_peer_ids,
            missing_conn_peer_ids,
            ..Default::default()
        },
    );
    assert!(service.build_route_info(&sender_session).is_none());
    assert!(
        sender_session
            .unreachable_peers_for_conn_info
            .lock()
            .is_empty()
    );

    // Credential endpoints without relay permission never publish a row.
    service
        .synced_route_info
        .peer_infos
        .write()
        .get_mut(&3)
        .unwrap()
        .feature_flag
        .as_mut()
        .unwrap()
        .is_credential_peer = true;
    assert_eq!(
        service.collect_missing_peer_ids(&session, None, false, &[]),
        (vec![], vec![])
    );
    // A credential allowed to relay still needs its outgoing connections.
    service
        .synced_route_info
        .peer_infos
        .write()
        .get_mut(&3)
        .unwrap()
        .noise_static_pubkey = vec![3; 32];
    install_credential_grant(&service, vec![3; 32], true);
    assert_eq!(
        service.collect_missing_peer_ids(&session, Some(&conn_info), false, &[]),
        (vec![], vec![3])
    );
}

/// P-AUDIT.8: `clear_expired_peer` removes a stale peer only when the topology
/// also says it is unreachable. While some neighbour still *claims* adjacency
/// to it, the peer survives with **stale** data and keeps appearing in
/// `list_routes` (i.e. in the status page) for as long as that claim holds.
///
/// This test pins down that behaviour so the expiry window can be reasoned
/// about precisely instead of guessed.
#[tokio::test]
async fn stale_peer_survives_while_a_neighbour_still_claims_adjacency() {
    let service = test_service_impl(1);
    let now = SystemTime::now();
    let old = now - Duration::from_secs(120);

    for peer_id in 1..=3 {
        install_peer_info(&service, peer_id, Vec::new(), false);
    }
    // me(1) <-> alive(2); alive(2) still advertises dead(3).
    install_conn_row(&service, 1, [2]);
    install_conn_row(&service, 2, [1, 3]);
    install_conn_row(&service, 3, [2]);

    // Only peer 3 went silent.
    for (peer_id, info) in service.synced_route_info.peer_infos.write().iter_mut() {
        info.last_update = Some((if *peer_id == 3 { old } else { now }).into());
    }
    service.update_route_table_and_cached_local_conn_bitmap();

    // Topology still considers 3 reachable (via 2), so expiry keeps it.
    assert!(
        service.route_table.topology_peer_reachable(3),
        "precondition: 3 is still reachable through 2"
    );
    service.clear_expired_peer().await;
    assert!(
        service.synced_route_info.peer_infos.read().contains_key(&3),
        "a stale-but-still-claimed peer must survive the 90s unreachable sweep"
    );

    // ...and it is still advertised to the UI.
    let routes = service
        .route_table
        .list_routes(1, &service.route_table_with_cost);
    assert!(
        routes.iter().any(|r| r.peer_id == 3),
        "the status page keeps a row for a peer nobody has heard from in 120s"
    );

    // Now the neighbour stops claiming it — the state every real mesh reaches
    // once its own neighbours prune the dead peer. Convergence follows.
    //
    // `next_hop_map` entries are only ever updated/inserted; an entry that
    // stops being reachable is dropped by `clean_expired_route_info` once the
    // snapshot version moves past it, so the version has to advance here too
    // (in production it advances on every route sync).
    install_conn_row(&service, 2, [1]);
    service.synced_route_info.version.set_if_larger(100);
    service.update_route_table_and_cached_local_conn_bitmap();
    service.route_table.clean_expired_route_info();
    assert!(!service.route_table.topology_peer_reachable(3));
    service.clear_expired_peer().await;
    assert!(
        !service.synced_route_info.peer_infos.read().contains_key(&3),
        "once unclaimed and stale, the peer is removed"
    );
    assert!(
        !service
            .route_table
            .list_routes(1, &service.route_table_with_cost)
            .iter()
            .any(|r| r.peer_id == 3),
        "and disappears from the status page"
    );
}

/// P-AUDIT.10: the relay-policy penalty must stay visible in the reported
/// `path_latency`.
///
/// It used to be `cost % AVOID_RELAY_COST`, which erased the penalty — so a
/// path forced by policy reported the same clean latency as a merit-chosen one
/// and the status page could not explain the routing.
#[test]
fn avoid_relay_penalty_is_visible_in_reported_path_latency() {
    // Chain 1 -> 2 -> 3. Peer 2 refuses relay duty, so the 1->2 edge (and thus
    // every path through 2) carries the policy penalty.
    let build = |avoid: bool| -> i32 {
        let service = test_service_impl(1);
        install_peer_info(&service, 1, Vec::new(), false);
        install_peer_info(&service, 2, Vec::new(), avoid);
        install_peer_info(&service, 3, Vec::new(), false);
        install_conn_row(&service, 1, [2]);
        install_conn_row(&service, 2, [1, 3]);
        install_conn_row(&service, 3, [2]);
        service.update_route_table_and_cached_local_conn_bitmap();
        service
            .route_table
            .get_topology_next_hop(3)
            .expect("3 should be reachable via 2")
            .path_latency
    };

    let clean = build(false);
    let penalised = build(true);

    // A measured quality edge is small; the penalty is far above it.
    assert!(
        clean < 1_000_000,
        "baseline path cost should be a measured quality value, got {clean}"
    );
    assert!(
        penalised > 1_000_000_000,
        "policy-forced path must report the penalty, not erase it (got {penalised})"
    );
    assert!(
        penalised > clean * 1000,
        "penalty must dominate so the policy is still effectively a veto"
    );
}

#[test]
fn missing_peer_resync_deduplicates_new_versions_and_waits_for_reachability() {
    let service = test_service_impl(1);
    for peer_id in 1..=3 {
        install_peer_info(&service, peer_id, Vec::new(), false);
    }
    install_conn_row(&service, 1, [2]);
    install_conn_row(&service, 2, [1]);
    install_conn_row(&service, 3, [2]);
    service.update_route_table_and_cached_local_conn_bitmap();
    let session = SyncRouteSession::new(1, 2);
    service.request_missing_peer_infos(
        &session,
        &SyncRouteInfoResponse {
            missing_peer_ids: vec![3, 3, 99, 2],
            missing_conn_peer_ids: vec![3, 3, 99, 2],
            ..Default::default()
        },
    );
    assert_eq!(session.unreachable_peers_for_peer_info.lock().len(), 1);
    assert!(
        !service
            .build_route_info(&session)
            .unwrap()
            .iter()
            .any(|p| p.peer_id == 3)
    );
    install_conn_row(&service, 2, [1, 3]);
    service.update_route_table_and_cached_local_conn_bitmap();
    let infos = service.build_route_info(&session).unwrap();
    assert_eq!(infos.iter().filter(|p| p.peer_id == 3).count(), 1);
    let list = service.build_conn_peer_list(&session, &mut 0).unwrap();
    assert_eq!(
        list.peer_conn_infos
            .iter()
            .filter(|p| p.peer_id.unwrap().peer_id == 3)
            .count(),
        1
    );
}

#[test]
fn stopped_sync_clears_matching_remote_initiator() {
    let session = SyncRouteSession::new(1, 2);
    let _session_lock = session.lock.lock();
    session.update_remote_state_locked(10, true);

    assert!(session.clear_dst_initiator_if_session_unchanged_locked(10));
    assert!(!session.dst_is_initiator.load(Ordering::Relaxed));
}

#[test]
fn stopped_sync_preserves_newer_remote_initiator() {
    let session = SyncRouteSession::new(1, 2);
    let _session_lock = session.lock.lock();
    session.update_remote_state_locked(10, true);

    session.update_remote_state_locked(11, true);

    assert!(!session.clear_dst_initiator_if_session_unchanged_locked(10));
    assert!(session.dst_is_initiator.load(Ordering::Relaxed));
}

#[test]
fn remote_state_change_invalidates_outbound_snapshot() {
    let service_impl = Arc::new(test_service_impl(1));
    let session = service_impl.get_or_create_session(2);
    let _session_lock = session.lock.lock();
    session.update_remote_state_locked(10, false);
    let snapshot = session.request_snapshot_locked();

    assert!(session.admit_inbound_locked(20, true));
    assert!(!service_impl.sync_request_is_current_locked(2, &session, snapshot));
    assert_eq!(session.dst_session_id.load(Ordering::Relaxed), 20);
    assert!(session.dst_is_initiator.load(Ordering::Relaxed));
    assert_eq!(session.rpc_tx_count.load(Ordering::Relaxed), 0);
    assert!(session.dst_saved_peer_info_versions.is_empty());
}

#[test]
fn replaced_session_invalidates_outbound_snapshot() {
    let service_impl = Arc::new(test_service_impl(1));
    let old_session = service_impl.get_or_create_session(2);
    let snapshot = {
        let _session_lock = old_session.lock.lock();
        old_session.request_snapshot_locked()
    };

    service_impl.remove_session(2);
    let new_session = service_impl.get_or_create_session(2);
    let _old_session_lock = old_session.lock.lock();

    assert!(!Arc::ptr_eq(&old_session, &new_session));
    assert!(!service_impl.sync_request_is_current_locked(2, &old_session, snapshot));
}

#[test]
fn active_remote_initiator_rejects_different_generation() {
    let session = SyncRouteSession::new(1, 2);
    {
        let _session_lock = session.lock.lock();
        assert!(session.admit_inbound_locked(20, true));
        assert!(!session.admit_inbound_locked(10, true));
        assert_eq!(session.dst_session_id.load(Ordering::Relaxed), 20);
    }

    session
        .last_contact_instant
        .store(Instant::now() - INITIATOR_SESSION_LIVENESS_TIMEOUT - Duration::from_secs(1));
    assert!(session.clear_stale_dst_initiator());

    let _session_lock = session.lock.lock();
    assert!(session.admit_inbound_locked(10, true));
    assert_eq!(session.dst_session_id.load(Ordering::Relaxed), 10);
}

#[test]
fn local_initiator_accepts_initial_responder_generation() {
    let session = SyncRouteSession::new(1, 2);
    session.update_initiator_flag(true);

    let _session_lock = session.lock.lock();
    assert!(session.admit_inbound_locked(20, false));
    assert_eq!(session.dst_session_id.load(Ordering::Relaxed), 20);
    assert!(!session.dst_is_initiator.load(Ordering::Relaxed));
}

#[test]
fn stale_responder_session_clears_initiator_after_liveness_timeout() {
    let session = SyncRouteSession::new(1, 2);
    {
        let _session_lock = session.lock.lock();
        session.update_remote_state_locked(10, true);
    }
    session
        .last_contact_instant
        .store(Instant::now() - INITIATOR_SESSION_LIVENESS_TIMEOUT - Duration::from_secs(1));

    assert!(session.clear_stale_dst_initiator());
    assert!(!session.dst_is_initiator.load(Ordering::Relaxed));
}

#[test]
fn healthy_responder_session_keeps_initiator() {
    let session = SyncRouteSession::new(1, 2);
    {
        let _session_lock = session.lock.lock();
        session.update_remote_state_locked(10, true);
    }

    assert!(!session.clear_stale_dst_initiator());
    assert!(session.dst_is_initiator.load(Ordering::Relaxed));
}

#[test]
fn initiator_session_ignores_liveness_timeout() {
    let session = SyncRouteSession::new(1, 2);
    session.update_initiator_flag(true);
    {
        let _session_lock = session.lock.lock();
        session.update_remote_state_locked(10, true);
    }
    session
        .last_contact_instant
        .store(Instant::now() - INITIATOR_SESSION_LIVENESS_TIMEOUT - Duration::from_secs(1));

    assert!(!session.clear_stale_dst_initiator());
    assert!(session.dst_is_initiator.load(Ordering::Relaxed));
}

#[tokio::test]
async fn initiator_sync_creates_session_and_marks_contact() {
    let peer_rpc = Arc::new(PeerRpcManager::new(TestPeerRpcTransport));
    let route = PeerRoute::new(
        1,
        Arc::new(NoopPeerContext::default()),
        Arc::new(TestPublicIpv6Runtime),
        peer_rpc,
    );
    let peers = Arc::new(Mutex::new(vec![2]));
    *route.service_impl.interface.lock().await = Some(Box::new(CountingInterface {
        my_peer_id: 1,
        peers,
        peer_identity_types: Arc::new(Mutex::new(HashMap::from([(
            2,
            Some(PeerIdentityType::Admin),
        )]))),
        list_peers_calls: Arc::new(AtomicU32::new(0)),
        get_peer_identity_type_calls: Arc::new(AtomicU32::new(0)),
    }));

    route
        .session_mgr
        .do_sync_route_info(2, 1, true, None, None, None, None)
        .await
        .expect("initiator sync should succeed");

    let session = route
        .service_impl
        .get_session(2)
        .expect("initiator sync should create the session");
    assert!(session.dst_is_initiator.load(Ordering::Relaxed));
    assert!(
        session.last_contact_instant.load().elapsed() < Duration::from_secs(5),
        "inbound sync should refresh the liveness timestamp"
    );

    route.stop().await;
    assert!(route.service_impl.sessions.is_empty());
    assert_eq!(route.task_count(), 0);
}

#[tokio::test]
async fn stale_non_initiator_sync_does_not_create_session() {
    let peer_rpc = Arc::new(PeerRpcManager::new(TestPeerRpcTransport));
    let route = PeerRoute::new(
        1,
        Arc::new(NoopPeerContext::default()),
        Arc::new(TestPublicIpv6Runtime),
        peer_rpc,
    );
    let peers = Arc::new(Mutex::new(vec![2]));
    *route.service_impl.interface.lock().await = Some(Box::new(CountingInterface {
        my_peer_id: 1,
        peers,
        peer_identity_types: Arc::new(Mutex::new(HashMap::from([(
            2,
            Some(PeerIdentityType::Admin),
        )]))),
        list_peers_calls: Arc::new(AtomicU32::new(0)),
        get_peer_identity_type_calls: Arc::new(AtomicU32::new(0)),
    }));

    let result = route
        .session_mgr
        .do_sync_route_info(2, 1, false, None, None, None, None)
        .await;

    assert!(matches!(result, Err(Error::Stopped)));
    assert!(route.service_impl.sessions.is_empty());
    assert_eq!(route.task_count(), 0);
}

#[tokio::test]
async fn stale_non_initiator_sync_preserves_newer_session_generation() {
    let (route, _peer_rpc) = test_route_with_admin_peer(Arc::new(NoopPeerContext::default())).await;
    let session = route.service_impl.get_or_create_session(2);

    route
        .session_mgr
        .do_sync_route_info(2, 22, true, None, None, None, None)
        .await
        .expect("new initiator generation should be accepted");
    let contact = session.last_contact_instant.load();
    let rx_count = session.rpc_rx_count.load(Ordering::Relaxed);

    let stale_peer_info = RoutePeerInfo {
        peer_id: 3,
        version: 1,
        ..Default::default()
    };
    let result = route
        .session_mgr
        .do_sync_route_info(
            2,
            11,
            false,
            Some(vec![stale_peer_info.clone()]),
            Some(vec![raw_route_peer_info(&stale_peer_info)]),
            None,
            None,
        )
        .await;

    assert!(matches!(result, Err(Error::Stopped)));
    assert_eq!(session.dst_session_id.load(Ordering::Relaxed), 22);
    assert!(session.dst_is_initiator.load(Ordering::Relaxed));
    assert_eq!(session.last_contact_instant.load(), contact);
    assert_eq!(session.rpc_rx_count.load(Ordering::Relaxed), rx_count);
    assert!(
        !route
            .service_impl
            .synced_route_info
            .peer_infos
            .read()
            .contains_key(&3)
    );

    route.stop().await;
}

#[tokio::test]
async fn stale_peer_info_does_not_restore_removed_acl_group() {
    let context = Arc::new(NoopPeerContext::new(CoreNetworkIdentity::new_credential(
        "default".to_owned(),
    )));
    let (route, _peer_rpc) = test_route_with_admin_peer(context).await;
    route.service_impl.get_or_create_session(2);

    let peer_info = |version, groups| RoutePeerInfo {
        peer_id: 3,
        peer_route_id: 30,
        version,
        groups,
        ..Default::default()
    };
    let sync_peer_info = |info: RoutePeerInfo| {
        let raw = raw_route_peer_info(&info);
        (Some(vec![info]), Some(vec![raw]))
    };

    let v1 = peer_info(
        1,
        vec![PeerGroupInfo {
            group_name: "legacy".to_owned(),
            group_proof: Vec::new(),
        }],
    );
    let (peer_infos, raw_peer_infos) = sync_peer_info(v1.clone());
    route
        .session_mgr
        .do_sync_route_info(2, 22, true, peer_infos, raw_peer_infos, None, None)
        .await
        .expect("v1 peer info should be accepted");
    assert_eq!(route.service_impl.get_peer_groups(3).as_ref(), &["legacy"]);

    let v2 = peer_info(2, Vec::new());
    let (peer_infos, raw_peer_infos) = sync_peer_info(v2);
    route
        .session_mgr
        .do_sync_route_info(2, 22, true, peer_infos, raw_peer_infos, None, None)
        .await
        .expect("v2 peer info should be accepted");
    assert!(route.service_impl.get_peer_groups(3).is_empty());

    let (peer_infos, raw_peer_infos) = sync_peer_info(v1);
    route
        .session_mgr
        .do_sync_route_info(2, 22, true, peer_infos, raw_peer_infos, None, None)
        .await
        .expect("stale peer info should be ignored");

    assert_eq!(
        route
            .service_impl
            .synced_route_info
            .peer_infos
            .read()
            .get(&3)
            .expect("peer info should remain present")
            .version,
        2
    );
    assert!(route.service_impl.get_peer_groups(3).is_empty());

    route.stop().await;
}

#[test]
fn credential_group_refresh_does_not_restore_removed_proof_group() {
    let service_impl = Arc::new(test_service_impl(1));
    let mut peer_infos = OrderedHashMap::new();
    peer_infos.insert(
        3,
        RoutePeerInfo {
            peer_id: 3,
            version: 3,
            noise_static_pubkey: vec![3; 32],
            ..Default::default()
        },
    );
    let all_trusted = HashMap::from([(
        vec![3; 32],
        TrustedCredentialPubkey {
            groups: vec!["credential".to_owned()],
            ..Default::default()
        },
    )]);

    let group_trust_lock = service_impl
        .synced_route_info
        .group_trust_update_lock
        .lock();
    service_impl
        .synced_route_info
        .set_peer_groups_locked(3, HashMap::from([("legacy".to_owned(), vec![1])]));

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let credential_refresh = std::thread::spawn({
        let service_impl = service_impl.clone();
        move || {
            started_tx.send(()).unwrap();
            service_impl
                .synced_route_info
                .update_credential_groups(&peer_infos, &all_trusted);
        }
    });
    started_rx.recv().unwrap();

    service_impl
        .synced_route_info
        .set_peer_groups_locked(3, HashMap::new());
    drop(group_trust_lock);
    credential_refresh.join().unwrap();

    assert_eq!(
        service_impl
            .synced_route_info
            .group_trust_map
            .get(&3)
            .as_deref(),
        Some(&HashMap::from([("credential".to_owned(), Vec::new())]))
    );
}

#[tokio::test]
async fn stop_waits_for_in_flight_route_sync_before_draining_sessions() {
    let peer_rpc = Arc::new(PeerRpcManager::new(TestPeerRpcTransport));
    let route = PeerRoute::new(
        1,
        Arc::new(NoopPeerContext::default()),
        Arc::new(TestPublicIpv6Runtime),
        peer_rpc,
    );
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    *route.service_impl.interface.lock().await = Some(Box::new(BlockingInterface {
        entered: entered.clone(),
        release: release.clone(),
    }));

    let sync_task = tokio::spawn({
        let session_mgr = route.session_mgr.clone();
        async move {
            session_mgr
                .do_sync_route_info(2, 1, true, None, None, None, None)
                .await
        }
    });
    crate::foundation::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .expect("route sync did not enter the interface call");

    let stop_task = tokio::spawn({
        let route = route.clone();
        async move { route.stop().await }
    });
    crate::foundation::time::timeout(Duration::from_secs(1), async {
        while !route.service_impl.stopped.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("route did not enter the stopped state");

    assert!(!stop_task.is_finished());
    assert!(matches!(
        route.session_mgr.get_or_start_session(3),
        Err(Error::Stopped)
    ));

    release.notify_one();
    sync_task
        .await
        .expect("route sync task panicked")
        .expect("in-flight route sync failed");
    crate::foundation::time::timeout(Duration::from_secs(1), stop_task)
        .await
        .expect("route stop did not finish")
        .expect("route stop task panicked");

    assert!(route.service_impl.sessions.is_empty());
    assert_eq!(route.task_count(), 0);
}

#[test]
fn builds_next_hop_and_proxy_lookup_from_snapshot() {
    let mut remote_proxy_peer = peer(3);
    remote_proxy_peer
        .info
        .proxy_cidrs
        .push("10.10.0.0/16".into());

    let snapshot = OspfRouteSnapshot {
        peer_infos: vec![peer(1), peer(2), remote_proxy_peer],
        conn_map: vec![connected(1, [2]), connected(2, [1, 3]), connected(3, [2])],
        suppressed_peer_ids: BTreeSet::new(),
        version: 1,
    };

    let table = OspfRouteTable::new();
    table.build_from_snapshot(
        1,
        &snapshot,
        NextHopPolicy::LeastHop,
        &DefaultRouteCostCalculator,
    );

    let next_hop = table.get_next_hop(3).unwrap();
    assert_eq!(next_hop.next_hop_peer_id, 2);
    assert_eq!(next_hop.path_len, 2);
    assert_eq!(
        table.get_peer_id_for_proxy(&"10.10.1.1".parse::<IpAddr>().unwrap()),
        Some(3)
    );
}

/// Phase B/D: with `DefaultRouteCostCalculator` every edge costs 1, so a
/// diamond has two equally cheap first hops. Both must be offered, primary
/// first, so the send path can fall through when one of them stops working.
#[test]
fn equal_cost_diamond_offers_every_first_hop() {
    let snapshot = OspfRouteSnapshot {
        peer_infos: vec![peer(1), peer(2), peer(3), peer(4)],
        conn_map: vec![
            connected(1, [2, 3]),
            connected(2, [1, 4]),
            connected(3, [1, 4]),
            connected(4, [2, 3]),
        ],
        suppressed_peer_ids: BTreeSet::new(),
        version: 1,
    };

    let table = OspfRouteTable::new();
    table.build_from_snapshot(
        1,
        &snapshot,
        NextHopPolicy::LeastCost,
        &DefaultRouteCostCalculator,
    );

    let hops = table.get_next_hops(4);
    assert_eq!(hops.len(), 2, "both first hops expected, got {hops:?}");
    assert!(hops.contains(&2) && hops.contains(&3), "got {hops:?}");
    assert_eq!(
        table.get_next_hop(4).unwrap().next_hop_peer_id,
        hops[0],
        "the primary must come first"
    );
}

/// A single-path topology must not invent an alternate.
#[test]
fn single_path_topology_has_no_alternates() {
    let snapshot = OspfRouteSnapshot {
        peer_infos: vec![peer(1), peer(2), peer(3)],
        conn_map: vec![connected(1, [2]), connected(2, [1, 3]), connected(3, [2])],
        suppressed_peer_ids: BTreeSet::new(),
        version: 1,
    };

    let table = OspfRouteTable::new();
    table.build_from_snapshot(
        1,
        &snapshot,
        NextHopPolicy::LeastHop,
        &DefaultRouteCostCalculator,
    );

    assert_eq!(
        table.get_next_hops(3),
        super::NextHopCandidates::from_slice(&[2])
    );
}

/// Non-uniform edge costs, so a test can make the direct edge the cheapest one
/// and an alternative much more expensive. Unlisted edges cost `UNLISTED`, so a
/// path that "somehow" avoids the edges under test cannot win by accident.
struct FixedEdgeCosts {
    costs: HashMap<(PeerId, PeerId), i32>,
}

const UNLISTED: i32 = 10_000;

impl FixedEdgeCosts {
    fn new(costs: &[(PeerId, PeerId, i32)]) -> Self {
        Self {
            costs: costs
                .iter()
                .map(|&(src, dst, cost)| ((src, dst), cost))
                .collect(),
        }
    }
}

impl RouteCostCalculatorInterface for FixedEdgeCosts {
    fn calculate_cost(&self, src: PeerId, dst: PeerId) -> i32 {
        self.costs.get(&(src, dst)).copied().unwrap_or(UNLISTED)
    }
}

/// A *torn-down* direct conn already hands the destination to a costlier relay.
///
/// This pins down what "the backup only accepts exactly-equal-cost paths" does
/// and does not cover. `RouteInterface::list_peers` is
/// `PeerMap::list_peers_with_conn`, so the moment the last conn to the
/// destination dies it leaves our advertised `connected_peers`; that changes the
/// snapshot, the OSPF loop rebuilds (also woken by the conn-removed peer event),
/// and the edge is gone from the graph — so Dijkstra is free to pick *any*
/// remaining path no matter how much worse it is. That is the user's rule
/// ("otherwise take the best quality") and it needs no unequal-cost backup.
///
/// What the equal-cost alternates do **not** cover is the window between the
/// conn dying and the rebuild, and a conn that is black-holing but still open
/// (see `open_but_degraded_direct_edge_is_not_detectable_locally`).
#[test]
fn torn_down_direct_edge_hands_over_to_a_costlier_relay() {
    // 1 (me) --2--> 3 is cheap; 1 --1--> 2 --50--> 3 is 25x worse.
    let costs = FixedEdgeCosts::new(&[
        (1, 2, 1),
        (1, 3, 2),
        (2, 1, 1),
        (2, 3, 50),
        (3, 1, 2),
        (3, 2, 50),
    ]);

    let with_direct = OspfRouteSnapshot {
        peer_infos: vec![peer(1), peer(2), peer(3)],
        conn_map: vec![
            connected(1, [2, 3]),
            connected(2, [1, 3]),
            connected(3, [1, 2]),
        ],
        suppressed_peer_ids: BTreeSet::new(),
        version: 1,
    };

    let table = OspfRouteTable::new();
    table.build_from_snapshot(1, &with_direct, NextHopPolicy::LeastCost, &costs);

    assert_eq!(table.get_next_hop(3).unwrap().next_hop_peer_id, 3);
    // No equal-cost alternate exists, so Phase B/D has nothing to offer here.
    assert_eq!(
        table.get_next_hops(3),
        super::NextHopCandidates::from_slice(&[3]),
        "the direct path is the only candidate, so there is no equal-cost backup"
    );

    // The last conn to 3 died: `list_peers_with_conn` drops it, so the advertised
    // set changes and the edge disappears from the graph on the next rebuild.
    let after_teardown = OspfRouteSnapshot {
        peer_infos: vec![peer(1), peer(2), peer(3)],
        conn_map: vec![
            connected(1, [2]),
            connected(2, [1, 3]),
            connected(3, [1, 2]),
        ],
        suppressed_peer_ids: BTreeSet::new(),
        version: 2,
    };
    table.build_from_snapshot(1, &after_teardown, NextHopPolicy::LeastCost, &costs);

    let next_hop = table.get_next_hop(3).unwrap();
    assert_eq!(
        next_hop.next_hop_peer_id, 2,
        "the only remaining path, 25x more expensive, must take over"
    );
    assert_eq!(next_hop.path_len, 2);
}

/// The counterpart, and the real remaining gap: while the conn is *open* the
/// advertised set does not change, so nothing local can notice it stopped
/// carrying traffic. `calculate_cost` keeps reporting the pre-failure number,
/// the snapshot is unchanged, and no rebuild is triggered — the next hop stays
/// the dead direct edge until the peer-center hands back an inflated cost.
///
/// This is why the fast liveness probe must feed the *local* view too, and not
/// only the reported one.
#[test]
fn open_but_degraded_direct_edge_is_not_detectable_locally() {
    let costs = FixedEdgeCosts::new(&[
        (1, 2, 1),
        (1, 3, 2),
        (2, 1, 1),
        (2, 3, 50),
        (3, 1, 2),
        (3, 2, 50),
    ]);

    let snapshot = OspfRouteSnapshot {
        peer_infos: vec![peer(1), peer(2), peer(3)],
        conn_map: vec![
            connected(1, [2, 3]),
            connected(2, [1, 3]),
            connected(3, [1, 2]),
        ],
        suppressed_peer_ids: BTreeSet::new(),
        version: 7,
    };

    let table = OspfRouteTable::new();
    table.build_from_snapshot(1, &snapshot, NextHopPolicy::LeastCost, &costs);

    // Rebuilding with an identical snapshot — which is all a conn that is alive
    // but black-holing can produce — keeps the dead direct edge as the choice,
    // because the only input that moved (the cost) comes from the peer-center.
    table.build_from_snapshot(1, &snapshot, NextHopPolicy::LeastCost, &costs);
    assert_eq!(table.get_next_hop(3).unwrap().next_hop_peer_id, 3);
    assert_eq!(
        table.get_next_hops(3),
        super::NextHopCandidates::from_slice(&[3])
    );
    assert_eq!(
        costs.calculate_cost(1, 3),
        2,
        "the local view of the edge cost is whatever the peer-center last told us"
    );
}

/// Phase F2: once our own conn to the destination is *sustainedly* degraded, the
/// local SPF must stop pricing that edge at face value — and must therefore hand
/// the destination to a relay that is far more expensive, which is exactly the
/// "otherwise take the best quality that is left" rule.
///
/// This is the case `open_but_degraded_direct_edge_is_not_detectable_locally`
/// shows is otherwise invisible: without the override the direct edge keeps its
/// stale cost of 2 and wins forever.
#[test]
fn sustainedly_degraded_own_edge_is_outpriced_by_a_costlier_relay() {
    let costs = FixedEdgeCosts::new(&[
        (1, 2, 1),
        (1, 3, 2),
        (2, 1, 1),
        (2, 3, 50),
        (3, 1, 2),
        (3, 2, 50),
    ]);
    let snapshot = OspfRouteSnapshot {
        peer_infos: vec![peer(1), peer(2), peer(3)],
        conn_map: vec![
            connected(1, [2, 3]),
            connected(2, [1, 3]),
            connected(3, [1, 2]),
        ],
        suppressed_peer_ids: BTreeSet::new(),
        version: 1,
    };

    let table = OspfRouteTable::new();
    table.build_from_snapshot_with_unusable(
        1,
        &snapshot,
        NextHopPolicy::LeastCost,
        &costs,
        &BTreeSet::from([3]),
    );

    let next_hop = table.get_next_hop(3).unwrap();
    assert_eq!(
        next_hop.next_hop_peer_id, 2,
        "the direct edge is connected-but-dead, so the 25x worse relay must take over"
    );
    assert_eq!(next_hop.path_len, 2);
}

/// The override prices the edge out; it must **not** delete it.
///
/// If it did, a probe false positive on the only path to a peer would retract
/// that peer's prefixes and turn a degraded link into an outage. `peer_reachable`
/// and the CIDR lookup therefore have to keep resolving.
#[test]
fn outpricing_a_degraded_own_edge_keeps_the_peer_reachable() {
    let mut remote_proxy = peer(3);
    remote_proxy.info.proxy_cidrs.push("10.10.0.0/16".into());

    let costs = FixedEdgeCosts::new(&[(1, 3, 2), (3, 1, 2)]);
    let snapshot = OspfRouteSnapshot {
        peer_infos: vec![peer(1), peer(3), remote_proxy],
        conn_map: vec![connected(1, [3]), connected(3, [1])],
        suppressed_peer_ids: BTreeSet::new(),
        version: 1,
    };

    let table = OspfRouteTable::new();
    // Single path, and it is the degraded one: still the only way through.
    table.build_from_snapshot_with_unusable(
        1,
        &snapshot,
        NextHopPolicy::LeastCost,
        &costs,
        &BTreeSet::from([3]),
    );

    assert!(
        table.peer_reachable(3),
        "a degraded-but-only path must not be retracted"
    );
    assert_eq!(
        table.get_peer_id_for_proxy(&"10.10.1.1".parse::<IpAddr>().unwrap()),
        Some(3)
    );
}

/// Only **our own** outgoing edges are re-priced. A neighbour we have no reason to
/// distrust stays at the cost the peer-center gave it — the local override must
/// not become a way to bias paths we know nothing about.
#[test]
fn the_override_only_touches_edges_leaving_us() {
    let costs = FixedEdgeCosts::new(&[
        (1, 2, 1),
        (1, 3, 2),
        (2, 1, 1),
        (2, 3, 50),
        (3, 1, 2),
        (3, 2, 50),
    ]);
    let snapshot = OspfRouteSnapshot {
        peer_infos: vec![peer(1), peer(2), peer(3)],
        conn_map: vec![
            connected(1, [2, 3]),
            connected(2, [1, 3]),
            connected(3, [1, 2]),
        ],
        suppressed_peer_ids: BTreeSet::new(),
        version: 1,
    };

    let table = OspfRouteTable::new();
    // 2 is flagged even though we are not adjacent to it via a usable conn —
    // only edge 2->3 could have been affected, and the cost 50 must be untouched,
    // so the result is identical to a build with no flags at all.
    table.build_from_snapshot_with_unusable(
        1,
        &snapshot,
        NextHopPolicy::LeastCost,
        &costs,
        &BTreeSet::from([2]),
    );
    assert_eq!(table.get_next_hop(3).unwrap().next_hop_peer_id, 3);
}

#[test]
fn default_proxy_cidr_is_excluded_from_specific_lookup() {
    let mut remote_proxy_peer = peer(3);
    remote_proxy_peer.info.proxy_cidrs.push("0.0.0.0/0".into());
    remote_proxy_peer
        .info
        .proxy_cidrs
        .push("10.10.0.0/16".into());

    let snapshot = OspfRouteSnapshot {
        peer_infos: vec![peer(1), peer(2), remote_proxy_peer],
        conn_map: vec![connected(1, [2]), connected(2, [1, 3]), connected(3, [2])],
        suppressed_peer_ids: BTreeSet::new(),
        version: 1,
    };

    let table = OspfRouteTable::new();
    table.build_from_snapshot(
        1,
        &snapshot,
        NextHopPolicy::LeastHop,
        &DefaultRouteCostCalculator,
    );

    assert_eq!(
        table.get_peer_id_for_proxy(&"10.10.1.1".parse::<IpAddr>().unwrap()),
        Some(3)
    );
    assert_eq!(
        table.get_peer_id_for_proxy(&"8.8.8.8".parse::<IpAddr>().unwrap()),
        None
    );
    assert_eq!(
        table.get_peer_id_for_default_route_proxy(&"8.8.8.8".parse::<IpAddr>().unwrap()),
        Some(3)
    );
    assert_eq!(
        table.get_peer_id_for_default_route_proxy(&"10.10.1.1".parse::<IpAddr>().unwrap()),
        None
    );
}

#[test]
fn default_proxy_remains_available_for_service_paths() {
    let mut remote_proxy_peer = peer(3);
    remote_proxy_peer.info.proxy_cidrs.push("0.0.0.0/0".into());

    let snapshot = OspfRouteSnapshot {
        peer_infos: vec![peer(1), peer(2), remote_proxy_peer],
        conn_map: vec![connected(1, [2]), connected(2, [1, 3]), connected(3, [2])],
        suppressed_peer_ids: BTreeSet::new(),
        version: 1,
    };

    let table = OspfRouteTable::new();
    table.build_from_snapshot(
        1,
        &snapshot,
        NextHopPolicy::LeastHop,
        &DefaultRouteCostCalculator,
    );

    // L3-specific proxy lookup still ignores /0 so exit_nodes stay ahead.
    assert_eq!(
        table.get_peer_id_for_proxy(&"8.8.8.8".parse::<IpAddr>().unwrap()),
        None
    );
    // ACL / KCP / wrapped-TCP use get_peer_id_by_ip_allowing_default_proxy,
    // which falls through to this dedicated /0 match.
    assert_eq!(
        table.get_peer_id_for_default_route_proxy(&"8.8.8.8".parse::<IpAddr>().unwrap()),
        Some(3)
    );
}

// ---------------------------------------------------------------------------
// CR-07: OSPF sender identity must come from the transport, not the body.
// ---------------------------------------------------------------------------

#[test]
fn verified_from_peer_id_accepts_matching_body() {
    assert_eq!(resolve_verified_from_peer_id(Some(7), 7).unwrap(), 7);
}

#[test]
fn verified_from_peer_id_rejects_impersonation() {
    // The attack this closes: an authenticated peer writes somebody else's id
    // into `SyncRouteInfoRequest::my_peer_id` and submits route updates under
    // that identity.
    assert!(resolve_verified_from_peer_id(Some(7), 9).is_err());
}

#[test]
fn verified_from_peer_id_rejects_missing_transport_identity() {
    // "Unauthenticated" must never degrade into "believe the caller". The
    // operation-RPC path has no tunnel, so it has no verified identity either.
    assert!(resolve_verified_from_peer_id(None, 9).is_err());
}

// ---------------------------------------------------------------------------
// CR-09: a peer-reported cost must never become a zero/huge Dijkstra weight.
// ---------------------------------------------------------------------------

#[test]
fn negative_edge_cost_does_not_wrap_into_a_free_path() {
    // The regression: `-1 as usize == 2^64 - 1`, and `1 + (2^64 - 1) == 0`.
    let w = edge_cost_from_i32(-1);
    assert_eq!(w, MIN_EDGE_COST);
    // Simulate one relaxation step: the result must not beat the start node.
    assert!(MIN_EDGE_COST + w > 0);
}

#[test]
fn negative_edge_cost_floors_regardless_of_magnitude() {
    for negative in [-1, -2, -1_000, i32::MIN] {
        assert_eq!(
            edge_cost_from_i32(negative),
            MIN_EDGE_COST,
            "cost {negative} must floor to MIN_EDGE_COST"
        );
    }
}

#[test]
fn zero_and_positive_edge_costs_stay_nonzero() {
    // 0 would degenerate `first_hop` tie-breaking even when nothing is hostile.
    assert_eq!(edge_cost_from_i32(0), MIN_EDGE_COST);
    assert_eq!(edge_cost_from_i32(1), 1);
    assert_eq!(edge_cost_from_i32(500), 500);
}

// ---------------------------------------------------------------------------
// CR-08: a peer-chosen `version` must not be able to freeze a third party.
// ---------------------------------------------------------------------------

#[test]
fn version_jump_limit_accepts_ordinary_increments() {
    // Real versions advance by exactly one per routing-state change.
    for stored in [0, 1, 7, 1_000, 500_000_000] {
        assert!(
            version_advance_is_plausible(stored, stored + 1),
            "stored={stored} should accept stored+1"
        );
    }
}

#[test]
fn version_jump_limit_rejects_the_u32_max_freeze() {
    // The attack: one member claims `u32::MAX` for a third party so that the
    // real peer's own `version + 1` can never win again.
    assert!(!version_advance_is_plausible(0, u32::MAX));
    assert!(!version_advance_is_plausible(1, u32::MAX));
    assert!(!version_advance_is_plausible(12_345, u32::MAX));
}

#[test]
fn version_jump_limit_rejects_stale_and_absurd_advances() {
    // Out-of-order updates were already ignored; keep that behaviour.
    assert!(!version_advance_is_plausible(10, 10));
    assert!(!version_advance_is_plausible(10, 9));
    // Exactly at the limit is still allowed, one past it is not.
    assert!(version_advance_is_plausible(0, MAX_VERSION_JUMP));
    assert!(!version_advance_is_plausible(0, MAX_VERSION_JUMP + 1));
    // A long-lived peer's absolute version is never capped, only the jump.
    assert!(version_advance_is_plausible(
        u32::MAX - MAX_VERSION_JUMP,
        u32::MAX
    ));
}
