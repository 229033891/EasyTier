# 流量导流（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-14（`[[policy_routes]]` P1–P5：`priority` 恢复立即切回；切换带仅 `quality`；FIFO 不抹显式 `/32`；对账循环启动即跑一轮）
- 范围：`exit_nodes` / `enable_exit_node` / `proxy_cidrs` / `manual_routes` / `manual_routes_append` / `[[policy_routes]]` / TUN 系统路由同步 / `allow_peer_default_without_exit`
- 规划中的改动见：[`../roadmap/traffic-steering-vNext.md`](../roadmap/traffic-steering-vNext.md)、[`../roadmap/domain-proxy.md`](../roadmap/domain-proxy.md)；默认路由 Phase 2：[`../roadmap/default-route-and-underlay-excludes.md`](../roadmap/default-route-and-underlay-excludes.md)（论证全文 [`../archive/default-route-and-underlay-excludes-2026-10.md`](../archive/default-route-and-underlay-excludes-2026-10.md)）
- DNS：[`magic-dns.md`](./magic-dns.md)
- **underlay / 黑洞防护**：[`underlay.md`](./underlay.md)（主题 SoT）

本文只描述 **代码今天做什么**。产品帮助文案应以本文为准。

---

## 1. 两层职责（不要混）

| 层 | 管什么 | 相关配置 |
|----|--------|----------|
| **L2 入口** | 包会不会进 TUN（OS 路由表） | 对端通告的 `proxy_cidrs`（**IPv4**；见 §3 平台说明，IPv6 proxy CIDR 不装）；本机 `exit_nodes` 安装的默认路由；`manual_routes`；本机 `[[policy_routes]]` 中 `Stable`/`Failover` 规则的 CIDR（L2 与 L3 **同源**） |
| **L3 选路** | 进 TUN 后交给哪个 peer | VIP、proxy CIDR LPM（不含 `/0`）、**`[[policy_routes]]`（次序 3.5）**、`exit_nodes`、`enable_exit_node` |

出口机 **不会** 因为 `enable_exit_node` 向 OSPF 通告 `0.0.0.0/0`。默认路由由**选用出口的客户端本机**安装。

---

## 2. L3 Peer 选路（IPv4）

实现：`easytier-core` → `PeerOutboundPacketRouter::get_msg_dst_peer_ipv4`。

顺序固定：

1. 广播 / 网段广播 → 多 peer  
2. VIP 精确命中  
3. 若目标不在本机虚拟网网段：proxy CIDR **最长前缀匹配，忽略 `0.0.0.0/0`**（更具体子网代理）  
3.5. 目标不在本网段且以上均未命中 → **本机 `[[policy_routes]]`**（`ArcSwap` 快照只读，无锁、无 await、无分配）：  
   - `Hit` → 交给该规则选出的 peer，并**始终置 `exit_node` 标志**（规则语义 = 「B 做网关」，见 §5）  
   - `Block`（`if_all_unavailable = closed` 且全部候选不可达）→ **直接丢包**，**不**回退 `exit_nodes` / 对端 `/0` / 本机出口  
   - `Miss`（无规则命中，或 `open` 规则降级）→ 继续 ↓  
4. 仍未命中且目标不在本网段 → 按 **`exit_nodes` 列表顺序**，第一个 VIP 可解析 **且有下一跳** 的 peer，并设置 `is_exit_node=true`  
5. 出口列表为空或无法解析/无下一跳时：才使用对端通告的 `0.0.0.0/0`（子网代理兜底，**不**打 `exit_node` 标志）  
6. （部分宿主，如 OHOS）`local_exit_node_fallback` → 本机出口  

IPv6：VIP / 非 `/0` 的 proxy LPM → `exit_nodes`（同样要求下一跳）→ Public IPv6 gateway → 对端 `::/0` 兜底；链路本地地址不进出口。

因此：

- 更具体的子网代理 CIDR **永远优先于** `exit_nodes`  
- 配置了可解析的 `exit_nodes` 时，公网走出口路径并带 `exit_node` 标志，不会被对端 `/0` 抢走  
- **`[[policy_routes]]` 命中优先于 `exit_nodes`**（次序 3.5 在 4 之前）；只对「规则显式覆盖」的目标生效，其余仍走原逻辑

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
- **停止实例 / UI 禁用网络时显式清理**（先 `nic.stop()` 停掉 route updater / Magic DNS，再调 `runtime_host/tun_common.rs::cleanup_tun_leftovers`，避免清理窗口内被重新装回）：  
  1. 按 `proxy_cidr_route_sync` 的**已安装**集合删本实例装过的 proxy CIDR / `/0`（只删自己装的条目）  
  2. 按 `packet_plane.public_ipv6_routes()` 删 public-ipv6 **client** 装上的 peer lease 路由（多为 `/128`；Windows 亦可作 client）  
  3. best-effort 删 TUN 接口上的 `::/0`（exit 与 public-ipv6 本机默认各一条，该集合不在同步状态里）  
  4. Windows：刷掉 TUN 适配器上的 IPv4 / IPv6 地址——**wintun 适配器是持久设备**，会话结束后接口、IP、路由都不会自动消失，不清就会在 `route print` 里留下指向已失效接口的条目和 on-link 子网路由  
  5. 按 `underlay_exclude_ips` + 当前物理默认网关卸掉第 3 节钉在**物理网卡**上的 `/32` `/128` 排除路由  
  注：updater 任务收尾那段清理要等事件总线 `Closed`，而任务自己持有 `global_ctx`，实际等不到，所以停止路径必须自己清一遍；`get_tun_device_name` 在 `nic.stop()` 后仍有效（仅错误路径会清空）
- **Windows 自动网卡名必须写回配置**，否则每次启用 / 每次重启都会新建一张 `et_*` 网卡（wintun 适配器持久，旧的不会自己消失）：
  - 配置 `dev_name` 为空时，`virtual_nic.rs::create_tun` 生成 `et_<全机网卡数>_<4 位随机>`；该名字要同时写进 **`GlobalCtx` 运行时 flags**（本实例后续流程用）与**共享 `TomlConfig`**——`GlobalCtx::flags` 只是副本，管理面 `get_network_instance_config` / `collect_network_info` 读的是 `instance.toml_config()`，不写回就永远读回空值
  - GUI 侧 `easytier-gui/.../manager.rs::persist_runtime_dev_name`：实例启动后读回名字落盘。`run_network_instance` **不等** `start()` 完成，所以 `pre_run_network_instance_hook` 存下的仍是空 `dev_name`；该函数只在「存储里 `dev_name` 为空且非 web 归属」时才轮询，成功一次后后续运行直接跳过
  - web 归属配置不动：console 侧 reconciler 用 `is_automatic_windows_dev_name` 主动忽略自动名，写回会把自动名当成用户配置  
- ACL / KCP / QUIC / wrapped-TCP 等旁路通过 `get_peer_id_by_ip_allowing_default_proxy` 仍可解析对端通告的 `/0`  
- **`[[policy_routes]]` 的 L2 贡献（P2 起）**：期望集合额外并入本机路由表中 `Stable` / `Failover` 规则的 CIDR，取自 **L3 热路径同一份 `ArcSwap` 快照**（L2 与 L3 同源，避免「装了路由但 L3 查不到」的黑洞）。**`Degraded` 规则不贡献 L2**。含显式 `/0` 规则时等价于「本节点声明全隧道」，与 `exit_nodes` 一样触发本机默认路由安装（仍受 `no_tun` 抑制）
- `manual_routes_append = true` 时 `manual_routes` 与上述集合**叠加**（走完整累加 + `/0` 策略）；默认 `false` = 旧覆盖语义
- `manual_routes` 开启：整表由手动列表覆盖，**不**自动加出口默认路由  
- `enable_exit_node` **不**向全网通告 `/0`  
- 可观测：`NetworkInstanceRunningInfo.proxy_cidr_route_sync` 展示「期望 vs 已安装 vs 最后错误」摘要  
  - **状态页展示规则**（2026-10-08 起）：字段**有值就展示**，空占位（`desired=[-] installed=[-] exit=…`）也照显示——空摘要不是「没数据」，而是「确实一条代理路由都没装」，便于一眼确认。字段缺失（老核心）或全空白不显示。
  - **摘要来源因平台而异**，排障时必须先分清：桌面 = L2 路由同步（`easytier/src/instance/virtual_nic.rs::report_proxy_cidr_route_sync`）；Android = GUI `easytier-gui/src/composables/mobile_vpn.ts::annotateNetworkInfoWithMobileVpnRoutes` 按 VpnService 实装路由**覆盖写入**（那边 L2 ifcfg 是 no-op，空占位等价于「VPN 没装任何路由」）；OHOS = `easytier-contrib/easytier-ohrs/src/exports/runtime_api.rs::annotate_ohos_proxy_cidr_route_sync` 覆盖（`nearby_management.rs` 调用）。
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

### 权限前提（UI 与文档都必须写）

安装 TUN 默认路由 `/0` 需要**管理员 / root 权限**：

| 平台 | 提权方式 |
|------|---------|
| Windows | 进程以管理员运行（GUI 提权链路见 `easytier-gui/src-tauri/src/elevate`） |
| Linux | `root`，或 `CAP_NET_ADMIN` |
| macOS | `root`（Network Extension 由宿主 App 授予） |
| Android | 由 `VpnService` 框架授予，无需应用自身提权 |
| OHOS / iOS | 由宿主框架授予 |

权限不足时的表现是**安装失败并保留原路由**，不是静默降级为部分路由。
用户报「出口节点配了但没生效」时，先确认权限，再查路由表。

---

## 4. 与 UI 文案

`exit_nodes` 的含义是：**本机把默认上网路由指向 TUN，进网后按出口列表转发**。出口机需开启 `enable_exit_node`。不要写成「出口机会向全网推送 0.0.0.0/0」。

对端通告的 `/0`：**不**自动变成「本机默认路由」；需要全隧道请配 `exit_nodes`，或显式 `allow_peer_default_without_exit` / `manual_routes`。

---

## 5. 域名代理 / 域名路由（`[[policy_routes]]`）

**已实现（统一路由模型 P1–P5；尽力语义，非零泄漏合规）。** 把「子网代理」「出口节点」「域名路由」收敛为一个模型：**A 上配置路由，B 做网关**。设计全文见 [`../roadmap/unified-routing-design.md`](../roadmap/unified-routing-design.md)。

配置形状（TOML）：

```toml
[[policy_routes]]
cidr      = "10.0.0.0/24"        # 与 domains 至少填一个，可同时填
domains   = ["*.atlassian.net"]  # 通配父 zone 须 ≥2 标签；`*.com` 之类被拒
peers     = ["10.144.0.9"]       # 候选网关，按序
select_by         = "priority"   # priority（按序）| quality（按链路质量）| balance（按流哈希分摊）
failover_when     = "unreachable" # unreachable | degraded | never
if_all_unavailable = "open"       # open = 回退物理网关；closed = 丢包（多 peer 时需 require_confirmed）
# quality_degraded_cost = 500     # 仅 failover_when = "degraded" 时必填：复合成本超此值视为劣化
```

实现要点：

- **L3**：`get_msg_dst_peer_ipv4` 在 VIP / 子网代理之后、`exit_nodes` 之前插入**次序 3.5**（§2），命中即交给该规则选出的 peer。热路径只读快照，无锁 / 无 await / 无分配。
- **`select_by` 三种模式**（对账循环约每秒重算；**启动时先跑一轮再 sleep**，避免首秒 `Degraded` 窗口）：
  - `priority` —— 按 `peers` 列表顺序，取第一个可达的；优选恢复后**立即切回**（不用切换带）。
  - `quality` —— 按**复合链路成本**（RTT + loss + jitter 同族编码）取最低的。成本是**复合分**，不能按单一维度解读。
  - `balance` —— 把候选列表交给热路径，按**内层流 5-tuple 哈希取模**分摊：同一条 TCP 流恒定走同一个 peer，不同流可能走不同 peer → **出口 IP 不唯一**。
- **规则 = 出口（重要）**：`[[policy_routes]]` 命中后**一律置 `exit_node` 标志**，即被指向的 peer **必须开启**「允许此节点成为出口节点」（`enable_exit_node`），否则该 peer 会**丢弃**这些包（不会静默回退到其它路径）。网关侧仍**优先查自己的 `proxy_cidrs`**：命中即按子网代理转发，没命中才 NAT 出网。
- **切换防抖（仅 `quality`）**：发布侧沿用 `MIN_DELTA=20`；`select_by = quality` 时，相对 10% / 绝对 5 cost 单位带内不动，带外还需**连续 3 个 tick** 才切。`priority` **不**走切换带（否则成本恒 0，优选永远切不回来）。
- **L2**：`cidr_monitor` 从**同一份快照**取 `Stable` / `Failover` 规则的 CIDR 作为本机代理路由（§3）。
- **域名 → IP**：订阅 MagicDNS `resolve()` 的**实际应答**（全代码库唯一能看到应用实际解析结果的位置），学到 `/32` 后**原子**更新 L2 + L3；旧 IP 保留，超上限 FIFO 淘汰（**不**淘汰规则上显式配置的 `/32`）。
- **配置校验硬失败**：缺 `select_by` / `failover_when` / `if_all_unavailable`、`closed` + 多 peer 未 `require_confirmed`、`quality_degraded_cost` 与 `failover_when = degraded` 未成对、`domains` 落在 `et.net.` 内或与 Split 父 zone 冲突 —— 均**加载失败并报错**，不静默取默认。

### 5.1 ⚠️ 泄漏面（产品必须明示）

| 限制 | 说明 |
|---|---|
| **DNS 查询本身不走指定出口** | MagicDNS 上游强制绑**物理网卡**。域名路由只保证「连接」走指定出口，**不保证「解析」**。 |
| **首包最长 1 秒直连窗口** | 域名 IP 靠观测学习：L3 同步就绪，但 L2 的 OS 路由受 **1 秒对账循环**约束 → 首次访问的连接包有**最长 1 秒**从物理网关直连出去。 |
| **切换不迁移已建立连接** | 目标节点掉线时新流切换 ≤2 秒；已建立的 TCP 流需重连。 |
| **`balance` 下出口 IP 不唯一** | P5 语义固有。 |
| **`quality` 阈值是复合分** | P4，不能按 RTT / loss 维度分解。 |

> **本设计不保证零泄漏。** 缩小首包窗口可配合 `DnsConfig.hosts` 静态绑定 IP 使 DNS 与路由时序对齐；真正零泄漏需另立项。

### 5.2 平台矩阵（域名路由可用性）

| 平台 | 域名路由 | 原因 |
|------|----------|------|
| Windows / macOS（非 NE）/ Linux（systemd）/ OpenWrt（手工） | ✅ | 本进程可控 TUN 路由 + MagicDNS 自动接线 |
| Android（Tauri / JNI） | ✅ | VpnService 装路由；MagicDNS 走 `fake_ip`（注意 JNI 关 DNS 时会回落运营商 DNS） |
| OHOS | ✅ | 同 Android 模式（宿主框架授予） |
| **iOS / macOS NE** | ❌ | OS 不走 EasyTier DNS，拿不到解析结果 |
| **`no_tun`** | ❌ | MagicDNS Runner 不启动，无观测来源 |

（DNS 接线矩阵的完整版见 [`magic-dns.md`](./magic-dns.md) §7。）

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
| `[[policy_routes]]` 配 `cidr = "10.0.0.0/24"`，`peers` 可达 | L3 命中次序 3.5 走该 peer；L2 同步装 `10.0.0.0/24` |
| `[[policy_routes]]` 配 `domains`，应用解析出 A 记录 | 观测学到 `/32`；L3 立即就绪，L2 在 ≤1 秒内装上 |
| `[[policy_routes]]` `if_all_unavailable = closed` 且全部候选不可达 | 命中目标**丢包**，**不**回退物理网关 |
| `[[policy_routes]]` `domains` 落在 `et.net.` 内，或通配父 zone 与 Split forwarder 冲突 | **加载失败并报错**（配置预检） |
| `[[policy_routes]]` `select_by = "priority"`，优选 A 掉线再恢复 | 先切备选；A 恢复后**下一对账 tick 立即切回 A**（不走切换带） |
| `[[policy_routes]]` `select_by = "quality"`，两候选成本不同 | 选**成本最低**的可达候选；成本差在带内、或未连续 3 tick 占优时不切 |
| `[[policy_routes]]` `select_by = "balance"`，两条不同 TCP 流 | 各按自身流哈希选候选，**可能走不同 peer**；同一条流恒定走同一 peer |
| `[[policy_routes]]` `failover_when = "degraded"` + `quality_degraded_cost = 500` | 候选复合成本 > 500 视为劣化 → 切下一个；全超阈值 → `open` 回退 / `closed` 丢包 |
| `[[policy_routes]]` `quality_degraded_cost` 与 `failover_when = degraded` 未成对 | **加载失败并报错** |
| `[[policy_routes]]` 命中，但目标 peer 未开 `enable_exit_node` | 该 peer **丢弃**这些包（无静默回退）—— 排障先查此项 |
| `[[policy_routes]]` 的 `cidr` 同时被某 peer 通告为子网代理 | 次序 3 先命中 → **规则不执行**（子网代理优先，§2） |
