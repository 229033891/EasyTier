# 消息弹窗（toast / confirm）审查与收敛

## Status

- Status: **Current**（审查 + 三批收敛已落地）
- 日期：2026-10-18
- 最近审阅：2026-10-18（第四批：顺带清理 —— 修复 `RemoteManagement.vue` 被写坏的 31 行注释、`showMainWindow()` 改 best-effort）
- 范围：**面向用户的弹出消息**（PrimeVue `toast.add`、`confirm.require`、原生 `confirm/alert`、`<Toast>`/`<ConfirmDialog>` 挂载）。不含表单类 `<Dialog>`（配置编辑、DNS/ACL 编辑等）本身
- 覆盖端：`easytier-gui`（桌面 + 安卓）、`easytier-web/frontend-lib`（共享库，两端复用）、`easytier-web/frontend`（Web 控制台）、`easytier-web/config-generator`（Magisk/浏览器配置生成器）
- 不在范围：消息**文案本身**的翻译质量（除 `t('error')` 这类键缺失缺陷）、后端日志、CLI 输出
- 相关：[`console-and-client-interaction-performance.md`](./console-and-client-interaction-performance.md)（交互现状）、[`windows-wintun-interface-missing.md`](./windows-wintun-interface-missing.md)（本轮同时收敛的 Windows 日志噪音）
- 索引：[`../README.md`](../README.md)

> 行号为撰写时锚点，可能漂移；以函数名 / i18n key 为准。

---

## 1. 一句话结论

弹窗机制**没有失控，但缺少统一策略**：

1. **机制已统一**（值得保留）：全部消息走 PrimeVue `toast.add` / `confirm.require`；每个应用只在 `App.vue` 挂载**一处** `<Toast>` + `<ConfirmDialog>`，无重复容器；生命周期常量 `TOAST_LIFE` 已是共享库导出（`modules/toast.ts`）。
2. **真正的问题是「重复」与「重复频率」**，不是数量：
   - **同一事实两处播报**：错误同时 toast + 内联（首屏加载失败）、同一 promise 的多个 `catch` 各弹一条、同一失败 toast + confirm 复读错误原文、复制成功既改按钮文案又弹 toast、库与宿主对同一事件各弹一次。
   - **在轮询循环里无节流播报**：5s 轮询失败**每轮一条** error（`Dashboard`、`DeviceManagement`），而 `usePollingList` / `deviceArchive` 早已各自手写 30s 窗口——同一策略被抄了两遍，另外两处漏抄。
3. **确实缺的确认**只有少数几处，且都是**不可逆**操作：重置他人密码（无确认）、配置生成器「保存并重启」（`ConfirmDialog` 已挂载却零调用）。
   - 清空日志曾按本项补过 confirm（复用当时零引用的 `logging_clear_confirm`），后按需求**撤除**：入口本身就是 `severity="danger"` 的「删除」按钮，再弹一层只是把同一句话问第二遍；文案 key 一并删除。
4. **顺带发现一个真实可见缺陷**：`t('error')` 在 cn/en 两个 locale 里**不存在**（只有 `web.common.error`），共 19 处 toast 标题实际渲染成字面量 `error`。

---

## 2. 盘点方法与规模

- 机器提取 4 个前端目录下 `toast.add` / `confirm.require` / `dialog.open` 调用点（含所属函数），再逐文件人工复核触发路径。
- 规模：**约 180 个消息点**（toast ≈147 / confirm ≈32 / dialog ≈16），分布：

| 区域 | 消息点 | 备注 |
|------|--------|------|
| `frontend-lib/src/components/RemoteManagement.vue` | 31 | 设备/网络管理主界面，两端共用 |
| `easytier-gui/src/pages/index.vue` | 27 | 托盘 + 服务 + 模式切换 |
| `frontend-lib/src/components/LoggingSettingsDialog.vue` | 13 | 日志清理/复制/级别 |
| `easytier-web/frontend/src/components/DeviceList.vue` | 13 | 设备列表（合并/退役/显示名） |
| `UserList.vue` / `ConfigTokenList.vue` | 11 / 11 | 用户与接入密钥 CRUD |
| 其余（Dashboard / RuntimeLogs / SystemDiagnostics / Login / config-generator / 其它） | ~70 | — |

- severity 分布：error 77 / success 23 / **secondary 18** / warn 16 / info 4；生命周期 114 处使用 `TOAST_LIFE.*`。

### 2.1 其它端的盘点结论（不纳入本轮统一改造）

| 端 | 结论 |
|----|------|
| LuCI（`luci-app-easytier`） | **不在本仓库**（同级仓库，其自带 `docs/web-console-performance-audit.md`），本次未覆盖 |
| `easytier-contrib/easytier-uptime/frontend`（element-plus，独立小工具） | 26 处：`ElMessage` 提示 + 4 处 `ElMessageBox.confirm`（审批通过/撤销审批/删除节点/退出登录——**破坏性操作确认齐备**，质量好于主前端）。问题只有「硬编码中文、无 i18n、无统一时长策略」；与主前端技术栈/部署形态不同，**未纳入本轮**，如需统一应单独立项 |
| `easytier-js/examples/web` | 无消息弹窗（纯示例） |
| `tauri-plugin-vpnservice`（Android 插件层） | 无 `Toast`/`AlertDialog`（由前端统一提示） |
| `easytier-web/config-generator` | 6 处，已纳入本轮（补了「保存并重启」确认） |

**判定准则**（本文所有「冗余 / 不必要」都按此口径，可复核）：

| 判定 | 含义 |
|------|------|
| 冗余 | 同一事实在同一时刻第二次呈现（toast + 内联、两条 catch、toast + confirm、按钮态 + toast） |
| 不必要 | 低价值或无受众（不可达分支、窗口不可见时弹、纯 no-op 成功提示） |
| 建议降级 | 可恢复的本地/瞬态问题用了 error(10s) 重级别 |
| 缺失 | 静默失败、破坏性无确认、>3s 操作无进度 |

---

## 3. 冗余 / 不必要（清单）

### 3.1 同一事实两处播报

| 位置 | 事实 | 重复对象 |
|------|------|----------|
| `easytier-web/frontend/src/components/SystemDiagnostics.vue`（原 :32 toast） | 诊断加载失败 | 同文件 :164 内联错误块（`loadError && !data`）：失败时 `data=undefined`，内联**必然**渲染同文案 + detail + 重试 |
| `easytier-web/frontend/src/components/RuntimeLogs.vue:175` | 起止时间非法（纯前端校验） | 同函数 :173 写入 `loadError`，模板 :397 内联展示 |
| `frontend-lib/.../RemoteManagement.vue:766 / :846 / :1015` | 同一 `loadCurrentNetworkConfig` 被拒 | 三处 `catch` 挂在**同一个** promise（去重在 :867-869），config 模式下同一次失败弹 2~3 条 error |
| `RemoteManagement.vue:1047`（confirm message = `errorDetail + hint`） | 删除网络失败 | :1081 toast 已完整展示同一 errorDetail → 一次失败读两遍 |
| `easytier-gui/src/pages/index.vue`（原 :567） | 模式保存时 RPC 连接失败 | `initWithMode` 已先带上下文弹 `mode.rpc_connection_failed` 再 throw |
| `config-generator/src/App.vue:205` | 复制成功 | 按钮文案同时切成 `config_copied`（:55） |
| `easytier-gui/src/pages/index.vue:1192` ↔ `RemoteManagement.vue:1751` | `managed_config_synced_overwrite`（控制台覆盖本地配置） | 库内 confirm + 宿主 info toast（脏稿时同屏出现） |

### 3.2 在轮询/循环里无节流

| 位置 | 周期 | 现状 |
|------|------|------|
| `easytier-web/frontend/src/components/Dashboard.vue:27` | 5s | 每轮失败弹一条 error，**无任何窗口** |
| `easytier-web/frontend/src/components/DeviceManagement.vue:109` | 5s | 同上。注（第五批更正）：此处**不会**与 `deviceArchive` 的告警叠条 —— `fetchDeviceArchive` 自己 `catch` 并返回 `lastArchive`，`loadMergedDevices` 不会因它 reject；只有 `list_machines` 失败才走到这里。两者同时失败时是两条**不同事实**（沿用旧快照 / 列表加载失败），不属于重复 |
| `frontend-lib/.../LoggingSettingsDialog.vue:90` | 每次切文件/刷新 | 每条失败各弹 10s error，无窗口 |
| 对照（正确范例） | — | `frontend/src/modules/usePollingList.ts:15` 与 `deviceArchive.ts:10` 各自手写 30s 窗口 |

### 3.3 不可达 / 低价值

| 位置 | 判定理由 |
|------|----------|
| `RemoteManagement.vue:1441` / `:1510`（`no_network_selected`） | 入口按钮分别 `v-if="showCombinedEditEntry"` / `v-if="selectedInstanceId"`，成立时不可能为空 |
| `easytier-web/frontend/src/components/DeviceList.vue:132/217/267`、`UserList.vue:116` | 对应按钮已 `disabled`/`hidden` 且 tooltip 同文案，仅 5s 轮询翻转的窄竞态可达 —— **建议保留判断、去掉 toast**（见 §6） |
| `easytier-gui/src/pages/index.vue:1277`（VPN 磁贴成功 toast） | 磁贴由系统快捷设置触发，WebView 不可见；有效反馈是系统 VPN 图标，回前台还会补弹一次 |

### 3.4 语义 / 键缺陷（顺带修复类）

| 位置 | 问题 |
|------|------|
| `LoggingSettingsDialog.vue` 7 处 + `easytier-gui/src/pages/index.vue` 12 处 | `summary: t('error')` —— locale 里**没有**根级 `error` 键（只有 `web.common.error`），实际渲染字面量 `error` |
| `LoggingSettingsDialog.vue:147/167/185` | `detail: String(e)`，与库内 `formatApiErrorDetail(e, t)` 两套口径 |
| `RemoteManagement.vue:1456` | 「进入编辑/读取配置失败」却复用 `save_failed`（保存失败）文案 |
| `RemoteManagement.vue:1332` | summary 用 `web.common.success`、正文说失败，且 warn 用了 `TOAST_LIFE.error` |

---

## 4. 缺失的反馈（分级）

### 4.1 不可逆操作缺二次确认（已修 / 待决策）

| 位置 | 现状 | 处理 |
|------|------|------|
| `easytier-web/frontend/src/components/UserList.vue` 重置用户密码 | 输入新密码点一下即生效，旧密码立即失效 | **已加 confirm**（复用 `reset_confirm`，新增中英文案） |
| `frontend-lib/.../LoggingSettingsDialog.vue` 清空日志 | 直接执行；`logging_clear_confirm` 文案曾存在但全仓零引用 | 曾加 confirm（复用现成 key），**现已撤除**：入口已是 danger「删除」按钮，二次确认属重复提问；key 已删 |
| `config-generator/src/App.vue:174-184` 「保存并重启」 | 一键重启（断流）；`main.ts` 已装 `ConfirmationService`、`App.vue:215` 已挂 `<ConfirmDialog>`，却零调用 | **已加 confirm**（`magisk_save_restart_confirm`）；⚠️ 包装 Promise 时必须补 `onHide`，见 §5.1 |
| `RemoteManagement.vue:2017` 放弃修改（丢草稿） | 无确认 | **已加 confirm**（`confirm_discard_changes`） |
| `easytier-gui` 托盘退出（service 模式 = 断网） | 无确认 | **已加 confirm**（`exit_stops_service_confirm`）；确认前先 `showMainWindow()`，否则窗口隐藏时看不到对话框；同样需 `onHide` 落定 |

### 4.2 静默失败（用户看不到任何反馈）

| 位置 | 场景 | 影响 |
|------|------|------|
| `RemoteManagement.vue:1730-1732` | 5s 轮询全失败仅 `console.debug` | **中高**：状态面板继续显示上次数据，用户以为是最新 → **已修**（常驻 `pollStale` 条 + 30s 限流 toast） |
| `RemoteManagement.vue:1754-1756` | 收到覆盖事件后干净态重载失败 | **中高**：在过期配置上编辑并保存，会反向覆盖控制台刚推送的配置 → **已修**（该路径现按交互式加载播报一次） |
| `RemoteManagement.vue:438-443` | `get_network_metas` 失败只标记 `networkMetaLoadFailed` | 中：Save 按钮变灰但原因只能悬停 tooltip 才看到 |
| `easytier-gui/src/pages/index.vue:574-579` | 保存失败后**回滚也失败**仅 console | 中：用户以为已回滚，实际半新半旧 |
| `easytier-gui/src/pages/index.vue:1206-1219` | 启动 `initWithMode` 抛错后 `onMounted` 其余步骤被跳过（无 catch） | 中：30s config-server 兜底轮询与事件监听**永不注册** |
| `easytier-gui/src-tauri/src/lib.rs:1498-1531` | 提权失败 `.expect()` → panic → release 无控制台 → 表现为「闪退」 | 高（结构性缺口，需真机确认） |
| `frontend-lib/.../Status.vue:503/631` | 复制失败仅 console | 低 |
| `easytier-web/frontend/src/modules/api.ts:297` | `console.log('login response:', response)` 打印登录响应 | 低（安全卫生） |

### 4.3 长操作无进度（>3s）

`RemoteManagement.vue` 的 `doSaveNetworkConfig`（Save 按钮只有 `disabled`，无 `:loading`）、`saveAndRunNewNetwork`、`doDeleteNetwork`、`doStartNetwork` / `doStopNetwork`、`editNetwork`；`ConfigEditDialog.vue:115`；`easyter-gui` 的服务启停/重启（托盘触发时 5~10s 无反馈）。

### 4.4 「窗口隐藏 = 所有 toast 不可见」这一结构性缺口

`easytier-gui/src-tauri/src/lib.rs:1666-1670` 关窗即隐藏到托盘，但消息仍走窗口内 toast/confirm —— 托盘触发的操作（启停服务、退出、VPN 磁贴）在窗口隐藏时**用户完全看不到反馈**。这是本项目最容易踩、也最需要产品决策的一点（是否引入原生通知 / 强制开窗），本文列为阶段 2。

---

## 5. 已落地改动（按批次累加，风险可控）

原则：**只做语义等价或严格减少打扰的改动**；不重写 180 个调用点，不动消息文案本身的分工，不新增依赖。

| # | 改动 | 文件 |
|---|------|------|
| 1 | 新增共享**重复提示门控**（纯函数、可单测）：`TOAST_REPEAT_COOLDOWN_MS`(30s) + `createToastRepeatGate()` + `addThrottledToast()`，并导出到库入口 | `frontend-lib/src/modules/toast.ts`、`easytier-frontend-lib.ts` |
| 2 | 两处**手写 30s 窗口**改用共享门控（行为等价，去掉两套实现） | `frontend/src/modules/usePollingList.ts`、`modules/deviceArchive.ts` |
| 3 | 两处**无节流的 5s 轮询**改为：首错即刻提示、持续失败按 30s 合并 | `frontend/src/components/Dashboard.vue`、`components/DeviceManagement.vue` |
| 4 | 删除与内联错误**必然重复**的 toast | `frontend/src/components/SystemDiagnostics.vue` |
| 5 | 纯前端校验的 toast 仅在**内联不可见**时发（已有日志行时） | `frontend/src/components/RuntimeLogs.vue` |
| 6 | 重置用户密码补 `confirm.require`（danger），并写清「旧密码立即失效」 | `frontend/src/components/UserList.vue` + 中英文案 `reset_confirm` |
| 7 | ~~清空日志补 `confirm.require`~~ — **已撤回**，恢复为点击即清空（`danger` 按钮 + `isClearing` 双击守卫已足够） | `frontend-lib/src/components/LoggingSettingsDialog.vue`、中英文案删除 `logging_clear_confirm` |
| 8 | `initWithMode` 已带上下文报错时，`onModeSave` 兜底 catch 不再重复弹（仍保留回滚与日志） | `easytier-gui/src/pages/index.vue` |
| 9 | 修复 `t('error')` 键缺失：19 处改用 `web.common.error` | `LoggingSettingsDialog.vue`、`easytier-gui/src/pages/index.vue` |
| 10 | 单测覆盖门控语义（首报放行 / 窗口内抑制 / 按 key 独立 / reset） | `frontend-lib/tests/toast-repeat-gate.spec.ts` |
| 11 | **共享库重复 catch 收敛**：`loadCurrentNetworkConfig` 每次加载尝试只播报一次（只有交互触发的加载才播报，轮询保持静默）；新增 `wasConfigLoadFailureReported` 让包装多个 loader 的 `catch` 仍能为其它 loader 兜底 | `frontend-lib/.../RemoteManagement.vue` |
| 12 | 修正错误文案：进入编辑读取配置失败不再复用「保存失败」 | 同上（`editNetwork`） |
| 13 | 语义修正：保存成功但自动启动未对齐的提示由「成功 + error 颜色/错误时长」改为 `warning` + `TOAST_LIFE.warn` | 同上 |
| 14 | **列表加载失败不再伪装成空态**：`ConfigTokenList` / `UserList` 接 `ListPageShell` 的 `:error` + `#error` + `#error-actions`（重试），并去掉重复 toast | `frontend/src/components/{ConfigTokenList,UserList}.vue` + `web.common.load_failed` / `web.common.retry` 文案 |
| 15 | **轮询失败常驻内联提示**：`RemoteManagement` 的 5s 轮询失败置位 `pollStale`，在面板顶部常驻「状态数据可能已过期 + 重试」，成功自动清除；同时按 30s 窗口最多补一条 warn toast（原来完全静默） | `frontend-lib/.../RemoteManagement.vue` + `data_stale` 文案 |
| 16 | **补齐不可逆操作确认**：配置生成器「保存并重启」（复用已挂载的 `ConfirmDialog`）、`RemoteManagement`「取消编辑/放弃修改」、GUI service 模式退出（会先停服务断网；确认前先 `showMainWindow()`，否则托盘退出看不到对话框） | `config-generator/src/App.vue`、`RemoteManagement.vue`、`easytier-gui/src/pages/index.vue` + 中英文案 |
| 17 | **拆分「操作失败」与「操作后刷新失败」**：`saveAndRunNewNetwork` / `doSaveNetworkConfig` 里保存/启动成功之后的刷新（meta/list/info）不再被报成 `start_failed` / `save_failed`，改为 `console.warn` + 继续走成功路径（原来会 `return` 并谎报失败） | `frontend-lib/.../RemoteManagement.vue` |
| 18 | 单测补充：门控用例覆盖「同窗口抑制 + 跨 key 独立 + 边界时刻放行」 | `frontend-lib/tests/toast-repeat-gate.spec.ts` |
| 19 | **修复 review 发现的真 bug：Promise 化的 confirm 在「关闭」时永久挂起** —— `ConfirmDialog` 的 X / Esc 只走 `Dialog.close() → onHide()`，**不会**调用 `reject`，所以第 16 项的两处 `await` 永不落定（「保存并重启」静默不重启；托盘退出点了没反应） | `config-generator/src/App.vue`、`easytier-gui/src/pages/index.vue` |
| 20 | `UserList.loadUsers` 自带 `catch`：重试失败会**刷新** `loadError`（原来只在 `onMounted` 里赋值，重试再失败会留着上一次文案）；并把**已零引用**的 `web.users.load_failed` 复用为内联标题 | `frontend/src/components/UserList.vue` |
| 21 | `saveAndRunNewNetwork`：刷新失败时**在 `catch` 里补一次 `setSelectedInstanceId`**（成功路径顺序完全不变）。原来 `loadNetworkMetas` 一抛错，刚启动的网络就不会被选中且只有 `console.warn` | `frontend-lib/.../RemoteManagement.vue` |
| 22 | 失败播报去重覆盖**非对象 rejection**：新增 `lastReportedConfigLoadError`（`WeakSet` 存不下字符串/`null`）。实测旧逻辑下一次字符串 rejection 会弹 **3 条**（共享路径 1 + 两个调用方兜底各 1） | 同上 |
| 23 | 共享加载尝试记录 `settled` + `reported`：在「已 reject 但条目还没清」的极窄窗口内加入的调用方也能拿到一次播报。`reported` **只在真正播报之后**才置位 —— 否则轮询发起的尝试（`reportFailure: false`）失败时会把额度用掉，随后加入的交互调用方反而被自己的守卫挡住（第五批修正） | 同上 |
| 24 | `RuntimeLogs` 两处失败分支统一为**同一条判定**「内联可见就只写内联，不可见才 toast」，顺带去掉 `!silent && lines.length === 0` 时内联与 toast 同时出现的重复 | `frontend/src/components/RuntimeLogs.vue` |
| 25 | 细节：`RemoteManagement` 的 `pollStale` 条补警示配色（`.network-stale` 此前只有工具类，GUI 端连 `.et-meta` 都没有；第五批又补了对比度与权重，见第 29 行）；`createToastRepeatGate` 注明 key 必须是有限字面量集合；`LoggingSettingsDialog` 注明 `require` 是「替换」而非「入队」 | `frontend-lib/src/style.css`、`modules/toast.ts`、`LoggingSettingsDialog.vue` |
| 26 | 单测补充：**同一次失败加载被多个调用方共享时只报 1 条**（对象 rejection 锁契约 + 字符串 rejection 作回归用例，已验证旧实现下该用例失败） | `frontend-lib/tests/remote-management-config.spec.ts` |
| 27 | **`RemoteManagement.vue` 注释编码修复**：31 行注释被 `c26fc7c9`（2026-09-30 那次保存按非 Unicode 代码页回写）写成 `?`。29 行从 `c26fc7c9^`（最后一次完好版本）**逐字恢复**；2 行（`1953`、`2465`）在 git 全历史中从未以完好形式存在（与损坏同一次保存写入），按现有代码重写。改动已校验：`31+/31-`、只落在注释行、ASCII 骨架一致、无 BOM/CRLF 变化 | `frontend-lib/.../RemoteManagement.vue` |
| 28 | `showMainWindow()` 改为 **best-effort**（`try/catch` + `console.warn`）：它是托盘流程在弹 confirm/toast 之前的**展示**步骤，窗口 API 抛错不应变成 tray 回调里的 unhandled rejection，也不应让「退出 / 停止服务」看起来毫无反应（同一函数覆盖全部 6 处调用点） | `easytier-gui/src/composables/tray.ts` |
| 29 | 第五批（review 复审）：① `.network-stale` 文字色改 `--et-warning`(#b45309，白底 ≈5:1 达 AA，深色下另给 #fbbf24)，图标仍用 `--et-warn`；**选择器加 `.et-meta` 提升一级权重** —— 否则 web 控制台的同权重 `.et-meta{color:…secondary}` 更晚加载会把它盖成灰字（真实存在的覆盖问题，不只是对比度）；② `reported` 闸门挪到守卫之后（见第 23 行）；③ `setSelectedInstanceId` 改为只在 `catch` 里补（见第 21 行），成功路径顺序与改动前完全一致；④ `ConfigTokenList` 重试按钮去掉死代码 `.catch()`，与 `UserList` 统一；⑤ `RuntimeLogs` 每个失败分支只取一次 `inlineErrorVisible()` | 多处 |

### 5.1 复盘：`confirm.require` 包装成 Promise 的正确写法

`primevue/confirmdialog` 的 `onHide()` 只在 `confirmation.onHide` 存在时回调，**不调用 `accept`/`reject`**；而 `Dialog` 默认 `closable` + `closeOnEscape`，X / Esc 都会走到这里。所以只要把 `confirm.require` 包成 Promise 并 `await`，就必须三件套齐备：

```ts
accept: () => resolve(true),
reject: () => resolve(false),
onHide: () => resolve(false),   // X / Esc：Promise 忽略二次 resolve，和上面并存安全
```

同一坑在 `easytier-gui/src/pages/index.vue:299` 早有注释（“reject may never fire on dismiss”），本轮补确认时又踩了一次 —— 凡是 `await confirm.require` 的写法都按上表检查。

**明确没做**（避免超出「风险可控」）：

- 不批量重写 ~26 处 CRUD toast 为 `runMutation` helper（机械但面大，需先补测试）。
- 不改任何消息文案的语义分工（每个动作的 key 与 severity 原样保留）。
- 不删除「防御性预检」类 toast（`DeviceList`/`UserList` 的 disabled 竞态兜底），宁可多一条也不静默。
- 不引入 Tauri 原生通知/对话框（涉及新依赖、Android 13+ 通知权限、各平台差异）。
- 不动「错误已内联 vs toast 唯一信号」中**内联不可见**的那些路径（删了会静默）。

---

## 6. 待决策 / 阶段 2 建议（按收益排序）

| 优先级 | 事项 | 收益 | 风险 | 需确认 |
|--------|------|------|------|--------|
| P1 | **`managed_config_synced_overwrite` 单一出口**：库与宿主二选一（当前「脏草稿 confirm」与「宿主 info toast」可能同屏，文案还互相矛盾，且 15s 轮询 adopt 可能反复触发） | 同一事件一次提示 | 中（跨包需同步发版） | 提示归属端；未选中实例被覆盖是否需要告知 |
| P2 | **其余列表/仪表盘的陈旧态**：`Dashboard`、`DeviceManagement`、`deviceArchive` 目前只有限流 toast，没有持久陈旧标记；`RuntimeLogs` 在「已有日志行」时只有 toast | 消掉最后一类「旧数据当最新」 | 中（模板改动） | 是否需要统一的「最后更新时间」文案 |
| P2 | **>3s 操作补 `:loading`**（RemoteManagement 的 save/delete/start/stop/edit、ConfigEditDialog） | 消除连点并发与「点了没反应」 | 低 | 是否加长文案（「正在下发/重建实例」） |
| P3 | **lifecycle / severity 语义收尾**：`RM` 的 warn 分支仍用 `TOAST_LIFE.success` 时长、`LSD` 的 `String(e)` → `formatApiErrorDetail`、`LSD` 复制/打开目录失败的 error(10s) 建议降 warn | 颜色/时长/文案一致 | 低 | warn 是否需要更长时长 |
| P3 | **隐藏窗口的可见反馈通道**：托盘触发的关键失败（启停服务、VPN 磁贴、TUN 故障）在窗口隐藏时 toast 不可见；`onStartService` 也未 `showMainWindow()` | 解决托盘场景零反馈 | 中-高（新依赖 + Android 通知权限） | 是否接受新增插件与权限申请 |
| P3 | **Android 静默失败补反馈**：`tun_device_error` / VPN 启动失败 / 授权被拒目前只有 `console`；`event_lagged` 无节流 | VPN 断了用户能知道原因 | 中（需限流） | 限流窗口与「仅首次」策略 |
| P4 | **草稿内删除是否确认**（ACL 规则/组、DNS、策略路由、VPN Portal）：现状「删链有确认、删规则没有」，且 `AclManager` 用原生 `confirm` 而全库其余用 `confirm.require` | 消除不一致 | 低-中 | 产品决策 |
| P4 | **CRUD toast 统一 helper（`runMutation`）**：UserList / ConfigTokenList / DeviceList 约 26 处同构 `toast.add`；仅收敛代码，不改文案与 key | 可维护性 | 中（面大，需先补 toast 断言测试） | 是否值得现在动 |
| P4 | **`exitApp` 的过度确认**：只看 `mode === 'service'` 就弹「正在运行的组网会立即断开」，服务其实已 `Stopped` 时属于多余确认（review 指出）。改成先 `getServiceStatus()` 再决定会多一次 await，收益不大 | 少一次无意义确认 | 低 | 是否值得为它多一次 IPC |
| P4 | **`--et-warning` 这个 token 从未定义**：`DnsCoverageBadge` / `Config.vue` / `DnsForwardersEditor` / `DnsHostsEditor` / `DnsUpstreamEditor` / `style.css:1114` 共 6 处都在用它，靠 fallback 生效 —— 而且 fallback 分两派（文字 #b45309、背景 #f59e0b）。谁都别单独去 `:root` 里"补"这个 token，否则 `DnsCoverageBadge` 的背景会一起变深；要补就得同时把 6 处 fallback 收敛成一套（文字色 / 填充色两个 token） | 消除隐性耦合 | 低 | 拆成 `--et-warning-text` / `--et-warning-fill` 还是保持现状 |

---

## 7. Changelog

| 日期 | 说明 |
|------|------|
| 2026-10-18 | 首版：全项目盘点约 180 个消息点；按「同一事实两处播报 / 轮询无节流 / 不可达 / 键缺陷」分类冗余，按「无确认 / 静默失败 / 无进度 / 窗口隐藏」分类缺失；落地第一批 10 项收敛（共享重复门控 + 4 处去重 + 2 处补确认 + `t('error')` 键修复 + 单测）；其余按 P1–P4 列为待决策 |
| 2026-10-18 | 第二批（按 review 决策）：① `RemoteManagement` 重复 catch 收敛为「每次加载尝试只播报一次」，且轮询加载保持静默；② 列表加载失败不再伪装空态（`ConfigTokenList` / `UserList` 补内联错误 + 重试）；③ `RemoteManagement` 5s 轮询失败新增常驻 `pollStale` 内联提示 + 30s 限流 toast；④ 补三处不可逆确认（保存并重启 / 放弃修改 / service 模式退出）；⑤ 拆分「操作失败」与「操作后刷新失败」，修掉「保存成功却报失败」的误导；⑥ 修正 `editNetwork` 错误文案与自动启动对齐失败的 severity/life |
| 2026-10-18 | 第三批（对第二批的自查修复）：① **真 bug**——Promise 化 confirm 缺 `onHide` 导致 X/Esc 后永久挂起（保存并重启静默失败、托盘退出无反应），两处补 `onHide`；② `UserList.loadUsers` 自带 catch（重试失败刷新文案）+ 复用死键 `web.users.load_failed`；③ `saveAndRunNewNetwork` 的选中项赋值移出刷新 `try`；④ 去重覆盖非对象 rejection（旧实现一次失败 3 条 toast，已用回归用例证实）；⑤ 共享尝试补 `settled`，晚到调用方不再静默；⑥ `RuntimeLogs` 判定统一；⑦ `pollStale` 条补配色、门控 key 约定与 `require` 替换语义写入注释；⑧ 单测 +2（205 passed） |
| 2026-10-18 | 第四批（顺带清理，均由第三批 review 引出）：① `RemoteManagement.vue` 被 `c26fc7c9` 写坏的 **31 行注释**修复（29 行逐字恢复自完好版本、2 行重写）；② `showMainWindow()` 改 best-effort，避免托盘确认/退出路径因窗口 API 抛错而静默失效 |
| 2026-10-18 | 第五批（复审收尾）：① `.network-stale` 对比度达标（文字 #b45309≈4.5:1）+ 选择器升权，修掉「web 控制台被 `.et-meta` 盖成灰字」这个实际存在的覆盖问题；② `reported` 闸门移到守卫之后，让 late-joiner 承诺与实现一致；③ `setSelectedInstanceId` 改回只补 `catch`，消除成功路径的 watcher 时序变化（review 建议的手工回归因此不再需要）；④ 死代码 `.catch()` 与重复 `inlineErrorVisible()` 清理；⑤ 更正 §3.2 关于 `DeviceManagement` 叠条的描述 |
