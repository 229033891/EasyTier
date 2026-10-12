# 配置同步能力各端一致性核验（Per-end Consistency）

## Status

- Status: **Done**（核验完成；结论为「有意不统一」，无待决策缺口）
- 日期：2026-10-12（同日复核：修正「移动 GUI 缺 W」误判）
- 范围：删除权门控、W 权威与 prune、残留判定 / UI 在**各端**的实现与差异
- 归属：对 [`config-delete-authority.md`](./config-delete-authority.md)（谁可以删）+ [`local-config-residue.md`](./local-config-residue.md)（删完如何收敛）的**横向覆盖核验**
- 关联实现：`PersistentConfig`（`easytier-core/src/management/full/remote_client.rs`）、`HeartbeatResponse`（`easytier-proto/proto/web.proto`）、`GetConfigServerStatusResponse`（`easytier-proto/proto/api_manage.proto`）

---

## 1. 结论（先给答案）

**各端未统一，且差异是设计取舍，不是缺口。**

先把「配置同步」拆成**四件不同的事**，否则会越对越乱：

| # | 能力 | 本质 | 需要的端 |
|---|------|------|----------|
| ① | 删除权门控 | 判定「本地能不能硬删」 | **被托管设备**（有本地硬删入口时） |
| ② | W 权威 + prune | 用控制台列表收敛本地 **web 缓存**残留 | **被托管设备**且持有 GUI/`localStorage` 类缓存 |
| ③a | 控制台残留 UI | 中间态（设备有库无、库停用仍跑）徽章 + 清除/接管/忽略 | **权威方（Web 控制台）** |
| ③b | 设备幽灵提示 | 误标 user/legacy 的前 Web 行，提示清除（不静默删） | **持有 GUI 缓存的被托管设备**（可选体验） |

端分两类，**需求本就不同**——

- **被托管设备**：
  - **easytier-gui（桌面 + Android Tauri）**：同一套 `index.vue` → ①+②+③b 已闭环。
  - **FFI**：① 最严；**无** GUI web 缓存 → ②/③b 无对象（不是漏实现）。
  - **纯 JNI / iOS 宿主 App**：delete=停用（无 ① 硬删门）；无 GUI 缓存 → 无 ②。
- **控制端 / 权威方**（Web 控制台、ohrs 近场、部分移动宿主作控制器）：本身是权威或发起方，delete 语义即「停用」，**不需要** ① 拒绝门与 ② prune；③a 仅 Web 控制台需要。

因此「各端统一」不是目标；**目标是「同类端口径一致」**。核验结论：**同类端口径已一致，无实质缺口。**

---

## 2. 各端能力矩阵（核验基准）

> 核验日期 2026-10-12；文件行号易漂移，以锚点函数名为准。  
> ③ 拆成 ③a（控制台）/ ③b（设备幽灵）；`-` = 该端本就不需要。

| 端 | ① 删除权门控 | ② W 权威 + prune | ③a 控制台残留 UI | ③b 设备幽灵提示 | 配置存储介质 |
|----|-------------|-----------------|------------------|-----------------|--------------|
| **easytier-gui**（桌面 + **Android Tauri**） | ✅ `bound && source==Web`，fail-closed | ✅ **完整闭环**（`index.vue` `maybePruneWebConfigsFromConsoleW`，仅 `connected && valid`） | `-`（非权威方） | ✅ `maybePromptFormerWebGhosts` | `localStorage.networkList` + `GUIStorage` |
| **Web 控制台**（后端 + 前端） | ✅ **权威方**：删 `keep=false`，停用 `true` | ✅ **生产 W**（`session.rs`） | ✅ 徽章/清除/接管/忽略（仅 `hostSurface==='web-console'`） | `-` | 控制台 SQLite（+ 忽略集 localStorage） |
| **FFI** | ✅ **最严**：`active && (tracked ∪ source==Web)`，锁毒化 fail-closed | `-`（无 GUI web 缓存可 prune） | `-` | `-` | 宿主决定 |
| **Android JNI** | ❌ **刻意无门**：`delete` → ungated `stop_network_instance` | `-`（非 GUI；无 networkList） | `-` | `-` | 宿主 App |
| **iOS** | ❌ 同 JNI（`easytier_ios_delete_instance` → `stop`） | `-` | `-` | `-` | 宿主 App |
| **移动插件**（`tauri-plugin-vpnservice`） | `-`（只管 TUN/VPN 生命周期） | `-` | `-` | `-` | — |
| **`mobile_vpn*.ts`** | `-`（VPN/磁贴辅助；删门在 Tauri `remove_network_instance`） | `-`（**不**负责 prune；由 `index.vue` 做） | `-` | `-` | — |
| **OpenHarmony / ohrs** | ❌ **无门**：近场 stop（单实例/活跃校验） | `-`（不跑 config-server client；status 桩） | `-` | `-` | 自有 `config_repo` |
| **JS / Go / WASM** | ❌ | `-` | `-` | `-` | — |

图例：✅ 完整接入；❌ 未接入（有意）；`-` 该端职责外。

**一句话**：跑通「控制台生产 W → 客户端消费 W 做 prune」的是 **easytier-gui（含 Android）↔ Web 控制台**；其余端按角色取舍，不是漏做。

---

## 3. 逐项核验明细

### 3.1 删除权门控（①）——两套真门 + 两个无门端

| 端 | 判据 | 锚点 | 失败方向 |
|----|------|------|----------|
| easytier-gui | `bound && source == Web` | `easytier-gui/src-tauri/src/lib.rs` `local_delete_blocked_by_console` | fail-closed（除 `rpc_client_unavailable`） |
| FFI | `active && (id ∈ tracked ∪ source == Web)` | `easytier-contrib/easytier-ffi/src/config_server.rs` `tracks_any_managed_instance` | fail-closed（锁毒化 → 拒绝） |
| Android JNI / iOS | **无门**（`delete` = `stop_network_instance`） | `easytier-android-jni/src/network_api.rs`；iOS `lib.rs` | —（无硬删入口） |
| ohrs | **无门**（仅校验单实例/活跃） | `easytier-contrib/easytier-ohrs/src/nearby_management.rs` `delete_network_instance` | —（无硬删入口） |

**口径差异说明（有意，非 bug）**：

- GUI 与 FFI 判据**不完全相同**：FFI 多并了 `tracked`。原因：`stop_network_instance` 之后 id 会离开 tracked，且别处起（service / 另一 run 路径）的 Web 源实例不在 tracked —— 只看 `source` 会漏，只看 `tracked` 会漏，故取**并集**。此设计已在 `config-delete-authority.md` §G2 记录。
- 两处**错误码一致**：`delete_managed_by_console`（GUI 常量 + FFI `DELETE_MANAGED_BY_CONSOLE`），便于宿主分类。
- JNI / iOS **刻意绕开门**：历史语义中 `delete` 就是「本地停用」，注释明确「gated FFI delete 是给要 reject-on-managed 的宿主」。→ **JNI/iOS delete 永不受门控，等价于停用。**

### 3.2 W 权威 + prune（②）——gui 闭环（含 Android）

链路（全部锚点）：

```
控制台 SQLite（source==Web 全部 id）
  → session.rs  heartbeat_response_with_revision   填 W，成功才 valid=true
  → web.proto   HeartbeatResponse.web_owned_instance_ids_valid / _ids
  → web_client.rs  缓存 W；会话建立前 / 结束后 clear_web_owned_instance_ids()
  → process_rpc.rs / GUI lib.rs  get_config_server_status（!connected 强制 valid=false）
  → index.vue   maybePruneWebConfigsFromConsoleW（桌面与 Android 共用此页）
  → prune_web_configs.ts  id ∈ W 必留；source==web && id ∉ W 才删
```

**双层「会话边界」防御**（防 stale W 误 prune）：

- `web_client.rs`：每条连接建立前与 `session.wait()` 返回后都 `clear_web_owned_instance_ids()`（`web_client.rs:991/999`、`1041/1051` 两处路径），使旧 W 不跨会话存活。
- `process_rpc.rs` + GUI `lib.rs`：`!connected` 时把 `valid` 强制为 `false`、ids 置空（同进程与 service 两分支各一处）。
- 效果：`store_heartbeat_web_owned_instance_ids` 在 `valid=false` 时 early-return 保留旧值也**无害** —— 旧值在会话边界已被清空，且离线态被上层强制 `valid=false`。

**消费 W 的只有 `easytier-gui` 的 `index.vue`（桌面 + Android Tauri）。** 特别地：

- **不要**用 `mobile_vpn*.ts` 有无调 `pruneAbsentWebConfigs` 判断移动端是否接 ②——那些模块只管 VPN/磁贴；prune 在配置页 `index.vue`。
- FFI / JNI / iOS / ohrs：**无** `localStorage.networkList` 类 web 缓存 → ② 无对象。
- ohrs 的 `get_config_server_status` 是**恒 `enabled/connected=false` 桩**（`nearby_management.rs`），并**显式**填 `web_owned_instance_ids_valid:false` + 空 ids；语义上 **fail-safe（不 prune）**，不会误删。

### 3.3 残留判定 / UI（③a / ③b）

- `PersistentConfig::is_disabled()`（`remote_client.rs`，默认 `false`）真实实现：
  - `GUIConfig`（list 时镜像 `enabled_networks`）
  - web SQLite `Model`（直读 `disabled`）
- `db_inst_ids` / `db_disabled_inst_ids` 由 `handle_list_network_instance_ids` 一次 `All` 派生；**③a** 消费方仅 `RemoteManagement.vue`（`hostSurface==='web-console'`）。
- **③b** 幽灵提示在 `index.vue` `maybePromptFormerWebGhosts`（与 prune 同页，桌面/Android 共用）。

> 注意 `D`（`disabled_inst_ids` = storage − running 差集）与 `D′`（`db_disabled_inst_ids` = storage 自记 `disabled=true`）不可混用：Save 接管 disabled 行、设备仍跑 → id ∈ R ∧ ∈ D′ 但 ∉ D。「库停用仍在跑」徽章必须用 `D′`。详见 `local-config-residue.md` §2.1。

---

## 4. 曾误判：移动 GUI「缺 W」（已澄清）

**错误论证（已废弃）**：因 `mobile_vpn*.ts` 未调用 `pruneAbsentWebConfigs`，推断 Android 移动 GUI 未接 ②。

**事实**：

- Android Tauri 与桌面共用 `easytier-gui`，配置页同为 `index.vue`。
- `maybePruneWebConfigsFromConsoleW` / `maybePromptFormerWebGhosts` 已在该页；Android 启动时仍走 `refreshConfigServerConnection`（见 `index.vue` 内 `type() === 'android'` 分支旁的同一套连接刷新）。
- `mobile_vpn_tile.ts` 用 `running ∪ disabled_inst_ids` 组 `configuredIds` 仅服务磁贴启停候选，**不参与** web 缓存 prune，语义无损。

**仍须区分的两层「移动」**：

| 层 | 角色 | ① / ② |
|----|------|-------|
| easytier-gui Android（Tauri） | 被托管设备 + GUI 缓存 | ①+②+③b ✅ |
| Android JNI / iOS 原生宿主 | 常作控制端或 delete=停用 | 无硬删门；无 GUI prune 对象 |

---

## 5. 为什么不强行统一（设计取舍）

- **控制端不需要 prune**：JNI/iOS/ohrs 作「发起控制」时，本地没有「控制台权威 GUI 副本」，prune 无对象。
- **门控与服务端权威绑定**：门控防止本地硬删控制台负责的行。控制端没被托管时加门会阻断正常停用。
- **ohrs 是近场控制**：`get_config_server_status` 恒 false 桩准确反映「该端不跑 config-server client」，不是漏实现。
- **FFI 无 ②**：无 GUI web 缓存；门控（①）已覆盖「勿硬删托管行」。

因此**建议保持现状**，仅登记差异；不必为「全端同一套 prune UI」立项。

---

## 6. 核验清单（可复跑）

| # | 检查 | 期望 |
|---|------|------|
| 1 | `grep -rn "is_disabled()" easytier-core` / GUI / web entity | 调用在 `remote_client` list；实现在 `GUIConfig` + SQLite `Model` |
| 2 | `grep -rn "web_owned_instance_ids"` 全仓库 | 生产=`session.rs`；缓存=`web_client.rs`；透出=`process_rpc.rs` + GUI `lib.rs`；消费=`index.vue` |
| 3 | `grep -rn "pruneAbsentWebConfigs"` | 仅 `easytier-gui`（`backend.ts` / 测试）；由 `index.vue` 在桌面与 Android 触发 |
| 4 | ohrs `get_config_server_status` | 恒 `enabled/connected=false`；显式 `web_owned_*` invalid/空（fail-safe） |
| 5 | JNI `delete_network_instance_jni` | 路由到 `stop_network_instance`，无门 |
| 6 | FFI `tracks_any_managed_instance` | `tracked ∪ source==Web`，锁毒化 → `true` |
| 7 | `delete_managed_by_console` 常量 | GUI 与 FFI 同名（稳定错误码） |
| 8 | `db_disabled_inst_ids` 消费方 | 仅 `RemoteManagement.vue`（`web-console`） |
| 9 | 勿把 `mobile_vpn*.ts` 当 prune 入口 | prune / 幽灵在 `index.vue`，不在 VPN 辅助模块 |

---

## 7. 决策记录

| 项 | 结论 |
|----|------|
| GUI vs FFI 门判据不同 | **保留**（FFI 并 `tracked` 是必要加强，已文档化） |
| 移动 JNI / iOS 无门（delete=停用） | **保留**（历史语义 + 注释） |
| ohrs 无门 + status 桩 | **保留**（近场控制端） |
| easytier-gui Android 与桌面 | **同一套** ①+②+③b（`index.vue`）；非缺口 |
| 强行全端统一 | **不做**（控制端 / 无缓存端不需要，见 §5） |
| 2026-10-12 复核 | 撤销「移动 GUI 缺 W」误判；③ 拆为 ③a/③b；Status → Done |
