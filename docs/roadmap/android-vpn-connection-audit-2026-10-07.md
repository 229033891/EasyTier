# 安卓 App 连接问题代码审查（releases/v2.7.41）

## Status

- Status: **Done（代码侧）**（A1–A14 / A3 / A9 / R1–R7 可落地项已修；现场复现与 CI 编译确认仍建议跑一遍）
- 日期：2026-10-07
- 分支：`releases/v2.7.41` / 本地工作区
- 触发：用户反馈「安卓 App 有时候无法连接、卡断」
- 审查范围：
  - Tauri GUI 安卓包（`easytier-gui` → APK `com.kkrainbow.easytier`，CI：`.github/workflows/android.yml`）
  - `tauri-plugin-vpnservice`（Kotlin + Rust）
  - core 的 mobile TUN 接入路径（`easytier/src/instance/runtime_host/tun_mobile.rs`、`virtual_nic.rs`）
- 相关 Current：[`../current/socket-protection.md`](../current/socket-protection.md)、[`../current/peer-connections.md`](../current/peer-connections.md)
- 相关 Roadmap：[`connection-stability-todo.md`](./connection-stability-todo.md)（本文的 A 系列缺陷属「安卓侧生命周期」，与该文 S1–S8「协议/选路」问题正交）

> 复核进展见 §6。**R4 / R7 已关单**（见 §6.6）。A3/A9 已落地。B1/B2 为静态判定，仍需在可编译环境确认。

### 修复进度（2026-10-07）

| # | 状态 | 说明 |
|---|------|------|
| A1 | **已修** | `self` 在 `onStartCommand` 重绑；插件改调 `stopInternal()`，不再误用 `onRevoke()` |
| A2 | **已修** | `establish` 前 `closeVpnInterface`；`vpnInterface` 改为可空并显式关闭 |
| A3 | **已修（最小闭环）** | Rust 30s watchdog：无 TUN 实例时插件直接 `stop_vpn`；emit `vpn_watchdog_tick` 驱动前端对账；实例停用钩子同步停 VPN |
| A4 | **已修** | `START_NOT_STICKY`；null intent/extras 直接 `stopSelf` |
| A5 | **已修** | 超时前 native 复核；`activeVpnInstanceId` 提前写入；等待窗口 8s |
| A6 | **已修** | 无实例返回 Err；可选 `instanceId`；失败清 `running`；`tun_device_error` → 前端重建 |
| A7 | **已修** | TUN 读/写失败发 `TunDeviceError`；sink 连续失败熔断；前端重建 |
| A8 | **已修** | `install_mobile_tun` 失败 `set_tun_device_error` → 前端重建；R7 另加瞬态 attach 原地重试 |
| A9 | **已修** | `NetworkCallback` + `setUnderlyingNetworks`；generation 暴露给 Rust watchdog；关 peer conn 触发 1s 重拨 |
| A10 | **已修** | `NicCtx::shutdown` + `drain/stop` 等待 JoinSet，避免 fd 复用 EEXIST |
| A11 | **已修** | `resolveVpnMtu(config)`：`config.mtu` 默认 1380，加密减 20 |
| A12 | **已修** | 去掉硬编码 `fd00::1/128`；**R3** 已加 `MyNodeInfo.virtual_ipv6` 并按实例下发 |
| A13 | **已修** | `Log.TAG`；`onStartCommand` try/catch；FGS start try/catch |
| A14 | **已修** | 瓦片按 `self` 实时状态切换；WebView 未就绪时 `dispatchTileAction` 返回 false |
| R4 | **已拍板** | 安卓 MTU 对齐桌面；蜂窝余量靠全网调 `mtu` |
| R7 | **已修** | 瞬态 attach 最多 3 次原地重试，再走前端重建 |

---

## 0. 结论摘要

按「现象可复现程度 + 影响面」排序。**A1 是本次审查中最确定、后果最重的一条**，其余为高概率/结构性问题。

| # | 级别 | 一句话 | 代码锚点 |
|---|------|--------|----------|
| A1 | **P0** | `TauriVpnService.self` 只在 `onCreate` 赋值；插件用 `onRevoke()` 当「内部停止」用，会把它置 null 且不再恢复 → 之后所有停止路径静默失效（不触发 `vpn_service_stop`、不关 PFD），前端 `waitVpnStatus` 超时抛错，状态与系统不一致 | `TauriVpnService.kt`、`VpnServicePlugin.kt`、`mobile_vpn.ts` |
| A2 | **P0** | 重复 `establish()` 不关旧 `vpnInterface` → 泄漏 tun fd；每次「重连」都多留一个打开着的 TUN | `TauriVpnService.kt` |
| A3 | **P0** | 安卓侧全部对账（reconcile / 10s 后台 sync）都跑在 **WebView 的 JS 定时器**里，Rust 侧无 watchdog；App 退后台 / Doze 时定时器被节流或冻结 → 孤儿 VPN、配置变更、实例启停都得不到收敛，「有时候」卡住只能靠把 App 拉回前台恢复 | `mobile_vpn.ts`；全仓无 Rust 侧定时器 |
| A4 | P1 | 孤儿 VPN：`START_STICKY` + 不处理 null intent → 进程被杀后系统用**默认参数**（`10.126.126.1/24`、MTU 1500、无路由/无 DNS）重建 TUN，而静态字段 `ipv4Addr` 是 null，与 `createVpnInterface` 的默认值不一致 | `TauriVpnService.kt` |
| A5 | P1 | 3s `waitVpnStatus` + 无重试：超时抛错但 VPN 实际已起 → `activeVpnInstanceId` 未落 → 下一轮判定「owner 变了」→ stop/start 抖动 | `mobile_vpn.ts` |
| A6 | P1 | `set_tun_fd` 静默丢 fd：取「第一个」已启用 TUN 实例、没有实例时直接跳过、`attach_fd` 用 `try_send`（接收端关闭即失败）→ 核心拿不到 TUN，但前端已把 `running` 置 true | `lib.rs`、`tun_mobile.rs`、`mobile_vpn.ts` |
| A7 | P1 | TUN 读流结束只 `notify_one()` 一个**没有任何消费方**的 `close_notifier`；`peers→nic` 的 sink 出错只 log 不退出 → 实例仍报「在线」，数据面已死 | `virtual_nic.rs` |
| A8 | P1 | `install_mobile_tun` 失败只 `tracing::error!`，无重试、无上报、无恢复 | `tun_mobile.rs` |
| A9 | P1 | 完全无网络切换处理（全仓无 `ConnectivityManager` / `setUnderlyingNetworks` / `NetworkCallback`）；Wi-Fi↔蜂窝 / 漫游后只能等 ping 失败阈值关连接再重连 | 全仓 grep 为空 |
| A10 | P1 | 重建 VPN 时旧 `AsyncFd` 可能尚未注销（`drain()` → drop `JoinSet` 是异步 abort），若 fd 号被复用 → `AsyncFd::new` epoll `ADD` 报 EEXIST → 新 fd 挂不上 | `tun_common.rs`、`virtual_nic.rs`、`tun_mobile.rs` |
| A11 | P2 | `mtu: 1300` 硬编码、忽略 `config.mtu`（默认 1380 / 加密后 1360），与其它节点不一致；用户把 mtu 调小（<1300）时安卓侧仍 1300 | `mobile_vpn.ts` |
| A12 | P2 | `addAddress("fd00::1", 128)` 硬编码，所有设备同一个 IPv6，且与实例实际 IPv6 无关 | `TauriVpnService.kt` |
| A13 | P2 | Kotlin 全用 `println`（无 TAG）排障困难；`onStartCommand` 里 `createVpnInterface` 抛异常会崩服务且无用户可见提示；`onDestroy/onRevoke` 里 `startForegroundService(MainForegroundService)` 无 try/catch（API 31+ 后台可能抛 `ForegroundServiceStartNotAllowedException`） | `TauriVpnService.kt` |
| A14 | P2 | 快捷开关瓦片用「上次动作」而不是真实状态决定 start/stop；`dispatchTileAction` 返回 true 时不会拉起 App，动作可能只落在 prefs 里没人消费 | `EasyTierVpnTileService.kt` |

---

## 1. 连接链路全景

### 1.1 修复前（审查时的 baseline）

下列标注（A1/A2/A6 等）对应 §2 里描述的**旧行为**，保留作对照：

```text
[用户点连接]
  … → doStartVpn → plugin startVpn({…, mtu:1300})
Kotlin: VpnServicePlugin.startVpn
          ├─ TauriVpnService.self?.onRevoke()        ← A1
          └─ startForegroundService(intent)
Kotlin: TauriVpnService.onStartCommand
          ├─ createVpnInterface(args).establish()    ← A2（旧 PFD 未关）
          └─ triggerCallback("vpn_service_start", {fd})
JS  : onVpnServiceStart → setTunFd(fd)               ← A6（失败被吞）
Rust: set_tun_fd → .next() 静默 Ok / try_send
```

### 1.2 修复后（当前工作区）

```text
[用户点连接]
  … → doStartVpn → resolveVpnMtu(config) → plugin startVpn({…, mtu})
Kotlin: VpnServicePlugin.startVpn
          ├─ prepare() 非 null → need_prepare（R5：先检查再 stop）
          ├─ TauriVpnService.stopInternal()           ← A1 已分离
          └─ startForegroundService(intent)
Kotlin: TauriVpnService.onStartCommand
          ├─ self = this；closeVpnInterface 再 establish   ← A1/A2
          ├─ START_NOT_STICKY；null intent → stopSelf      ← A4
          └─ triggerCallback("vpn_service_start", {fd})
JS  : onVpnServiceStart → setTunFd(fd, activeVpnInstanceId)
          └─ 失败 → 清 running（R1）
Rust: set_tun_fd(fd, instance_id?) → 无实例 Err（A6）
          └─ attach_fd try_send 明确 Err
Rust: install_mobile_tun 失败 → set_tun_device_error
          └─ manager emit tun_device_error → handleMobileTunDeviceError（R1）
                └─ transition 宽限 + 30s 节流 → doStopVpn + reconcile
```

对账入口（仍在 JS；A3 未改）：
- 事件驱动：`post_run_network_instance`、`dhcp_ip_changed`、`proxy_cidrs_updated`、`event_lagged`、`vpn_service_stop`、`tun_device_error`（Rust `manager.rs` emit）
- 定时驱动：`setInterval(10s)`（`BACKGROUND_VPN_SYNC_INTERVAL_MS`）
- 失败重试：`scheduleVpnReconcile`，2s 一次、最多 60 次（120s）

---

## 2. 缺陷详述

### A1（P0）`self` 语义被 `onRevoke()` 污染，停止路径永久失效

**代码**

```kotlin
// TauriVpnService.kt
override fun onCreate() { self = this }                     // :57-61  唯一赋值点
override fun onRevoke() {                                   // :73-81  系统回调
    disconnect(); setMainForegroundServiceEnabled(true)
    stopForeground(STOP_FOREGROUND_REMOVE); self = null; ...
}
private fun disconnect() {                                  // :83-89
    if (self == this && this::vpnInterface.isInitialized) {
        triggerCallback("vpn_service_stop", JSObject()); vpnInterface.close()
    }
    clearStatus()
}
```
```kotlin
// VpnServicePlugin.kt
fun startVpn(...) { TauriVpnService.self?.onRevoke(); ... }  // :101  「先停旧的」
fun stopVpn(...)  { TauriVpnService.self?.onRevoke(); activity.stopService(...) }  // :129
fun getVpnStatus(...) { ret.put("running", TauriVpnService.self != null) }         // :139
```

**机制**：`onRevoke()` 是 `VpnService` 的**系统回调**，被当成内部「stop」调用。它执行后 `self = null`，但 `onStartCommand`（服务实例仍然存活）**不会重新赋值 `self`**（只有 `onCreate` 会）。于是：

1. `startVpn` 在「服务还活着」时被调用一次（JS 认为没连、原生其实在连；或瓦片按 START），流程变成 `self=null → establish → vpn_service_start`。
2. 此后 `getVpnStatus.running` 恒为 `false`（`self != null` 判定），**但 VPN 实际在跑**；瓦片也显示 INACTIVE。
3. 之后任何 `stopVpn`：`self?.onRevoke()` 是空操作 → 只 `stopService` → `onDestroy` → `disconnect()` 因 `self != this` **既不触发 `vpn_service_stop` 也不关 PFD**。
4. 前端 `doStopVpn` 等 `curVpnStatus.running === false`，3s 超时 → `throw new Error('wait vpn status timeout')`（`mobile_vpn.ts:338`）。在 `reconcileNetworkInstance` 里这次抛错被 `catch` 掉（`:597-603`），于是继续走 `doStartVpn` —— 而 `doStartVpn` 开头是 `if (curVpnStatus.running) return`（`:346`），`running` 仍是 true → **直接返回，什么都不做**。
5. 结论：UI/状态认为「已连接」，原生 VPN 已死或残留，**用户表现为「连不上 / 卡死」**；只能靠 10s 后台 sync 里 `get_vpn_status` 把 `running` 刷成 false 才可能恢复，而该 sync 本身又受 A3 影响。

**建议**：`self = this` 移到 `onStartCommand`（或抽一个 `private fun markRunning()`）；把「内部停止」与系统回调分开（例如加 `stopInternal()` 只做 `disconnect + stopForeground`，`onRevoke()` = `stopInternal()` + `stopSelf()`）；`disconnect()` 的守卫改成「PFD 已初始化就关、就发事件」，不要绑在 `self` 上。

**验证**：`adb logcat | grep -E "vpn on (start command|revoke|destroy)|vpn_service_(start|stop)"`，观察 `start` 之后是否出现 `self` 语义错位（表现为 stop 时没有 `vpn on revoke` 日志）；Rust 侧看 `wait vpn status timeout` / `vpn service owner changed`。

---

### A2（P0）重复 `establish()` 不关旧 PFD

`onStartCommand` 每次都 `vpnInterface = createVpnInterface(args)`（`:46`），`disconnect()` 只关「当前字段」。任何「服务还活着又收到 start」的路径（A1 的第 1 步、瓦片重复点击、`startVpn` 重试）都会泄漏一个已 establish 的 tun fd。泄漏 fd 意味着该 tun 设备不会被系统回收，VPN 网络可能长期残留 —— 用户侧最直观的表现就是「关了 VPN 但系统钥匙图标还在 / 流量还被接管」。

**建议**：`onStartCommand` 开头 `if (this::vpnInterface.isInitialized) runCatching { vpnInterface.close() }`；或者干脆 `stopSelf()` 后重建服务。

---

### A3（P0）对账只活在 WebView，后台必失效

`mobile_vpn.ts` 是唯一的收敛器：孤儿 VPN 清理、配置变更（路由/DNS/IP 变化）、实例启停后重启 VPN，全靠 `setInterval(10s)` + `setTimeout` 链。Android 对后台 App 的 WebView 定时器有节流/冻结，Doze 下更彻底。**VPN 的常态恰恰是「App 在后台」**，所以：

- 进程被系统杀掉后服务被 `START_STICKY` 拉起（A4）→ 没有人消费 fd、没有人清理 → 系统显示「VPN 已连接」而实际无数据；
- 后台期间改了配置（Web 控制台改路由/关网络）→ 不收敛；
- 后台期间 VPN 被系统/其它 App 顶掉 → 不恢复。

**建议**：把「最小闭环」搬到 Rust：Rust 侧起一个 30s 级别的 watchdog，直接调 `get_vpn_status` 等价的原生查询 + 与实例状态对账，只把「需要 UI 决策」的部分留给 JS；或者让 Kotlin 直接 JNI 回调 Rust（不经过 WebView）。

---

### A4（P1）`START_STICKY` + 不处理 null intent → 孤儿 VPN

`onStartCommand` 返回 `START_STICKY`（`:54`），且 `args` 为 null 时直接落到默认值（`createVpnInterface` `:167-171`：`10.126.126.1/24`、MTU 1500、无 routes、无 DNS）。系统在进程被杀后用 **null intent** 重启服务时，会建立一个参数错误的 TUN；此时 `onCreate` 已执行（`self != null`）→ `get_vpn_status.running = true`，而静态 `ipv4Addr` 是 null（`:42` 直接取 `args?.getString`），与 `createVpnInterface` 用的默认值**不一致**，前端拿到的快照是自相矛盾的。

**建议**：改为 `START_NOT_STICKY`（VPN 需要用户显式动作）；若保留 sticky，则在 `args == null` 时直接 `stopSelf()` 而不是用默认参数建立 TUN；`ipv4Addr/routes/dns` 静态字段应写入 `createVpnInterface` 实际使用的值。

---

### A5（P1）3 秒超时 + 无重试 → 抖动

`doStartVpn` 里 `await waitVpnStatus(true, 3)`（`:373`）依赖 `vpn_service_start` 事件在 3s 内到达。慢机 / 首次 establish / 前台服务被延迟时容易超时；超时后异常被 `catch`（`:614-624`）只 `console.error`，**既不重试也不回滚**，且 `activeVpnInstanceId` 没写 → 下一轮 `stopVpnOwnedByOtherInstance` 认为「owner 变了」（`undefined !== instanceId`）→ 先 stop 再 start，出现反复拆建。

**建议**：改成事件驱动 + 明确的失败重试（例如 3 次、指数退避），超时后主动 `get_vpn_status` 复核一次再决定；`activeVpnInstanceId` 应在 `start_vpn` 返回后立即写，而不是等 `waitVpnStatus`。

---

### A6（P1）`set_tun_fd` 静默丢 fd

```rust
if let Some(uuid) = get_client_manager!()?.get_enabled_instances_with_tun_ids().next() {
    instance_manager.attach_tun_fd(uuid, fd)?;
}
Ok(())   // 没有实例时静默返回 Ok
```
- 取的是「第一个」已启用 TUN 实例，不保证是本次 VPN 对应的那个；
- 没有实例时静默成功（前端 `onVpnServiceStart` 只 `catch` 并 `console.error`，`running` 依然是 true）；
- `attach_fd` 用 `try_send`（`tun_mobile.rs:119-123`），接收端已被 `prepare()` take 且任务退出后即关闭 → 返回 `failed to send TUN fd`，同样只被 `console.error` 吞掉。

任一分支命中，都是「前端认为已连接、核心没有 TUN」。

**建议**：`set_tun_fd` 传入/校验目标 instance id（从 `vpn_service_start` 带上）；没有实例时返回 Err 让前端可见；`attach_fd` 改为带超时的 `send().await`，失败要上报事件而不是静默。

---

### A7（P1）TUN 死亡没有任何恢复路径

`TunStream::poll_next` 在 `len == 0`（EOF）或 `Err` 时返回 `Poll::Ready(None)`（`virtual_nic.rs:96-131`）—— 注意 `AsyncDevice::poll_read` 内部用 `try_io` 处理了 `WouldBlock`，所以**真正的错误（含 fd 被关闭后的 EBADF）会直接终止读流**。终止后：

```rust
while let Some(ret) = stream.next().await { ... }
close_notifier.notify_one();          // :975
tracing::error!("nic closed when recving from it");
```
而 `close_notifier` 全仓**没有任何 await 点**（只有 `virtual_nic.rs:874/895/966/984` 的赋值与 `notify_one`）。反方向的 `do_forward_peers_to_nic` 里 `sink.send()` 出错只 `tracing::error!` 后继续循环（`:993-996`）。

结果：实例继续报「在线」、Peer 连接照旧，但设备侧流量进不去也出不来。

**建议**：给 `NicCtx` 增加一个可被 runtime host 观察的「TUN 已死」信号，并让 `NativeTunRuntime` 在收到后走 `set_tun_device_error` + 上报事件，让前端能触发重建（而不是静默）。同时 `sink.send` 连续失败应计数熔断，而不是无限循环刷日志。

---

### A8（P1）`install_mobile_tun` 失败无重试

```rust
if let Err(error) = Self::install_mobile_tun(...).await {
    tracing::error!(?error, "failed to attach mobile TUN fd");
}
```
失败后 fd 被消费掉、实例没有 TUN、循环继续等下一个 fd。没有任何重试与上报。

---

### A9（P1）无网络切换处理

全仓 `ConnectivityManager` / `setUnderlyingNetworks` / `NetworkCallback` 均无命中。Wi-Fi↔蜂窝、Wi-Fi 漫游、热点切换后：

- 核心的 UDP 传输 socket 五元组失效，只能等对端 ping 失败累积（默认 `ping_fail_close_count = 5`，自适应间隔 1s~32s）才关连接再重连 → 表现为几十秒到数分钟的「卡断」；
- `disallowedApplications: ['com.kkrainbow.easytier']` 让 App 自身流量走物理网，因此切换后不会自动重绑定。

**建议**：在插件里注册 `registerDefaultNetworkCallback`，网络切换时 emit 事件给前端/Rust，主动触发一次「重建 connector / 重连」；同时按需 `setUnderlyingNetworks`。

---

### A10（P1）fd 复用 + 旧 `AsyncFd` 未注销 → EEXIST

`install_mobile_tun` 先 `nic_state.drain()`（`tun_common.rs:75-89`）：`stop()` 取走并 drop `NicCtxContainer` → drop `JoinSet` 触发 `abort_all()`。tokio 的 abort 是「标记取消 + 等调度器丢 future」，`AsyncFd` 的析构（epoll `DEL`）不保证在 `drain()` 返回前完成。而 Android 侧 `disconnect()` 关闭 PFD 后 `establish()` 很容易拿到**同一个 fd 号**。若旧的 epoll 注册还没摘掉，`AsyncFd::new` 的 `epoll_ctl(ADD)` 会返回 EEXIST → `failed to attach mobile TUN fd` → 新 TUN 挂不上（叠加 A8 的静默失败）。

**建议**：`drain()` 后显式等待旧 NIC 任务结束（JoinSet 里 join 一轮）再 install；或在 attach 前对 fd 做一次 `dup`（`AsyncDevice` 用 dup 出来的 fd，避免 fd 号复用与跨进程语义纠缠）。

---

### A11（P2）MTU 硬编码 1300

`mobile_vpn.ts:356` 固定 `mtu: 1300`，不读 `config.mtu`（core 默认 `mtu = 1380`，加密时桌面 TUN 取 1360）。用户在网络配置里调 mtu 对安卓端**完全不生效**：调小（如 1280）时安卓 TUN 仍是 1300 → 出向可能超路径 MTU；调大则拿不到收益。建议至少 `mtu: config.mtu`（并按加密开关扣减），与桌面 `virtual_nic.rs:735-742` 的算法保持一致。

### A12（P2）硬编码 `fd00::1/128`

`TauriVpnService.kt:180` 给每台设备的 TUN 都加 `fd00::1/128`。所有设备同一个 IPv6，且与实例真实 IPv6 无关；网络里一旦用到 IPv6（子网代理 / `fd00::/64` 路由），源地址选择会指向一个并非本节点持有的地址。建议由前端下发实例真实 IPv6（或干脆不下发 IPv6，交给 core 的 ipv6 配置）。

### A13（P2）Kotlin 健壮性

- `println` 全量替代 `Log.i(TAG, ...)`，logcat 里没有 tag 可过滤，线上排障成本高；
- `onStartCommand` 中 `createVpnInterface` 抛异常（`addRoute` CIDR 非法、`addDisallowedApplication` 包名不存在、`establish()` 返回 null）会直接崩掉服务，且没有任何用户可见反馈；
- `setMainForegroundServiceEnabled(true)` 在 `onDestroy/onRevoke` 中调用，API 31+ 后台启动前台服务有 `ForegroundServiceStartNotAllowedException` 风险，建议包 try/catch。

### A14（P2）快捷开关瓦片

`handleClick` 先读 `pendingAction(this)` 再回写同一动作（`:64-65`）：若上次动作因 App 未就绪没被消费，这次会**重放旧动作**而不是按真实状态切换；`dispatchTileAction` 只要回调存在就返回 true（`VpnServicePlugin.kt:41-46` 无条件 `true`），此时不会拉起 App，动作只躺在 prefs 里。建议以 `TauriVpnService.self != null`（修好 A1 之后即为可信状态）为唯一判据，并让 handler 在 WebView 未就绪时返回 false 以触发 `openApp()`。

---

## 3. 复现与确认方法

**日志采集**
- Kotlin 侧（建议先加 TAG）：`adb logcat -s TauriVpnService:* VpnServicePlugin:*`
- Rust 侧日志在 App 私有目录 `cache_dir/logs`（`lib.rs:897` 附近），默认 level `warn`；排障时用设置里的「日志」临时调到 `info`/`debug`
- 前端 console 走 WebView，可用 `adb logcat | grep -i chromium` 或桌面版 Chrome 远程调试 `chrome://inspect`

**要盯的关键串**

| 日志 | 含义 |
|------|------|
| `vpn service is not ready, retrying` + `reason` | 对账没拿到 virtual_ipv4 / network info |
| `start vpn service failed` | `doStartVpn` 抛错（A5/A1） |
| `wait vpn status timeout` | 没收到 `vpn_service_start/stop` 事件（A1/A5） |
| `vpn service owner changed` | owner 判定不一致 → 拆建（A1/A5） |
| `set tun fd failed` | `set_tun_fd` 失败（A6） |
| `failed to attach mobile TUN fd` | `install_mobile_tun` 失败（A8/A10） |
| `nic closed when recving from it` | TUN 读流终止（A7） |
| `vpn service reconcile stopped after maximum attempts` | 120s 内没收敛（A3/A4） |

**建议复现用例**
1. VPN 运行中 → 杀掉 Activity/让 WebView 重载 → 点「断开」→ 观察是否出现 `wait vpn status timeout`，以及系统 VPN 图标是否残留（A1/A2）。
2. VPN 运行中 → 退后台 10 分钟 → 从最近任务划掉 App → 再点快捷开关 START → 观察是否出现「连不上」（A3/A4）。
3. 反复点快捷开关 START/STOP 5 次 → `adb shell ls -l /proc/<pid>/fd | grep -c tun` 或观察 fd 数增长（A2）。
4. Wi-Fi 与移动数据互切 → 记录从切换到达通的时间（A9）。
5. 在网络配置里把 mtu 改成 1280 → 抓包确认 TUN MTU 是否为 1260（1380 默认减加密 20；A11 已修）。

---

## 4. 修复优先级建议

| 批次 | 内容 | 风险 | 说明 |
|------|------|------|------|
| 第 1 批（纯 Kotlin，改动小） | A1、A2、A4、A13 | 低 | **已完成** |
| 第 2 批（Rust 侧健壮性） | A6、A7、A8、A10 | 中 | **已完成**（`TunDeviceError` 上报；JoinSet 等待） |
| 第 3 批（架构） | A3（Rust watchdog）、A9（网络切换） | 中高 | **均已完成** |
| 第 4 批（配置一致性） | A5、A11、A12、A14 | 低 | **已完成** |

---

## 5. 待确认

1. 用户看到的具体现象是哪一类：①VPN 起不来（授权后没反应）；②起来了但完全没流量；③能上网但访问内网节点不稳定；④UI 显示已连接但系统 VPN 图标异常。
2. 能否提供一次现场 `logcat` + App 内 Rust 日志（尤其是出现 `wait vpn status timeout` / `failed to attach mobile TUN fd` 的那次）。
3. 复现环境：Android 版本、厂商 ROM、是否开了省电优化、是否 Wi-Fi/蜂窝切换后出现。

---

## 6. 修复复核（2026-10-07 晚，静态核对）

复核对象：工作区改动（`tauri-plugin-vpnservice/android/**`、`easytier/src/instance/**`、`easytier-gui/**`）。
**本地未编译**（本机 debug / release-fast 的 C 依赖 `cl.exe` 编不过，用户要求跳过），Rust 侧结论均为**读代码静态判定**，请以实际编译为准。

### 6.1 判定为已修好

| 缺陷 | 修法（锚点） | 评价 |
|------|--------------|------|
| A1 | `self = this` 移入 `onStartCommand`（`TauriVpnService.kt:61`）；新增 `stopInternal()` / `stopInternalLocked()` 与系统回调 `onRevoke()` 分离；`disconnect()` 不再拿 `self` 当守卫，改成「PFD 存在就关 + 发事件」 | 正解，语义干净 |
| A2 | `vpnInterface` 改可空；establish 前先 `closeVpnInterface(emitStopEvent = false)`（`:67`） | 正确，且不会多发一次 stop 事件 |
| A4 | `START_NOT_STICKY`；null intent/extras 直接 `stopSelf()`；静态字段改写入**实际生效值**（`:75-77`） | 正确 |
| A5 | `waitVpnStatus` 超时前先 `get_vpn_status` 复核（`mobile_vpn.ts:322-329`）；3s→8s；`doStartVpn` 先占 owner 再等事件并多次重申；`onVpnServiceStop` 不再清 owner | 正确；「复核 + 占 owner」正好互相兜底 |
| A6 | `set_tun_fd(fd, instance_id)`；无实例 Err；前端失败清 `running`；`tun_device_error` 重建 | 已闭环（R1） |
| A7 | 读流结束 / sink 连续 3 次失败 → `set_tun_device_error` + 熔断；前端重建 | 已闭环（R1/R6） |
| A8 | attach 失败 → `set_tun_device_error`；前端重建 | 已闭环（R1；R7 原地重试已加） |
| A11 | `resolveVpnMtu()` 对齐桌面 `flags.mtu - 20`（`mobile_vpn.ts:333-339`） | 正确；R4 已拍板保持全网一致 |
| A12 | 去掉 `addAddress("fd00::1", 128)` | 去掉是对的（原本就是错的），见 R3 |
| A13 | 全部换 `Log.i/w/e` + TAG；`createVpnInterface` 异常包 try/catch 并清理；`startForegroundService` 包 try/catch | 正确，见 R2 |
| A14 | 瓦片按 `self == null` 判定、不再复用 pending action；`tileActionReady` 让 WebView 未就绪时回退拉起 App | 正确 |
| A10 | `NicCtx::shutdown()`（abort_all + 排空 join）由 `stop()` 等待（`virtual_nic.rs:1024`、`tun_common.rs:78-86`） | 设计正确；B1 已修，待 CI 编译确认 |

### 6.2 曾阻塞项（静态判定；**已于 §6.6 修复**）

**B1. `easytier/src/instance/runtime_host/tun_common.rs:43-48` — E0382 use of moved value**

`Box::<dyn Any + Send>::downcast` 的签名是 `fn downcast<T: Any + Send>(self: Box<Self>) -> Result<Box<T>, Box<Self>>`，**按值消费** `boxed`：

```rust
let Some(boxed) = self.nic_ctx.take() else { return; };
if let Ok(nic) = boxed.downcast::<NicCtx>() {          // ← 这里已经 move 掉 boxed
    nic.shutdown().await;
} else if let Ok(mut tasks) = boxed.downcast::<JoinSet<()>>() {   // ← 使用已 move 的值
```

修法（用 `match` + `Err(boxed)` 重新绑定）：

```rust
async fn shutdown_tasks(&mut self) {
    let Some(boxed) = self.nic_ctx.take() else { return; };
    match boxed.downcast::<NicCtx>() {
        Ok(nic) => nic.shutdown().await,
        Err(boxed) => {
            if let Ok(mut tasks) = boxed.downcast::<JoinSet<()>>() {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
            }
        }
    }
}
```

**B2. `easytier-gui/src-tauri/src/lib.rs:260` — E0716 temporary value dropped while borrowed**

```rust
let mut ids = get_client_manager!()?.get_enabled_instances_with_tun_ids();
```

`get_client_manager!()` 展开出的 `MappedRwLockReadGuard` 是**临时值**（没有绑到名字上），而 `get_enabled_instances_with_tun_ids(&self) -> impl Iterator + '_` 借用了它；临时值在 `let` 语句结束即析构 → `ids` 后续（`:261` / `:265`）使用悬垂借用。

修法（把 borrow 收在一条语句里）：

```rust
let ids: Vec<uuid::Uuid> = get_client_manager!()?
    .get_enabled_instances_with_tun_ids()
    .collect();
let Some(first) = ids.first().copied() else {
    return Err("no enabled TUN instance to attach fd".to_string());
};
if ids.len() > 1 {
    tracing::warn!(%first, "multiple TUN instances enabled; attaching fd to the first one");
}
first
```

### 6.3 残留缺口 / 新引入风险

> **已关闭（§6.6）**：R1、R2、R5、R6、B1、B2 —— 下列保留原文作审查记录；当前状态见 §6.6。

<details>
<summary>R1–R2、R5–R6（已修，点击展开原文）</summary>

**R1（已修）** — `manager.rs` emit `tun_device_error`；`handleMobileTunDeviceError` + transition 宽限 / 30s 节流；`setTunFd` 失败清 `running`。

**R2（已修）** — catch 分支补 `setMainForegroundServiceEnabled(true)`。

**R5（已修）** — `prepare()` 提到 `stopInternal()` 之前。

**R6（已修）** — `set_tun_device_error` 不再清空 `tun_device_name`。

</details>

<details>
<summary>R3 / R4 / R7（已关单，点击展开原文）</summary>

**R3（已修）** — `MyNodeInfo.virtual_ipv6` + `doStartVpn` / Kotlin `addAddress` 下发真实 IPv6。

**R4（已拍板）** — 保持 A11：安卓对齐桌面 `flags.mtu - 20`（全网一致性）。移动侧余量变小（1360+加密+UDP/IP 可能 > 部分蜂窝 1400）时，应把**全网** `mtu` 调低（如 1320），不在安卓端单端硬编码。

**R7（已修）** — `install_mobile_tun` 对瞬态 attach 错误（EEXIST / busy）最多 3 次原地重试（50ms×attempt）；耗尽后仍 `set_tun_device_error` → 前端重建（R1）。`run_for_mobile` 中间失败不再提前 emit，避免重试被 GUI 重建打断。

</details>

### 6.4 已验证 / 未验证

- ✅ `vue-tsc --noEmit` → 0 错
- ✅ `vitest run` → 28 passed（`mobile_vpn.test.ts` 21 + `mobile_vpn_tile.test.ts` 7），含 ownership / tile / **TUN 错误恢复 4 例** / **A9 underlay 4 例**
- ✅ `eslint src/composables/mobile_vpn.test.ts` → 0 错（`mobile_vpn.ts` / `event.ts` 的告警为文件既有风格债，非本次引入）
- ✅ Kotlin 静态核对：无 `isInitialized` 残留、无已删 import 的引用、`stopInternalLocked` 的 private 跨 companion 访问合法、`@JvmStatic` 调用点正确
- ✅ Tauri 参数约定：`instanceId`（camelCase）→ `instance_id`（snake_case）与 `backend.ts` 既有约定一致（如 `update_network_config_state`）
- ✅ R6 复验：`global_ctx.rs:265-276` 的 `set_tun_device_error` 已保留 `tun_device_name`
- ⛔ 本地 Rust 编译：按用户要求跳过（本机 C 依赖编不过）；`tun_mobile.rs` 因 `cfg(mobile)` 本地也覆盖不到
- ⚠️ B1/B2 为静态判定，需在能编译的环境确认

### 6.5 下一步建议

1. **CI 编译确认** B1/B2 与 `cfg(mobile)` 路径（本地 MSVC 环境跳过）。
2. **现场验证**：§5 待确认的三项 + §3 复现用例，尤其 A3 后台 Doze、A9 Wi‑Fi↔蜂窝切换、R4 路径 MTU 场景。

### 6.6 复核跟进（同日，对照工作区）

文档 §6 写完后工作区又推进了一轮；静态再对一次：

| 项 | 状态 | 说明 |
|----|------|------|
| B1 | **已修** | `shutdown_tasks` 改为 `match` + `Err(boxed)` |
| B2 | **已修** | `set_tun_fd` 对 iterator `collect()` 在同一语句内 |
| R2 | **已修** | catch 分支补 `setMainForegroundServiceEnabled(true)` |
| R5 | **已修** | `prepare()` 检查提到 `stopInternal()` 之前 |
| R1 | **已修（闭环）** | `manager.rs:469-480` emit `tun_device_error`；`event.ts:112-118/129` 监听；`mobile_vpn.ts` `handleMobileTunDeviceError` → `doStopVpn(true)` + `onNetworkInstanceUpdate` 重建；`setTunFd` 失败清 `running` |
| R6 | **已修** | `set_tun_device_error` 不再清空 `tun_device_name`，保留给 `cleanup_tun_leftovers` |
| R3 | **已修** | `MyNodeInfo.virtual_ipv6` + `NodeSnapshot.ipv6_addr` + `doStartVpn`/`ipv6Addr`/Kotlin `addAddress` |
| R4 | **已拍板** | 保持桌面对齐的 `resolveVpnMtu`；蜂窝余量靠全网调低 `mtu`，不安卓单端硬编码 |
| R7 | **已修** | `tun_mobile.rs`：瞬态 attach 最多 3 次原地重试；失败再 `set_tun_device_error`；`run_for_mobile` 不再提前 emit |
| A3 | **已修（最小闭环）** | `android_vpn_watchdog.rs`：30s 探测 + 孤儿 `stop_vpn`；`notify_vpn_stop_if_no_tun` 同步停；前端听 `vpn_watchdog_tick`。后台「实例在 / VPN 不在」的自动拉起仍依赖 WebView 醒着时的 tick→JS（故意不做无虚拟 IP 的盲启动） |
| A9 | **已修** | Kotlin `registerDefaultNetworkCallback` + debounce + `setUnderlyingNetworks`；`underlayNetworkGeneration` 进 `get_vpn_status`；Rust watchdog / `notify_underlay_network_changed` 关 peer conn → ManualConnector 1s 重拨；JS 听 `default_network_changed` 快路径 |

**R1 的两道防抖（本次新增，避免修复本身变成重启风暴）**

| 机制 | 常量 | 作用 |
|------|------|------|
| transition 宽限窗 | `VPN_TRANSITION_GRACE_MS = 15000` | `doStartVpn` / `doStopVpn` 入口写入 `vpnTransitionDeadline`。正常换 TUN 时旧 fd 被主动关闭，core 必然报一次 `TunDeviceError` —— 这类「预期内」的错误必须被忽略，否则每次启动都会自触发一轮重建 |
| 重建节流 | `VPN_TUN_ERROR_REBUILD_INTERVAL_MS = 30000` | 坏 fd 会持续报错，无节流则无限抖动。节流用 `lastTunErrorRebuildAt > 0` 作哨兵，**避免启动后 30s 内的首个真实错误被吞掉** |

另有一道归属过滤：只有当 `instanceId` 等于 `activeVpnInstanceId` 或 `desiredVpnInstanceId` 时才动作，防止已停止的陈旧实例把 VPN 拉起来。
