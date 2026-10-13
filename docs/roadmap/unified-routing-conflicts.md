# 统一路由模型 × 现有功能 冲突分析

## Status

- Status: **Roadmap**（Risk Analysis / 已完成代码核查）
- 最近审阅：2026-10-13
- 对象：[`unified-routing-design.md`](./unified-routing-design.md)（统一路由模型）
- 核查方式：逐行阅读 `easytier-core` / `easytier` 相关实现 + `docs/current/` 现状文档
- 索引：[`../README.md`](../README.md)

> **结论先行**：发现 **8 处交互点**，其中 **2 处是设计缺陷必须修正**（C1 / C2），**3 处需在实现中显式处理**（C3 / C4 / C5），**3 处已核实无冲突**（C6 / C7 / C8）。

---

## 冲突汇总

| ID | 交互点 | 严重度 | 类型 | 需修正设计 |
|---|---|---|---|---|
| **C1** | IP 来源假设错误 | 🔴 **严重** | **设计缺陷** | ✅ 必须 |
| **C2** | `static_hosts` 断连清理 | 🔴 **严重** | **设计缺陷** | ✅ 必须 |
| **C3** | MagicDNS 守卫规则静默丢弃条目 | 🟠 重要 | 实现约束 | ⚠️ 需处理 |
| **C4** | 解析优先级与路由语义不一致 | 🟠 重要 | 实现约束 | ⚠️ 需处理 |
| **C5** | TTL 语义冲突 | 🟠 重要 | 设计细节 | ⚠️ 需澄清 |
| **C6** | DNS 上游强制走物理网卡（DNS 泄漏） | 🟡 需明示 | **固有约束** | 📝 文档 |
| **C7** | 平台无 DNS 接线 | 🟡 需明示 | 平台限制 | 📝 文档 |
| **C8** | Fake IP / ACL 顺序 | 🟢 低 | **无冲突** | ❌ |
| **C9** | KCP/QUIC/wrapped-TCP 旁路绕过本地路由表 | 🟠 重要 | **实现缺口** | ✅ 必须 |

> **结论**：**2 处设计缺陷必须修正**（C1 / C2）+ **4 处需在实现中处理**（C3 / C4 / C5 / **C9**）+ **3 处已核实无冲突或需文档明示**（C6 / C7 / C8）。

---

## C1 🔴 IP 来源假设错误（设计缺陷）

### 问题

设计文档 §9.8 的伪代码假设 IP 来自静态 hosts：

```rust
if let Some(ips) = dns.static_hosts_lookup(pat).await {   // ← 只查 hosts
    cidrs.extend(ips.into_iter().map(|ip| Ipv4Cidr::new(ip, 32).unwrap()));
}
```

**但外部域名的解析答案根本不来自 hosts。**

`docs/current/magic-dns.md` §2 定义的本地解析优先级：

| 次序 | 来源 | 适用 |
|---|---|---|
| 1 | 静态 hosts（`DnsConfig.hosts`） | 用户显式配置的条目 |
| 2 | MagicDNS 权威 zone（路由 hostname） | mesh 内节点名 |
| 3 | Split forwarder | 特定后缀 → 指定上游 |
| 4 | **默认上游**（空则系统 DNS） | **其余全部** |

`atlassian.net` / `amazon.com` 这类外部域名走的是**第 4 级**。

**结果：用户不会为每个外部域名配 hosts 条目 → 域名路由拿不到 IP → 规则静默失效。**

### 根因

`DnsConfig.hosts` 是「**静态映射**」功能，设计目标是内外网分离解析（split-horizon），不是「为路由提供域名列表」。把两者混为一谈是设计错误。

### 正确的 hook 点

```rust
// easytier-core/src/gateway/magic_dns/packet.rs:21-24
#[async_trait]
pub trait MagicDnsQueryResolver: Send + Sync + 'static {
    async fn resolve(&self, query: MagicDnsQuery) -> Option<Vec<u8>>;
    //                                ↑ MagicDnsQuery { source, payload }  ← 原始 DNS 请求
    //                                                             ↑ 返回完整 DNS 响应包
}
```

**这是全代码库中唯一能看到「应用实际解析了什么域名、得到什么答案」的位置。**

它被安装为 NIC 包过滤器：

```rust
// easytier-core/src/instance/packet_plane.rs:149-164
pub async fn register_magic_dns_resolver(
    &self, fake_ip, resolver: Arc<dyn MagicDnsQueryResolver>,
) -> MagicDnsResolverRegistration {
    let pipeline = self.peer_manager
        .add_managed_nic_packet_process_pipeline(magic_dns_packet_filter(
            fake_ip, self.peer_manager.my_peer_id(), resolver,
        )).await;
    ...
}
```

### 修正方案

**从「查 hosts 表」改为「订阅 `resolve()` 的实际返回值」。**

```
MagicDnsPacketFilter 拦截 dst==10.255.255.254 的 DNS 查询
      ↓
resolver.resolve(query)  → 走四级优先级解析
      ↓
【新增 hook】解析响应包中的 QNAME + A 记录
      ↓
更新 DomainRouteTable：domain → [IP...]
```

**收益**：

| | 静态 hosts 方案 | 订阅 resolve() 方案 |
|---|---|---|
| CDN 换 IP | ❌ 用户须手工更新 | ✅ **自动跟随** |
| Split DNS | ❌ 绕过 | ✅ **天然尊重**（走的就是 split 路径） |
| hosts 优先级 | ✅ | ✅（走的就是 hosts 路径） |
| 未访问过的域名 | 编译了路由（浪费） | **不编译**（按需） |
| 多 A 记录 | 单值 | ✅ 全部纳入 |

### ⚠️ 由此引出的新问题：首包泄漏

```
t=0    App 查询 atlassian.net → MagicDNS 本地应答（不过 TUN）
t=0+ε  hook 记录 atlassian.net → 1.2.3.4
t=0+2ε 编译 1.2.3.4/32 → 装 OS 路由
t=0+3ε App 连接 1.2.3.4 → 进 TUN → 走指定节点 ✅
```

若编译是**异步**（设计文档 §9.7 的 1 秒对账循环），则 `t=0+ε` 到 `t=0+2ε` 之间的**首个连接包会走物理网关**。

**对合规场景（必须走美国），首包直连即违规。**

| 方案 | 说明 | 代价 |
|---|---|---|
| **A：hook 内同步更新快照** | `resolve()` 返回前就 `ArcSwap::store()`，时序压到微秒级 | 仍有竞态窗口，但不是 1 秒 |
| **B：DNS 查询本身也走指定节点** | 见 C6 —— ❌ 做不到 | — |
| **C：显式接受首包泄漏** | 要求用户用静态 hosts 消除时序差 | 牺牲「自动跟随」优势 |

**建议**：取 **A + 文档明示**。窗口从 1 秒降到微秒级，并在文档中说明「若合规要求零泄漏，请配合静态 hosts 使用」。

---

## C2 🔴 `static_hosts` 断连清理（设计缺陷）

### 问题

若从 **MagicDNS server 的 record store** 读取条目，会遇到：

```markdown
docs/current/magic-dns.md:78
| MagicDNS 静态通道 | 已开 MagicDNS 的 mesh 节点 |
  UpdateDnsRecord + client="static-hosts" → static-hosts:<tunnel>；
  断连清理；TTL 长于路由 1s |
```

测试 `test_static_hosts_cleared_on_client_disconnect`（`dns_server/tests.rs:732`）确认：**隧道断开即清理静态 hosts 记录**。

**后果**：mesh 断连 → 条目消失 → 编译出的 `/32` 撤销 → **流量中断**。而 mesh 断连时用户往往正需要这条链路兜底。

### 修正方案

**区分两个数据源：**

| 来源 | 用途 | 是否受断连影响 |
|---|---|---|
| 本地 `DnsConfig`（配置真值） | **权威**：判断「用户配了哪些域名」 | ❌ 不受影响 |
| `resolve()` 实际返回值 | **观测**：判断「当前解析到什么 IP」 | ⚠️ 受影响（但仅 IP 集合，非配置） |

**规则**：

```rust
// 域名规则是否存在 → 只看本地 DnsConfig
if !local_dns_config.contains_domain(pattern) { 规则不生效 }

// IP 集合 → 来自 resolve() 观测，可以为空
// 但「配置存在 + IP 为空」是一个必须告警的状态，不是静默失效
```

**告警要求**：配置了域名但观测不到任何 IP 时，**必须发 warn 并在 UI 展示**，否则用户会遇到「配了没反应」且无线索。

---

## C3 🟠 MagicDNS 守卫规则会静默丢弃条目

### 问题

`magic-dns.md` §3 定义了 4 条守卫，违反时**条目被静默跳过**：

| 守卫 | 内容 | 对域名路由的影响 |
|---|---|---|
| 单标签 zone | `*.com` / `com` 被拒（`MIN_STATIC_HOST_ZONE_LABELS = 2`） | `domains = ["*.com"]` 无法工作 |
| **route zone 同名** | 静态条目的 zone 与任一 MagicDNS 路由 zone 同名 → 跳过（精确与通配都查） | **若某 peer 通告了同名 zone，该域名的 hosts 条目消失** |
| 路由 TLD | zone 等于 `et.net.` → 跳过 | 内网域名，本就不该走域名路由 |
| **Split 占父 zone** | 通配的父 zone 已被 Split forwarder 占用 → 跳过（Split 赢） | **若用户同时配了 split DNS，域名路由失效** |

skip 判定共用 `server_instance.rs::static_host_skip_reason()`。

### 冲突场景

```toml
# 用户想给 atlassian.net 做路由分流
[ dns_config ]
hosts = [{ name = "*.atlassian.net", ips = [...] }]

# 但企业同时配了 Split DNS
forwarders = [{ domains = ["corp.atlassian.net"], servers = ["10.0.0.53"] }]
#         ↑ 若 atlassian.net 被 split 占用，通配条目被跳过
```

### 处理要求

1. **域名路由必须能感知「对应条目被守卫跳过」** → 读 `static_host_skip_reason()` 的判定结果
2. **UI 必须展示跳过原因** —— 沿用现有控制台提示模式
3. **配置校验阶段预检**：写 `routes[].domains` 时就检查是否会与现有 hosts/split 冲突，**提前报错**而非运行时静默

---

## C4 🟠 解析优先级与路由语义不一致

### 问题

优先级第 2 级是 **MagicDNS 权威 zone（路由 hostname）**，答案是 EasyTier **虚拟 IP**（如 `10.144.144.5`）：

```markdown
magic-dns.md:21
1. 路由 hostname → A（mesh 内节点名，TTL 约 1s）
```

### 冲突

若用户为 `*.et.net` 或某个内网 hostname 配了域名路由：

| 情况 | 期望 | 实际 |
|---|---|---|
| `app.et.net` 是 mesh 内节点名 | 走 EasyTier 内网路由 | DNS 答虚拟 IP → **本就该走 mesh，不该走域名路由** |

**结论：域名路由应主动排除 MagicDNS 路由 hostname 类域名。**

### 处理要求

订阅 `resolve()` 时需要区分答案来源：

```
若 QNAME 命中 MagicDNS 路由 zone
   → 该域名本就属于 mesh，跳过域名路由
若 QNAME 命中静态 hosts
   → 按 C1 的规则处理（权威来自本地配置）
若 QNAME 走上游
   → 正常纳入域名路由
```

**因此 hook 不能只读响应包，还要能区分解析路径。** 建议扩展 `MagicDnsQueryResolver` 返回值，携带「来源」信息（hosts / route / split / upstream）。

---

## C5 🟠 TTL 语义冲突

### 问题

设计文档 §9.8 假设「TTL 驱动撤销」：

```rust
sleep(ttl_recheck_interval).await;   // 建议 min(ttl/2, 60s)
```

**但 hosts 条目没有「自动过期」机制。**

| 项 | 实际语义 |
|---|---|
| `DnsHostEntry.ttl_secs`（默认 300s） | **DNS 响应的 TTL**，告诉客户端缓存多久 |
| 条目生命周期 | **只由显式配置变更/删除决定**，不随时间消失 |

**后果**：若按 TTL 撤销路由，会在条目仍然有效时误撤销 → **流量断**。

### 澄清后的规则

| 事件 | 路由处理 |
|---|---|
| `resolve()` 返回新 IP | **新增** `/32`，不撤销旧的（旧 IP 保留直到显式清理） |
| `resolve()` 不再返回某 IP | 需**判定策略**：见下 |
| hosts 条目被**删除** | **立即撤销**对应 `/32` |
| TTL 到期 | **不触发任何路由变更** |

**「不再返回某 IP」的处理是设计缺口**，需拍板：

| 方案 | 说明 | 风险 |
|---|---|---|
| A：保留旧 IP | 永不误删，但陈旧路由残留 | CDN 换 IP 后旧 IP 可能已被回收 |
| B：超时清理 | N 分钟未见即撤销 | 需定 N；长时间空闲的域名会被误删 |
| **C：保留 + 上限**（建议） | 保留旧 IP，但单域名 `/32` 数量设上限，超限按 LRU 淘汰 | 实现略复杂，行为可预测 |

---

## C6 🟡 DNS 上游强制走物理网卡（DNS 泄漏）

### 现状

```markdown
magic-dns.md:92
转发套接字经 magic_dns_forward_connector() 绑物理默认网卡
（与 RuntimeDnsResolver / TUN exclude 同族），
避免上游指 VPN 内地址时进 TUN 成环。
```

**MagicDNS 的上游查询被强制绑定物理网卡，不会进 TUN。**

### 后果

即使配了「`atlassian.net` 走美国节点」：

```
DNS 查询 atlassian.net  →  从本地 ISP 发出  ⚠️ DNS 泄漏
后续连接 1.2.3.4        →  走美国节点        ✅
```

**对合规场景，这是需要明示的固有约束** —— 至少让用户知情。

### 可选缓解（未来）

- 让域名规则支持同时指定「DNS 也走该节点」（需改 `magic_dns_forward_connector`，会引入成环风险）
- 首期不做，但**文档必须写明**

---

## C7 🟡 平台无 DNS 接线

```markdown
magic-dns.md:110-112
| Android Tauri / JNI | ✅ | VpnService DNS = fake_ip + /32 |
| iOS / macOS-NE      | ❌ | 首期明确不支持 |
| no_tun             | ❌ | 不启动 MagicDNS Runner |
```

| 平台 | 域名路由可用性 |
|---|---|
| Windows / macOS(非 NE) / Linux(systemd) / OpenWrt(手工) | ✅ |
| **iOS / macOS NE** | ❌ OS 不走 EasyTier DNS → 拿不到 IP |
| **`no_tun` 模式** | ❌ MagicDNS 不启动 |
| Android | ✅（但需注意 JNI 回落运营商 DNS 的分支） |

**必须在文档的功能矩阵中标注平台差异。**

---

## C8 🟢 Fake IP 与 ACL —— 已核实无冲突

### Fake IP 无冲突

`MAGIC_DNS_FAKE_IP = 10.255.255.254` 是 **MagicDNS 服务的监听地址**，**不是** Tailscale 式的「假答案」。

```rust
// magic_dns/packet.rs:123
if ip_packet.version() != 4 || ip_packet.dst_addr() != fake_ip {
    return false;      // ← 只拦截发给 MagicDNS 自己的包
}
```

执行顺序：

```
App → UDP 10.255.255.254:53
      ↓ 进入 TUN → send_msg_by_ip
      ↓ :2910 run_nic_packet_process_pipeline
      │    └─ MagicDnsPacketFilter 命中 → 本地应答
      ↓ :2931 get_msg_dst_peer_ipv4   ← 永远不会到这里
```

**MagicDNS 的包在路由之前就被拦截，不会走域名路由。** ✅

> 唯一的运营约束：该地址的 `/32` 路由由 MagicDNS 机制自己维护，设计文档 §9.3 改造 `resolve_proxy_cidrs` 时**不要把它清掉**。

### ACL 无冲突

ACL 在 `run_nic_packet_process_pipeline` 内执行（`peer_manager.rs:2642`），**早于路由**。

```
:2642 acl_filter.process_packet_with_acl()    ← ACL 先
:2652 nic_packet_process_pipeline              ← MagicDNS filter
:2931 get_msg_dst_peer_ipv4                    ← 路由最后
```

**两者都是包过滤器，不是选路器，互不干扰。**

ACL 有自己的缓存（`acl/processor.rs:83` `AclCacheKey`），不经过 `get_msg_dst_peer_ipv4`，因此不受设计 §8.2 优先级表改动影响。

> `traffic-steering.md:76` 提到「ACL / KCP / QUIC 等旁路通过 `get_peer_id_by_ip_allowing_default_proxy` 仍可解析对端 `/0`」—— 这条路径**不走**新增的次序 3.5，**需确认是否期望**（见 §4 Q1）。

---

## C9 🟠 KCP / QUIC / wrapped-TCP 旁路绕过本地路由表

### 背景：`get_peer_id_by_ip_allowing_default_proxy` 是有意设计

```rust
// easytier-core/src/peers/route/mod.rs:146-159
/// VIP / more-specific proxy CIDR first, then peer-advertised `/0`/`::/0`.
///
/// Used by ACL / KCP / QUIC / wrapped-TCP service paths that need the owning
/// peer even when only a default-route proxy matches. Must **not** be used by
/// the L3 outbound packet router, which keeps `exit_nodes` ahead of peer `/0`.
async fn get_peer_id_by_ip_allowing_default_proxy(&self, ip: &IpAddr) -> Option<PeerId> {
    if let Some(peer_id) = self.get_peer_id_by_ip(ip).await { return Some(peer_id); }
    self.get_peer_id_for_default_route_proxy(ip).await
}
```

**这不是遗漏，是刻意与 L3 router 区分**（注释明确写了「Must **not** be used by the L3 outbound packet router」）。

### 但它不查 LocalRouteTable

```rust
// easytier-core/src/peers/peer_manager.rs:2673-2684（check_allow_wrapped_proxy_to_dst 内）
let Some(dst_peer_id) = self.route
    .get_peer_id_by_ip_allowing_default_proxy(dst_ip).await
else {
    return false;      // ← 找不到 owner → false
};
```

### 触发场景

```
A 配：routes = [{ cidr = "1.2.3.4/32", peers = ["B"] }]

L3 选路（次序 3.5）        → 命中 B → 正常发包 ✅
check_allow_kcp_to_dst     → 不查 LocalRouteTable → None → false
check_allow_quic_to_dst    → 同上 → false
wrapped-TCP 传输选择        → 同上 → 不启用
```

调用点：

| 调用方 | 位置 | 后果 |
|---|---|---|
| `check_allow_kcp_to_dst` | `dataplane/mod.rs:627` | KCP 传输不启用 |
| `check_allow_quic_to_dst` | `dataplane/mod.rs:627` 同分支 | QUIC 传输不启用 |
| `wrapped_transport/packet_plane.rs:264-277` | 闭包返回 bool | wrapped-TCP 不启用 |

### 影响评估

| 维度 | 影响 |
|---|---|
| **功能** | ✅ **不损坏** —— 三者都是「门禁」而非「必需」，false 时回落标准中继 |
| **性能** | ⚠️ **可能显著** —— KCP / QUIC 正是为**高丢包环境**优化的传输 |

**本项目场景是跨洲链路（国内 → 美国），丢包是常态** → 失去这三条传输优化可能造成**可感知的性能退化**。

⚠️ **这是最隐蔽的一类问题**：功能测试全绿（流量能通），但用户实际体验变差。

### ACL 不受影响 ✅

```rust
// easytier-core/src/peers/acl/filter.rs:373-376
let dst_groups = packet
    .get_dst_peer_id()          // ← 读包头，不是路由查询
    .map(|peer_id| route.get_peer_groups(peer_id))
    .unwrap_or_else(|| Arc::new(Vec::new()));
```

ACL 从**包头**取 peer id（由发送方填好），**不调用** `get_peer_id_by_ip_allowing_default_proxy`，因此不受影响。

### 修复方案（不改 `Route` trait）

`check_allow_wrapped_proxy_to_dst` 本身挂在 `PeerOutboundPacketRouter` 上（`peer_manager.rs:2673`，struct 定义 `:2552`），**可直接访问 `self.local_route_table`**：

```rust
async fn check_allow_wrapped_proxy_to_dst(&self, dst_ip, ...) -> bool {
    // ① 先查本地 routes 表（次序 3.5）
    let owner = if let Some(peer_id) = self.local_route_table.lookup(dst_ip).await {
        Some(peer_id)
    } else {
        // ② 再走原有逻辑（VIP / proxy CIDR / 对端 /0）
        self.route.get_peer_id_by_ip_allowing_default_proxy(dst_ip).await
    };

    let Some(dst_peer_id) = owner else { return false };
    // ...原有 feature flag 检查不变
}
```

**不修改 `Route` trait**，避免跨模块耦合（`Route` 在 `peers/route`，`LocalRouteTable` 由 `PeerOutboundPacketRouter` 持有）。

### 必须补的测试

| # | 场景 | 期望 |
|---|---|---|
| 1 | `1.2.3.4 → B`，且 B 支持 KCP | KCP **启用**（回归基线） |
| 2 | `1.2.3.4 → B`，且 B 不支持 KCP | KCP 禁用，回落标准中继 |
| 3 | 无 LocalRouteTable 命中 | 行为与现状**完全一致** |
| 4 | 跨境高丢包压测 | KCP/QUIC 实际生效（抓包确认） |

> **第 4 条是真正的验收** —— 前三条只验证逻辑正确性，第 4 条才验证性能没退化。

---

## 需拍板的新增问题

| # | 问题 | 建议 | 关联 |
|---|---|---|---|
| **Q12** | IP 来源改为订阅 `resolve()`？ | ✅ 是（否则 C1 无解） | C1 |
| **Q13** | 「不再返回某 IP」如何处理？ | **C 方案：保留 + 上限** | C5 |
| **Q14** | 首包泄漏是否接受？ | 接受，但用**同步更新**压缩窗口 + 文档明示 | C1 |
| **Q15** | 是否扩展 `MagicDnsQueryResolver` 暴露答案来源？ | ✅ 是（否则 C4 无解） | C4 |
| **Q16** | 域名路由是否需检查守卫冲突并预检报错？ | ✅ 是 | C3 |
| **Q18** | `check_allow_wrapped_proxy_to_dst` 是否需接入 LocalRouteTable？ | ✅ **是** —— 否则 KCP/QUIC/wrapped-TCP 全部失效（跨洲高丢包场景性能退化） | C9 |

**Q15 ~ Q18 已全部拍板，实现时直接执行。**

---

## 对设计的修订建议汇总

| 修订项 | 位置 | 性质 |
|---|---|---|
| §9.8 IP 来源：hosts 查询 → **订阅 `resolve()`** | §9.8 | **必须** |
| §9.8 TTL 驱动撤销 → **配置变更驱动** | §9.8 | **必须** |
| hook 内**同步**更新快照（非等 1 秒对账） | §9.7 §9.8 | **必须**（缓解 C1 时序） |
| 区分「本地配置」与「观测 IP 集合」 | §9.8 | **必须**（C2） |
| 排除 MagicDNS 路由 hostname 类域名 | §9.8 | 需（Q15） |
| 配置校验阶段预检守卫/ split 冲突 | §9.10 | 需（C3） |
| **`check_allow_wrapped_proxy_to_dst` 接入 LocalRouteTable** | §9.4 | **需（C9）** |
| 「不再返回」的撤销策略 | §7 §9 | 需（Q13） |
| 文档明示 DNS 泄漏与平台差异 | §12 §17 | 需（C6 / C7） |
| 确认 ACL 旁路是否应走次序 3.5 | §8.2 | 待确认（Q1 相关） |
| `resolve_proxy_cidrs` 改造时保留 MagicDNS `/32` | §9.3 | 需（C8） |

---

## 结论

- **C1 / C2 是设计缺陷，必须在动工前修正**，否则域名路由对外部域名完全无效
- **C1 的修正方向是「订阅 `MagicDnsQueryResolver::resolve()`」** —— 这是代码库里唯一正确的 hook 点，且天然尊重 hosts / split / 上游的全部分支
- **C9 是最容易漏的性能坑** —— 功能测试全绿，但 KCP/QUIC/wrapped-TCP 全部失效，跨洲高丢包场景会明显退化
- **C3 / C4 / C5 是实现约束**，需要在配置校验与运行时告警中显式处理
- **C6 / C7 是固有约束**，必须写进用户文档，不能埋雷
- **C8 已核实无冲突**，但需注意别在 `resolve_proxy_cidrs` 改造中误删 MagicDNS 的 `/32`