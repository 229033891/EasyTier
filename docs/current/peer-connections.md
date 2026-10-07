# 节点间连接（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-07
- 范围：同一对 peer 之间的 `PeerConn` / 默认发送路径
- 规划中的多链路带宽聚合（异质优先，**未实现**）见：[`../roadmap/multi-link-bonding.md`](../roadmap/multi-link-bonding.md)
- 连接稳定性（质量选路 / 保底）见：[`../roadmap/connection-stability-todo.md`](../roadmap/connection-stability-todo.md)
- 隧道 scheme 与伪装差距见：[`tunnels-and-transport.md`](./tunnels-and-transport.md)
- 索引：[`../README.md`](../README.md)

本文只描述 **代码今天做什么**。

---

## 1. 术语

| 术语 | 含义 |
|------|------|
| **Peer** | 对端节点（按 `peer_id`） |
| **PeerConn** | 一条具体底层隧道（TCP / UDP / QUIC / WG / ring 等），绑定一条 `Tunnel` |
| **default_conn** | 该 Peer 上缓存的「当前用于发送」的那条 PeerConn |

同一对 peer 之间 **可以同时存在多条 PeerConn**（多 listener、多协议、打洞 + 直连、重连残留等）。  
安全侧可跨多条 PeerConn 复用 Peer 级会话（见 `easytier/docs/peer_conn_secure_mode_v3.md`）。

---

## 2. 发送路径：多连接，单活动出口

实现：`easytier-core` → `peers::conn::Peer::select_conn` / `send_msg`。

1. 若已有缓存的 `default_conn` 且仍可用 → **所有 `send_msg` 走这一条**。  
2. 否则在存活连接中按**综合质量分**（延迟 + 丢包 + 抖动）挑选一条（打洞连接在 ping 未确认前不会抢流量；高丢包路径可被熔断），写入 `default_conn`。  
3. **不会**按包或按流把流量分摊到多条 PeerConn 上。

因此：

- CLI / 状态里可能看到 `peer_conn_count > 1`。  
- **有效吞吐仍受当前默认那条隧道限制**。  
- 多连接今天的用途是 **路径冗余、选优、故障切换**，不是带宽聚合。  
- **热备条数 ≠ 聚合带宽**；状态里同时出现 tcp+udp 只表示多条隧道存活，**不会**按协议或宽带拆流发送。

### 2.1 链路度量（今日）

| 指标 | 来源 | 是否参与 `select_conn` | 状态面 |
|------|------|------------------------|--------|
| **RTT**（`latency_us`） | `WindowLatency` 窗口均值 | **是**（质量分一项） | `PeerConnStats.latency_us` |
| **丢包**（`loss_rate`） | Ping 0/1 窗口均值 | **是**（质量分 + 熔断） | `PeerConnInfo.loss_rate` |
| **抖动**（`jitter_us`） | 同 RTT 窗口的连续样本绝对差均值 | **是**（质量分一项） | `PeerConnStats.jitter_us` |
| **质量分**（`quality_score`） | `select_conn` 综合分（越低越好） | 状态面「质量分」列 + tooltip 分项 | `PeerConnInfo.quality_score` / `quality_fused`；★=`default_conn_id` |

同 peer 多 PeerConn 的 `select_conn`（`peers/conn/conn_select.rs`）使用综合质量分（默认 `w_lat=1` / `w_loss=4` / `w_jitter=1`），丢包超过熔断阈值（默认 20%）时在有替代路径时禁止成为 `default_conn`；切换需相对边际（默认 10%）**且**绝对分差（默认 0.005）连续窗口（默认 2，配合 5s 缓存清空）。已关闭的 PeerConn 不参与选路。权重/阈值可通过 `flags.conn_select_*` 覆盖（百分制权重；0 = 默认）。状态面展示每条隧道的 score / rtt / jitter / loss，并标注当前 `default_conn`。

跨 peer 的 OSPF：配置键 `latency_first`（UI：「质量优先选路」）开启时用 LeastCost（最小化质量边代价，可多跳）；关闭时用 LeastHop（先最少跳，同跳再比代价）。边代价由同一质量分编码进 peer-center `DirectConnectedPeerInfo.latency_ms`（`round(score*1000)`，熔断路径另加固定加成；发布端 `|Δ|<20` 不改写，配合约 60s 上报节流）。零丢包/抖动时量级仍≈ RTT 毫秒，与历史行为兼容。

### 2.2 拨号与选路（今日：全量拨号 + 质量选路，无档位）

| 项 | 今日 |
|----|------|
| **初始节点 URL** | `[[peer]]` / `peer_urls` / `public_server_url`（完整 tunnel URL），全部恒维持连接 |
| **硬约束** | `disable_p2p`（不主动直连）/ `p2p_only`（绝不中转）/ 允许中转开关（`disable_relay_data` 取反：**仅 OSPF 避让，不硬丢包**） |
| **遗留可选** | `prefer_peer_relay`：UI 隐藏；**不**抑制打洞；仅 TOML 可启用 OSPF 对端中继拓扑投影（与质量分正交） |
| **选优** | 同 peer 按 `select_conn` 综合质量分（RTT+loss+jitter）；对端间 OSPF 由 `latency_first`（质量优先选路）在 LeastHop / LeastCost 间切换，LeastCost 边代价同源编码进 `latency_ms`；`lazy_p2p` 减少无业务时的背景打洞 |
| **Web 控件** | `Config.vue` 基础设置「允许作为中转节点」；高级设置「质量优先选路」(`latency_first`)；`disable_p2p` 在高级设置 |

示例（有序列表；**443 不一定可用**，按实际可达端口填写）：

```toml
[[peer]]
uri = "tcp://home-relay.internal:5000"
[[peer]]
uri = "wss://relay.example.com:8443/et"
[[peer]]
uri = "wss://relay.example.com/et"   # 隐式 443；仅当网络放行 443 时作为备选
```

---

## 3. 与运营商「单连接限速」的关系

部分网络对单条 TCP/UDP 流有带宽上限。  
在现状模型下，即使两端之间有多条 PeerConn，数据面仍只使用 `default_conn`，**无法靠多连接叠加带宽**。

---

## 4. 缺口（非今日行为）

以下 **尚未实现**；读者与 UI 文案不得写成 Current 能力：

| 缺口 | 说明 | Roadmap |
|------|------|---------|
| 多链路并行发送（bonding） | `bond_count>1` 时按流哈希分摊到多条 PeerConn | [`../roadmap/multi-link-bonding.md`](../roadmap/multi-link-bonding.md) |
| 异质成员集 | 指标大致相当时优先不同协议 / 出口 / remote；不足时同质补齐（每类约 3–5） | 同上（`diversity-first, replica-fill`） |
| 双宽带自动拆流 | Socket 选项层虽有 `bind_device`（指定发包网卡），无「按多网卡维持 N 条并计入 diversity」的控制器 | 同上 Phase 3 |

今日已有的多协议并存、质量选 `default_conn`、断链后切热备，**不等于**上表能力。

---

## 5. 验收对照（现状）

| 场景 | 期望（今天） |
|------|----------------|
| 两节点仅一条存活隧道 | 全部流量走该隧道 |
| 两节点多条存活隧道 | 状态可列出多条；发送仍只走 `default_conn`（通常为质量分更优者）；热备 ≠ 聚合 |
| 默认隧道断开 | 重新 `select_conn`，切到另一条存活连接（若有） |
| 希望 N 条并行加带宽 / 按协议或宽带拆流 | **未实现**（见 §4） |

> 质量选路相关单元验收已合入；bonding 仅文档立项。非 443 保底组网（P0.5）与双路径现网抽样（P1.8b）待补，见 [`../roadmap/connection-stability-todo.md`](../roadmap/connection-stability-todo.md)。
