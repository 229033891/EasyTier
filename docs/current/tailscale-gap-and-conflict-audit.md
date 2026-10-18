# EasyTier vs Tailscale：延迟 / 卡顿 / 偶发不通 — 代码层审查

## Status

- Status: **Current**（静态审查 + 2026-10-17 一轮代码修复；**仍无现网抓包/压测**）
- 最近审阅：2026-10-17 v2 **勘误**（§8 P1 删过时的 `P-AUDIT.8` 旧条目、§1.2 删重复行、§5.1 补记 P-AUDIT.7 已落地、§11 修正「easytier crate 无法构建」）；同日首版（**P-AUDIT.1/2/3/4/9/10 + S1b + `latency_first` 默认统一 + KCP 对齐 已合入、待现网验收**；新增 §5.1「不通 vs 卡顿」分栏与失效链、§9 设计原则三轴、§10 默认值一致性；**并推翻两条初版结论**：§3 C7「状态页绿一小时」、§5.1「纯 P2P 无中继」）
- 范围：对照 Tailscale（WireGuard 会话保持 + DERP）语义，区分 **代码 bug/冲突** vs **设计默认** vs **运维/环境**
- 关联：[`p2p-direct-stability-audit.md`](./p2p-direct-stability-audit.md)（S1–S5/S1b）、[`p2p-direct-throughput-audit.md`](./p2p-direct-throughput-audit.md)（B1–B3）、[`peer-connections.md`](./peer-connections.md)、[`traffic-steering.md`](./traffic-steering.md)
- 修复跟踪：[`../roadmap/connection-stability-todo.md`](../roadmap/connection-stability-todo.md)（P1.5 + 本文 P-AUDIT.*）
- 索引：[`../README.md`](../README.md)

> 本文只描述**今天代码做什么**与分类结论。建议修复清单**不是已落地行为**。

---

## 1. 结论摘要

相对 Tailscale，用户感知的「连接慢 / RDP 卡 / 有时 VIP ping 不通」来自三层叠加：

| 层 | 占比（定性） | 本质 |
|----|--------------|------|
| **设计差距** | 主因 | 拆连重建、TCP 回落、静态 MTU、出厂 bond、队列满丢包（S1–S5） |
| **尖锐冲突 / 半 bug** | 加重偶发不通 | 发送缓存跳过质量熔断；`has_peer`≠live conn；无路由/`BufferFull`/入 NIC **静默丢** |
| **运维/环境** | 易误判为产品 bug | 本机代理劫持、Windows 残留 TUN、underlay exclude 不全、L2/L3 策略 |

**一句话**：UI「已连接」= 至少一条 `PeerConn.is_closed == false`；VIP/RDP 还要过 L2→L3→`default_conn`/bond→隧道队列。Tailscale 把这些绑在同一会话上；EasyTier 拆开了，所以状态绿仍可能业务黑。

```mermaid
flowchart LR
  UI["UI: PeerConn 行 !is_closed"] --> Ctrl["控制面存活"]
  VIP["VIP ping / RDP"] --> L2["L2 OS 进 TUN"]
  L2 --> L3["L3 OSPF / VIP 下一跳"]
  L3 --> Send["send_msg: bond 或 default_conn"]
  Send --> Data["UDP/TCP 隧道"]
  Ctrl -.不等于.-> Data
```

### 1.1 2026-10-17 一轮修复的落地清单（**代码已合入，全部待现网验收**）

| ID | 一句话 | 类别 |
|----|--------|------|
| P-AUDIT.1 | 热路径 `default_conn`/bond 缓存尊重 closed + loss fuse；fused incumbent 暴露窗口 ≈20s → 0 | bugfix |
| P-AUDIT.2 | 空 `dst_peers` 丢包计数 `data_plane_no_dst_peer_drops` | 可观测性 |
| P-AUDIT.3 | NIC 满 / 隧道环满计数（含**中继转发腿**） | 可观测性 |
| P-AUDIT.4 | `has_peer_with_live_conn` 闸门；**坏直连不再堵死中继兜底** | **行为变更** |
| S1b | UDP 打洞 conn 保活上限 32s → **10s** | bugfix |
| DEF | `latency_first` core/GUI 默认统一为 `true`（质量优先） | **行为变更** |
| P-AUDIT.9 | Bond 成员改**一致哈希**；纯重排导致 0 流迁移（原按位置哈希会重排） | bugfix |
| P-AUDIT.10 | `AVOID_RELAY_COST` 有限化 + `path_latency` 如实上报 + Web 渲染「政策绕行」 | **行为变更**（状态面数值） |
| KCP | GUI `enable_kcp_proxy` 由 `true` 对齐 core `false`（「KCP 模式不开启」） | **行为变更**（新建 GUI 配置） |
| P-AUDIT.5 | 状态页协议列在**所有隧道均为自带重传传输**时给出 ⚠ + 说明（TCP-over-TCP 提示） | 可观测性 |
| P-AUDIT.7 | `ping_fail_close_count` 默认 **5 → 10**；顺带修掉 `peer_conn.rs` 里硬编码的 `5` 回落值（与 `gen_default_flags` 会漂移） | **行为变更** |
| P-AUDIT.11 | L2 路由安装改**事件驱动**：`peer_info` 变更即唤醒 `ProxyCidrMonitor`，1s tick 降为兜底；`maintain_sessions` 在 `PeerAdded`/`PeerRemoved` 时立即重算 | 延迟优化 |
| P-AUDIT.12 | 查不到路由的首包**短暂暂存**（`PendingPackets`，TTL 500ms、每目标 4 包、全局 64 包）而不是直接丢；新指标 `data_plane_no_dst_peer_buffered` | 延迟优化 |

> **行为变更需要 release note**：`latency_first`（既有未显式配置的 TOML/CLI 部署升级后跨 peer 选路可能改走中继）、P-AUDIT.4（坏直连改走中继而非直接失败）、P-AUDIT.10（`path_latency` 不再抹掉政策惩罚，数值会变大）、KCP 默认关闭、`ping_fail_close_count` 5→10（拆连判定从 ~15s 放宽到 ~30s）。
> **`cargo test -p easytier-core --lib` 964 passed / 0 failed**（本轮 P-AUDIT 口径；全仓 CR 轮合入后同口径 **986**、叠加 `proxy-smoltcp-stack` **1150**，见 [`../roadmap/code-review-fixes-2026-10.md`](../roadmap/code-review-fixes-2026-10.md) §6）；Web `vitest` 192 passed；clippy 无新增告警（P-AUDIT.9 遗留的 `bond_member_for_flow is never used` 已于 CR 轮加 `#[cfg(test)]` 门控消除）。四份文档（本文 / `peer-connections.md` / `p2p-direct-stability-audit.md` / Roadmap）已同步。

### 1.2 仍未解决（按用户原则的重要性排序）

| 优先级 | ID | 为什么重要 |
|---|---|---|
| P2 | **P-AUDIT.10b** / **8b** | proto 缺「中间跳拒载」「最近可达」字段，状态面只能看出「被绕行」「有路由」，看不出细节 |
| P2 | P-AUDIT.6 队列背压 | 满则丢是设计，但突发下内层 TCP 误判拥塞。**本轮明确不做**（见 §11） |
| P2 | PMTU / MSS 钳制 | 出厂 1280 已对齐，极端嵌套仍可能超。**本轮不做**（见 §11） |
| P2 | P1.5 抖动宽限 | 拆连阈值已放宽到 10；「仅在无替代路径时才激进拆」的正确形态仍需改（见 §8 P1）。**2026-10-17 实测补强证据**：同一 peer 的 4 条 conn 在 **19 秒内被全部拆掉**，`loss_rate_1` 递增 0.10→0.14（雪崩），ET 缺少「保留最后一条可用 conn」的保护 |
| **待定性** | **连接中途偶发丢包 / 200–500ms 时延尖峰** | 2026-10-17 现网实测**证伪**了「ET 控制面动作」与「bond 切路」两个假设（ET 全程静默时同样复现）。成因仍未定位，落点疑在对端上行 / NAT / 中继排队。**完整证据、方法论与重测清单见 [`packet-loss-investigation-2026-10.md`](./packet-loss-investigation-2026-10.md)** |
| — | ~~P-AUDIT.8 收紧过期策略~~ | **已推翻**：实测收敛在 ~1s，见 §3 C7。收紧反而有 flapping 风险 |

---

## 2. UI「已连接」含义（已核实）

| 信号 | 位置 | 含义 | 不保证 |
|------|------|------|--------|
| `PeerConnInfo.is_closed == false` | `peer_conn.rs` ~1549；`list_peer_conns` 过滤已关 | 隧道对象未关 | 该 conn 是 `default_conn` / bond 成员；`loss_rate` 低 |
| `quality_score` / `quality_fused` | `peer.rs` `list_peer_conns` ~411–445 | 状态面解释选路 | 发送快路径**不**每包重算 |
| `PeerMap.has_peer` | `peer_map.rs:148-150` | map 里有 `Peer` 条目 | `has_live_conns()`（对比 `list_peers_with_conn` :366-373） |
| `send_msg_by_ip` 成功 `Ok` | `peer_manager.rs:3328-3330` | 可能是**故意丢包后返回 Ok** | 包已进对端 |

因此：**状态页绿 ≠ VIP 可达** —— 这是产品语义，不是显示 bug；但若干静默丢路径会让排障极难（§3）。

---

## 3. Bug / 尖锐冲突（代码证据）

> 每条给出：**机制**（为什么会坏）→ **改前/改后行为** → **影响面**（是 bugfix 还是行为变更）→ **单测锚点**。

### C1 — `default_conn` 快路径不重验 closed / 质量（✅ 已修 = P-AUDIT.1）

**位置**：`easytier-core/src/peers/conn/peer.rs` `Peer::send_msg` / `select_conn`

**机制（三段叠加，所以窗口比「5s」大得多）**

1. 缓存命中即直接 `conn.send_msg`，**不查 `is_closed()`、不查 `loss_fuse`**。
2. 主动清缓存有两条路，都不够快：
   - 定时任务 `default_conn_clear_task`：每 **5s**，且**仅当 `conns.len() > 1`** 才 `store(None)`；
   - 关连事件监听：仅当被关的 conn **指针匹配**缓存时才清。
3. 即使缓存清了，`select_conn` → `pick_default_conn` 还有**质量迟滞**：`switch_windows = 3`，需连续 3 个窗口（≈15s）更优才切。

⇒ **一条已 fused 的 incumbent 最坏可承载 ≈ 5s（缓存）+ ≈ 15s（迟滞）≈ 20s 的业务**。

**单连接场景为什么不需要修**：`conns.len() == 1` 时定时任务不清缓存，看似是洞；但此时 `has_unfused_alternative` 恒为 `false`，新逻辑按设计**仍返回 false**（唯一路径即使 fused 也继续发）。**这是刻意的 no-op，不是漏网** —— 避免无替代路径时反复拆了又建（抖动）。此语义由 `cached_path_unusable` 的 `has_unfused_alternative` 参数表达。

**改前 / 改后**

| | 改前 | 改后 |
|---|---|---|
| cached conn 已 closed | 继续发（靠指针匹配兜底，多数情况能清） | `is_closed()` 立即失效并重选 |
| cached conn fused 且有**已验证未熔断**替代 | 继续发 ≤20s | 立即失效重选（`incumbent_unusable` 分支本就绕过迟滞） |
| cached conn fused 但**唯一**路径 | 继续发 | **继续发**（设计如此，不抖动） |
| 每包开销 | `ArcSwap::load` + send | 同 + 一次 conn 扫描（纯原子读，见下） |

**一致性要点**：判定必须用 `is_unfused_verified`（`!unverified_hole_punch && !fused`），与 `pick_default_conn`、`pick_bond_set` 内部的 `has_unfused` **逐字一致**。若只用 `!fused`，会在「唯一替代是未验证打洞」时错误清缓存。

**性能**：新增的每包 `conn_select_metrics` 扫描只做原子读（jitter 走 15 元素窗口，`bond_count=2` 时 ≈46 次 relaxed 原子读 ≈46ns，50k pps 下 ≈0.23% 单核）。早期版本每包还会 `context.flags()` 深拷贝 `FlagsInConfig`（50+ 字段、约 5 次堆分配），已改为 `PeerContext::{bond_config, conn_select_config}` 借快照，**零分配**。

**影响面**：纯 bugfix，不改变选路语义（只让既有规则更早生效）。

**单测**：`conn_select::tests::cached_path_unusable_closed_or_fused_with_alternative`、`is_unfused_verified_matches_selector_alternative_rule`。

### C2 — bond 缓存只验 `!is_closed()`，质量只在重建时生效（✅ 已修 = P-AUDIT.1 缓存侧 + P-AUDIT.9 选流侧）

**原缺陷（两个独立层面，都已修）**

**(a) 缓存不校验质量** —— `select_bond_conns` 命中缓存的条件只有「成员均未 closed」；成员从「好」劣化到 fused 后，最多要拖到下次 5s 清缓存或 `inspect_err` 失败才重建。

- 出厂 `peer_link_bond_count=2`（`toml.rs:171`）→ **默认就走 bond 分支**。
- **已修**：与 `default_conn` 共用 `cached_path_unusable` + `is_unfused_verified`，且判定为**每包**执行而非等 5s。

**(b) 流按位置哈希钉成员（这才是「部分流通、部分不通」的根因）** —— `bond_member_index(key, member_count) = key % member_count` 读的是成员的**位置**，而 `pick_bond_set` 在质量档变化时会**重排**成员（`cmp_candidate_score_then_id`）→ 同一流的哈希目标换到另一条 conn。

- 症状：VIP ICMP 与 RDP 五元组不同 → 分到不同成员 → **一条通一条不通**；换路瞬间两条独立 UDP 路径的时延差造成乱序 → RDP 视作丢包。
- **已修（P-AUDIT.9）**：改为**一致哈希**（对每个成员的 `conn_id` 求哈希取最小）—— 纯重排导致 **0 个流迁移**，且**绝不会**把流指向**已在集合里的另一个成员**。
- ⚠️ **刻意不强制选最优成员**：bonding 的价值是聚合吞吐；压成「永远走最优」等于删能力（与把 `bond_count` 降回 1 同类错误）。质量仍通过 `pick_bond_set` 决定**成员资格**。

**分类**：**尖锐冲突**；与稳定性文档 S4 同根。

### C3 — 已死的 Peer 条目堵死中继兜底（✅ 已修 = P-AUDIT.4；初版只看到一半）

**初版误判**：初版只记了「网关闸门用 `has_peer` 太宽」，**漏了更严重的一半** —— 直连分支本身也没有 live conn 检查，而它排在 relay 分支**前面**。

**机制（改前 `send_msg_internal` 的分支顺序）**

```text
① latency_first && gateway≠dst && has_peer(gateway)      → relay     ← 闸门只查 contains_key
② get_peer_by_id(dst)                                    → 直连     ← 同样不查 live conn ★
③ is_self(dst)                                           → 自发
④ foreign_network_client.has_next_hop(dst)               → 跨网
⑤ get_gateway_peer_id(dst) && has_peer(gateway)          → relay     ← 同样只查 contains_key
```

`has_peer` 的问题：`peer_map.contains_key(&peer_id)`。而 Peer 条目由**关连事件监听**异步清理，**最后一条 conn 断开后条目仍在一段时间内存在**。

★ 于是：直连 conn 已死 → `get_peer_by_id(dst)` 仍返回它 → **分支 ② 命中** → `Peer::send_msg` → `select_conn()` 返回 `None` → `PeerNoConnectionError` → **函数直接返回，永远走不到分支 ⑤ 的中继腿**。

同理，分支 ①/⑤ 的闸门放行一个已死网关后，中继内部同样会失败（`RelayPeerMap` → `send_msg_to_next_hop` → `send_msg_directly` → `Peer::send_msg` → `PeerNoConnectionError`）。

**这正是「中继本该救你，却没救成」的最坏形态**，也是 §5.1 里「不通」链的关键一环。

**改法（保持边界克制）**

| 判断 | 改前 | 改后 |
|---|---|---|
| 「能不能交给它」（发送前可用性） | `has_peer` / 无检查 | **`has_peer_with_live_conn`**（`my_peer_id` 或 `has_live_conns()`） |
| 「下一跳是谁」（选路/拓扑） | `has_peer` | **保持 `has_peer`** |

**为什么不把 `get_gateway_peer_id` 也改掉**：它回答的是拓扑问题（「按 OSPF，下一跳应该是谁」），不是可用性问题。把它改成 live conn 会**改变路由决策本身**（可能选到不同的中继），属于行为变更而非可用性修复。两者必须分开 —— 这条边界已写进 `PeerMap::has_peer_with_live_conn` 的 doc comment 与 [`peer-connections.md`](./peer-connections.md)。

对比参考：`list_peers_with_conn`（`peer_map.rs:366-373`）本来就用 `has_live_conns()` 过滤；问题是转发决策没用它。

**影响面**：**这是行为变更**。改后「直连条目还在但 conn 已死」的目标会**改走中继**而不是直接失败。对直连更近的用户可能多一跳 —— 但那是质量选路该做的判断，比直接丢包好。

**单测**：`peers::tests::has_peer_with_live_conn_tracks_conn_liveness_not_map_presence` —— 核心断言是 `has_peer(2)` 与 `has_peer_with_live_conn(2)` **在关连后分叉**（前者仍 true、后者 false）。

### C4 — 无下一跳时 `Ok(())` 静默丢（✅ 已修 = P-AUDIT.2）

**位置**：`peer_manager.rs` `send_msg_by_ip` 的 `dst_peers.is_empty()` 分支。

**改前**：仅一条 `tracing::info!`，无计数器。上层拿到 `Ok(())`，无法区分「已发出」与「故意丢弃」。

另：policy `RouteLookup::Block` 返回空 `dst_peers`（**丢包本身是设计**，防泄漏到物理网关），但同样无计数器。

**改后**：`no_dst_peer_drops` 计数器 +1（`data_plane_no_dst_peer_drops`，标签 `network_name`），日志降为 `debug`。

> **为什么是 `debug` 不是 `warn`**：这条路径**每包**触发，`warn!` 会在丢包时把日志打爆。**以计数器为可观测信号**。若确需 `warn` 语义，按同文件 `take_path_unavailable_warn_token()` 的令牌限流范式改。
> **policy Block 复用同一计数器**：它返回空集，自然落到 `dst_peers.is_empty()` 这条路径上，不重复计数。

### C5 — 入 NIC / 发送环静默或难观测丢（✅ 已修 = P-AUDIT.3；背压部分见 P2）

| 点 | 位置 | 改前 | 改后 |
|----|------|------|------|
| 入 NIC | `NicPacketProcessor::try_process_packet_from_peer` | `trace!` 丢包 | `data_plane_nic_channel_drops` + `debug` |
| UDP 出站环满 | `tunnel/udp.rs` `force_send`；容量 `socket/ring.rs` **128**（预留 4） | `TunnelError::BufferFull` 上抛，无统计 | `data_plane_tunnel_buffer_full` + `debug` |
| 中继转发腿 BufferFull | `PeerPacketRouter::handle_packet` | 无统计 | **同计数器**（初版遗漏，中继节点恰恰最需要） |

**症状**：小包 ICMP 常通、RDP 突发卡顿（与吞吐文档 B2 / 稳定性 S5 一致）。

**分类**：满则丢是**设计**；缺计数/状态面才是**缺陷**。

**覆盖范围**：`note_tunnel_buffer_full()` 必须在**全部 4 处** `send_msg_internal` 调用点执行（`send_msg_by_ip` 的 fanout 分支 / 指定 peer 分支、中继转发分支）。漏掉任一处，对应拓扑就不可观测。

### C6 — LeastHop「有 Peer 条目即当直连」（P1，与 C3 相关）

**位置**：`get_gateway_peer_id` :202-204。

若 Peer 仍在 map 但无可用 conn，LeastHop 仍选 dst 为 gateway，后续 `send_msg_directly` / `peer.send_msg` 失败路径依赖 C3。多跳 VIP 在「以为直连」时更容易黑。

> **2026-10-17 的处置**：该短路**保持不变**（它回答的是拓扑问题）。它的实际后果 —— 一条已死的直连**堵死中继兜底** —— 已由 **C3 / P-AUDIT.4** 在**发送前可用性闸门**上解决；且 `latency_first` 默认改为 `true` 后，最短跳短路对绝大多数目标**不再生效**（LeastCost 下 `get_gateway_peer_id` 会走路由表，允许中继腿按代价胜出）。

> **2026-10-17 补（复核后）**：现网长黑洞（[`packet-loss-investigation-2026-10.md`](./packet-loss-investigation-2026-10.md) §10）之后，「该短路保持不变」被重新审议。首版「选路也只选有活连接的下一跳 + 兜底 relay」的改法**未被接受**：它只校验可用性、不校验方向性，会引入 A↔B 跨节点环路，且不解决 §10 的故障（DEV 侧网关 conn 是活的）。**现状（Phase A）**：该短路保留，另加一条——**路由点名的那个下一跳若在本机已无活连接，则不被选中**，返回空让调用方尽早失败；仍**不搜索**替代下一跳。方案与问题点见 [`../roadmap/live-next-hop.md`](../roadmap/live-next-hop.md) §4、§6。

### C7 — 「状态页长时间显示已连接」：**初版判断错误，实测后推翻（不改代码）**

**初版说法（错）**：「peer 信息在 `REMOVE_UNREACHABLE_PEER_INFO_AFTER = 90s` / `REMOVE_DEAD_PEER_INFO_AFTER = 3660s` 下才清理，所以对端掉线后状态页最长可绿**一小时**。」

**错在哪**：`REMOVE_*` 这两个常量管的是 **peer_info 记录（hostname / IPv4 / feature flag）的垃圾回收**，**不是状态页那一行的去留**。状态页的行由 `list_routes` 产出，而它有一道更早的闸门：

```rust
// peer_ospf_route.rs::list_routes
let Some(next_hop_peer) = self.get_next_hop(*item.key()) else {
    continue;                       // ← 没有下一跳 ⇒ 这一行直接不出现
};
```

**真实链路（逐环追踪）**

| 环 | 机制 | 代码锚点 | 耗时 |
|---|------|----------|------|
| ① | 对端 conn 全部断开后，邻居**立刻**不再通告它 —— `RouteInterface::list_peers()` 只收 `peer_map.list_peers_with_conn()`，后者按 `has_live_conns()` 过滤 | `peer_manager.rs` `PeerManagerRouteInterface::list_peers`；`peer_map.rs::list_peers_with_conn` | 即时 |
| ② | 邻居下一次 route sync 后，本机 `conn_map[邻居]` 不再含它 → 图里少一条边 | `sync_route_info` | ~1s |
| ③ | `gen_next_hop_map_*` 不再为它产出条目 → `get_next_hop` 返回 `None` → **状态页那一行消失** | `list_routes` 的 `continue` | 同一次 rebuild |
| ④ | 陈旧的 `next_hop_map` 条目由 `clean_expired_route_info()` 清理（`retain(v.version >= cur_version)`） | `build_from_snapshot` 末尾 `:675` 调用 | 同一次 rebuild |
| ⑤ | `peer_infos` / `ipv4_peer_id_map` 记录被 GC（`clear_expired_peer`，每 60s 跑、阈值 90s） | `:3482` | 90–150s（**但此时已不影响状态页**） |

⇒ **状态页残留 ≈ 一次 route sync（~1s）量级，不是 90s，也不是 3660s。**

**仍然存在的边角**（已由单测固化）：只要还有邻居**继续声称**与该对端的邻接，它就会带着陈旧数据留在表里 —— 直到那个邻居也把它 prune 掉。单测
`stale_peer_survives_while_a_neighbour_still_claims_adjacency` 先断言「陈旧但仍被声称 → 存活且仍出现在 `list_routes`」，再断言「邻居停止声称 + 版本推进 → 被移除且从 `list_routes` 消失」。

**为什么不去收紧过期策略**：① 正常路径已收敛到 ~1s，收紧没有收益；② 误删一条**其实还能用**的路由会直接打断在途流量（flapping），风险远大于收益；③「有路由」与「最近可达」的状态面区分需要新增 proto 字段 + 跨语言 codegen，改动面大且**无现网环境验收**，记为 P-AUDIT.8b（`cargo check -p easytier` 本身可跑，见 §11）。

> **教训**：本条与 §5.1 的「不是没有中继，是默认不让中继兜底」是同一类错误 —— **只读了常量就下结论，没有把「谁真正门控了用户可见行为」追到底**。凡是影响用户感知的判断，都应先找到**实际门控点**再下结论。

### C8 — 未验证打洞 conn 的 OSPF 边代价空窗（**已复核，暂不改**）

**机制**：`best_edge_cost`（`peer_map.rs`）过滤掉 `unverified_hole_punch`；若某 peer 只有这一条未验证的 conn，它不会写进 `direct_peer_info`，于是 `RouteCostCalculatorImpl::calculate_cost` 落到 `unwrap_or(500)`。

**为何不改（量级复核）**：`ospf_edge_cost_from_score(score) = round(score × 1000)`，默认权重下 ≈ RTT 毫秒量级，`500` ≈ **50ms RTT 等效**。它**不会让路径不可用**，只是让新打通的直连**晚约 1s**（首个 ping 间隔）参与竞争。给未验证 conn 一个「乐观初始代价」的收益 ≈1s，代价是**可能优先一条其实没真打通的路**（此刻 `select_conn` 同样尚未验证它）。**收益不抵风险**，故保留现状并在此记录，避免重复讨论。

**边界确认（避免误判为路由中断）**：OSPF **邻接**来自 `conn_map[src].connected_peers`（`build_peer_graph_from_snapshot`），与边代价**相互独立**。代价缺失只降级为 500，**邻接仍在** → 不会导致「查不到路由」。

---

## 4. 设计差距（非 bug；解释 vs Tailscale）

与 [`p2p-direct-stability-audit.md`](./p2p-direct-stability-audit.md) S1–S5 对齐，此处只列对照，不重复长文。

| ID | EasyTier 默认 | 代码锚点 | Tailscale/WG 对照 | 用户症状 |
|----|---------------|----------|-------------------|----------|
| S1 | 连续 ping 失败 **5** 次拆 PeerConn | `peer_conn_ping.rs:327-336`；`toml.rs:159` | 会话保持，短暂丢包后续传 | 闪断后 RDP 冻结数秒（重握手） |
| S2 | `default_protocol = "udp,tcp"` | `toml.rs:114`；`protocol_preference.rs` | DERP 非裸 TCP 隧道叠 TCP | UDP 被阻 → TCP-over-TCP → 卡死 |
| S3 | flags.mtu **1300**，加密后 TUN **1280**（2026-10-16 前为 1380→1360） | `toml.rs` / `virtual_nic.rs:735-742` | TUN **1280**（已核实） | 出厂已对齐；极端路径仍可再调低 |
| S4 | `peer_link_bond_count=2` 流哈希 | `toml.rs:171`；`peer.rs` bond | 单会话 | 换成员乱序/粘坏链 |
| S5 | 环深 128，满则丢 | `ring.rs` / `udp.rs` | 背压更常见 | 突发卡顿 |
| **S1b** | **UDP 打洞 conn 缺 NAT 保活上限**：自适应 ping 退避到 32s，而 NAT UDP 老化常见 ~30s | **2026-10-17 已修**：UDP 打洞 **10s**（TCP 打洞 1s 不变）；`peer_manager.rs` `hole_punch_ping_max` | WG 靠 rekey + 自身 keepalive 维持映射 | 空闲后最长 ~32s 数据黑洞无人察觉，随后拆连 + 重打洞 |

### S1b 详解 — UDP 打洞缺 NAT 保活上限（✅ 已修）

**机制**：控制 ping 是唯一会刷新 NAT 映射的流量。`PingIntervalController` 自适应退避 `1<<backoff_idx`，`max_backoff_idx = 5` → 上限 **32s**。而 NAT UDP 状态老化常见 **~30s**。

⇒ 一条空闲的 UDP 打洞 conn，**两个 ping 之间就可能跨过老化线**，映射消失。此后发往该 conn 的包被 NAT 丢弃，而 EasyTier **完全不知情** —— 直到下一次 ping 失败、连丢 5 次才拆连重打洞。

**为什么 TCP 打洞早就修过、UDP 反而漏了**：`apply_tcp_hole_punch_ping_policy`（源自 #2632）显式**排除** UDP：

```rust
if conn.tunnel_type().is_some_and(is_udp_hole_punch_tunnel_type) {
    return;                        // UDP 打洞不设 1s 上限
}
conn.set_ping_max_interval(Some(Duration::from_secs(1)));
```

当时的问题是「中间设备掐空闲 **TCP** 流」，UDP 不存在该问题；但**UDP 有 NAT 老化这个不同的问题**，被同一处逻辑顺带忽略了。

**改法**：抽成纯函数 `hole_punch_ping_max(tunnel_type, is_directly_connected)`，按 scheme 选上限。

| conn 类型 | 上限 | 理由 |
|---|---|---|
| 直连（非打洞） | `None` → 沿用全局 **10s** | 2026-10 起全局默认也是 10s；此前是 32s，是**直连隧道黑洞**的成因——公网可达≠无 NAT（对端在 NAT 后时本方向映射仍会老化） |
| TCP / FakeTCP 打洞 | **1s** | #2632，中间设备掐空闲 TCP（不变） |
| UDP 打洞 | **10s** | NAT 老化 ~30s，留 3× 余量；代价是每 conn 每 10s 一个 9 字节控制包 |

> **2026-10-17 更新：打洞专用的 10s 上限已推广到全局默认**（`DEFAULT_PING_INTERVAL_MAX_SEC` 32→10），并新增 GUI 表单与 CLI `--ping-interval-max-sec` 配置入口。运维仍可显式设置：`PeerConn::max_ping_interval()` 返回 `min(cap, flags_max)`，所以：
- 设得**更严**（如 3s）→ 生效，用户意图保留
- 设得**更松**（如 60s）→ 被收紧到 10s

NAT 保活是**正确性要求**而非调优旋钮，不允许用大值关掉。

**单测**：`udp_hole_punch_gets_nat_safe_ping_ceiling` 把设计意图固化成可执行断言 —— `10s ≤ 默认值`（同为 10）**且** `10s × 3 ≤ 30s`（老化余量）；另有 `tcp_like_hole_punch_keeps_aggressive_ceiling`、`directly_connected_conns_keep_the_global_default`、`per_conn_cap_never_relaxes_the_operator_setting`。

另：KCP/QUIC proxy **出厂一律不启用**（core `false`，GUI 模板 2026-10-17 由 `true` 对齐为 `false`，见 §11）—— 弱网无自动抗丢包产品化（Roadmap P2.5）。

---

## 5. 默认组合冲突矩阵

| # | 组合 | 触发 | 可见症状 | 应改语义？ |
|---|------|------|----------|------------|
| X1 | `bond_count=2` + 质量迟滞（~15s）+ 5s 缓存 | 一成员丢包升高但未 closed | 同 peer 下流级间歇失败；UI 双绿 | **是**（C1/C2：热路径尊重 fuse） |
| X2 | `udp,tcp` + RDP/SSH | UDP 限速/阻断回落 TCP | 「有时很卡」；状态显示 tcp | 配置可缓；**UI 强提示**或交互场景默认勿回落 tcp |
| X3 | S1 拆连 + 无抖动宽限（P1.5 未做） | 短暂丢包 | 比 Tailscale 更易整段冻结 | Roadmap P1.5 |
| X4 | `p2p_only` / `disable_relay_data` + 需多跳 VIP | 无直连到目标 | UI 有其它 peer 绿；目标 VIP 不通 | **否**（政策）；须文档/UI 标明 |
| X5 | L3 policy `closed` + L2 仍可能装过路由 | 候选全不可用 | ping 丢、无错；peer 仍绿 | Block 是设计；需可观测（C4） |
| X6 | TUN `/0` + underlay exclude 不全 | 出站被本机代理/错误 nic 劫持 | 相对 Tailscale「同网更差」 | 环境；见稳定性审计 §5 步骤 1 |
| X7 | Windows 残留 `et_*` | 异常退出未清 | VIP 进死接口；core 仍报 PeerConn | 运维；见 `traffic-steering` |

---

## 5.1 「不通」与「卡顿」是两条独立的链（**排障时先分清**）

初版把两者混在一起，导致修复清单偏向了「卡顿」。用户报的两个症状需要分开定位：

| | **卡顿 / 劣化** | **不通 / 黑洞** |
|---|---|---|
| 典型症状 | RDP 冻结、卡顿、时延跳变 | ping 不通、完全无流量 |
| 主要机制 | S1–S5、C1/C2：选错路、承载劣化路径太久、丢包放大 | **黑洞期无人察觉** → 检测 → 拆连 → 重建，四段串联 |
| 时间尺度 | 秒～十秒 | **十秒～一分钟**（改前最长 ~2 分钟） |
| 已修 | P-AUDIT.1（缓存侧）、P-AUDIT.9（选流侧）、S1b（部分） | S1b、P-AUDIT.4、P-AUDIT.7（拆连阈值 5 → 10，短暂抖动不再触发拆-建） |
| 关键特征 | 有流量但质量差 | **UI 全程绿**，控制面毫无异常 —— 但这是因为**PeerConn 客观存在**，**不是**路由过期（后者只有 ~1s，见 §3 C7） |

### 「不通」的完整失效链（改前 / 改后）

| 阶段 | 耗时 | 代码依据 | 改后 |
|------|------|----------|------|
| ① NAT 映射老化，包开始黑洞 | — | UDP 层无 keepalive；UDP 打洞 ping 退避到 32s | **10s 上限**（S1b） |
| ② 黑洞开始 → 首次失败 ping | 最长 **32s** | `peer_conn_ping.rs` `PingIntervalController` | **≤10s** |
| ③ 连续失败 → 拆 conn | 改前 5 次 ≈ 10–15s；改后 10 次 ≈ 25–30s（失败后退避重置为 1s，每轮 1s+2s 超时） | `ping_fail_close_count` **5 → 10**（P-AUDIT.7） | **已放宽**：容忍度近乎免费（两种情况重建都要 ~30s） |
| ④ direct task 感知 conn 消失并重投 | ≤5s | `DIRECT_TASK_LOOP_INTERVAL_MS`；main loop 以 `is_finished()` 移除并重投，`backoff_index` **每次调用重置为 0** | 未改（**已复核：重连不走 60s 退避尾段**） |
| ⑤ 取 IP 列表 + 打洞 + 握手 | 2–13s | 打洞 2–6s；握手 5s | 未改 |

**改后最坏窗口**：~10s（②）+ ~30s（③，10 次 × ~3s）+ ~5s（④）+ ~13s（⑤）≈ **45–60s**；改前 ≈ **60–120s**（② 32s + ③ 15s）。

**这期间 UI 显示什么**：**全程「已连接」**。这不是过期策略的问题（见 §3 C7：状态页残留只有 ~1s），而是**①→③ 期间 PeerConn 客观存在且 `is_closed()==false`** —— 控制面无从得知数据面已黑洞。**要缩短这个窗口，只能从「更快发现黑洞」入手**（S1b 已把发现前的等待从 32s 压到 10s），或从「黑洞期不要拆」入手（P1.5 抖动宽限，减少拆-建带来的二次伤害）。

> **注意区分另一种「不通」**：上表讲的是**已连通的链路中途黑洞**（秒～分钟级）。还有一种是**刚连上时的首包竞态**——peer 已 up、子网已宣告，但本机系统路由 / route-sync 会话还没装好，这期间发过去的包查不到 dst 就被丢。这是**百毫秒～秒级**的另一条链，由 P-AUDIT.11（事件驱动安装）和 P-AUDIT.12（无路由包暂存 500ms 后重发）处理，与上面的黑洞链互不重叠。排障时看时间尺度就能分开：秒级且首次访问必现 → 首包竞态；十秒级且伴随成簇拆连日志 → 黑洞链。

### 2026-10-17 现网实测修正：上面这条链只解释了「黑洞」，没解释「偶发丢包」

用 `ping -n 240 -w 1000 <对端 VIP>` + ET warn 级日志做同步对齐，得到两条必须纠正的归因：

| 原归因 | 实测结论 |
|---|---|
| 「黑洞 = NAT 映射老化」 | **成立**。报告中 `12:57:22–12:58:30` 连续 27 次失败（~71s），DEV 日志在 `12:57:43` 出现 `closing the connection … peer 2752719068`，`12:58:33` 恢复；黑洞起点早于 ET 察觉 21s，与上表 ②③ 的量级一致 |
| 「残留 ~2% 丢包 = bond / conn_select 切出口」 | **证伪**。在一个 4 分钟窗口内 ET **全程零日志**（唯一一条是 wireguard 定时器），丢包 0.4% 与 413/234/72ms 尖峰照样出现；另一个窗口 150 包 **0% 丢包、max 24ms** |
| 「是 ET 在自查 / 换路」 | **证伪**。关掉本机代理后尖峰反而更大（524ms），且两次尖峰期间 ET 日志均为空白 |

因此上表的失效链**只适用于「黑洞期」**。**「连接中途偶发丢包 / 时延尖峰」是另一条尚未定性的链**，其形态是**瞬时排队**（3 个尖峰集中在 18 秒内，其余时间完全平稳），不符合稳定劣化，也不符合 ET 控制面动作的特征。

方法学、原始数据、已确认缺陷与双侧取证方法见 [`packet-loss-investigation-2026-10.md`](./packet-loss-investigation-2026-10.md)。

### 关键澄清：不是「没有中继」，而是「默认不让中继兜底」

初版曾据「纯 P2P 组网」推断「无中继」，**该推断错误**：

- `disable_relay_data = false`（出厂默认）→ **任何节点都能给其它节点做中继**，与是否配置 `public_server_url` 无关（后者默认就是空）。中继**服务器** ≠ 中继**能力**。
- 真正的问题在 `get_gateway_peer_id`：`policy = LeastHop` 时，只要目标有直连 `PeerConn` 就**立即返回目标**，中继腿永不参与竞争 —— 即已拍板的「不区分直连/中转」原则在代码里没落实。已由 **P-AUDIT「DEF」**（`latency_first` 默认统一为 `true`）修正。

> **排查口诀**：先确认 `latency_first` 实际取值，再看 `easytier-cli route` 的 `cost` 是 1（直连）还是 >1（中继），最后才看丢包。

---

## 6. 运维 / 环境（勿当成 core bug）

> 2026-10-17 现网实测把第 1、3 项从「推断」升级为「已证实」，并给出了可复现的判定命令。
> 细节与数据见 [`packet-loss-investigation-2026-10.md`](./packet-loss-investigation-2026-10.md)。

1. **本机透明代理抢走 ET 的外层流量**（**已证实**）。mihomo / Clash 的 `tun` + `auto-route` 会装一条 `0.0.0.0/0 metric 0` 的默认路由，压过物理网关（实测 metric 256）。**ET 到公网服务器与对端公网端点的全部外层流量因此穿过代理的用户态协议栈**（gvisor）。实测该路径 2% 丢包，切回物理网卡后 0%。
   - 判定：`Find-NetRoute -RemoteIPAddress <et 公网服务器 IP>` → 出口若是 `Meta` / TUN 网卡，即命中。
   - 修复：代理侧 `route-exclude-address` 排除 ET 的服务器 / 对端公网 IP；或不用 ET 时关闭 TUN 模式。**注意代理自身不要设 `dns-hijack: any:53` 而不排除 ET 的虚拟 DNS（`10.255.255.254`）** —— 实测那样会让 ET 虚拟 DNS 彻底不可用（节点名解析返回 fake-ip）。

2. **反向劫持：ET 为对端发布的代理子网装路由，可能吃掉本机代理的出口 IP**（**已证实**）。实测 `et` 网卡上有 23 条 `Protocol=NetMgmt` / metric 9000 的 on-link 路由（`104.16.0.0/13`、`172.16.0.0/16`、`192.168.8.0/22` 等），导致本机代理节点的 IP（`104.19.32.86`）出口变成 `et`。**源头是某个节点的 `proxy_networks` / `proxy_cidrs` 发布了整份代理 IP 列表**；清源头比在代理侧加排除更重要。
   - 判定：`Get-NetRoute -InterfaceAlias <dev_name> -AddressFamily IPv4 | Where-Object Protocol -eq NetMgmt`。

3. **Windows wintun / 路由残留**。叠加第 2 项时，残留路由的清理不及时会持续影响选路。

4. **出口 `/0` 与物理默认路由 metric 战争**（Roadmap default-route Phase 2）。实测这是**双向**的：代理抢 ET 的默认路由，ET 的子网路由抢代理节点的出口 IP。

5. **`find_interface_index(<dev_name>)` 失败会让 socket 创建直接失败**（**已证实的代码缺陷，不是环境问题**）。实测 `easytier::arch::windows: Failed to find interface index for et` 出现 **202 次**，每波约 32 次；失败经 `set_ip_unicast_if` → `setup_socket_for_win` → `setup_socket2_ext` 逐层 `?` 上抛，导致那一刻所有需要把出口网卡钉在该网卡上的隧道 / 打洞 socket **全部创建失败**。
   - 判定：`Select-String 'Failed to find interface index' <日志>`，观察计数是否持续增长。
   - 根因是按硬编码 `dev_name` 匹配 `network_interface` 枚举结果，适配器重建 / 枚举异常时失配，且**被当作致命错误**。

6. 状态页 **2s** 轮询：短暂黑洞与秒级尖峰均可能未截获，**排障必须用 `ping` + 日志时间对齐，不能只看状态页**。

---

## 7. 排障与配置（**多数项已出厂修复，此处用于验证与老配置对齐**）

先看两个**一眼能判断**的状态页信号：

| 信号 | 含义 | 对准 |
|---|---|---|
| 「协议」列带 ⚠ | 到该对端**所有**隧道都用自带重传的传输 ⇒ 交互式 TCP 流已是 TCP-over-TCP | S2 / X2 |
| 延迟列显示「政策绕行」 | 该路径含**拒载中转**的节点（`disable_relay_data` / 中继白名单不匹配），绕行是政策强制而非质量选择 | §9 政策轴 |

再看 CLI 与配置：

| 优先 | 检查 / 建议 | 说明 | 对准 |
|------|---------------|------|------|
| 0 | **用 `ping` + 日志时间对齐，不要只看状态页** | 状态页 2s 轮询会漏掉秒级尖峰；ET 日志时间戳是 **UTC**，需 `+8` 换算成本地时间 | §6.6 |
| 0 | 排除代理/路由混杂 | `Find-NetRoute -RemoteIPAddress <ET 公网服务器 IP>` 看出口是否落在代理 TUN 网卡；`Get-NetRoute -InterfaceAlias <dev_name> \| Where-Object Protocol -eq NetMgmt` 看是否装了代理子网路由 | §6.1 / §6.2 |
| 0 | 确认 `latency_first` 实际取值 | 出厂已为 `true`（质量优先）；老配置可能没这一行 | §9 |
| 1 | `easytier-cli route` 看 `cost` | `cost==1` 直连；`>1` 走中继。`>1` 且体验差 ⇒ 质量选路结果如此，或被政策否决 | §9 |
| 2 | `peer_link_bond_count` | **出厂 2**。**对照实验**可设 1 排除 bond 影响，但**不建议长期改**（会删掉聚合能力，见 §3 C2）。**注意：2026-10-17 实测已排除 bond/选路作为偶发丢包的原因** | C2 / S4 |
| 2 | 查日志有无成簇拆连 | `Select-String 'closing the connection' <日志>`，看**同一 peer 是否数秒内多次**。出现即命中 P1.5 抖动宽限缺失（雪崩） | §6 P1.5 |
| 3 | `mtu` | 出厂已 **1300**（TUN 1280）；老配置仍 1380 时改掉或重建 | S3 |
| 4 | `ping_fail_close_count` | 出厂已 **10**；老配置仍 5 想放宽可显式写 5 | S1 / X3 |
| 5 | `default_protocol` | 出厂 `udp,tcp`；确认 UDP 可用时可收紧为 **`udp`** | S2 / X2 |

---

## 8. 修复清单

### P0（代码已落地，**待现网验收**）

| ID | 项 | 今日行为 | 验收 |
|----|-----|----------|------|
| P-AUDIT.1 | 热路径尊重 closed + fuse | `send_msg`：cached `default_conn` closed，或 fused 且存在**已验证未熔断**替代 → 清缓存并 `select_conn`。bond 缓存同样失效后 `pick_bond_set`。唯一路径即使 fused 仍发送。判定复用 `is_unfused_verified`，与 `pick_default_conn` / `pick_bond_set` 的 `has_unfused` **同一谓词**（未验证打洞不算替代）。per-packet 配置解析走 `PeerContext::{bond_config, conn_select_config}`（借快照，**不** clone `FlagsInConfig`） | 单测 `cached_path_unusable_*` / `is_unfused_verified_*`；现网高丢包双路径应离开坏链 |
| P-AUDIT.4 | 转发前可用性闸门 | 新增 `PeerMap::has_peer_with_live_conn`（`my_peer_id` 或 `has_live_conns()`）；`send_msg_internal` 的**两处网关闸门 + 直连分支**均改用它。此前一条已死的直连条目会**先匹配上分支**并在 `Peer::send_msg` 内失败，**永远不会落到可用的中继腿**。选路 `get_gateway_peer_id` 仍用 `has_peer`（拓扑 ≠ 可用性） | 单测 `has_peer_with_live_conn_tracks_conn_liveness_not_map_presence`（断言 `has_peer` 与新谓词**会分叉**）；三节点 mesh 中坏网关应改走直连而非失败 |
| P-AUDIT.2 | 空 `dst_peers` 可观测 | `send_msg_by_ip` 空下一跳：`data_plane_no_dst_peer_drops` + `debug`。policy Block 仍返回空集，由该路径计数 | VIP 无路由时指标上升 |
| P-AUDIT.3 | BufferFull / NIC 满可观测 | NIC `try_send` 失败：`data_plane_nic_channel_drops` + `debug`。出站 `TunnelError::BufferFull`：`data_plane_tunnel_buffer_full`（**含中继转发腿** `PeerPacketRouter::handle_packet`） | 突发丢包可对上计数 |
| **S1b** | **UDP 打洞 NAT 保活上限** | UDP 打洞 conn 控制 ping 上限 **32s → 10s**（TCP 打洞 1s 不变）；`hole_punch_ping_max()` 为纯函数，`min(cap, ping_interval_max_sec)` 只收紧不放宽；直连 conn 不变 | 单测 `udp_hole_punch_gets_nat_safe_ping_ceiling`（断言 10s < 32s 且 10s×3 ≤ 30s） |
| **DEF** | **`latency_first` 默认一致性** | core `gen_default_flags()` / `PeerPolicyConfig::default()` 由 `false` → **`true`**，与 GUI 模板统一。改前 `false`（LeastHop）下 `get_gateway_peer_id` 对有直连的目标**直接短路返回目标**，中继腿永不参与竞争 —— 即「不区分直连/中转」这条已拍板原则（旧 §2.2）在代码里未落实 | TOML/CLI 与 GUI 建出的网络行为一致；⚠️ **行为变更**，既有部署升级后跨 peer 选路可能改变 |

> **日志级别约定**：P-AUDIT.2/.3 的三条丢包路径都是**每包**触发，若用 `warn!` 会在丢包时打爆日志；因此统一 `debug!`，**以计数器为可观测信号**。若确需 `warn` 语义，按同文件 `take_path_unavailable_warn_token()` 的令牌限流范式改。

### P1（**P-AUDIT.5 / .7 / .9 / .10 已落地、待现网验收**；P1.5 / .5b / .10c 未做或核实为非问题。以下含机制与验收要求，便于直接施工）

| ID | 项 | 建议改法（机制级） | 验收 | 跟踪 |
|----|-----|--------------------|------|------|
| **P-AUDIT.9** | **Bond 成员改一致哈希（消除重映射）** | ✅ 已修。缺陷：`bond_member_index(key, member_count) = key % member_count` **按位置**取成员，而 `pick_bond_set` 在质量档变化时会**重排**成员（`cmp_candidate_score_then_id`）→ 同一流的哈希目标换到另一条 conn → 乱序 → RDP 视作丢包（S4）。<br>**改法**：`bond_member_for_flow(key, &[PeerConnId])` 对**每个成员的 conn_id** 求哈希取最小。一致哈希性质：**只有「自己的成员离开」或「新成员胜出」才迁移**，绝不会被指向**已在集合里的另一个成员**；**纯重排导致 0 个流迁移**。<br>⚠️ **刻意不强制选最优成员**：bonding 的意义是聚合吞吐，压成「永远走最优」等于删能力 —— 与把 `bond_count` 降回 1 是同一种错误。质量仍通过 `pick_bond_set` 决定**成员资格**。<br>热路径内联最小哈希，**不**分配 `Vec<PeerConnId>` | 单测 `consistent_hashing_never_repoints_a_flow_to_an_existing_sibling`（含「纯重排 0 迁移」）；`positional_hashing_repoints_flows_on_reorder_or_count_change`（反例） | S4 / 本文 §9 |
| **P-AUDIT.10** | **政策否决透明化** | ✅ 已修（核心）。`avoid_relay_data` 的 peer 其出边 `cost += AVOID_RELAY_COST`，原先为 `i32::MAX`：既是**质量体系里的一票否决**，又让 `path_latency = cost % AVOID_RELAY_COST`（`:449`）**把自己的痕迹整除抹掉** → 状态页显示干净延迟，看不出路径被政策否决。<br>**改法**：① `AVOID_RELAY_COST` 由 `i32::MAX` 改为 **1e9**（约 1000× `OSPF_EDGE_COST_MAX`，仍是事实否决，但有限、且 `saturating_add` 不再溢出）；② `path_latency` 停止抹除惩罚，**如实上报**；③ Web `latencyMs()` 对 ≥ `POLICY_FORCED_PATH_COST` 的值渲染为「政策绕行 / Policy detour」而非 `1000000000ms`。<br>**未做**：proto 无「中间跳是否拒绝中转」字段（`Route.feature_flag` 只带**终点**的 flag），故无法在状态面标注**具体哪个中继**拒载 —— 需新增字段 + 跨语言 codegen，未做 | 单测 `avoid_relay_penalty_is_visible_in_reported_path_latency`（含「惩罚仍占主导 ⇒ 仍是事实否决」） | §9 政策轴 |
| P-AUDIT.7 | 拆连阈值 | ✅ 已修（**前提已复核推翻**）：曾以为「曾连通后掉线」的 peer 会进 `[…,30s,60s]` 退避尾段。实际 `launch_task` 每次 `tokio::spawn(try_direct_connect)`，`backoff_index` 是**该次调用的局部变量**（初始 0）；任务返回后被 `foundation/task.rs` 的 main loop 以 `is_finished()` 移除并重投 ⇒ **重连退避从 1s 重来，60s 尾段只在「单次调用内反复打洞失败」时累积**。<br>**实际杠杆是拆连阈值本身**：`ping_fail_close_count` 5 → **10**。理由：拆连后重建要 ~5s(任务循环)+2–6s(打洞)+5s(握手)，**重建代价远大于故障窗口**，所以为「丢几个包就拆」而付重建代价是亏的。每轮失败 ~3s（1s 间隔 + 2s 超时，失败后退避重置为 1s），10 轮 ≈ 30s 才判死，期间短暂抖动根本不会触发拆连。<br>**取舍**：真死链路的发现晚 ~15s，但重建两种情况都要 ~30s，**总恢复时间几乎不变** ⇒ 容忍度近乎免费。<br>**顺带修掉一个会漂移的隐患**：`peer_conn.rs` 里 `ping_fail_close_count` / `ping_interval_max_sec` 的 `0 → 5 / 32` 回落值原本是**硬编码**，与 `gen_default_flags()` 各自独立；现改为引用 `config::toml::DEFAULT_PING_FAIL_CLOSE_COUNT` / `DEFAULT_PING_INTERVAL_MAX_SEC` | 单测（阈值本身无分支可测；已在 `config::toml` 建常量并加注释）；`three_node.rs` 里依赖旧时间窗的集成测试超时已同步 15s→40s | S1 / §5.1 |
| **P-AUDIT.11** | **首包等待：L2 路由安装与 route-sync 会话启动改事件驱动** | ✅ 已修。缺陷：`ProxyCidrMonitor` 固定 `sleep(1s)` 后比对 `peer_info_last_update`，`maintain_sessions` 也固定 1s 重扫 interface peer 集。对端刚宣告子网时，本机装系统路由要等最多 1s；新 peer 建 route-sync 会话也要等最多 1s。这 1~2s 里发往内网设备的包在 `send_msg_by_ip` 查不到 dst 被**直接丢弃**（`no_dst_peer_drops`），只能靠应用层 SYN 1s/2s/4s 退避重试。<br>**改法**：① `PeerRouteServiceImpl` 新增 `peer_info_notify: Arc<Notify>`，在 `update_peer_info_last_update()` 里 `notify_one()`；`PeerRoute::peer_info_wake()` → `PeerManagerCore::proxy_cidr_wake()` 暴露给 reconciler。② `ProxyCidrMonitor` 改为 `select!(notified(), sleep(RECONCILE_TICK))`，**1s tick 保留为漏信号兜底**。③ `update_my_peer_info_routine` 在 `PeerAdded`/`PeerRemoved` 时 `session_mgr.sync_now("peer_set_changed")`，让 `maintain_sessions` 立即重算。<br>**为什么用 `notify_one` 不用 `notify_waiters`**：`notify_one` 在无人等待时**存一个 permit**，reconciler 忙时到达的唤醒不会被丢弃；`notify_waiters` 会丢。单测已固化该性质。<br>**边界**：conn 级事件（`PeerConnAdded`/`Removed`）**刻意不触发** —— 它们不改变 peer 集，且 bonding 会高频churn。`route_algo = none` 时 `proxy_cidr_wake()` 返回 `None`，reconciler 退回纯 1s 轮询。 | `cargo test -p easytier-core --lib` **1019 passed / 0 failed / 1 ignored**；新增 `wake_permit_is_observed_before_the_reconcile_tick`、`tick_still_fires_without_a_wake_signal`。**现网收益待验**（应表现为 `data_plane_no_dst_peer_drops` 在首次访问内网设备时下降） | §5.1 ①② |
| **P-AUDIT.12** | **首包被丢：无路由的包短暂暂存而非直接丢** | ✅ 已修（与 P-AUDIT.11 同一问题链的**兜底层**）。P-AUDIT.11 把「已连接 → 首包可通」压到百毫秒级，但**竞态窗口不可能归零**：路由同步 RPC 往返 + OS 装路由仍需时间，对端刚宣告子网的瞬间发来的包仍会查不到 dst。原行为是 `send_msg_by_ip` 直接 `no_dst_peer_drops.inc()` 丢弃，应用只能等自己的重传定时器（TCP 初始 RTO ~1s）。<br>**改法**：新增 `peers/pending_packets.rs`（`PendingPackets`，`Mutex<HashMap<IpAddr, VecDeque<Entry>>>` + `Notify`）。查不到路由的包**暂存最多 500ms**，由 flusher 任务（`PeerManagerCore::start_pending_packet_flush`，在 `start_components` / attached 路径各启动一次）在路由出现后按原样重发。<br>**容量有界**：每目标 4 包、全局 64 包；超限即退回**原来的计数丢弃**，所以 `data_plane_no_dst_peer_drops` 的语义不变。新增 `data_plane_no_dst_peer_buffered` 单独统计「被暂存」的量。<br>**不阻塞发送方**：`push` 只拿一次锁、不 await —— 这与 P-AUDIT.6 拒绝的「背压」是两件事。<br>**刻意不暂存的三类**：① 策略 `Block` 的包（安全边界，暂存等于软化它，且延后投递可能发出当时被禁止的流量）；② 广播/组播（空 fanout 是「本来就没人收」，不是缺路由）；③ 已过 TTL 的包（记为真丢）。<br>**`not_send_to_self` 随包保存**：重发时用原值，否则自投递包的 `not_send_to_tun`/`no_proxy` 循环防护标志会变（`tcp_proxy_service` 和 `packet_plane` 都有传 `false` 的调用方）。<br>**重发不重跑 NIC 流水线**：包已经过 `run_nic_packet_process_pipeline`，重跑会二次套用 ACL/DNS 过滤；因此拆出 `send_msg_by_ip_after_pipeline(..., allow_buffer)`，flusher 传 `allow_buffer=false` 防止再入队延长寿命。<br>**同目标保序**：`take_all` 按目标成组取出、组内保持入队顺序 ⇒ 同一目标的包不会乱序。 | `cargo test -p easytier-core --lib` **1026 passed / 0 failed / 1 ignored**（新增 7 个：取回、TTL 过期、每目标上限、全局上限、`put_back` 保留原截止时间、过期槽位不阻塞新包、`not_send_to_self` 往返）。**现网收益待验**（`no_dst_peer_buffered` 上升且 `no_dst_peer_drops` 下降即为生效） | §5.1 首包竞态 |
| **P-AUDIT.8** | ~~路由/Peer 残留 90s~61min~~ | **⚠️ 初版前提已被推翻，不做代码改动。** 原以为「对端掉线后状态页最长绿一小时（`REMOVE_DEAD_PEER_INFO_AFTER=3660s`）」。实测追踪后发现**不成立**，详见 §3 C7 | 单测 `stale_peer_survives_while_a_neighbour_still_claims_adjacency` 已把真实行为固化 | §3 C7 |
| P1.5 | 短时抖动宽限 | 短暂丢包不立即拆 conn（`peer_conn_ping.rs:327`）。无中继的组网里这是**从「可选优化」升为「必需」**的项 | 丢 1–2 包不触发拆连 | 已有 Roadmap |
| P-AUDIT.5 | TCP 回落产品提示 | ✅ 已修：状态页「协议」列在**所有隧道均为自带重传传输**（tcp/faketcp/quic/kcp）时加 ⚠ 并给 tooltip，说明 TCP-over-TCP 成因与排查方法。**混合集合不提示**（UDP 仍可走 ⇒ 提示只是噪音）。判定为纯函数 `allPathsRetransmit` | Web `vitest` 通过 | X2 |
| P-AUDIT.5b | 配置面默认值提示 | 未做：`ping_fail_close_count` 等在 GUI 无控件（`network.ts` 默认模板不含该字段），只能走 TOML/CLI。要暴露需新增 proto + UI 字段 | — | — |
| **P-AUDIT.10c** | OHRS / Android `path_latency` 大数值 | **核实为非问题**：该字段仅在 `easytier-ohrs` 的 API 类型之间透传（`napi_types.rs` / `runtime_state.rs`），**全仓无消费方**（Kotlin 侧 `pathLatency` 零命中；默认构造里一律 `None`）⇒ 没有可显示的 UI，不存在回归。记录在此以免后续加 UI 时踩坑 | — | `easytier-ohrs` |
| — | 打洞刚成时的代价空窗 | **已复核，暂不改**，详见 §3 C8 | — | `peer_map.rs` `best_edge_cost` |

### P2

| ID | 项 | 说明 |
|----|-----|------|
| P-AUDIT.6 | 队列背压 | 满时等待/降速，而非仅丢（吞吐 B2） |
| S3/PMTU | MTU 自适应或 MSS 钳制 | 稳定性审计 §6；出厂值已于 2026-10-16 对齐 1280 |
| P2.5 | KCP/QUIC 弱网产品化 | 既有 Roadmap；**出厂一律不启用**（见 §11） |
| — | `forward_counter > 7` 硬丢包 | 深 mesh 中继的天花板，是硬上限而非质量判断；要加深还是显式告警待定 |

---

## 9. 设计原则：选路 / 存活性 / 政策是三根正交的轴

用户诉求（`../roadmap/connection-stability-todo.md` §2.2，2026-10-06 拍板，本文 §9 复述）：

> 用户要的始终是**两设备之间低延迟、低抖动、低丢包**，**不区分直连还是中转**。

这条原则**只适用于选路轴**。代码里有两类东西不是选路，容易被它误伤：

| 轴 | 回答的问题 | 判据 | 是否受该原则约束 |
|----|-----------|------|-----------------|
| **选路** | 该走哪条路？ | **只看质量分** | ✅ 是，无例外 |
| **存活性 (liveness)** | 该多快放弃一条路？ | **当前是否存在替代路径** | ❌ 否 —— 无替代路径时拆 conn = 硬中断，与质量无关 |
| **政策 (policy)** | 管理员是否禁止某类路径？ | 管理员指令（可硬否决） | ❌ 否 —— 安全合规要求可强制绕路 |

**为什么 liveness 必须独立**：2 节点 mesh 拆掉唯一那条 conn 是**硬中断**；有中继时拆直连只是优雅降级。这是**存活性**问题，质量分回答不了。所以 P-AUDIT.7 的判据应是「**还有没有别的路**」，而不是「有没有中继」—— 这比按拓扑分类更准确。

**为什么 policy 仍需保留硬否决**：若安全策略要求「流量不得经第三方节点」，`disable_relay_data` 必须一票否决，哪怕绕路慢 10 倍。**问题不在于它存在**，而在于它①伪装成质量（混在边代价里）、②影响被 `% AVOID_RELAY_COST` 抹掉（排障看不到）。见 P-AUDIT.10。

### 9.1 「中继有时更快」是常态，不是边角情况

`latency_first=true` 后，`send_msg_internal` 取 OSPF least-cost 下一跳，若 ≠ 目标即走 `relay_peer_map`；Dijkstra 对边代价**求和**，故「两条干净的路」可压过「一条很烂的路」：

| 场景 | 直连 | 中继 | 胜者 |
|---|---|---|---|
| 同城 Wi-Fi 干扰重（丢包 5%、抖动 40ms） | RTT 20ms 但代价高 | A→R 15ms 光纤 + R→D 15ms 干净 | **中继** |
| UDP 打洞落在端口受限映射上，绕路诡异 | RTT 看着低、实际差 | 稳定 | **中继** |
| UDP 被限速/阻断 → 回落 TCP（TCP-over-TCP） | 高延迟 + 重传放大 | QUIC/KCP 中继 | **中继** |
| 打洞失败 | 无 | 有 | **中继**（唯一可用） |
| 同一交换域 | RTT 1ms | RTT 30ms | 直连 |

**两个必须一并记住的代价**：① 多一跳 RTT 是实打实的，`w_lat=1 / w_loss=4 / w_jitter=1` 意味着**丢包权重是延迟的 4 倍**——对 RDP 合理，对实时音频可能相反（可调 `conn_select_w_*`）；② `forward_counter > 7` 是**硬丢包**而非质量判断，深 mesh 里中继存在天花板。

---

## 10. 出厂默认值一致性（core vs GUI）

同一份配置从不同入口建出会得到**不同默认值**，是已确认的一类缺陷。逐字段比对（`gen_default_flags()` × `types/network.ts` 模板）：

| 字段 | core | GUI | 处置 |
|------|------|-----|------|
| `latency_first` | ~~`false`~~ → **`true`** | `true` | 2026-10-17 统一为质量优先 |
| `enable_kcp_proxy` | `false` | ~~`true`~~ → **`false`** | 2026-10-17 拍板「KCP 模式不开启」，**以 core 为准** |
| 其余 ~30 项 | — | — | 逐字段比对**一致**（`disable_encryption`/`enable_kcp_proxy` 之类为等价表达） |

> **约定**：`gen_default_flags()` 与 `types/network.ts` 的默认模板**必须逐字段一致**；任何一侧改动都要同步另一侧**并在两处留注释**。`PeerPolicyConfig::default()` 是第三个回落点（proto 字段缺失时用），也需一致。

---

## 11. 局限与未验证项

1. ~~静态读代码 + 常量核实；**未**现网 A/B Tailscale、未抓包确认路径 MTU / 代理劫持。~~ **2026-10-17 部分解除**：已完成三轮现网 ping 实测并**证实**代理劫持（§6.1 / §6.2，判定命令见该节）；**仍未做**的是抓包确认路径 MTU，以及与 Tailscale 的同条件 A/B。
2. **2026-10-17 的九项改动全部未做现网验收。** 其中数项改了数据面路径（选路策略 `latency_first`、保活间隔 S1b、转发闸门 P-AUDIT.4、bond 选流 P-AUDIT.9），建议至少在**两节点**与**三节点 mesh（含中继腿）**两种拓扑各验一轮 —— P-AUDIT.4 / P-AUDIT.9 的价值恰恰只在多节点时才体现。
3. `§5.1` 的失效链时间（② 32s→10s、③ ~10–15s、④ ≤5s、⑤ 2–13s）均为**按常量推算**。**2026-10-17 已实测到一个真实样本**：一次 71s 黑洞中，黑洞起点（`12:57:22`）早于 ET 察觉（`12:57:43`）**21s**，与「②≤10s + ③ 起步」的量级一致（该次仍跑在默认 32s 的旧构建上）。**②③ 的完整时间分布仍无统计样本。**
4. ~~`easytier` crate 在本环境无法构建~~ **2026-10-17 复验：`cargo check -p easytier` 通过**（前提是注入 MSVC 环境变量 PATH/INCLUDE/LIB 并把 `CARGO_TARGET_DIR` 指到可写目录；旧结论「缺原生工具链」是误判，工具链本机就有）。仍**未跑**的是 `cargo test -p easytier`（CLI / `three_node.rs` 等集成测试）⇒ §8 的 CLI 展示改动**已通过编译**、但集成测试未回归。
5. 未覆盖 Android VPN 生命周期（见 Archive 安卓审计）；OHRS 侧 `path_latency` **核实无消费方**（P-AUDIT.10c），故无需处理。
6. QUIC/WG/faketcp 直连细节未逐 scheme 复查。
7. **本文有两条初版结论已被实测推翻**（§3 C7「状态页绿一小时」、§5.1「纯 P2P 无中继」）。两者的共同教训写在了 §3 C7 末尾。
7b. **2026-10-17 第三条被推翻的初版结论**：「残留偶发丢包 = bond / conn_select 切出口」。实测证明 ET 控制面静止时丢包照样出现，**且尖峰与本机代理无关**（§5.1 修正小节）。**「连接中途偶发丢包 / 200–500ms 尖峰」目前仍未定性**，落点需双侧同时抓包才能判定；证据、方法论、重测清单见 [`packet-loss-investigation-2026-10.md`](./packet-loss-investigation-2026-10.md)。
8. **本轮明确不做的三项及理由**（避免下次重复评估）：
   - **P-AUDIT.6 队列背压**：把「环满即丢」改成「阻塞等待」会在数据面引入**背压链**——上游 TUN 读阻塞 → 转发协程阻塞 → 反压到 PeerConn 发送队列 → 可能与 `select!`/定时器交互产生**死锁或活锁**，且无法用单测可靠覆盖。这属于架构级改动，需要压测与故障注入，不是可以在无现网环境里安全落地的项。
   - **PMTU / MSS 钳制**：需要路径探测（试探 + 缓存），涉及定时器、缓存失效、与多路径的交互；出厂 MTU 已对齐 1280，属**剩余风险**而非已知故障。
   - **P-AUDIT.8b / 10b（proto 新字段）**：需要新增 proto 字段 + 跨语言 codegen（Rust / TS / Kotlin），并经现网验收；属协议层改动，与「修 bug」不同量级，故暂不做。~~编译无法验证~~ 已不成立（`cargo check -p easytier` 可跑）。

---

## 12. Changelog

| 日期 | 变更 |
|------|------|
| 2026-10-16 | 首版：C1–C6 + S1–S5 分栏 + 冲突矩阵 + P-AUDIT 修复清单 |
| 2026-10-16 | 出厂 `flags.mtu` 1380→**1300**；Config UI 中/英帮助与占位；安卓 `resolveVpnMtu` 回退；Go TUN 示例 MTU **1280** |
| 2026-10-17 | P-AUDIT.1–.3：热路径 fuse 失效 + 无下一跳/NIC 满/BufferFull 计数（含中继转发腿）；每包配置解析改为借快照，去掉 `FlagsInConfig` 深拷贝（待现网） |
| 2026-10-17 | **`latency_first` 出厂默认 `false` → `true`**（core `toml.rs` / `PeerPolicyConfig` 与 GUI 模板统一）。此前 TOML/CLI 部署拿到 LeastHop：有直连时中继腿永不参与竞争；GUI 部署一直是 LeastCost，两种入口行为相反 |
| 2026-10-17 | **S1b 修复：UDP 打洞 conn 控制 ping 上限 32s → 10s**（TCP 打洞 1s 保持）。per-conn 上限取 `min(cap, ping_interval_max_sec)`，只收紧不放宽；直连 conn 不变 |
| 2026-10-17 | **P-AUDIT.4 修复**：`PeerMap::has_peer_with_live_conn`；`send_msg_internal` 两处网关闸门 + 直连分支改用它。此前已死的直连/网关条目会先匹配上分支并导致失败，不会落到可用的中继腿（= C3/C6，也是「本该中继更快却没走成」） |
| 2026-10-17 | **P-AUDIT.9 修复**：Bond 流→成员由 `key % member_count`（按位置）改为**一致哈希**（按 conn_id 取最小）；纯重排导致 0 个流迁移。**保留**按哈希分摊（不强制最优成员，不降 `bond_count`） |
| 2026-10-17 | **P-AUDIT.10 修复**：`AVOID_RELAY_COST` `i32::MAX`→`1e9` + `saturating_add`；`path_latency` 不再 `% AVOID_RELAY_COST` 抹掉惩罚；Web 与 CLI 渲染为「政策绕行 / policy detour」 |
| 2026-10-17 | GUI `enable_kcp_proxy` `true`→`false` 与 core 对齐（拍板「KCP 模式不开启」） |
| 2026-10-17 | **§3 C7：推翻「状态页绿一小时」**（实测收敛 ~1s，`REMOVE_*` 只管 peer_info GC）；**§5.1：推翻「纯 P2P 无中继」**（`disable_relay_data=false` 默认下任意节点可中继）。两条初版结论均以单测固化真实行为 |
| 2026-10-17 | 修正 `p2p-direct-stability-audit.md` 摘要表 S3 的陈旧 MTU 数值（1360/1432 → 1280/1352） |
| 2026-10-17 | **P-AUDIT.5**：状态页「协议」列在全部隧道为自带重传传输时给 ⚠ + TCP-over-TCP 说明（混合集合不提示） |
| 2026-10-17 | **P-AUDIT.7**：`ping_fail_close_count` 默认 5 → **10**（拆连判定 ~15s → ~30s，短暂抖动不再触发拆-建）；把 `peer_conn.rs` 里硬编码的 `5`/`32` 回落值改为引用 `config::toml` 常量，消除与 `gen_default_flags()` 的漂移隐患；同步 `three_node.rs` 中依赖旧时间窗的测试超时 |
| 2026-10-17 | **P-AUDIT.10c 核实为非问题**：OHRS/Android 侧 `path_latency` 全仓无消费方，无需处理 |
| 2026-10-17 | **新增 [`packet-loss-investigation-2026-10.md`](./packet-loss-investigation-2026-10.md)**：现网三轮 ping 实测的完整证据记录（方法学 / 原始数据 / 已证伪假设 / 重测清单） |
| 2026-10-17 | **§5.1 新增实测修正小节**：推翻「偶发丢包 = bond / conn_select 切出口」——ET 控制面全程静默时丢包与 200–500ms 尖峰照样出现；该现象**仍未定性**，与 §5.1 的「黑洞失效链」是两条独立的链 |
| 2026-10-17 | **§6 重写**：① 本机代理抢 ET 外层流量由「推断」升级为**已证实**（附 `Find-NetRoute` 判定命令）；② 新增**反向劫持**——ET 为对端发布的代理子网装 23 条 metric 9000 路由，反过来吃掉本机代理的出口 IP，源头是某节点的 `proxy_networks`；③ 新增 `find_interface_index` 失败经四层 `?` 上抛、导致隧道/打洞 socket 批量创建失败的**代码缺陷** |
| 2026-10-17 | **§7 排障表**改为「`ping` + 日志时间对齐优先」，并补入成簇拆连与代理子网路由两项检查；**§11** 记录已实测样本与仍未定性的部分 |
| 2026-10-18 | **P-AUDIT.11**：L2 路由安装与 route-sync 会话启动改**事件驱动**（`peer_info_notify` + `notify_one` → `ProxyCidrMonitor::select!`；`PeerAdded/Removed` → `session_mgr.sync_now`）。1s tick 保留为漏信号兜底。目标是把「已连接 → 首包可通」从 1~3s 压到百毫秒级；`cargo test -p easytier-core --lib` 1019 passed / 0 failed |
| 2026-10-18 | **P-AUDIT.12**：无路由的包改**短暂暂存后重发**（`PendingPackets`：TTL 500ms / 每目标 4 包 / 全局 64 包，超限退回原计数丢弃），新增指标 `data_plane_no_dst_peer_buffered`。策略 `Block`、广播/组播、已过期三类不暂存；`not_send_to_self` 随包保存；重发不重跑 NIC 流水线。`cargo test -p easytier-core --lib` **1026 passed / 0 failed**（新增 7 个单测） |
| 2026-10-18 | **P-AUDIT.12 打磨**（实现细节，收益仍待现网）：flusher 挂 `peer_info_wake` + 20ms tick；`PushOutcome.dropped` 把 per-dst 挤出/过期 reclaim 计入 `no_dst_peer_drops`；relay 字段改名 `handshake_pending_packets` 并与 `PendingPackets` 互指。同期 **L2 宿主对齐**：Android GUI / OHOS 新 core 吃 `desired_proxy_cidrs`（与 JNI 一致），旧 core 回退拼装 |
