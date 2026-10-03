# 流量导流（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-03
- 范围：`exit_nodes` / `enable_exit_node` / `proxy_cidrs` / `manual_routes` / TUN 系统路由同步
- 规划中的改动见：[`../roadmap/traffic-steering-vNext.md`](../roadmap/traffic-steering-vNext.md)、[`../roadmap/domain-proxy.md`](../roadmap/domain-proxy.md)

本文只描述 **代码今天做什么**。产品帮助文案应以本文为准。

---

## 1. 两层职责（不要混）

| 层 | 管什么 | 相关配置 |
|----|--------|----------|
| **L2 入口** | 包会不会进 TUN（OS 路由表） | 对端通告的 `proxy_cidrs`；本机 `exit_nodes` 安装的默认路由；`manual_routes` |
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
- 对端宣告的 `0.0.0.0/0`（非子网出口）仍用高度量，避免没配出口时抢物理默认网关  
- 出口不可达或清空 `exit_nodes`：从期望集合去掉本机默认路由，其它 CIDR 不动；卸 `::/0` 后若本机仍是 Public IPv6 提供者，按原 metric 把提供者默认路由装回同一 TUN  
- `manual_routes` 开启：整表由手动列表覆盖，**不**自动加出口默认路由  
- `enable_exit_node` **不**向全网通告 `/0`  
- 未指定 cost 时：前缀 `/0`（及 `::/0`）使用较低度量（Windows 约 1，Linux 约 50），其余 CIDR 仍用高度量（Windows 9000 / Linux 65535）  
- 升级后需重启网络实例（或 GUI 服务）才会按新度量重装已存在的默认路由  
- `add`/`remove` **仅成功时**（含「已存在 / 已不存在」）记入已安装集合；其它失败打 warn，路由同步用独立 1 秒 interval 对照**上次期望集合**重试，不被其它事件重置

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
| 对端通告 `0.0.0.0/0`，本机 **未** 配出口 | 本机可能安装高度量默认路由（通常赢不过物理网关）；L3 走子网代理 `/0` 兜底 |
| 对端通告 `0.0.0.0/0`，本机 **已** 配可解析出口 | L3 走 `exit_nodes`，带 `exit_node` 标志 |
| `manual_routes` 开启 | L2 完全由手动列表决定，忽略动态 proxy CIDR 与出口默认路由 |
