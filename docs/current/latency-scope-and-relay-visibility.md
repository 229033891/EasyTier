# 「延迟」vs「VIP 可达」：口径边界与中转可见性

## Status

- Status: **Current**（审查文档；文中缺陷与建议为结论；**§5 序 1/2/3/4/5/7/11 前端已改代码，待验收** —— 未验收前勿当生产帮助依据）
- 最近审阅：2026-10-22（前端对齐落地待验收；Q5/§3.9；文档核对）；2026-10-21（首次成文）
- 范围：只讨论 peer 表上并排的 **「延迟」列** 与 **「VIP可达」列** 的测量对象、口径差异、中转路径下的可观测性、「是否应合并为一列」（§3.6），以及 **名义含义与实际显示是否一致、如何对齐**（§0 Q5 / §3.9）
- 基线：`git HEAD = 1a69bc4c`（分支 `dev`；对齐改动在工作区未合入时以 PR 为准）；截图内网版本 `2.7.8-1a69…` 对应该基线**改前**行为
- 关联：
  - [`latency-measurement-and-display-audit.md`](./latency-measurement-and-display-audit.md)（测量点全量盘点；本文是它对两列口径的**专题下钻**）
  - [`peer-connections.md`](./peer-connections.md)（一条 peer 多条 PeerConn、`default_conn`）
  - [`../roadmap/gui-connected-vs-reachable.md`](../roadmap/gui-connected-vs-reachable.md)（VIP 探测的契约与三态定义）
  - [`../roadmap/live-next-hop.md`](../roadmap/live-next-hop.md)（Phase F 快速活性探测；**不是**本文的任何一个指标）
  - [`routing-bug-audit-2026-10.md`](./routing-bug-audit-2026-10.md)（B15 深路径清 `latency_first`、B16 转发位权威性 —— §3.8.2 的选路依据）
- 索引：[`../README.md`](../README.md)

本文只描述 **代码今天做什么**，以及据此可判定的展示缺陷。

---

## 0. 结论先行（五个问题，逐条给答案）

### Q1：这两个指标测量的是「两个节点之间的延迟」吗？

**都不是严格的「两节点之间的网络延迟」。** 三者是三条不同的腿：

| 表格里的数字 | 真实测量对象 | 是不是「两节点网络延迟」 |
|---|---|---|
| 「延迟」列 · **直连行** | 本节点 ↔ **直连邻居** 的一条 PeerConn 上，**控制面** Ping/Pong 的 RTT（15 样本滑动**平均**） | **最接近**，但只覆盖**一跳**，且含隧道封装/加解密与调度；**不含** mesh 转发 |
| 「延迟」列 · **中转行**（无直连 conn） | `route.path_latency`：**逐跳代价累加**，每跳 = `RTT平均 + 400×丢包率 + 抖动`（ms 当量） | **是近似值**：默认权重下 ≈ **各跳 RTT 平均之和**，但含罚分项、占位值与夹紧（§3.7） |
| 「VIP可达」列 | 本节点 VIP → 对端 VIP 的一次 **ICMPv4 Echo 往返**，走数据面；回包由**对端宿主栈/ICMP 代理**生成 | **不是纯网络值**。含两端用户态 mesh 处理 + **对端 TUN↔内核整圈往返** |

⇒ **在直连行上**，「VIP可达」比「延迟」大 4–5ms 是**正常的、由设计决定的**，不是链路抖动，也不是采集 bug（§2.3）。中转行不存在这个比较（见 §2.2）。

### Q2：存在中转时，能否体现实测值？

**分三种拓扑，两种能、一种不能，且能体现的那一种也无法分解：**

| 拓扑 | 探测走哪条路 | 中转是否体现在数值里 |
|---|---|---|
| 本节点 ↔ 对端**有直连 conn** | `send_probe_msg_on_conn`：**强制绑定该 conn** | **不能**。数值只含这一跳 + 两端处理；中转段不在里面 |
| 本节点 ↔ 对端**无直连 conn**（relay-only） | `send_probe_msg_by_ip`：走 OSPF 路由 | **能**。这就是真实 mesh 端到端值，含每一个中继跳 |
| 有直连 conn，但 **数据面实际走中转**（典型：`latency_first=true` 且 LeastCost 首跳 ≠ dst；见矩阵 #2） | 探测仍**强制走直连 conn** | **不能，而且更糟**：界面显示的那条路**不是**在用的路，且 UI 无任何提示。若 `latency_first=false`，LeastHop 有直连短路，通常不会走到这一行 |

即使「能体现」，也只能给出一个**总时长**：没有逐跳 RTT，无法回答「是中继节点慢还是对端慢」；同一行的「延迟」列又只是路径代价，**不能用来交叉验证**。

**但这个问题不能只看探测一端**——数据面到底走哪条路由选路决定（两张表 + 包头一位 + 深路径降级），因此把选路也接进来后，答案是**六种分叉**而不是一句话：见 **§3.8.3**。摘要：只有「relay-only 且 `latency_first=true`」时显示值与在用路径同表；「有直连却走中继」「`latency_first` 关闭」「≥3 跳」「首跳已死」四种情况下，显示值描述的不是实际在走的路。

### Q3：优化清单

见 §5。P0 全是纯前端、零协议改动，可直接排期。

### Q4：两个延迟该不该合并为一个？

**要看「两个」指哪两个，答案相反：**

| 被问的对象 | 结论 |
|---|---|
| 「延迟」列 vs 「VIP可达」列 | **不必合并、不应合并测量**（语义/覆盖面/消费者/采样特性四重不同，且 `../roadmap/gui-connected-vs-reachable.md:98` 已否决「单一健康度灯」）。**可选**做法是合并**单元格**：一格两行 `1ms`（链路）/ `≈6ms`（端到端） |
| 「延迟」列**内部**（直连 RTT vs 中转路径代价） | **必须拆开**。不是量纲问题（默认权重下都是 ms 当量），而是**口径**：中转那个数是「逐跳平均估算 + 丢包/抖动罚分」，会被 `500`/`+10000`/`≥1e9` 占位值覆盖 |

详见 §3.6。

### Q5：两列分别代表什么？实际显示与名义一致吗？能否改成一致？

**名义（产品意图）**

| 列 | 名义含义 |
|---|---|
| **延迟** | 到该 peer 的**链路 / 路径时延**（用户读成「网络有多慢」） |
| **VIP可达** | 对端 **VIP 在数据面是否通**；通时附带**业务往返**耗时（含 mesh + 对端主机栈） |

**实际显示**（**改前**；**待验收**后中转延迟优先 VIP，见 §5 序 3）

| 行类型 | 「延迟」实际 | 「VIP可达」实际 |
|---|---|---|
| **直连行**（有 PeerConn） | 该 conn **控制面** Ping 15 样本平均 RTT（`ceil`） | 绑该 conn 的数据面 ICMP **单次** RTT（core `floor`；UI 待验收为 `≈`+`ceil`）；不通则状态词 |
| **中转行**（无 conn） | **改前**：`path_latency` 路径代价仍渲染成 `Nms`。**待验收**：VIP `≈Nms`，否则灰字「路径代价」/「政策绕行」 | 走路由的数据面 ICMP **端到端实测**（若有记录；待验收时延迟列已用该值则 VIP 格只留状态词） |

**名义 vs 实际**

| | 一致？ | 说明 |
|---|---|---|
| 延迟 · 直连 | **基本一致** | 确是一跳隧道时延（含加解密/调度，非裸 IP） |
| 延迟 · 中转 | **改前不一致**；待验收后与名义对齐（VIP 实测或灰字代价） | 改前名义像延迟、实际是代价却带 `ms` |
| VIP · 直连 | **部分一致** | 「通不通」对；数字是业务往返，常被误读成第二列链路延迟 |
| VIP · 中转 | **较一致** | 正是「VIP 通不通、多慢」；无逐跳分解 |
| 两列并排 | **易误导** | 直连差 4–5ms 多半是处理开销；中转代价与 VIP 不可互减 |

**能否改成一致？（可以；展示对齐，不新增测量）** — 详见 **§3.9** / §5

| 目标 | 动作 |
|---|---|
| 延迟列一列一种口径 | 直连保持 conn RTT；中转**禁止**再把 `path_latency` 渲染成实测 `Nms`（首选显 VIP 实测，或灰字「路径代价」） |
| VIP 钉死为「可达 + 业务往返」 | `≈` + 加强 tip；**不要**改成第二列链路延迟 |
| 对齐后还要不要两列？ | **直连：要**（两量；或一格两行）。**中转（B 方案后）：不要双显同一 VIP 数**；延迟=实测时延，VIP=可达状态（或合并格），代价旁注 |

⇒ 这不是「用 VIP 取代延迟后删掉一列」，而是「列名说什么，格子里就是什么」。

---

## 1. 两个字段的报文级差异

```
①「延迟」列（直连行） —— 控制面，一跳，无对端业务参与

  本节点                                          直连邻居
  ┌──────────────────────────────┐   tunnel   ┌──────────────────────────┐
  │ pingpong()                   │  (noise)   │ 收到 Ping               │
  │  t0: 组 Ping(PacketType::Ping)│ ─────────► │  回 Pong                │
  │  t1: 收到 Pong(seq 匹配)      │ ◄───────── │                          │
  └──────────────────────────────┘            └──────────────────────────┘
        RTT = t1 - t0  → 15 样本滑动平均 → PeerConnStats.latency_us


②「VIP可达」列 —— 数据面，端到端，含对端内核

  本节点                                                                 对端
  ┌────────────────────────┐          ┌────────────────┐          ┌──────────────────────┐
  │ 手工组 ICMPv4 Echo      │  mesh    │ 中继跳（若有）  │  mesh    │ 解密→解压→管线匹配    │
  │ 加密→压缩→查路由/强制conn│ ───────► │ 解密→再加密     │ ───────► │ →写 nic_channel→TUN  │
  │                        │          │ 排队→转发       │          │ →【内核】生成 Echo Reply│
  │ 管线过滤器匹配回包       │ ◄─────── │                │ ◄─────── │ →TUN 读→mesh 回程     │
  └────────────────────────┘          └────────────────┘          └──────────────────────┘
        RTT = 单次实测（非平均），已有 20s 周期与 3s 超时
```

三条关键代码事实：

1. `vip_probe.rs:331` —— VIP 的 RTT 是 **`as_millis()`（向下取整）**。
   `statusDisplay.ts::connLatencyMs` —— 延迟列是 **`Math.ceil(latency_us/1000)`（向上取整）**。
   ⇒ **两列取整方向相反**，同一真实值必然差 0–1ms（§4 P0-2）。
2. `peer_conn_ping.rs:405` → `peer_conn.rs:416` `WindowLatency::new(15)` —— 延迟列是 **15 样本平均**；Ping 间隔退避到 `DEFAULT_PING_INTERVAL_MAX_SEC = 10`（`toml.rs:139`），⇒ 窗口覆盖约 **2 分钟**，是**滞后**指标。
3. `peer_manager.rs:1084` `NicPacketProcessor` —— 发往对端**自身 VIP** 的 Data 包会被写入 `nic_channel` 交给 TUN（`not_send_to_tun` 只在「目标是**本机** VIP」时才置位，见 `peer_manager.rs:2580-2594`）。
   ⇒ **VIP 探测的回包由对端操作系统产生**，本特性的探测耗时里必然包含对端主机栈的一整圈往返。

---

## 2. Q1 详解：为什么这不是同一个「延迟」

### 2.1 逐项对照

| 维度 | 「延迟」列 · 直连行 | 「延迟」列 · 中转行 | 「VIP可达」列 |
|---|---|---|---|
| 生产点 | `peer_conn_ping.rs:357` `pingpong()` | `peer_ospf_route.rs` Dijkstra 累加 | `vip_probe.rs` `run_prober()` |
| 承载报文 | `PacketType::Ping/Pong`（控制通道） | 无（派生值） | `PacketType::Data` 里的手写 ICMPv4 |
| 覆盖范围 | **一跳** PeerConn | 全路径（含中转） | 全路径（含中转）**或**强制单跳 |
| 是否端到端 | 否（到邻居为止） | 是（但是估算） | 是（实测） |
| 统计口径 | 15 样本平均，滞后 ~2min | **逐跳** 15 样本平均之**和**（按选路用的那张表求最短路） | **单次**采样（最新一次） |
| 单位 | 毫秒（`ceil`） | **毫秒当量**：`Σ(RTT平均 + 400×丢包率 + 抖动)`，另外可能撞上 `+10000`(fused) / `500`(未测量) / `≥1e9`(避让中继) 占位与夹紧 | 毫秒（`floor`） |
| 含对端业务处理 | 否 | 否 | **是** |
| 参与选路 | **是**（`conn_quality_score`→边权） | 是（它就是路由表） | **否**（纯展示，见 `vip_probe.rs:25-32`） |

### 2.2 「延迟」列的两个数字不是同一种东西

`Status.vue::rowLatency`（**改前**行为；**待验收**后中转优先 VIP，见 §5 序 3）：

```ts
function rowLatency(row: PeerConnTableRow) {
  if (row.conn)
    return connLatencyMs(row.conn)   // ① 实测 RTT
  // 改前：⑤ OSPF 路径代价渲染成 Nms
  // 改后（待验收）：VIP RTT ≈Nms，否则灰字「路径代价」/「政策绕行」
  return latencyMs(row.pair, ...)
}
```

中转 peer 没有 conn ⇒ 落到第二个分支。**改前**显示 `path_latency` 却带 `ms` 后缀。已在 [`latency-measurement-and-display-audit.md`](./latency-measurement-and-display-audit.md) §3.2 / §4 P0 记录；修法见 §3.7.4 B / §5 序 3。

**需要修正一个常见说法**：说 `path_latency`「不是毫秒」是**过度表述**。默认权重下它就是**各跳 RTT 平均之和**的量级（默认 `w_lat = 1.0`、`w_loss = 0.4`、`w_jitter = 1.0`，见 `conn_select.rs`；`ospf_edge_cost_from_score` 注释写「≈ RTT milliseconds」）—— 只是带着罚分项和占位值。详见 §3.7。

### 2.3 直连行「延迟 1ms + VIP 6ms」是正常的：RTT 预算拆解

| 段 | 内容 | 典型量级（推断） |
|---|---|---|
| 本端 mesh 出站 | 压缩 + 加密 + 查路由/绑定 conn + 塞发送环 | 0.1–1ms（用户态调度） |
| 去程隧道单程 | 隧道封装 + 内核 socket 收发 | ≈ `延迟列 / 2` |
| 对端 mesh 入站 | 解密 + 解压 + 管线匹配 + 写 `nic_channel` | 0.1–1ms |
| **对端内核往返** | TUN 写 → 内核 ICMP 应答 → TUN 读 | **1–4ms（WinTun 上最显著）** |
| 回程（镜像） | 同上 | 0.5–2ms |
| **合计** | | **≈ 5–6ms** ← 与截图的 5/6/7ms 吻合 |

> 口径说明：**代码路径为核实结论**（`NicPacketProcessor` 写 TUN、回包由对端栈产生），**分段量级为推断**，未做逐段埋点实测。要坐实需按 §7 的抓包方法验证。

**产品含义**：VIP 列本质上是一个「**用户视角的业务往返耗时**」，不是链路质量。它偏大恰恰说明两端处理开销不小，而不是网络慢。**目前 UI 完全没有表达这一点**，用户会把它当链路延迟读，于是得出「延迟 1ms 但 VIP 6ms，说明网络在抖」的错误结论。

### 2.4 反向也会发生：VIP 比「延迟」小

截图中存在 `VIP可达 5ms` 但 `延迟 8ms` 的行。三个独立原因，均可解释：

1. **取整方向相反**（§1 第 1 条）—— 5.4ms 在 VIP 列显示 5，在延迟列显示 6。
2. **平均 vs 单次** —— 延迟列是 15 样本平均（约 2 分钟窗口），VIP 是最新单次。拥塞窗口过去后，单次值低于平均值是正常的。
3. **控制通道自身排队** —— `ctrl_resp_sender` 是 broadcast，Pong 要经接收循环转发；而探测回包走的是 peer 数据管线。两条通路的调度位置不同。

⇒ **这两个数字方向不一致不代表任何一方出错**，但当前 UI 允许用户把它们相减，这是设计缺口。

---

## 3. Q2 详解：中转的可见性边界

### 3.1 链路枚举规则决定了一切

`vip_probe.rs:342` `collect_links()` 遍历**路由快照**，对每个 peer：

```rust
let mut conn_ids = peer_manager.peer_live_conn_ids(route.peer_id);
if conn_ids.is_empty() {
    // 节点级条目：relay-only / 无法命名链路 → 走路由路径
    links.push((LinkKey { peer_id, conn_id: None }, peer_vip));
} else {
    // 逐链路条目：每条 live PeerConn 各一条
    for conn_id in conn_ids { links.push((LinkKey { peer_id, conn_id: Some(conn_id) }, peer_vip)); }
}
```

`vip_probe.rs:499` / `:503` 据此分派：

| `conn_id` | 发送函数 | 路径 |
|---|---|---|
| `Some(conn_id)` | `send_probe_msg_on_conn` → `peer.send_msg_on_conn(conn_id, msg)` | **强制这条 conn**，绕过 OSPF |
| `None` | `send_probe_msg_by_ip` → `get_msg_dst_peer_ipv4` + 正常路由 | **OSPF 选路**，含中转跳 |

⇒ **中转能否被体现，完全取决于「该 peer 有没有 live conn」，而不是「实际流量走不走中转」。**

### 3.2 能体现的情形（relay-only）

`conn_ids.is_empty()` ⇒ 节点级条目 ⇒ 走路由路径 ⇒ **数值是真实的 mesh 端到端 RTT，每一个中继跳的排队与转发开销都在里面**。

这是本特性设计时明确要覆盖的场景（roadmap §2.5 / §5：全中继网络不能显示 `[0/0]`）。在这种拓扑下，VIP 列 **确实**比「延迟」列更有信息量——「延迟」列压根没有可用的 conn 数字，退回成路径代价。

### 3.3 不能体现的情形（有直连 conn 但实际走中转）—— 本审查最重要的一条

强制绑定 conn 的探测有一个未被处理的副作用：**它测的是「这条链路」的通不通，而不是「业务实际走的路」的通不通。**

只要满足下面全部条件，「VIP可达」显示的数值就与在用的路径无关：

1. 该 peer 存在至少一条 live PeerConn（有直连）；
2. OSPF 对它的最优路径却不是那条直连（直连边代价高 / `fused` / 丢包熔断 / `AVOID_RELAY_COST` 策略 / `latency_first` 用另一张表）；
3. 于是用户流量走 `本机 → 中转 → 对端`。

此时：

- 行是按 conn 展开的 ⇒ 这一行显示的是**那条直连链路**；
- 但用户流量根本不走它 ⇒ `VIP可达 6ms` 描述的是一个**不在用的路径**；
- 而「延迟」列同理（也是那条 conn 的控制面 RTT）。

⇒ **在恐怕最需要排障的时刻（直连劣化、已切中转），表格里两列都还在描述那条已经被弃用的链路。** 这与 [`latency-measurement-and-display-audit.md`](./latency-measurement-and-display-audit.md) §4 P1「前端无条件优先 `path_latency_latency_first`」是同一族问题的两个侧面。

> 注：`route.next_hop_peer_id` / `next_hop_peer_id_latency_first` 已在快照里下发；前端 `statusDisplay.ts` 有 **module-private** 的 `nextHopPeerId()`（当前无条件优先 `*_latency_first`）。⇒ **判定「本行是不是在用路径」纯前端可做**（§4 P0-4 / §5 序 4），但须 **export 或抽公共 API**，且与序 11（统一取哪张表）一起做，否则在 `latency_first=false` / 深路径下会误标。

### 3.4 即使能体现，也无法分解

节点级条目只给一个总 RTT，协议里没有逐跳耗时：

- 无法回答「慢在中继还是慢在对端」；
- 「延迟」列在同一行是路径**代价**，量纲不同，**不能**用「VIP RTT − 延迟」去估算中转段（这是最容易被误用的地方，应在 tooltip 明确禁止这种减法）；
- `PeerVipReachability` 无 `hop_count` / 无 `next_hop`，前端连「这条记录经过几个节点」都表达不出来。

⇒ **中转可见性的现状是：能告诉你「通不通、总共多慢」，不能告诉你「哪里慢」。**

### 3.5 时效性：链路一多，数值就过期

`vip_probe.rs:93-98`：

```
PROBE_INTERVAL = 20s
MAX_CONCURRENT_PROBES = 3      // 每轮只探 3 条
cursor 按 3 前进 → 公平轮转（覆盖性有保证，但周期随链路数线性增长）
```

| 链路数 L | 单条链路刷新周期 |
|---|---|
| 3 | 20s |
| 9 | 60s |
| 18 | 120s |
| **25（截图）** | **约 180s** |

且 `vip_probe.rs` `snapshot_reachability` 虽带 `checked_at_unix_ms`，**改前前端从未渲染**
（仅类型声明）。**前端已改（待验收）**：`statusDisplay.ts::vipCheckedAtRelative` + `Status.vue::rowVipTip` 展示相对时间与刷新周期提示（§5 序 5）。grep `checked_at` / `vipCheckedAtRelative` 即可复核。

⇒ 在 25 条链路的现网里，VIP 数字**最长仍可能是 ~3 分钟前的**（调度未改）；待验收后 tip 应能说明「多久前测的」。回答「中转能否体现**实际**值」时仍须带上周期限定。

---

### 3.6 专题：两个延迟该不该合并为一列？

> 本节回答现网反馈的「两个延迟是否有必要合并为一个」。结论对**两种可能的歧义**分别给出，因为它们的答案**相反**。

### 3.6.1 先分清「两个延迟」指哪两个

| 歧义 | 指哪两个数 | 正确动作 |
|---|---|---|
| **A** | 「延迟」列 vs 「VIP可达」列（两个都带 ms） | **不要合并测量**；只可合并**单元格**（可选） |
| **B** | 「延迟」列**内部**：直连行实测 RTT vs 中转行路径代价 | **必须拆开**（这是该拆，不是该合） |

两条结论方向相反，混在一起讨论必然达不成一致。下面分开说。

### 3.6.2 歧义 A 的结论：不要合并为一个数

**四条硬理由，任一条都足以否决：**

1. **语义不同，无法互相代替。** ①（控制面一跳 RTT）再小也**不能**证明 VIP 可达 —— 「控制面通、数据面不通」正是本特性存在的理由（`../roadmap/gui-connected-vs-reachable.md` §1）。②（VIP 往返）再大也只能说明「慢」，不能说「断」。合并必然丢掉其中一半语义。
2. **覆盖面不同，一方根本没有对方的数据。** ① 是一跳；② 是端到端（含对端内核整圈）。**relay-only peer 没有任何 conn ⇒ ① 无值**；反过来 ① 也永远看不到中转段与对端处理开销。
3. **消费者不同，合并会破坏一条写死的不变量。** ① 喂选路（`conn_quality_score` → `ospf_edge_cost_from_score` → 边权）；② 被明令**禁止**喂选路（`vip_probe.rs:25-32`）。合并必然让一方妥协：要么路由开始依赖**最长 180s 陈旧、且掺了对端宿主栈抖动**的样本（§3.5 + §2.3），要么放弃「业务往返确实通」这条证据。
4. **采样特性不同，合并必须选一种统计口径，另一种信息即丢失。** ① 是 1–10s 周期、15 样本平均、相对稳定；② 是 20s 周期、**单次**采样、25 条链路时 180s 才刷一遍。一个数字不可能同时是「稳定的选路输入」和「最新的端到端实测」。

**产品侧还有一条硬约束**：`../roadmap/gui-connected-vs-reachable.md:98` 明确写「与延迟、丢包、代价列分开；**不合并成单一健康度灯**」。⇒ 合并成单值等于撤销该决策，需要另立 roadmap 复议，不能以「UI 优化」名义夹带。

### 3.6.3 合并的收益是真的，但可以不靠「合并测量」拿到

| 合并想解决的问题 | 不合并测量的解法 |
|---|---|
| 表格已 15 列，横向拥挤 | 同一单元格**上下两行**：`1ms`（链路）/ `≈6ms`（端到端），列头收成「时延」并在 tooltip 定义两行口径 |
| 用户把两列相减当抖动 | 两行同格 ⇒ 视觉上就是「一个指标的两种粒度」，tooltip 加一句「差值主要是两端处理开销，不可相减得抖动」（§5 序 1） |
| 认知负担 | 只在「直连行」叠两行；中转行只显示 VIP 实测值 + 「路径代价」灰字（§3.6.4） |

⇒ **推荐形态**：一格两行、带 `≈` 与单位、可分别排序与告警。空间与认知收益全额拿到，两个测量值仍然可辨。成本约 1 天纯前端。

### 3.6.4 歧义 B 的结论：这里必须拆，不是合

中转行的「延迟」是**逐跳平均之和 + 罚分项**（估算，可被 `500`/`+10000`/`≥1e9` 覆盖），「VIP可达」是**单次端到端实测**。二者**不能**合并成一个「更准的延迟」——不是因为量纲（默认权重下两者都是 ms 当量，§3.7.1），而是因为**口径**：把估算当实测、把罚分当 RTT 是双重失真。

正解按 §3.7.4 排：**优先用已有的实测值（B 方案）**，把代价降级为独立指标或灰字；退而求其次是不显示数字（A 方案）。这才是「一列只放一种口径」的正确形态。对齐后是否还保留两列：见 **§3.9.4**（直连仍要两量；中转 B 方案后勿双显同一 VIP）。

### 3.6.5 若采纳「一格两行」，验收方式

- 单测：直连行合并单元格必须**同时**含两个数值且带 `≈`；任一为 `unknown`/`未探测` 时降级为单值；
- 除 unknown 外，**不允许出现只有一个数的单元格**（否则等于事实上的合并）；
- 排序/筛选仍分别作用于两个值（不能只留一个可排序字段）；
- 中转行的延迟列**不得**渲染 `path_latency`（任何含罚分/占位值的数都不许进这一列，§5 序 3）；

### 3.7 专题：延迟列能否显示「中转时的延迟」？

> 本节回答「延迟列是否有可能显示中转时的延迟」。
>
> - **改前（渲染 `path_latency`）**：能，而且当时显示的就是路径代价当量 —— 但只能当「近似值」：四种附加项会严重失真（丢包/抖动罚分、未测量占位 `500`、`fused`/避让中继的 `+10000`/`≥1e9`）。
> - **待验收（§5 序 3 B）**：中转行延迟列改显 VIP 实测（或灰字代价），**不再**把 `path_latency` 渲染成 `Nms`。下面 §3.7.1–3.7.3 仍描述 **`path_latency` 公式本身**（选路与 tip 旁注仍用）。

#### 3.7.1 数据链与公式（默认配置下 ≈ 各跳 RTT 之和）

```
每跳边权（对端上报，per-peer 取最优 conn —— 丢包/抖动都由对端算好再上报）
  score     = w_lat·(RTT/1e6) + w_loss·loss + w_jitter·(jitter/1e6)   conn_select.rs
  edge_cost = round(score × 1000)  [+10000 if fused]                  conn_select.rs
  默认      w_lat = 1.0 / w_loss = 0.4 / w_jitter = 1.0
  ⇒ edge_cost(ms) ≈ RTT_ms + 400×loss_rate + jitter_ms
        │
        ▼ Dijkstra 沿路径累加（LeastCost 选最省；LeastHop 只按跳数选路，累加仍是真实边权）
  path_latency = Σ edge_cost                                          peer_ospf_route.rs
        │
        ▼ 前端（改前）Math.ceil(path_latency) + "ms"
        ▼ 前端（待验收）latencyMs：VIP ≈Nms / pathCostLabel / 政策绕行；代价可进 tip
                                                          statusDisplay.ts::latencyMs
```

⇒ **默认权重下 `path_latency` 的量纲就是毫秒，数值≈各跳 RTT 平均之和**。`ospf_edge_cost_from_score` 自己的注释也写「With default `w_lat=1`, zero loss/jitter maps ≈ RTT milliseconds」。所以「用 `path_latency` 当中转延迟」**在公式上成立**——问题全在附加项；**展示侧**已改走 VIP（待验收）。

#### 3.7.2 准确度分三段

| 场景 | `path_latency` 实际值 | 能否读成中转延迟 |
|---|---|---|
| 干净两跳（丢包 0、抖动 ~0） | `RTT₁ + RTT₂`（ms） | **可以**。误差 <2ms（各是 15 样本平均，且不含中继的转发处理） |
| 有丢包 / 有抖动 | `RTT₁+RTT₂ + 400×(loss₁+loss₂) + (jit₁+jit₂)` | **不行**，偏差可达数倍（见下表） |
| 含未测量跳 / `fused` / 避让中继 | 混入 `500` / `+10000` / `≥1e9` | **完全不能**，与延迟无关 |

丢包罚分的量级（`w_loss = 0.4`，且丢包率是 100 样本窗口 ⇒ **丢 1 个包即 1%**）：

| 真实 RTT | 丢包率 | 显示值 | 偏差 |
|---|---|---|---|
| 8ms | 0 | 8 | 0 |
| 8ms | 1%（丢 1 个包） | 12 | +50% |
| 8ms | 5% | 28 | +250% |
| 8ms | 20%（fused 阈值附近） | 88 | +1000% |

⇒ **丢一个包就把它抬高 4ms**；两跳叠加两次。抖动同理按 ms 全额计入。

#### 3.7.3 一条可操作的判读规则

**同一 peer：`VIP 值` 通常应 ≥ `延迟值`（容差 ±1ms 取整）**

- 直连行：`VIP ≈ 延迟 + 4~5ms`（§2.3 两端处理开销）；
- 中转行：`VIP ≈ 延迟 + 处理开销`（VIP 是实测端到端，延迟是逐跳平均之和）。

⇒ **一旦看到 `VIP` 明显小于 `延迟`（如 VIP 5ms、延迟 40ms），几乎必然是延迟列里混进了罚分项（丢包/抖动）或未测量占位值**，而不是「VIP 比延迟准」。这条规则纯前端可做，可作为一致性提示（§5 序 4 的扩展）。

#### 3.7.4 想让它「真的」显示中转延迟，有三条落地路径

| 方案 | 做法 | 代价 | 评价 |
|---|---|---|---|
| **A（保守）** | 中转行延迟列不显示数字，改灰字「路径代价」（= §5 序 3） | 纯前端，最低 | 立刻消除误导，但也放弃了唯一的中转数值 |
| **B（推荐先做）** | 中转行延迟列改显示 **VIP 实测 RTT**（真实端到端），代价另开一列或进 tooltip | 纯前端 | **不需要新增测量**：relay-only peer 的 VIP 记录本来就是实测端到端（§3.2），同一快照里已有 |
| **C（彻底）** | `Route` 增 `path_rtt_ms`（沿路径累加纯 RTT 项，不含 loss/jitter），与 `path_latency` 并存 | proto + core + web（= §5 序 10） | 唯一能给出「与在用路由一致、且不含罚分」的逐跳累加 ms；需另立设计 |

**关键认识**：中转行并非没有实测数据 —— **实测数据就在 VIP 列里**，只是延迟列没用它，反而用了含罚分的代价。所以 B 方案的本质是「把已有的实测值放到延迟列，把代价降级为独立指标」，与 §3.6 的「不合并测量、可合并单元格」不冲突。

> 注意时序差异：VIP 是单次采样（25 链路时约 180s 刷新一次，§3.5），代价是每跳 15 样本平均（约 2min，§1 第 2 条）。两者都慢，量级相当，所以在同一格并列不会造成「一新一旧」的观感撕裂。

### 3.8 专题：把选路逻辑接进来 —— 表格数字对应的是哪张表、哪条路

> §3.3 / §3.7 只看了「数字怎么算出来」。真正决定「这个数字描述的是不是**在实际用的那条路**」的是选路：**有两张路由表**，且**数据面用哪张表不取决于前端**。本节把链路打通。

#### 3.8.1 两张表 + 三个消费点

| 表 | 构建方式 | 谁在用 |
|---|---|---|
| **A：`route_table`（LeastHop）** | Dijkstra 边权归一化为 1，先定最少跳，再在等跳子图里按真实权重选 | `route.cost` / `route.next_hop_peer_id`；数据面 `policy == LeastHop` 时 |
| **B：`route_table_with_cost`（LeastCost）** | Dijkstra 直接用真实边权 | `route.cost_latency_first` / `next_hop_peer_id_latency_first` / `path_latency_latency_first`；数据面 `policy == LeastCost` 时 |

`list_routes()` 一次把两张表的结果**一起**下发（`peer_ospf_route.rs:1032-1062`）：

```1039:1058:easytier-core/src/peers/route/peer_ospf_route.rs
            let Some(next_hop_peer) = self.get_next_hop(*item.key()) else {
                continue;
            };
            let next_hop_peer_latency_first = route_table_with_cost.get_next_hop(*item.key());
            let mut route: CoreRouteInfo = item.value().clone().into();
            route.next_hop_peer_id = next_hop_peer.next_hop_peer_id;
            route.cost = next_hop_peer.path_len as i32;
            route.path_latency = next_hop_peer.path_latency;

            route.next_hop_peer_id_latency_first =
                next_hop_peer_latency_first.map(|x| x.next_hop_peer_id);
```

⇒ **`route.cost` / `next_hop_peer_id` 来自 A 表；`*_latency_first` 三个字段来自 B 表。**

#### 3.8.2 数据面到底用哪张表：由**包头一位**决定，不是前端

`send_msg_internal`（`peer_manager.rs:5031-5060`）：

1. `policy = get_next_hop_policy(hdr.is_latency_first())` ⇒ `true → LeastCost`，`false → LeastHop`（`peer_manager.rs:199-204`）；
2. 该位在**源节点**写入：`hdr.set_latency_first(packet_policy.latency_first)`，默认 **`latency_first = true`**（`toml.rs:168`）；
3. **转发节点看的是包头这位，不是自己的配置**；
4. **深路径降级**：`forward_counter > 2 && is_latency_first` ⇒ 强制置 false（`peer_manager.rs:4695-4697`），即 ≥3 跳后改用 LeastHop。

> 第 3、4 条不是本次新发现：分别登记为 [`routing-bug-audit-2026-10.md`](./routing-bug-audit-2026-10.md) 的 **B16**（转发用发包方位的位）与 **B15**（深路径静默清位），状态均为「⏸ 待决」。本文引用它们只是为解释「为什么展示与事实会分叉」，不重复论证其取舍。

选定策略后的出接口顺序（`send_msg_internal`）：

```
gateway = get_gateway_peer_id(dst, policy)          // B 表 + 活跃性过滤 + 等代价备份
  ├─ latency_first 且 gateway != dst 且该 gateway 有活 conn → 走中继（relay_peer_map）
  ├─ 否则本机有到 dst 的活直连 conn                        → 直连（peer.send_msg，走 default_conn/bond）
  ├─ 否则 dst 是自己                                       → 本地投递
  └─ 否则再试 gateway …
```

注意两处容易忽略的细节：

- **`get_gateway_peer_id` 会做活跃性过滤**：首跳死了会顺延到**等代价备份**（`live-next-hop.md` Phase B/D）。等代价 ⇒ `path_latency` 不变，所以数字仍成立；但**首跳死了且无可用备份时，`send_msg_internal` 会回落到直连**（若有），此时表格仍在描述那条已死的路径。
- **LeastHop 有短路**：`peer_map.rs:318-320`，只要 dst 有活直连 conn 就直接返回 dst ⇒ 该策略下数据面**优先直连**，不看多跳。

#### 3.8.3 六种分叉矩阵（这才是完整答案）

| # | 条件 | 数据面实际走 | 「延迟」列描述 | 结论 |
|---|---|---|---|---|
| 1 | 有直连 conn，且它就是 LeastCost 首跳（`next_hop == dst`） | 直连 | 该 conn 的实测 RTT | **一致**（默认最常见） |
| 2 | 有直连 conn，但 LeastCost 首跳是某中继 | **中继** | 该**直连 conn** 的 RTT | **分叉**（§3.3 的 Case C，已在选路层坐实） |
| 3 | 无直连（relay-only），`latency_first = true` | LeastCost 路径 | **改前** B 表 `path_latency`；**待验收** VIP 实测（同策略） | **改前同表**（含罚分）；待验收后延迟列用 VIP，与数据面同路（§3.8.5） |
| 4 | 无直连，`latency_first = false`（或 `p2p_only`） | **LeastHop** 路径 | **改前**无条件读 B 表；**待验收** `resolveRouteTableKind` → A 表 | **改前分叉**；**待验收已修**（与数据面同表；深路径仍见 #5） |
| 5 | 任意，但路径深度 ≥3（`forward_counter > 2`） | 被强制降级到 LeastHop | UI 仍按本机配置选表（通常 B） | **仍分叉**：深路径降级 GUI 不可见（B15） |
| 6 | LeastCost 首跳已死且无等代价备份 | 回落直连（若有），否则丢/失败 | 已死路径的 cost / 或 VIP | **分叉**：数字可能描述已不通的路 |

⇒ 对用户问题的完整回答：**「延迟列能不能显示中转延迟」取决于落在哪一行**。#3 在改前能（同表代价）；待验收后 #3/#4 中转延迟改 VIP 实测更贴数据面。#5/#6 仍可能分叉，#1 是直连无所谓中介，**#2 最危险 —— 两列都在描述一条被弃用的链路**。

#### 3.8.4 由此新增的展示缺陷：前端自己也在混用两张表

> **改前**（审查基线）下表成立。**前端已改（待验收）**：`RouteTableKind` + `routeHopCost` / `nextHopPeerId` / `pathLatencyCost` 三处同表；LeastCost 时已消费 `cost_latency_first`（`statusDisplay.ts::routeHopCost`）。深路径 `forward_counter>2` 与包头位仍不可见。核对请用 §7 符号引用，勿按旧行号。

| 列 | 改前取值 | 改前来源表 |
|---|---|---|
| 延迟 | `path_latency_latency_first ?? path_latency` | **B 表** |
| 路径 · 下一跳 | `next_hop_peer_id_latency_first ?? next_hop_peer_id` | **B 表** |
| 路径 · 跳数 | `info.route.cost` | **A 表** |
| 路径 · LeastCost 跳数 | `cost_latency_first` | **改前从未使用** |

⇒ **改前**：同一行里跳数来自 A 表、下一跳与延迟来自 B 表，三者可能不是同一条路径。这比 §4 P1-2 的「分母混合粒度」更隐蔽。

修法（纯前端，§5 序 11，**已改待验收**）：三处统一到同一张表——按生效策略选表（默认 `latency_first && !p2p_only` → B；`latency_first=false` 或 `p2p_only` → A，对齐 `peers/context.rs` `packet_policy`）；消费 `cost_latency_first`。

#### 3.8.5 为什么这决定了 §3.7.4 的方案选择

- **仅对 relay-only / 节点级探测**（`send_probe_msg_by_ip`，`peer_manager.rs:4219` 盖本机 `latency_first` 位）：探测与本地用户流量**同一策略**，且中途同样受 `forward_counter > 2` 降级 ⇒ 此时 VIP 值与本地用户流量走的是同一条路，比前端无条件读的 `path_latency_latency_first` 更贴近事实。
- **有 live conn 时不适用本条**（`send_probe_msg_on_conn` 强制绑 conn，见 §3.3 / 矩阵 #2）——B 方案本来就只改**中转行**（无 conn），不依赖「有直连却走中继」场景。
- ⇒ 所以 §3.7.4 的 **B 方案（中转行改用 VIP 实测值）不仅省事，而且在 #4/#5/#6 三种分叉下比 `path_latency` 更准**。这是选路分析给出的额外依据。
- 反过来，任何「把 `path_latency` 标成延迟」的方案，在 #4/#5/#6 下都会持续说谎，而且用户无法从界面上判别自己落在哪一行。

### 3.9 专题：名义含义 · 实际显示 · 对齐目标（产品口径）

> 本节把 §0 **Q5** 展开为可验收的对齐目标。与 §3.6（该不该合并）、§3.7（中转延迟能否显示）、§3.8（选哪张表）正交：**先定「列代表什么」**，再定单元格里放哪个字段。

#### 3.9.1 两列的契约（应对齐到的名义）

| 列 | 契约（稳定产品语义） | 非契约（禁止暗示） |
|---|---|---|
| **延迟** | **链路 / 路径时延**（直连=该隧道控制面 RTT；中转=端到端时延的诚实展示，见下） | 不是 OSPF 代价；不是丢包/熔断罚分；不是「到网外目标」的距离 |
| **VIP可达** | **数据面 VIP 通不通**；通时附带**业务往返** RTT | 不是链路质量；不是选路输入；不可与延迟列相减得「抖动」 |

Roadmap（`../roadmap/gui-connected-vs-reachable.md:98`）已要求与延迟/丢包/代价列分开，不合并成单一健康度灯 —— 本契约与之兼容。

#### 3.9.2 今日偏差（一句话）

- **直连延迟**：名义 ≈ 实际（可接受）。
- **中转延迟**：名义 ≠ 实际（代价冒充 `ms`）—— **主缺陷**，对应 P0-5 / §5 序 3。
- **VIP 数字**：名义是业务往返，UI 弱提示 ⇒ 易被当成第二列链路延迟 —— 对应 P0-1 / §5 序 1。

#### 3.9.3 对齐后的单元格规则（目标态，尚未落地）

| 行类型 | 延迟列应显示 | VIP 列应显示 | 代价（`path_latency`） |
|---|---|---|---|
| 直连 | 该 conn 控制面 RTT（保持） | 可达：`≈` + 业务 RTT；不可达/未探测：状态词 | 不进延迟列；可选 tooltip |
| 中转 | **VIP 实测 RTT**（§3.7.4 B）或灰字「路径代价」/空（A） | **可达状态**为主；若延迟列已用 B，**勿再重复同一个 ms**（可只保留 Tag 色 + 状态，或与延迟合并为一格） | 降级为独立列 / 灰字 / tooltip，**禁止**再标成无说明的 `Nms` |

#### 3.9.4 对齐后「还要不要两列」

| 场景 | 结论 |
|---|---|
| 直连 | **要两量**（两列或一格两行）。链路 RTT ≠ VIP 业务往返；合并测量会丢掉「控制面通、数据面不通」。 |
| 中转 + B 方案 | **不要双显同一 VIP 数**。一列承载「时延实测」，VIP 列承载「通不通」；代价旁注。 |
| 中转 + A 方案 | 延迟列无数字（或灰字代价）时，VIP 列仍是唯一实测时延来源 ⇒ **两列仍都有用**，但延迟列须标明不是 RTT。 |

#### 3.9.5 与 §5 的映射

| 对齐动作 | §5 |
|---|---|
| VIP 口径（`≈`、tip、不可相减） | 序 1（+ 序 5 新鲜度） |
| 中转延迟去代价冒充 | 序 3（B 优先） |
| 取自同一张路由表 / 非在用路径标记 | 序 11 → 序 4 |
| 取整一致 | 序 2 |

---

## 4. 已核实缺陷清单

### P0-1 —— VIP 列仍易被当成链路延迟读（口径提示不足）

改前：可达时只输出 `6ms`，tip 口径偏弱。列头「VIP可达」与「延迟」视觉同级。
**前端已改（待验收）**：可达显示 `≈Nms` / `≈<1ms`；`vip_boundary_tip` 写明端到端、含主机栈、不可与延迟列相减得抖动（§5 序 1–2）。

### P0-2 —— 两列取整方向相反

- VIP core：`vip_probe.rs` `elapsed().as_millis()` —— **floor**
- 延迟：`statusDisplay.ts::connLatencyMs` `Math.ceil(latency_us / 1000)` —— **ceil**

同一 5.4ms 在改前左列 `5ms`、右列 `6ms`。**前端已改（待验收）**：`formatVipRttMs` 对展示做 `Math.ceil` + `≈`（§5 序 2）；core 存储仍为 floor。

### P0-3 —— `0ms` 与 `<1ms` 不可区分

VIP core sub-ms 保留为 `0`。**前端已改（待验收）**：展示为 `≈<1ms`（§5 序 2）。

### P0-4 —— 界面上无法区分「本行链路」与「实际在用的路径」

见 §3.3。`next_hop_peer_id` 已在前端可用，判定逻辑缺失。

### P0-5 —— 中转行「延迟」把含罚分的估算渲染成实测 RTT

`path_latency` 是「逐跳 15 样本平均之和 + `400×丢包率` + 抖动」，并可能撞上 `500`（未测量）/ `+10000`（fused）/ `≥1e9`（避让中继）。默认权重下它 ≈ 各跳 RTT 之和，所以**看起来完全像延迟**，这比明显错误的数字更危险（§3.7.2 丢包罚分表：丢 1 个包 → +4ms，5% → +250%）。
沿用 [`latency-measurement-and-display-audit.md`](./latency-measurement-and-display-audit.md) §4 P0。修复方案见 §3.7.4 / 本文 §5 序 3。**前端已改（待验收）**：中转行延迟优先 VIP 实测 / 否则灰字「路径代价」，不再渲染 `path_latency` 为 `Nms`。

### P1-1 —— 探测时间未展示

`checked_at_unix_ms` 已下发但未渲染（§3.5）。用户无法判断数值新鲜度。**前端已改（待验收）**：VIP tip 含相对时间 + 刷新周期提示（§5 序 5）。

### P1-2 —— `[M/N]` 的分子分母混合了两种粒度

`M/N` 按**链路**计（roadmap §2.3），但 relay-only peer 的节点级条目**也计 1 条**。⇒ 25 这个分母里既有「PeerConn 条数」也有「节点数」，跨拓扑不可比。已由 roadmap 记录；**前端已改（待验收）**：badge tip 说明 N 含中继节点条目（§5 序 7）。粒度本身未改。

### P1-3 —— 同一 peer 多条 conn 时，VIP 列造成节点重复观感

截图里 DEV 一台设备因 3 条 conn 占 3 行，VIP 分别为 `5ms`/`3ms`/`7ms`。这是**正确**的（每链路各测），但没有任何分组提示，容易被读成「三台不同的机器时好时坏」。

### P1-4 —— 前端在同一行混用两张路由表

跳数取 A 表 `route.cost`、下一跳与延迟取 B 表 `*_latency_first`，`cost_latency_first` 从未使用（§3.8.4）。⇒ 三者可能不是同一条路径，界面却呈现为自洽。**前端已改（待验收）**：跳数/下一跳/路径代价读取统一到 `RouteTableKind`（默认 LeastCost；`latency_first=false` 或 `p2p_only` 时 LeastHop）（§5 序 11）。深路径 `forward_counter>2` 仍无法在 GUI 侧感知。

### P1-5 —— 展示未反映「数据面用哪张表」

数据面按**包头 `latency_first` 位**（默认 true；core 有效位为 `latency_first && !p2p_only`，`peers/context.rs` ~L537）+ `forward_counter > 2` 降级选表（§3.8.2）。**前端已改（待验收）**：`resolveRouteTableKind(latencyFirst, { p2pOnly })` 与 core 对齐（序 11）；深路径静默降级仍属 B15 待决。

### P2-1 —— 探测无逐跳信息

§3.4。要分解中转贡献需新增字段（`hop_count` 或逐跳 RTT），成本高，建议只做一次性诊断模式。

### P2-2 —— 探测调度不区分链路重要性

纯 round-robin（`cursor.fetch_add(take)`）。当前实际在用的 `default_conn` 并不优先刷新，热门链路和被弃用链路拿到相同的刷新频率。

---

## 5. 优化建议（按落地成本排序）

> **落地进度（2026-10-22）**：序 **1/2/3/4/5/7/11** 前端已改（`statusDisplay.ts` / `Status.vue` / i18n），vitest 相关用例已更新；**待现网/联调验收**后，再把 §0–§3「今天做什么」改写成新现状。序 6/8/9/10 未做。

| 序 | 改动 | 落点 | 成本 | 风险 | 状态 / 验收 |
|---|---|---|---|---|---|
| **1** | VIP 加 `≈`；加强 `vip_boundary_tip`（端到端/不可相减） | `vipStateLabel` / tip + i18n | 纯前端 | 极低 | ✅ 已改，待验收 |
| **2** | VIP `Math.ceil`；`0`→`≈<1ms` | `formatVipRttMs` | 纯前端 | 低 | ✅ 已改，待验收 |
| **3** | 中转延迟：VIP 实测优先；否则灰字「路径代价」；不渲染 `path_latency` 为 `Nms`；中转 VIP 格只留「可达」防双显 | `rowLatency` / `latencyMs` | 纯前端 | 低 | ✅ 已改，待验收 |
| **4** | 「非在用路径」灰点 + tip（直连行但 next hop ≠ dst） | `isDirectConnNotOnActivePath` + Status | 纯前端 | 低 | ✅ 已改，待验收 |
| **5** | tip 含 `checked_at` 相对时间 + 刷新周期 | `vipCheckedAtRelative` / `rowVipTip` | 纯前端 | 低 | ✅ 已改，待验收 |
| **6** | 同 peer 多 conn 分组视觉增强 | Status 表格 | 纯前端 | 中 | 未做 |
| **7** | `[M/N]` tip 说明 N 含中继节点条目 | `vip_reachability_badge_tip` | 纯前端 | 极低 | ✅ 已改，待验收 |
| **8** | 探测调度优先在用链路 | `vip_probe.rs` | core | 中 | 未做 |
| **9** | 逐跳 / hop_count | proto + core | 中 | 中 | 未做 |
| **10** | `path_rtt_ms` 与代价分列 | proto + core + web | 大 | 中 | 未做 |
| **11** | 跳数/下一跳/代价同表；按 `latency_first && !p2p_only` 选表（对齐 `peers/context.rs`） | `resolveRouteTableKind` + Status `latency-first` / `p2p-only` | 纯前端 | 低 | ✅ 已改，待验收（深路径降级仍不可见） |

**建议的排期**：1→2→3→**11→4**→5→7 一批（纯前端；**序 4 紧接序 11**）——本批已改；8 单独；9/10 另立 roadmap。

**可选（取决于是否采纳 §3.6.3 的「一格两行」）**：把序 1 与序 6 合并成一个「时延」单元格重构，成本约 1 天纯前端；前置条件是序 3 先落地且中转行**不得再渲染 `path_latency`**（B 显 VIP 或 A 灰字均可），否则同格里会出现「代价」与「ms」两种口径。

---

## 6. 待现场核对（代码无法自证）

| 现象（截图） | 代码预期 | 需核对什么 |
|---|---|---|
| 首行（`路由 = 本机`）在 VIP 列出现 `6ms` | `collect_links` 跳过 `route.peer_id == my_peer_id`（`vip_probe.rs:347-349`）⇒ 本机不应有探测记录 ⇒ 前端应渲染「未探测」 | 是截图列对齐误读，还是前端 join 落到了本机行（`vipReachabilityIndex.forConn` 的 peer 级回退分支） |
| DEV 三条 conn 的「延迟」全为 `8ms` | 每条 conn 各自 15 样本平均，通常应有差异 | 三条 conn 是否骑在同一条物理路径上（同 IP 不同端口）；若确实不同路径而数值全同，需查 `connLatencyMs` 取值是否被 `default_conn` 串了 |
| `0ms` 与 `6ms` 在同一 peer 的不同会话间跳变 | sub-ms 保留为 0；6ms 是常态 | 0ms 那条是否落在本机回环/同机路径上 |

---

## 7. 核对方法（只读）

> 前端符号用 `文件::符号`（避免行号腐化）。§5 序 1–5/7/11 **已改、待验收**时，下列「改前结论」行请以代码当前行为为准，并对照 §5 进度表。

| 结论 | 复核点 |
|---|---|
| 延迟列测的是控制面一跳 | `peers/conn/peer_conn_ping.rs`（`pingpong` / `record_latency`）、`peers/conn/peer_conn.rs`（`WindowLatency::new(15)`） |
| 延迟列取整向上 | `frontend-lib/.../statusDisplay.ts::connLatencyMs`（`Math.ceil`） |
| 中转行延迟（改前=代价；改后优先 VIP） | `Status.vue::rowLatency` → `statusDisplay.ts::latencyMs`（VIP / `pathCostLabel` / 政策占位） |
| VIP 是数据面 ICMP | `instance/vip_probe.rs`；`build_echo_request`、`try_match_probe_reply` |
| VIP RTT core 取整向下；展示 `ceil`+`≈` | core：`vip_probe.rs`（`as_millis`）；UI：`statusDisplay.ts::formatVipRttMs` / `vipStateLabel` |
| 逐链路 vs 节点级分派 | `vip_probe.rs::collect_links`、`send_probe_msg_on_conn` / `send_probe_msg_by_ip` |
| 强制绑定 conn | `peer_manager.rs::send_probe_msg_on_conn` → `peer.send_msg_on_conn` |
| 回包由对端内核产生 | `peer_manager.rs` `NicPacketProcessor` / `not_send_to_tun` |
| 探测周期与并发 | `vip_probe.rs` `PROBE_INTERVAL`（20s）、`MAX_CONCURRENT_PROBES`（3）、round-robin `cursor` |
| 探测时间 tip（待验收已渲染） | `statusDisplay.ts::vipCheckedAtRelative`；`Status.vue::rowVipTip` |
| 探针不喂路由 | `vip_probe.rs` 模块不变量注释 |
| `[M/N]` 含中继节点级条目 | `statusDisplay.ts::vipLinkTotal`；roadmap `gui-connected-vs-reachable` §2.3 / §2.5 |
| 两张表一起下发 | `peer_ospf_route.rs::list_routes`（`cost/next_hop/path_latency` = A；`*_latency_first` = B） |
| 数据面按包头选表 | `peer_manager.rs::get_next_hop_policy` / `send_msg_internal` |
| 深路径降级 | `peer_manager.rs`（`forward_counter > 2` ⇒ 强制 LeastHop） |
| 有效 `latency_first`（含 p2p_only） | core：`peers/context.rs` `packet_policy`（`latency_first && !p2p_only`）；UI：`resolveRouteTableKind` |
| `latency_first` 默认值 | `config/toml.rs`（`latency_first: true`） |
| LeastHop 直连短路 | `peers/conn/peer_map.rs` |
| 前端同表选路（待验收） | `resolveRouteTableKind` / `routeHopCost` / `nextHopPeerId` / `pathLatencyCost`（LeastCost 消费 `cost_latency_first`） |

**抓包验证 §2.3 的 RTT 预算**（推荐，一次即可定性）：

1. 在对端节点抓 TUN 网卡与隧道网卡，过滤 `icmp`；
2. 对同一对端分别运行：`ping <对端VIP>`（走 OS/TUN）与观察 GUI 的 VIP 值；
3. 若两者量级接近而远大于「延迟」列 ⇒ 证实差值主要来自两端主机栈与 mesh 处理，而非链路。

---

## 8. changelog

| 日期 | 变更 |
|---|---|
| 2026-10-21 | 首次成文：厘清「延迟」/「VIP可达」的实际测量对象（一跳控制面 avg vs 端到端数据面单次）；给出中转可见性的三种拓扑与唯一盲区（有直连 conn 但实际走中转）；列出 10 条优化建议与 10 条缺陷（P0×5 / P1×3 / P2×2） |
| 2026-10-21 | 追补 §3.6「两个延迟该不该合并」：**歧义 A（延迟列 vs VIP列）不合并测量、只可合并单元格**；**歧义 B（延迟列内部）必须拆开**。附四条否决理由与「一格两行」的落地形态/验收方式 |
| 2026-10-21 | 追补 §3.7「延迟列能否显示中转延迟」：**能**（默认权重下 `path_latency ≈ Σ 各跳 RTT 平均`，`conn_select.rs:31-47/147-152`）。**同时修正 §0/§2.1/§2.2/§3.6.4 先前「不是毫秒」的过度表述** —— 实际是「毫秒当量 + 丢包/抖动罚分 + 占位值」。附丢包罚分量级表、`VIP ≥ 延迟` 判读规则、A/B/C 三条落地路径（B 可在纯前端用已有的 VIP 实测值替换） |
| 2026-10-21 | 追补 §3.8「把选路逻辑接进来」：**两张路由表**（LeastHop / LeastCost）由 `list_routes` 一起下发，数据面按**包头 `latency_first` 位**（默认 true）+ `forward_counter>2` 降级选表（`peer_manager.rs:199-204`、`:4695-4697`、`:5031-5060`）。给出**六种分叉矩阵**（§3.8.3），新增缺陷 **P1-4**（前端同行混用两表：跳数取 A 表、下一跳与延迟取 B 表，`cost_latency_first` 从未使用）与 **P1-5**（展示未反映数据面选表），新增优化序 **11**。并据此加固 §3.7.4：VIP 探测与本地用户流量同策略，故在四种分叉下比 `path_latency` 更准 |
| 2026-10-22 | 文档核对：P0-1 改为「已有 `vip_boundary_tip`、口径仍不足」；§3.8.5 / Q2 第三行限定「同路」仅 relay-only；序 4 依赖序 11 且需 export `nextHopPeerId`；序 5 落点改回序 1；排期改为 1→2→3→11→4→…；一格两行前置改为「不得渲染 `path_latency`」 |
| 2026-10-22 | 追补 **Q5 / §3.9**：两列**名义含义**（延迟=链路/路径时延；VIP=数据面通不通+业务往返）vs **实际显示** vs **一致性矩阵**；对齐目标态与「直连仍要两量、中转 B 后勿双显同一 VIP」；映射到 §5 序 1/2/3/4/11 |
| 2026-10-22 | **前端落地（待验收）**：§5 序 1/2/3/4/5/7/11；`formatVipRttMs` / `RouteTableKind` / 中转延迟用 VIP / 非在用路径灰点；vitest 更新。§0–§3 现状叙述待验收后再改写 |
| 2026-10-22 | 文档欠账：§7 改符号引用；§3.8.4 / P1-4/5 / 序 11 记 `cost_latency_first` 已用 + `p2p_only→LeastHop`（`peers/context.rs`）；§2.2/P0-2/3 标注改前 vs 待验收 |
| 2026-10-22 | 措辞欠账：§3.5 去掉「全文无 checked_at」错断言；Q5 / §3.7 / §3.7.1 / 矩阵 #4 标改前 vs 待验收 |
