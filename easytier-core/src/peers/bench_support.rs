//! criterion 基准（`easytier/benches/routing_lookup.rs`）专用构造辅助。
//!
//! 仅在 `test-utils` feature 下编译，不进入默认构建。
//!
//! 选路热路径的 `PeerMap` / `Route` 都是 crate 内部类型（`pub(crate)`），
//! 基准作为独立 crate 拿不到它们，所以这里暴露**最小**的可 `pub` 入口：
//! 造一个空 `PeerMap` + 一个可控的 mock `Route`。
//! 只做构造，不改动任何生产逻辑。

use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use cidr::{Ipv4Cidr, Ipv6Cidr};
use quanta::Instant;

use super::route::{ArcRoute, Route, RouteInterfaceBox};
use crate::config::PeerId;
use crate::proto::core_peer::peer::Route as CoreRoute;
use crate::proto::peer_rpc::RoutePeerInfo;

pub use super::conn::peer_map::PeerMap;
pub use super::route::local_route_table::{
    AllUnavailable, FailoverWhen, LocalRouteTable, RouteLookup, RouteRule, RuleState, RuleUpdate,
    SelectBy, build_snapshot,
};

/// The IP that `local_route_table_with_rules` rule 0 (`10.0.0.0/24`) owns.
pub const POLICY_ROUTE_HIT_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);

/// Empty unified routing table — the zero-regression baseline: one `ArcSwap`
/// load, then `Miss`.
pub fn empty_local_route_table() -> LocalRouteTable {
    LocalRouteTable::empty()
}

/// A unified routing table with `count` explicit `/24` rules, each pinned to its
/// own peer and already reconciled to `Stable`, so `lookup` returns `Hit` for an
/// in-range IP and `Miss` outside. Mirrors the §12.5 scale points (1K / 10K).
pub fn local_route_table_with_rules(count: usize) -> LocalRouteTable {
    let table = LocalRouteTable::empty();
    let rules: Vec<RouteRule> = (0..count)
        .map(|i| {
            let cidr = Ipv4Cidr::new(Ipv4Addr::new(10, (i >> 8) as u8, (i & 0xff) as u8, 0), 24)
                .expect("/24 is valid");
            RouteRule {
                id: i as u32,
                cidr: Some(cidr),
                domains: Vec::new(),
                peers: vec![IpAddr::V4(Ipv4Addr::new(
                    172,
                    16,
                    (i >> 8) as u8,
                    (i & 0xff) as u8,
                ))],
                merge_exit_nodes: false,
                select_by: SelectBy::Priority,
                failover_when: FailoverWhen::Unreachable,
                if_all_unavailable: AllUnavailable::Open,
                require_confirmed: false,
                quality_degraded_cost: None,
            }
        })
        .collect();
    table.store(build_snapshot(rules));
    // Reconcile pass: without this every rule stays `Degraded` and `lookup`
    // would take the Degraded branch instead of the hot `Hit` path.
    let updates: Vec<RuleUpdate> = (0..count)
        .map(|i| RuleUpdate {
            idx: i,
            active_peer: Some(i as u32 + 100),
            state: RuleState::Stable,
            challenger: None,
            resolved_peers: Vec::new(),
            pinned_peer: None,
            costed_peers: Vec::new(),
        })
        .collect();
    table.apply_peer_updates(&HashMap::new(), &HashMap::new(), &updates);
    table
}

/// 造一个空 `PeerMap`（无 route）。
///
/// 基准随后用 `PeerMap::add_route` 注入 mock route。
pub fn new_peer_map(my_peer_id: PeerId) -> Arc<PeerMap> {
    let (tx, _rx) = super::create_packet_recv_chan();
    Arc::new(PeerMap::new(
        tx,
        Arc::new(super::test_support::NoopPeerContext::default()),
        my_peer_id,
    ))
}

/// 基准专用 route：可控的「VIP 精确命中」与「peer `/0` 默认代理命中」。
///
/// 用 `HashMap` 模拟真实实现的 DashMap O(1) 精确查找，
/// 使基准反映的是**每包固定成本**（`routes` 锁 + async trait 分发），而非线性扫描。
#[derive(Default)]
pub struct BenchRoute {
    vips: HashMap<Ipv4Addr, PeerId>,
    default_proxy: Option<PeerId>,
}

impl BenchRoute {
    pub fn new() -> Self {
        Self::default()
    }

    /// 让 `ip` 精确命中 `peer_id`（VIP 路径）。可重复调用以填充多个条目。
    pub fn with_vip(mut self, ip: Ipv4Addr, peer_id: PeerId) -> Self {
        self.vips.insert(ip, peer_id);
        self
    }

    /// 让 `get_peer_id_for_default_route_proxy` 返回 `peer_id`（proxy `/0` 路径）。
    pub fn with_default_proxy(mut self, peer_id: PeerId) -> Self {
        self.default_proxy = Some(peer_id);
        self
    }

    /// 转成 `ArcRoute` 供 `PeerMap::add_route` 使用。
    pub fn into_arc(self) -> ArcRoute {
        Arc::new(self)
    }
}

#[async_trait::async_trait]
impl Route for BenchRoute {
    async fn open(&self, _interface: RouteInterfaceBox) -> Result<u8, ()> {
        Err(())
    }

    async fn close(&self) {}

    async fn get_next_hop(&self, _peer_id: PeerId) -> Option<PeerId> {
        None
    }

    async fn list_routes(&self) -> Vec<CoreRoute> {
        Vec::new()
    }

    async fn list_proxy_cidrs(&self) -> BTreeSet<Ipv4Cidr> {
        BTreeSet::new()
    }

    async fn list_proxy_cidrs_v6(&self) -> BTreeSet<Ipv6Cidr> {
        BTreeSet::new()
    }

    async fn get_peer_id_by_ipv4(&self, ipv4: &Ipv4Addr) -> Option<PeerId> {
        self.vips.get(ipv4).copied()
    }

    async fn get_peer_id_for_default_route_proxy(&self, _ip: &IpAddr) -> Option<PeerId> {
        self.default_proxy
    }

    async fn get_peer_info(&self, _peer_id: PeerId) -> Option<RoutePeerInfo> {
        None
    }

    async fn get_peer_info_last_update_time(&self) -> Instant {
        Instant::now()
    }

    fn get_peer_groups(&self, _peer_id: PeerId) -> Arc<Vec<String>> {
        Arc::new(Vec::new())
    }
}
