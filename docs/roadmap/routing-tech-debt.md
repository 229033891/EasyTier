# 路由热路径技术债（独立可推进）

## Status

- Status: **Roadmap**（Tech Debt / 待清理，**可独立于路由功能推进**）
- 最近审阅：2026-10-13
- 范围：出站选路热路径上的既有性能债
- **独立性**：**不依赖** [`unified-routing-design.md`](./unified-routing-design.md)；可先做，路由功能延后
- 索引：[`../README.md`](../README.md)

> **分阶段实施**：每阶段 **1～2 周**，**独立可测试、可单独上线**（见 §5）。

---

## 1. 热路径现状（已逐行核实）

```
每个出站 TCP 包
  │
  ├─ gateway/proxy/tcp_proxy_service.rs:320-330
  │     while let Some(data) = output_rx.recv().await {      // 逐包
  │         peer_manager.send_msg_by_ip(packet, dst, false)
  │
  └─ peers/peer_manager.rs:2897 send_msg_by_ip
        ├─ :2910 run_nic_packet_process_pipeline（ACL + pipelines）
        └─ :2931 self.get_msg_dst_peer_ipv4(&ipv4).await     // ← 每包
              │
              ├─ peers/peer_manager.rs:2832
              │    let mut dst_peers = vec![];                // ← TD-3 每包分配
              │
              ├─ peers/peer_manager.rs:2838
              │    self.peers.get_peer_id_by_ipv4(ipv4)       // → peer_map.rs:265
              │         for route in self.routes.read().await.iter() { ... }
              │         ↑ TD-1 tokio 异步锁（routes 列表通常只有 1 个元素）
              │            ↓
              │         route.get_peer_id_by_ipv4()           // peer_ospf_route.rs:4730
              │              └─ lookup_proxy_peer()                  // :673
              │                   let map = self.cidr_peer_id_map.load();   // ✅ ArcSwap 无锁
              │                   map.get_lpm(&cidr)                            // ✅ 真 LPM
              │
              └─ peers/peer_manager.rs:2844
                   self.resolve_exit_node_peer().await          // → peer_manager.rs:2825
                        let exit_nodes = self.exit_nodes.read().await;   // ← TD-4 二次异步锁
                        peers/conn/peer_map.rs:285
                          for exit_node in exit_nodes {
                              get_peer_id_by_ipv4(vip).await    // ← TD-5 重复 O(N) 遍历
                          }
```

---

## 2. ✅ 已核实：性能基础比预期好

### 2.1 CIDR 查找已是 ArcSwap + 真 LPM

```rust
// easytier-core/src/peers/route/peer_ospf_route.rs:673-692
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

**结论**：无锁读 + 真 LPM + `/0` 排除均已实现。**这不是需要引入的新技术栈。**

### 2.2 ✅ 跨 route 的 LIFO 顺序不是活跃问题

```rust
// peers/conn/peer_map.rs:402-405
pub async fn add_route(&self, route: ArcRoute) {
    let mut routes = self.routes.write().await;
    routes.insert(0, route);        // ← 后加入优先（LIFO）
}
```

全仓 `add_route` 仅 3 处：

| 位置 | 场景 |
|---|---|
| `peer_manager.rs:746` | 初始化，**一次** |
| `peer_manager.rs:1888` | 路由同步 reconciler |
| `foreign_network/mod.rs:538` | foreign network |

**实践中每个 `PeerMap` 只持有 1 个 route**，LIFO 顺序在正常场景下不触发。

| 场景 | 是否影响 |
|---|---|
| 常规运行（单 route） | ❌ 不影响 |
| 理论多 route | ⚠️ 跨 route 是 LIFO，非全局 LPM |

**→ 不是活跃 bug，不阻断清理工作。** 记为 backlog（TD-7）。

---

## 3. 债务清单

| ID | 债务 | 严重度 | 修复难度 | 是否行为变更 |
|---|---|---|---|---|
| **TD-6** | 热路径无基准测试 | 🔴 最高 | 低 | ❌ |
| **TD-1** | `routes: tokio::RwLock<Vec<ArcRoute>>`，**9 处** `read().await` | 🔴 高 | 中 | ❌ |
| **TD-3** | 每包 `Vec` 分配（`peer_manager.rs:2832`） | 🟠 中 | **低** | ❌ |
| **TD-4** | 出口路径二次异步锁（`exit_nodes.read().await`） | 🟠 中 | 低 | ❌ |
| **TD-5** | VIP→PeerId 每包重复遍历解析 | 🟠 中 | 低 | ❌ |
| ~~TD-2~~ | ~~CIDR 无索引线性扫描~~ | — | — | ❌ **已证伪**（§2.1） |
| **TD-7** | 跨 route 为 LIFO 而非全局 LPM | 🟢 低 | 低 | ✅ 是（backlog） |

### TD-1 细节

```rust
// peers/conn/peer_map.rs:10, 49-50
use tokio::sync::RwLock;
routes: RwLock<Vec<ArcRoute>>,
```

`self.routes.read().await` 共 **9 处**（行 207 / 225 / 257 / 266 / 276 / 306 / 319 / 438 等），其中 `:266` 在热路径。

> ⚠️ 范围澄清：该锁保护的是 **route 对象列表**，**不是 CIDR 映射**（后者已是 ArcSwap）。改造面比初判小。

### TD-3 细节

```rust
// peers/peer_manager.rs:2830-2832
pub async fn get_msg_dst_peer_ipv4(&self, ipv4_addr: &Ipv4Addr) -> (Vec<PeerId>, bool) {
    let mut dst_peers = vec![];      // ← 每包堆分配
```

全仓调用点仅 3 处（`peer_manager.rs:1751 / 1755 / 1761`），影响可控。

---

## 4. 可用依赖

```toml
# easytier-core/Cargo.toml
arc-swap.workspace = true      # :25  ← 已在 peer_ospf_route.rs 中使用
dashmap.workspace = true       # :36
parking_lot.workspace = true    # :48
criterion                     #      ← 工作区已有基准（easytier/benches/）
```

---

## 5. 分阶段实施

> 每个阶段**独立可测试、可单独上线**。前一阶段不依赖后一阶段。

### P-T1：建立度量 + 低风险快赢（**1 周**）

| # | 内容 | 对应 |
|---|---|---|
| T1-1 | 为 `get_msg_dst_peer_ipv4` 建 `criterion` 基准（`easytier/benches/routing_lookup.rs`） | TD-6 |
| T1-2 | 三档规模：10 / 100 / 1000 路由 | TD-6 |
| T1-3 | 覆盖四条路径：VIP 命中 / proxy CIDR 命中 / 出口节点 / 未命中 | TD-6 |
| T1-4 | 产出 **before 基线报告** | TD-6 |
| T1-5 | 返回类型 `Vec<PeerId>` → `SmallVec<[PeerId; 1]>` | TD-3 |
| T1-6 | 基准对比，验证分配下降 | TD-3 |

**独立性**：只动返回值类型与新增基准，**不改任何选路语义**。

**上线判据**：基准可复现；单包分配计数 = 0；功能回归全绿。

**⭐ 最优先项**：即使后续不做任何清理，**基准 harness 本身长期有效**，且是评估其他性能工作的前提。

---

### P-T2：出口路径去重（**1 周**）

| # | 内容 | 对应 |
|---|---|---|
| T2-1 | `resolve_exit_node_peer` 的 `exit_nodes.read().await` 纳入快照 | TD-4 |
| T2-2 | VIP→PeerId 预解析缓存，消除每包重复遍历 | TD-5 |
| T2-3 | 基准对比 + 回归 | — |

**独立性**：只影响出口节点路径；不改 VIP 匹配与 proxy CIDR 匹配。

**上线判据**：出口路径 P99 改善；四条基准路径结果与 T1 基线一致。

---

### P-T3：`routes` 列表 ArcSwap 化（**1.5 周**）

| # | 内容 | 对应 |
|---|---|---|
| T3-1 | `routes: tokio::RwLock<Vec<ArcRoute>>` → `ArcSwap<RouteSnapshot>` | TD-1 |
| T3-2 | 快照内保留 `Vec<ArcRoute>` + 预解析 `Ipv4 → PeerId` | TD-1 / TD-5 |
| T3-3 | 逐处替换 **9 处** `read().await` | TD-1 |
| T3-4 | 写侧：构建新快照 → `store()` 原子换 | TD-1 |
| T3-5 | 并发压测（多写者 + 高 PPS 读） | — |

**⚠️ 关键约束**

```rust
pub struct RouteSnapshot {
    /// 必须保留原始列表：广播选路 / 路由统计 / CLI 展示仍需遍历
    routes: Vec<ArcRoute>,
    /// 预解析索引：VIP → PeerId（加速精确命中）
    exact: HashMap<Ipv4Addr, PeerId>,
}
impl RouteSnapshot {
    #[inline]
    fn lookup(&self, ip: &Ipv4Addr) -> Option<PeerId> {
        self.exact.get(ip).copied()                    // O(1) 精确命中
            .or_else(|| self.routes.iter()             // 回退：保持现有语义
                .find_map(|r| block_on_lookup(r, ip)))
    }
}
```

> **`routes` 列表必须保留** —— 索引是加速查询，不是替换数据源。
> **回退路径必须与原逻辑逐字节等价**，确保行为不变。

**独立性**：纯读侧性能改造。

**上线判据**：四条路径行为**完全一致**；读侧无锁竞争；吞吐不低于基线。

---

### P-T4：行为对齐与 backlog（**1 周，可选**）

| # | 内容 | 对应 |
|---|---|---|
| T4-1 | 跨 route 全局 LPM 语义对齐 | TD-7 |
| T4-2 | 广播 / 统计路径评估是否也需索引 | TD-7 |

⚠️ **这是行为变更，不是纯性能改造。** 若 P-T1 实测显示规模不敏感，可**无限期推迟**。

---

## 6. 阶段汇总

| 阶段 | 内容 | 工时 | 上线判据 | 行为变更 |
|---|---|---|---|---|
| **P-T1** | 基准 + `SmallVec` | **1 周** | 分配=0，回归全绿 | ❌ |
| **P-T2** | 出口路径去重 | **1 周** | 出口 P99 改善 | ❌ |
| **P-T3** | `routes` ArcSwap 化 | **1.5 周** | 四路径行为一致 | ❌ |
| **P-T4** | LPM 语义对齐（可选） | **1 周** | 需单独评估 | ✅ **是** |
| | **建议范围 P-T1~T3** | **3.5 周** | | |

> **P-T4 建议暂不做** —— 待 P-T1 基准数据证明规模确实敏感后再评估。

---

## 7. 风险

| 风险 | 缓解 |
|---|---|
| P-T3 改造面超预期（9 处锁） | 逐处替换；每处单独 PR + 基准验证 |
| 索引与源数据不一致 | 索引**只在快照构建时生成**，与数据同生命周期 |
| `SmallVec` 是公开 API 变更 | 全仓仅 3 个调用点，影响可控 |
| 基准无法复现 | 记录环境与 `Cargo.lock`；报告附硬件信息 |
| P-T3 收益不明显 | **由 P-T1 基准决定是否继续**，避免无效投入 |

---

## 8. 独立性说明（**可延后路由功能**）

### 8.1 与统一路由模型的依赖关系

| 阶段 | 是否依赖路由功能 | 说明 |
|---|---|---|
| **P-T1** | ❌ **完全不依赖** | 只碰基准文件 + 返回类型 |
| **P-T2** | ❌ 不依赖 | 只碰出口路径 |
| **P-T3** | ❌ 不依赖 | 只碰 `peer_map.rs` 内部结构 |
| **P-T4** | ⚠️ 间接 | 建议等路由功能启动时一并评估 |

**结论：P-T1 ~ P-T3 可完全独立推进，路由功能延后不影响其价值。**

### 8.2 代码重叠点（唯一一处）

两者都会改 `get_msg_dst_peer_ipv4`：

| |改动 | 类型 |
|---|---|---|
| 技术债 T1-5 | 返回类型 `Vec` → `SmallVec` | 签名变更 |
| 路由功能 P1 | 插入「次序 3.5」分支 | 逻辑新增 |

**建议**：先做技术债（T1-5），把签名固定下来；路由功能启动时在新签名上插入分支。**顺序反了也可行**，只是可能需要二次调整。

### 8.3 若近期只做技术债，路由功能延后

| 问题 | 影响 |
|---|---|
| **代码会漂移** | 设计文档中的行号锚点届时需重新定位 |
| **基准数据会过期** | before基线反映的是今天的热路径；**但基准 harness 仍然有效** |
| **技术债会不会白做** | ❌ 不会 —— T1/T2/T3 全是**严格改进**，与是否实现路由功能无关 |
| **需求会不会变** | 可能。届时应复核 [`unified-routing-design.md`](./unified-routing-design.md) §1.1 的约束是否仍成立 |

### 8.4 建议的推进节奏

```
近期（现在 ~ 1 个月）
  └─ P-T1（1 周）           ← 基准 + SmallVec，收益明确、风险极低
  └─ 视基准数据决定 P-T2     ← 若基准显示出口路径有问题则做

路由功能启动时（明年）
  └─ 复核设计文档的行号锚点与 §1.1 约束
  └─ P-T3（1.5 周）        ← 可选：若 P-T1 基线良好，收益有限
  └─ 统一路由 P1（1.5 周）  ← 从 D1/D2/D3 开始
  └─ ...
```

**优先级建议**

| 阶段 | 建议 | 理由 |
|---|---|---|
| **P-T1** | ⭐ **现在做** | 1 周、风险极低、基准 harness 长期有效、后续所有性能决策的依据 |
| **P-T2** | 视P-T1 数据决定 | 只在出口路径确实慢时才做 |
| **P-T3** | 推迟到路由功能启动时 | 改造面最大（9 处锁），无明确痛点时不值得动 |
| **P-T4** | 暂不做 | 行为变更，需单独评估 |

---

## 9. 结论

- ✅ **性能基础比预期好**：`ArcSwap` + 真 LPM 已在 CIDR 路径上实现（§2.1）
- ✅ **跨 route 的 LIFO 顺序在实践中不触发**，不是活跃 bug（§2.2）
- ✅ **P-T1 ~ P-T3 可完全独立于路由功能推进**
- ⭐ **P-T1（1 周）是最值得现在做的事**：风险极低，且把后续的性能判断从「猜测」变成「有数据」
- **P-T3 建议推迟** —— 无明确痛点时不值得动9 处锁
- **路由功能延后到明年是合理的** —— 设计文档已完整保存，届时复核锚点即可