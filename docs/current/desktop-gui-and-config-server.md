# 桌面 GUI、服务进程与 Config-Server 同步

## Status

- Status: **Current**
- 最近审阅：2026-10-22（§2.3.1：note 提前到 save/run 前；抑制预算 **2**（reconcile + AdoptRuntime）；另源下发仍提示；同步 tip 文案缩短）；2026-10-14（§2.5 存储优先 + 热更写回 + Service/Remote Web 运行态指纹采纳；徽章/RPC warn）
- 上次审阅：2026-10-09（补 §2.5：读配置权威；GUI RPC 优先 vs Web 存储优先；补 §6：配置页 / 运行页职责）
- 范围：Windows/Linux/macOS 桌面 `easytier-gui`、可选后台服务 `ET-Gui`、config-server 会话与 web-owned 配置回写
- 索引：[`../README.md`](../README.md)
- 发布前人工回归清单：[`system-overview.md`](./system-overview.md) §7
- Console Patch 接收端：[`web-managed-config.md`](./web-managed-config.md)
- Core 边界：[`architecture.md`](./architecture.md)
- 控制台删除后 GUI/设备残留（Roadmap）：[`../roadmap/local-config-residue.md`](../roadmap/local-config-residue.md)

本文给实现复查 / 其他 agent 用：说明**进程分工**与**权威同步路径**，避免再把 GUI 进程内的 `WEB_CLIENT` 当成服务模式下的会话所有者。

---

## 1. 进程模型（Windows 上任务管理器常见双进程）

| 进程（用户可见） | sc / 内部名 | 职责 |
|------------------|-------------|------|
| **ET**（窗口） | GUI 前台 | Tauri UI、本机配置编辑、通过 RPC 查询/控制服务 |
| **ET Gui Service** | `ET-Gui` | 后台 daemon：持有网络实例、config-server `WebClient` 会话 |

同一安装目录同一份可执行文件；服务安装参数带 `--daemon`（见 `easytier-gui/src-tauri/src/service.rs`）。

```text
┌─────────────────────┐         RPC (loopback)          ┌──────────────────────────┐
│  GUI 前台进程         │  GetConfigServerStatus /        │  ET-Gui 服务进程           │
│  - UI / 本地 TOML     │  ReportManagedNetworkConfig /   │  - CoreInstance(s)         │
│  - normal 模式才有    │  实例管理 …                     │  - WebClient (config-srv)  │
│    本进程 WebClient   │◄──────────────────────────────►│  - REPORT_CLIENT registry  │
└─────────────────────┘                                 └────────────┬─────────────┘
                                                                     │ config-server
                                                                     ▼
                                                            ┌────────────────┐
                                                            │ easytier-web   │
                                                            │ Console / API  │
                                                            └────────────────┘
```

- **Normal 模式**：GUI 进程内跑实例与（可选）`WebClient`；无 `ET-Gui` 服务亦可。
- **Service 模式**：GUI 切到服务后会清空本进程 `WEB_CLIENT`；状态与同步必须以**服务进程**为准。
- **Remote 模式**：GUI 连的是**另一台机器**的节点进程（本机不跑实例、不装服务）。
  `WebClient` 在**远端**节点进程内，GUI 同样只走 RPC——**语义与 Service 一致**，
  所以 §2.3 的上报门控、§2.5 的读权威顺序对两者都适用。
  限制：`set_tun_fd` 在此模式下不支持。

| 模式 | 实例在哪 | `WebClient` 在哪 | GUI 本地存储的角色 |
|------|---------|-----------------|-------------------|
| Normal | GUI 进程 | GUI 进程 | 权威副本 |
| Service | ET-Gui 服务进程 | ET-Gui 服务进程 | 仅界面缓存，可能过期 |
| Remote | 远端节点进程 | 远端节点进程 | 仅界面缓存，可能过期 |

**Service 与 Remote 下 GUI 本地 `source` 缓存可能缺失或过期**（`GuiHooks` 只在持有
`WebClient` 的进程安装）。判断 web-owned 一律以权威侧为准，见 §2.3 第 0 步。

---

## 2. Config-server 回写：权威路径

### 2.1 问题（已修）

旧逻辑：服务模式状态已走 RPC（显示 Connected），保存 web-owned 配置却只查 GUI 本进程 `WEB_CLIENT` → 恒报「client 未运行」。

### 2.2 进程级 registry（权威）

实现：`easytier-core/src/management/full/config_server_client.rs`

| 符号 | 作用 |
|------|------|
| `ConfigServerReportClient` | 可上报 managed network config 的 trait |
| `install_config_server_report_client` / `clear_…` | `WebClient` 启动安装；Drop 时用 `Arc::ptr_eq` 清理 |
| `report_via_process_client` | 本进程入口 |
| `error_code::*` | 稳定 wire：`not_enabled` / `not_connected` / `not_authorized` / `revision_conflict` / `ownership_conflict` / `invalid` |

`WebClient` 构造时先 install report client，再 `mark_enabled`；与 `GetConfigServerStatus` 共用同一「本进程是否有会话」来源。

### 2.3 GUI 保存顺序

实现：`easytier-gui/src-tauri/src/lib.rs`（`save_network_config` 同步段）

0. **判定是否 web-owned**：`resolve_is_web_owned` 经 §2.5 取配置时，**`ConfigSource` 仍以运行中实例（RPC）为准**（配置正文才是存储优先）；GUI 本地 `storage.persisted_source` 只作兜底。
   判定是**权威优先**而非任一侧 `Web` 即上报（纯函数 `should_report_to_config_server`）：
   owner 明确 `Web` → 上报；明确 `User` → 不上报（即使本地仍是 `Web`）；
   owner 答不上来（RPC 调用失败）→ 才看本地缓存。
   不可达时跳过上报，**不**让已成功的本地保存变成失败。
1. 本地持久化成功后；
2. 先 `report_via_process_client`（normal 同进程命中 registry）；
3. 仅当错误为 `NotEnabled` 时，再 RPC `WebClientService.ReportManagedNetworkConfig` 到服务进程；
4. 文案用 `gui_sync_message` / `gui_sync_message_for_error_code`，不暴露裸 Debug。

> **客户端改配置并推控制台**：连着 config-server 且实例为 web-owned 时，上述第 2–3 步仍会把本次保存写入控制台 SQLite。未连接 / User 源 → 只落本机，不推库。

#### 2.3.1 客户端改本地配置：两种连接情形（验收对照）

| 情形 | 保存时发生什么 | UI 提示（各代表一次动作） | 是否合理 |
|------|----------------|---------------------------|----------|
| **未连控制台**，或实例为 **User 源** | 只写本机存储；**不** `ReportManagedNetworkConfig`；运行中实例**不**因 Save 重建 | 一条：`save_ok_local_hint` / `save_ok_unchanged` | **合理**：本地权威，控制台无副本可推 |
| **已连控制台** 且实例 **Web 托管** | ① 先写本机；② 上报控制台 SQLite；③ 若行仍启用且在线，控制台 reconcile 可热更/重建设备；④ 成功后写回本机缓存并 emit `managed_config_synced_overwrite` | 首条：「已保存到本机…」。自保存回声：`noteLocalConfigSaved` 在 save/run **前**；宿主 tip 在 12s 窗内按预算 **抑制最多 2 次**（`SYNC_OVERWRITE_ECHO_BUDGET`：reconcile 回写 + 随后 `AdoptRuntime` 的二次 emit；见 `managedConfigSyncTip.ts`）。预算用尽后同窗内再来的事件（控制台另源下发）仍提示。编辑器仍可静默 reload | **总体合理**（落盘 ≠ 运行态生效 ≠ 控制台回写）。首条在 GUI 侧恒按「本地已存、未重建」措辞，**不区分**随后是否热更成功 |

**已知边界**：

1. GUI `save_network_config` 对前端仍返回 void/`unchanged`，故首条 toast **看不到**「已推到控制台」；推送成败目前混在 save 的 Err（`gui_sync_message`）或（窗口外的）同步 tip 里。
2. 上报失败时本地其实已存成功，但命令返回 `Err` → 前端可能显示「保存失败」类提示（文案里会说明已落本地）——体验略拧，但避免静默丢同步。
3. 自保存回声抑制仅作用于**宿主 tip**；`RemoteManagement` 对同事件的编辑器刷新不受影响。失败保存也可能提前 note，最多吞掉预算内（默认 2 条）真实覆盖 tip（可接受）。更稳的长期方案：事件带 revision / 配置指纹，前端比对后再决定。

> **为什么第 0 步要问权威侧**：GUI 本地 `source` 缓存只由 `GuiHooks::pre_run_network_instance` 写入，
> 而该 hook 只在**持有 config-server `WebClient` 的那个进程**里安装。Service / Remote 模式下
> 那是 ET-Gui 或远端节点，GUI 进程永远学不到 `source = Web`；若只看本地缓存，
> Console 建的网络在 GUI 里编辑保存会被**静默丢弃**（编辑只落 localStorage）。
> 不要改成给服务进程补 hooks——它拿不到 GUI 的 `AppHandle`，发不出事件，只会掩盖问题。

服务端 RPC：`easytier-core/.../process_rpc.rs` → 再次 `report_via_process_client`（读**服务进程** registry）。

OHOS nearby：`easytier-contrib/.../nearby_management.rs` 对相关方法返回 stub `not_enabled`（不假装可同步）。

### 2.4 Revision / ownership

- 本地编辑已落盘后，revision 冲突时**保留本地编辑**；冲突以 Console 当前 revision 提示，可重试推送或从 Console 重载。
- 新旧 GUI/服务混部：旧服务无新 RPC 方法时，错误应可理解；发布要求 **GUI 与 ET-Gui 成对升级**。

### 2.5 读配置权威：`handle_get_network_config_with_source`

实现：`easytier-core/.../remote_client.rs`（trait 默认）+ Web 覆盖 `easytier-web/.../client_manager/mod.rs`。

| 调用方 | 顺序 | 权威 | 原因 |
|--------|------|------|------|
| **GUI / ET-Gui**（trait 默认 + GUI 覆盖） | **正文：本地存储优先**（RPC 只补空 `dev_name`）；**`ConfigSource`：RPC 优先**；无存储行才整份用 RPC；**Web 托管且运行态指纹变化时采纳运行态并写回存储** | **已保存正文**（本地 Save 领先时）/ **设备运行态**（控制台下发后）/ **运行时所有权** | Save 只写存储、不重建运行中实例 → 正文用存储，避免「保存后重进仍见旧网络名/密码」。Service/Remote 下 `GuiHooks` 不在 GUI 进程，靠 `last_observed_runtime_fp` 检测设备侧变化并采纳，避免编辑页长期过期、保存盖回控制台。 |
| **Web 控制台**（`ClientManager` 覆盖） | **SQLite 优先** → 设备 RPC 仅当无行 | **Web 库** | 托管编辑以库为准；避免打开配置页时等忙设备。设备本地改过且未回写库的内容，编辑页不显示；从控制台保存会按 reconcile 盖回设备。 |

**热更 / 覆盖回写本地（Normal）**：控制台 Full overwrite（`on_managed_config_overwrite`）与 hot-patch（`on_managed_config_patched`）成功后，持有 `WebClient` 的进程须把运行中 TomlConfig **写回 GUI 存储**并派发 `managed_config_synced_overwrite`。写回失败只 `warn`、不假成功 tip。脏稿收到同步事件时**确认后再** force reload。

**Service / Remote Web 采纳**：GUI 存储记录 `last_observed_runtime_fp`；本地 Save **不**刷新该指纹。下次 `get_config` 若 RPC 源为 Web 且运行态指纹相对上次观察已变、且正文实质不同 → 采纳运行态写回存储并 emit。首次观察只记指纹、保留本地正文（覆盖「Save 领先于 runtime apply」窗口）。

不要把两套读序合并成「全局 RPC 优先」或「全局只读存储、不叠 runtime」。

---

## 3. 与 Console managed config 的关系

| 方向 | 文档 | 代码重心 |
|------|------|----------|
| Console → 节点（下发） | [`web-managed-config.md`](./web-managed-config.md) | Web Session Full/PATCH、revision CAS |
| 节点 → Console（web-owned 回写） | **本文 §2** | `ReportManagedNetworkConfig` + registry |

不要把「Console PATCH 接收」与「桌面回写 Console」混成一条路径。

---

## 4. 配置页 UI 约定（桌面 / frontend-lib）

- 协议类控件统一 `Select`（端口转发、UrlInput 初始/监听协议等）；少选项切换可保留 `SelectButton`。
- **开关类控件统一 `ToggleSwitch`**（2026-10-08 起，不再用 `ToggleButton` + 固定宽度 + `on/off-icon`）：布尔项「开」= 功能启用。
- **负逻辑字段走 `inverted` 机制正向展示**（2026-10-08 起）：底层字段名与后端语义不变，UI 只显示正向标签，避免一屏「禁用 / 不允许」。当前 9 个：`disable_p2p`→「允许 P2P 直连」、`disable_kcp_input`、`disable_quic_input`、`disable_tcp_hole_punching`、`disable_udp_hole_punching`、`disable_sym_hole_punching`、`disable_upnp`、`disable_ipv6`、`disable_encryption`（→ `allow_*`）。**以 `Config.vue` 的 `bool_flags[].inverted` 为准**；实现是 `advancedFlagGroups` 里给 `model` 套一个取反的 computed（`!cfg[field]`），未填过的字段因 proto3 `bool` 默认 `false` 而显示为「开」。
- **反转展示会改变冲突提示的措辞方向**：改这些字段的 help / 冲突文案时，务必同步 `configConflicts.ts` 里对应的 `*_help` key（例：`disable_p2p_conflict_help`、`p2p_only_blocks_disable_p2p_help`、`disable_ipv6_conflict_help`、`disable_encryption_algo_conflict_help`）。`advancedFlagConflictHelpKey()` 返回的 key 会**优先于** `inverted.help`。
- 占位提示色：`--et-placeholder-color`（`#a8b5c5`），**不要**用 `--text-color-secondary`（过深，像已填值）。见 `easytier-web/frontend-lib/src/style.css`。
- **响应式断点两个，别混**：`Config.vue` 的 `@media (max-width: 760px)`（高级开关分组 2 列→1 列、开关项转双列、紧凑网格转单列）；`src/style.css` 的 `@media (max-width: 640px)`（`.config-inline-label` 11rem→5.5rem、`.config-inline-expand` 的 `margin-left` 归零）。`.config-inline-*` 是 Config 与 `dns/*Editor` 共用的全局类，必须放在 `src/style.css`（scoped 穿不进子组件）。
- 自定义 `Select` `#value` 槽若渲染 placeholder，须显式浅灰色类（例：`RemoteManagement` 的 `network-select-placeholder`）。

---

## 5. Agent 复查清单（最短）

1. 服务模式：Connected 时保存 web-owned → 应同步成功，不得再报 client 未运行。
2. **Service / Remote 模式：Console 建网络 → ET-Gui 自动运行 → 不在 GUI 启停、直接编辑保存
   → Console 必须收到**。这是 §2.3 第 0 步修掉的回归；GUI 本地没有 `source = Web` 是**正常**的。
3. Normal + config-server：本进程 `report_via_process_client` 成功路径。
4. 服务未启 config-server：`not_enabled` 文案。
5. 判定表单测：`lib.rs` 的 `web_owned_report_tests`（权威 Web 覆盖本地缺失 / 陈旧 User；
   User 与 Legacy 不上报）。
6. Drop/替换 `WebClient`：旧实例 Drop 不清除新 registry（`ptr_eq`）。
7. 流量导流 L2/L3 另见 [`traffic-steering.md`](./traffic-steering.md)，勿与本文混审。

---

## 6. 配置页 / 运行页职责（Web + GUI）

两端统一为 **两个页面、两种职责**（2026-10-08 已落地）：

| 页面 | 只管什么 | 底部主区 |
|------|----------|----------|
| **运行页（状态）** | 看运行信息；启停网络 | **启动 / 停止**；入口「节点配置」 |
| **配置页** | 改参数；落盘 / 丢弃草稿 | 脏：**取消编辑 + 保存**；干净：**返回运行页**（无启停主按钮） |

- Web：`?mode=status|config`；配置模式不挂启停主按钮。
- GUI combined：壳内 `isEditingNetwork` 切换；停止态默认运行空态（启动 + 节点配置）。
- 「取消编辑」= 脏时重拉已保存配置；「返回运行页」= 干净时退出编辑，不重拉。
- 未保存草稿、当前页签**不同步**跨端；已保存配置与运行态仍走既有 Console / RPC 路径。
- 可选后续（Roadmap）：配置页次要「保存并运行」— 见 [`../roadmap/config-vs-run-pages.md`](../roadmap/config-vs-run-pages.md)。

---

## 7. 关键文件速查

| 区域 | 路径 |
|------|------|
| Registry | `easytier-core/src/management/full/config_server_client.rs` |
| WebClient install | `easytier-core/src/management/full/web_client.rs` |
| RPC | `easytier-core/src/management/full/process_rpc.rs`、`easytier-proto/proto/api_manage.proto` |
| GUI save/sync | `easytier-gui/src-tauri/src/lib.rs` |
| 服务安装 | `easytier-gui/src-tauri/src/service.rs`、`windows/installer-hooks.nsh` |
| OHOS stub | `easytier-contrib/easytier-ohrs/src/nearby_management.rs` |
