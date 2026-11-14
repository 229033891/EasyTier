# 配置编辑锁（Client Edit Authority）

## Status

- Status: **In progress**（C1–C5 已落地，联调清单待收尾）
- 日期：2026-11-14
- 范围：管理员可按设备把「Web 源配置在客户端只读」作为机器级开关（`devices.local_edit_locked`）
- 关联文档：
  - [`config-delete-authority.md`](./config-delete-authority.md)（删除权；本功能不改变删除权规则）
  - [`console-save-and-push-semantics.md`](./console-save-and-push-semantics.md)（单写者裁决）
  - [`client-web-console-interaction.md`](./client-web-console-interaction.md)

## 1. 规则（定稿）

编辑锁 = `已绑定 && local_edit_locked && Web 源 → 客户端只读`：

| 已绑定 | 机器开关 | 行归属 | 客户端表现 |
|---|---|---|---|
| 否 | 任意 | 任意 | 可编辑（现状，无变化） |
| 是 | 关（默认） | 任意 | 可编辑（现状，无变化；升级零行为变化） |
| 是 | 开 | User 源 | 可编辑（锁只管控制台下发的配置） |
| 是 | 开 | Web 源 | **只读**：UI 置灰只读 + 编辑/导入入口隐藏 + 保存被拒 |

三条硬性说明：

- **服务端判定用绑定（`enabled`）不用在线**：控制台 `ReportNetworkConfig` 与 GUI `save_network_config` 的门都只看「已绑定 + 开关 + Web 源」，设备断网也照拒——否则设备断网即获得推送权，控制台内容会被静默分叉。
- **客户端视角必然 fail-open**：锁值只来自心跳，会话 teardown 即清空（`clear_local_edit_locked`），所以离线设备的 GUI 会显示可编辑、本地保存也能落盘。这是有意接受的取舍：宁可让离线编辑只生效于本机、由下一心跳/控制台推送覆盖回去，也不接受「断网即永久锁死客户端」（无法证伪的锁会把 GUI 变成砖）。服务端权威门仍在，离线期间的编辑**推不回控制台**。
- **删除权规则不变**：锁只挡「编辑/保存/推送回」，控制台对 Web 行的删除权归属仍按 delete-authority 文档执行。
- 锁是**机器级**（`devices` 表逐设备），不是逐实例/逐配置；UI 层面按「该行是 Web 源」生效。

## 2. 双闸设计（fail 方向刻意不对称）

| 闸 | 位置 | fail 方向 | 理由 |
|---|---|---|---|
| 客户端 UI 门 | 心跳下发 `HeartbeatResponse.local_edit_locked`，GUI `editAuthority` prop | **fail-open**（没收到 flag / 未连接 = 可编辑） | 保证旧客户端与升级期零行为变化；锁是配置安全边界，不是安全红线 |
| 服务端权威门 | 控制台 `ReportNetworkConfig`（session）+ GUI `save_network_config`（bound 三态复用删除权口径） | **fail-closed**（bound=true 时拒绝） | 客户端旧版本 / UI 被绕过时，推送与保存仍在源头被拦；错误码稳定 `edit_locked`（控制台）/ `edit_locked_by_console`（GUI 本地保存） |

flag 经心跳下发，生效延迟一个心跳周期；由服务端权威拒绝兜底，不新增专用推送通道。

## 3. 落地对照（代码）

### C1 控制台侧（easytier-web + proto）

| 项 | 位置 |
|---|---|
| 迁移 | `easytier-web/src/migrator/m20261030_000015_device_local_edit_locked.rs`（ALTER TABLE + down 重建表） |
| 实体 | `db/entity/devices.rs` `local_edit_locked: bool` |
| DB 读写 | `db/mod.rs` `set_device_local_edit_locked` / `get_device_local_edit_locked`（stub 行 + `ON CONFLICT` 只碰 flag，不碰 display_name；心跳 upsert 对锁中性） |
| REST | `restful/mod.rs`：`DeviceArchiveItem.local_edit_locked` + `PUT /api/v1/devices/{device-id}/local-edit-lock`（`UpdateDeviceLocalEditLockJsonReq`） |
| proto | `web.proto` `HeartbeatResponse.local_edit_locked=6`；`api_manage.proto` `GetConfigServerStatusResponse.local_edit_locked=7` + `edit_locked` 错误码注释 |
| 心跳下发 | `client_manager/session.rs` `heartbeat_response_with_revision`（读失败 fail-open + warn）；`client_manager/mod.rs` `HeartbeatPolicy::response()` 默认 false |
| 权威拒绝 | `session.rs` `handle_report_network_config`：machine_id/token 校验后 `conflict("edit_locked", None)` |

### C2 客户端核心（easytier-core + ohrs）

| 项 | 位置 |
|---|---|
| 稳定码 + 映射 | `config_server_client.rs`：`error_code::EDIT_LOCKED`、`ReportNetworkConfigError::EditLocked`、`gui_sync_message_for_error_code` 分支 |
| trait 默认 | `fn local_edit_locked(&self) -> bool { false }`（未接线的宿主自动 fail-open） |
| 缓存存取 | `web_client.rs` controller `local_edit_locked: AtomicBool` + `store_heartbeat_local_edit_locked` / `clear_local_edit_locked` / `cached_local_edit_locked`；**会话 teardown 4 处**在 `clear_web_owned_instance_ids()` 后配对 clear（重连重新学习，避免陈旧锁残留） |
| 上报前置门 | `WebClientReportFacade.report_network_config` 开头 → `Err(EditLocked)` |
| 状态查询 | `process_rpc.rs` `get_config_server_status`：`connected && cached` 才权威（沿用 W 列表口径）；ohrs stub 字面量补字段 |

### C3 GUI

| 项 | 位置 |
|---|---|
| 后端硬门 | `easytier-gui/src-tauri/src/lib.rs` `save_network_config`：`resolve_local_edit_lock`（`Ok` 用值；`rpc_client_unavailable`=未绑定放行；其它 Err 按 bound+locked，同删除权 H1 fail-closed）+ `local_edit_blocked_by_console` → 稳定码 `edit_locked_by_console` |
| 状态透出 | `ConfigServerStatusDto.local_edit_locked`（connected 才权威，同 W 列表） |
| 前端接线 | `composables/backend.ts` 类型 + `pages/index.vue` `configServerEditLocked`（探针失败滞回同 `configServerEnabled`）+ `:edit-authority` 注入 |
| 编辑器门 | `RemoteManagement.vue` `editAuthority` prop → `isConsoleManagedEdit`（fail-open，meta 缺失不锁）→ Config `read-only`、编辑/导入按钮隐藏（导出保留）、`canSaveConfig` 拒绝 + 文案 |

### C4 控制台前端（easytier-web/frontend）

| 项 | 位置 |
|---|---|
| 开关 UI | `DeviceManagement.vue` 设备详情 footer：ToggleSwitch + 开启前确认弹窗 + toast |
| API | `modules/api.ts` `set_device_local_edit_lock`；`list_devices` 带出 flag |
| 列表合并 | `frontend-lib/src/modules/utils.ts` DeviceInfo/ArchiveRow 合并 `local_edit_locked` |
| 文案 | cn/en：`web.device.local_edit_lock*`、`web.device_management.edit_managed_by_console` / `save_config_disabled_edit_locked` |

## 4. 测试

| 层 | 用例 |
|---|---|
| web db | `device_local_edit_lock_roundtrip_and_upsert_neutral`（读写往返、stub 行、心跳 upsert 不清锁、与 display_name 互不覆盖） |
| web session | `report_network_config_rejects_when_local_edit_locked`（**权威门**：锁开时客户端上报被拒 `edit_locked`，锁关放行） |
| core | `local_edit_lock_follows_heartbeat_and_session_boundary`（心跳存取 + teardown 清空）；`edit_locked_error_mapping_is_stable`（错误码/文案/映射）；`report_error_codes_map_to_typed_variants`（`edit_locked` 不得落到 `Invalid` 兜底） |
| GUI | `edit_authority_tests` 6 例（锁+Web 才拦 / User 不拦 / 需绑定 / 需开关 / 未知源 fail-open / 状态探针失败 fail-closed） |
| frontend-lib | `remote-management-config.spec.ts` edit authority 3 例（Web+console 只读且入口隐藏；User+console 可编辑；Web+local 可编辑）；`api-error.spec.ts` `edit_locked_by_console` / `edit_locked` 文案映射 |

## 5. 联调清单

1. 控制台开启开关 → 客户端下一心跳周期：Web 行编辑器只读、编辑/导入入口隐藏、`save_network_config` 返回 `edit_locked_by_console`（UI 显示本地化文案）；User 行不受影响。
2. 控制台关闭开关 → 心跳周期内解锁；断线重连后状态以新会话为准。
3. 旧客户端（无 flag 解析）混部：UI 可编辑，但 `ReportNetworkConfig` 仍被控制台拒绝（`edit_locked`）——双闸兜底成立。
4. 客户端绑定但离线：**服务端仍拒推送**（锁不因离线解除）；客户端本地因无心跳而 fail-open，可编辑/落盘，但 `ReportNetworkConfig` 被拒 → 编辑只留在本机，上线首心跳/控制台推送后收敛。验收时以「控制台内容未被改写」为准，不要期望离线时 GUI 也置灰。
5. GUI service/remote 模式：owner RPC 抖动时本地保存仍按 fail-closed 拒绝；`rpc_client_unavailable`（无宿主）放行。
6. 控制台对锁定设备仍可正常编辑下发（单写者不变）；删除权规则照旧。

## 6. 防做过头

- 不做逐实例锁（机器级足够）；不做专用推送通道（心跳 + 权威拒绝兜底）。
- 不改 keep / stale / disabled / 删除权语义。
- 不把锁语义写到 `docs/current/`（未验收前只更新本文档与联调清单）。
