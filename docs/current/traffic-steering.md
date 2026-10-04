# 流量导流（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-03
- 范围：`exit_nodes` / `enable_exit_node` / `proxy_cidrs` / `manual_routes` / TUN 系统路由同步
- 规划中的改动见：[`../roadmap/traffic-steering-vNext.md`](../roadmap/traffic-steering-vNext.md)、[`../roadmap/default-route-and-underlay-excludes.md`](../roadmap/default-route-and-underlay-excludes.md)、[`../roadmap/domain-proxy.md`](../roadmap/domain-proxy.md)

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

## 3. L2 系统路由

实现：`easytier-core` → `resolve_proxy_cidrs`；`easytier` → `virtual_nic` 的 `apply_route_changes`。

- 期望集合 = 对端 OSPF 通告的 proxy CIDR（排除本节点）  
- **本机 `exit_nodes` 中至少有一个 VIP 可解析且有下一跳**、未开 `manual_routes`、未 `no_tun`：另外加入本机管理的 `0.0.0.0/0`（低度量）；IPv4 `/0` **成功装上之后**再装 `::/0`（按 TUN 接口）  
- 装本机出口默认路由前，会先把 **underlay 排除宿主路由**（`/32`/`/128`）装到**物理默认网关**上：已连接 peer 隧道的 `resolved_remote_addr`、对端 `stun_info.public_ip`（公网地址）、以及本进程 **config-server / 管理面** 连接目标（URL 字面量 IP、隧道远端、DNS 解析结果中的公网地址，DNS 有 TTL 缓存）。避免 P2P/打洞与管理面心跳被吸进 TUN；卸默认路由时**先卸 TUN `/0`，再卸排除路由**；updater 正常退出也会清理排除路由  
- ACL / KCP / QUIC / wrapped-TCP 等旁路通过 `get_peer_id_by_ip_allowing_default_proxy` 仍可解析对端通告的 `/0`；L3 出站顺序仍是「更具体 CIDR → `exit_nodes` → 对端 `/0`」  
- 对端宣告的 `0.0.0.0/0`（非子网出口）仍用高度量（Windows 9000 / Linux 65535 / Darwin 7；本机出口默认用的是 1 / 50 / 1），避免没配出口时抢物理默认网关。**注意**：EasyTier 只写路由度量，**从不设置接口度量**（全仓库无 `InterfaceMetric` / `UseAutomaticMetric` / `netsh`），Windows 显示值为「路由度量 + 系统决定的接口度量」，因此「高度量」只是相对优先级，不是硬保证  
- 出口不可达或清空 `exit_nodes`：卸掉本机默认路由与上述排除路由，其它 CIDR 不动；卸 `::/0` 后若本机仍是 Public IPv6 提供者，按原 metric 把提供者默认路由装回同一 TUN  
- `manual_routes` 开启：整表由手动列表覆盖，**不**自动加出口默认路由  
- `enable_exit_node` **不**向全网通告 `/0`  
- 本机出口默认 `/0`（及配套 `::/0`）用低度量；对端宣告的 `/0` 与更具体 CIDR 仍用高度量  
- 升级后需重启网络实例（或 GUI 服务）才会按新度量重装已存在的默认路由  
- `add`/`remove` **仅成功时**（含「已存在 / 已不存在」）记入已安装集合；其它失败打 warn，路由同步用独立 1 秒 interval 对照期望集合重试，不被其它事件重置

### 平台差异（本机路由的实际安装方）

「本机 `exit_nodes` 才装有效默认」这条规则**只在桌面 TUN 路径成立**。各平台实际安装方不同：

| 平台 | 路由安装方 | 对端通告的 `/0` 会怎样 |
|------|-----------|----------------------|
| Windows / Linux / macOS（非 NE）/ FreeBSD | 本进程 `apply_route_changes` | 按高度量装（见上），无出口时通常不生效 |
| Android（GUI / Web 控制台） | `VpnService.Builder.addRoute`，由 GUI 把**所有 peer 的 `proxy_cidrs`** 加配置 routes 汇总后下发 | **直接成为 VpnService 默认路由**（Android 无路由度量概念），不经过 exit 门控 |
| Android（`easytier-android-jni`） | Kotlin 侧汇集所有 peer 的 `proxy_cidrs` 后交给 VpnService | 同上 |
| OHOS | `aggregate_tun_routes` 汇总所有 peer 的 `proxy_cidrs`，再经 `simplify_routes` 吸收同族前缀 | 任一对端宣告 `/0` 时，**其它更具体的 TUN 路由会被折叠进 `0.0.0.0/0`** |
| iOS / macOS NE | 由宿主 App / NE provider 的 `includedRoutes` 决定，本仓库不安装 | 本仓库无法约束 |

结论：**「对端 `/0` 在桌面几乎不生效、在移动端是全隧道」是同一配置在不同平台的实际语义差异**，不是 bug。排查移动端「整机流量都进了 VPN」时先看对端是否宣告了 `/0`。

补充：对端宣告的 **IPv6 proxy CIDR 目前不写入任何 OS 路由**（`::/0` 只在本机出口时装），因此 IPv4 与 IPv6 在 L2 上本就不对称。

---

## 4. 与 UI 文案

`exit_nodes` 的含义是：**本机把默认上网路由指向 TUN，进网后按出口列表转发**。出口机需开启 `enable_exit_node`。不要写成「出口机会向全网推送 0.0.0.0/0」。

---

## 5. 域名代理

**未实现。** 现状仅有手工 `proxy_cidrs` + MagicDNS 等基础设施。  
见 [`../roadmap/domain-proxy.md`](../roadmap/domain-proxy.md)。

---

## 6. 验收对照（现状）

| 场景 | 期望（今天） |
|------|----------------|
| 本机 `exit_nodes` 可解析且有下一跳，未开 `manual_routes` / `no_tun` | TUN 安装 `0.0.0.0/0` 与 `::/0`；公网 L3 走出口列表并带 `exit_node` 标志 |
| `exit_nodes` 非空但全部不可达 | **不**装本机默认路由；公网走系统原默认网关 |
| 清空 `exit_nodes` 或出口掉线 | 卸掉上述默认路由；对端子网 CIDR 保留 |
| 只配 `enable_exit_node`，任何节点都未配 `exit_nodes`、也未宣告 `/0` | 系统路由表通常 **不会** 出现 TUN 默认路由 |
| 对端通告 `10.0.0.0/24` | 本机可同步安装对应 TUN 路由；命中走 LPM，非 exit 标志 |
| 对端通告 `0.0.0.0/0`，本机 **未** 配出口 | 本机**可能**安装高度量默认路由：物理默认网关仍在时它赢不过；**物理默认缺失或度量劣于高度量时它会成为唯一默认路由**，从而真实接管本机流量（L3 走子网代理 `/0` 兜底） |
| 对端通告 `0.0.0.0/0`，本机 **已** 配可解析出口 | L3 走 `exit_nodes`，带 `exit_node` 标志 |
| `manual_routes` 开启 | L2 完全由手动列表决定，忽略动态 proxy CIDR 与出口默认路由 |
| 对端通告 `0.0.0.0/0`，本机**无物理默认路由**（或物理度量劣于高度量） | TUN 高度量 `/0` 成为生效默认路由；本机发起的公网流量经对端 `/0` 子网代理转发（**这是高度量 `/0` 唯一真实生效的场景**） |
| 移动端（Android / OHOS）同样的对端 `/0` | 由 `addRoute` / OHOS tun 路由直接生效，**与 exit 门控无关**，表现为整机流量进 VPN；iOS / macOS NE 由宿主 NE 配置决定 |
| 对端通告 `::/0` | 本机**不**装任何 IPv6 默认路由（`::/0` 只随本机出口安装） |
