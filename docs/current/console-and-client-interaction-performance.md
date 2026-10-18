# 控制台与客户端交互性能审查（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-16（指针：Roadmap 阶段 G S10–S13 代码已合入、**待现网验收**后才改写本文正文；F1/F2/F4/F12 表格行已加「→ 已改 Ns，待验收」内联标注；2026-10-15 首版审查结论仍有效）
- 范围：**控制台 / 客户端 UI 的交互延迟**（不含数据面转发性能）
- 覆盖：ET Web 控制台（`easytier-web` + 前端）、桌面 / 安卓 GUI（`easytier-gui`）、节点侧管理 RPC（`easytier-core`）
- OpenWrt LuCI 另见：[`../../luci-app-easytier/docs/web-console-performance-audit.md`](../../luci-app-easytier/docs/web-console-performance-audit.md)（该文 P1–P3 已于 2026-10-16 修复，待真机复测）
- 相关：[`peer-connections.md`](./peer-connections.md)、[`p2p-direct-stability-audit.md`](./p2p-direct-stability-audit.md)、[`../roadmap/client-web-console-interaction.md`](../roadmap/client-web-console-interaction.md)（§9 S1–S9；阶段 G S10–S13）、[`../roadmap/poll-to-push-roi.md`](../roadmap/poll-to-push-roi.md)（轮询→推送选型；**P0 已落地，待 GUI 验收**）
- 索引：[`../README.md`](../README.md)

> 本文区分 **[已核实]**（逐行读过代码）与 **[审查报告]**（委托审查产出、带行号锚点，其中抽样复核过）。**未做真机 profiling 与抓包。**

---

## 1. 结论

Web 控制台与安卓客户端**确实都慢**，且两者的**根因相同**：一个**每 2 秒轮询的全量拓扑快照 RPC**，外加若干**未收敛的轮询密度**。

| 端 | 最重的一处 | 周期 | 单次成本 |
|----|-----------|------|----------|
| **Web 控制台** | `RemoteManagement` 状态页 → `collect_network_info` | **2 s** | 全量 peer+route 快照（含 3× 冗余载荷） |
| **安卓 / GUI** | 同上（GUI 复用同一 Vue 组件） | **2 s** | 同上 **+ Android 额外一次 Binder 往返** |
| **GUI 外壳** | `refreshConfigServerConnection` / `isClientRunning` | **各 1 s** | 每秒 ≥2 次跨 Tauri IPC |
| **Web 列表页** | `DeviceList` / `NetworkList` → `/machines` + `/devices` | **2 s** | 每轮 `2×D` 设备 RPC（D=设备数） |
| **Web 设备页** | `DeviceManagement` → 同样两个全量列表 | **3 s** | 仅为解析 1 个设备 |
| **OpenWrt LuCI** | 状态页 / 日志页 | **3 s / 2 s** | `top`×2 + `curl`(3s) + 全量日志 |

**已排除的假因**：S5（Dashboard 2s→5s）与 S4（会话反向索引）**确已落地**；`document.hidden` 守卫、`shallowRef`、轮询不叠加（self-scheduling）等也**确实做对了**（§6）。问题在**未被上一轮审查覆盖的路径**上。

### 1.1 跨平台差异：为什么 Windows 正常，OpenWrt 卡，安卓慢

**先排除一个错误假设**：Windows 与安卓跑的是**同一套前端** —— `easytier-gui` 通过 workspace junction（`easytier-gui/node_modules/easytier-frontend-lib`）直接引用 `easytier-web/frontend-lib`，两端渲染的是**同一份 `RemoteManagement.vue`**。若「共享前端轮询慢」是唯一原因，**Windows 也应同样慢**。⇒ 差异**不在前端**，而在平台侧。

同一份 2 秒轮询，三端真实代价的差异来自五处：

| 维度 | Windows | OpenWrt | Android |
|------|---------|---------|---------|
| **加密后端** | full → 含 `openssl-crypto` | default（`--features=jemalloc`）**不含 `openssl-crypto`** → RustCrypto<br>⚠️ **x86 上两者都走 AES-NI，差距不足以解释卡顿**（见 §1.1 修正） | 视构建 |
| **逐包加密成本** | 低 | 低（x86） | 低～中 |
| **额外 IPC/轮询** | 无 | 无 | **每次轮询多一次 Binder**（见下，已核实） |
| **UI 运行时** | 原生 WebView2 | **Lua + shell 派生进程**（`top`×2、`curl` 3s 无负缓存、11× CLI，见 LuCI 文档） | WebView + JNI + VpnService |
| **定时器** | 不受限 | 不受限 | **Doze / WebView 节流**（本仓代码自述，见下） |
| **载荷规模** | 叶子节点 → peer/route 少 | 常为网关/枢纽 → **peer/route 多，载荷成倍** | 视角色 |

**已核实的平台分支**：`easytier-gui/src/modules/api.ts:20-23`

```ts
// Android DummyIfConfiger never updates L2 sync — fill from VpnService.
if (type() === 'android')
    return await annotateNetworkInfoFromVpnService(info, inst_id)
return info
```

⇒ **Android 每次 `get_network_info` 都额外跨一次 IPC 调 `get_vpn_status()`**（`mobile_vpn.ts:200-211` 内 `syncVpnStatusFromNative(await get_vpn_status())`）；**Windows 直接返回**。该分支在 2 秒轮询上持续生效。

**安卓定时器节流：本仓代码自己写明了**（`easytier-gui/src-tauri/src/android_vpn_watchdog.rs:1-11`）：

> WebView `setInterval` is throttled/frozen in the background and under Doze,
> but the Tauri/Rust runtime keeps running while the process is alive.

⇒ 同一份 `RemoteManagement.vue` 的 2 秒轮询与 `index.vue:1192` 的 1 秒轮询，在 Windows 上正常、**在 Android 上会被节流甚至冻结**。这是「同一份前端、两端体验不同」的**直接书面依据**（该文件本身存在的目的就是绕开这一点：用 Rust 侧看门狗替代 JS 定时器）。

**已核实的「无差异」**：`easytier-core` 内**没有任何 `cfg(target_os = "android")` 分支**（全仓 grep 无命中）—— 核心数据路径三端是同一份代码。
⇒ **核心代码不是差异来源**；在「同为 x86」的前提下，差异来自 **节点在网格中的角色（数据量）、各端 UI 运行时（LuCI shell / Android WebView）、网络路径** —— 详见 §1.2。

**⚠️ 修正（二轮）：加密差异在 x86 对 x86 下不成立 —— 撤回「软件 AES 是主因」**

前版本把「OpenWrt 走纯软件 AES」列为主因。**该结论对 x86 场景不成立，特此更正并撤回**。

仍成立、且值得记录的**构建层事实**：

- 控制台会话数据面**硬编码 AES-GCM**（`web_security.rs:20`），且**每个包**都过加/解密过滤器（`:85-130`）。
- 各平台**发布构建的特性集确实不同**：
  - OpenWrt：`--features=jemalloc` → **default + jemalloc**（`.github/workflows/openwrt.yml:104`）
  - `full` 相对 default 的**唯一增量是 `aes-gcm` + `openssl-crypto`**（`easytier/Cargo.toml:316-356` 逐项比对）
  - ⇒ OpenWrt 构建**不含 `openssl-crypto`**，后端落到 RustCrypto
- 后端优先级 **OpenSSL → ring → RustCrypto**（`tunnel/encrypt/mod.rs:163-204`）；唯一的卸载路径是 **WASI 专用**（`mod.rs:260-276`），其它平台无运行期硬件探测。

**但为何不足以解释卡顿**：RustCrypto `aes-gcm` 在 **x86_64 上同样经 `aes` crate 走 AES-NI**。
⇒ **x86 对 x86 时，OpenSSL 与 RustCrypto 的差距远小于异构场景**，「纯软件 AES」不成立为主因。**需要从软件行为 / 数据量 / 网络路径重新归因。**

### 1.2 重新归因：x86 对 x86 下真正可能的差异

三轮排查后的状态（用户已逐条回答）：

| # | 候选 | 结论 |
|---|------|------|
| ~~R1~~ | 载荷随 peer/route 数增长 | **已排除** —— 用户确认两端 peer/route 数差距不大 |
| ~~R2~~ | LuCI 页面自身的 shell 开销 | **已排除** —— 场景是「在控制台里管理 OpenWrt 设备」，非使用 LuCI 页面 |
| ~~R3~~ | 网络路径 RTT/丢包 | **已排除** —— 用户确认 RTT/丢包正常 |
| ~~（撤回）~~ | 软件 AES（无硬件加速） | **已撤回** —— 同为 x86，两端都走 AES-NI |
| **R6** | **每次快照的「网卡枚举」开销随网卡数量增长** | ✅ **已核实，当前主因**（见 §1.3） |
| R4 | Android 软件约束（Doze 节流 / VpnService / Binder） | ✅ 仍然成立，解释安卓侧 |

### 1.3 R6 已否证 —— 环境性因素已全部排除，改为要求「先量出慢在哪一段」

**R6（网卡枚举）已否证并撤回**：实测 OpenWrt `ip -o link show | wc -l` = **6**，Windows `Get-NetAdapter | Measure-Object` Count = **6**。
⇒ **两者网卡数相同**，原假设「路由器网卡远多于 PC」的前提不成立。
（该机制本身真实存在 —— 每轮 4 遍枚举，`network.rs:464-508` —— 属**普遍性开销**，但**不是本环境差异的来源**。）

**至此所有「环境性」因素已逐一排除**：

| 因素 | 状态 | 依据 |
|------|------|------|
| CPU 架构 / 算力 | 同为 x86 | 用户确认 |
| 加密后端（AES-NI） | 同为 x86 ⇒ 均走硬件 AES | 用户确认 + 已撤回 |
| peer / route 数 | 差距不大 | 用户确认 |
| RTT / 丢包 | 正常 | 用户确认 |
| **网卡数** | **6 vs 6** | **实测** |
| 共享前端代码 | 同一份 `RemoteManagement.vue` | junction 已核实 |
| 核心代码分支 | **无** `os_type` / `target_os` 分支 | 全仓 grep |
| 控制台部署 | Linux x86，与 OpenWrt 分离 | 用户确认 |

⇒ **当所有可测的环境因素都相等时，问题大概率不在「平台相关的代码路径」**，而在：
**(a)** 用户实际执行的**具体操作**不同；**(b)** 设备的**运行时状态 / 配置**差异；或 **(c)** 对「Windows 侧」的理解有偏差。

#### 需要先澄清的关键歧义

「Windows 和控制台交互没问题」是指：

- **(A) 在控制台里管理一台 Windows 设备**（与 OpenWrt 同为被管设备）—— 则二者可严格对比；或
- **(B) 用 Windows 上的浏览器访问控制台**（Windows 只是浏览器载体）—— 则三者**并非同一维度**，此前「为什么 Windows 设备不卡」的追问方向本身不成立。

#### ✅ 决定性测量已完成（HAR 实测，2026-10-15）—— 设备侧 RPC 已排除

用户在控制台会话中导出了 HAR（145 个请求 / 约 95 秒）。结论：

| 端点 | 样本 | p50 | max | 说明 |
|------|------|-----|-----|------|
| `machines/{id}/networks/info/{id}` | 17 | **84 ms** | 174 ms | 即 `collect_network_info`（全量快照 RPC） |
| `machines/{id}/networks` | 30 | 83 ms | 223 ms | |
| `machines/{id}/networks/config/{id}` | 18 | 48 ms | 214 ms | |
| `machines` | 35 | 49 ms | 115 ms | |
| `devices` | 35 | 48 ms | 116 ms | |

**按设备拆分**（同一 RPC）：

| machine | n | p50 | max |
|---------|---|-----|-----|
| `69cc9738…` | 12 | **84 ms** | 101 ms |
| `7a9a0a45…` | 5 | **81 ms** | 174 ms |

⇒ **两台设备的快照 RPC 耗时几乎相同**（84 vs 81 ms）。

**其他事实**：

- **145 / 145 全部 HTTP 200**，无超时、无重试、无 pending。
- 请求耗时几乎全在服务端 **TTFB**：`wait` p50 ≈ 80 ms、`receive` ≈ 2 ms；`dns` = 0（已有连接）。
- `networks/info` 响应体仅 **约 29～35 KB**。

**由此可以确定**：

1. **「控制台 → 设备」的 RPC 路径是健康的**（~80 ms 量级）。此前怀疑的「设备侧处理慢」在本次采样中**不存在**。
2. **负载绝对量很小（~30 KB）** ⇒ §2.1 的「3 倍冗余」在**代码上成立**，但**实际影响应下调**（见 §2.1 修正）。
3. ⇒ 若用户**仍感觉卡**，而网络侧全部 ≤223 ms，那么瓶颈只可能在**浏览器渲染**（§4.2 A4：13 列无虚拟滚动表格每 2 秒全量重建）。
   **HAR 无法捕获渲染耗时**——需要 `Performance` 面板（非 `Network`）才能定位。

> 📌 **用户反馈：本问题「目前好像没有那么慢了」** —— 与本次 HAR 实测（全部 ~80 ms、无异常）互相印证。**本文件保留全部结论，但当前无可复现的性能缺陷证据。**

#### 仍未被排除的一条：config-server 会话协议

- LuCI 插件文档给出的自建控制台格式是 **`udp://server:22020/用户名`**（`luci-app-easytier/.../cbi/easytier.lua:46-48`），`web_protocol` 默认亦为 **`udp`**（`easytier.lua:704`）。
- 核心侧过滤器名为 **`SecureDatagramTunnelFilter`**（`web_security.rs:80`，**datagram 语义**），且管理 RPC 层未见重传实现。
- 而快照响应可达数百 KB。

⇒ **若 OpenWrt 侧走 UDP 会话而 Windows 侧走 TCP**，则大响应在 UDP 上会明显更慢。
**核对方法**：看两端实际生效的 scheme（`easytier-cli` connector / 控制台会话状态），或抓包看 22020 是 UDP 还是 TCP。

> ⚠️ 以上**尚未验证**。在拿到 §1.3 的 DevTools 数据之前，不再新增猜测性归因。

**关于「路由器自带控制台」**：LuCI 插件确实具备在路由器上跑控制台的能力（`easytier-web-embed`，`easytier.lua:489`），其状态页还每次渲染都派生 `top` / `cat /proc`（`:107`、`:111`）。
但**用户已确认本环境控制台在 Linux、与 OpenWrt 分离**，故该路径**不适用于本环境**，仅作其他部署形态的参考。

**修正后的放大链**：

```
每 2 秒一轮（§3 的轮询密度）
   ├─【本机成本】节点重建快照：4 遍网卡枚举 + 逐网卡 await + 2 次 UDP 探测
   │        └─ O(网卡数)；本环境 6 vs 6 ⇒ **不构成差异**（§1.3 已否证）
   ├─【网络成本，§2.1】约 3 倍冗余载荷，经控制台会话逐包 AES-GCM 加密后发送
   └─【各端 UI 成本】Android：额外 Binder + VpnService 拆建 + Doze 节流（R4）
```

**结论**：§2 的快照冗余与 §3 的轮询密度是**共同根因**（对所有端都成立），但它们**不能解释** Windows 与 OpenWrt 的差异 —— 因为相关环境因素已证明相等。

差异来源的当前状态：

- **Android**：**R4** —— WebView 定时器 Doze 下被节流、VpnService 反复拆建、每次轮询多一次 Binder。**已核实，成立。**
- **OpenWrt**：**尚未确定**。R1（数据量）、R2（LuCI 页面）、R3（网络）、R6（网卡数）**均已排除**。
  ⇒ **必须先做 §1.3 的 DevTools 测量**，判定慢在**设备侧 RPC** 还是**浏览器渲染**，再继续归因。

> 📌 **已确认事实**：控制台在 **Linux x86**；OpenWrt 与控制台是两台独立 x86 机器；两端网卡数均为 **6**；peer/route 数相近；RTT/丢包正常；核心代码无 OS 分支。
> ⇒ **在所有可测环境因素均相等的前提下，不应再假设存在「平台相关的代码路径差异」。**

---

## 2. 共同的根因：2 秒一次的「全量拓扑快照」

**[已核实]** 完整链路：

```
RemoteManagement.vue  STATUS_POLL_MS = 2000        (:635, task :1684-1720)
  └─ loadCurrentNetworkInfo()                      (:1459-1493)
       └─ props.api.get_network_info(inst_id)      (:1473)
            └─ REST GET /machines/{id}/networks/info/{inst_id}
                 └─ easytier-web client_manager handle_collect_network_info
                      └─ 节点 RPC collect_network_info        (60s 超时, rpc_timeout.rs:37)
                           └─ network_instance_running_info() (instance_info.rs:13-87)
```

节点侧每次都构建**完整快照**（`easytier-core/src/management/full/instance_info.rs`）：

| 字段 | 行 | 内容 |
|------|-----|------|
| `peers` | `:31-45` | 全部 peer，**每个 peer 携带其全部 conn**（含隧道信息） |
| `routes` | `:47-52` | 全部路由 |
| **`peer_route_pairs`** | `:53` | **peers 与 routes 的连接结果** |
| `events` | `:73` | 事件（**有上界 20**，`event_journal.rs:70` ✅） |
| `foreign_network_summary` | `:79` | 外层网络汇总 |
| `proxy_cidr_route_sync` | `:80` | 代理 CIDR 同步状态 |

### 2.1 载荷约 3 倍冗余 [已核实]

`api_manage.proto:242-256` 同时下发 `peers`(5)、`routes`(4)、`peer_route_pairs`(6)。而 `PeerRoutePair` 的定义是（`api_instance.proto:103-106`）：

```proto
message PeerRoutePair {
  Route route = 1;
  PeerInfo peer = 2;      // ← 内嵌一份完整 PeerInfo
}
```

构造处 `easytier-proto/src/api.rs:323-334`：

```rust
for route in routes.iter() {
    let peer = peers.iter().find(|peer| peer.peer_id == route.peer_id);  // 线性查找
    let pair = PeerRoutePair { route: Some(route.clone()), peer: peer.cloned() };  // 深拷贝
```

⇒ **每个 route 克隆一份完整 `PeerInfo`**；同一份 peer 数据在 `peers` 与 `peer_route_pairs` 里**各发一次**。O(路由 × 节点) 比较 + O(路由) 次深拷贝。

> N 个节点的全互联网络：R≈N、P≈N ⇒ **该结构本身**是「理论最小」的约 3 倍。

**⚠️ 定级下调（HAR 实测后）**：实测 `networks/info` 响应体仅 **约 29～35 KB**（见 §1.3），且耗时几乎全在 TTFB（`receive` ≈ 2 ms）。
⇒ **该冗余在代码层面成立，但绝对量很小**，在当前规模下**不构成可感知瓶颈**。
⇒ 应作为**中等优先级的整洁性优化**（去掉重复字段 / 前端 join），**而非**此前所称的「单项最大收益点」。**该定级已在 §7 修正。**

### 2.2 快照本身也是串行的 [已核实]

- `collect_network_infos`（`instance/manager.rs:754-765`）：`for instance in self.list()` 内 `.await` ⇒ 多实例时延迟是**累加**而非取最大。
- `list_peer_snapshots`（`peers/peer_manager.rs:1517-1544`）：**每个 peer** 都要 `list_peer_conns(...).await` + `get_peer_default_conn_id(...).await` ⇒ O(peers) 次锁往返，而它每 2 秒被调用一次。

---

## 3. Web 控制台

| ID | 位置 | 周期 | 问题 | 状态 |
|----|------|------|------|------|
| **F1** | `RemoteManagement.vue:635` | **2 s** | 全量快照（§2） | [已核实] → **S12 已改 5s，待验收** |
| **F1b** | `remote_client.rs:122-143` | 随 F1 | **同一次 2 s 轮询里还有第二个重 RPC**：`list_network_instance_ids` 内含 **2 次 RPC**（`list_network_instance` + `list_network_configs(ListNetworkProps::All)`）**并逐行 clone 全部配置**（`manager.rs:464-492` 的 `entry.value().clone()`） | [审查报告] |
| **F2** | `DeviceManagement.vue:105-117` | **3 s** | `loadMergedDevices` → `/machines` + `/devices` **两个全量列表**，仅为解析 1 个设备 | [已核实] → **S13 已改 5s，待验收** |
| **F3** | `network.rs:290-300` | 随 F2/F4 | `handle_list_machines` **N+1**：循环内 `get_heartbeat_requests().await` + `get_machine_location().await`，各取一次 `RwLock` | [已核实] |
| **F4** | `DeviceList.vue:54-58`、`NetworkList.vue:346-350` | **2 s** | 每轮 `1×/machines + 1×/devices + 2×D 设备 RPC` | [已核实] → **S13 已改 5s，待验收** |
| **F5** | `instance/manager.rs:754-765` | — | 实例循环串行 `.await` | [已核实] |
| **F6** | `peer_manager.rs:1517-1544` | — | 每 peer 锁往返 | [已核实] |
| **F7** | `restful/mod.rs:225-231` | 5 s | `/summary` 遍历全部机器 + 每机器一次 await（S5 只降了频率，未降 O(machines) 成本） | [审查报告] |
| **F9** | `RuntimeLogs.vue:238-240` | **3 s** | 裸 `setInterval`，**无 `document.hidden` 守卫**（共享 helper 有守卫，唯独此处绕过）⇒ 后台标签页持续拉日志 | [审查报告] |
| **F10** | `Status.vue:37-95`、`:691-781` | 随 F1 | 四个 computed 各自遍历 `peer_route_pairs`；`DataTable` **无 paginator / 无虚拟滚动**；另有一个 2 s 定时器算速率 | [审查报告] |
| **F12** | `rpc_timeout.rs:11,37` | — | **只读**的 `collect_network_info` 被划到 **60 s** 超时；设备不可达时页面空转最长 60 s | [已核实] → **S11 已改 15s probe，待验收** |

### 3.1 两处自相矛盾/失效的配置 [已核实]

1. **注释与实现不符**：`rpc_timeout.rs:3` 写「Keep status/meta at 5s」，但 `:37` 把只读的 `collect_network_info` 归入 60 s 慢路径。**读路径不该用 60 s**。
2. **15 s 探针从未启用**：`MANAGED_RPC_PROBE_TIMEOUT_MS = 15_000`（`rpc_timeout.rs:10`）在本文件内**仅有定义与测试引用**，生产路径只用 5 s / 60 s 两档。

### 3.2 开发者已知晓隧道争用 [已核实]

`DeviceList.vue:56` 的注释：

```js
// list_machines 与状态页 CollectNetworkInfo 共享设备隧道；略放宽减轻争用
```

⇒ 团队**已经知道**这些轮询在共享同一条设备隧道上互相争用，并做过 1s→2s 的放宽。但**没有再往下收敛**，也未针对隧道带宽做载荷裁剪。

---

## 4. 安卓 / 桌面 GUI

**为什么 GUI 会命中 Web 侧的问题**：GUI 通过 workspace 链接直接引用同一份组件源码 ——
`easytier-gui/node_modules/easytier-frontend-lib` → `easytier-web/frontend-lib`（junction）。
因此 GUI 渲染的 `RemoteManagement` / `Status` **就是本文 §3 审查的那份代码**，**F1、F10 原样命中**。

### 4.1 定时器全清单 [审查报告]

| # | 周期 | 位置 | 驱动 |
|---|------|------|------|
| T1 | **2 s** | `RemoteManagement.vue:1720` | 主状态轮询（实例列表 + 全量网络信息）= **F1** |
| T2 | **1 s** | `index.vue:1192` | `refreshConfigServerConnection()` |
| T3 | **1 s** | `index.vue:1277` | `isClientRunning()`（service 模式另加 `getServiceStatus`） |
| T6 | **2 s** | `Status.vue:505` | tx/rx 速率重算 |
| T7 | **2 s** | `NetworkChart.vue:296` | 图表数据点推送 |
| T8 | **2 s** | `mobile_vpn.ts:346` | VPN reconcile 重试（有界，上限 60 次） |
| T9 | **2.5 s** | `mobile_vpn.ts:306` | 路由刷新去抖 |
| T4 | **10 s** | `mobile_vpn.ts:1039` | `syncMobileVpnService()` |
| T5 | **30 s** | `android_vpn_watchdog.rs:23,46` | Rust 侧孤儿 VPN 看门狗 |
| T11 | **3 s** | `EasyTierManager.kt:38` | **另一套**并行 Android VPN 监控（见 §4.3） |

### 4.2 关键问题

| ID | 位置 | 周期 | 问题 | 状态 |
|----|------|------|------|------|
| **A1** | `index.vue:1192-1194` | **1 s** | `refreshConfigServerConnection()`。normal 模式已是本地读（✅ 无 RPC）；**service/remote 模式每秒发 `get_config_server_status` RPC** | [已核实] |
| **A1b** | `index.vue:350,288,296` | 随 A1 | ⚠️ **可升级为重启风暴**：`connected` 时调 `maybePruneWebConfigsFromConsoleW` → `list_network_instance_ids()` + `sendConfigs()` → `load_configs`，而后者在 `manager.rs:912-1008` 会**清空全部配置并重启所有实例**。有 `lastPrunedConsoleWebOwnedKey` 守卫（非每秒触发），但**控制台 W 一变，1 秒级轮询就可能拉起一轮实例重启** | [审查报告] |
| **A2** | `index.vue:1277-1295` | **1 s** | `isClientRunning()`（`lib.rs:1058-1060`）跨 IPC | [已核实] |
| **A3** | `modules/api.ts:20-22` | 随 F1 | Android 下 `annotateNetworkInfoFromVpnService` → `get_vpn_status()`（`mobile_vpn.ts:205` → Kotlin `VpnServicePlugin.kt:161`）⇒ **每次 2 s 轮询多一次 Binder 往返** | [审查报告] |
| **A4** | `Status.vue:691-781` | 随 F1 | 13 列 `DataTable` **无虚拟滚动 / 无分页**，且 `curNetworkInfo` 每 2 s **整体替换**（`RemoteManagement.vue:1487-1492`）⇒ 全部 computed 失效 + 整表重建 —— **单帧最大开销** | [审查报告] |
| **A5** | `index.vue:390-422` | — | `waitForConfigServerOutcome` 500 ms 粒度阻塞最长 **25 s**，期间 `isModeSaving` 为 true ⇒ 界面「假死」；`waitVpnStatus` 以 **50 ms** 轮询（最多 160 次唤醒） | [审查报告] |
| **A6** | `VpnServicePlugin.kt:114-147` | — | `startVpn` 在 **主 looper** 上同步 `stopInternal()` 拆 TUN 再 `establish()`，JS 全程 await | [审查报告] |
| **A7** | `RemoteManagement.vue:285-291` | — | `watch(currentNetworkConfig, …, { deep: true })` ⇒ 配置编辑期每次按键深度遍历整个 config 对象 | [审查报告] |

**净效应**：GUI 常驻**每秒 ≥2 次跨 IPC**（T2/T3），叠加 2 s 的全量快照（T1）、Android 额外 Binder 往返（A3）与整表重建（A4）。Android 上 WebView↔Rust 桥接本就慢于桌面，且持续阻止 WebView 休眠 ⇒ 交互卡顿。

### 4.3 一套疑似无用的并行实现 [审查报告]

`easytier-contrib/easytier-android-jni/kotlin/.../EasyTierManager.kt` 是**另一套独立的 Android VPN 实现**：
- `MONITOR_INTERVAL = 3000L`（`:38`），由 **`Handler(Looper.getMainLooper())`** 驱动（`:41,:63-71`）—— **主线程**；
- `monitorNetworkStatus()`（`:128-212`）在主 looper 上 `EasyTierJNI.collectNetworkInfos(10)` 并用 Moshi **解析完整 `NetworkInstanceRunningInfoMap`**；
- 任一 IPv4/CIDR/MagicDNS 变化即 `restartVpnService()` ⇒ **`stopVpnService()` + `startVpnService()` 完整弹一次 VPN**，且**无去抖/无宽限**（与 TS 路径的 `VPN_TRANSITION_GRACE_MS` 等保护不同）。

**推断**：Tauri 构建走的是 `tauri-plugin-vpnservice`，`EasyTierManager` 疑为旧版独立 JNI 应用的遗留，**很可能不在 Tauri APK 的运行路径上**。
**待确认**：若任一路径会构造 `EasyTierManager`，则额外叠加**第三个 3 秒主线程轮询**与无条件 VPN 弹跳。建议用 logcat 搜其 `TAG = "EasyTierManager"` 定性。

> ✅ 注：`android_vpn_watchdog.rs` 本身**没有问题** —— 30 s 间隔、Tokio 运行时、`MissedTickBehavior::Delay`、只用 `try_read`（锁竞争不会被误判为「无 TUN 实例」）、5 s 启动延迟，**不阻塞 UI 线程**。

---

## 5. 与 RDP 卡顿的关系（隧道争用）

这解释了「打开控制台，RDP 更卡」：

```
任一控制台页面打开
   ├─ 每 2~3 s 拉取全量快照 / 全量列表（经 EasyTier 隧道）
   ├─ GUI 每秒 ≥2 次 IPC + Android 额外 Binder 往返
   └─ 突发流量涌入发送路径
            └─ 环缓冲仅 128（可用 124），满即静默丢包
                     └─ 与 RDP 抢同一队列 ⇒ RDP 丢包 ⇒ 卡顿
```

队列机制见 [`p2p-direct-throughput-audit.md`](./p2p-direct-throughput-audit.md) §3 B2；闪断恢复代价见 [`p2p-direct-stability-audit.md`](./p2p-direct-stability-audit.md) §3.1。

**放大因素**：全量快照载荷约为最小值的 3 倍（§2.1），而它每 2 秒重发 —— 即在同一条隧道上持续注入数倍于必要的流量。

---

## 6. 已经做得对的部分（诚实对照）

避免把本文读成「到处都烂」，以下经核实无问题：

| 项 | 依据 |
|----|------|
| S5 已落地：Dashboard `/summary` = **5 s** | `Dashboard.vue:35` [已核实] |
| S4 已落地：会话查找按本机绑定 URL，非全表扫 | `client_manager/mod.rs:489` [审查报告] |
| `document.hidden` 守卫已存在 | `utils.ts:544-548`（仅 `RuntimeLogs` 绕过）[审查报告] |
| 轮询**不会叠加**：`PeriodicTask` 自调度 `setTimeout` | `utils.ts:531-553` [审查报告] |
| `shallowRef` 避免深度代理 | `usePollingList.ts:30` [审查报告] |
| 轮询并发保护 + 手动刷新去重 | `usePollingList.ts:51-87` [审查报告] |
| toast 有 30 s 限流，失败设备不刷屏 | `usePollingList.ts:37-49`、`deviceArchive.ts:9-10` [审查报告] |
| 日志载荷有界 | `admin_diagnostics.rs:466-469,485-488` [审查报告] |
| `/devices` 返回 6 字段 DTO，非整行 | `restful/mod.rs:253-265` [审查报告] |
| 响应压缩已开启（br/deflate/gzip/zstd） | `restful/mod.rs:449-454` [审查报告] |
| 事件列表**有上界 20**，不会无限增长 | `event_journal.rs:70` [已核实] |
| metas / infos 已是**批量** RPC | `api.ts:427-436,531-536` [审查报告] |
| Rust 侧 VPN 看门狗不阻塞 UI | `android_vpn_watchdog.rs:23,44-47` [已核实] |

---

## 7. 修复优先级

| 序 | 项 | 改动面 | 预期 |
|----|-----|--------|------|
| **0** | **先做 §1.3 的 DevTools 测量**（非代码改动）：分别选中 OpenWrt / Windows 设备，比较 `networks/info` 请求耗时，判定慢在设备侧还是浏览器侧 | 无 | **在归因确定前不要动手改代码** |
| — | ~~R6 网卡枚举加 TTL 缓存~~ **已降级为普通优化**：机制真实（每轮 4 遍枚举，`network.rs:464-508`），但本环境 6 vs 6，**不是差异来源**；仍值得做，属普适收益 | core（`common/network.rs`） | O(网卡数) → O(1) |
| **1** | **裁剪 F1 载荷**：`peer_route_pairs` 改为前端自行 join（`peers`/`routes` 已在包内），或加 `fields` 参数按需返回 | proto / core / 前端 | 载荷约降至 1/3（网络 + 加密双重减负） |
| **2** | **A4/F10 表格虚拟化 + 停止整体替换**：peer 表加 `virtualScrollerOptions`/`paginator`、按稳定 id 设 `:key`；`curNetworkInfo` 仅在指纹变化时替换 | 前端 | **单帧体感最大**；风险低 |
| **3** | **F12 读路径脱离 60 s**：`collect_network_info` 归回 5 s（或启用已存在但生产未用的 15 s 探针），60 s 只留给写操作 | 仅 web | 消除「设备不可达 → 白屏 60 s」 |
| **4** | **A1b 把 prune 移出 1 秒轮询**：改事件驱动或 ≥30 s 周期；`load_configs` 改 diff-apply 而非 clear-restart | GUI + core | 消除「控制台一变 → 实例重启风暴」 |
| **5** | **降低轮询密度**：F1 的 2 s → 5 s（与 S5 对齐），2 s 只做轻量 running/error 探针；T2/T3 的 1 s IPC 改事件驱动（`vpn_service_*` 事件已存在） | 前端 / GUI | 直接砍掉大部分基线频率 |
| **6** | **合并/并行化后端扇出**：`handle_list_network_instance_ids` 两次 RPC 合一（`remote_client.rs:122-143`）并去掉逐行 clone（`manager.rs:464-492`）；F5/F6/F3 的循环改 `join_all` 或单次取锁批量构建 | core + web | 纯收益，无接口变更 |
| **7** | **A3 去掉每次轮询的 `get_vpn_status`**：改用 `vpn_service_start/stop` 事件维护缓存 | Android | 每秒省一次 Binder 往返 |
| **8** | **A6 把 TUN establish/teardown 移出主 looper** | Android 插件 | 消除单次切换的界面阻塞 |
| **9** | **F9 / F2 / F4**：`RuntimeLogs` 改用 `PeriodicTask`（自带 hidden 守卫）；列表页 2 s/3 s → 5 s；归档 `/devices` 与实时 `/machines` 分离频率 | 前端 | 降低基线 |
| **10** | **确认 `EasyTierManager.kt` 是否死代码**（§4.3），是则删除 | Android | 排除潜在第三个 3 s 主线程轮询 |
| **11** | ~~弱 CPU 平台换 AEAD 后端~~ **（已降级，见 §1.1 修正）**：x86 对 x86 时 OpenSSL/RustCrypto 都走 AES-NI，收益有限。仅在**异构平台（MIPS/老 ARM）**或实测证明加密占 CPU 时才评估 `chacha20`（注意 `easytier-core/Cargo.toml:101` 的 default **不含** `chacha20`，需构建期开启） | 构建配置 + 网络配置 | 仅异构场景有效 |
| **12** | **降低载荷规模（= 第 1 项的背景）**：`peer_route_pairs` 前端自行 join、按需字段 | proto / core / 前端 | ⚠️ **定级下调**：实测响应仅 ~30 KB（§1.3），**收益有限**，属整洁性优化 |
| **13** | ~~确认「与控制台交互」的含义~~ **已确认**：为「在控制台里管理 OpenWrt 设备」（非 LuCI 页面）⇒ R2 不适用 |

> **重要前提（HAR 实测后修订）**：本次采样中**未复现任何性能缺陷** —— 全部请求 200、`networks/info` p50 = 84 ms、两台设备一致（§1.3）。
> ⇒ **上表应视为「若问题复现时的候选清单」，而非「已确诊的缺陷修复单」。**
> ⇒ **首要动作是复现并定位**（若再现卡顿，用 `Performance` 面板而非 `Network`，因网络侧已证明健康）。
>
> ~~注意第 1 项的双重收益…~~ **该论证的前提（加密成本为主要瓶颈）已随 x86 修正而失效，一并撤回。**

---

## 8. 局限

1. **已完成一次网络侧实测**（HAR，§1.3）：结果**未复现性能缺陷**（全部 200、p50 84 ms）。但这**只覆盖网络层**——渲染/主线程耗时 HAR 无法反映，故「浏览器渲染是否导致卡顿」**仍未验证**。
2. **数据面未测**：本文只审 UI 交互路径；§5 的隧道争用因果链中，「队列满丢包」已由代码证实，但「控制台轮询导致 RDP 可感卡顿」的**定量**关系未实测。
3. **审查报告项未逐行复核**：标 `[审查报告]` 的条目来自委托审查（带行号锚点）。我**亲自复核**了 F3、F4、F12、A1、A2 与全部 `[已核实]` 项 —— **均准确**；其余名未逐条验证。其中 **A1b（重启风暴的触发概率）、A3/A5/A6、以及 F4 的 `META_TTL_MS` vs 2 s 间隔推断**最需进一步确认。
4. **`EasyTierManager.kt` 待运行时确认**（§4.3）：代码确实存在且含 3 s 主线程轮询 + 无条件 VPN 弹跳，但**是否加载进 Tauri APK 未验证**；建议用 logcat 搜其 TAG 定性。
5. ~~跨平台归因含部署拓扑推断~~ **已由用户确认并修正**：控制台运行在 **Linux x86**（不在 Windows），故「Windows 端近似回环」的推断**作废**。
6. ~~路由器跑纯软件 AES~~ **已撤回**（见 §1.1 修正）：用户确认**控制台与 OpenWrt 均为 x86**，RustCrypto 的 `aes` 在 x86_64 上同样走 AES-NI，故该差异不足以解释卡顿。
7. **已确认的部署事实**（用户提供）：控制台运行在 **Linux x86**，**OpenWrt 与控制台是分开的两台机器**，均为 x86。
   ⇒ 因此 §1.2 的归因**不再包含 CPU 能力差异**；「控制台宿主是弱 CPU」亦已排除。
8. **§1.2 的三个问题已获用户回答**：peer/route 数差距不大（排除 R1）、RTT/丢包正常（排除 R3）、场景为「在控制台里管理 OpenWrt 设备」而非使用 LuCI 页面（排除 R2）。
9. ~~R6 待实测量化~~ **R6 已否证**：实测两端网卡数均为 **6**（§1.3），前提不成立。
10. **累计已排除 / 撤回的假设（如实记录，避免重复踩坑）**：① 软件 AES（x86 不成立）；② R1 数据量；③ R2 LuCI 页面；④ R3 网络路径；⑤ R6 网卡数。
    ⇒ **三种独立归因尝试均被实测否证**，这本身是重要信号：**在全部可测环境因素相等时，应先复现问题，而非继续在代码里寻找平台差异。**
11. **结论状态**：本文件对**普遍性开销**（2 秒全量快照、3 倍字段冗余、Android 软件约束、LuCI shell 开销）的记录**仍然有效**；但**「Windows vs OpenWrt 体验差异」这一具体问题，尚未找到成立的原因**。
12. 基于 **v2.7.6** 源码审查；LuCI 侧基于 2.7.5（见其独立文档）。
