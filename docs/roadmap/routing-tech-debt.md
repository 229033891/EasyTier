# 路由热路径技术债（独立可推进）

## Status

- Status: **Roadmap**（Tech Debt；**P-T1 已完成**；剩余项 **评估后暂不实施**）
- 最近审阅：2026-10-13（基线后评估：TD-1/4/5/7 与 T1-6 搁置；P-T1 可正常上线）
- 范围：出站选路热路径上的既有性能债
- **独立性**：**不依赖** [`unified-routing-design.md`](./unified-routing-design.md)
- 索引：[`../README.md`](../README.md)

> **分阶段实施**：P-T1 已落地可上线；P-T2～T4 **默认不做**，仅在有 profiling 痛点或产品要改语义时重启（见 §10）。

---

## 1. 热路径现状（已逐行核实）

> 实际转发实现在 `PeerOutboundPacketRouter`（`peer_manager.rs`）；`PeerManager::send_msg_by_ip` / `get_msg_dst_peer_*` 仅为薄包装转发。

```
每个出站 TCP 包
  │
  ├─ gateway/proxy/tcp_proxy_service.rs:320-330
  │     while let Some(data) = output_rx.recv().await {      // 逐包
  │         peer_manager.send_msg_by_ip(packet, dst, false)
  │         └─ → PeerOutboundPacketRouter::send_msg_by_ip
  │
  └─ peers/peer_manager.rs:2910  PeerOutboundPacketRouter::send_msg_by_ip
        ├─ :2923 run_nic_packet_process_pipeline（ACL + pipelines）
        └─ :2944-2947 match ip → get_msg_dst_peer_ipv4 / ipv6   // ← 每包（V4 在 :2945）
              │
              ├─ :2842
              │    let mut dst_peers = SmallVec::<[PeerId; 1]>::new();  // ← TD-3 已改 SmallVec（单目的地零堆分配）
              │
              ├─ :2848
              │    self.peers.get_peer_id_by_ipv4(ipv4)       // → peer_map.rs:265-272
              │         for route in self.routes.read().await.iter() { ... }
              │         ↑ TD-1 tokio 异步锁（routes 列表通常只有 1 个元素）
              │            ↓
              │         route.get_peer_id_by_ipv4()           // peer_ospf_route.rs:4730
              │              ├─ ① ipv4_peer_id_map.get()     // DashMap，VIP 精确 O(1)
              │              └─ ② 未命中且非同网段
              │                   → get_peer_id_for_proxy()
              │                        └─ lookup_proxy_peer()      // :673
              │                             let map = cidr_peer_id_map.load();  // ✅ ArcSwap
              │                             map.get_lpm(...)                     // ✅ 真 LPM
              │
              └─ :2854
                   self.resolve_exit_node_peer().await          // → :2831
                        let exit_nodes = self.exit_nodes.read().await;   // ← TD-4 二次异步锁
                        peers/conn/peer_map.rs:285
                          for exit_node in exit_nodes {
                              get_peer_id_by_ipv4(vip).await    // ← TD-5 重复遍历 + 再取 routes 锁
                          }
```

**每包固定成本（单 route 实践）**：主要是 `routes` 的 tokio `read().await` + async trait 分发；VIP 命中后 OSPF 层已是 DashMap，**不是**每包 LPM。LPM 只发生在 proxy CIDR 回退路径。

---

## 2. ✅ 已核实：性能基础比预期好

### 2.1 VIP 精确命中已是 DashMap；CIDR 已是 ArcSwap + 真 LPM

```rust
// peer_ospf_route.rs:278-279 — VIP / 节点地址精确命中
pub fn get_peer_id_by_ipv4(&self, ipv4_addr: &Ipv4Addr) -> Option<PeerId> {
    self.ipv4_peer_id_map.get(ipv4_addr).map(|p| p.peer_id)  // DashMap
}

// peer_ospf_route.rs:673-692 — proxy CIDR
fn lookup_proxy_peer(&self, ip: &IpAddr, allow_default: bool) -> Option<PeerId> {
    match ip {
        IpAddr::V4(ipv4) => {
            let map = self.cidr_peer_id_map.load();                              // ← ArcSwap，无锁
            let (cidr, pv) = map.get_lpm(&Ipv4Cidr::new(*ipv4, 32).unwrap())?;  // ← 真最长前缀
            if !allow_default && cidr.network_length() == 0 { return None; }    // ← 排除 /0
            Some(pv.peer_id)
        }
        IpAddr::V6(ipv6) => { /* 同构 */ }
    }
}
```

**结论**：

- VIP 路径：DashMap O(1)，**无需**再为「精确命中线性扫」发债
- CIDR 路径：无锁读 + 真 LPM + `/0` 排除均已实现
- **这不是需要引入的新技术栈。** 热路径剩余成本在 `PeerMap.routes` 锁与出口路径重复解析（TD-1 / TD-4 / TD-5）

### 2.2 ✅ 跨 route 的 LIFO 顺序不是活跃问题

```rust
// peers/conn/peer_map.rs:402-405
pub async fn add_route(&self, route: ArcRoute) {
    let mut routes = self.routes.write().await;
    routes.insert(0, route);        // ← 后加入优先（LIFO）
}
```

全仓 `PeerMap::add_route` 仅 3 处：

| 位置 | 场景 |
|---|---|
| `peer_manager.rs:747` | 初始化挂载 OSPF route，**一次** |
| `peer_manager.rs:1894` | `run()` 时若存在 ospf_route 再 `add_route` |
| `foreign_network/mod.rs:538` | foreign network 自有 `PeerMap` |

**实践中每个 `PeerMap` 只持有 1 个 route**，LIFO 顺序在正常场景下不触发。

| 场景 | 是否影响 |
|---|---|
| 常规运行（单 route） | ❌ 不影响 |
| 理论多 route | ⚠️ 跨 route 是 LIFO，非全局 LPM |

**→ 不是活跃 bug，不阻断清理工作。** 记为 backlog（TD-7）。

---

## 3. 债务清单

| ID | 债务 | 严重度 | 处置（2026-10-13 评估） | 是否行为变更 |
|---|---|---|---|---|
| ~~TD-6~~ | ~~热路径无基准~~ | — | ✅ **已完成**（harness + before 基线） | ❌ |
| ~~TD-3~~ | ~~每包 `Vec` 分配~~ | — | ✅ **已完成**（内部 `SmallVec`） | ❌（公开仍 `Vec`） |
| ~~TD-2~~ | ~~CIDR 无索引线性扫描~~ | — | ❌ **已证伪**（§2.1） | ❌ |
| **TD-1** | `routes` 每包 tokio `RwLock`（9 处） | 原标中高 | ⏸ **暂不实施**（基线 ~165 ns 且规模不敏感；ROI 差） | ❌ |
| **TD-4** | 出口 `exit_nodes.read().await` | 原标中 | ⏸ **暂不实施**（出口 ~172 ns，与 VIP 同量级） | ❌ |
| **TD-5** | 出口 VIP→PeerId 每包重复解析 | 原标中 | ⏸ **暂不实施**（同上；缓存还易误跳过 gateway 检查） | ❌ |
| **TD-7** | 跨 route LIFO ≠ 全局 LPM | 低 | ⏸ **无限期 backlog**（实践单 route；属行为变更） | ✅ |
| T1-6 | SmallVec 分配 A/B | — | ❌ **不做**（harness 只测延迟；基线已在 SmallVec 之后） | ❌ |

### TD-1 细节

```rust
// peers/conn/peer_map.rs:10, 49-50
use tokio::sync::RwLock;
routes: RwLock<Vec<ArcRoute>>,
```

`self.routes.read().await` 共 **9 处**：`207 / 225 / 257 / 266 / 276 / 306 / 319 / 438 / 447`，其中 `:266`（`get_peer_id_by_ipv4`）在热路径。

> ⚠️ 范围澄清：该锁保护的是 **route 对象列表**，**不是** VIP DashMap / CIDR ArcSwap。改造面比「重写选路索引」小；收益主要是去掉每包 tokio 锁，而非加速 LPM。

### TD-3 细节

```rust
// peers/peer_manager.rs:2836-2842 — PeerOutboundPacketRouter
pub async fn get_msg_dst_peer_ipv4(&self, ipv4_addr: &Ipv4Addr) -> (SmallVec<[PeerId; 1]>, bool) {
    let mut dst_peers = SmallVec::<[PeerId; 1]>::new();   // ← T1-5 已改：单目的地零堆分配
```

**影响面（不是「3 个调用点」）**：

| 位置 | 角色 |
|---|---|
| `PeerOutboundPacketRouter::get_msg_dst_peer_ipv4` `:2836` | 实现（分配发生处） |
| 同结构 `get_msg_dst_peer` `:2790`、`send_msg_by_ip` `:2945`（V4 分支） | 内部调用 |
| `PeerManagerCore::{get_msg_dst_peer, get_msg_dst_peer_ipv4, get_msg_dst_peer_ipv6}` `:1752-1770` | 公开 API 薄包装（T1-5 已在边界 `into_vec()` 转换，公开签名保持 `Vec`） |
| `gateway/dataplane/mod.rs`（`get_msg_dst_peer`） | 下游调用 |
| `peer_manager_tests.rs` | 测试 |

T1-5 已完成：内部 `SmallVec`、公开仍 `Vec`；`get_msg_dst_peer_ipv6` 已对称改完。

---

## 4. 可用依赖

```toml
# easytier-core/Cargo.toml — 已有
arc-swap.workspace = true      # :25  ← 已在 peer_ospf_route.rs 中使用
dashmap.workspace = true       # :36  ← VIP 精确命中已用
parking_lot.workspace = true    # :48

# easytier/Cargo.toml — criterion 与 benches/；P-T1 起含 routing_lookup
criterion = "0.8.2"            # [[bench]] packet_bytes_extraction / routing_lookup（+ async_tokio）

# P-T1 / T1-5 已新增（2026-10-13）
smallvec = "1.16"              # 根 Cargo.toml [workspace.dependencies]；easytier-core/Cargo.toml 引用
```

---

## 5. 分阶段实施

> 每个阶段**独立可测试、可单独上线**。前一阶段不依赖后一阶段。

### P-T1：建立度量 + 低风险快赢（**1 周**）

| # | 内容 | 对应 |
|---|---|---|
| T1-1 | 为选路热路径建 `criterion` 基准 | TD-6 |
| T1-2 | 三档规模：10 / 100 / 1000 路由（或等价 CIDR/VIP 条目） | TD-6 |
| T1-3 | 覆盖四条路径：VIP 命中 / proxy CIDR 命中 / 出口节点 / 未命中 | TD-6 |
| T1-4 | 产出 **before 基线报告** | TD-6 |
| T1-5 | 热路径内部用 `SmallVec<[PeerId; 1]>`（需新增 `smallvec`）；**公开 `PeerManager` API 仍返回 `Vec`**（边界转换） | TD-3 |
| T1-6 | 基准对比，验证单目的地路径分配下降 | TD-3 |

**基准落地约束（重要）**：

- `PeerOutboundPacketRouter` 为 `pub(crate)`，挂在 `easytier-core`；现有 criterion 只在 **`easytier`** crate（`easytier/benches/`）
- 可选方案（择一）：
  1. 在 **`easytier-core`** 增加 `[[bench]]`，直接测 `pub(crate)` API；或
  2. 经公开 `PeerManager::get_msg_dst_peer_ipv4` 测（更重，需搭实例）；或
  3. 抽出可 `pub` 的纯查找辅助供 bench 调用
- 勿默认「丢一个 `easytier/benches/routing_lookup.rs` 就能直接调用 router」

> **实际落地（2026-10-13）**：**选项 1 不可行** —— bench target 是独立 crate，拿不到 `pub(crate)` 项。
> 采用「选项 3 的轻量版」：在 `easytier-core` 加 feature-gated 的 `pub mod peers::bench_support`
> （`feature = "test-utils"`），只暴露 `new_peer_map()` + mock `BenchRoute`（不改生产逻辑）；
> 基准文件仍放在 `easytier/benches/`（criterion 与 `[[bench]]` 都在该 crate）。
- T1-2 的「10 / 100 / 1000」指 **VIP/CIDR 条目数**，不是 `PeerMap` 里 1000 个 route 对象（实践通常只有 1 个）

**SmallVec 约束**：优先 **内部 `SmallVec`、公开 API 保持 `Vec`**，避免类型泄漏到 dataplane / 外部调用方。广播路径本身会多 peer，堆分配仍属预期。

**独立性**：只动内部分配、依赖与新增基准，**不改任何选路语义**（公开签名尽量不变）。

**上线判据**：基准可复现；**单目的地**路径分配计数 = 0；功能回归全绿。

**⭐ 最优先项**：即使后续不做任何清理，**基准 harness 本身长期有效**，且是评估其他性能工作的前提。

**实施进度（2026-10-13）**

- ✅ **T1-1 / T1-2 / T1-3**：新增 `easytier/benches/routing_lookup.rs`（criterion + `async_tokio`）。
  覆盖四条路径 `vip_hit` / `vip_miss` / `proxy_default_hit` / `exit_node`；规模档位 10 / 100 / 1000（VIP 条目数）。
- ✅ **T1-5**：`PeerOutboundPacketRouter::get_msg_dst_peer{,_ipv4,_ipv6}` 返回 `SmallVec<[PeerId; 1]>`；
  公开 `PeerManagerCore` API 仍返回 `Vec`（`into_vec()` 边界转换），未泄漏到 dataplane。
- 新增 `easytier-core/src/peers/bench_support.rs`（`feature = "test-utils"`）：
  暴露 `new_peer_map()` 与 mock `BenchRoute`，让基准能构造 `pub(crate)` 的 `PeerMap`。
- `easytier-core/src/peers/mod.rs`：`test_support` 的 gate 由 `#[cfg(test)]` 放宽为
  `#[cfg(any(test, feature = "test-utils"))]`（基准与单测共用构造辅助）。
- ✅ **T1-4**：before 基线已产出（见下表；`--save-baseline before`）。
- ❌ **T1-6**：不做。harness 只测延迟；基线已在 T1-5 之后，无 SmallVec 前后 A/B；
  单目的地零堆分配由 `SmallVec<[PeerId; 1]>` 类型不变量保证。

**基线（2026-10-13，`cargo bench -p easytier --bench routing_lookup -- --save-baseline before`）**

| 环境 | 值 |
|---|---|
| 主机 | `DEV` / Intel Xeon E5-2680 v4 @ 2.40GHz / ~24 GB RAM |
| rustc | 1.95.0（`rust-toolchain.toml`） |
| 耗时 | 约 24 min（含 bench profile 编译；exit 0） |
| 目标目录 | `.cache\cargo-target` + vcvars |

criterion 报告中位数（ns/iter；`[low mean high]` 取 mean）：

| 规模（VIP 条目） | vip_hit | vip_miss | proxy_default_hit | exit_node |
|---|---|---|---|---|
| 10 | 165.00 | 170.73 | 307.96 | 175.75 |
| 100 | 166.98 | 166.19 | 308.95 | 171.64 |
| 1000 | 168.16 | 163.33 | 302.94 | 172.18 |

**读数要点**：

- VIP / miss / exit_node 约 **163–176 ns**，规模 10→1000 **几乎不敏感**（符合「测固定成本、非线性扫」预期）
- `proxy_default_hit` 约 **303–309 ns**（~1.8× VIP），规模同样不敏感
- 本基线 = **T1-5 之后**（已含内部 SmallVec）；用作 P-T2 / P-T3 对比锚点
- **解读约束**：mock `BenchRoute`（HashMap），量的是每包 `routes` 锁 + async trait 固定成本；
  `exit_node` 在 mock 下基本走未命中路径（无真实 gateway）。勿当成真实 OSPF DashMap/LPM 吞吐
- 复跑：`cargo bench -p easytier --bench routing_lookup -- --save-baseline before`
  （对比：`--baseline before`）

---

### P-T2：出口路径去重（**1 周**）

| # | 内容 | 对应 |
|---|---|---|
| T2-1 | `exit_nodes` 改为廉价可读结构（如 `ArcSwap<Vec<IpAddr>>` / `Arc<Vec<_>>`），去掉热路径 `exit_nodes.read().await` | TD-4 |
| T2-2 | 减少出口 VIP→PeerId 的重复 `routes` 遍历（缓存或一次性解析），**不得**跳过可达性检查 | TD-5 |
| T2-3 | 基准对比 + 回归 | — |

**⚠️ 实现约束（勿按「只缓存 PeerId」误做）**

- `exit_nodes` 在 **`PeerOutboundPacketRouter`**，`routes` 在 **`PeerMap`** —— **不要**塞进同一个 `RouteSnapshot`
- 现逻辑在解析 PeerId 后还会 `get_gateway_peer_id`（出口是否可达）。缓存若跳过该检查 → **行为变更**（可能打到不可达出口）
- 失效至少覆盖：`update_exit_nodes`；拓扑 / 下一跳变化时须使「可达出口」缓存失效，或每包仍做 gateway 检查
- 可接受的最小改动：只把 `exit_nodes` ArcSwap 化 + 循环内少抢 `routes` 锁；**保留** gateway 检查

**独立性**：只影响出口节点路径；不改 VIP 匹配与 proxy CIDR 匹配。

**上线判据**：出口路径 P99 改善；四条基准路径结果与 T1 基线**语义一致**（含不可达出口回退）。

---

### P-T3：`routes` 列表 ArcSwap 化（**1.5 周**）

| # | 内容 | 对应 |
|---|---|---|
| T3-1 | `routes: tokio::RwLock<Vec<ArcRoute>>` → `ArcSwap<Vec<ArcRoute>>`（或 `Arc<[ArcRoute]>`） | TD-1 |
| T3-2 | ~~PeerMap 层再维护 VIP `exact` HashMap~~ → **默认不做**（见下方） | — |
| T3-3 | 逐处替换 **9 处** `read().await`：`load()` 后仍调用现有 `route.get_peer_id_by_ipv4()` 等 | TD-1 |
| T3-4 | 写侧：`add_route` / `clear_resources` 时构建新 `Vec` → `store()` | TD-1 |
| T3-5 | 并发压测（多写者 + 高 PPS 读） | — |

**⚠️ 关键约束 — 默认只换列表容器**

```rust
// ✅ 推荐范围：去掉 PeerMap 每包 tokio RwLock
routes: ArcSwap<Vec<ArcRoute>>,  // load() 同步；查找仍进 ArcRoute 内部

// ❌ 默认不要做：在 PeerMap 快照里再抄一份 VIP→PeerId
// exact: HashMap<Ipv4Addr, PeerId>,
```

**为何不要在 PeerMap 抄 VIP 索引**：

- `PeerMap.routes` **几乎只在启动时变**；VIP/CIDR 在 OSPF 内（DashMap / ArcSwap）**持续更新**，且 **不通知 PeerMap**
- 若在 `add_route` 时构建一次 `exact` → 很快**过期**，变成正确性 bug，不是性能优化
- VIP/CIDR 新鲜数据已在 route 内部；P-T3 的收益是 **`load()` 替代 `read().await`**，不是「第一次」建精确索引

若将来要做 PeerMap 级 `exact`，必须先有 **OSPF sync → PeerMap 重建快照** 的通知链路（当前不存在）——单独立项，不在本阶段默认范围。

> **禁止**在热路径对 async `Route` 方法 `block_on`。  
> 行为须与原逻辑语义等价（单 route 实践下即：同一 `ArcRoute` 上的查找结果不变）。

**独立性**：纯读侧性能改造（容器替换）。

**上线判据**：四条路径行为**完全一致**；读侧无 `routes` 的 tokio 锁竞争；吞吐不低于基线。

---

### P-T4：行为对齐与 backlog（**1 周，可选**）

| # | 内容 | 对应 |
|---|---|---|
| T4-1 | 跨 route 全局 LPM 语义对齐 | TD-7 |
| T4-2 | 广播 / 统计路径评估是否也需索引 | TD-7 |

⚠️ **这是行为变更，不是纯性能改造。** 若 P-T1 实测显示规模不敏感，可**无限期推迟**。

---

## 6. 阶段汇总

| 阶段 | 内容 | 状态 | 行为变更 |
|---|---|---|---|
| **P-T1** | 基准 + 内部 `SmallVec` | ✅ **已完成并可上线** | ❌（公开 API 保 `Vec`） |
| **P-T2** | 出口路径去重 | ⏸ **暂不实施**（见 §10） | ❌ |
| **P-T3** | `routes` → ArcSwap（仅列表） | ⏸ **暂不实施**（见 §10） | ❌ |
| **P-T4** | LPM 语义对齐 | ⏸ **无限期 backlog** | ✅ **是** |

> **建议范围已收窄为仅 P-T1。** P-T2～T4 默认不做。

---

## 7. 风险

| 风险 | 缓解 |
|---|---|
| P-T3 改造面超预期（9 处锁） | 逐处替换；范围限定为「只换容器」；每处基准验证 |
| PeerMap 抄 VIP 索引过期 | **默认不做** PeerMap 级 `exact`；查找仍进 OSPF 内部 map |
| P-T2 缓存跳过 gateway 检查 | 保留 `get_gateway_peer_id`；配置/拓扑变更时失效 |
| `exit_nodes` 与 `routes` 绑同一快照 | 分属 Router / PeerMap，分别 ArcSwap |
| `SmallVec` 泄漏到公开 API | **内部 SmallVec，公开仍 `Vec`** |
| 基准无法碰到 `pub(crate)` router | ✅ 已用 `peers::bench_support`（`feature = "test-utils"`）+ `easytier/benches/routing_lookup.rs`；测 PeerMap 固定成本，非真实 OSPF DashMap/LPM |
| 基准无法复现 | 记录环境与 `Cargo.lock`；报告附硬件信息 |
| P-T3 收益不明显 | **由 P-T1 基准决定是否继续**，避免无效投入 |
| 热路径误用 `block_on` | 设计评审明确禁止；回退保持 async |

---

## 8. 独立性说明（**可延后路由功能**）

### 8.1 与统一路由模型的依赖关系

| 阶段 | 是否依赖路由功能 | 说明 |
|---|---|---|
| **P-T1** | ❌ **完全不依赖** | 基准 + 返回类型 / 依赖 |
| **P-T2** | ❌ 不依赖 | 只碰出口路径 |
| **P-T3** | ❌ 不依赖 | 只碰 `peer_map.rs` 内部结构 |
| **P-T4** | ⚠️ 间接 | 建议等路由功能启动时一并评估 |

**结论：P-T1 ~ P-T3 可完全独立推进，路由功能延后不影响其价值。**

### 8.2 代码重叠点（唯一一处）

两者都会改 `PeerOutboundPacketRouter::get_msg_dst_peer_ipv4`（及 `PeerManagerCore` 包装）：

| |改动 | 类型 |
|---|---|---|
| 技术债 T1-5 | 内部 `SmallVec`；公开 `PeerManagerCore` 仍 `Vec` | 分配（签名尽量不变） |
| 路由功能 P1 | 插入「次序 3.5」分支 | 逻辑新增 |

**建议**：先做技术债（T1-5），把签名固定下来；路由功能启动时在新签名上插入分支。**顺序反了也可行**，只是可能需要二次调整。

### 8.3 若近期只做技术债，路由功能延后

| 问题 | 影响 |
|---|---|
| **代码会漂移** | 设计文档中的行号锚点届时需重新定位 |
| **基准数据会过期** | before 基线反映的是今天的热路径；**但基准 harness 仍然有效** |
| **技术债会不会白做** | ❌ 不会 —— T1/T2/T3 全是**严格改进**，与是否实现路由功能无关 |
| **需求会不会变** | 可能。届时应复核 [`unified-routing-design.md`](./unified-routing-design.md) §1.1 的约束是否仍成立 |

### 8.4 建议的推进节奏

```
已完成
  └─ P-T1 ✅                 ← harness + SmallVec + before 基线

默认停在这里（不做 P-T2/T3/T4）

路由功能启动时（若需要）
  └─ 复核设计文档行号锚点与 §1.1
  └─ 仅当 profiling 证明锁/出口是热点时，再开 P-T2 或 P-T3
  └─ 统一路由 P1 …
```

**优先级建议**

| 阶段 | 建议 | 理由 |
|---|---|---|
| **P-T1** | ✅ **完成** | 基线 ~165–310 ns，规模不敏感；可上线 |
| **P-T2** | ⏸ **暂不实施** | 出口与 VIP 同量级；无明确痛点 |
| **P-T3** | ⏸ **暂不实施** | 改造面大，基线未显示收益 |
| **P-T4** | ⏸ **无限期 backlog** | 行为变更，实践不触发 |

---

## 9. 结论

- ✅ **性能基础比预期好**：VIP 已是 DashMap；CIDR 已是 `ArcSwap` + 真 LPM（§2.1）
- ✅ **跨 route 的 LIFO 顺序在实践中不触发**，不是活跃 bug（§2.2）
- ✅ **P-T1 完成**：TD-3 / TD-6 已关；T1-6 明确不做
- ⏸ **TD-1 / TD-4 / TD-5 / TD-7 暂不实施**（详 §10）——不是「还欠着必须清」，是「数据证明可以搁置」
- **路由功能延后不影响**：剩余债不阻塞发版；统一路由启动时再复核锚点即可

---

## 10. 2026-10-13 评估：剩余项是否要做 & 能否上线

### 10.1 剩余项

| 项 | 要不要做 | 依据 |
|---|---|---|
| TD-1（routes ArcSwap） | **否（暂）** | `vip_hit` ~165 ns 已含该锁；规模不敏感；9 处改动 ROI 差 |
| TD-4 / TD-5（出口去重） | **否（暂）** | `exit_node` ~172 ns，与 VIP 同量级；非每包必经；缓存易误改可达性语义 |
| TD-7（全局 LPM） | **否** | 实践单 route；属行为变更，非性能债 |
| T1-6（分配 A/B） | **否** | 无前后基线；类型不变量已保证单目的地零堆分配 |

**重启条件**（任一即可重新开 P-T2/T3）：真实高 PPS profiling 显示 `routes` / `exit_nodes` / `resolve_exit_node_peer` 排队；或统一路由大改 `PeerMap` 时顺手做。

### 10.2 当前代码可否正常上线

**可以。** P-T1 生产改动面小、选路语义不变：

| 改动 | 上线影响 |
|---|---|
| `PeerOutboundPacketRouter` 内部 `SmallVec` | 查找逻辑未改；仅容器；广播仍可 spill 到堆 |
| `PeerManagerCore` 公开 API 仍返回 `Vec`（`into_vec()`） | 下游 / dataplane 签名不变 |
| `bench_support` + `routing_lookup` | 仅 `feature = "test-utils"` / bench；**不进默认发版产物** |
| `smallvec` 依赖 | 成熟 crate；热路径常用 |

**不阻塞发版的项**：未做的 TD-1/4/5/7 —— 基线表明不是上线前必须清的性能债。

**常规发版门禁仍适用**（与本债无关）：合入前按 [`../ops/ci-failure-log.md`](../ops/ci-failure-log.md) 跑 clippy / 回归；本篇不替代整仓验收。
