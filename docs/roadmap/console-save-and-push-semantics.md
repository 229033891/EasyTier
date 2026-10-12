# 控制台保存与下发语义设计（Save → Push）

## Status

- Status: **In progress**（§9.1–9.4 已落地；§9.5 Start 已与 reconcile 共享 epoch fence，联调「保存 1 秒内点启用」与版本门规矩仍待收尾）
- 日期：2026-10-12
- 范围：Web 控制台保存配置后的落库、下发、收敛全链路
- 相关文档：[`../current/web-managed-config.md`](../current/web-managed-config.md)（接收端增量收敛）、[`client-web-console-interaction.md`](./client-web-console-interaction.md)（交互与超时分级）
- 关联实现：`keep_local_config` 停用/删除拆分（`api_manage.proto:262`，`process_rpc.rs`，GUI/FFI/ohrs 适配）

### 已落地入口（对照代码）

| 能力 | 位置 |
|------|------|
| Console Save 主路径 | `ClientManager::save_web_network_config`（`easytier-web/src/client_manager/mod.rs`） |
| REST | `handle_save_network_config` → 上述主路径；**禁止**再前后双 `invalidate` |
| Upsert 启用态 | `upsert_network_config` / `insert_or_update_console_save_network_config`（`easytier-web/src/db/mod.rs`） |
| Trait 覆盖（防 GUI 默认禁用） | `handle_save_network_config_with_source`：Web 源转发 `save_web_network_config`；REST 主路径仍是后者 |
| Drift 日志 | `apply_web_source_runtime_reconcile`（patch / overwrite run） |
| 前端回显 | `SaveNetworkConfigResponse` + `RemoteManagement.vue` toast（`action` / `reason`） |
| 在线判定 | `Session::runtime_is_current`（Save 路径） |
| 失败暂停 | `ManagedRuntimeState::{reconcile_fail_streak, reconcile_paused}`；`MANAGED_RECONCILE_FAIL_LIMIT=5` |
| User 源接管确认 | `RemoteManagement.vue` + `NetworkMeta.source` |
| 自启推迟门 T=5s | `easytier/src/core.rs` + `WebClient::wait_first_heartbeat`；revision 持久化 `managed_config_revision` |
| 覆盖同步提示 | `InstanceMutationHooks::on_managed_config_overwrite` → FFI/ohrs/GUI 事件 |

## 1. 背景与改造前问题

改造前控制台“保存配置”的真实语义是**暂存 + 停用**，分三段：

1. 保存时零 RPC：Web 未覆盖时走 trait 默认 `handle_save_network_config_with_source`（`easytier-core/.../remote_client.rs`）→ SQLite upsert + 标 `disabled=true`；REST 层前后各一次 revision 失效。设备此时还在跑旧配置。
2. 下次心跳杀旧实例：revision 不一致触发 Full reconcile，`load_round_configs` 取 `EnabledOnly`，已保存行被排除出 desired，`cleanup_stale_web_source_instances` 把还在跑的旧实例按 `keep_local_config=false` 硬删。
3. 新配置永不下发：下发入口只认启用行；disabled 行够不着。

即：**静默杀隧道 + 永不生效**。本次设计把它切成干净的三步；§9.1–9.3 已按下文落地。

## 2. 目标语义（三步）

```text
保存 → 落库（必须成功，才谈下发；不改变启用态）
    → 行仍为启用 + 客户端在线：直接热重载（Patch；底座变更自动降级重建）
    → 行仍为停用：只落库，不下发（等用户 Start / 再启用）
    → 行仍为启用 + 客户端离线：等下次在线
        ├─ 控制台点启动时推送（revision 比对，不一致则 overwrite）
        └─ 客户端本地自行启动：先用本地配置起，心跳后收敛覆盖
```

### 2.1 Save 与 Push 的关系（无独立 Push 按钮）

本设计**不**新增面向用户的独立「Push / Apply」API 或按钮：

| 场景 | 行为 |
|------|------|
| 启用中 + 在线 | Save = 落库 + 触发下发（notify reconcile） |
| 启用中 + 离线 | Save = 只落库；上线后由 Start / 心跳补齐 |
| 已停用（disabled） | Save = 只落库；**不**自动拉起，也不进 desired |

若产品日后需要「只存不下发」，另开显式开关/接口，不在本期范围。

## 3. 第 1 步：保存只落库

### 3.1 定义（已落地）

- 保存 = SQLite 持久化 + **一次**内存 fence bump（`bump_applied_config_revision` = `invalidate_managed_runtime_state`：`applied=None`、`pending=Full`、epoch++），**不改变既有行的启用态**，**不发任何设备写路径 RPC**（见 §4.3 只读 prepare 例外）。
- 控制台 upsert 同时 `clear_managed_config_revision`（DB target → `None`）。随后 heartbeat Full reconcile 走「无 revision」路径（`target.is_none()` 时仍检查并收敛 running web 配置），由 notify 立刻唤醒，不必等 3.5s 心跳间隔。
- DB 事务提交必须发生在 bump 之前，notify 唤醒必须在 bump 之后（同任务顺序 await，见 §8.1 之 2）。
- REST：**提交成功后只 bump（+ 启用在线时 prepare）+ notify 一次**，禁止再双踢。

> 注：早期草案写「target 推进、applied 不动」。控制台 Save 不写新的 managed revision 字符串，实际是 **清 DB revision + 清 applied + pending Full**；与 webhook Exact Set（写 target revision）不同路径，勿混用。

### 3.2 Upsert 与启用态（已落地，写死）

| 路径 | INSERT `disabled` | ON CONFLICT `disabled` |
|------|-------------------|-------------------------|
| 控制台 Save（`insert_or_update_console_save_network_config`） | 新建行：`true`（同事务，避免心跳抢先拉起）；已存在：不适用 | **不改**（保留 Stop/Start） |
| 通用 `insert_or_update_user_network_config` | `false` | **不改** |
| Managed / Exact Set / bootstrap（`sync_disabled_on_conflict=true`） | `false` | **强制 `false`（启用）** |
| Run+save | upsert 后另调 `update_network_config_state(..., false)` 显式启用 | — |

- Web 覆盖 `handle_save_network_config_with_source`：去掉 trait 默认的无条件 `disabled=true`。该默认是 GUI 本地语义（保存即停用），**必须保留给 GUI**。
- REST 走 `save_web_network_config`，不依赖 trait 入口做 bump/notify。

### 3.3 已停用行 / 新建行再保存（已落地）

- 行已是 `disabled=true`：upsert + bump，**不** notify、**不**自动 Start；响应 `pending` + `reason: disabled`。
- 品牌新建行：同事务 INSERT 为 disabled，响应同上；用户点 Start 后再推送。

### 3.4 现状

§3.1–3.3 已落地；剩余见 §9。

## 4. 第 2 步（在线且启用）：热重载

仅当保存后该行仍为**启用**且会话判定在线时进入本步；否则 Save 响应 `action: pending` + `reason`（见 §4.3），结束。

### 4.1 复用现有链条，不新建协议

热更新全链路已存在，只需接上：

- 设备接收端：`ConfigRpc.PatchConfig` → `apply_config_patch`（`easytier-core/src/management/instance_rpc/config.rs:22`）。
- 控制台 diff 端：`web_source_runtime_patch`（`easytier-web/src/client_manager/runtime_reconcile.rs`）。
  - 可热更：hostname、ACL、端口转发、proxy networks、relay 开关、DNS（有客户端版本门）、vpn portal 客户端、credentials；
  - 动底座（网络名/密钥、listeners/connectors、IP/DHCP、加密等，经 `hot_patch_base` 判定）→ 返回 `None` → 全量 `overwrite:true` 重建；
  - 无差异 → `Unchanged`，零操作。
- 执行与验证：`prepare_web_source_runtime_reconcile`（get 现状定 Patch/Run/Unchanged）→ `apply_web_source_runtime_reconcile`（执行后 get 验证收敛，`ensure_runtime_config_converged`）。

### 4.2 接法：唤醒 reconcile，不内联写路径 RPC（已落地）

保存成功并 bump 后 `notify` 该会话立刻跑一轮 reconcile（`notify_managed_runtime_state_changed`），**不在 save handler 里内联** `patch_config` / `run_network_instance`。理由：内联要和心跳 reconcile 抢 `mutation_fence`；唤醒复用串行、失败重试、版本门全部逻辑。

### 4.3 回显机制（已落地）

`prepare` 三态（Patch / Run / Unchanged）依赖读设备现状，与「save 不内联写 RPC」不矛盾：**允许一次短超时只读 prepare，禁止在 save 里 apply**。

DNS 等版本门：只读 prepare 复用**当前会话已有的** `client_version`（心跳 `easytier_version`）；**禁止**再调独立版本查询接口。

```text
Save HTTP 处理（启用 + 在线时）：
  1. DB upsert 提交（保留 disabled；新建行同事务 INSERT disabled）
  2. revision bump（一次；清 applied + pending Full）
  3. 短超时（≤ FAST 5s）调用 prepare（list + get + diff，不 patch/run）
       ├─ 成功 → 响应 action: patched | rebuilt | unchanged
       └─ 超时 / 读失败 → 响应 action: pending, reason: prepare_timeout
  4. notify 唤醒 reconcile（负责真正 apply；与步骤 3 的 action 提示独立）
```

约定（**写死单码**：只有 `action`；未收敛/未下发统一用 `pending` + `reason`，不另设 `idle`）：

| 响应字段 | 含义 |
|----------|------|
| `action: unchanged` | prepare 认为已收敛，仍 notify（防 TOCTOU） |
| `action: patched` | prepare 认为可热更；实际 apply 由 reconcile 完成 |
| `action: rebuilt` | prepare 认为需 overwrite 重建；实际 apply 由 reconcile 完成 |
| `action: pending` | 未当场得到 prepare 结论或本轮不下发；DB 已是真理 |

`reason`（仅 `action=pending` 时必填）：

| `reason` | 何时 |
|----------|------|
| `disabled` | 行仍为停用（含新建），只落库 |
| `offline` | 无会话 / `!runtime_is_current`（半开、superseded、无心跳） |
| `prepare_timeout` | 只读 prepare 超时或读失败 |

- **失败不报错挡保存**：DB 已提交则 HTTP 仍 200；`action=pending` + 文案「已保存，同步中」（见 §8.1 之 4）。
- **前端**：以 `action` 提示是否可能断流（`rebuilt`）；`pending` 用 `reason` 区分文案；最终是否收敛以会话/实例状态为准，不阻塞保存按钮。
- **不做**：Save 内联 apply；也不做「先返回 pending、另开 WebSocket 推送 prepare 结果」作为本期必选项。

## 5. 第 2 步（离线或未启用）：等上线 / 等启用

### 5.1 控制台点启动时推送——已存在，不用做

Start 本来就做 revision 比对 + 健康探测（`client_manager/mod.rs` soft-start），不一致则 `overwrite` 下发。离线期间多次保存只保留最后一次 DB 配置 + fence fence，可接受。

### 5.2 客户端本地自行启动——不等不拉，用现有收敛覆盖

- 不新增设备→控制台的 pull RPC。版本号已在每次心跳回包里（`managed_config_revision`）；pull 会和控制台的推打架，不值得。
- 效果等价性：设备上线后首次心跳，reconcile 发现 DB 有**启用**配置而设备没在跑，`run_missing`（`overwrite:false`）自动推下去。
- 本地先跑了旧版又被覆盖：设计内行为（单写者：控制台赢）；覆盖写 drift 日志（已落地，见 §8.1 之 3）。

## 6. 客户端启动时的版本比对规则

“连上用新的，连不上用旧的”成立，但按启动类型拆两种实现：

### 6.1 开机自启（GUI/desktop、ohrs）→ 推迟自启，预算等待

```text
boot → 连控制台（预算 T，建议 5s，非 20s 直连超时）
  ├─ 等到首次心跳回包：比 revision；旧→ 不抢跑，等紧随的 reconcile 推新版；
  │   已最新 → 起本地版
  └─ 超时/不可达：直接起本地现配置；上线后心跳收敛兜底
```

T 是上限不是目标（首次回包通常百毫秒级），超时不续杯。用“等推”代替“拉”，零新协议。

**已落地（服务态 / `easytier` CLI `--config-server`）**：
1. 启动 WebClient 后 `wait_first_heartbeat(5s)`。
2. 比对待持久化 revision（state dir 下 `managed_config_revision`）与心跳回包：
   - 相同或双方皆空 → 起本地 toml（含 Web 源）；
   - 否则 → **跳过** `source=web` 的 boot start，等 reconcile 推；User 源仍立即起。
3. 超时/不可达 → 全部按原样起本地。
4. 持久化 revision **仅**在「本地已与 console 对齐」时写入：boot 判定可起本地、reconcile/run 成功 apply、或本地 report 成功。心跳本身只更新内存，避免 defer 后崩溃被误判为已最新。

ohrs/FFI **无** OS 级 boot 环；显式 `run_network_instance` **不**套 5s 门（§6.2）。

### 6.2 用户显式启动（FFI 点运行、GUI 手动启用）→ 不等，立即起本地版

显式动作拼响应，阻塞等网络是最差体验。保持本地立即起 + 后台收敛覆盖。

**已落地提示**：`on_managed_config_overwrite`（overwrite 成功后，不回滚）：
- FFI：`managed_config_synced_overwrite` 回调事件
- ohrs：同名 drain 事件
- GUI：Tauri 事件 + toast（`managed_config_synced_overwrite`）

## 7. 现状对照表

| 能力 | 状态 |
|---|---|
| 保存落库 + 去掉禁用副作用（§3.2） | **已落地** |
| 单次 bump（§3.1；禁止双 invalidate） | **已落地** |
| Upsert 保留 disabled；新建同行禁用；managed 强制启用（§3.2） | **已落地** |
| Patch 计算/执行/验证/版本门 | 已存在（§4.1） |
| save 后 notify 唤醒 reconcile（§4.2） | **已落地** |
| 保存回显 action（只读 prepare，§4.3） | **已落地** |
| drift 覆盖日志（§8.1 之 3） | **已落地** |
| Start 时 revision 比对下发 | 已存在 |
| 上线后 run_missing 自动补齐 | 已存在 |
| 独立 Push API / 按钮 | **不做**（§2.1） |
| 在线判定对齐 `runtime_heartbeat_is_current`（§8.1 之 1） | **已落地**：`Session::runtime_is_current`（running + 非 superseded + 存储心跳 owns session）；半开 → `offline` |
| 失败上限与人工态（§8.1 之 4） | **已落地（后端）**：仅 **Web/managed** 失败累加 streak；≥5 轮 `reconcile_paused`；Save/Start bump 清零恢复。无独立「重试」API/UI |
| 归属确认接管（§8.1 之 5） | **已落地（前端）**：User 源 Save **与** Run（`saveAndRun`）均确认接管。未做硬置灰 |
| 自启推迟门 T（§6.1） | **已落地**（T=5s；revision 文件绑定 machine-id；仅 Web apply/patch/成功 report 落盘；console `None`=空库起本地不 defer） |
| 显式启动同步提示（§6.2） | **已落地**（overwrite tip 事件；显式 Start 仍立即起本地） |
| 设备 pull RPC | 不做（§5.2） |

## 8. 边界定义

### 8.1 正确性边界（不定会丢配置或脑裂）

1. **在线判定以会话为准**（已落地）：Save 用 `Session::runtime_is_current`（`is_running` + `!is_superseded` + 存储心跳满足 `runtime_heartbeat_is_current_locked`）。半开/无心跳/`superseded` → `reason: offline`（不再误进 prepare）。墙钟「心跳年龄」仍由 session rx timeout 间接保证。
2. **DB 与 revision 的原子序**：`upsert 提交 → bump（一次）→（可选只读 prepare）→ notify`，顺序锁死，禁止并发拆分或恢复双 invalidate。
3. **单写者裁决 + 留痕**：控制台 DB 版无条件赢；覆盖时写 drift 日志（`web-managed config drift: ...`）。**已落地。**
4. **失败不回滚 DB**（已落地后端暂停）：超时/patch 拒收/新版起不来，`applied` 不动；连续 **managed** 失败 `MANAGED_RECONCILE_FAIL_LIMIT`（5）轮 → `reconcile_paused`（User 源失败不计入），**不**清实例侧错误。Save/Start 的 `invalidate_managed_runtime_state` 清零 streak/pause 后恢复。**不自动回滚旧版**。Save HTTP 在 DB 已提交时不因后续同步失败而 5xx。专用人工重试 API/UI 可后续加。
5. **归属边界**（已落地前端确认）：`NetworkMeta.source`；User 源点保存 **或** 运行（Save→Web）先确认接管。未做硬置灰；未做后端 `confirm_takeover` 强制闸。

### 8.2 体验边界

6. **保存后立即 Start 的竞态**（代码已共享 epoch fence）：Start/Stop 在运行时变异前 `invalidate`（bump `runtime_config_epoch`），与 reconcile 的 `begin_managed_runtime_mutation` 对齐。**联调仍必测**「保存 1 秒内点启用」；不可仅凭代码改动标完成。
7. **patch 字段必带版本门**：新增 patch 字段必须加客户端版本门 + 测试。宁可整单降级重建，不发“收了不收敛”的 patch。
8. **回显与真实收敛解耦**：`action: patched|rebuilt` 只表示 prepare 预测；隧道是否已按新配置跑，以 reconcile 完成后的实例状态为准。GUI 本地 Save 回 `{action:'unchanged'}`（无下发语义）。

## 9. 落地顺序

1. ~~地基：§8.1 之 2/3 + §3（save 去禁用、单次 bump、upsert 启用态）~~ **已完成**
2. ~~主链：save 后 notify + §4.3 只读 prepare 回显~~ **已完成**
3. ~~护栏：失败上限暂停、归属确认、在线判定收紧（§8.1 之 1/4/5）~~ **已完成**（重试专用 API/UI、后端 takeover 闸可选后续）
4. ~~启动规则：自启推迟门 T + 显式启动同步提示（§6）~~ **已完成**（含 machine-id 绑定、patch 落盘、空控制台 `None` 起本地）
5. 体验：Save∥Start **联调**、版本门规矩（§8.2 之 6/7）——fence 代码已齐，联调未完成前勿标已完成。
