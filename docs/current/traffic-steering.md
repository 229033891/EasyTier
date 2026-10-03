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
| **L2 入口** | 包会不会进 TUN（OS 路由表） | 对端通告的 `proxy_cidrs`；`manual_routes` |
| **L3 选路** | 进 TUN 后交给哪个 peer | VIP、proxy CIDR LPM、`exit_nodes`、`enable_exit_node` |

**重要：`exit_nodes` 只参与 L3，不会安装本机默认路由 `0.0.0.0/0`。**

---

## 2. L3 Peer 选路（IPv4）

实现：`easytier-core` → `PeerManager::get_msg_dst_peer_ipv4`。

顺序固定：

1. 广播 / 网段广播 → 多 peer  
2. `get_peer_id_by_ipv4(dst)`  
   - 虚拟 IP 精确命中  
   - 若目标不在本机虚拟网网段：再走 **proxy CIDR 最长前缀匹配（LPM）**  
3. 仍未命中且目标不在本网段 → 按 **`exit_nodes` 列表顺序**，第一个 VIP 可解析的 peer，并设置 `is_exit_node=true`  
4. （部分宿主，如 OHOS）`local_exit_node_fallback` → 本机出口  

IPv6：VIP →（Public IPv6 gateway）→ proxy LPM → `exit_nodes`；链路本地地址不进出口。

因此：

- 更具体的子网代理 CIDR **永远优先于** `exit_nodes`  
- 若有 peer 通告 `0.0.0.0/0`，公网地址通常在步骤 2 命中 → **`exit_nodes` 被跳过**，且 **不带** `exit_node` 标志（走子网代理路径）

出口机侧：

- 带 `exit_node` 标志的包：需 `enable_exit_node`（或宿主强制）才由 TCP/UDP/ICMP 代理接  
- 仅靠对方通告的 CIDR（含 `0.0.0.0/0`）命中时：按子网代理 `lookup`，不依赖 `exit_node` 标志  

---

## 3. L2 系统路由

实现：`easytier` → `virtual_nic` 的 `run_proxy_cidrs_route_updater` / `apply_route_changes`。

- 期望集合来自对端 OSPF 通告的 proxy CIDR（排除本节点），或被 `manual_routes` **整表覆盖**  
- **与 `exit_nodes` 无联动**：配出口列表不会自动增加/删除 `0.0.0.0/0`  
- 已知缺口：`add`/`remove` 失败时仍可能更新内存「已同步」集合，导致装不上/删不掉且不重试（见 roadmap）

---

## 4. 与 UI 文案的差异（已知）

当前部分 locale（如 `exit_nodes_help`）写「本机所有上网流量都会转发」。  
按本文：**只有已经进入 TUN 的流量**才会按 L3 走出口；OS 默认路由不会因 `exit_nodes` 自动安装。  
在 roadmap Phase A 落地前，帮助文案宜理解为「选路意图」，而非「已接管系统默认路由」。

---

## 5. 域名代理

**未实现。** 现状仅有手工 `proxy_cidrs` + MagicDNS 等基础设施。  
见 [`../roadmap/domain-proxy.md`](../roadmap/domain-proxy.md)。

---

## 6. 验收对照（现状）

| 场景 | 期望（今天） |
|------|----------------|
| 只配 `exit_nodes`，不配任何 `0.0.0.0/0` | 系统路由表通常 **不会** 出现 TUN 默认路由；公网流量未必进 EasyTier |
| 对端通告 `10.0.0.0/24` | 本机可同步安装对应 TUN 路由；命中走 LPM，非 exit 标志 |
| 对端通告 `0.0.0.0/0` | 本机可能安装默认路由；公网走子网代理路径，exit 列表常被跳过 |
| `manual_routes` 开启 | L2 完全由手动列表决定，忽略动态 proxy CIDR |
