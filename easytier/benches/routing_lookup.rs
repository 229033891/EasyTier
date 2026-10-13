//! 出站选路热路径基准（P-T1 / TD-6）。
//!
//! 覆盖四条路径：
//!   - `vip_hit`             VIP 精确命中（`PeerMap::get_peer_id_by_ipv4`）
//!   - `vip_miss`            未命中（同上，返回 None）
//!   - `proxy_default_hit`   peer `/0` 默认代理命中（`get_peer_id_by_ipv4_allowing_default_proxy`）
//!   - `exit_node`           出口节点解析（`PeerMap::resolve_exit_node_peer`，即 TD-4 锁 + TD-5 重复遍历）
//!
//! 说明：`exit_node` 在 mock 环境下测的是**未命中时**的出口解析开销 ——
//! 要真正返回 Some 需真实 peer + 网关连通性（`get_gateway_peer_id`），PeerMap 层构造不出。
//!
//! 三档规模（VIP 条目数）由 `SCALES` 给出，对应 `routing-tech-debt.md` §5 P-T1 T1-2。
//! 用 mock route（`HashMap` 精确查找）模拟真实 DashMap，测的是**每包固定成本**
//! （`routes` 锁 + async trait 分发），而非线性扫描。
//!
//! 运行：`cargo bench -p easytier --bench routing_lookup`

use std::hint::black_box;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};

use easytier_core::peers::bench_support::{BenchRoute, new_peer_map};

/// VIP 条目数档位（T1-2：10 / 100 / 1000）。
const SCALES: &[usize] = &[10, 100, 1000];

const HIT_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);
const MISS_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);
const EXTERNAL_IP: Ipv4Addr = Ipv4Addr::new(8, 8, 8, 8);

fn bench_routing_lookup(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

    for &scale in SCALES {
        let vip_map = rt.block_on(async {
            let map = new_peer_map(1);
            let mut route = BenchRoute::new();
            for i in 0..scale {
                let ip = Ipv4Addr::new(10, 1, (i >> 8) as u8, (i & 0xff) as u8);
                route = route.with_vip(ip, i as u32 + 100);
            }
            route = route.with_vip(HIT_IP, 7);
            map.add_route(route.into_arc()).await;
            map
        });

        let proxy_map = rt.block_on(async {
            let map = new_peer_map(1);
            map.add_route(BenchRoute::new().with_default_proxy(9).into_arc())
                .await;
            map
        });

        let mut g = c.benchmark_group(format!("routing_lookup/{scale}"));

        g.bench_function("vip_hit", |b| {
            b.to_async(&rt)
                .iter(|| vip_map.get_peer_id_by_ipv4(black_box(&HIT_IP)))
        });

        g.bench_function("vip_miss", |b| {
            b.to_async(&rt)
                .iter(|| vip_map.get_peer_id_by_ipv4(black_box(&MISS_IP)))
        });

        g.bench_function("proxy_default_hit", |b| {
            b.to_async(&rt).iter(|| {
                proxy_map.get_peer_id_by_ipv4_allowing_default_proxy(black_box(&EXTERNAL_IP))
            })
        });

        let exit_nodes = [IpAddr::V4(EXTERNAL_IP)];
        g.bench_function("exit_node", |b| {
            b.to_async(&rt)
                .iter(|| vip_map.resolve_exit_node_peer(black_box(&exit_nodes)))
        });

        g.finish();
    }
}

fn criterion_config() -> Criterion {
    Criterion::default()
        .measurement_time(Duration::from_secs(5))
        .warm_up_time(Duration::from_secs(2))
}

criterion_group! {
    name = benches;
    config = criterion_config();
    targets = bench_routing_lookup
}
criterion_main!(benches);
