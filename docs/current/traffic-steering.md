# 流量导流（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-06
- 范围：`exit_nodes` / `enable_exit_node` / `proxy_cidrs` / `manual_routes` / TUN 系统路由同步 / `allow_peer_default_without_exit`
- 规划中的改动见：[`../roadmap/traffic-steering-vNext.md`](../roadmap/traffic-steering-vNext.md)、[`../roadmap/domain-proxy.md`](../roadmap/domain-proxy.md)；默认路由 Phase 2：[`../roadmap/default-route-and-underlay-excludes.md`](../roadmap/default-route-and-underlay-excludes.md)（论证全文 [`../archive/default-route-and-underlay-excludes-2026-10.md`](../archive/default-route-and-underlay-excludes-2026-10.md)）
- DNS：[`magic-dns.md`](./magic-dns.md)

本文只描述 **代码今天做什么**。产品帮助文案应以本文为准。

---

## 1. 两层职责（不要混）

| 层 | 管什么 | 相关配置 |
|----|--------|----------|
| **L2 入口** | 包会不会进 TUN（OS 路由表） | 对端通告的 `proxy_cidrs`（**IPv4**；见 §3 平台说明，IPv6 proxy CIDR 不装）；本机 `exit_nodes` 安装的默认路由；`manual_routes` |
| **L3 选路** | 进 TUN 后交给哪个 peer | VIP、proxy CIDR LPM（不含 `/0`）、`exit_nodes`、`enable_exit_node` |

出口机 **不会** 因为 `enable_exit_node` 向 OSPF 通告 `0.0.0.0/0`。默认路由由**选用出口的客户端本机**安装。

---

## 2. L3 Peer 选路（IPv4）

实现：`easytier-core` → `PeerOutboundPacketRouter::get_msg_dst_peer_ipv4`。

顺序固定：

1. 广播 / 网段广播 → 多 peer  
2. VIP 精确命中  
3. 若目标不在本机虚拟网网段：proxy CIDR **最长前缀匹配，忽略 `0.0.0.0/0`**（更具体子网代理）  
4. 仍未命中且目标不在本网段 → 按 **`exit_nodes` 列表顺序**，第一个 VIP 可解析 **且有下一跳** 的 peer，并设置 `is_exit_node=true`  
5. 出口列表为空或无法解析/无下一跳时：才使用对端通告的 `0.0.0.0/0`（子网代理兜底，**不**打 `exit_node` 标志）  
6. （部分宿主，如 OHOS）`local_exit_node_fallback` → 本机出口  

IPv6：VIP / 非 `/0` 的 proxy LPM → `exit_nodes`（同样要求下一跳）→ Public IPv6 gateway → 对端 `::/0` 兜底；链路本地地址不进出口。

因此：

- 更具体的子网代理 CIDR **永远优先于** `exit_nodes`  
- 配置了可解析的 `exit_nodes` 时，公网走出口路径并带 `exit_node` 标志，不会被对端 `/0` 抢走  

出口机侧：

- 带 `exit_node` 标志的包：需 `enable_exit_node`（或宿主强制）才由 TCP/UDP/ICMP 代理接  
- 仅靠对方通告的非默认 CIDR 命中时：按子网代理 `lookup`，不依赖 `exit_node` 标志  

---

## 3. L2 系统路由（D+）

实现：`easytier-core` → `resolve_proxy_cidrs`；`easytier` → `virtual_nic` 的 `apply_route_changes`。

- 期望集合 = 对端 OSPF 通告的 proxy CIDR（排除本节点）  
- **对端通告的 `0.0.0.0/0` 默认不进入期望集合**（不写入 OS）。L3 仍可作选路兜底。  
- **本机 `exit_nodes` 中至少有一个 VIP 可解析且有下一跳**、未开 `manual_routes`、未 `no_tun`：加入本机管理的 `0.0.0.0/0`（低度量）；IPv4 `/0` **成功装上之后**再装 `::/0`（按 TUN 接口）  
- 逃生阀：`allow_peer_default_without_exit=true` 时，无 exit 也可把对端 `/0` 装进 OS（桌面用高度量；移动端无度量概念，会成为真默认）  
- 装 TUN 默认路由前，会先把 **underlay 排除宿主路由**（`/32`/`/128`）装到**物理默认网关**上：已连接 peer 隧道的 `resolved_remote_addr`、对端 `stun_info.public_ip`、以及本进程 **config-server / 管理面** 连接目标。卸默认时**先卸 TUN `/0`，再卸排除**  
- 桌面 DNS：TUN 就绪后，peer / STUN / config-server 域名解析经 `RuntimeDnsResolver`，hickory 套接字绑定物理默认网卡，避免首次查询被 TUN `/0` 吸走  
- 排除门控：**期望集合将含 TUN `/0`** 时才装（覆盖 exit、逃生阀、`manual_routes` 含 `/0`）  
- ACL / KCP / QUIC / wrapped-TCP 等旁路通过 `get_peer_id_by_ip_allowing_default_proxy` 仍可解析对端通告的 `/0`  
- `manual_routes` 开启：整表由手动列表覆盖，**不**自动加出口默认路由  
- `enable_exit_node` **不**向全网通告 `/0`  
- 可观测：`NetworkInstanceRunningInfo.proxy_cidr_route_sync` 展示「期望 vs 已安装 vs 最后错误」摘要  
- `add`/`remove` **仅成功时**记入已安装集合；失败 warn，1 秒 interval 对照期望集合重试  

### 平台差异

| 平台 | 路由安装方 | 对端通告的 `/0` 会怎样 |
|------|-----------|----------------------|
| Windows / Linux / macOS（非 NE）/ FreeBSD | 本进程 `apply_route_changes` | **默认不装**；仅 `allow_peer_default_without_exit` 或本机 exit 时进入期望集合 |
| Android（GUI / Web） | `VpnService.Builder.addRoute`，由 GUI `getRoutesForVpn` 汇总 | 无**可达** exit 且未开逃生阀时**过滤**对端 `/0`；exit VIP 出现在路由表且有下一跳时显式加 `/0` |
| Android（`easytier-android-jni`） | Kotlin 汇集 peer `proxy_cidrs` | 同上（按 NetworkConfig 的 exit VIP 可达性 / 逃生阀） |
| OHOS | `aggregate_tun_routes` | 无可达 exit 且未开逃生阀时过滤对端 `/0`；可达时插入 `/0`（再 `simplify`） |
| iOS / macOS NE | 宿主 App / NE `includedRoutes` | 本仓库无法约束 |

补充：对端宣告的 **IPv6 proxy CIDR 目前不写入任何 OS 路由**（`::/0` 只在本机出口时装）。

---

## 4. 与 UI 文案

`exit_nodes` 的含义是：**本机把默认上网路由指向 TUN，进网后按出口列表转发**。出口机需开启 `enable_exit_node`。不要写成「出口机会向全网推送 0.0.0.0/0」。

对端通告的 `/0`：**不**自动变成「本机默认路由」；需要全隧道请配 `exit_nodes`，或显式 `allow_peer_default_without_exit` / `manual_routes`。

---

## 5. 域名代理

**未实现。** 现状仅有手工 `proxy_cidrs` + MagicDNS 等基础设施（DNS 侧见 [`magic-dns.md`](./magic-dns.md)）。  
见 [`../roadmap/domain-proxy.md`](../roadmap/domain-proxy.md)。

---

## 6. 验收对照（现状 / D+）

| 场景 | 期望 |
|------|----------------|
| 本机 `exit_nodes` 可解析且有下一跳，未开 `manual_routes` / `no_tun` | TUN 安装 `0.0.0.0/0` 与 `::/0`；有 underlay 排除；公网 L3 走出口并带 `exit_node` 标志 |
| `exit_nodes` 非空但全部不可达 | **不**装本机默认路由；公网走系统原默认网关 |
| 清空 `exit_nodes` 或出口掉线 | 卸掉上述默认路由与排除；对端子网 CIDR 保留 |
| 只配 `enable_exit_node`，未配 `exit_nodes`、也未宣告 `/0` | 系统路由表通常 **不会** 出现 TUN 默认路由 |
| 对端通告 `10.0.0.0/24` | 本机可同步安装对应 TUN 路由；命中走 LPM，非 exit 标志 |
| 对端通告 `0.0.0.0/0`，本机 **未** 配出口，逃生阀关（默认） | OS **无** TUN `/0`；L3 仍可按兜底选路 |
| 对端通告 `0.0.0.0/0`，本机未配出口，`allow_peer_default_without_exit=true` | 桌面装高度量 `/0`（有物理默认时通常不抢；无物理默认时会接管）；移动端会成为真默认 |
| 对端通告 `0.0.0.0/0`，本机 **已** 配可解析出口 | L2 装低度量 `/0`；L3 走 `exit_nodes`，带 `exit_node` 标志 |
| `manual_routes` 开启 | L2 完全由手动列表决定 |
| 移动端同样的对端 `/0` + 无 exit | **不**再整机吸流（与桌面一致）；有 exit 才全隧道 |
| 对端通告 `::/0` | 本机**不**装任何 IPv6 默认路由（`::/0` 只随本机出口安装） |
