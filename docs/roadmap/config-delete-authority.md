# 配置删除权归属方案（Delete Authority）

## Status

- Status: **In progress**（G1–G4 + B1/B2/高优先修补已落地，联调清单待收尾）
- 日期：2026-10-12
- 范围：Web 源 / User 源行在控制台与各客户端的删除权限
- 关联文档：
  - [`console-save-and-push-semantics.md`](./console-save-and-push-semantics.md)（单写者裁决 §8.1.3；User 源 Save/Run 接管确认 §8.1.5——**不是**删除权，仅同属归属边界）
  - [`client-web-console-interaction.md`](./client-web-console-interaction.md)
  - [`local-config-residue.md`](./local-config-residue.md)（**删完之后**设备 / GUI 残留如何收敛；与本文「谁可以删」互补）
- 关联实现：`keep_local_config`（`easytier-proto/proto/api_manage.proto` `DeleteNetworkInstanceRequest` 字段 2，L266）、disabled 同步、stale 清理

## 1. 规则（定稿）

删除权 = `bound && Web-source ? console : local`：

| 控制台绑定 | 行归属 | 删除权 | 客户端表现 |
|---|---|---|---|
| 未配置 / 已解绑 | 任意 | 本地 | 随便删（现状，无需改）；已解绑的 Web 行**可能**被 reconcile 复活 |
| 已绑定 | User 源 | 本地 | 随便删；reconcile **永不**复活 User 行 |
| 已绑定 | Web 源 | **仅控制台** | 客户端删入口拒绝/置灰 + “由控制台管理”；**停用保留** |

> 前两行客户端表现同为「随便删」，唯一差别是**控制台重新绑定后会不会被 reconcile 复活**（Web 行会，User 行不会）。这是选择行的持久化归属，不是门的松紧差异。

三条硬性说明：

- 门控条件是**绑定**不是在线。绑了但离线，Web 行依然锁——离线删同步不上去，上线还会被复活，只会制造“删了又活”。
- **绑定** = WebClient / config-server client 已创建（`get_config_server_status.enabled`），**不是** settings 里填了 URL，也**不是** `connected`。启停瞬间可能有极短窗口；比用在线态好，弱网下删除键不会乱闪。
- 控制台删 Web 行：在线时 `keep=false` 硬删（先设备 RPC，成功再删控制台 DB；cleanup 同旗标），设备 purge + 推 `delete_network_instance` 事件。设备离线时整单失败（`ClientNotFound`），**本期不改**为离线 tombstone / 上线补删。

## 2. 为什么是这个规则（不再论证，只留结论）

- 控制台 DB 是 Web 行的 single source of truth，边缘可删即多写者，时序一乱就脑裂。
- 删除不可逆、停用可逆：停用放客户端（现有 disabled 同步），删除放控制台，标准能力分级。
- 审计：删除收归控制台才有人、有时间、有 revision。
- 现状代码本就按此写（keep 旗标、disabled 同步、stale 清理），客户端删 Web 行今天实际也删不掉（删完被 `run_missing` 复活，或经 disabled 同步降级为停用）——本方案只是把事实挑明并收紧。

## 3. 现状核验（2026-10-12，dev）

| 端 | 现状 | 结论 |
|---|---|---|
| 控制台删除 | `RemoteManagement.vue` 仅按 `currentNetworkControl.deletable` 门控，**不分 Web/User**；后端先 RPC `keep=false`，成功再删 DB | 基本可用；**不得**在此按钮上加「bound+Web 置灰」（见 G1） |
| GUI（tauri）`remove_network_instance` | `enabled`+Web 门；RPC 失败按 bound；宿主注入 `deleteAuthority` | **已落地**（G1 + H1） |
| FFI | `stop_network_instance` 无门（停用）；`delete`/`retain` managed+active 整单拒绝 | **已落地**（G2 + B1） |
| ohrs | teardown 先 `drop(managed)` 再 mark/delete；补 `stop_network_instance` 事件 | **已落地**（G3 + H2） |
| 未绑定各端 | core `remote_managed=false` 只看 `is_deletable` | 已实现，不用动 |

## 4. 缺口与实施（按端）

### G1. GUI 删除门

**后端门：**

- **绑定**：`get_config_server_status.enabled`（非常态 `connected`）。
  - `Ok(enabled)` → 用该值。
  - `Err("rpc_client_unavailable")` → **未绑定**（放行本地删）。
  - **其它 Err**（service/remote RPC 抖动）→ **按 bound=true**，避免「未绑定 + 缓存 Web」硬删后上线复活（H1）。
- **行源**：`handle_get_network_config_with_source`，失败回退 `storage.persisted_source`。
- 门控：`source == Web && bound` → `Err("delete_managed_by_console")`。
- **异常一律 fail-closed**：只有 `rpc_client_unavailable`（确认无宿主进程）才算未绑定；其余 Err 一律按 `bound=true`。
- 行源读不到（owner RPC 失败）时回退本地 sticky `Web` 缓存，缓存为空才放行——**不得**默认按“本地可删”。

**前端置灰：**

- GUI 传 `deleteAuthority: 'console' | 'local'`；控制台不传。
- 禁用按钮的 tooltip 挂在外层 `<span>`（原生 `disabled` 不派发 mouseenter，H3）。
- 探针失败：保留上次 `configServerEnabled`，仅 `rpc_client_unavailable` / 服务未装时清 false（防 1s 轮询闪烁）。

### G2. FFI（B1 定稿：补显式 stop）

- **`stop_network_instance`**（新）：`delete_owned_*` 停用路径，**无** managed 门——bound+Web 可停（§1「停用保留」）。
- **`delete_network_instance` / `retain`**：managed+active → 整单拒绝，错误前缀稳定码 `delete_managed_by_console`（提示改用 stop）。
  - **判据 = `active && (id ∈ 运行时 tracked || 实例自身 config_source == Web)`**。不能只看 tracked：tracked 只是「本进程 WebClient 主动 run 过」的运行时集合，别处（GUI/service 模式、另一条 run 路径）起的 Web 源实例不在其中；`stop_network_instance` 之后 id 又会离开 tracked。只看 tracked 会出现「文档说不该删、实际放行」。
  - **锁毒化 fail-closed**：`CONFIG_SERVER_CLIENT` / `instance_ids` 取锁失败时按「已绑定」拒绝，与 GUI 后端同向（不再 fail-open 放行）。
- Android JNI / iOS「delete」入口已改调 **stop**（历史语义就是停用）。
- 测试：`lock_config_server_gate_for_test` 串行化全局 active/tracked，防与 data-plane 测例竞态（B2）。

### G3. ohrs

- `stop_web_client`：先抽 hooks 状态 → **`drop(managed)`（abort heartbeat）** → 再 mark/delete → 写 pending stop 事件。
- 事件 `success` 位必须按**实际移除集合**逐实例给：`delete_network_instances` 返回的是**剩余** id，故 `removed = tracked − remaining`，按 `removed.contains(id)` 置位；整体返回值 `ret = !removed.is_empty()`。不可用 `.map(|_| true)`——tracked 过期（实例早已不在）时会向 ArkTS 谎报停止成功。
- `stop_kernel` 对 Web 源已 `mark_user_disabled`——属 stop。

### G4. 解绑 / 重绑过渡（文案）

- 解绑：本地副本可删；弹窗「已同步的配置保留为本地副本」。
- 重绑：确认「将以控制台为准同步」。

## 5. 注意点（非缺口，防做过头）

- 控制台删 **User 行**：本期不动范围；建议复用 `confirm_save_takeover_user_source`。
- 绑定判定用 `enabled`，不用 `connected`。
- **不**改变 keep / stale / disabled 传输语义；只加客户端门。
- **不**做离线 tombstone。

## 6. 联调清单

1. 绑定 + Web 行：GUI/FFI **delete** 拒绝；FFI/Android/iOS **stop** 成功；控制台在线仍可删。
2. 绑定 + User 行：三端可删；控制台不复活。
3. 解绑后 Web 行可删；重绑后以控制台为准收敛。
4. 控制台删 Web 行（设备在线）：设备 purge + delete 事件；DB 行消失。
5. 控制台删 Web 行（设备离线）：整单失败，DB 行仍在。
6. 绑了但离线：客户端 Web 行删除键仍锁；service RPC 抖动不误放行硬删。
7. FFI 侧同向核对：tracked 集合缺失但实例 `config_source == Web` 时 **delete 必须被拒**（§1 口径）；`stop_network_instance` 之后再次 delete 仍被拒。
8. FFI 锁毒化：人为毒化 `CONFIG_SERVER_CLIENT` / `instance_ids` 后，delete 按**拒绝**处理，不得放行。
9. ohrs：`stop_web_client` 时若 tracked 已过期（一个都没真正移除），事件 `success` 必须为 `false`。
10. GUI 置灰不依赖本地 URL：清空 settings 里的 `config_server_url`，只要 owner 进程 `status.enabled=true`，删除键仍应置灰。

## 7. 落地对照（代码）

| 项 | 位置 |
|---|---|
| G1 后端门 + H1 | `easytier-gui/src-tauri/src/lib.rs`：`remove_network_instance` bound 三态 |
| G1 宿主注入 | `deleteAuthority` + 独立 `hostSurface`（`'web-console' \| 'gui-client'`）；GUI / DeviceManagement |
| G1 tooltip H3 | `RemoteManagement.vue` 删除按钮外包 `<span v-tooltip>` |
| G1 探针滞回 | `index.vue` refreshConfigServerConnection catch |
| G2 stop / delete | FFI `stop_network_instance`；`delete`/`retain` 经 `*_by_name_gated`（门在 `mutation_lock` 内） |
| G2 门判据修正 | `config_server.rs` `tracks_any_managed_instance`（tracked ∪ `config_source==Web`，锁毒化 fail-closed） |
| G2 Android/iOS | JNI/iOS 调 `stop_network_instance` |
| G2 测试串行 B2 | `lock_config_server_gate_for_test` |
| G3 ohrs H2 | `stop_web_client` 先 `drop(managed)`；`PENDING_CONFIG_SERVER_EVENTS` 上限 256 |
| 文案 | `locales/{cn,en}.yaml` `delete_managed_by_console` |
| 前端单测 | `remote-management-config.spec.ts` delete authority / hostSurface |
