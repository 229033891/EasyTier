# 托盘图标的运行态语义（tray run state）

## Status

- Status: **In progress**（代码已落地并通过静态校验；**未真机验收**，故不写入 `current/`）
- 最近审阅：2026-10-18（源码逐行核对）
- 索引：[`../README.md`](../README.md)
- 范围：`easytier-gui` 桌面托盘（Windows / macOS / Linux）。移动端没有托盘，`refreshTrayRunStateInner`
  在 `type() === 'android'` 时短路，不产生探针 RPC。
- 关联：[`poll-to-push-roi.md`](./poll-to-push-roi.md)（本轮给 30s 轮询新增了探针成本）；
  「停服务 / 退出 / 关窗口分别停谁」见 [`../current/desktop-gui-and-config-server.md`](../current/desktop-gui-and-config-server.md) §1.1
  （托盘配置子菜单在服务停掉后整段置灰，正是该节的直接推论）。

## 问题

旧实现把「RPC 客户端是否连上内核」直接当运行态：

```ts
await setTrayRunState(clientRunning.value ? 'running' : 'idle')
```

`clientRunning` = `is_client_running()` = `rpc_manager.is_running()`，而 normal 模式在 `init_rpc_connection`
里**无条件**创建 instance manager 并连上客户端（`easytier-gui/src-tauri/src/lib.rs` 的 `is_normal_mode` 分支）。
后果：ET 一开着（或 service 模式服务在跑），托盘恒为绿色——**0 个网络实例时也是绿的**；灰色只在
「连不上内核」时出现。托盘唯一的价值是关窗后一眼看出「我现在在网里吗」，旧行为把这个信息位浪费在
「进程活着」上（图标存在本身已经表达了这件事）。

## 语义与配色

颜色回答「用户此刻该做什么」，不回答「内部哪个组件活着」。

| 状态 | 图标主色 | 含义 | 判据 | 用户动作 |
| --- | --- | --- | --- | --- |
| `idle` | 灰 `#9AA0A6` | 客户端在，但没有任何网络在跑（含全部被停用） | `clientRunning` 且无实例在跑 | 无 |
| `connecting` | 黄 `#FFCF56` | 启动 / 重连 / handshake 中 | `beginTrayConnecting` 嵌套 hold（`trayConnectingHold`） | 等待 |
| `running` | 绿 `#299C46` | 至少一个网络在跑 | 存在 `running !== false` 的实例 | 无 |
| `error` | 红 `#D64545` | 需要介入 | 见下 | 检查配置 / 服务 / 链路 |

`error` 的两种来源（tooltip 区分，颜色不区分）：

1. **内核或服务不可达**，且不属于「刻意停用」：
   - `normal` 模式：GUI 自带内核，连不上就是故障；
   - `remote` 模式：远端不可达 → 本机什么都管不了，红（tooltip 写明是对端/链路问题）；
   - `service` 模式：仅当服务状态为 `Running` 时算故障；`Stopped` / `NotInstalled` 是托盘菜单自己
     就能制造的状态，显灰——否则「停止服务」之后会永远报红。判据函数：`isRunFault(mode, serviceStatus)`。
2. **实例启动失败** —— `running === false` 且 `error_msg` 非空。任意一个失败即红（即使别的实例正常），
   因为这是唯一能把它暴露给用户的信号。

## 判据来源

- **L0**：`is_client_running()`（`easytier-gui/src-tauri/src/lib.rs`）。
- **L1/L2**：`list_network_instance_ids()` 的 `running_inst_ids`，再对每个 id 取
  `collect_network_info` 的 `running` / `error_msg`。
  - **不能只看 `running_inst_ids` 的长度**：它是内核实例表全量（`instance/manager.rs::instance_ids()`
    = `list()` 的 key），**包含启动失败后仍留在表里的实例**；心跳侧专门减掉 failed 才敢叫 running
    （`web_client.rs::running_instances_for_heartbeat`），GUI 这条 RPC 没有减。
  - **成本**：0 个实例时只有 1 次 list、0 次探针；N 个实例时 N 次 `collect_network_info`
    （上限 `TRAY_PROBE_MAX = 8`，超出只影响 tooltip 计数，不影响绿/灰）。频率 = 30s fallback +
    事件 + 变更后 500ms 去抖。
  - **探针失败 = 未知，不是「没在跑」**：未知快照计入 running，避免把一次 RPC 抖动显示成灰；
    list 整体失败时保持上一次图标（不臆造灰色）。
- **聚合规则**：纯函数 `easytier-gui/src/composables/tray_run_state.ts::deriveTrayRunState`。
  优先级：`clientRunning === false` > 实例失败 > 有实例在跑 > 空闲。纯函数不引入 Tauri 依赖，可直接单测。

## 刷新时机

- 30s fallback 轮询 + `client_rpc_state_changed` / `post_run_network_instance` / `vpn_service_stop` 事件
  （`pages/index.vue::refreshClientRunningAndTray`）。
- GUI 自身发起的启动 / 停止 / 删除由 `GUIRemoteClient` 调 `requestTrayRunStateRefresh()` 主动触发
  （`modules/api.ts`），不必等 30s。
- **控制台侧**（配置服务器下发、远端控制台停用/启用）没有事件到 GUI，最长 30s 后才反映。

## 剩余缺口 / 验收清单

未完成，因此本文停在 Roadmap 层：

- [ ] **真机矩阵**（normal 模式）：0 实例 → 灰；1 个可用网络 → 绿；故意写坏一个配置使其启动失败 → 红；
      启动/重连瞬间 → 黄。
- [ ] **service 模式**：停止服务 → 灰（不是红）；服务 `Running` 但 RPC 不通 → 红。
- [ ] **观感定稿**：16px + 深色任务栏下的四色可辨识度（尤其与「连接中」黄的区分）；色盲场景目前只能靠
      tooltip，是否加角标/形状差异待定。
- [ ] 「在跑但一个 peer 都没连上」目前仍显绿：4 色方案下的刻意取舍；真实跑一段时间后再决定是否加
      橙色 `degraded`（对应 `is_closed` 全为 true 且超滞后窗口）。
- [ ] 复查 `poll-to-push-roi.md` 记的 BidirectRpc 余量：新增探针是否触发状态页/心跳竞争。

## 已知边界（有意为之）

- **与窗口内实例下拉的「运行中」标签可能不一致（托盘更严格，且托盘是对的）**：`RemoteManagement`
  的绿/红 tag 直接用 `running_inst_ids` 判断，而那也是「实例表里有这个 id」，包含启动失败后仍留在表里、
  或启动被取消（`RecoveryGuard` → `stop()`，`latest_error` 可能为空）的实例。托盘改成逐实例探
  `running` / `error_msg`，因此**看到窗口说「运行中」而托盘是灰时，以托盘为准**。要彻底一致，需要让
  GUI 侧的状态标签也走逐实例真值（或让 `list_network_instance_ids` 不再把非就绪实例算作 running）——
  不在本次范围。
- 颜色是唯一信号，形状未变：色盲用户只能靠 tooltip。
- 图标是**单色平面 logo**（三个旧图标的 alpha 剪影逐像素相同），所以「换状态」= 换颜色。重新生成用
  `script/gen-tray-icons.py`（4 态 × 7 尺寸 16/24/32/48/64/128/256，PNG payload），**不要手改 `.ico`**。
  顺带修掉了 `tray-running.ico` 只有 256 单尺寸、被 Windows 缩放渲染的问题。新图标必须登记在
  `easytier-gui/src-tauri/tauri.conf.json` 的 `bundle.resources`，否则装包后丢图标。
- `icons/icon-inactive.ico` 全仓库无引用（与旧 idle 同图），保持原样未动。

## 验证（已跑口径）

```powershell
# 状态机：12 例，覆盖优先级与边界
cd easytier-gui; node node_modules/vitest/vitest.mjs run            # 5 files / 54 tests passed
# 图标是否与生成器一致
python script/gen-tray-icons.py --check
# 类型 / lint
cd easytier-gui; node node_modules/vue-tsc/bin/vue-tsc.js --noEmit   # exit 0
node node_modules/eslint/bin/eslint.js src/pages/index.vue src/composables/tray.ts `
  src/composables/tray_run_state.ts src/composables/tray_run_state.test.ts src/modules/api.ts  # exit 0
# 前端集成（含新 i18n 文案）
cd easytier-web/frontend-lib; node node_modules/vite/bin/vite.js build   # exit 0
cd easytier-gui; node node_modules/vite/bin/vite.js build                # exit 0
```
