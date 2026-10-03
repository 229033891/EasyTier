# 多链路带宽聚合（PeerConn Bonding）

## Status

- Status: **Roadmap**（Draft / 仅立项，尚未改代码）
- 最近审阅：2026-10-03
- 索引：[`../README.md`](../README.md)
- **现状行为**：[`../current/peer-connections.md`](../current/peer-connections.md)
- **市场对比**：[`market-comparison-2026-10.md`](./market-comparison-2026-10.md)（ZeroTier Multipath；Tailscale 仍单路径；VeloCloud DMPO 为站间 SD-WAN）

MVP 语义对齐 **ZeroTier Multipath 的 balance-xor（按流哈希）**；  
**不对标** 华为/VeloCloud 站点多 WAN / DMPO 包级优化。

---

## 1. 背景与问题

今天同一对 peer 可有多条 `PeerConn`，但发送只走缓存的 `default_conn`（见 Current）。  
当运营商或中间设备限制 **单条连接** 的带宽时，用户无法通过「多开几条隧道」提升吞吐。

目标：在正确性可接受的前提下，允许同一对 peer 上 **并行使用多条链路**，提高聚合带宽；失败时仍可回退到单链路。

---

## 2. 目标

1. 可配置：对指定 peer（或全局）启用多链路发送（例如目标并行数 `N`）。  
2. 主动维护最多 `N` 条可用 PeerConn（同协议或多协议策略待定）。  
3. 数据面按策略分摊流量（见 §3），使总吞吐有机会超过单连接上限。  
4. 链路增减时不破坏现有加密 / anti-replay（继续 Peer 级会话或明确 per-conn 边界）。  
5. 可观测：每条链路的速率、RTT、是否参与 bonding。

非目标（首期）：

- 不替代多跳 OSPF 选路；bonding 仅作用于 **直连下一跳** 的多条 PeerConn。  
- 不承诺线性翻倍（运营商可能限「同五元组组」或总流量）。  
- 不做跨 peer 的 ECMP 带宽叠加（那是路由问题，另议）。

---

## 3. 推荐分摊策略（默认建议）

| 策略 | 说明 | 利弊 |
|------|------|------|
| **按流哈希（推荐 MVP）** | 对内层五元组或 flow id 哈希到某条 PeerConn | 同流有序，对 TCP 友好；单大象流仍吃单连接上限 |
| 按包轮询 | 包级 stripe | 易乱序，伤 TCP；仅适合可乱序友好负载或自研可靠层 |
| 加权按 RTT/丢包 | 动态调权 | 更优，实现与调参更重 |

MVP 建议：**按流哈希 + 主动维持 N 条同类型隧道（优先 UDP/QUIC，TCP 次之）**。

---

## 4. 分阶段

### Phase 1 — 可观测与「准 bonding」基线

- 状态面明确列出：各 PeerConn、`default_conn`、是否仅冗余。  
- 文档 / UI 帮助写清：多连接 ≠ 已聚合带宽（与 Current 一致）。

### Phase 2 — 主动多隧道 + 按流分摊（MVP）

1. 配置项（草案）：`peer_link_bond_count` 或等价 flags（默认 1 = 今日行为）。  
2. 对直连 peer 维持最多 N 条存活隧道。  
3. `send_msg`：`N==1` 保持今日 `default_conn`；`N>1` 按流选 conn。  
4. 基础测试：人工限速单连接时，多流业务总吞吐可超过单连接；单 TCP 流不因乱序明显退化。

### Phase 3 — 增强（可选）

- 按链路质量加权；坏链路熔断。  
- 与打洞 / 多路径（蜂窝+Wi‑Fi）协同。  
- 单流加速（需可乱序信道或应用层拆流，单独评估）。

---

## 5. 风险

| 风险 | 缓解 |
|------|------|
| TCP 乱序 | MVP 仅按流哈希，禁止同流跨 conn |
| 加密 / nonce / 重放 | 沿用 PeerSession 跨 conn 语义；实现前对照 secure mode 文档 |
| 运营商按用户总限速 | 文档声明「不保证翻倍」；验收用可控限速环境 |
| 连接风暴 | 上限 N、退避、与现有 alive URL 去重逻辑对齐 |

---

## 6. 验收要点（实现时）

1. `bond_count=1`：行为与 [`../current/peer-connections.md`](../current/peer-connections.md) 一致。  
2. `bond_count=N>1`：状态可见 N 条（或尽量接近）参与发送；多并行流总吞吐可高于单连接人为限速。  
3. 断开其中一条：进行中流迁移或重哈希后仍可达，无长时间黑洞。  
4. 关闭 bonding：回到单 `default_conn`。

---

## 7. 结论

- **需求成立**：现状多连接不能解决单连接限速。  
- **当前状态：仅文档立项，暂不实现代码。**  
- 启动时以 Phase 2 为第一刀，并先更新 Current 中的发送路径说明。
