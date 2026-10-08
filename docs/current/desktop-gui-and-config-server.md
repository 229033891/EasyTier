# 桌面 GUI、服务进程与 Config-Server 同步

## Status

- Status: **Current**
- 最近审阅：2026-10-08（补 §4：配置页开关控件 `ToggleSwitch` + 负逻辑字段正向展示约定）
- 范围：Windows/Linux/macOS 桌面 `easytier-gui`、可选后台服务 `ET-Gui`、config-server 会话与 web-owned 配置回写
- 索引：[`../README.md`](../README.md)
- 深度复查记录（含 UI 下拉）：[`../archive/service-mode-web-config-sync-and-select-ui-2026-10-04.md`](../archive/service-mode-web-config-sync-and-select-ui-2026-10-04.md)
- Console Patch 接收端：[`web-managed-config.md`](./web-managed-config.md)
- Core 边界：[`architecture.md`](./architecture.md)

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

1. 本地持久化成功后；
2. 先 `report_via_process_client`（normal 同进程命中 registry）；
3. 仅当错误为 `NotEnabled` 时，再 RPC `WebClientService.ReportManagedNetworkConfig` 到服务进程；
4. 文案用 `gui_sync_message` / `gui_sync_message_for_error_code`，不暴露裸 Debug。

服务端 RPC：`easytier-core/.../process_rpc.rs` → 再次 `report_via_process_client`（读**服务进程** registry）。

OHOS nearby：`easytier-contrib/.../nearby_management.rs` 对相关方法返回 stub `not_enabled`（不假装可同步）。

### 2.4 Revision / ownership

- 本地编辑已落盘后，revision 冲突时**保留本地编辑**；冲突以 Console 当前 revision 提示，可重试推送或从 Console 重载。
- 新旧 GUI/服务混部：旧服务无新 RPC 方法时，错误应可理解；发布要求 **GUI 与 ET-Gui 成对升级**。

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
2. Normal + config-server：本进程 `report_via_process_client` 成功路径。
3. 服务未启 config-server：`not_enabled` 文案。
4. Drop/替换 `WebClient`：旧实例 Drop 不清除新 registry（`ptr_eq`）。
5. 流量导流 L2/L3 另见 [`traffic-steering.md`](./traffic-steering.md)，勿与本文混审。

---

## 6. 关键文件速查

| 区域 | 路径 |
|------|------|
| Registry | `easytier-core/src/management/full/config_server_client.rs` |
| WebClient install | `easytier-core/src/management/full/web_client.rs` |
| RPC | `easytier-core/src/management/full/process_rpc.rs`、`easytier-proto/proto/api_manage.proto` |
| GUI save/sync | `easytier-gui/src-tauri/src/lib.rs` |
| 服务安装 | `easytier-gui/src-tauri/src/service.rs`、`windows/installer-hooks.nsh` |
| OHOS stub | `easytier-contrib/easytier-ohrs/src/nearby_management.rs` |
