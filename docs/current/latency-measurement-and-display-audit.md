# 延迟测量与展示（现状审查）

## Status

- Status: **Current**（审查文档；文中缺陷与重构顺序为结论，**非**已落地行为承诺）
- 最近审阅：2026-10-20（基线说明与 Current 门槛对齐）；2026-10-19（首次成文：全量盘点延迟测量点、厘清「延迟」列双口径、给出重构顺序）
- 范围：系统内**所有**产生延迟数值的地方，以及前端对这些数值的展示口径
- 基线：`git HEAD = 51ef4424`；关联选路修复见 [`routing-bug-audit-2026-10.md`](./routing-bug-audit-2026-10.md)（⚠️ 待合入 / 待验收）；前端 `vitest` 28 文件 / 241 用例全过
- 关联：[`peer-connections.md`](./peer-connections.md)（default_conn 与多链路）、[`traffic-steering.md`](./traffic-steering.md)（出口与子网代理）、[`../roadmap/gui-connected-vs-reachable.md`](../roadmap/gui-connected-vs-reachable.md)（VIP 可达探测的设计动机）
- 索引：[`../README.md`](../README.md)

本文只描述 **代码今天做什么**，以及已核实的展示口径缺陷。

---

## 1. 结论先行

1. **系统里真正独立「测」延迟的只有 2 处**：控制面 conn 级 RTT、数据面 VIP 级 RTT。其余全是这两者的派生、旁路或落库副本。
2. **前端 peer 表上的「两个延迟」不是冗余**：一个测控制面单跳隧道，一个测数据面完整 mesh 路径，测的是不同的腿，**不应合并**。
3. **真正的问题是「延迟」列一列塞了两种口径**：直连行显示实测 RTT，中转行显示 OSPF 路径**代价**（含丢包/抖动加权，不是毫秒），两者同列、同单位后缀、同样式，用户无法区分。
4. 另有两处口径不一致：`latencyMs()` / `connLatencyMs()` 两套并行实现；前端无条件优先 `path_latency_latency_first`，而数据面实际用哪套路由表取决于运行时的 `latency_first` 开关。

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

表格是 **per-conn** 的：`statusDisplay.ts:530` `flattenPeerConnRows` 把每个直连隧道拆成独立行；**中转 peer 没有 conn**，作为单行存在（`conn: undefined`）。

| 列 | 渲染点 | 表头（cn / en） | 字段 | 口径 |
|---|---|---|---|---|
| **延迟** | `components/Status.vue:978` | 延迟 / Latency | `conn.stats.latency_us` | ① 控制面单跳 RTT |
| **VIP可达** | `components/Status.vue:916` → `statusDisplay.ts:735` `vipStateLabel` | VIP可达 / VIP | `PeerVipReachability.rtt_ms` | ② 数据面端到端 RTT |

**这两列不是冗余，不应合并。** `docs/roadmap/gui-connected-vs-reachable.md:98` 明确要求「与延迟、丢包、代价列分开；不合并成单一健康度灯」。

VIP 列的取值规则（`statusDisplay.ts:743`）：**可达时只显示 `rtt_ms` 本身**（如 `7ms`），不加「可达」二字——数字本身就是数据面已应答的证据；不可达/未探测/未知各走独立状态词。

### 3.2 「延迟」列的双口径

```ts
// easytier-web/frontend-lib/src/components/Status.vue:106
function rowLatency(row: PeerConnTableRow) {
  if (row.conn)
    return connLatencyMs(row.conn)                                  // 直连：① 的实测 RTT
  // 中转无直连：回退到路由汇总延迟
  return latencyMs(row.pair, { policyForcedLabel: t('path_latency_policy_forced') })  // 中转：⑤ 的路径代价
}
```

| 行类型 | 显示值 | 真实含义 |
|---|---|---|
| 直连 peer | `5ms` | ① 实测 RTT（µs→ms，`Math.ceil`） |
| 中转 peer | `12ms` | ⑤ **OSPF 路径代价累加**（RTT+丢包+抖动加权 ×1000），**不是毫秒** |

两者同列、同单位后缀、同样式，**用户无法区分**。中转行那个数字的量纲是「代价」而非「时延」，基本不可解释。

---

## 4. 已核实的缺陷

### P0 — 「延迟」列一列两种口径

见 §3.2。纯前端可修，风险最低。

### P1 — `latencyMs()` 与 `connLatencyMs()` 两套并行实现

`statusDisplay.ts:78` 的 `latencyMs()` 有 4 级回退：

1. default conn 的 `latency_us`
2. 所有 conn 中 `latency_us` 的最小值
3. `route.path_latency_latency_first ?? route.path_latency`
4. ≥ `POLICY_FORCED_PATH_COST`（1e9）→ 显示「政策绕行」标签而非天文数字

但 `Status.vue:106` 对**有 conn 的行绕开它**，直接调更窄的 `connLatencyMs()`（`statusDisplay.ts:570`，只有第 1/2 级）。同概念两处实现，回退规则分裂。

### P1 — 前端无条件优先 `path_latency_latency_first`

```ts
// statusDisplay.ts:100
const pathLatency = numericValue(info.route?.path_latency_latency_first)
  ?? numericValue(info.route?.path_latency)
```

但数据面实际用哪套路由表，取决于运行时 `latency_first` 开关：

- `peers/peer_manager.rs:197` `get_next_hop_policy(is_latency_first)` → `NextHopPolicy::LeastCost` / `LeastHop`
- `peers/route/peer_ospf_route.rs:5230` `get_next_hop_with_policy()` 按 policy 选表

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

值早已是 `ospf_edge_cost_from_score(...)`，即**质量代价**而非毫秒。`peer_map.rs:665` 自己承认："The field remains named `latency_ms` for wire compatibility."

且该值是**对端上报的**——`peer_ospf_route.rs:107-133`（注释 `:107-123` + 实现 `edge_cost_from_i32` `:124-133`）专门处理过恶意负值上报（钳到 `MIN_EDGE_COST`）；但**上钳缺失**，详见 `routing-bug-audit-2026-10.md` B8。

同类问题：`api_instance.proto` 的 `Route.path_latency` 同样是代价而非时延。

### P2 — VIP 列失败时信息量为 0

`statusDisplay.ts:743` 只在 reachable 时显示 `rtt_ms`；unreachable 只有「不可达」二字，没有耗时/超时信息。控制面那侧有 `loss_rate` 列，数据面这侧没有对应物。

---

## 5. 建议的重构顺序

| 序 | 改动 | 范围 | 风险 |
|---|---|---|---|
| 1 | 给中转行的数值加 tooltip「路径代价，非实测 RTT」；或中转行不显示该数字 | 纯前端 | 极低 |
| 2 | `latencyMs` / `connLatencyMs` 合并为一个带显式 mode 的函数，回退链只留一份 | 纯前端 | 低 |
| 3 | 后端按当前 `NextHopPolicy` 只发布实际在用的那份 `path_latency`（或把 policy 状态告诉前端） | core + 前端 | 中 |
| 3b | 补上"**这份代价来自哪张表**"的标注：转发侧按**包头位**选表（`routing-bug-audit-2026-10.md` B16），深路径还会被 `forward_counter > 2` 静默降级（B15）⇒ 只按本地 `latency_first` 开关判断不够 | core + 前端 | 中 |
| 4 | `latency_ms` → `edge_cost`、`Route.path_latency` → `path_cost` | proto + core + web | 中（`peer_rpc.proto` 是 peer↔peer 内部协议） |

---

## 6. 附：被撤回的「出口端到端探测」及其不变量

2026-10-19 前，仓库里存在一套「出口节点端到端质量探测」实验（锚点 `1.1.1.1` / `8.8.8.8`，每 60s 一轮、经每个候选出口发真实公网 ICMP、把 RTT 喂给 `select_by = quality`）。**该实验已整体撤回**（`exit_probe.rs`、proto 字段、`disable_exit_egress_probe` 开关、roadmap 文档均已删除，工作区无残留）。

撤回理由：复杂度与稳定性收益不成比例。

**保留的不变量**（写进后续设计时应遵守）：

> **探针不喂路由、不触发拆链、不计入流量统计。**

`vip_probe.rs:39-47` 把这条写死并给了理由（探针与用户流量争抢 conn 发送环、丢包会被计入路径、idle 探针流量会扰动 lazy-p2p）。被撤回的实验恰恰违反了它——把 `anchor_rtt_ms` 塞进了 `select_by = quality` 的选路输入。

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

**这条对本文件的意义**：前端"延迟"列对中转行显示的是 OSPF 路径代价，**它永远不可能等于"到目标的真实时延"** —— 即便 B1（出口归属按代价）修好了，也只覆盖到**第一跳**。所以中转行那个数字的 tooltip（「路径代价，非实测 RTT」）不是权宜之计，而是**协议能力的真实边界**。

---

## 7. 核对方法

本文结论均可按以下方式复核（只读）：

| 结论 | 复核点 |
|---|---|
| 测量点只有 2 处 | `peers/conn/peer_conn_ping.rs`（①）、`instance/vip_probe.rs`（②） |
| 派生链 | `peers/conn/conn_select.rs`（`conn_quality_score` / `ospf_edge_cost_from_score`）、`peers/conn/peer_map.rs`（`best_edge_cost` 的未测量分支） → `peers/route/peer_ospf_route.rs` |
| STUN 不面向用户 | `connectivity/stun/client.rs:313` |
| 前端两列 | `frontend-lib/src/components/Status.vue:978`、`:916` |
| 延迟列双口径 | `frontend-lib/src/components/Status.vue:106-111` |
| 回退链分裂 | `frontend-lib/src/modules/statusDisplay.ts:78`、`:570` |
| 路由表二选一 | `peers/peer_manager.rs:197` → `peers/route/peer_ospf_route.rs:5230` |
| `latency_ms` 语义 | `easytier-proto/proto/peer_rpc.proto:289`、`peers/conn/peer_map.rs:665` |
| 撤回无残留 | 代码/proto 内 grep `exit_probe` / `exit_egress` / `EgressCostSource` / `measured_egress` / `disable_exit_egress_probe` / `anchor_rtt` → **0 命中**（仅本文档自身提及这些名字，故须排除 `*.md` 再查） |
