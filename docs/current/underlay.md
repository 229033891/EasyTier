# Underlay：物理链路保活与黑洞防护

## Status

- Status: **Current**
- 最近审阅：2026-10-10
- 读者：排障（出口节点装上 `/0` 后断网、DNS 黑洞）、实现复查
- 索引：[`../README.md`](../README.md)
- 入口：[`system-overview.md`](./system-overview.md)
- 相邻：[`traffic-steering.md`](./traffic-steering.md)（L2 路由全貌）、[`socket-protection.md`](./socket-protection.md)（Host bypass 契约）

本文是 **underlay 主题的 Current SoT**。此前该概念散落在 9 个文件 / 4 个 docs 分区，
且 `Current` 两处措辞不一致，本文收敛之。

---

## 0. 先消歧：四种「underlay」不是一回事

这个词在本仓库被用于**四个互不相干的概念**。读代码或文档时先确认是哪一种：

| # | 含义 | 作用域 | 主要实现 | 本文 |
|---|------|--------|---------|------|
| 1 | **underlay 排除路由** | L2 · 桌面 exit-node | `virtual_nic.rs` | §1 |
| 2 | **underlay DNS 绑定** | L2 · 桌面域名解析 | `RuntimeDnsResolver` | §2 |
| 3 | **underlay 网络变化** | 连接保持 · Android | Kotlin `NetworkCallback` + watchdog | §3 |
| 4 | **underlay 出口多样性** | 选路 · 多网卡 | `bind_device` | §4（**未实现**） |

四者常被混谈。典型误判：把 §3 的 Android 日志 `underlay network generation changed`
当成 §1 的排除路由问题——两者毫无关系。

---

## 1. Underlay 排除路由（exit-node `/0` 的保命机制）

### 1.1 问题

本机配 `exit_nodes` 后，TUN 上会装 `0.0.0.0/0`，**所有**流量都被吸进隧道。
如果连隧道的那个 peer 地址、STUN 服务器地址、config-server 地址也在这条 `/0` 后面，
就会形成**黑洞**：隧道自己找不到回家的路。

### 1.2 做法

在**物理默认网关**上，为「隧道自己要用到的目的地」钉一批 `/32`（IPv6 为 `/128`）主机路由，
这些路由比 TUN 的 `/0` 更具体，因此优先匹配，流量绕过 TUN 直出。

**排除目标的三个来源**（`easytier-core/src/instance/packet_plane.rs:106` `underlay_exclude_ips()`）：

| 来源 | 说明 |
|------|------|
| 已连接 peer 隧道的 `resolved_remote_addr` | 隧道对端地址 |
| 对端 `stun_info.public_ip` | 打洞用的公网地址 |
| 本进程 config-server / 管理面连接目标 | `config_server_underlay_ips()`，仅 `web-client` feature 下非空 |

**过滤规则**（`underlay_exclude.rs:70` `should_exclude_ip`）：

| 检查 | 作用 |
|------|------|
| `is_excludable_underlay_ip` | 拒绝 unspecified / loopback / broadcast / multicast / link-local（IPv4 另拒 documentation）；**因此允许 RFC1918 与 ULA 私网地址** |
| `peer_manager.is_local_virtual_ip` | 拒绝 overlay VIP——本机虚拟地址必须继续走 TUN，不能被拉回物理 |
| `peer_manager.is_easytier_managed_ipv6` | 拒绝 EasyTier 托管的 IPv6（前缀租约等） |

### 1.3 门控：只在本机**真的会装** TUN `/0` 时才装

统一门控 `will_have_tun_default`（`easytier/src/instance/virtual_nic.rs:1199`）：

```text
will_have_tun_default = (已安装集合 ∪ 本次新增 − 本次删除) 中存在 network_length() == 0 的项
needs_excludes = will_have_tun_default
```

注意它算的是**变更后的期望状态**，不是「当前装没装」。因此以下三条路径都会触发排除：
本机 `exit_nodes`、逃生阀 `allow_peer_default_without_exit=true`、`manual_routes` 含 `/0`。

不装 TUN `/0` 时不装排除——避免给用户留下无来由的静态路由。

### 1.4 装卸顺序（顺序错了就是黑洞）

**安装**：先钉排除路由，**再**装 TUN `/0`。

```
reconcile_underlay_exclude_routes(...)   // 先
apply_route_changes(...)                 // 后
```

**卸载**：**先**卸 TUN `/0`，**再**卸排除路由。反过来会在中间窗口产生断连。

`virtual_nic.rs:1212-1237`（装）与 `:1252` / `:1286`（卸）都遵守这个次序。

### 1.5 停止时的清理

停止实例 / UI 禁用网络会显式清理，共 5 步（完整清单见
[`traffic-steering.md`](./traffic-steering.md) §3）。与 underlay 相关的是第 5 步：
按 `underlay_exclude_ips` + 当前物理默认网关，卸掉钉在**物理网卡**上的 `/32` `/128`。

实现：`easytier/src/instance/runtime_host/tun_common.rs:211` `cleanup_underlay_exclude_routes`。
它用**与 updater 完全相同的 desired 集合**和**同一条物理默认路由**推导出目标条目，
因此不会误删用户自己或其它实例的路由。

> **已知实现债**：route updater 任务的收尾要等事件总线 `Closed`，而任务自己持有
> `global_ctx`，实际等不到。所以停止路径必须自己清一遍，不能依赖 updater 收尾。
> 见 [`traffic-steering.md`](./traffic-steering.md) §3 与 `traffic-steering.md:70`。

### 1.6 物理默认网关会变

`reconcile_underlay_exclude_routes`（`virtual_nic.rs:1487`）**每轮重新发现**物理默认。
网关或 ifindex 变化时，先卸掉旧 via 上的主机路由，再换下一跳重装。
换网（Wi-Fi ↔ 有线）后不会留下指向旧网关的死条目。

### 1.7 可观测

- `NetworkInstanceRunningInfo.proxy_cidr_route_sync` 展示「期望 vs 已安装 vs 最后错误」。
- 注意该摘要**来源因平台而异**，排障前必须先分清，详见
  [`traffic-steering.md`](./traffic-steering.md) §3 的可观测小节。

---

## 2. Underlay DNS 绑定（防首次查询被 `/0` 吞掉）

排除路由只解决**已知 IP**。DNS 解析是另一个问题：TUN `/0` 装上后，
**第一次**域名查询本身还没有 IP 可排除，会被吸进隧道 → 隧道尚未建好 → 查询失败 → 黑洞。

**做法**：TUN 就绪后，peer / STUN / config-server 的域名解析经 `RuntimeDnsResolver`
（`easytier/src/common/dns.rs`），把 hickory 的 DNS UDP/TCP 套接字**绑定到物理默认网卡**
（`SO_BINDTODEVICE` / `IP_BOUND_IF` / `IP_UNICAST_IF`），并排除已注册的 TUN ifname。

系统 `lookup_host` **仅在**无 TUN 排除注册且无物理默认时使用。

契约层面见 [`socket-protection.md`](./socket-protection.md)：
DNS TCP fallback 的套接字同样必须受 bypass 保护，不得让系统 DNS 静默绕过该契约。

> 换网后依赖短 TTL 刷新物理 ifname——这是当前实现的已知取舍。

---

## 3. Underlay 网络变化（Android 连接保持）

**与 §1/§2 无关。** 这里是「设备换了底层网络」的事件，用于触发 peer 重连。

机制：Kotlin `registerDefaultNetworkCallback` + debounce + `setUnderlyingNetworks`
→ `underlayNetworkGeneration` 进 `get_vpn_status` → Rust watchdog `notify_underlay_network_changed`
关掉现有 peer conn → ManualConnector 1s 后重拨。JS 侧另有 `default_network_changed` 快路径。

**已知历史问题（A9）**：启动后 1～2 秒内 underlay generation 抖动会误触发重连，
表现为「刚连上就断」。已修，防护三层：

| 防护 | 位置 |
|------|------|
| 同 `networkHandle` 不 bump generation | `TauriVpnService.applyUnderlayNetwork` |
| 每次注册 callback / `clearStatus` 清零 generation 与 id | 同上 |
| 启动宽限期 5s（延长不重置已 seed 的 generation） | `easytier-gui/src-tauri/src/underlay_reconnect_grace.rs` |

**完整排障流程见** [`../ops/android-startup-auto-stop.md`](../ops/android-startup-auto-stop.md)，
本文只做概念消歧，不重复症状表。

---

## 4. Underlay 出口多样性（**未实现**）

多网卡场景下希望「按不同物理出口拨号并计入 bond 多样性」。当前：

- socket 层**已有** `bind_device` / 本地源 IP 能力；
- **缺少**按多网卡主动拨号、并把多样性计入控制器的那一层。

属于 Phase 3 范围，见 [`../roadmap/multi-link-bonding.md`](../roadmap/multi-link-bonding.md) §2。
当前实现允许在只有一条可用 underlay 时同质填满 bond 集，**不得因此判失败**。

---

## 5. 测试现状

| 范围 | 覆盖 |
|------|------|
| `underlay_exclude_ips` 纯逻辑 | `easytier-core/src/gateway/proxy/underlay_exclude.rs` 3 个 |
| 路由同步 / 装卸 | `easytier-core/src/gateway/proxy/cidr_monitor.rs` 13 个（`:387` 起）、`easytier/src/common/ifcfg/netlink.rs` 5 个 |
| 装卸顺序、黑洞 | **无端到端断言**——没有任何测试断言「写进 OS 的 `/0` 或其度量」 |
| Android underlay | Kotlin `vitest` 有 A9 用例 4 个；宽限期逻辑有桌面可跑单测 |

---

## 6. 常见误区

| 现象 | 别误判为 | 实际查 |
|------|---------|--------|
| exit-node 装上后完全断网 | 路由装失败 | 排除路由是否已钉（§1.2 三来源是否齐全）；物理默认是否找得到（§1.6） |
| 首次解析失败、重试就好 | DNS 坏了 | §2 的首次查询被 `/0` 吞掉，是预期行为已被防护覆盖 |
| Android 刚连上就断 | 排除路由问题 | §3 的 generation 误触发，看 `android-startup-auto-stop.md` |
| 停网后 `route print` 有残留条目 | 泄漏 | §1.5 的 5 步清理；wintun 适配器**持久**，不清不会自己消失 |

---

## 7. 已知缺口

1. **无端到端黑洞断言**：没有任何测试断言写进 OS 的 `/0`、其度量或装卸顺序。
2. **换网后的 DNS ifname 刷新依赖短 TTL**，不是事件驱动的立即刷新。
3. **underlay 出口多样性未实现**（§4）。
4. **保底 URL 的 host:port 尚不能加入排除列表**（端口自定义场景），
   见 [`../roadmap/connection-stability-todo.md`](../roadmap/connection-stability-todo.md) P3.3。
5. 默认路由 **Phase 2**（`/1` + `/1` 拆分）仍开放，
   见 [`../roadmap/default-route-and-underlay-excludes.md`](../roadmap/default-route-and-underlay-excludes.md)。

---

## 8. 代码锚点

| 主题 | 路径 |
|------|------|
| 排除集合产出 | `easytier-core/src/instance/packet_plane.rs:106` `underlay_exclude_ips` |
| 排除集合聚合 | `easytier-core/src/gateway/proxy/underlay_exclude.rs` |
| config-server 排除 | `easytier-core/src/management/full/config_server_status.rs` `underlay_exclude_candidate_ips` |
| 门控 + 装卸顺序 | `easytier/src/instance/virtual_nic.rs:1199` / `:1487` |
| 停止清理 | `easytier/src/instance/runtime_host/tun_common.rs:211` |
| 桌面 underlay DNS | `easytier/src/common/dns.rs` → `RuntimeDnsResolver` |
| Android underlay callback | `tauri-plugin-vpnservice/.../TauriVpnService.kt` |
| 启动宽限期 | `easytier-gui/src-tauri/src/underlay_reconnect_grace.rs` |
| 路由同步测试 | `easytier-core/src/gateway/proxy/cidr_monitor.rs` |
| 平台路由测试 | `easytier/src/common/ifcfg/netlink.rs` |

---

## 9. 维护

行为变更时：先改本文与对应 Current 专题，再改引用本文的 Roadmap / Ops / Archive。
方案论证全文留档在
[`../archive/default-route-and-underlay-excludes-2026-10.md`](../archive/default-route-and-underlay-excludes-2026-10.md)
（**不**当规划依据）。