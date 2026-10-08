# 安卓：启动后马上自动停止

## Status

- Status: **Ops**（代码防护已落地，待新包现场验收）
- 最近审阅：2026-10-08
- 适用范围：安卓 EasyTier App 点「运行 / 连接」后数秒～约 1 分钟内实例自行停止（非进程崩溃）
- 现场案例：2026-10-08，卸载重装后恢复
- 索引：[`../README.md`](../README.md)
- 相关：
  - [`../roadmap/android-vpn-connection-audit-2026-10-07.md`](../roadmap/android-vpn-connection-audit-2026-10-07.md)（A9 underlay / VpnService 生命周期）
  - [`../current/socket-protection.md`](../current/socket-protection.md)（`VpnService.protect`）

---

## 1. 现象

| 表现 | 说明 |
|------|------|
| 启动后很快「停了」 | UI 从运行中变离线 / 网络实例被禁用；系统 VPN 钥匙图标也可能消失 |
| 进程未必崩溃 | 常见是 **实例被正常 `stop`**，不是 panic / Native 崩 |
| 可反复 | 同一安装包、同一本地状态下，可能连起停两轮（换新 `my_id` 再起再停） |
| 重装常可恢复 | **卸载 App 再安装** 后不再复现（本案例已确认） |

与「长时间运行后 Wi‑Fi↔蜂窝卡断」不同：本条聚焦 **刚启动就断**。

---

## 2. 日志如何定性

排障时把日志级别调到 `info`/`warn`，导出 App 私有目录下最新 `easytier.log`（或设置页提供的日志导出）。

### 2.1 关键串

| 日志关键字 | 含义 |
|------------|------|
| `closed peer conns after underlay network change` | A9：看门狗认为 underlay generation 变了，主动关掉现有 peer 连接以触发重拨 |
| `android vpn watchdog: underlay network generation changed` | 同上，Rust 侧告警 |
| `underlay network changed generation=…` | Kotlin `NetworkCallback` / `setUnderlyingNetworks` 上报 generation 递增 |
| `lifecycle: … shutdown withdrawal instance=…` | **`CoreInstance::stop`**：实例被上层关掉（不是崩） |
| `unexpected packet type: 4/8/9`（握手阶段收到 Ping/Rpc） | 建连不稳或对端已在发业务包；** alone 不构成 stop 命令** |
| `TunDeviceError` / `failed to attach mobile TUN fd` | TUN 路径问题（另一类根因，见 §4） |
| `armed underlay reconnect grace` | 启动宽限期已 arm（`doStartVpn` / `set_tun_fd` 触发） |
| `skip underlay peer reconnect during startup grace` | 宽限期内 `reconnect_peers_after_underlay_change` 被跳过 |
| `android vpn watchdog: underlay generation changed during startup grace; seeding only` | 宽限期内 generation 变化只 seed，不触发重连 |

### 2.2 本案例时间线（摘要 · 防护落地前）

以下为 **代码加固之前** 现场一份完整 log 的典型形态（用于对照旧包 / 未合入防护的构建）：

1. 实例启动，开始连 peer、握手；
2. 约 **1～2 秒内** 出现 underlay generation 变化 → `closed peer conns after underlay network change`；
3. 随后 peer 关连、路由空图、握手 WARN 刷屏；
4. 约数秒～几十秒后出现 **`shutdown withdrawal`**（实例被禁用/停止）；
5. 有时会再起一轮（新 `my_id`），再停一次。

结论：**不是 panic**；能钉死的是 **A9 underlay 误触发重连**，完整停机则是上层 `stop`（外部 VPN 断开禁用实例、配置重推等）。

**含 §6 防护的新包**：启动数秒内更常见 `armed underlay reconnect grace` / `skip … during startup grace`，且同 `networkHandle` 不应再 bump generation。若仍出现 `shutdown withdrawal`，优先按 §5 查 revoke / Always-on / 其它 VPN，而不是再假定「假切换拆隧道」。

---

## 3. 机制说明（为何会「启动就断」）

A9 已落地：Kotlin `registerDefaultNetworkCallback` + `setUnderlyingNetworks`，generation 经 `get_vpn_status` 交给 Rust watchdog；generation 变化时关 peer conn，由 ManualConnector 约 1s 重拨。

正常用途：Wi‑Fi ↔ 蜂窝、漫游后尽快重建五元组。

误伤场景（**§6 之前**；同 netId 去重 + 5s 宽限期用于压这段）：

1. **VpnService 刚 `establish` / 首次 `setUnderlyingNetworks`** 时，系统常立刻回调一次「默认网络变化」，generation 从种子值跳到新值；
2. 若看门狗 / JS 快路径把这次当成真实切换 → **刚建好的隧道被拆掉**；
3. 旧安装残留状态（SharedPreferences、上次 VPN owner、generation 计数、脏配置）会放大误判；
4. 若随后系统 `revoke`/`destroy` VpnService，前端 `vpn_service_stop` → `handleExternalVpnDisconnect` → `updateNetworkConfigState(..., true)` **禁用实例** → 日志出现 `shutdown withdrawal`。

因此（旧包）用户观感是「一点连接就自动停」。

代码锚点（行号可能漂移）：

- `easytier-gui/src-tauri/src/underlay_reconnect_grace.rs`（5s 宽限期 / seed；含主机可跑单测）
- `easytier-gui/src-tauri/src/android_vpn_watchdog.rs`（generation 对比、关 conn、调用宽限期）
- `tauri-plugin-vpnservice/.../TauriVpnService.kt`（underlay callback / 同 netId 去重）
- `easytier-gui/src/composables/mobile_vpn.ts`（`armUnderlayReconnectGrace`、`handleExternalVpnDisconnect`）

---

## 4. 解决方案

按成本从低到高。含 §6 的新包应先确认版本与日志（§4.0）；旧包或仍复现时再清数据 / 重装。

### 4.0 先确认是否已含防护

1. 确认 App 构建已包含 §6（同 netId 去重 + 启动宽限期）；
2. 复现一次，看 log 是否有 `armed underlay reconnect grace`，且启动后数秒内**不应**再出现 `closed peer conns after underlay network change`（宽限期内应为 `skip … grace` / `seeding only`）；
3. 若宽限期日志正常仍 `shutdown withdrawal` → 转 §4.3 / §5（revoke、Always-on 等），不要只重复卸载。

### 4.1 推荐（旧包 / 残留态）：卸载后重装（本案例有效）

1. 系统设置 → 应用 → EasyTier → **卸载**（不要只清缓存就指望与完全卸载等价）；
2. 重新安装当前 APK / 商店包；
3. 重新授权 VPN，导入或拉取配置后启动。

**为何有效**：清掉 App 私有数据（prefs、本地网络配置态、与 VpnService 相关的残留），underlay generation / owner 状态回到干净初值，误触发路径往往消失。

注意：卸载会丢掉本机已存的网络配置（除非走 Web 托管配置，重装后可再拉）。有重要本地 TOML 时先导出。

### 4.2 次选：仅清除应用数据

若不想卸包：设置 → 应用 → EasyTier → **清除存储 / 清除数据**，再开 VPN 权限并重新配置。效果接近重装，但保留 APK 版本不变。

### 4.3 启动前自检（排除系统侧抢占）

| 检查项 | 操作 |
|--------|------|
| 其他 VPN | 关掉其它 VPN App；确认没有 Always-on VPN 指向别的应用 |
| EasyTier Always-on | 若开启 Always-on 且异常，先关掉再手动点连接 |
| 省电 / 后台限制 | 对 EasyTier 关闭「电池优化」；允许前台服务 / 后台活动（厂商 ROM 差异大） |
| VPN 权限 | 系统弹窗必须允许；拒绝后再点连接可能导致立刻 revoke |

### 4.4 仍复现时的取证

1. 调高日志级别，复现一次，导出完整 `easytier.log`；
2. 同步抓：`adb logcat -s TauriVpnService:* VpnServicePlugin:*`；
3. 对照 §2.1：是否有 underlay 关键字、是否有 `shutdown withdrawal`、是否有 `TunDeviceError`；
4. 注明：机型 / Android 版本 / App 版本 / 是否 Web 托管配置 / 是否刚从旧版覆盖安装。

---

## 5. 与其它根因的区分

| 线索 | 更可能是 |
|------|----------|
| 有 `closed peer conns after underlay…`，且发生在启动后数秒内（尤其无 grace 日志） | **本条（A9 误触发；旧包或未合入 §6）** |
| 有 `armed … grace` / `skip … grace`，仍很快 `shutdown withdrawal` | 多半 **revoke/destroy / Always-on / 其它 VPN**，不是假切换拆隧道 |
| 仅有 `shutdown withdrawal`，无 underlay 字样 | 用户点停、配置服禁用、系统 revoke、Always-on 策略 |
| `failed to attach mobile TUN fd` / `TunDeviceError` | TUN fd / A8·A10 类问题 |
| 只有握手 Timeout、`no peer in graph`，长时间无 stop | 对端不可达 / 凭据 / 网络，不是「自动停」 |
| 日志里有 panic / FATAL / tombstone | 真崩溃，另案处理 |

---

## 6. 代码侧加固（已落地）

针对「establish 后假切换」已做：

1. **同 `networkHandle` 不 bump**（`TauriVpnService.applyUnderlayNetwork`）：去掉「仅当 `generation > 0` 才去重」的漏洞；seed 后同一 underlay 的 `onAvailable` / capability 抖动不再把 generation 从 0→1。
2. **会话重置**：每次注册 underlay callback / `clearStatus` 时清零 `underlayNetworkGeneration` 与 `underlayNetworkId`，避免覆盖安装残留放大误判。
3. **启动宽限期 5s**（`underlay_reconnect_grace` + watchdog）：`doStartVpn` 在 `start_vpn` 前调用 `arm_underlay_reconnect_grace`，`set_tun_fd` 成功后再延长；宽限期内 generation 只 seed，JS 快路径与 30s watchdog 都不 `close_peer_conn`。延长宽限期时不重置已 seed 的 generation（避免 `set_tun_fd` 二次 arm 清掉 JS 快路径记过的值）。逻辑抽到独立模块以便桌面 `cargo test` 覆盖 enter/extend/expiry。

仍可能误伤的边角（未改）：宽限期外的真实 `onLost`→新网卡切换仍会重连（符合 A9）；系统 `revoke`/`destroy` 仍会走 `handleExternalVpnDisconnect` 禁用实例。

Status：防护已合入代码；现场是否还复现需新包验证后回写本节。

---

## 7. 一页清单（给支持 / 用户）

1. 确认是「实例停」还是「App 进程没了」。
2. 确认 App 是否已含 §6；看 log 有无 `armed underlay reconnect grace`。
3. 看有没有 `underlay` + `shutdown withdrawal`（有 grace 仍停 → 偏 revoke / Always-on）。
4. 旧包或残留态：先清数据或 **卸载重装**。
5. 查 Always-on / 其它 VPN / 省电限制。
6. 仍复现 → 带完整 log + logcat + App 版本提 issue。
