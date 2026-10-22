# 延迟测量与展示（现状审查）

## Status

- Status: **Current**（审查文档；文中缺陷与重构顺序为结论，**非**已落地行为承诺）
- 最近审阅：2026-10-22（展示侧 §5 对齐改动**已改代码、待验收**，见专题 §5；Q5/§3.9；文档核对）；2026-10-20；2026-10-19（首次成文）
- 范围：系统内**所有**产生延迟数值的地方，以及前端对这些数值的展示口径
- 基线：`git HEAD = 1a69bc4c`（与专题下钻一致；原基线 `51ef4424` 为其祖先）；关联选路修复见 [`routing-bug-audit-2026-10.md`](./routing-bug-audit-2026-10.md)（**已合入** `main`/`dev`，⚠️ **待验收**）
- 关联：[`peer-connections.md`](./peer-connections.md)（default_conn 与多链路）、[`traffic-steering.md`](./traffic-steering.md)（出口与子网代理）、[`../roadmap/gui-connected-vs-reachable.md`](../roadmap/gui-connected-vs-reachable.md)（VIP 可达探测的设计动机）、[`latency-scope-and-relay-visibility.md`](./latency-scope-and-relay-visibility.md)（**专题下钻**；**名义 vs 实际 vs 对齐**见该文 **Q5 / §3.9**；展示侧修法以该文 §5 为准）
- 索引：[`../README.md`](../README.md)

本文只描述 **代码今天做什么**，以及已核实的展示口径缺陷。两列「名义含义 / 是否与显示一致 / 如何对齐」的产品口径以专题下钻为准。

---

## 1. 结论先行

1. **系统里真正独立「测」延迟的只有 2 处**：控制面 conn 级 RTT、数据面 VIP 级 RTT。其余全是这两者的派生、旁路或落库副本。
2. **两列名义不同，直连上不是冗余**：延迟 = 链路/路径时延；VIP = 数据面通不通 + 业务往返。测的是不同的腿，**不应合并测量**。详见专题 **Q5 / §3.9**。
3. **真正的问题是「延迟」列一列塞了两种口径**：直连行显示实测 RTT（与名义基本一致），中转行显示 OSPF 路径**代价**却标成 `ms`（与名义**不一致**）。详见专题 §3.7 / §3.9。
4. 另有两处口径不一致：`latencyMs()` / `connLatencyMs()` 两套并行实现（仍在）；前端无条件优先 `path_latency_latency_first`（**已修，待验收**：`resolveRouteTableKind` / `pathLatencyCost`，含 `p2p_only`）。
5. **对齐目标（前端已改、待验收）**：列名说什么，格子里就是什么——中转延迟去掉代价冒充；VIP 加强「业务往返」口径；直连仍保留两量；中转若延迟列改用 VIP 实测则勿双显同一数字。详见专题 §5。

---

## 2. 延迟测量点全量盘点

### 2.1 真正的独立测量（2 处）

| # | 位置 | 测什么 | 机制 | 周期 | 输出字段 |
|---|---|---|---|---|---|
| **①** | `easytier-core/src/peers/conn/peer_conn_ping.rs` | 单条 `PeerConn` 的**控制面** RTT | 控制通道上的 Ping/Pong，`WindowLatency` 15 样本滑动窗口 | 随 ping 间隔（`config/toml.rs:139` `DEFAULT_PING_INTERVAL_MAX_SEC = 10`） | `PeerConnStats.latency_us`（+ `jitter_us`、`loss_rate`） |
| **②** | `easytier-core/src/instance/vip_probe.rs` | 到对端 **VIP** 的**数据面**端到端 RTT | 手工构造 ICMPv4 Echo 注入数据面，经 peer 包管线回包匹配 | 20s | `PeerVipReachability.rtt_ms` |

两者**完全独立**：① 走 Noise 加密的控制通道，只覆盖一跳隧道；② 走数据面，覆盖完整 mesh 转发路径（含所有中转跳）。多跳场景下 ② ≥ ① 是正常现象。

> ② 是**纯展示**的。`vip_probe.rs` 明确规定：探测结果不参与选路、不触发拆链、不计入流量统计、不参与 `recent_traffic` 启发式。**这条不变量很重要，见 §6。**

### 2.2 派生链（不是新测量）

```
① latency_us / jitter_us / loss_rate
        │
        ▼  conn_select.rs  conn_quality_score()
   w_lat·(RTT/1e6) + w_loss·loss + w_jitter·(jitter/1e6)
        │
        ▼  conn_select.rs  ospf_edge_cost_from_score()
   score×1000, clamp(1, 1_000_000), fused 时 +10_000
        │
        ▼  peer_map.rs  best_edge_cost()
   未测量（quality_score == 0，即还没收到第一个 pong）的 conn ⇒ OSPF_EDGE_COST_UNMEASURED(500)
   而不是下界 1 —— "没测过"不是"完美"，1 会让它成为全图最便宜边权
        │
        ▼  peer_ospf_route.rs  Dijkstra 沿路径累加
   path_latency（两套路由表：route_table / route_table_with_cost）
```

| # | 位置 | 是什么 |
|---|---|---|
| ③ | `peers/conn/conn_select.rs`（`conn_quality_score`） | ①的加权变换 |
| ④ | `peers/conn/conn_select.rs`（`ospf_edge_cost_from_score`） | ③的整数编码（`OSPF_EDGE_COST_MAX = 1_000_000`） |
| ④b | `peers/conn/peer_map.rs`（`best_edge_cost`） | 未测量 conn 的代价抬到 500（`OSPF_EDGE_COST_UNMEASURED`） |
| ⑤ | `peers/route/peer_ospf_route.rs` | ④沿路径累加；含 `AVOID_RELAY_COST = 1_000_000_000` 的策略绕行惩罚 |

**量纲提示**：③ 内部把 µs 除以 `1e6` 得到「秒」，④ 再 `×1000` 变回「毫秒当量」。所以 ⑤ 的 `path_latency` **读起来像毫秒，但不是实测毫秒**——它含丢包与抖动加权，且是**路径累加**。

### 2.3 旁路与落库

| # | 位置 | 说明 |
|---|---|---|
| ⑥ | `connectivity/stun/client.rs:313` | 到 STUN 服务器的 RTT。**只服务 NAT 打洞决策**，不进 peer 表，不面向用户 |
| ⑦ | `easytier-web/src/peer_history.rs:85` | 采样 ① 的 `latency_us` 进 SQLite；REST 侧按桶 `AVG`（`restful/peer_history.rs:73`） |

**汇总**：2 个测量点 + 1 条派生链（①→③→④→⑤）+ 1 个旁路（⑥）+ 1 个落库副本（⑦）。

---

## 3. 前端展示现状

### 3.1 peer 表的两列

表格是 **per-conn** 的：`statusDisplay.ts::flattenPeerConnRows` 把每个直连隧道拆成独立行；**中转 peer 没有 conn**，作为单行存在（`conn: undefined`）。

| 列 | 渲染点 | 表头（cn / en） | 字段 | 口径 |
|---|---|---|---|---|
| **延迟** | `Status.vue` 延迟列 / `rowLatency` | 延迟 / Latency | `conn.stats.latency_us`（直连） | ① 控制面单跳 RTT |
| **VIP可达** | `Status.vue` VIP 列 → `statusDisplay.ts::vipStateLabel` | VIP可达 / VIP | `PeerVipReachability.rtt_ms` | ② 数据面端到端 RTT |

**这两列不是冗余，不应合并。** `docs/roadmap/gui-connected-vs-reachable.md` 明确要求「与延迟、丢包、代价列分开；不合并成单一健康度灯」。

VIP 列取值（`vipStateLabel`）：**改前**可达只显示 `7ms`；**前端已改（待验收）**显示 `≈7ms` / `≈<1ms`（专题 §5 序 1–2）。不可达/未探测/未知各走独立状态词。

### 3.2 「延迟」列的双口径

```ts
// Status.vue::rowLatency — 改前示意；改后（待验收）中转优先 VIP，见专题 §5 序 3
function rowLatency(row: PeerConnTableRow) {
  if (row.conn)
    return connLatencyMs(row.conn)                                  // 直连：① 的实测 RTT
  return latencyMs(row.pair, { /* vipRttMs / pathCostLabel / policyForced */ })
}
```

| 行类型 | 改前显示 | 改前含义 | 改后（待验收） |
|---|---|---|---|
| 直连 peer | `5ms` | ① 实测 RTT（`ceil`） | 不变 |
| 中转 peer | `12ms` | ⑤ **路径代价**冒充 `ms` | VIP `≈Nms`，否则灰字「路径代价」/「政策绕行」 |

改前两者同列同后缀，**用户无法区分**。修法以专题 §5 为准。

---

## 4. 已核实的缺陷

### P0 — 「延迟」列一列两种口径

见 §3.2。纯前端可修，风险最低。

### P1 — `latencyMs()` 与 `connLatencyMs()` 两套并行实现

`statusDisplay.ts::latencyMs` 与 `::connLatencyMs` 仍并行：有 conn 的行走后者；中转行走前者（现含 VIP / 灰字代价，待验收）。同概念两处实现，回退规则仍分裂（专题未把合并实现列入本批）。

### P1 — 前端无条件优先 `path_latency_latency_first`

**改前**：

```ts
const pathLatency = numericValue(info.route?.path_latency_latency_first)
  ?? numericValue(info.route?.path_latency)
```

**前端已改（待验收）**：`pathLatencyCost(info, routeTableKind)` / `resolveRouteTableKind(latencyFirst, { p2pOnly })`，与 core `latency_first && !p2p_only`（`peers/context.rs`）对齐。

但数据面实际用哪套路由表，仍取决于运行时策略：

- `peers/peer_manager.rs:199` `get_next_hop_policy(is_latency_first)` → `NextHopPolicy::LeastCost` / `LeastHop`
- `peers/route/peer_ospf_route.rs:5331` `get_next_hop_with_policy()` 按 policy 选表

**`latency_first = false` 时数据面走 default 表，GUI 却显示 least-cost 表的路径延迟——显示的不是实际在用的路径。**

**补审补充（2026-10-19，详见 `routing-bug-audit-2026-10.md` B15/B16）**：这条"显示的不是在用的路"比原文更严重——实际选哪张表**在单跳内都不由本地配置唯一决定**：

1. **转发节点按包头的位选表**（B16）。`is_latency_first` 是**源节点**按自己的 `context.packet_policy()` 盖上去的（`peers/peer_manager.rs` 多处 `hdr.set_latency_first(...)`），转发侧读 `is_latency_first()` 决定查哪张表 ⇒ 本地 `latency_first` 配置**对过境流量不具权威性**。一台故意配 `latency_first = false` 的节点仍会按上游的意图做 least-cost 选路。
2. **深路径会静默降级**（B15）。`forward_counter > 2` 时强制 `set_latency_first(false)` ⇒ latency-first 只在浅路径生效，更深路径退回 least-hop。
3. ⇒ GUI 若只按"本实例的 `latency_first` 配置"或"无条件优先 least-cost 字段"去显示，**在过境流量和深路径上都可能与实际不一致**。修 P1 时不能只比较本地开关，需要后端在 `Route` 上明确标注"本次发布来自哪张表"。

顺带：`latency_first` 出厂为 `true`（`config/toml.rs:168`），所以上述不一致是**默认配置下**的常态，而非边缘场景。

### P2 — `latency_ms` 字段名与语义不符

```proto
// easytier-proto/proto/peer_rpc.proto:289
message DirectConnectedPeerInfo { int32 latency_ms = 1; }
```

值早已是 `ospf_edge_cost_from_score(...)`，即**质量代价**而非毫秒。`peer_map.rs:752` 自己承认："The field remains named `latency_ms` for wire compatibility."

且该值是**对端上报的**——`peer_ospf_route.rs:107-145`（注释 `:107-123` + 实现 `edge_cost_from_i32` `:124-145`）既做负值下钳（`MIN_EDGE_COST`），也按 `OSPF_EDGE_COST_MAX` **上钳**（[`routing-bug-audit-2026-10.md`](./routing-bug-audit-2026-10.md) **B8 ✅ 已修**）。**残留问题是字段名/语义不符**（代价仍叫 `latency_ms`），不是钳制缺失。

同类问题：`api_instance.proto` 的 `Route.path_latency` 同样是代价而非时延。

### P2 — VIP 列失败时单元格信息量偏低

`vipStateLabel` 在 reachable 时显示 RTT（现为 `≈Nms`，待验收）；unreachable 的**单元格**只有「不可达」二字（无耗时）。tooltip 侧已有 `error` 与 `vip_boundary_tip`；缺口是**格内**无可扫读的失败细节。见专题 P0-1 / P1-1。

---

## 5. 建议的重构顺序

> **展示侧（名义对齐、中转行双口径、VIP 口径提示、两表统一等）以 [`latency-scope-and-relay-visibility.md`](./latency-scope-and-relay-visibility.md) §3.9 + §5 为权威**（排期 1→2→3→11→4→5→7）。下表保留本文件的测量/协议层顺序，避免与专题下钻分叉。

| 序 | 改动 | 范围 | 风险 |
|---|---|---|---|
| 1 | **中转行「延迟」列**：不再把 `path_latency` 渲染成实测 `Nms`。**首选**改显该 peer 的 VIP 实测 RTT（专题 §3.7.4 B）；退路为灰字「路径代价」或不显示数字（A）。详见专题 §5 序 3 | 纯前端 | 低 |
| 2 | `latencyMs` / `connLatencyMs` 合并为一个带显式 mode 的函数，回退链只留一份 | 纯前端 | 低 |
| 3 | 后端按当前 `NextHopPolicy` 只发布实际在用的那份 `path_latency`（或把 policy 状态告诉前端）；纯前端过渡方案见专题 §5 序 11 | core + 前端 | 中 |
| 3b | 补上"**这份代价来自哪张表**"的标注：转发侧按**包头位**选表（`routing-bug-audit-2026-10.md` B16），深路径还会被 `forward_counter > 2` 静默降级（B15）⇒ 只按本地 `latency_first` 开关判断不够 | core + 前端 | 中 |
| 4 | `latency_ms` → `edge_cost`、`Route.path_latency` → `path_cost`（B8 上钳已落地；本项只改名/语义） | proto + core + web | 中（`peer_rpc.proto` 是 peer↔peer 内部协议） |

---

## 6. 附：被撤回的「出口端到端探测」及其不变量

2026-10-19 前，仓库里存在一套「出口节点端到端质量探测」实验（锚点 `1.1.1.1` / `8.8.8.8`，每 60s 一轮、经每个候选出口发真实公网 ICMP、把 RTT 喂给 `select_by = quality`）。**该实验已整体撤回**（`exit_probe.rs`、proto 字段、`disable_exit_egress_probe` 开关、roadmap 文档均已删除，工作区无残留）。

撤回理由：复杂度与稳定性收益不成比例。

**保留的不变量**（写进后续设计时应遵守）：

> **探针不喂路由、不触发拆链、不计入流量统计。**

`vip_probe.rs:25-32` 把「display-only」写进契约；`:32-47` 补充为何不能当 keepalive（探针与用户流量争抢 conn 发送环、丢包会被计入路径、idle 探针流量会扰动 lazy-p2p）。被撤回的实验恰恰违反了它——把 `anchor_rtt_ms` 塞进了 `select_by = quality` 的选路输入。

如果将来仍需要「到某个真实目标的时延」，建议：

- **不要新增周期性探针**——② 的 VIP 探测已经是真实端到端 RTT
- 真要做，做成**用户手动触发的一次性诊断**，不周期运行、不进任何选路路径

这样复杂度是 O(1) 而非 O(出口数 × 锚点数)，也不会有探针流量扰动选路。

### 6.1 交叉引用：这个缺口是真的（`routing-bug-audit-2026-10.md` B17）

被撤回的实验之所以想存在，是因为**当前协议里确实没有"出口节点 → 网格外目标"的距离**：

- `peer_manager.rs`（`peer_path_cost`）= **到该出口的整条路径**的代价（OSPF least-cost 表的 `path_latency`，**含中转段**）；直连边代价（`peer_edge_cost`）只作为路由表尚未收敛时的兜底；
- `select_by = quality` 用上面这个值排序；
- `exit_nodes` 配置、`select_by = priority/balance` 连第一跳都不比；
- mesh peer 的路由信息里**没有**"我到某个网外目标有多远"这个量（`proxy_cidrs` 只声明"我代理这段地址"，不带代价）。

⇒ **"选离目标更近的那个出口"这件事当前做不到**。被撤回的出口端到端探测是三条候选路径之一（另两条：让出口节点把到网外目标的代价上报进路由信息、或把出口语义降级为明确的配置优先级）。详见 `routing-bug-audit-2026-10.md` §1e.3 / §1e.4。

**这条对本文件的意义**：中转行若继续渲染 `path_latency`，**该数字永远不可能等于「到目标的真实时延」**（即便 B1 修好也只覆盖到出口第一跳）——这是**协议对「网外距离」的能力边界**。但「中转 peer 的端到端实测」并非没有：relay-only 的 VIP 探测已经给出（专题 §3.2 / §3.7.4 **B**）。因此正确动作是**停止把代价当 RTT 展示**，改用已有 VIP 实测或灰字标明代价，而不是指望 `path_latency` 自己变成真实时延。

---

## 7. 核对方法

本文结论均可按以下方式复核（只读）：

| 结论 | 复核点（优先符号，避免行号腐化） |
|---|---|
| 测量点只有 2 处 | `peers/conn/peer_conn_ping.rs`（①）、`instance/vip_probe.rs`（②） |
| 派生链 | `conn_select.rs`（`conn_quality_score` / `ospf_edge_cost_from_score`）、`peer_map.rs`（`best_edge_cost`）→ `peer_ospf_route.rs` |
| STUN 不面向用户 | `connectivity/stun/client.rs` |
| 前端两列 | `Status.vue` 延迟列 / VIP 列；`statusDisplay.ts::vipStateLabel` |
| 延迟列双口径 | `Status.vue::rowLatency`；中转修法见专题 §5 序 3（待验收） |
| 回退链分裂 | `statusDisplay.ts::latencyMs`、`::connLatencyMs` |
| 路由表二选一 | `peer_manager.rs::get_next_hop_policy` → `peer_ospf_route.rs::get_next_hop_with_policy`；UI `resolveRouteTableKind`（含 `p2p_only`） |
| `latency_ms` 语义 | `peer_rpc.proto` `DirectConnectedPeerInfo.latency_ms`、`peer_map.rs`；上钳 `edge_cost_from_i32`（B8） |
| 撤回无残留 | 代码/proto 内 grep `exit_probe` / `exit_egress` / `EgressCostSource` / `measured_egress` / `disable_exit_egress_probe` / `anchor_rtt` → **0 命中**（排除 `*.md`） |

## 8. changelog

| 日期 | 变更 |
|---|---|
| 2026-10-22 | 文档核对：基线升至 `1a69bc4c`；B8 上钳改为「已修」；§5 展示侧对齐专题下钻 B 方案；刷新 §3.1/§7 行号与不变量引用；§6.1 澄清「`path_latency` 非真实时延」≠「中转无实测」 |
| 2026-10-22 | §1 增补：两列名义契约与对齐目标指向专题 **Q5 / §3.9**（名义 vs 实际 vs 能否一致；直连两量 / 中转勿双显） |
| 2026-10-22 | 展示侧前端对齐（专题 §5 序 1–5/7/11）**已改、待验收**；本文件「今天做什么」待验收后与专题一并改写 |
| 2026-10-22 | 文档欠账：§3.1/3.2/§4/§7 改符号引用并标注改前 vs 待验收；记 `p2p_only` 与 `resolveRouteTableKind` |
