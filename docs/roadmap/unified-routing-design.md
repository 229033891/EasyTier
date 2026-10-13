# 统一路由模型（CIDR / 域名 → 节点）

## Status

- Status: **Roadmap**（Design / 开放问题已拍板；**前置 P-T1 已完成**；2026-10-13 **规格自洽修订**后可择机启动）
- 最近审阅：2026-10-13（对照代码复核：热路径/CIDR LPM/P3 计划与合规边界已修正）
- 范围：把**子网代理**、**出口节点**、**域名路由**统一为一个模型 —— **A 上配置路由，B 做网关**
- 现状依据：[`../current/traffic-steering.md`](../current/traffic-steering.md)（流量导流 SoT）
- DNS 现状：[`../current/magic-dns.md`](../current/magic-dns.md)
- 相关草案：[`domain-proxy.md`](./domain-proxy.md)（peer-driven 方案，见 §12）
- **冲突分析**：[`unified-routing-conflicts.md`](./unified-routing-conflicts.md)（已核查；**C1/C2 为设计缺陷，§9.8 已修正**）
- **前置项**：[`routing-tech-debt.md`](./routing-tech-debt.md) —— **P-T1 已完成**；TD-1/4/5/7 暂不实施
- 索引：[`../README.md`](../README.md)

> **§11 的开放问题已全部拍板。** 前置 P-T1 已满足。  
> ⚠️ **§9.8 已按冲突分析 C1/C2/C5 重写**。  
> ⚠️ **2026-10-13 规格修订**：统一热路径为「快照只读 `active_peer`」；补齐 CIDR LPM；重写 §15.4；合规目标降为「尽力出口 + 已知泄漏面」（见 §1.1 / §12.6）。

---

## 目录

1. [需求](#1-需求)
2. [术语澄清：L2 入口层 / L3 选路层](#2-术语澄清l2-入口层--l3-选路层)
3. [现状核对（含代码锚点）](#3-现状核对含代码锚点)
4. [统一模型](#4-统一模型)
5. [配置模型](#5-配置模型)
6. [策略语义](#6-策略语义)
7. [数据流与状态机](#7-数据流与状态机)
8. [与现有优先级表的关系](#8-与现有优先级表的关系)
9. [实现设计（含伪代码）](#9-实现设计含伪代码)
10. [歧义处理](#10-歧义处理)
11. [开放问题](#11-开放问题)
12. [企业级要求](#12-企业级要求)
13. [依赖图与可交付分支](#13-依赖图与可交付分支)
14. [风险矩阵](#14-风险矩阵)
15. [分阶段实施](#15-分阶段实施)
16. [验收场景](#16-验收场景)
17. [需要同步修改的文档](#17-需要同步修改的文档)

---

## 1. 需求

### 1.1 背景

**场景**：跨国团队需要访问海外 SaaS（Atlassian、AWS、Oracle Cloud 等），同时国内业务必须走本地网络。

**约束**：

| # | 约束 | 性质 | 本设计可达性 |
|---|---|---|---|
| **C1** | 特定域名（Atlassian）宜从指定国家出口 | 产品强诉求 | ⚠️ **尽力**：连接可按规则走指定 peer；**DNS 上游与首包窗口仍可能直连**（§9.8.4 / 冲突 C6）。**首期不做零泄漏合规保证** |
| **C2** | 国内流量不能绕远路 | 性能 | ✅ CIDR/域名分流可满足 |
| **C3** | 海外域名清单与 CDN IP 持续变动 | 运维 | ✅ 订阅 MagicDNS `resolve()` 跟随 CDN（非手工 CIDR） |

**为什么现有做法不够**：

| 做法 | 失败原因 |
|---|---|
| 全部走出口节点 | 违反 **C2**（国内也绕远） |
| 全部直连 | 海外慢，且无法声明「这些域名优先走指定出口」 |
| 手工维护 `manual_routes` CIDR | 违反 **C3**：CDN IP 天天变，改一次错一次 |

**根因**：现有分流粒度只能到「IP/CIDR」或「全局出口列表」，**无法表达「哪些域名走哪个节点」**。

> **合规边界（必须写进产品文档）**：本设计交付的是「**策略分流 / 可运维出口**」，不是「审计级零泄漏专线」。若监管要求 DNS 与首包也不得出本地 ISP，需另立项（DNS-over-exit、未就绪流阻断等）。

---

### 1.2 目标

| # | 目标 |
|---|---|
| **G1** | **域名级 / CIDR 级策略路由** —— 每个目标可指定走哪个节点 |
| **G2** | **B 侧零配置** —— 目标节点只开一个开关，不感知域名、不解析 DNS |
| **G3** | **策略集中在 A** —— 便于审计与管控 |
| **G4** | **企业级运维品质** —— 热路径可验证、可观测、可故障转移（§12.5）；**不含**零泄漏合规保证（§12.6） |

---

### 1.3 非目标（明确不做）

| # | 不做 | 原因 |
|---|---|---|
| **N1** | 访问控制 / 权限 | **交网关侧防火墙处理**（已确认） |
| **N2** | 配置中心下发 | **首期人工配置**（已确认），后续再议 |
| **N3** | IPv6 / AAAA | 首期仅 IPv4（Q8） |
| **N4** | 无缝切换 / 连接迁移 | 三层网络模型固有限制，见 §12.3 |
| **N5** | 站点到站点 VPN、ZTNA 到内网 | 不在本文范围 |
| **N6** | 出口节点主动探测 / 选址 | 仅用既有 cost 排序，不做主动探测 |

---

### 1.4 功能需求

| # | 需求 | 实现 | 验收场景 |
|---|---|---|---|
| **FR-1** | 指定 CIDR 走指定节点 | `routes[].cidr` + L3 次序 3.5 | 2 |
| **FR-2** | 指定域名走指定节点 | `routes[].domains` + DNS 编译 | 4、10、11 |
| **FR-3** | 多候选节点 + 优先级故障转移 | `peers[]` 顺序 + `select_by=priority` | 5 |
| **FR-4** | 全部候选不可用时的 per-rule 策略 | `if_all_unavailable = open\|closed` | 6、7 |
| **FR-5** | 三种选点策略（平级、per-record） | `select_by = priority\|quality\|balance` | 8、9 |
| **FR-6** | 三种切换时机 | `failover_when = unreachable\|degraded\|never` | 5、8 |
| **FR-7** | B 侧零配置 | 复用 `enable_exit_node`（§4.2） | 4 |
| **FR-8** | `exit_nodes` 向后兼容 | 降级为语法糖（§5.3） | 3、12 |
| **FR-9** | 可观测 | CLI + UI 展示生效规则与状态 | 全部可查 |
| **FR-10** | 合规风险显式化 | `require_confirmed` 硬失败（Q7） | 14 |

### 1.5 非功能需求

| # | 需求 | 指标 | 验证 |
|---|---|---|---|
| **NFR-1** | 热路径零锁零分配零 async | `lookup_peer()` 无锁、无 `.await`、alloc = 0 | §16.2 P1、P2 |
| **NFR-2** | 不劣化现状性能 | **10K** 规则相对 1K 劣化 < 20%（100K stretch） | §16.2 P3、P4 |
| **NFR-3** | 快速故障转移 | 掉线 → 新流切换 **≤ 2 秒** | §16.2 P6 |
| **NFR-4** | 切换无黑洞 | L2/L3 原子切换 | §16.2 P7 |
| **NFR-5** | **零回归** | 未配置时行为**完全一致** | 场景 1 |
| **NFR-6** | 配置错误必须显式失败 | 不静默取默认 | 场景 13、14 |
| **NFR-7** | 长时间稳定 | 24h 压测无内存增长、无路由震荡 | §16.2 P8 |

### 1.6 当前能力与差距

| 现有 | 能做 | 不能做 |
|---|---|---|
| 子网代理 | B 宣告网段 → A 自动装路由 | 策略在 B，A 零配置 |
| 出口节点 | A 配 `exit_nodes` → 全流量走同一出口 | 无法按 CIDR / 域名区分 |
| `manual_routes` | 手工 IP 列表 → L2 精确控制 | **整表覆盖**干掉 peer 网段；IP 手工维护 |
| `DnsConfig.hosts` | 域名 → IP 映射 | **不参与路由** |

---

### 1.7 最终形态（用户视角）

**Before（现状）**

```toml
# 全部流量走美国 —— 违反 C2
exit_nodes = ["10.144.0.9"]

# 想只让 atlassian 走美国，必须手工维护 CIDR —— 违反 C3
manual_routes = ["104.18.0.1/32", "104.18.0.2/32", "172.64.1.0/24"]
```

**效果**：CDN 换 IP 后策略立即失效；且 `manual_routes` 会连带干掉 peer 宣告的内网网段。

**After（本设计交付后）**

```toml
[[routes]]
domains            = ["*.atlassian.net"]
peers              = ["10.144.0.9"]       # 美国节点
select_by          = "priority"
failover_when      = "unreachable"
if_all_unavailable = "closed"             # 不通就断，不降级本地直连
require_confirmed  = true

[[routes]]
domains            = ["oraclecloud.com"]
peers              = ["10.144.0.10"]      # 另一个出口
select_by          = "priority"
failover_when      = "unreachable"
if_all_unavailable = "open"

[[routes]]
cidr               = "198.51.100.0/24"    # 也可以纯按网段
peers              = ["10.144.0.11", "10.144.0.12"]
select_by          = "quality"
failover_when      = "degraded"
if_all_unavailable = "open"
```

**目标主机 B 侧**：只需原有的一个开关，**不感知域名、不解析 DNS**。

```toml
enable_exit_node = true
```

**效果对比**

| 维度 | Before | After |
|---|---|---|
| 域名级分流 | ❌ 只能手工维护 IP | ✅ 域名声明 |
| CDN 换 IP | ❌ 策略失效 | ✅ 跟随 MagicDNS 实际解析（观测 A 记录） |
| 与内网代理共存 | ❌ `manual_routes` 覆盖掉 | ✅ 累加 |
| 出口失败策略 | ⚠️ 静默失效 | ✅ `closed` 不通即断（**不**等于零泄漏） |
| 多出口 | ❌ 只有全局列表 | ✅ per-record 声明 |
| 出口不可达时 | ⚠️ 静默失效 | ✅ 明确 warn / 阻断 |

### 1.8 需求追溯

| 需求 | 验收场景 |
|---|---|
| FR-1 | 2 |
| FR-2 | 4、10、11 |
| FR-3 | 5 |
| FR-4 | 6、7 |
| FR-5 | 8、9 |
| FR-6 | 5、8 |
| FR-7 | 4 |
| FR-8 | 3、12 |
| FR-10 | 14 |
| NFR-5 | 1 |
| NFR-6 | 13、14 |
| NFR-1/2/3/4/7 | §16.2 P1–P8 |

---

## 2. 术语澄清：L2 入口层 / L3 选路层

> ⚠️ **不是 OSI 模型的二层/三层。** 本文沿用 `traffic-steering.md` §1 的措辞，更准确地说是「**入口层**」和「**选路层**」。

### 2.1 比喻

```
包裹寄到北京（目标 IP 8.8.8.8）

【L2 入口】= 楼下门禁——「这栋楼收不收？」
    包裹在门口被刷掉 → EasyTier 完全不知情

【L3 选路】= 楼内分拣 ——「进来了交给哪个部门？」
    进了楼但没人认识 → 卡住
```

### 2.2 具体链路

```
A 的应用：connect(8.8.8.8)
     │
     ▼
┌──────────────────────────────────────────────┐
│ 操作系统路由表← 【L2 入口层】   │
│  8.8.8.8 匹配 0.0.0.0/0 via TUN     ✅       │
│  （EasyTier 只是往这里装条目）               │
└───────────────────┬──────────────────────────┘
                    │ 包进入 TUN
                    ▼
┌──────────────────────────────────────────────┐
│ PeerOutboundPacketRouter      ← 【L3 选路层】│
│  get_msg_dst_peer_ipv4(8.8.8.8)              │
│  → 命中本地路由表 → peer B│
└───────────────────┬──────────────────────────┘
                    │ 加密包发给 B
                    ▼
              B 收到 → NAT → 出网
```

### 2.3 两者区别

| | L2 入口层 | L3 选路层 |
|---|---|---|
| 谁决定 | **操作系统**（EasyTier 只负责装条目） | **EasyTier 自己** |
| 干什么 | 决定包**进不进**隧道 | 决定包**交给谁** |
| 现状配置来源 | **B 推送**（peer 通告网段自动装） | **A 配置**（`exit_nodes`） |
| 代码位置 | `easytier` → `virtual_nic` → `apply_route_changes` | `easytier-core` → `PeerOutboundPacketRouter` |

### 2.4 常见误区

> 「A 加了 OS 路由，包能到 B 吗？」
>
> **不能保证。** 加 OS 路由只做了 L2。L3 还要 EasyTier 知道「8.8.8.8 该交给 B」。

**本文的统一模型，L2 与 L3 都由 A 侧同一份配置驱动。**

---

## 3. 现状核对（含代码锚点）

> 以下行号对应 2026-10-13 的 `main`。实现前请以符号名为准重新定位。

### 3.1 既有三种能力分别在哪一层

| 功能 | L2 入口层 | L3 选路层 |
|---|---|---|
| 子网代理 | B 宣告网段 → A 自动装 | 同左，LPM 自动命中 |
| 出口节点 | A 有 `exit_nodes` → **自动装 TUN `/0`** | `exit_nodes` 列表顺序 |
| 域名路由 |域名解析 → 装 `/32`（要新建） | 域名 → 节点（要新建） |

### 3.2 B 侧本就是统一的（关键证据）

```rust
// easytier-core/src/gateway/proxy/tcp_proxy_engine.rs:235-244
pub fn check_packet_from_peer_fast(
    &self, mode: TcpProxyMode, ctx: &TcpProxyPeerContext,
) -> bool {
    match mode {
        TcpProxyMode::Tcp => !self.cidr_table.is_empty() || ctx.enable_exit_node || ctx.no_tun,
        //           ↑ 子网代理触发         ↑ 出口节点触发        ↑ 本机出口
        TcpProxyMode::KcpSrc | TcpProxyMode::QuicSrc => true,
    }
}
```

**三个触发条件是 OR 关系，同一个代理引擎。** B 上不存在「子网代理模式」和「出口节点模式」两套东西 —— **B 只要开着，就同时具备两种能力。**

> 注：这是**快速路径门禁**（`check_packet_from_peer_fast`）。慢速路径另有分支（`try_handle_peer_packet`，`:246`）。两者共同决定是否代理。

差异仅在包上 `is_exit_node` 标志影响目标地址解析（mapped-CIDR 场景，见 `tcp_proxy_engine.rs:291`）。

ICMP 引擎同构（`icmp_proxy_engine.rs:70`）。

### 3.3 链路质量评分模型已存在

```rust
// easytier-core/src/peers/conn/conn_select.rs:96-107
/// Composite quality score (lower = better).
pub fn conn_quality_score(
    latency_us: u64, loss_rate: f64, jitter_us: u64, cfg: &ConnSelectConfig,
) -> f64 {
    cfg.w_lat * norm_rtt + cfg.w_loss * norm_loss + cfg.w_jitter * norm_jitter
}
```

默认权重（`easytier-core/src/config/toml.rs:162-164`）：

| 维度 | 配置项 | 默认值 | 归一化权重 |
|---|---|---|---|
| 延迟 RTT | `conn_select_w_lat` | 100 | 1.0 |
| **丢包 loss** | `conn_select_w_loss` | 400 | **4.0** |
| 抖动 jitter | `conn_select_w_jitter` | 100 | 1.0 |

设计意图（`conn_select.rs:3`）：

> Lower score is better. **Defaults: loss weight dominates slight RTT wins**
> （丢包权重压倒轻微的 RTT 优势 —— 有丢包的链路再快也不要）

配套能力：

| 函数 | 位置 | 作用 |
|---|---|---|
| `ospf_edge_cost_from_score` | `conn_select.rs:121` | score → OSPF edge cost |
| `ospf_edge_cost_ms` | `conn_select.rs:131` | 原始指标 → edge cost（同一公式） |
| `apply_ospf_cost_hysteresis` | `conn_select.rs:145` | 变化小于 `min_delta` 时保持旧值 |
| `OSPF_FUSE_COST_BONUS` | — | 链路熔断时加罚分 |

**该分数已编码进 OSPF edge cost，即「链路质量」在 EasyTier 里是一等公民。**

### 3.4 三层选路，只有一层未用质量

| 层级 | 选什么 | 位置 | 用链路质量？ |
|---|---|---|---|
| ① 连接选择 | 同一 peer 的多条 PeerConn | `conn_select.rs` | ✅ RTT+loss+jitter 加权 |
| ② OSPF 路径 | 到目标 peer 的中继路径 | `peer_ospf_route.rs:707-716` | ✅ 同一 score → edge cost |
| **③ 出口选择** | **哪个 peer 当网关** | `peer_map.rs:285` | ❌ **列表顺序** |

**本文要做的：把 ③ 接上既有评分模型。**

### 3.5 L3 选路的实际代码结构

```rust
// easytier-core/src/peers/peer_manager.rs — PeerOutboundPacketRouter::get_msg_dst_peer_ipv4
// （2026-10-13：内部已改 SmallVec；公开 PeerManagerCore API 仍返回 Vec）
pub async fn get_msg_dst_peer_ipv4(&self, ipv4_addr: &Ipv4Addr)
    -> (SmallVec<[PeerId; 1]>, bool)
{
    let mut is_exit_node = false;
    let mut dst_peers = SmallVec::<[PeerId; 1]>::new();
    if self.is_all_peers_broadcast_ipv4(ipv4_addr) {
        dst_peers.extend(Self::select_ipv4_broadcast_peers(...)); // 次序 1：广播
    } else if let Some(peer_id) = self.peers.get_peer_id_by_ipv4(ipv4_addr).await {
        dst_peers.push(peer_id);                                  // 次序 2+3：VIP + proxy CIDR LPM
    } else if !同网段 {
        if let Some(peer_id) = self.resolve_exit_node_peer().await {
            dst_peers.push(peer_id);
            is_exit_node = true;                                  // 次序 4：exit_nodes
        } else if let Some(peer_id) = self.route
            .get_peer_id_for_default_route_proxy(...).await {
            dst_peers.push(peer_id);                              // 次序 5：对端 /0
        }
    }
    // 次序 6：local_exit_node_fallback → my_peer_id
    (dst_peers, is_exit_node)
}
```

**注意：次序 2+3 合并在一个调用里。** `PeerMap::get_peer_id_by_ipv4` 遍历 `routes`（tokio `RwLock`）再进 OSPF DashMap / CIDR LPM。

**本设计插入点**：`get_peer_id_by_ipv4` **之后**、`resolve_exit_node_peer` **之前**（次序 **3.5**）。

> ⚠️ **优先级含义**：对端已宣告的 VIP / 更具体 proxy CIDR **永远优先于**本地 `routes[]`。域名编译出的 `/32` 若与 peer proxy 重叠，走 peer。文档与 UI 须明示。

### 3.6 出口节点选择 = 优先级故障转移（**已存在的原语**）

```rust
// easytier-core/src/peers/conn/peer_map.rs:285-303
pub async fn resolve_exit_node_peer(&self, exit_nodes: &[IpAddr]) -> Option<PeerId> {
    for exit_node in exit_nodes {                                  // ① 按列表顺序
        let peer_id = match exit_node {
            IpAddr::V4(ip) => self.get_peer_id_by_ipv4(ip).await,  // ② 解析 VIP → PeerId
            IpAddr::V6(ip) => self.get_peer_id_by_ipv6(ip).await,
        };
        let Some(peer_id) = peer_id else { continue };             // ③ 不可达跳过
        if self.get_gateway_peer_id(peer_id, NextHopPolicy::LeastHop).await.is_some() {
            return Some(peer_id);                                  // ④ 有下一跳 → 命中
        }
    }
    None                                                          // ⑤ 全不可达
}
```

**这就是标准的「列表优先级 + 可达性检查」故障转移原语。** 新模型的 `select_by = "priority"` 直接复用。

> 包装层：`peer_manager.rs:2825` 私有方法读取 `self.exit_nodes` 后转调。

### 3.7 L2 期望集合的现有构造

```rust
// easytier-core/src/gateway/proxy/cidr_monitor.rs:131-150
pub(crate) fn resolve_proxy_cidrs(
    peer_routes: BTreeSet<Ipv4Cidr>,
    config: ProxyCidrConfigSnapshot,
) -> BTreeSet<Ipv4Cidr> {
    if let Some(manual_routes) = config.manual_routes {
        return manual_routes;                          // ← 早退！整表覆盖，peer 网段全丢
    }
    let mut routes = peer_routes;
    let install_local_default = config.has_exit_nodes && !config.no_tun;
    let keep_peer_default =
        (config.allow_peer_default_without_exit && !config.no_tun) || install_local_default;
    if !keep_peer_default { routes.remove(&ipv4_default_cidr()); }
    if install_local_default { routes.insert(ipv4_default_cidr()); }
    routes
}
```

```rust
// easytier-core/src/gateway/proxy/cidr_monitor.rs:181-197
let local_exit_default = !config.services.proxy.no_tun
    && config.services.manual_routes.is_none()
    && peer_manager.get_peer_map()
        .resolve_exit_node_peer(&exit_nodes).await.is_some();
    //↑ 出口节点真的可达才装 TUN /0
```

**⚠️ 三处必须改：**
1. `manual_routes` 的早退覆盖 → 改为累加
2. `install_local_default` 从 `exit_nodes` 推导 → 改为显式声明
3. 期望集合缺少本地路由表来源

### 3.8 可直接复用的既有能力

| 能力 | 位置 | 复用方式 |
|---|---|---|
| **域名 → IP 观测** | `MagicDnsQueryResolver::resolve`（`magic_dns/packet.rs`） | **权威 IP 来源**（§9.8）；**不是**查 `DnsConfig.hosts` |
| 静态 hosts | `DnsConfig.hosts` | 可选：对齐 DNS/路由时序、缩小首包窗口；**非**域名路由主路径 |
| 通配符匹配 | `*.suffix`，RFC 4592 多级 | `domains` 字段沿用同一语义 |
| DNS 响应 TTL | `DnsHostEntry.ttl_secs` | **仅** DNS 应答缓存时长；**不**驱动路由撤销（冲突 C5） |
| 1 秒对账循环 | `traffic-steering.md` / `cidr_monitor` | 可达性变化 → 重装 L2 |
| 可达性安全阀 | `cidr_monitor.rs`（exit 可达才装 `/0`） | 不可达不装路由 |
| 按流哈希原语 | `flow_key_from_payload`（`conn_bond`） | `select_by = balance` |
| 优先级故障转移 | `PeerMap::resolve_exit_node_peer` | `select_by = priority`（**对账循环内**复用，非热路径） |
| 质量评分 + 防抖 | `conn_select` / `direct_peer_info` | `select_by = quality`（预计算进快照） |
| 无锁快照 | `arc-swap` | 热路径读侧零锁 |
| 单目的地栈分配 | `smallvec`（P-T1 已落地） | 公开 API 边界仍 `Vec` |

### 3.9 跨 peer 链路质量的读取路径（Q3 结论：**可行**）

```rust
// easytier-core/src/peers/conn/peer_map.rs:488-526
pub(crate) async fn direct_peer_info(peer_maps: &[Arc<PeerMap>]) -> PeerInfoForGlobalMap {
    for peer in peers {
        let conns = peer_map.list_peer_conns(peer).await;
        //  :503-504  list_peer_conns 附带 quality_score / quality_fused
        //            （与 select_conn 同一公式）
        let raw_cost = conns.iter()
            .filter(|c| !c.unverified_hole_punch)
            .map(|c| ospf_edge_cost_from_score(f64::from(c.quality_score), c.quality_fused))
            .min()                                          // :515 取最优的一条连接
            .unwrap_or_else(|| continue);

        let last = LAST_OSPF_EDGE_COST.get(&peer);
        let cost = apply_ospf_cost_hysteresis(raw_cost, last, OSPF_EDGE_COST_MIN_DELTA); // :521
        LAST_OSPF_EDGE_COST.insert(peer, cost);

        ret.direct_peers.insert(peer, DirectConnectedPeerInfo { latency_ms: cost }); // :525
    }
    ret
}
```

**A 侧可直接读取：`global_peer_map[A].direct_peers[B].latency_ms`**

#### ⚠️ 精度限制（不影响排序）

字段名 `latency_ms` 是**历史遗留名**（`peer_map.rs:487` 注释：「为 wire 兼容保留旧名」），实际是**复合分编码**：

```
cost = round(score × 1000)，clamp 到 [1, 1_000_000]
score = w_lat×rtt_秒 + w_loss×loss率 + w_jitter×jitter_秒

默认权重下：cost ≈ rtt_ms + 4000×loss率 + jitter_ms
```

**不可逆分解** —— 拿不到独立的 RTT / loss / jitter。

**但 `select_by = "quality"` 只需给候选排序**，cost 与质量单调对应，排序结果完全正确。

> 只有当需要「因为 loss 高所以排除 B」这类**基于单一维度**的规则时才受限 → 降级为「cost 超过绝对阈值」，见 §12.4。

#### 防抖

| 环节 | 机制 | 位置 |
|---|---|---|
| 编码 | score → cost | `conn_select.rs:121` |
| **发布防抖** | 变化 < `OSPF_EDGE_COST_MIN_DELTA = 20` 不更新 | `conn_select.rs:115/145` |
| **切换防抖** | margin_pct / abs_margin / windows 三重门槛 | `conn_select.rs:70-72` |

**两重防抖已存在** —— 链路抖动导致横跳的坑 EasyTier 已解决，`quality` 策略直接受益。

### 3.10 ⚠️ 热路径现状（企业级性能的关键约束）

**路由查询是每包一次。** 现状（P-T1 后，见 [`routing-tech-debt.md`](./routing-tech-debt.md)）：

```
出站包 → PeerOutboundPacketRouter::send_msg_by_ip
       → get_msg_dst_peer_ipv4
            → PeerMap::get_peer_id_by_ipv4
                 → routes.read().await（tokio RwLock，实践常 1 个 route）
                 → OSPF：VIP DashMap O(1) / proxy CIDR ArcSwap+LPM
基线（mock PeerMap 固定成本）：VIP/exit ~165–176 ns，规模不敏感
```

**本设计硬约束（§12.1）：新增 `LocalRouteTable` 路径必须无锁、无分配、无 async。**

> **明确不做**：不顺手把存量 `PeerMap.routes` 改成 ArcSwap（TD-1 已评估暂缓）。只保证**新增路径**净收益为正。

---

## 4. 统一模型

### 4.1 核心命题

> **B = 网关（提供转发能力），A = 路由表（持有全部策略）。**

```
┌─ 源主机 A ─────────────────────────┐
│  routes[] ← 全部策略集中在这里      │
│    CIDR → peer│
│    域名 → peer                   │
│    选择策略 / 切换策略 / 失败策略    │
└──────────────┬────────────────────┘
               │ EasyTier 隧道
      ┌────────┴─────────┐
┌─ 目标主机 B ─┐    ┌─ 目标主机 C ─┐
│ 网关（开关不变）  │    │ 网关│
│ 不需要感知域名   │    │ 不需要感知域名 │
└────────────────┘    └───────────────┘
```

### 4.2 B 侧要求（**已确认**）

| 项 | 结论 |
|---|---|
| B 的开关 | **`enable_exit_node` 保持不变**（可理解为「网关模式」） |
| B 是否宣告 `0.0.0.0/0` | **首期不做**。A 侧已明确目标节点，宣告 `/0` 对本场景无收益 |
| B 是否配置域名 | **不需要** |
| B 是否解析 DNS | **不需要** |

### 4.3 关键洞察：出口节点不需要「知道」目标

| | 子网代理 | 网关 / 出口节点 |
|---|---|---|
| B 做什么 | **代连**目标设备 | **NAT** 转发 |
| B 需要知道目标 IP 吗 | ✅ **必须** | ❌ **不需要** |

**所以权威在 A 侧是必然的，不是偏好选择。** 这也是 `domain-proxy.md`（peer-driven）在出口场景下属过度设计的原因。

---

## 5. 配置模型

### 5.1 Schema（**暂定**）

```toml
[[routes]]
# ---- 匹配条件（cidr 与 domains 至少填一个，可同时填）----
cidr    = "0.0.0.0/0"                          # 可选
domains = ["*.atlassian.net"]                   # 可选

# ---- 候选节点（按序，至少一个）----
peers = ["10.144.0.9", "10.144.0.10"]           # 必填，VIP

// ---- 策略（全部必填，无默认值）----
select_by         = "priority"                   # priority | quality | balance
failover_when     = "unreachable"                # unreachable | degraded | never
if_all_unavailable = "closed"                    # open | closed
require_confirmed  = false                       # closed + 多 peer 时必须为 true（Q7）
```

**必填校验**：三个策略字段缺任一 → **配置加载失败**（不静默取默认）。

> 理由：选错策略在合规场景下是**静默失效**，比报错危险得多。

### 5.2 典型组合

```toml
# 合规出口：固定美国，不通就断
[[routes]]
cidr = "0.0.0.0/0"
peers = ["10.144.0.9"]
select_by = "priority"
failover_when = "unreachable"
if_all_unavailable = "closed"

# 通用加速：优先 B，B 质量差就退到 C
[[routes]]
cidr = "0.0.0.0/0"
peers = ["10.144.0.9", "10.144.0.10"]
select_by = "priority"
failover_when = "degraded"
if_all_unavailable = "open"

# 带宽分摊：按流哈希
[[routes]]
cidr = "0.0.0.0/0"
peers = ["10.144.0.20", "10.144.0.21"]
select_by = "balance"
failover_when = "unreachable"
if_all_unavailable = "open"

# 域名走特定出口（可与 CIDR 规则并存）
[[routes]]
domains = ["*.atlassian.net"]
peers = ["10.144.0.30"]
select_by = "priority"
failover_when = "unreachable"
if_all_unavailable = "closed"
```

### 5.3 `exit_nodes` 降级为语法糖（**已确认向后兼容**）

```toml
exit_nodes = ["10.144.0.9"]
    ↓ 加载期内存展开（不落盘、不回写配置）
RouteRule {
    cidr: 0.0.0.0/0,
    peers: ["10.144.0.9"],
    select_by: Priority,
    failover_when: Unreachable,
    if_all_unavailable: Open,        // 与现有行为一致
}
```

共存期内旧写法行为完全不变。

---

## 6. 策略语义

### 6.1 三个正交字段（**暂定**）

**策略是用户在每条记录上的声明，各记录独立、互不影响。不同记录可用不同策略。**

#### `select_by` —— 怎么挑 peer（互斥三选一，必填）

| 值 | 语义 | 现状支持 |
|---|---|---|
| `priority` | 按 `peers` 列表顺序，取第一个可达 | ✅ **已存在**（`peer_map.rs:285`） |
| `quality` | 取 `conn_quality_score` 最优的可达节点 | ⚠️ 需接线（模型已有 `conn_select.rs:97`） |
| `balance` | 按内层 IP 五元组哈希分散 | ⚠️ 原语已有（`conn_bond.rs:340`） |

#### `failover_when` —— 何时切到下一个 peer（互斥三选一，必填）

| 值 | 语义 |
|---|---|
| `unreachable` | 仅当当前 peer **不可达** 时切换 |
| `degraded` | 当前 peer **质量劣化**时也切换（仅对 `priority` 有意义） |
| `never` | 不切换 |

#### `if_all_unavailable` —— 全部候选不可用时（互斥二选一，必填）

| 值 | 语义 | 适用 |
|---|---|---|
| `open` | 不装 L2 路由 → 流量走物理网关 | 通用出口（**现有 `exit_nodes` 行为**） |
| `closed` | 阻断（丢包 / 黑洞） | **强策略**：候选全挂时不降级物理直连（**仍不**堵住 DNS/首包泄漏，见 §12.6） |

### 6.2 为什么需要三个字段而非一个

**能表达单字段三选一表达不了的组合：**

| `select_by` | `failover_when` | 语义 | 场景 |
|---|---|---|---|
| `priority` | `unreachable` | 固定用第一个，断了才切 | 合规（确定性出口） |
| `priority` | `degraded` | 优先第一个，质量差就退 | 「优先 B，B 差就退」 |
| `quality` | `unreachable` | 选最优，断了重选 | 通用加速 |
| `balance` | 任意 | 按流分散 | 带宽分摊 |

**第 2 行是单字段模型表达不了的。**

### 6.3 ⚠️ 质量优选与合规需求冲突

```
需求：Atlassian 宜固定走美国出口
质量优选：B(美国) RTT=180ms 有丢包  vs  C(日本) RTT=45ms 无丢包
        → loss 权重是 lat 的 4 倍 → 选 C ❌ 策略意图失效
```

**「延迟最低」与「必须从美国出」互斥。** 因此 `select_by` 必须 per-record 声明，不能全局配置。

### 6.4 负载均衡的硬约束

**必须按流哈希，绝不能按包轮询：**

| 做法 | 后果 |
|---|---|
| 按包轮询 | 同 TCP 流的包分散到不同节点 → **乱序/丢包**；各节点 NAT 表半截 → 连接建不起来 |
| **按流哈希** | 同流恒定走同一节点 → NAT 表一致 ✅ |

`flow_key_from_payload`（`conn_bond.rs:340`）已对内层 IP 五元组（`parse_ip_flow_key`，`:357`）哈希，天然满足。

### 6.5 ⚠️ 故障转移链的合规陷阱

```
规则：atlassian.net → [B(美国), C(日本)]，closed

B 挂 → 切到 C ❌ 合规失效（流量跑到日本了）
```

**若候选列表中存在不满足合规要求的节点，自动故障转移本身就是合规风险。**

**建议：** 配置校验时明确提示；必要时提供「本条记录所有候选必须同合规域」的声明性检查。**（已定：采用 `require_confirmed` 硬失败，见 §9.10 步骤 ⑤）**

---

## 7. 数据流与状态机

### 7.1 全流程

```
【配置】A 预先声明
   *.atlassian.net → [B, C]  select_by=priority, failover_when=unreachable, closed

【运行时 · 每客户端独立】
  ① App DNS → MagicDNS → ObservingResolver 订阅应答 A 记录
  ② 维护 域名 → IP[]（观测写入；**非** TTL 撤销，见 §9.8.5）
  ③ L2：把观测到的 /32 装进 OS 路由 → TUN（受 ~1s 对账约束）
  ④ L3：快照 by_ip 登记「1.2.3.4 → active_peer」
  ⑤ 每秒对账：更新 active_peer / RuleState（§7.2 / §9.7）
  ⑥ 规则删除或超限 LRU → 撤销对应 /32【记日志】

  其余流量 → 物理网关直连（不进 TUN）
  ⚠️ DNS 上游查询本身仍可能走物理网卡（§12.6）
```

**B 侧：只开网关开关，收到包 → NAT 出网。不参与 ①–⑥。**

### 7.2 每条规则的状态机

```
                  ┌──────────┐
                  │  STABLE  │  ← 至少一个候选 peer 可达，L2 路由已安装
                  └────┬─────┘
          出现不可达   │   全部不可达
      ┌────────────────┴────────────────┐
      ▼                                 ▼
┌──────────┐                      ┌─────────────┐
│ FAILOVER │ ←── 有 peer 恢复 ──    │  DEGRADED   │
│ 切到下一个 │                     │ 全部不可用   │
└──────────┘                      └──────┬──────┘
                                          │
                        if_all_unavailable = "open"
                            ├──────────────┴──────────────┐
                            ▼                             ▼
                    撤销 L2 路由                   阻断（丢包）
                    流量走物理网关                记 error 日志
                            │                             │
                            └──────────► 有人恢复 ◄────────┘
```

**转移由 1 秒对账循环驱动**（复用 `traffic-steering.md:82` 描述的 reconciler）。

### 7.3 L2 路由的安装/撤销规则

| 规则状态 | L2 期望集合贡献 | 实际效果 |
|---|---|---|
| STABLE（至少一个可达） | 规则匹配的 CIDR | 包进 TUN |
| DEGRADED + `open` | **不贡献** | 包不进 TUN，走物理网关 |
| DEGRADED + `closed` | **不贡献**（阻断在 L3 之前？见 Q8） | 需实现黑洞策略 |

> **`closed` 的阻断点位置（已定）**：**分平台实现** —— Linux 在 L2 层装黑洞路由（`blackhole` type）；其余平台降级为 L3 层拒绝，**必须在文档中明示平台差异**。

---

## 8. 与现有优先级表的关系

### 8.1 现有 L3 顺序（`traffic-steering.md` §2）

| 次序 | 规则 | `is_exit_node` |
|---|---|---|
| 1 | 广播 | — |
| 2 | VIP 精确命中 | false |
| 3 | peer 通告的 proxy CIDR LPM（不含 `/0`） | false |
| 4 | `exit_nodes` 列表顺序 | **true** |
| 5 | 对端通告的 `0.0.0.0/0` | false |
| 6 | `local_exit_node_fallback` | true |

### 8.2 目标顺序（**插入位已定：3.5**）

| 次序 | 规则 | 变更 |
|---|---|---|
| 1 | 广播 | 不变 |
| 2 | VIP 精确命中 | 不变 |
| 3 | peer 通告的 proxy CIDR LPM（不含 `/0`） | 不变（子网代理语义更强） |
| **3.5** | **本地 routes 表 LPM** | **新增** |
| 4 | `exit_nodes`（= routes 的语法糖） | 内部重定向到 routes |
| 5 | 对端 `/0` | 不变 |
| 6 | `local_exit_node_fallback` | 不变 |

### 8.3 L2 期望集合

```
effective_l2 = list_proxy_cidrs()      # peer 通告（子网代理）
             ∪ compile(routes)         # 本地路由表（新增）
             + [/0 if 本机显式声明全隧道] # 显式声明，不再从 exit_nodes 推导
```

> **必须改为「累加」，不能沿用 `manual_routes` 的早退覆盖。**
> 现存早退（`cidr_monitor.rs:135-137`）：`manual_routes` 一开即返回，整表覆盖，peer 网段全丢。

---

## 9. 实现设计（含伪代码）

### 9.1 核心数据结构

```rust
// 新增：easytier-core/src/peers/route/local_route_table.rs

pub enum SelectBy {
    /// 按 peers 顺序取第一个可达（复用现有 resolve_exit_node_peer 语义）
    Priority,
    /// 取 quality score 最优的可达
    Quality,
    /// 按流哈希分散
    Balance,
}

pub enum FailoverWhen {
    /// 仅不可达时切
    Unreachable,
    /// 不可达或质量劣化时切（仅 Priority 有意义）
    Degraded,
    /// 不切
    Never,
}

pub enum AllUnavailable {
    /// 不装 L2 路由 → 流量走物理网关（现有行为）
    Open,
    /// 阻断
    Closed,
}

pub struct RouteRule {
    pub id: RuleId,                      // 稳定标识，用于安装/撤销记账
    pub cidr: Option<Ipv4Cidr>,          // 至少一个与 domains 非空
    pub domains: Vec<DomainPattern>,     // 复用 magic_dns 的通配语义
    pub peers: Vec<IpAddr>,              // 按序，VIP
    pub select_by: SelectBy,
    pub failover_when: FailoverWhen,
    pub if_all_unavailable: AllUnavailable,
}

pub enum DomainPattern {
    Exact(String),        // "example.com"
    Wildcard(String),     // "*.example.com"（RFC 4592 多级）
}

/// 规则 → 候选 peer + 当前生效状态（**对账循环写入；热路径只读**）
pub struct ResolvedRule {
    pub rule: RouteRule,
    pub compiled_cidrs: BTreeSet<Ipv4Cidr>,  // 显式 cidr ∪ 观测到的域名 /32
    pub resolved_peers: Vec<PeerId>,         // VIP 已解析（对账用）
    pub state: RuleState,                    // Stable | Failover | Degraded
    /// 当前选中的出口 peer；热路径直接使用，**禁止**在热路径重算
    pub active_peer: Option<PeerId>,
    pub is_egress: bool,                     // 是否置 is_exit_node
}

pub enum RuleState {
    Stable,      // 至少一个可达
    Failover,    // 主 peer 不可达，已切到后备
    Degraded,    // 全部不可用
}
```

### 9.2 索引结构与热路径契约

```rust
pub struct LocalRouteSnapshot {
    rules: Vec<ResolvedRule>,                 // 含 active_peer / state（对账写入）
    /// 显式 CIDR 规则，按前缀长度**降序**预排序（热路径 LPM）
    cidr_sorted: Vec<(Ipv4Cidr, RuleIdx)>,
    /// 精确 IP → 规则：域名观测 /32、以及可选展开的短前缀（见下）
    by_ip: HashMap<Ipv4Addr, SmallVec<[RuleIdx; 2]>>,
    by_domain: HashMap<String, Vec<RuleIdx>>, // 诊断用，非热路径
    peer_quality: HashMap<PeerId, i32>,       // 对账预计算（quality 策略）
}

pub struct LocalRouteTable {
    snapshot: ArcSwap<LocalRouteSnapshot>,
}

impl LocalRouteTable {
    /// 热路径唯一入口：**同步 / 无锁 / 无分配 / 无 async**
    /// 返回已预选的 (PeerId, is_exit_node)；未命中或 Degraded 返回 None
    #[inline]
    pub fn lookup_peer(&self, ip: &Ipv4Addr) -> Option<(PeerId, bool)> {
        let snap = self.snapshot.load(); // Guard；全程持有，禁止返回内部引用
        let idx = match_rule(&snap, ip)?;
        let r = &snap.rules[idx];
        if r.state == RuleState::Degraded {
            return None;
        }
        let peer = r.active_peer?;
        Some((peer, r.is_egress))
    }
}

/// ① by_ip 精确命中 → ② cidr_sorted 最长前缀（顺序扫，前缀已降序）
fn match_rule(snap: &LocalRouteSnapshot, ip: &Ipv4Addr) -> Option<RuleIdx> {
    if let Some(idxs) = snap.by_ip.get(ip) {
        return disambiguate_sync(snap, ip, idxs.as_slice());
    }
    for (cidr, idx) in &snap.cidr_sorted {
        if cidr.contains(ip) {
            return Some(*idx); // 已按 prefixlen 降序 → 首个命中即 LPM
        }
    }
    None
}
```

**热路径契约（硬，与 §12.1 一致）：**

| 允许 | 禁止 |
|---|---|
| `ArcSwap::load` + 读快照字段 | 任何锁 / `await` / DNS |
| `by_ip` O(1) + `cidr_sorted` 顺序 LPM | 调用 `get_peer_id_by_ipv4` / `resolve_exit_node_peer` |
| 直接读 `rules[i].active_peer` | 在热路径 `select_peer` / 算 quality / 分配 `Vec` |

**索引职责：**

| 索引 | 用途 | 热路径 |
|---|---|---|
| `by_ip` | 域名观测 `/32`；可选：极短前缀（如 `/32`–`/28`）展开 | ✅ |
| `cidr_sorted` | **所有显式 CIDR（含 `/0`、`/24`）的 LPM**；**禁止**把大网段整表展开进 `by_ip` | ✅ |
| `by_domain` | CLI / UI | ❌ |

> **复杂度**：`by_ip` 命中 O(1)；CIDR miss 后扫 `cidr_sorted` 为 **O(R_cidr)**（规则条数，非前缀树）。百～千条规则可接受；若要冲 10 万条 CIDR，需另换真正的 LPM 结构（首期不强制）。

### 9.3 L2 期望集合改造

```rust
// 现状：cidr_monitor.rs:131-150
pub(crate) fn resolve_proxy_cidrs(
    peer_routes: BTreeSet<Ipv4Cidr>,
    config: ProxyCidrConfigSnapshot,
) -> BTreeSet<Ipv4Cidr> {
    if let Some(manual_routes) = config.manual_routes {
        return manual_routes;                    // ← 早退覆盖（问题所在）
    }
    let mut routes = peer_routes;
    let install_local_default = config.has_exit_nodes && !config.no_tun;
    // ...
}
```

**目标伪代码：**

```rust
pub(crate) fn resolve_proxy_cidrs(
    peer_routes: BTreeSet<Ipv4Cidr>,
    config: ProxyCidrConfigSnapshot,
    local_routes: &CompiledLocalRoutes,   // ← 新增来源
) -> BTreeSet<Ipv4Cidr> {
    // ① 本地规则贡献的 CIDR（仅 Stable 状态，见 §7.3）
    let mut routes: BTreeSet<Ipv4Cidr> = peer_routes;

    for (rule, resolved) in local_routes.iter() {
        match resolved.state {
            RuleState::Stable | RuleState::Failover => {
                routes.extend(resolved.compiled_cidrs.iter().copied());
            }
            RuleState::Degraded => {
                match rule.if_all_unavailable {
                    AllUnavailable::Open => { /* 不贡献 → 流量走物理网关 */ }
                    AllUnavailable::Closed => { /* 见 Q8：阻断策略 */ }
                }
            }
        }
    }

    // ② 兼容层：manual_routes 默认保持「整表覆盖」（Q5 决策：opt-in 才累加）
    if let Some(manual_routes) = &config.manual_routes {
        if config.manual_routes_append {
            // opt-in 新行为：累加，保留 peer 网段
            routes.extend(manual_routes.iter().copied());
        } else {
            // 默认：保持现状语义（整表覆盖）—— 零回归
            return finalize_routes(manual_routes.clone(), config);
        }
    }

    // ③ /0 处理：显式声明，不再从 exit_nodes 推导
    let declares_full_tunnel = local_routes.has_default_route_rule()
        || config.exit_default_route.unwrap_or(config.has_exit_nodes && !config.no_tun);
    let install_local_default = declares_full_tunnel && !config.no_tun;

    let keep_peer_default =
        (config.allow_peer_default_without_exit && !config.no_tun) || install_local_default;
    if !keep_peer_default {
        routes.remove(&ipv4_default_cidr());
    }
    if install_local_default {
        routes.insert(ipv4_default_cidr());
    }
    routes
}
```

**⚠️ 步骤 ② 是破坏性行为变更**，建议：
- 保留 `manual_routes` 的覆盖语义，加 `manual_routes_append: bool` 开关，默认 `false`
- 或在新版本明确变更并给出迁移说明

### 9.4 L3 选路改造

**插入点：`PeerOutboundPacketRouter::get_msg_dst_peer_ipv4`，在 `resolve_exit_node_peer` 之前。**

```rust
pub async fn get_msg_dst_peer_ipv4(&self, ipv4_addr: &Ipv4Addr)
    -> (SmallVec<[PeerId; 1]>, bool)
{
    let mut is_exit_node = false;
    let mut dst_peers = SmallVec::<[PeerId; 1]>::new();

    if self.is_all_peers_broadcast_ipv4(ipv4_addr) {
        dst_peers.extend(...); // 次序 1
    } else if let Some(peer_id) = self.peers.get_peer_id_by_ipv4(ipv4_addr).await {
        dst_peers.push(peer_id); // 次序 2+3：VIP + peer proxy（优先于本地表）
    } else if !同网段 {
        // ↓↓↓ 次序 3.5：同步快照查找（无 await）↓↓↓
        if let Some((peer_id, egress)) = self.local_route_table.lookup_peer(ipv4_addr) {
            dst_peers.push(peer_id);
            is_exit_node = egress;
        } else if let Some(peer_id) = self.resolve_exit_node_peer().await {
            dst_peers.push(peer_id); // 次序 4
            is_exit_node = true;
        } else if let Some(peer_id) = self.route
            .get_peer_id_for_default_route_proxy(...).await
        {
            dst_peers.push(peer_id); // 次序 5
        }
    }
    // 次序 6：local_exit_node_fallback …
    (dst_peers, is_exit_node)
}
```

### 9.4.1 传输优化旁路接入（冲突 C9）

`get_peer_id_by_ip_allowing_default_proxy`（`peers/route/mod.rs:146-159`）**是刻意与 L3 router 区分的有意设计**（注释明确写了「Must **not** be used by the L3 outbound packet router」）。

**但它不查 `LocalRouteTable`**，导致走本地路由表的流量会失去三种传输优化：

```rust
// peers/peer_manager.rs:2673-2684
// 修复前：找不到 owner → false → KCP/QUIC/wrapped-TCP 全部禁用
let Some(dst_peer_id) = self.route
    .get_peer_id_by_ip_allowing_default_proxy(dst_ip).await
else { return false; };
```

**修复点**（不改 `Route` trait，避免跨模块耦合）：

```rust
// PeerOutboundPacketRouter::check_allow_wrapped_proxy_to_dst
async fn check_allow_wrapped_proxy_to_dst(&self, dst_ip: &IpAddr, ...) -> bool {
    let owner = match dst_ip {
        IpAddr::V4(v4) => self.local_route_table.lookup_peer(v4).map(|(p, _)| p),
        IpAddr::V6(_) => None, // 首期仅 IPv4（N3）
    };
    let owner = match owner {
        Some(p) => Some(p),
        None => self.route.get_peer_id_by_ip_allowing_default_proxy(dst_ip).await,
    };
    let Some(dst_peer_id) = owner else { return false; };
    // ...原有 feature flag / next-hop 检查不变
}
```

**受影响的三条路径**：

| 路径 | 调用点 |
|---|---|
| `check_allow_kcp_to_dst` | `gateway/dataplane/mod.rs:627` |
| `check_allow_quic_to_dst` | 同上分支 |
| wrapped-TCP 传输选择 | `gateway/proxy/wrapped_transport/packet_plane.rs:264-277` |

> ⚠️ **ACL 不受影响** —— 它从包头取 peer id（`peers/acl/filter.rs:373-376` 的 `packet.get_dst_peer_id()`），不走路由查询。

**验收要求**（除逻辑正确性外，必须做性能验证）：

| # | 场景 | 期望 |
|---|---|---|
| 1 | `1.2.3.4 → B`，B 支持 KCP | KCP **启用** |
| 2 | `1.2.3.4 → B`，B 不支持 KCP | 禁用，回落标准中继 |
| 3 | 无 `LocalRouteTable` 命中 | 行为与现状**完全一致** |
| **4** | **跨境高丢包压测** | **KCP/QUIC 实际生效（抓包确认）** |

> 第 4 条才是真正的验收 —— 前三条只验证逻辑，第 4 条才验证**性能没退化**。

### 9.5 热路径 vs 对账：职责分割（**硬**）

| 路径 | 做什么 | 不做什么 |
|---|---|---|
| **热路径** `lookup_peer` | `load` 快照 → `by_ip` / `cidr_sorted` → 读 `active_peer` | 选 peer、查 gateway、算 quality、分配 |
| **对账循环**（~1s） | VIP→PeerId、gateway 可达、`select_by`、写 `active_peer`/`state`、刷新 `peer_quality`、触发 L2 diff | 不在转发热路径上跑 |

**旧版伪代码里的 `async fn lookup` / `select_peer().await` 已废止** —— 违反 §12.1。

消歧（多规则命中同一 IP）在**构建快照 / 观测写入**时完成，结果写入 `by_ip` 的单一胜者（或 `SmallVec` 但热路径只取已消歧的首个）；`closed` 优先于 `open`。

### 9.6 `select_peer` —— 仅在对账循环调用

```rust
/// 在 reconcile 内调用；结果写入 ResolvedRule.active_peer 后 store 快照
async fn select_peer_for_reconcile(
    peers: &PeerMap,
    snap: &LocalRouteSnapshot,
    r: &RouteRule,
    resolved: &[PeerId],
    flow_hint: Option<u64>, // balance 用；无连接五元组时可用 VIP 哈希占位
) -> Option<PeerId> {
    let reachable: Vec<(PeerId, i32)> = resolved.iter().copied()
        .filter(|&p| peers.get_gateway_peer_id(p, LeastHop).await.is_some())
        .map(|p| (p, snap.peer_quality.get(&p).copied().unwrap_or(i32::MAX)))
        .collect();
    if reachable.is_empty() { return None; }

    match r.select_by {
        SelectBy::Priority => {
            // 列表顺序；failover_when=degraded 时跳过 cost 超阈值的当前首选
            pick_priority(&r, &reachable)
        }
        SelectBy::Quality => {
            reachable.into_iter().min_by_key(|(_, c)| *c).map(|(p, _)| p)
        }
        SelectBy::Balance => {
            // 对账无法看到每包五元组：快照可存「候选有序列表」；
            // 热路径 balance 若需要按流哈希，须在快照中存放 reachable PeerId 列表，
            // 由 lookup_peer 用 flow_key_from_payload % len —— **仅 balance 规则**允许热路径一次取模（仍无锁无分配）。
            // 首期也可简化为对账固定一个 peer（行为退化，须文档说明）。
            pick_balance(flow_hint, &reachable)
        }
    }
}
```

**⚠️ `direct_peer_info` / `get_gateway_peer_id` 禁止出现在热路径**；只在对账预填 `peer_quality` 与 `active_peer`。

### 9.7 可达性对账循环（每秒）

```rust
async fn reconcile(table: &LocalRouteTable, peers: &PeerMap) {
    loop {
        sleep(Duration::from_secs(1)).await;
        let old = table.snapshot.load_full();
        let mut new = (*old).clone();
        // 刷新 peer_quality（来自 direct_peer_info / LAST_OSPF_EDGE_COST）
        refresh_peer_quality(&mut new, peers).await;

        for r in &mut new.rules {
            let chosen = select_peer_for_reconcile(peers, &new, &r.rule, &r.resolved_peers, None).await;
            let new_state = if chosen.is_none() {
                RuleState::Degraded
            } else if r.active_peer == chosen {
                RuleState::Stable
            } else {
                RuleState::Failover
            };
            r.active_peer = chosen;
            r.state = new_state;
        }
        table.snapshot.store(Arc::new(new));
        // state/compiled_cidrs 变化 → 触发 L2 diff（既有 cidr_monitor 机制）
    }
}
```

### 9.8 域名 → IP 观测（订阅 MagicDNS 解析结果）

> ⚠️ **本节已按冲突分析修正。** 原设计假设 IP 来自 `DnsConfig.hosts` 静态查询 —— **这是错的**：外部域名走的是 MagicDNS 解析优先级的第 4 级（默认上游），根本不经过 hosts。
> 详见 [`unified-routing-conflicts.md`](./unified-routing-conflicts.md) C1 / C2。

#### 9.8.1 Hook 点

```rust
// easytier-core/src/gateway/magic_dns/packet.rs:21-24
// 全代码库中唯一能看到「应用实际解析了什么域名、得到什么答案」的位置
#[async_trait]
pub trait MagicDnsQueryResolver: Send + Sync + 'static {
    async fn resolve(&self, query: MagicDnsQuery) -> Option<Vec<u8>>;
}
```

**采用「订阅实际返回值」而非「查 hosts 表」** —— 它天然尊重四级解析优先级（hosts → 路由 zone → split → 上游），无需重复实现分支判断。

#### 9.8.2 数据源分离（修正 C2）

| 数据 | 来源 | 断连时 | 用途 |
|---|---|---|---|
| **规则是否存在** | 本地 `DnsConfig` | ❌ 不受影响 | **权威** |
| **IP 集合** | `resolve()` 观测 | 可能为空 | 观测 |

**不得从 MagicDNS server record store 读取** —— `static_hosts` 会在隧道断开时清理（`dns_server/tests.rs:732` 确认），会导致路由误撤销。

#### 9.8.3 伪代码

```rust
/// 包装现有 resolver，订阅解析结果
struct ObservingResolver {
    inner: Arc<dyn MagicDnsQueryResolver>,
    table: Arc<LocalRouteTable>,
    local_dns_config: Arc<DnsConfig>,     // 权威：判断规则是否存在
}

#[async_trait]
impl MagicDnsQueryResolver for ObservingResolver {
    async fn resolve(&self, query: MagicDnsQuery) -> Option<Vec<u8>> {
        let response = self.inner.resolve(query.clone()).await;
        if let Some(resp) = &response {
            // ⚡ 同步更新快照（不等 1 秒对账）—— 见 9.8.4
            self.observe(query, resp);
        }
        response
    }
}

impl ObservingResolver {
    fn observe(&self, query: MagicDnsQuery, resp: &[u8]) {
        let qname = parse_qname(query.payload);          // 规范化小写、去尾点
        let ips   = parse_a_records(resp);

        // ① 规则权威来自本地配置，不来自观测
        if !self.local_dns_config.has_domain_rule(&qname) { return; }

        // ② 排除 MagicDNS 路由 hostname 类域名（冲突 C4）
        //    这类答案本就是 EasyTier 虚拟 IP，该走 mesh 而非域名路由
        if self.is_magic_route_hostname(&qname) { return; }

        // ③ 原子更新：compiled_cidrs（L2）与 by_ip（L3）同批换入快照
        //    旧 IP 不删除（TTL 语义见 9.8.5）
        self.table.observe_ips(&qname, ips);
    }
}
```

#### 9.8.4 首包泄漏与缓解（冲突 C1 衍生）

```
t=0     DNS 查询 → MagicDNS 本地应答
t=0+ε   resolve() 返回 → hook 同步更新快照
t=0+2ε  快照已含新 /32；但 OS 路由由 1s 对账循环装（≤1s）
t=0+3ε  App 发起连接  ← 若此时路由未装，首包从物理网关出去
```

⚠️ **L2 的 OS 路由安装受 1 秒对账循环约束**，因此首个连接包存在**最长 1 秒**的直连窗口。

| 缓解 | 效果 | 代价 |
|---|---|---|
| hook 内同步更新快照（已采纳） | L3 立即就绪 | L2 仍受 1s 约束 |
| **配合静态 hosts** | DNS 与路由时序对齐，消除窗口 | 需手工维护 IP |
| 缩短 reconcile 间隔 | 窗口变小 | 增加路由 churn |

**⚠️ 用户文档必须明示：**

> 域名路由**不保证 DNS 查询本身**走指定出口（冲突 C6），
> 且**首次访问**的连接包存在最长 1 秒直连窗口。
> 本设计**不保证**零泄漏。缩小首包窗口可配合 `DnsConfig.hosts`；DNS 上游仍走物理网卡（§12.6）。真正零泄漏需另立项。

#### 9.8.5 TTL 与撤销语义（修正 C5）

⚠️ **`DnsHostEntry.ttl_secs` 是 DNS 响应 TTL，不是条目生命周期。** hosts 条目只由配置变更/删除决定，不随时间消失。

| 事件 | 路由处理 |
|---|---|
| `resolve()` 返回新 IP | **新增** `/32`；旧 IP **保留** |
| hosts 条目被**删除** | **立即撤销**对应全部 `/32` |
| TTL 到期 | **不触发任何路由变更** |
| 「不再返回某 IP」 | **保留 + 数量上限**（Q13 建议方案 C） |

**一致性要求：** `compiled_cidrs`（L2）与 `by_ip`（L3）必须**同批原子更新**，否则会出现「L2 已装、L3 查不到」的黑洞窗口。

**补充约束（冲突 C3）：** 配置校验阶段需预检 `domains` 是否会与 `DnsConfig.hosts` 的守卫规则（单标签 zone / route zone 同名 / split 占父 zone）或 Split forwarder 冲突，**提前报错**而非运行时静默失效。

### 9.9 `exit_nodes` 兼容转换

```rust
impl From<&TomlConfig> for LocalRouteTable {
    fn build(cfg: &TomlConfig) -> Self {
        let mut rules = vec![];

        // ① 新语法：routes[]
        for r in &cfg.routes {
            rules.push(RouteRule {
                cidr: r.cidr,
                domains: parse_domain_patterns(&r.domains),   // 校验 RFC 4592
                peers: r.peers.clone(),
                select_by: parse_select_by(&r.select_by),      // 必填，缺失则报错
                failover_when: parse_failover(&r.failover_when),
                if_all_unavailable: parse_all_unavail(&r.if_all_unavailable),
                ..Default::default()
            });
        }

        // ② 旧语法：exit_nodes → 等价规则（内存展开，不回写）
        if !cfg.exit_nodes.is_empty() {
            rules.push(RouteRule {
                cidr: Some(ipv4_default_cidr()),
                domains: vec![],
                peers: cfg.exit_nodes.clone(),
                select_by: SelectBy::Priority,
                failover_when: FailoverWhen::Unreachable,
                if_all_unavailable: AllUnavailable::Open,
                ..Default::default()
            });
        }

        Self::build_and_index(rules)
    }
}
```

### 9.10 配置校验规则

```rust
fn validate(rule: &RouteRule) -> Result<()> {
    // ① 匹配条件至少一个
    if rule.cidr.is_none() && rule.domains.is_empty() {
        bail!("route 至少需要 cidr 或 domains 之一");
    }
    // ② 候选 peer 至少一个
    if rule.peers.is_empty() {
        bail!("route 至少需要一个 peers");
    }
    // ③ 域名语法（RFC 4592：只允许最左单标签 *，父域 ≥2 标签）
    for pat in &rule.domains {
        pat.validate_wildcard_syntax()?;
    }
    // ④ 数量上限（防路由表膨胀）
    if rule.domains.len() > MAX_DOMAINS_PER_RULE { bail!("domains 超出上限") }
    if rule.peers.len() > MAX_PEERS_PER_RULE { bail!("peers 超出上限") }

    // ⑤ ⚠️ require_confirmed 硬校验（Q7：加载失败，不只是 warn）
    if rule.if_all_unavailable == AllUnavailable::Closed && rule.peers.len() > 1 {
        if !rule.require_confirmed {
            bail!(
                "route {}: if_all_unavailable=closed 且配置了 {} 个候选 peer；\
                 故障转移可能把流量送到不满足合规要求的出口。\
                 确认所有 peer 均满足合规后，设置 require_confirmed = true",
                rule.id, rule.peers.len()
            );
        }
    }
    Ok(())
}
```

**Q7 决策的意图**：把「合规风险」从「静默失效」变成「**无法配置**」。宁可配置失败，也不让用户不小心把流量送到不合规出口。

---

## 10. 歧义处理

### 10.1 同一 IP 命中多条规则

1. **特异性优先**：取 `max(CIDR 前缀长度, 域名后缀标签数)` 最大者
2. **特异性相同** → 取更严格者（`if_all_unavailable=Closed` 优先于 `Open`）
3. **仍相同** → 合并候选 peer 列表（先后者在前）

### 10.2 CDN 共享 IP

```
104.18.0.1 同时属于 atlassian.net 和 cdn.example.net

→ 特异性：域名标签数都是 2 → 平手
→ 严格性：若一条 closed 一条 open → closed 胜
→ 若都 open → 按配置顺序取第一条
```

### 10.3 规则间冲突

| 冲突 | 处理 |
|---|---|
| 两条规则匹配同一 IP 且 peer 不同 | 按 §10.1 消歧 |
| `cidr` 与 `domains` 在同一条规则内同时命中 | **同一条规则，无冲突** |
| `exit_nodes` 展开的 `/0` 与显式 `/0` 规则 | 合并候选列表，显式在前 |

---

## 11. 开放问题

| # | 问题 | 影响 | 状态 |
|---|---|---|---|
| **Q1** | routes 表插入优先级表的哪一位 | 影响所有用户行为 | ✅ **已定：3.5**（peer proxy LPM 之后、`exit_nodes` 之前） |
| **Q2** | 本地 routes 能否覆盖更具体的 peer proxy CIDR | 语义边界 | ✅ **已定：不能**（子网代理语义更强，LPM 更具体者胜） |
| **Q3** | ~~A 能否读到「A → peer」的 quality score~~ | — | ✅ **已核实可行**：`direct_peer_info` → `direct_peers[B].latency_ms`（§3.9）。代价是复合分不可分解，但排序够用 |
| **Q4** | ~~质量指标更新频率与传播延迟~~ | — | ⚠️ **防抖已有**（`MIN_DELTA=20` + 三重切换门槛）。**peer-center 上报间隔待实测** |
| **Q5** | `manual_routes` 改累加的迁移方案 | 影响现有用户 | ✅ **已定：新增 `manual_routes_append: bool = false`**，默认保持整表覆盖（现状），新行为需显式开启 → **零回归** |
| **Q6** | `if_all_unavailable=closed` 的阻断实现 | 平台差异 | ✅ **已定：分平台** —— Linux 用 `blackhole` 路由类型；其余平台降级为 L3 层拒绝 + 文档明示差异 |
| **Q7** | 故障转移链的合规语义 | 影响配置校验 | ✅ **已定：新增 `require_confirmed: bool`** —— `closed` 且 `peers.len() > 1` 时必须显式确认，否则**配置加载失败** |
| **Q8** | IPv6 / AAAA 支持 | 首期范围 | ✅ **已定：首期仅 IPv4**，IPv6 单独立项 |
| **Q11** | 企业级性能验收指标 | 决定是否达标 | ✅ **已定：见 §12.5**（P99<1μs / 零分配 / 10万条劣化<20% / 快照切换<10ms / 切换≤2s） |
| ~~Q9~~ | ~~配置分发~~ | — | ✅ **已定：首期人工配置** |
| ~~Q10~~ | ~~安全性~~ | — | ✅ **已定：交网关防火墙处理** |
| **Q12** | 域名 IP 来源是否改为订阅 `resolve()`？ | **设计可行性** | ✅ **已定：是** —— 否则外部域名拿不到 IP，域名路由完全失效（冲突 C1） |
| **Q13** | 「`resolve()` 不再返回某 IP」如何处理？ | 影响路由表卫生 | ✅ **已定：方案 C（保留 + 数量上限）** —— 旧 IP 保留，仅在超限时 LRU 淘汰（冲突 C5） |
| **Q14** | 首包最长 1 秒直连窗口是否接受？ | 合规影响 | ✅ **已定：接受** —— hook 内同步更新快照 + 文档明示 + 建议配合静态 hosts（冲突 C1 衍生） |
| **Q15** | 是否扩展 `MagicDnsQueryResolver` 暴露答案来源？ | **设计可行性** | ✅ **已定：是** —— 否则无法区分 MagicDNS 路由 hostname（冲突 C4） |
| **Q16** | 是否在配置校验阶段预检守卫 / split 冲突？ | 影响可诊断性 | ✅ **已定：是** —— 提前报错，避免运行时静默失效（冲突 C3） |
| **Q17** | KCP/QUIC/wrapped-TCP 旁路是否需接入本地路由表？ | 影响传输优化 | ✅ **已定：需要接入** —— 否则三种传输优化全部失效，跨洲高丢包场景性能退化（冲突 C9）。修复点见 §9.4.1 |

### 11.1 拍板结果的影响

| 决策 | 对设计的约束 |
|---|---|
| Q1 = 3.5 | 插入点在 `get_msg_dst_peer_ipv4` 的 `resolve_exit_node_peer()` 调用**之前**（§9.4） |
| Q5 = 开关 | `manual_routes` 语义**默认不变**，累加是 opt-in（§9.3 步骤 ② 需加开关判断） |
| Q6 = 分平台 | 需定义平台能力查询接口；不支持的平台走 L3 拒绝分支 |
| Q7 = `require_confirmed` | 配置结构新增字段；校验逻辑见 §9.10 步骤 ⑤ 升级为**硬失败** |
| Q11 | 性能基准纳入 **CI 门禁**，不达标阻断合入 |

### 11.2 前置依赖与排期弹性

**前置最低限度（技术债 P-T1）已完成（2026-10-13）。** 详见 [`routing-tech-debt.md`](./routing-tech-debt.md) §5 / §10。

| 热路径项 | 当时假设 | 现状（P-T1 后） |
|---|---|---|
| 每包堆分配（单目的地） | ≥1 次 `Vec` | ✅ 内部 `SmallVec<[PeerId; 1]>`；公开 API 仍 `Vec` |
| 可复现基线 | 无 | ✅ `routing_lookup` before：VIP/exit ~165–176 ns，规模不敏感 |
| `routes` tokio 锁（TD-1） | 待清 | ⏸ 暂不实施（固定成本已量化；ROI 差） |
| 出口二次锁 / 重复解析（TD-4/5） | 待清 | ⏸ 暂不实施（与 VIP 同量级） |

> §12「读侧零锁」等仍是**本设计新增路径**的硬约束，不等于要求先做完存量 TD-1。  
> 存量 `PeerMap.routes` 锁：仅当启动本设计前 profiling 仍痛，或大改 `PeerMap` 时，再开 P-T3。

#### 技术债与本设计的依赖

| 技术债阶段 | 状态 | 是否阻塞本设计 |
|---|---|---|
| P-T1 基准 + `SmallVec` | ✅ 已完成 | ❌ 不阻塞（前置已满足） |
| P-T2 出口路径去重 | ⏸ 暂不实施 | ❌ 不阻塞 |
| P-T3 `routes` ArcSwap 化 | ⏸ 暂不实施 | ❌ 不阻塞（本设计新增路径自带无锁快照即可） |

#### 若本设计继续延后，需注意

| 项 | 说明 |
|---|---|
| **代码行号锚点会漂移** | 本文所有 `:NNNN` 锚点届时需重新定位 |
| **设计内容需复核** | §1.1 的约束 C1/C2/C3 可能变化；§11 的拍板结论可能需重审 |
| **P-T1 不会白做** | 基线 harness + SmallVec 已落地，与本设计是否实现无关 |

#### 唯一代码重叠点

两者都会改 `get_msg_dst_peer_ipv4`：

| | 改动 | 类型 | 状态 |
|---|---|---|---|
| 技术债 T1-5 | 内部 `SmallVec`；公开 `PeerManagerCore` 仍 `Vec` | 分配（公开签名不变） | ✅ 已完成 |
| 本设计 P1 | 插入「次序 3.5」分支 | 逻辑新增 | 待启动 |

**建议**：本设计启动时直接在**当前** `get_msg_dst_peer_ipv4`（已 SmallVec）上插入分支即可。

#### 排期（已收敛）

| 方案 | 顺序 | 适用 |
|---|---|---|
| **B（当前推荐）** | ✅ P-T1 已完成 → **可直接评估本设计 P1～P6**；P-T2/T3 仅在有痛点时补 | 默认 |
| ~~A~~ | ~~P-T1~T3 全部做完再进本设计~~ | ❌ 基线后不再推荐 |

---

## 12. 企业级要求

> 本功能面向生产环境，**功能正确性与性能稳定性同等重要，缺一不可**。

### 12.1 性能（硬约束）

| 维度 | 要求 | 方案 | 验证 |
|---|---|---|---|
| **锁** | 读侧**零锁** | `ArcSwap` 快照 | 基准无 lock 竞争 |
| **复杂度** | `by_ip` **O(1)**；CIDR **O(R_cidr)** 顺序 LPM（首期） | `HashMap` + 前缀降序 `cidr_sorted` | 以 **1K / 10K** 规则为主档；10 万 CIDR 需另换 LPM 结构后再承诺 |
| **分配** | 热路径**零分配** | 只返回 `PeerId` / bool；不 clone 规则 | alloc 计数断言 |
| **更新** | 写侧不阻塞读 | 对账构建新快照 → `store()` | 长读压测 |
| **无回归** | 未配置时行为一致 | 次序 3.5 仅在 peer VIP/proxy **未命中**后进入 | 场景 1 + before/after 基线 |

**禁止在 `lookup_peer()` 中：**
- ❌ 任何锁（含 `parking_lot`）
- ❌ DNS / 配置 IO
- ❌ **任何 `async` / `.await`**（含 `get_gateway_peer_id`）
- ❌ 分配（`Vec` / `String` / `clone` / `into_vec`）
- ❌ 现场 `select_peer` / 重算 quality（必须读快照里的 `active_peer`）

### 12.2 并发

| 关注点 | 设计 |
|---|---|
| 读写比 | 读极频繁（每包） / 写极罕见（配置变更） → `ArcSwap` 正合适 |
| 写者 | 后台任务（配置变更 + 对账循环），构建新快照后一次换入 |
| 状态一致性 | `RuleState` 与 `compiled_cidrs` **在同一个新快照内**，读侧永远看到一致状态 |
| 对账与读隔离 | 对账只写快照，不触碰读路径 |
| 多实例 | 每实例独立表，无共享状态 |

### 12.3 可用性

| 场景 | 要求 | 设计 |
|---|---|---|
| 目标掉线 | 切换时间可控 | 1 秒对账（复用 `traffic-steering.md:82`）→ **≤2 秒** |
| 切换期黑洞 | **不允许** | L2 与 L3 在**同一快照**切换 → 不存在中间态 |
| 全部不可达 | 不黑洞 | `open` → 撤销 L2，流量走物理网关<br>`closed` → 显式阻断 + error 日志 |
| 快照切换 | 原子 | `ArcSwap::store()` 一次换，读侧要么全旧要么全新 |
| 配置变更 | 不影响存量流 | 新快照只影响**新建查询** |

#### ⚠️ 必须写进文档的固有限制

> **路由切换不会迁移已建立的连接。**
> TCP 流一旦建立，B 挂掉后该流连接会断，客户端需重连。
>
> 这是三层网络模型的**固有约束**，不是实现缺陷。`failover` 只保证**新流**走备用节点。
>
> **若要求「无缝切换」，需引入连接迁移或双活 NAT 表，已超出本设计范围。**

### 12.4 quality 策略的降级规则

因 cost 是复合分不可分解（§3.9），`degraded` 判定只能基于**绝对阈值**：

```toml
[[routes]]
peers = ["10.144.0.9", "10.144.0.10"]
select_by = "priority"
failover_when = "degraded"
quality_degraded_cost = 500      # cost > 500 视为劣化 → 切下一个
```

> 语义是「这条链路整体质量差」，而非「因为丢包高」。文档必须如此表述，不可暗示能按维度判定。

### 12.5 性能验收指标（**已定**）

| 指标 | 建议阈值 | 测量方法 |
|---|---|---|
| `lookup_peer` P99 | **< 1 μs**（空/`by_ip` 命中） | `criterion`；与 `routing_lookup` 基线同环境对比 |
| 规则规模（首期） | **1K / 10K** 下 P99 相对 1K 劣化 **< 20%** | 三档对比；**100K 列为 stretch**，需真正 LPM 后再承诺 |
| 零分配 | 热路径 alloc = 0 | 分配计数断言 |
| 快照 `store` | P99 < 10 ms（10K 规则） | 对账路径 |
| 切换生效 | ≤ 2 秒 | 拔网线 → **新流**走备用 |
| 稳态吞吐 | 不低于改造前（未配 `routes[]`） | before/after |

### 12.6 合规与泄漏边界（**产品必须明示**）

本设计**不**声称满足审计级「流量零出本地 ISP」。已知窗口：

| 面 | 原因 | 首期态度 |
|---|---|---|
| DNS 上游查询 | MagicDNS 绑物理网卡防成环（冲突 C6） | 文档明示；DNS-over-exit **另立项** |
| 域名首访连接 | L2 装路由受 ~1s 对账约束（§9.8.4） | 接受 + 建议静态 hosts 缩小窗口；或后续「未就绪则 drop」 |
| 无 MagicDNS 平台 | iOS/NE、`no_tun`（冲突 C7） | 功能矩阵标 ❌ |
| peer proxy 优先 | 次序 2+3 盖过本地表 | 文档/UI 明示 |

`if_all_unavailable=closed` 只保证「候选 peer 全挂时不静默改走物理默认」——**盖不住上表泄漏面**。

---

## 13. 依赖图与可交付分支

### 13.1 依赖图

```
D1 配置层            RouteRule / schema / 校验 / exit_nodes 兼容
     ↓
D2 路由表            LocalRouteTable + ArcSwap 快照 + 消歧
     ↓
     ├──┬──────┐
     ↓        ↓
D3 L2 接线   D4 L3 接线          ← 两个高风险点
  累加改造      插入次序 3.5
     └──┬──────┘
        ↓
D5 对账 + failover 状态机（含快照原子换）
        ↓
    ┌───┴───┬──────────┬──────────┐
    ↓       ↓          ↓          ↓
D6 quality  D7 域名     D8 balance  D9 GUI/Web
                        + 文档
```

### 13.2 可独立交付的分支

| 分支 | 内容 | 独立价值 | 工时 |
|---|---|---|---|
| **A（最小可用）** | D1~D5，仅 `priority` | **不同 CIDR 走不同节点 + 优先级故障转移 + 合规阻断** | **3 周** |
| **B** | A + D7 | 域名级分流（**原始需求**） | +1.5 周 |
| **C** | A + D6 | 链路质量优选 | +1 周 |
| **D** | A + D8 | 带宽分摊 | +0.5 周 |
| **E** | 全部 + D9 | 完整交付 | +1 周 |

**分支 A 即覆盖核心需求**；B 覆盖原始需求；C/D/E 为增强。

---

## 14. 风险矩阵

| # | 风险 | 触发 | 影响面 | 探测方法 | 缓解 | 严重度 |
|---|---|---|---|---|---|---|
| **R1** | `manual_routes` 语义变更 | D3 改早退为累加 | **所有用 `manual_routes` 的用户** | 回归：配 manual_routes + peer 宣告网段 | 加 `manual_routes_append: bool = false`，默认保持现状 | 🔴 高 |
| **R2** | L3 优先级表改动 | D4 插入次序 3.5 | 所有实例的选路 | 验收场景 1（空表=基线）、场景 4 | 空表时**完全走原代码路径**；插入位可配置 | 🔴 高 |
| **R3** | **热路径性能退化** | D2/D4 每包新增查找 | 全网吞吐 | **基准测试 before/after**（§12.5） | ArcSwap 无锁 + 预排序 + 零分配；**不达标则阻断合入** | 🔴 高 |
| **R4** | L2/L3 非原子更新 | D5 状态切换 | 瞬时打不中 | 压测中持续 ping | 同一快照内切换（§12.3） | 🟠 中 |
| **R5** | `closed` 阻断跨平台差异 | Q6 未决 | Windows/Linux/macOS/Android 不一致 | 各平台分别验证 | 先只支持 Linux `blackhole`，其余降级为 L3 拒绝 + 文档 | 🟠 中 |
| **R6** | **策略误配** | `closed` + 多候选 | 流量跑到非预期出口 | 配置校验 | Q7：`require_confirmed`，不确认则**加载失败** | 🔴 高 |
| **R7** | DNS 覆盖失效 | 浏览器 Secure DNS 等 | 域名规则静默失效 | 回落时日志 | 回落必打 warn；UI 显示「当前 DNS 未覆盖」 | 🟠 中 |
| **R8** | CDN IP 轮换 | 陈旧 `/32` 残留 | 旧 IP 仍进 TUN | 观测对新 A；表大小 | 方案 C：保留 + 上限 LRU；手动刷新 CLI（**非** TTL 撤销） | 🟠 中 |
| **R13** | 合规期望落差 | 用户以为零泄漏 | DNS/首包仍可能直连 | 文档/UI | §12.6 明示；另立项 DNS-over-exit | 🟠 中 |
| **R9** | quality 精度受限 | 复合分不可分解 | 无法按维度 `degraded` | — | 降级为绝对阈值（§12.4） | 🟢 低 |
| **R10** | quality 切换震荡 | 上报延迟 | 频繁切换 | 长时间压测 | 复用既有双重防抖 | 🟢 低 |
| **R11** | 路由表膨胀 | AWS 类大域名 | 内存/OS 路由表压力 | 监控表大小 | 数量上限 + 防抖合并 + 超限拒绝 | 🟢 低 |
| **R12** | 用户期望「无缝切换」 | 切换不迁移连接 | 认知落差 | 现场反馈 | **文档明示固有限制**（§12.3） | 🟢 低 |

### 三个高风险项的处置

**R1（唯一破坏性变更）**
```rust
// 建议：新增开关，默认关闭
manual_routes_append: bool = false
// false → 保持整表覆盖（现状）
// true  → 改为累加（新行为）
→ 零回归
```

**R2（碰核心选路）**
```rust
// 空表时直接跳过新分支，不引入任何额外开销
if local_route_snapshot.is_empty() {
    // 原代码路径，一行不改
} else {
    // 新逻辑
}
→ 「不配置 = 行为完全不变」= 最强回归保证
```

**R3（性能）**
```
性能基准测试纳入 CI 门禁：
  快照查找 P99 < 1μs  且  零分配  且  10万条规则下劣化 < 20%
不达标 → 阻断合入
```

---

## 15. 分阶段实施

> **原则：每个阶段 1～2 周，独立可测试、可单独上线。** 前一阶段上线不依赖后一阶段；任一阶段可中止而不留下半成品。

**前提：1 人，熟悉本仓库（本文行号锚点有效）。**

### 15.1 阶段总览

| 阶段 | 名称 | 工时 | 交付能力 | 可独立上线 |
|---|---|---|---|---|
| **P1** | 路由表骨架 + L3 接线 + `priority` | **1.5 周** | CIDR → 节点（L3 层） | ✅ |
| **P2** | L2 接线 + 状态机 + 合规阻断 | **1.5 周** | **完整 CIDR 能力** | ✅ |
| **P3** | 域名观测（resolve 订阅 + 消歧） | **1.5 周** | **域名分流（尽力）** | ✅ |
| **P4** | `quality` 策略 | **1 周** | 链路质量优选 | ✅ |
| **P5** | `balance` 策略 | **1 周** | 带宽分摊 | ✅ |
| **P6** | GUI/Web + 文档 + CI 门禁 | **1 周** | 生产就绪 | ✅ |
| | **合计** | **7.5 周** | | |

---

### 15.2 P1 — 路由表骨架 + L3 接线（1.5 周）

| # | 内容 | 对应 |
|---|---|---|
| 1 | `RouteRule` / `SelectBy` / `FailoverWhen` / `AllUnavailable` 结构 + 校验（含 `require_confirmed` 硬失败） | §9.1 §9.10 |
| 2 | `LocalRouteTable` + `ArcSwap`；热路径仅 `lookup_peer`（`by_ip` + `cidr_sorted` LPM） | §9.1 §9.2 |
| 3 | `exit_nodes` 兼容转换（内存展开，不回写） | §9.9 |
| 4 | `get_msg_dst_peer_ipv4` 插入次序 **3.5**（同步 `lookup_peer`，无 await） | §9.4 |
| 5 | **传输优化旁路接入**（`check_allow_wrapped_proxy_to_dst` 先查本地表） | §9.4.1 |
| 6 | 对账循环内 `select_by = priority`（复用 `resolve_exit_node_peer` 语义；写 `active_peer`） | §9.6 §9.7 |
| 7 | 单元测试 + 性能基准（断言零分配、热路径无 async） | §12.1 |

**本阶段能力**：`routes` 表在 **L3 层**生效。CIDR → 节点的选路正确。

**已知限制（必须写进文档）**：
1. **L2 尚未接线**，未命中 peer proxy CIDR 的流量进不了 TUN
2. ⚠️ **首包最长 1 秒直连窗口** —— 见 §9.8.4

> ⚠️ **本阶段单独上线时必须显式标注「实验性，不建议生产使用」**，并在文档中说明需手工配合 `manual_routes` 才能完整工作。

**上线判据**
- 四条基准路径行为与现状**完全一致**（空表时）
- 配置样例可解析、可校验
- 快照查找 P99 < 1 μs、分配计数 = 0
- **§9.4.1 验收场景 1–3 全绿**（KCP/QUIC 逻辑正确性）

---

### 15.3 P2 — L2 接线 + 状态机 + `closed` 阻断（1.5 周）

| # | 内容 | 对应 |
|---|---|---|
| 1 | `resolve_proxy_cidrs` 改累加 + `manual_routes_append` 开关（默认 false = 现状） | §9.3 |
| 2 | 1 秒对账循环 + `RuleState` 状态机 | §7.2 §9.7 |
| 3 | **快照原子换**（L2 与 L3 在同一快照切换 → 消除黑洞） | §12.3 |
| 4 | `if_all_unavailable = closed` 阻断（分平台：Linux `blackhole` / 其余 L3 拒绝） | Q6 |
| 5 | `failover_when = unreachable` 接线 | §9.6 |
| 6 | 集成测试（§16.1 场景 1–7） | §16.1 |

**本阶段能力**：**完整 CIDR 级统一路由** —— 分支 A 交付。

**上线判据**
- 验收场景 1–7 全绿
- 目标掉线 → 新流切换 **≤ 2 秒**
- 切换期间**无黑洞**（P7）
- 压测 24h 无内存增长

---

### 15.4 P3 — 域名观测与编译（1.5 周）

> ⚠️ **以 §9.8 为准**；禁止再实现「查 `DnsConfig.hosts` / TTL 驱动撤销」。

| # | 内容 | 对应 |
|---|---|---|
| 1 | 包装 `MagicDnsQueryResolver`，订阅 `resolve()` 应答中的 QNAME + A 记录 | §9.8.1 / Q12 |
| 2 | 规则权威来自本地 `routes[].domains`；IP 集合来自观测（修正 C2） | §9.8.2 |
| 3 | 扩展 resolver 返回**答案来源**（hosts / route / split / upstream），跳过 MagicDNS 路由 hostname（C4 / Q15） | §9.8.3 |
| 4 | **原子更新**：`compiled_cidrs`（L2）与 `by_ip`（L3）同批 `store` | §9.8.5 / §12.3 |
| 5 | TTL：**不**撤销路由；「不再返回的 IP」按方案 C（保留 + 上限 LRU）（C5 / Q13） | §9.8.5 |
| 6 | 配置预检：与 hosts 守卫 / split 冲突提前报错（C3） | §9.8.5 |
| 7 | 文档/UI：DNS 泄漏、首包 ≤1s 窗口、平台矩阵（C6/C7/§12.6） | §9.8.4 |
| 8 | 验收场景 10–11；**不含**「TTL 到期撤销」 | §16.1 |

**本阶段能力**：不同域名走不同出口（**尽力**；非零泄漏合规）。

**上线判据**
- 验收场景 10–11 全绿
- 观测到新 A 记录后 L2/L3 同批可见；删规则立即撤销对应 `/32`
- 产品文档已写明 §12.6 泄漏面；iOS/NE / `no_tun` 标为不支持

---

### 15.5 P4 — `quality` 策略（1 周）

| # | 内容 | 对应 |
|---|---|---|
| 1 | 对账循环预计算 `peer_quality`（接 `direct_peer_info`，**不在热路径调用**） | §3.9 §9.6 |
| 2 | `select_by = "quality"` 接线（按 cost 排序） | §9.6 |
| 3 | `failover_when = "degraded"` + `quality_degraded_cost` 绝对阈值 | §12.4 |
| 4 | 复用既有双重防抖（`MIN_DELTA=20` + 三重门槛） | §3.9 |

**上线判据**：验收场景 8；24h 压测无切换震荡。

**⚠️ 文档要求**：必须说明 cost 是**复合分**，阈值语义是「这条链路整体质量差」，**不能**按 RTT/loss 维度分解（§3.9 §12.4）。

---

### 15.6 P5 — `balance` 策略（1 周）

| # | 内容 | 对应 |
|---|---|---|
| 1 | 接入 `flow_key_from_payload`（内层 5-tuple 哈希） | §9.6 |
| 2 | 按流一致分配 | — |
| 3 | 故障时重分布的行为说明 | §12.3 |

**上线判据**：验收场景 9 —— **同一条 TCP 流的包恒定走同一节点**。

**⚠️ 文档要求**：必须明示「路由切换**不迁移已建立连接**」（§12.3），且负载均衡下**出口 IP 不唯一**。

---

### 15.7 P6 — GUI/Web + 文档 + CI 门禁（1 周）

| # | 内容 |
|---|---|
| 1 | GUI / Web 配置界面 + i18n |
| 2 | 校验提示（`require_confirmed` 确认对话框等） |
| 3 | 同步更新 4 篇文档（§17） |
| 4 | **性能基准纳入 CI 门禁**：`lookup_peer` P99 < 1 μs、分配 = 0、**10K** 规则劣化 < 20% |
| 5 | 跨平台验证（§16.2 P1–P8） |

**上线判据**：**性能门禁通过** → 此时才称「生产就绪」。

---

### 15.8 里程碑与最短路径

| 里程碑 | 累计 | 覆盖 | 说明 |
|---|---|---|---|
| **M1 = P1+P2** | **3 周** | 不同 CIDR 走不同节点 | 分支 A |
| **M2 = +P3** | **4.5 周** | **原始需求全部** | 分支 B |
| **M3 = +P4/P5** | **6.5 周** | 增强策略 | 分支 C/D |
| **M4 = +P6** | **7.5 周** | 生产就绪 | 分支 E |

> **M2（4.5 周）满足原始需求；M4 才算生产就绪（性能门禁通过）。**

### 15.9 与技术债清理的衔接

```
技术债 P-T1 ✅（基准 + SmallVec）   ← 最低限度已完成（2026-10-13）
        ↓
本设计 P1（1.5 周）                 ← 可随时启动；不依赖 P-T2/T3
        ↓
本设计 P2（1.5 周）→ 里程碑 M1
        ↓
本设计 P3（1.5 周）→ 里程碑 M2
        ↓
P4 / P5 / P6
```

**每一步都可暂停**，且暂停点都有可用的交付物。

详见 [`routing-tech-debt.md`](./routing-tech-debt.md) §8 / §10。

| 情况 | 建议 |
|---|---|
| **当前** | P-T1 已完成 → **可直接进本设计 P1** |
| 稳妥（旧方案） | ~~先做 P-T1~P3~~ → 基线已证明可跳过 |
| 基线数据好 | ✅ **跳过 P-T2/P-T3**，不要为不存在的问题改造 |

---

## 16. 验收场景

### 16.1 功能正确性

| # | 场景 | 期望 | 对应需求 |
|---|---|---|---|
| 1 | `routes` 表为空且未配 `exit_nodes` | 行为与当前主干**完全一致**（回归基线） | NFR-5 |
| 2 | 仅 CIDR → B | 该网段走 B；其余本地直连；**未装 TUN `/0`** | FR-1 |
| 3 | 仅 `0.0.0.0/0` → B | 等价于现有 `exit_nodes=[B]`，行为一致 | FR-8 |
| 4 | 子网代理 + 域名路由并存 | `10.0.0.5` 走 peer X（次序 3）；`1.2.3.4` 走 B（次序 3.5） | FR-2、FR-7 |
| 5 | `peers=[B,C]`，B 不可达，`unreachable` | 切 C；`is_exit_node=true` | FR-3、FR-6 |
| 6 | `peers=[B,C]` 全部不可达，`open` | 不装 L2 路由，流量走物理网关，记 warn | FR-4 |
| 7 | `peers=[B,C]` 全部不可达，`closed` | 阻断 + error 日志 | FR-4 |
| 8 | `quality` 策略 | 选 cost 最优可达节点；`degraded` 时自动切 | FR-5、FR-6 |
| 9 | `balance` 策略 | 同一条 TCP 流的包恒定走同一节点；不同流分散 | FR-5 |
| 10 | 观测到新 A 记录 / 删除域名规则 | L2+L3 同批更新；删规则立即撤销 `/32`（**TTL 到期不撤销**） | FR-2 |
| 11 | 同 IP 属多个域名 | 按 §10 消歧，结果稳定 | FR-2 |
| 12 | `exit_nodes` 与 `routes` 共存 | 两者行为独立，互不干扰 | FR-8 |
| 13 | 配置缺 `select_by` | **加载失败并报错**，不静默取默认 | NFR-6 |
| 14 | `closed` + 多 peer 未确认 | **加载失败**（`require_confirmed`） | FR-10、NFR-6 |

### 16.2 性能与稳定性（企业级门禁）

| # | 场景 | 期望 | 依据 |
|---|---|---|---|
| P1 | `lookup_peer` 基准（1K/10K；`by_ip` 命中） | P99 < 1 μs；无 `.await` | §12.5 |
| P2 | 热路径分配计数 | **= 0** | §12.1 |
| P3 | 规则表为空时吞吐 | 与改造前**持平** | R2 缓解 |
| P4 | 规则表 **10K** 条时吞吐 | 相对 1K 劣化 < 20%（100K = stretch） | §12.5 |
| P5 | 长读压测下快照切换 | P99 < 10 ms | §12.5 |
| P6 | 目标节点掉线 → 新流切换 | **≤ 2 秒** | §12.3 |
| P7 | 切换期间持续探测 | **无黑洞**（L2/L3 原子切换） | R4 缓解 |
| P8 | 长时间压测（≥24h） | 无内存增长、无路由震荡 | R10 缓解 |
| **P9** | **跨境高丢包压测** | **KCP/QUIC 实际生效**（抓包确认） | §9.4.1 场景 4 |

### 16.3 已知固有限制（必须写入用户文档）

| 限制 | 章节 | 说明 |
|---|---|---|
| **DNS 查询不走指定出口** | §9.8.4 / 冲突 C6 | MagicDNS 上游强制绑物理网卡 |
| **首包最长 1 秒直连窗口** | §9.8.4 | L2 路由安装受 1s 对账约束 |
| **切换不迁移已建立连接** | §12.3 | TCP 流需重连 |
| **负载均衡下出口 IP 不唯一** | §6.4 | `balance` 策略的固有语义 |
| **iOS / macOS-NE / `no_tun` 无域名路由** | 冲突 C7 | 这些平台拿不到 DNS |
| **quality 阈值是复合分** | §12.4 | 不能按 RTT/loss 维度分解 |

---

## 17. 需要同步修改的文档

实现时同 PR 更新（按 `docs/README.md` 分区约定）：

- [ ] [`../current/traffic-steering.md`](../current/traffic-steering.md) §1 两层职责 + §2 L3 次序 + §5「域名代理」章节
- [ ] [`traffic-steering-vNext.md`](./traffic-steering-vNext.md) §3 优先级表、§4.3 边界（「域名不替代出口」需修订）
- [ ] [`domain-proxy.md`](./domain-proxy.md) §4.3 组件关系表
- [ ] GUI / Web i18n 帮助文案（只描述 Current 行为）