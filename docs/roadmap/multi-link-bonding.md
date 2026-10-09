# 多链路带宽聚合（PeerConn Bonding）

## Status

- Status: **Roadmap**（Phase **2a/2b 已合入**；**剩余：Phase 3** 出口/`bind_device` 多样性；默认 `bond_count=1`）
- 日期：2026-10-07
- 最近审阅：2026-10-09（Phase 2 已落地；日常行为见 Current `peer-connections.md`）
- 索引：[`../README.md`](../README.md)
- **现状行为**：[`../current/peer-connections.md`](../current/peer-connections.md)（默认单 `default_conn`；可选 bonding）
- **代码锚点**：`easytier-core/src/peers/conn/conn_bond.rs`、`peer.rs`（`select_bond_conns` / `send_msg`）
- **连接稳定性 / 质量门**：[`connection-stability-todo.md`](./connection-stability-todo.md)（综合分、熔断；与 bonding **正交**，默认 `bond_count=1`）
- **市场对比**：[`market-comparison-2026-10.md`](./market-comparison-2026-10.md)（ZeroTier Multipath；Tailscale 仍单路径；VeloCloud DMPO 为站间 SD-WAN）

发送面 MVP 对齐 **ZeroTier Multipath 的 balance-xor（按流哈希）**。  
成员面 **不对标** 华为/VeloCloud 包级 DMPO；目标是降低共模故障 + 突破单连接限速，不是站点 SD-WAN。

> **日常阅读**：Phase 2a/2b 行为以 [`../current/peer-connections.md`](../current/peer-connections.md) 为准。下文 §1–§7 保留设计论证；**待实现工作见 §5 Phase 3**。

---

## 1. 背景与问题

今天同一对 peer 可有多条 `PeerConn`，但发送只走缓存的 `default_conn`（见 Current）。  
当运营商或中间设备限制 **单条连接** 的带宽时，用户无法通过「多开几条隧道」提升吞吐。

同时：若 N 条隧道都是 **同一出口、同一协议、同一 remote** 的简单复制，则共模故障（某一宽带抖动、某一 UDP 路径丢包）仍会一起倒下，对稳定性帮助有限。

目标：在正确性可接受的前提下，允许同一对 peer 上 **并行使用多条链路**，提高聚合带宽；成员集 **优先异质路径**，不足时再用同质复制补齐；失败时仍可回退到单链路（`bond_count=1`）。

---

## 2. 目标

1. 可配置：全局（或指定 peer）`peer_link_bond_count` / 等价 flags，目标并行数 `N`（**默认 1** = 今日行为；建议硬顶 **5**）。  
2. 维护最多 `N` 条参与发送的 PeerConn（主动维持见 Phase 2b；Phase 2a 先用已存活连接）；成员选择见 §3（**diversity-first, replica-fill**；同质每类最多约 **3–5**）。  
3. 数据面按流哈希分摊（§4），使 **多并行流** 总吞吐有机会超过单连接上限。  
4. 链路增减时不破坏现有加密 / anti-replay（沿用 Peer 级 `PeerSession` JOIN；见 `easytier/docs/peer_conn_secure_mode_v3.md`）。  
5. 可观测：每条链路的速率、RTT/loss/jitter、是否在 bond 集、diversity 键摘要。

非目标（首期）：

- 不替代多跳 OSPF 选路；bonding 仅作用于 **直连下一跳** 的多条 PeerConn。  
- 不承诺线性翻倍（运营商可能限「同五元组组」或用户总流量）。  
- 不做跨 peer 的 ECMP 带宽叠加（路由问题，另议）。  
- **不**承诺单条大象流（含单个 RDP/单 TCP）因 bonding 变快——同流粘滞在一条 conn 上。  
- **不**做包级喷洒 / DMPO。  
- Current / UI **不得**在代码落地前写「已支持双宽带自动拆流」。

---

## 3. 成员策略：异质优先、同质补齐

策略名：**`diversity-first, replica-fill`**。

```text
candidates → quality_gate(RTT/loss/jitter) → diversity_pick → replica_fill → bond_set(|S|≤N)
                                                                              ↓
                                                                    per-flow hash send
```

### 3.1 质量门（先于多样性）

- 候选须通过延迟 / 丢包 / 抖动门槛；默认 **复用** `select_conn` 综合分与丢包熔断语义（[`../current/peer-connections.md`](../current/peer-connections.md) §2.1）。  
- 熔断路径（`quality_fused`）在有替代时 **默认不进** bond 集。  
- 未验证打洞连接不抢 bond 成员位（与今日「未确认不抢 `default_conn`」一致）。  
- 实现期可为 bond 另设略宽/略严阈值，但不得绕过「高丢包饿死业务」原则。

### 3.2 何时异质优先（指标大致相当）

**拍板**：多条候选的延迟 / 丢包 / 抖动（或综合质量分）**大致在同一档**时，成员选择 **先做 diversity，再比细微分差**——避免「略好 1ms 的同质路径」占满 bond 集、挤掉协议/出口不同但指标接近的路径。

| 情形 | 行为 |
|------|------|
| 多条线路指标大致相当（同质量档） | **异质优先**：覆盖更多 scheme / 出口 / remote |
| 某条明显更差（高出质量档或触发熔断） | 不进或淘汰；不因「异质」强行纳入差路径 |
| 异质候选仍不足 `|S|<N` | **同质补齐**（§3.4），每类上限 3–5 |

「大致相当」实现草案（验收锚点，实现期可微调）：

- 以综合质量分为准；相对最优候选，相对差 **≤ `conn_select` 切换相对边际**（今日默认约 **10%**）且未熔断 → 视为同质量档。  
- 可选放宽：实现期若档过窄导致几乎总是同质占满，可试 **1×～2×** 该边际，并在单测/现网记录所选倍数。  
- 绝对分差阈值可与 `conn_select` 绝对边际对齐或略宽；**文档固定语义，数值以 Flags 为准**。

### 3.3 Diversity 键（按优先级）

| 优先级 | 键 | 含义 | 今日代码基础 |
|--------|-----|------|----------------|
| 1 | `tunnel_scheme` / 协议类 | udp / tcp / wg / quic / ws(s) 等 | 多 scheme 已可并存（状态面常见 tcp+udp） |
| 2 | underlay 出口 | `bind_device` 或本地源 IP（宽带 A vs B） | Socket 层已有 `bind_device`；**缺**按多网卡主动拨号并计入 diversity 的控制器 → Phase 3 |
| 3 | remote 地址类 | 不同 peer URL / 不同公网下一跳 | 多 `[[peer]]` URL、打洞 vs 直连等已可产生不同 remote |

挑选规则（草案）：在已过质量门、且处于**同质量档**的集合中，贪心最大化「尚未覆盖的 diversity 键」覆盖数；档内再按质量分优劣打破平局，直到 `|bond_set| == N`、同质类触顶、或候选耗尽。

**意图**：指标差不多时，N 条连接尽量不是「一条线路的简单复制」，而是例如 UDP + TCP、或不同出口/不同 remote 的组合，以降低共模故障、提高稳定性。

### 3.4 Replica-fill（同质补齐）

当异质候选不足以填满 `N` 时：

- **允许**同协议、同出口、甚至相近 remote 再开/再纳入并行隧道补齐。  
- 同质复制仍可能帮助「单连接限速」场景（多五元组绕过 per-flow 限速）。  
- **约束次序**：先满足全局 `|bond_set| ≤ N`（`bond_count`，硬顶建议 **5**），再满足每类同质上限；成员挑选时 **先占异质槽，再同质补齐**。  
  例：`N=5` 且已有 udp+tcp 两条异质成员时，同质最多再补 3 条，不会每类各开到 5。  
- **同质每类上限**：建议配置范围 **3–5**，默认 **`replica_fill_max=5`**；允许更小（如 1–2）做保守灰度。计入同一 diversity 键组合（同 scheme + 同出口 + 同 remote 类）的副本数，达到上限后不再为该类加开。  
- 另受拨号退避、`alive_client_urls` / 去重约束，避免连接风暴。  
- 仅有一条可用 underlay 时，**允许** bond 集在 `min(N, replica_fill_max)` 内全为同质成员（验收须覆盖，不得视为失败）。

### 3.5 可行性小结

| 维度 | 结论 |
|------|------|
| 多 PeerConn 并存 | 今日已有；热备成立，**非**聚合 |
| 指标相当 → 异质优先 | 同质量档内先 diversity，再比细微分差 |
| 协议 / remote 异质 | **Phase 2a MVP** 可做 |
| 出口 / 多宽带异质 | **Phase 3**；落地前文档与 UI 不宣称已支持 |
| 同路径复制 | 允许作 fill；每类 **3–5**（默认 5）；须防风暴 |
| 稳定性 | 异质降共模；MVP **禁止**同流跨 conn；默认 `N=1` 可回退 |

---

## 4. 发送面分摊策略

| 策略 | 说明 | 利弊 |
|------|------|------|
| **按流哈希（MVP 必选）** | 对内层五元组或 flow id 哈希到 bond 集内某条 PeerConn | 同流有序，对 TCP/RDP 友好；单大象流仍吃单连接上限 |
| 按包轮询 | 包级 stripe | 易乱序，伤 TCP；**首期禁止** |
| 加权按 RTT/丢包 | 动态调权选成员或调流量 | Phase 3；与质量门/熔断协同 |

MVP：**按流哈希 + diversity-first 成员集（不足 replica-fill）**；`bond_count=1` 时行为与 Current 完全一致（单 `default_conn`）。

成员断开：该流重哈希到剩余成员或回退单路径，**无长时间黑洞**（见 §7）。

---

## 5. 分阶段

### Phase 1 — 可观测与「准 bonding」基案

- 状态面列出各 PeerConn、`default_conn`（★）、质量分分项（已大部分落地，见稳定性 TODO P2.1）。  
- 文档 / UI：**多连接 ≠ 已聚合带宽**；热备条数 ≠ 聚合带宽（Current + 稳定性 P2.2 **已完成**）。

### Phase 2a — 发送 MVP（第一刀）✅

1. 配置：`peer_link_bond_count`（默认 **1**，硬顶 **5**）；`peer_link_replica_fill_max`（默认 **5**）。  
2. `send_msg`：`N==1` → 今日 `default_conn`；`N>1` → 对已有多 conn **按流**选 bond 成员。  
3. 成员集：质量门 → **同质量档内**协议 / remote 多样性优先 → 不足则同质补齐（每类 ≤ `replica_fill_max`）。  
4. 尚不强制「主动狂开」新隧道；优先用已存活的异质连接。

### Phase 2b — 主动维持 N + 风暴控制

1. 对直连 peer 主动维持最多 N 条存活隧道（与 hole punch / `lazy_p2p` / alive-URL 去重协同；**已合入**：`bond` 未满的 peer 继续参与 direct 拨号，沿用既有 5s 周期与拨号内退避）。
2. 拨号退避、上限、失败冷却（沿用 direct 既有 `[1,2,2,5,5,10,30,60]s` 内退避 + 5s 外层周期）。
3. 状态面标注「in bond set」与 diversity 键摘要（**已合入**：`PeerConnInfo.in_bond_set` / `bond_class`；CLI 主表与 GUI 状态表展示）。

### Phase 3 — 出口多样性与增强

- 多网卡 / `bind_device`（宽带 A vs B）纳入 diversity 并主动拨号。  
- 按链路质量加权；坏链路从 bond 集熔断。  
- 与蜂窝+Wi‑Fi 等多路径协同。  
- 单流加速（需可乱序信道或应用层拆流）——**单独评估，非默认**。

---

## 6. 风险

| 风险 | 缓解 |
|------|------|
| TCP / RDP 乱序 | MVP 仅按流哈希，禁止同流跨 conn |
| 共模故障（N 条同路径复制） | 同质量档内 diversity-first；同质每类封顶 3–5 |
| 加密 / nonce / 重放 | 沿用 PeerSession 跨 conn；并行发送做 stress |
| 运营商按用户总限速 | 声明「不保证翻倍」；验收用可控单连接限速环境 |
| 连接风暴 | 硬顶 N≤5、`replica_fill_max`、退避、alive-URL 去重 |
| 与 `select_conn` / 打洞打架 | `N=1` 路径不变；bonding 开启时发送走 bond 集，质量门仍共用 |
| 预期误读（RDP 变快） | 明确单流不加速；RDP 丢包卡顿仍靠质量选路 |

---

## 7. 验收要点（实现时）

1. `bond_count=1`：行为与 [`../current/peer-connections.md`](../current/peer-connections.md) 一致（零回归）。  
2. `bond_count=N>1`：状态可见至多 N 条参与发送；多并行流总吞吐可高于单连接人为限速。  
3. **异质场景（指标相当）**：多种 scheme/remote 均在同质量档（§3.2 锚点）时，bond 集 **不应** 被单一同质路径占满而挤掉其它达标异质路径（除非异质候选本就不足）。  
4. **单 underlay / 同质补齐**：仅一条可用线路时，允许在 `min(N, replica_fill_max)` 内同质填满；不得因此判失败。  
5. 同一 diversity 键组合的副本数不得超过 `replica_fill_max`；且始终 `|bond_set| ≤ N`（先异质后同质）。  
6. 断开其中一条：进行中流重哈希或回退后仍可达，无长时间黑洞。  
7. 关闭 bonding / `N=1`：回到单 `default_conn`。  
8. 熔断高丢包路径不进入（或尽快退出）bond 集（有替代时）。

---

## 8. 结论

- **需求成立**：现状多连接不能解决单连接限速；纯同质复制也不足以最大化稳定性。  
- **成员语义已拍板**：指标大致相当时 **异质优先**；不足则 **同质补齐**（每类一般 **3–5**，默认 5）。  
- **当前状态**：Phase **2a**（按流发送 + diversity 成员集）与 Phase **2b**（主动补链 + 状态标注）已合入；默认 `bond_count=1`。出口/`bind_device` 多样性仍属 Phase 3。  
- sticky key 须在压缩/加密前计算（`ZCPacket.bond_flow_key`），避免经典 AEAD 路径按包喷洒。
