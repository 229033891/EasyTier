# 流量导流（现状）

## Status

- Status: **Current**（⚠️ **待验收**：下文 §2 / §5 中标注「待验收」的段落描述的是工作区未合入修复，合入并验收前勿当可依赖现状）
- 最近审阅：2026-10-20（策略路由六项加固：§5 的 L1 确认护栏 / L2 代价来源 / L3 `never` 真钉死 / L5 无路由表拒绝；复核修补：R4 每规则代价归因 / R5 热补丁按实例自身路由算法校验；新增 **§5.0.1 热生效边界**、**§5.3 升级影响**；§2·§5 待验收标注）；2026-10-18（Android GUI / OHOS 与 JNI 对齐：新 core 吃 `desired_proxy_cidrs`，旧 core 回退本地拼装）；2026-10-16（修正 `manual_routes` 语义：仅覆盖**学习集合**，append 模式仍自动装 exit `/0`、policy 显式 `/0` 两种模式均生效；Android JNI 逃生阀改由 `NetworkInstanceRunningInfo` 下发；补对端 `/0` 兜底需 `enable_exit_node`）
- 范围：`exit_nodes` / `enable_exit_node` / `proxy_cidrs` / `manual_routes` / `manual_routes_append` / `[[policy_routes]]` / TUN 系统路由同步 / `allow_peer_default_without_exit`
- 规划中的改动见：[`../roadmap/traffic-steering-vNext.md`](../roadmap/traffic-steering-vNext.md)、[`../roadmap/domain-proxy.md`](../roadmap/domain-proxy.md)；默认路由 Phase 2：[`../roadmap/default-route-and-underlay-excludes.md`](../roadmap/default-route-and-underlay-excludes.md)（论证全文 [`../archive/default-route-and-underlay-excludes-2026-10.md`](../archive/default-route-and-underlay-excludes-2026-10.md)）
- DNS：[`magic-dns.md`](./magic-dns.md)
- **underlay / 黑洞防护**：[`underlay.md`](./underlay.md)（主题 SoT）

本文只描述 **代码今天做什么**。产品帮助文案应以本文为准。

---

## 1. 两层职责（不要混）

| 层 | 管什么 | 相关配置 |
|----|--------|----------|
| **L2 入口** | 包会不会进 TUN（OS 路由表） | 对端通告的 `proxy_cidrs`（**IPv4**；见 §3 平台说明，IPv6 proxy CIDR 不装）；本机 `exit_nodes` 安装的默认路由（须无 override 模式 `manual_routes`）；`manual_routes`；本机 `[[policy_routes]]` 中 `Stable`/`Failover` 规则的 CIDR（桌面与 L3 **同源**；Android/OHOS **新 core** 经 `desired_proxy_cidrs` 同源安装，见 §3 / §5.2） |
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
5. 出口列表为空或无法解析/无下一跳时：才使用对端通告的 `0.0.0.0/0`（子网代理兜底，**不**打 `exit_node` 标志）。⚠️ 兜底路径**同样要求对端开了 `enable_exit_node`**：不带标志的包只有当对端 `proxy_cidrs` 精确命中时才做子网代理转发，否则按 `Pass` 交回普通转发、不会 NAT 出网（`udp_proxy_engine.rs:198` / `:226-228`、`tcp_proxy_engine.rs:552-557`）。所以「兜底选路」不等于「能出网」  
6. （部分宿主，如 OHOS）`local_exit_node_fallback` → 本机出口  

IPv6：VIP / 非 `/0` 的 proxy LPM → `exit_nodes`（同样要求下一跳）→ Public IPv6 gateway → 对端 `::/0` 兜底；链路本地地址不进出口。

因此：

- 更具体的子网代理 CIDR **永远优先于** `exit_nodes`  
- 配置了可解析的 `exit_nodes` 时，公网走出口路径并带 `exit_node` 标志，不会被对端 `/0` 抢走  
- **`[[policy_routes]]` 命中优先于 `exit_nodes`**（次序 3.5 在 4 之前）；只对「规则显式覆盖」的目标生效，其余仍走原逻辑
- ⚠️ **待验收**：在 `[[policy_routes]]` 表内部，`exit_nodes` 与**默认路由**规则**合并成同一条规则**（候选顺序 = 显式在前、`exit_nodes` 在后），不是二选一。之前它们是两条 `/0` 规则，而 LPM 索引每个 `(前缀, 网段)` 只保留一条 ⇒ **`exit_nodes` 被整条丢弃**（配了显式 `/0` 规则时 `exit_nodes` 完全不生效）。
  - 合并在**对账期**完成（每轮用**活**的 `exit_nodes` 生成候选）⇒ **热补丁 ≤1s 生效**（`config_patch` → `update_exit_nodes` 不必重建表）。**显式 `/0` 与 sugar `/0` 两条路径都如此**。
  - 合并后候选数超过 16、合并导致 `closed`+多 peer 未 `require_confirmed`、显式 `/0` 规则同时带 `domains`、或显式 `/0` 规则用 `failover_when = "never"`，均**加载失败并报错**（不静默截断、不静默失效）。见 [`routing-bug-audit-2026-10.md`](./routing-bug-audit-2026-10.md) B7

出口机侧：

- 带 `exit_node` 标志的包：需 `enable_exit_node`（或宿主强制）才由 TCP/UDP/ICMP 代理接  
- 仅靠对方通告的非默认 CIDR 命中时：按子网代理 `lookup`，不依赖 `exit_node` 标志  

---

## 3. L2 系统路由（D+）

实现：`easytier-core` → `resolve_proxy_cidrs`；`easytier` → `virtual_nic` 的 `apply_route_changes`。

- 期望集合 = 对端 OSPF 通告的 proxy CIDR（排除本节点）  
- **对端通告的 `0.0.0.0/0` 默认不进入期望集合**（不写入 OS）。L3 仍可作选路兜底。  
- **本机 `exit_nodes` 中至少有一个 VIP 可解析且有下一跳**、**未以 override 模式配置 `manual_routes`**、未 `no_tun`：加入本机管理的 `0.0.0.0/0`（低度量）；IPv4 `/0` **成功装上之后**再装 `::/0`（按 TUN 接口）。实现门闩在 `collect_proxy_cidr_state`：`manual_routes_append = false` 且配了 `manual_routes` 时不置 `has_exit_nodes`；**`manual_routes_append = true` 时照常置位**（append 语义 = 「手动列表是补充，不是对自节点默认路由意图的否决」）  
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
- **`[[policy_routes]]` 的 L2 贡献（P2 起）**：`cidr_monitor` → `resolve_proxy_cidrs` 期望集合额外并入本机路由表中 `Stable` / `Failover` 规则的 CIDR，取自 **L3 热路径同一份 `ArcSwap` 快照**（L2 与 L3 同源，避免「装了路由但 L3 查不到」的黑洞）。**`Degraded` 规则不贡献 L2**。含显式 `/0` 规则时等价于「本节点声明全隧道」，可触发本机默认路由安装（仍受 `no_tun` 抑制）。桌面直接装该期望集；**Android / OHOS 新 core** 经 `desired_proxy_cidrs` 下发后由宿主原样安装（旧 core 仍本地拼装，见 §3 / §5.2）
- `manual_routes` 只覆盖**学习集合**（对端 proxy CIDR — 含对端 `/0` — 与 policy `local_routes` 中的**非 `/0`** 条目）；**不覆盖本机自己的默认路由意图**：
  - **用户手写在 `manual_routes` 里的 `/0`**：两种模式下都保留（与 policy 显式 `/0` 同为「本机声明全隧道」；`no_tun` 除外）
  - `manual_routes_append = true`：手动列表与「对端 proxy CIDR + policy `local_routes`」**叠加**，且 `exit_nodes` 仍自动装 `/0`（低度量）
  - `manual_routes_append = false`（默认）：学习集合整表只剩手动列表，`exit_nodes` **不**自动装 `/0`；但 policy 规则里**显式**的 `cidr = "0.0.0.0/0"` **仍生效**
  - 逃生阀 `allow_peer_default_without_exit` 保留的是**对端通告**的 `/0`，属学习集合 → override 模式下仍被手动列表覆盖（append 模式下正常生效）
- `enable_exit_node` **不**向全网通告 `/0`  
- **宿主侧路由聚合所需的策略字段**：`NetworkInstanceRunningInfo.allow_peer_default_without_exit`（`optional bool`）与 `exit_nodes`（`repeated string`）由核心下发，供 Android JNI 等**自己装路由、读不到 core config** 的宿主使用。用 `optional` 而非裸 bool 是因为 proto3 标量无 presence，漏发与 `false` 不可区分（老核心不填时宿主保留上一次的值）。桌面路径不读这两个字段，只看 `resolve_proxy_cidrs`。另有 `desired_proxy_cidrs`（`repeated string` #16）：核心按需计算的 L2 期望集合（含 manual_routes / policy CIDR / 观测 `/32`），宿主原样安装、不再复刻语义；老核心不发，按 `allow` 存在位回退旧拼装。
- 可观测：`NetworkInstanceRunningInfo.proxy_cidr_route_sync` 展示「期望 vs 已安装 vs 最后错误」摘要  
  - **状态页展示规则**（2026-10-08 起）：字段**有值就展示**，空占位（`desired=[-] installed=[-] exit=…`）也照显示——空摘要不是「没数据」，而是「确实一条代理路由都没装」，便于一眼确认。字段缺失（老核心）或全空白不显示。
  - **摘要来源因平台而异**，排障时必须先分清：桌面 = L2 路由同步（`easytier/src/instance/virtual_nic.rs::report_proxy_cidr_route_sync`）；Android = GUI `easytier-gui/src/composables/mobile_vpn.ts::annotateNetworkInfoWithMobileVpnRoutes` 按 VpnService 实装路由**覆盖写入**（那边 L2 ifcfg 是 no-op，空占位等价于「VPN 没装任何路由」）；OHOS = `easytier-contrib/easytier-ohrs/src/exports/runtime_api.rs::annotate_ohos_proxy_cidr_route_sync` 覆盖（`nearby_management.rs` 调用）。
- `add`/`remove` **仅成功时**记入已安装集合；失败 warn，1 秒 interval 对照期望集合重试  

### 平台差异

| 平台 | 路由安装方 | 对端通告的 `/0` 会怎样 | `[[policy_routes]]` L2（CIDR / 域名 `/32`） |
|------|-----------|----------------------|---------------------------------------------|
| Windows / Linux / macOS（非 NE）/ FreeBSD | 本进程 `apply_route_changes`（`resolve_proxy_cidrs`） | **默认不装**；仅 `allow_peer_default_without_exit` 或本机 exit（且 `manual_routes` 非 override 模式）时进入期望集合 | ✅ 与 L3 同源快照 |
| Android（GUI / Web） | `VpnService.Builder.addRoute`，由 GUI `getRoutesForVpn` 汇总 | 新 core：期望集合已含过滤；老 core：无**可达** exit 且未开逃生阀时**过滤**对端 `/0`，可达 exit 时显式加 `/0` | ✅ 新 core（`allow_peer_default_without_exit` presence → 装 `desired_proxy_cidrs` + MagicDNS `/32`）；老 core ❌ 本地拼装 |
| Android（`easytier-android-jni`） | 新 core 下发的 `desired_proxy_cidrs`（#16）原样安装；老 core 回退为 Kotlin 拼装 | 新 core：期望集合已做过滤（含 exit `/0` / 逃生阀 `/0` / 手写路由 / policy CIDR），每 3 秒刷新；老 core 同上行 | ✅ 新 core（含 policy CIDR / 观测 `/32`）；老 core ❌ |
| OHOS | `aggregate_tun_routes` | 新 core：期望集合已含过滤；老 core：无可达 exit 且未开逃生阀时过滤对端 `/0`，可达时插入 `/0`（再 `simplify`） | ✅ 新 core（`core_route_policy_present` → `desired_proxy_cidrs` + MagicDNS `/32`）；老 core ❌ 虚拟网段 + `manual_routes` + peer `proxy_cidrs` + exit `/0` |
| iOS / macOS NE | 宿主 App / NE `includedRoutes` | 本仓库无法约束 | ❌ 本仓库无法约束 |

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
select_by         = "priority"   # priority（按序）| quality（按整条路径的代价，含中转段）| balance（按流哈希等概率分摊）
failover_when     = "unreachable" # unreachable | degraded | never
if_all_unavailable = "open"       # open = 回退物理网关；closed = 丢包（多 peer 时需 require_confirmed）
# quality_degraded_cost = 500     # 仅 failover_when = "degraded" 时必填：复合成本超此值视为劣化
```

实现要点：

- **L3**：`get_msg_dst_peer_ipv4` 在 VIP / 子网代理之后、`exit_nodes` 之前插入**次序 3.5**（§2），命中即交给该规则选出的 peer。热路径只读快照，无锁 / 无 await / 无分配。
- **`select_by` 三种模式**（对账循环约每秒重算；**启动时先跑一轮再 sleep**，避免首秒 `Degraded` 窗口）：
  - `priority` —— 按 `peers` 列表顺序，取第一个可达的；优选恢复后**立即切回**（不用切换带）。
  - `quality` —— **待验收**：按**到该候选的整条路径的复合代价**（RTT + loss + jitter 同族编码，各边代价累加，**中转段计入**）取最低的。代价是**复合分**，不能按单一维度解读。路径代价取自 OSPF least-cost 表的 `path_latency`（`Route::get_path_cost_with_policy`），所以**只靠中转可达的候选也能被正确比较**；直连边代价仅作为路由表尚未收敛时的兜底。**代价来源（L2）**：两种代价**单位相同但来源不同** —— `path` 是各跳**公布**边权的累加（含中转惩罚），`edge` 是**本地首跳直连实测**。两者**不做归一化**（同公式编码，属设计取舍）；来源会随代价一起发布到状态面「代价（来源）」列 / `easytier-cli route policy` 的 `costs=[id:cost(source)]`，供排障时判断阈值比较的到底是哪一个。**只列本规则真的据以排序/判定的候选**：只有 `quality` 或 `failover_when = degraded` 的规则才会读出代价，`priority` / `balance` + `unreachable` 的规则代价列恒为空 —— 即使它的某个候选恰好也被同表另一条 `quality` 规则测过，也不会"继承"显示那个数字（否则显示内容会随规则先后顺序变化）。**来源切换时滞回基线会被重置**——否则路由表短暂遗忘再记起该 peer 时，`path`/`edge` 会互相当成基线，把发布代价钉在一个没人量过的数值上。⚠️ **`route_algo = none` 时 `quality` / `degraded` 会加载失败**（没有路由表就没有代价可算，与其静默把所有候选判成不可测，不如报错，并提示改用 `priority` + `unreachable`）。注意该项**只对嵌入方有意义**：`route_algo` 不是 TOML 字段，常规配置无法表达 `none`，所以产品路径上不会触发。
  - `balance` —— 把候选列表交给热路径，按**内层流 5-tuple 哈希取模**分摊：同一条 TCP 流恒定走同一个 peer，不同流可能走不同 peer → **出口 IP 不唯一**。⚠️ 分摊是**等概率**的，**不按链路质量或带宽能力加权** ⇒ 一条 10ms 候选和一条 500ms 候选仍各承担约一半流量。⚠️ **待验收（L1）**：默认路由规则若合并了 `exit_nodes`，分摊池用的是**合并后的生效候选**（显式 `peers` 在前、`exit_nodes` 在后），不是配置里单独写的 `peers` 列 —— 出口节点会一起参与 `flow_key % N`。状态面 / `easytier-cli route policy` 展示「生效候选」可核对。
- **`if_all_unavailable = "closed"` 的确认按「生效候选」判定（所有 `select_by` 一视同仁）**：只要**合并后（去重）**的候选多于一个，就必须显式 `require_confirmed = true`，否则**加载失败**。这条**不分** `priority` / `quality` / `balance`：
  - `balance` 下每个出口都是**独立黑洞**（该出口掉线时，哈希到它的那条流直接丢包）；
  - `priority` / `quality` 下出口是**主备链的备用**，但“1 个 `peers` + 1 个 `exit_nodes`”同样意味着配置里那句“只有一个网关，挂了就断网”**悄悄多了一个退路**，用户既没同意第二出口，也没同意它挂掉时的行为。
  - **口径（2026-10-20 拍板，取严）**：以前能存的「单 peer + 出口、未勾确认」配置，现在会**加载失败**，必须勾选后才能保存。代价是老配置要重新确认一次；换来的是“配置写什么就是什么”。
  - `exit_nodes` 热补丁（`config_patch` / `update_exit_nodes`）走同一判定，违反则**拒绝更新并告警**，原有出口列表保持不变。
- **⚠️ 待验收：`select_by` 依赖 `failover_when` 提供多个候选**：`failover_when = "never"` 时候选集只剩 `peers` 的**第 1 个**（"从不切换"语义），`quality` / `balance` 就**没有东西可选**了。⇒ **配置校验会直接报错**（`failover_when = "never"` 只能配 `select_by = "priority"`），不再静默退化。
- **`failover_when = "never"` = 钉死那个 peer**（L3，2026-10-20）：`never` 约束的是**流量交给谁**，不是"规则还在不在"。第一个解析成功的 peer 会被**锁定**（`pinned_peer`），之后：
  - **该 peer 不可达时规则照常路由到它** —— 不再翻成 `Degraded` 后落到物理网关（`open`）或丢包（`closed`），那正是用户明确排除的出口。**该状态持续到配置变更**，不存在"某秒之后自动解除"；
  - L2 **继续**把该规则的 CIDR 装进路由表（否则包根本进不了 TUN，锁定无从谈起）；
  - 锁定**按远端静态公钥识别节点，不按 `PeerId`**。`PeerId` 是节点启动时随机取的 `u32`、由对端在握手里自报 ⇒ 重启就变、数值还会被别的节点重新用到。所以：
    - 出口节点**重启换了新 `PeerId`、公钥不变** ⇒ 自动跟随到新 id，**不会**永久黑洞；
    - 旧 `PeerId` 被**另一个节点**占用 ⇒ 公钥不符，**解除锁定**，绝不把流量发给陌生节点；
    - 节点只是**掉线**（从 peer map 被回收、可能长达数分钟） ⇒ 锁定**保留**，继续等它。

  状态面「锁定 peer」列 / `easytier-cli route policy` 的 `pinned=` 字段可核对。**代价（必须知道）**：在节点回来之前，该网段**一直不通** —— 包会进入隧道并在隧道层丢弃。这是"只用这个出口"的直接含义，不是缺陷。
- **规则 = 出口（重要）**：`[[policy_routes]]` 命中后**一律置 `exit_node` 标志**，即被指向的 peer **必须开启**「允许此节点成为出口节点」（`enable_exit_node`），否则该 peer 会**丢弃**这些包（不会静默回退到其它路径）。网关侧仍**优先查自己的 `proxy_cidrs`**：命中即按子网代理转发，没命中才 NAT 出网。
- **切换防抖（仅 `quality`）**：发布侧沿用 `MIN_DELTA=20`；`select_by = quality` 时，相对 10% / 绝对 5 cost 单位带内不动，带外还需**连续 3 个 tick** 才切。`priority` **不**走切换带（否则成本恒 0，优选永远切不回来）。
- **L2（桌面）**：`cidr_monitor` 从**同一份快照**取 `Stable` / `Failover` 规则的 CIDR 作为本机代理路由（§3）；override 模式下非 `/0` 的 policy CIDR 被 `manual_routes` 覆盖，但显式 `/0` 规则不受影响。Android / OHOS **未接线**（§5.2）。
- **域名 → IP（桌面）**：订阅 MagicDNS `resolve()` 的**实际应答**（全代码库唯一能看到应用实际解析结果的位置），学到 `/32` 后**原子**更新 L2 + L3；旧 IP 保留，超上限 FIFO 淘汰（**不**淘汰规则上显式配置的 `/32`）。移动端即便 L3 表有观测结果，宿主也**不会**把这些 `/32` 装进 VpnService / TUN 路由表。
- **配置校验硬失败**：缺 `select_by` / `failover_when` / `if_all_unavailable`、`closed` + 多 peer 未 `require_confirmed`、`quality_degraded_cost` 与 `failover_when = degraded` 未成对、`domains` 落在 `et.net.` 内 —— 均**加载失败并报错**，不静默取默认。**待验收**：`failover_when = "never"` + `select_by ≠ "priority"` 也**加载失败并报错**（候选只剩第一个，两种模式都无从选择）。

通配规则与 Split forwarder 同后缀**共存合法**（2026-10-16 起）：policy 规则经 ObservingDnsResolver 观测真实应答学 IP（含 Split 应答），不抢占 Catalog zone；「Split 赢」仅对静态 hosts **通配**条目成立（`static_host_skip_reason`）。

### 5.0.1 哪些改动能热生效，哪些必须重启

策略路由表**只在实例启动时构建一次**。下表是运维前提，务必按此预期操作：

| 改动 | 是否热生效 | 说明 |
|---|---|---|
| `exit_nodes` 列表 | ✅ **≤1 秒** | `config_patch` → `update_exit_nodes`，对账循环每轮用**当前**的出口列表重新合并候选，不重建规则。新增出口同样受 §5 的不变量约束（候选上限、`closed` 确认），违反则整条更新被拒绝并告警，旧列表保持不变 |
| **`[[policy_routes]]` 规则本身**（增 / 删 / 改 `cidr`、`domains`、`peers`、`select_by`、`failover_when`、`if_all_unavailable`、`quality_degraded_cost`） | ❌ **必须重启实例** | 见下 |

**为什么改规则必须重启**：实例级热补丁接口 `InstanceConfigPatch` 中**没有** `policy_routes` 字段 —— 这不是实现遗漏，而是当前协议层面的约束。可行的路径只有整实例覆盖（`RunNetworkInstance` + `overwrite`），它会销毁并重建 `PeerManagerCore`。

**整实例覆盖的副作用（必须知道）**：

- 所有隧道断开重连、TUN 网卡重建、OSPF 重新收敛 ⇒ **流量中断数十秒**，不是"配置一存就生效"；
- 重建瞬间所有规则回到 `Degraded`，L2 不再往里装代理路由。对账循环的首轮**不等待 1 秒 tick 立刻跑**，所以这个窗口很短，但**不是零** —— 窗口内按规则 CIDR 的 OS 流量会走物理网关（`open` 规则），而不是被拦住。
- **DNS 学到的 `/32` 全部丢失** —— 域名规则的地址是运行时观测来的，重建后要从下一次 DNS 应答开始重新积累。期间该域名规则没有可用地址，行为等同未命中规则。

**对运维的建议**：把 `[[policy_routes]]` 当**部署期配置**看待，随实例重启一起发布；运行期只调 `exit_nodes`（这也是唯一被支持的热操作）。

### 5.1 ⚠️ 泄漏面（产品必须明示）

| 限制 | 说明 |
|---|---|
| **DNS 查询本身不走指定出口** | MagicDNS 上游强制绑**物理网卡**。域名路由只保证「连接」走指定出口，**不保证「解析」**。 |
| **首包最长 1 秒直连窗口** | 域名 IP 靠观测学习：L3 同步就绪，但 L2 的 OS 路由受 **1 秒对账循环**约束 → 首次访问的连接包有**最长 1 秒**从物理网关直连出去。 |
| **切换不迁移已建立连接** | 目标节点掉线时新流切换 ≤2 秒；已建立的 TCP 流需重连。 |
| **`balance` 下出口 IP 不唯一** | P5 语义固有。分摊是**等概率**的，不按链路质量或带宽能力加权。 |
| **`quality` 阈值是复合分** | P4，不能按 RTT / loss 维度分解。 |

> **本设计不保证零泄漏。** 缩小首包窗口可配合 `DnsConfig.hosts` 静态绑定 IP 使 DNS 与路由时序对齐；真正零泄漏需另立项。

### 5.2 平台矩阵（域名路由可用性）

此处「可用」= **L3 能选 peer 且 L2 能把目标装进系统/VPN 路由**（否则包进不了 TUN）。

| 平台 | 域名 / `policy_routes` 导流 | 原因 |
|------|-----------------------------|------|
| Windows / macOS（非 NE）/ Linux（systemd）/ OpenWrt（手工） | ✅ | 本进程 `resolve_proxy_cidrs` + MagicDNS 自动接线；观测 `/32` 进 OS |
| Android（Tauri） | ✅ 新 core；老 core ⚠️ | 新 core：`getRoutesForVpn` 装 `desired_proxy_cidrs`（含 policy CIDR / 观测 `/32`）；老 core 仍只拼 peer `proxy_cidrs` + 配置 `routes`（仅全隧道近似） |
| Android（JNI） | ✅ 新 core；老 core ⚠️ | 新 core 下发期望集合（含 policy CIDR / 观测 `/32`）直接装进 VpnService；老 core 同上（仅全隧道近似）。两种关 DNS 都回落运营商 DNS |
| OHOS | ✅ 新 core；老 core ⚠️ | 新 core：`aggregate_tun_routes` 装 `desired_proxy_cidrs`；老 core 仍本地拼装（未读 `l2_contributing_cidrs`） |
| **iOS / macOS NE** | ❌ | OS 不走 EasyTier DNS，拿不到解析结果 |
| **`no_tun`** | ❌ | MagicDNS Runner 不启动，无观测来源 |

（DNS 接线矩阵的完整版见 [`magic-dns.md`](./magic-dns.md) §7。）

### 5.3 升级影响（2026-10 策略路由加固）

本版调整了两条与 `[[policy_routes]]` 相关的**用户可见行为**。两者都是把「配置写的」与「实际跑的」对齐，但升级后**老配置的表现会变**，发版说明与帮助文案需要覆盖。

| # | 变更 | 升级后表现 | 需要做什么 |
|---|---|---|---|
| **E1** | **`if_all_unavailable = "closed"` 的确认按「合并后候选数」判定**（原先只看配置里的 `peers`，出口节点在其后才并入，可绕过确认） | 若配置是「1 个 `peers` + 有 `exit_nodes` + `closed` + 未勾 `require_confirmed`」，**升级后加载失败并报错**，点名 `require_confirmed=true` | 在配置里显式勾选「我接受丢包风险」（`require_confirmed = true`），或去掉 `exit_nodes` / 改用 `if_all_unavailable = "open"` |
| **E2** | **`failover_when = "never"` 改为真正钉死目标 peer**（原先该 peer 掉线后规则整体变 `Degraded`，`open` 时退回物理网关、`closed` 时丢包） | 目标 peer 掉线后，流量**持续**只交给它（隧道层丢弃），**不再**自动退回物理网络或其他出口；L2 路由也继续保持。**持续到配置变更**，不是几秒的窗口 | ① 若确实想「只用这个出口，它挂了就不通」——无需改动，这正是新语义；② 若原本**依赖**它掉线时自动兜底——改用 `failover_when = "unreachable"` 并配置备用 `peers`，或改用 `open` |

**补充说明（E2）**：钉住的是**节点身份（远端静态公钥）**，不是 `PeerId` —— 后者是随机的、重启会变、数值还可能被别的节点重用。因此：出口节点**重启后自动跟随**（不黑洞）；旧 `PeerId` 若被**别的节点**占用则**解除锁定**（不误发）。状态面「锁定 peer」列 / `easytier-cli route policy` 的 `pinned=` 可核对。

**新增可观测字段**（不影响行为）：生效候选、锁定 peer、每个候选的代价及其**来源**（`path` = 整条路径累加 / `edge` = 本地首跳实测）。

---

## 6. 验收对照（现状 / D+）

| 场景 | 期望 |
|------|----------------|
| 本机 `exit_nodes` 可解析且有下一跳，未配 `manual_routes`（或 `manual_routes_append = true`）/ `no_tun` | TUN 安装 `0.0.0.0/0` 与 `::/0`；有 underlay 排除；公网 L3 走出口并带 `exit_node` 标志 |
| `exit_nodes` 非空但全部不可达 | **不**装本机默认路由；公网走系统原默认网关 |
| 清空 `exit_nodes` 或出口掉线 | 卸掉上述默认路由与排除；对端子网 CIDR 保留 |
| 只配 `enable_exit_node`，未配 `exit_nodes`、也未宣告 `/0` | 系统路由表通常 **不会** 出现 TUN 默认路由 |
| 对端通告 `10.0.0.0/24` | 本机可同步安装对应 TUN 路由；命中走 LPM，非 exit 标志 |
| 对端通告 `0.0.0.0/0`，本机 **未** 配出口，逃生阀关（默认） | OS **无** TUN `/0`；L3 仍可按兜底选路（但需对端开 `enable_exit_node` 才能真正出网） |
| 对端通告 `0.0.0.0/0`，本机未配出口，`allow_peer_default_without_exit=true` | 桌面装高度量 `/0`（有物理默认时通常不抢；无物理默认时会接管）；移动端会成为真默认 |
| 对端通告 `0.0.0.0/0`，本机 **已** 配可解析出口 | L2 装低度量 `/0`；L3 走 `exit_nodes`，带 `exit_node` 标志 |
| `manual_routes` 开启（`manual_routes_append = false`）且含 `0.0.0.0/0` | 手动列表里的 `/0` 照装（用户自己声明全隧道）；对端仍被过滤、`exit_nodes` 不自动加 |
| `manual_routes` 开启（`manual_routes_append = false`） | L2 学习集合完全由手动列表决定；**不**因 `exit_nodes` 自动加 `/0`；逃生阀保留的对端 `/0` 同样被覆盖 |
| `manual_routes_append = true` 且同时配了 `exit_nodes`（可解析） | L2 = 对端 proxy + policy `local_routes` + 手动列表 + **本机 `/0`**（append 不再否决自节点默认路由意图） |
| `manual_routes_append = true`，未配 `exit_nodes` | L2 = 对端 proxy（含逃生阀保留的 `/0`）+ policy `local_routes` + 手动列表；无本机 `/0` |
| `[[policy_routes]] cidr = "0.0.0.0/0"`（override 模式也含） | L2 装本机 `/0`（本机声明全隧道），并同时保留 `manual_routes` 条目 |
| 移动端同样的对端 `/0` + 无 exit | **不**再整机吸流（与桌面一致）；有 exit 才全隧道 |
| 对端通告 `::/0` | 本机**不**装任何 IPv6 默认路由（`::/0` 只随本机出口安装） |
| `[[policy_routes]]` 配 `cidr = "10.0.0.0/24"`，`peers` 可达（**桌面**） | L3 命中次序 3.5 走该 peer；L2 同步装 `10.0.0.0/24` |
| 同上（**Android / OHOS**） | L3 仍可命中；**L2 不装**该 CIDR（除非已全隧道或手写进 `routes` / `manual_routes`） |
| `[[policy_routes]]` 配 `domains`，应用解析出 A 记录（**桌面**） | 观测学到 `/32`；L3 立即就绪，L2 在 ≤1 秒内装上 |
| 同上（**Android / OHOS**） | L3 表可有观测结果；宿主路由**不**装这些 `/32` |
| `[[policy_routes]]` `if_all_unavailable = closed` 且全部候选不可达 | 命中目标**丢包**，**不**回退物理网关 |
| `[[policy_routes]]` `domains` 落在 `et.net.` 内 | **加载失败并报错**（配置预检） |
| `[[policy_routes]]` `domains = ["*.atlassian.net"]` 且 Split forwarder 也配 `atlassian.net` | **合法共存**（预检放行）：观测学 IP 不受 Split 影响；注意静态 hosts 的**通配**条目遇 Split 仍让位（Split 赢） |
| `[[policy_routes]]` `select_by = "priority"`，优选 A 掉线再恢复 | 先切备选；A 恢复后**下一对账 tick 立即切回 A**（不走切换带） |
| `[[policy_routes]]` `select_by = "quality"`，两候选成本不同 | 选**成本最低**的可达候选；成本差在带内、或未连续 3 tick 占优时不切 |
| `[[policy_routes]]` `select_by = "balance"`，两条不同 TCP 流 | 各按自身流哈希选候选，**可能走不同 peer**；同一条流恒定走同一 peer |
| `[[policy_routes]]` `failover_when = "degraded"` + `quality_degraded_cost = 500` | 候选复合成本 > 500 视为劣化 → 切下一个；全超阈值 → `open` 回退 / `closed` 丢包 |
| `[[policy_routes]]` `quality_degraded_cost` 与 `failover_when = degraded` 未成对 | **加载失败并报错** |
| `[[policy_routes]]` 命中，但目标 peer 未开 `enable_exit_node` | 该 peer **丢弃**这些包（无静默回退）—— 排障先查此项 |
| `[[policy_routes]]` 的 `cidr` 同时被某 peer 通告为子网代理 | 次序 3 先命中 → **规则不执行**（子网代理优先，§2） |
