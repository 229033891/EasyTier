# 控制台删除后本地配置残留：处理方案

## Status

- Status: **In progress**（P0–P2 代码已落地；§5 验收待人工回归）
- 日期：2026-10-12（P0-4 落地同日；二次修订：`is_disabled()` 落地 + `D`/`D′` 拆分，见 §2.1 / §4.1）
- 范围：Web 控制台删除 / 停用 / Save 接管之后，Windows（及同语义桌面）设备与 GUI 缓存上仍可见的「幽灵配置」
- 归属：[`web-evolution.md`](./web-evolution.md) 体验与托管一致性；删除权规则见 [`config-delete-authority.md`](./config-delete-authority.md)
- 关联 Current：[`../current/desktop-gui-and-config-server.md`](../current/desktop-gui-and-config-server.md)、[`../current/web-managed-config.md`](../current/web-managed-config.md)
- 关联 Roadmap：[`console-save-and-push-semantics.md`](./console-save-and-push-semantics.md)、[`client-web-console-interaction.md`](./client-web-console-interaction.md)
- 索引：[`../README.md`](../README.md)

---

## 1. 背景与目标

控制台删除成功后，现网仍可能出现：

- Windows GUI `localStorage.networkList` 里仍有已删实例（服务模式 GUI 关掉期间被删，再开 GUI 被 rehydrate）；
- 设备 toml / 运行实例仍在（删除 RPC 超时、离线、`ClientNotFound`）；
- 缓存 `source` 被标成 `user` / `legacy`，现有 prune 故意放过；
- Save 接管新建 **disabled** Web 行时，设备仍在跑 → 曾被心跳 stale cleanup 误杀（已修 keep 集）；修完后可能短暂「控制台停用、设备仍跑」；
- **反向误删**（曾）：控制台仅**停用**某 Web 行，GUI 冷启动按 `R` prune 误清缓存 → **P0-4 已改以 `W` 判定**（见 §4.1）。

目标：按 **归属分层** 收敛残留，避免「一律全量覆盖本地」或「一律保留幽灵」。

非目标：改 VPN 数据面；离线 tombstone 全量方案（删除权文档本期明确不做，见 §5 可选）。

---

## 2. 原则（定稿）

| 归属 | 权威 | 残留处理 |
|------|------|----------|
| **Web 托管** | 控制台 SQLite | 以控制台为准收敛；本地多出的 Web 副本应清除或标成待清理 |
| **User 本地** | 设备 / GUI | 控制台删除**不**动；除非用户本机删除或主动「接管 / 上报」 |
| **中间态**（库无设备有、库 disabled 设备仍跑、源标错） | 可见 + 可操作 | UI 标明；提供清除 / 接管 / 忽略；禁止静默误删 User |

三句硬约束：

1. **连上控制台 ≠ 每次启动全量覆盖本地**（心跳 reconcile：User 不覆盖；Web 仅缺跑 / 漂移 / 控制台 Start·Run 时收敛）。
2. **删除权**仍按 [`config-delete-authority.md`](./config-delete-authority.md)：bound + Web → 仅控制台硬删。
3. **GUI 缓存不是权威**；prune 以控制台列表 **W** 为权威（P0-4 已落地）；P1 再补控制台侧残留可见与动作。

### 2.1 四个「已知集」必须分清（本文核心，勿再混淆）

残留判定全部围绕「某 id 在不在某个集合里」，而工程里有**来源不同、含义不同**的集合。混用它们是本类 bug 的根因（见 §4.1）：

| 集合 | 来源 | 含义 | 刷新时机 |
|------|------|------|----------|
| **R** = running ids | **设备 RPC** `list_network_instance`（`remote_client.rs:129-133`） | 设备当前**在跑**的实例 | 每次 list |
| **D** = disabled ids | **进程侧 storage** `list_network_configs(All) − R`（`remote_client.rs:137-158`） | 该 storage 记录过、但没在跑的实例 | 每次 list |
| **D′** = storage `disabled=true` | `PersistentConfig::is_disabled()`（`remote_client.rs:441-447`） | storage 自己记的**启用态**（DB / GUI 镜像），**可与 R 相交** | 每次 list（并入 `db_disabled_inst_ids`） |
| **L** = GUI 缓存 | 前端 `localStorage.networkList`（`event.ts:28-37` ← `save_configs` 事件回写） | GUI **上次**看到的全部行（含 web / user / legacy） | 进程内 hook 触发时 |
| **W** = 控制台 Web 行 | 控制台 SQLite（该机器名下 `source==Web` 的全部 id） | Web 行的**权威全集**（含 enabled / disabled） | 心跳 `HeartbeatResponse.web_owned_instance_ids*`；GUI 经 `GetConfigServerStatus` |

关键事实（**2026-10-12 实测，同日二次修订**）：

- **`D` 与 `D′` 是两回事，别混**：`D`（`disabled_inst_ids`）是「storage 有、设备没跑」的差集；`D′`（`db_disabled_inst_ids`）是 storage 自己记的 `disabled=true`。Save 接管新建 disabled Web 行、设备仍在跑 → 该 id **同时** ∈ `R` 和 ∈ `D′`，但 **∉ `D`**。残留 UI 判「库停用仍在跑」必须用 `D′`，不能用 `D`。
- **`is_disabled()` 已落地**（`PersistentConfig` trait 默认 `false`）：`GUIStorage` 在 `list_network_configs` 时把 `enabled_networks` 镜像进 `disabled` 字段；web SQLite 直读 `Model.disabled`；core 侧默认 `false`（无 enablement 概念）。
- **GUI 桌面模式 `D` 的冷启动限制仍在，但已无害**：`GUIStorage.network_configs` `new()` 即空 `DashMap`（`manager.rs:181-188`），只被前端 `load_configs`（= `sendConfigs`）填充；`initWithMode` 里 **list 在 sendConfigs 之前** → **冷启动首次 list 时 `D = ∅`**。但该次 list **只喂 `knownDeviceInstanceIds`（供 lastInstanceId 恢复），不再驱动 prune**；真正 prune 在心跳后 `maybePruneWebConfigsFromConsoleW` 内重跑 list，那时 storage 已填 —— `D` 与 `D′` 均可用。
- 结论：**残留/prune 判定不能只靠 `R`**（会误删停用行）。企业级解：以**控制台列表 W**（该机器名下全部 Web 行 id）为权威 prune；`R`/`D`/`D′` 作辅助（`D′` 供残留徽章）。

---

## 3. 残留场景矩阵

> 集合记号见 §2.1：**R** = 设备在跑、**D** = storage 已停用、**W** = 控制台该机器全部 Web 行、**L** = GUI 缓存。判定原则：**只删「可证属于 Web 且已不在 W」的行**，其余一律保留待用户处置。

| # | 场景 | 今日表现 | 期望 |
|---|------|----------|------|
| A | 控制台删成功（设备 RPC `keep=false` + DB 删）+ GUI 重连 | 冷启动 `D=∅`，knownIds≈`R`，prune 清掉「不在 R 的 web 行」——**恰好**清对了该删的 | 改为以 **W** 为准 prune（P0，见 §4.1）；不能靠 `R` 巧合 |
| B | 删除 RPC 超时 / 设备挂住 | 整单失败或库未删；设备仍在 | 失败可观测 + 可重试；库已无则心跳 stale 清设备 Web |
| C | 设备离线时删 | 整单失败，DB 行仍在（删除权文档定稿） | 保持；可选后续 tombstone（非本期） |
| D | 缓存标成 `user`/`legacy` 的前 Web 幽灵 | prune 故意保留 | P1：控制台已无且可证曾 Web → 提示清除或可配置自动清 |
| E | 纯本地草稿（从未上控制台） | 保留 | 必须保留 |
| F | Save 接管 INSERT disabled，设备仍跑 | keep 集已含 disabled，不再误硬删 | P2：Save 后对齐启停（自动 Start 或显式 Stop 设备） |
| G | 设备有、控制台无（Web 源） | Full reconcile stale cleanup | 保持；控制台 UI 标「仅设备残留」+ 一键清除（P1） |
| H | list RPC 失败 | 不得当「设备空集」prune / 清 `lastInstanceId` | 已修：仅 list 成功才更新 knownIds |
| **I** | **控制台仅「停用」某 Web 行（未删）+ GUI 冷启动** | **`D=∅` ⇒ knownIds 只有 `R`；该行不在 `R`（没在跑）⇒ 被 prune 误删，GUI 缓存丢失** | **P0 必修**：以 `W`（控制台列表）判定，`W` 里有的行**必须保留**（见 §4.1） |
| J1 | list RPC **失败**（服务未起 / 真离线） | 不传 knownIds → **不 prune**（场景 H） | 保持 fail-safe |
| J2 | list **成功**但 `R=∅`（服务在、实例全停用） | knownIds=`∅`，按 R 会清掉**所有** web 缓存（含仍应保留的停用行） | P0：只认 `W`；接 W 前应停用按 R 的 prune（§4.1 过渡） |

---

## 4. 已落地（战术，2026-10-12）

| 项 | 行为 | 代码锚点（易漂移） |
|----|------|-------------------|
| GUI web prune **只认 W**（P0-4） | 心跳带 `web_owned_instance_ids_valid` + ids；客户端缓存；`GetConfigServerStatus` 透出；GUI 仅在 `connected && valid` 时 prune。会话建立前/结束后清空 W；`!connected` 强制 `valid=false`。`id ∈ W` 必留；`source==web && id ∉ W` 才删。`initWithMode` **不**再按 `R` prune | `web.proto` / `api_manage.proto`；`session.rs` 填 W；`web_client.rs` 缓存+clear；`process_rpc.rs` / GUI DTO；`prune_web_configs.ts`；`backend.sendConfigs`；`index.vue` `maybePruneWebConfigsFromConsoleW` |
| list 失败安全 | 失败不覆盖 `knownDeviceInstanceIds`，避免误清 `lastInstanceId`；list 的 R/D **不**参与 web 缓存 prune | `index.vue` |
| 选中态 | `clientRunning` 恢复时仅当 id ∈ knownIds 才恢复；否则 `clearLastNetworkInstanceId` | `index.vue` |
| stale cleanup keep 集 | 用 **全部** Web 行（含 disabled），禁止 EnabledOnly 把 Save 接管行当 orphan 硬删 | `runtime_revision.rs` `cleanup_stale_web_source_instances` |
| Start 落库 | 启用路径：**upsert → soft-start/run → update state**（upsert 在 run 之前，避免设备已跑、库无行） | `ClientManager::handle_update_network_state` |
| 单测 | prune 按 W（含停用必留、空 W 清 web）；cleanup keep 集含 disabled；残留分类 / 前 Web 幽灵 | `prune_web_configs.test.ts`；`config-residual.spec.ts`；`former_web_ghosts.test.ts`；`managed_config.rs` tests |
| 控制台残留 UI（P1） | list 带 `db_inst_ids` / `db_disabled_inst_ids`；徽章 + 清除/接管/忽略（仅 web-console） | `remote_client.rs` list；`RemoteManagement.vue`；`config-residual.ts` |
| storage `disabled` 语义（P1 前置） | `PersistentConfig::is_disabled()`（默认 `false`）→ list 单次 `All` 派生 `db_inst_ids` + `db_disabled_inst_ids`；GUI 镜像 `enabled_networks`，web 直读 `Model.disabled` | `remote_client.rs` trait+list；`manager.rs` `GUIConfig.disabled`；`user_running_network_configs.rs` |
| GUI 误标前 Web 幽灵（P1） | W 有效后对 user/legacy ∩ 设备 meta=Web ∩ id∉W **提示**清除 | `index.vue` `maybePromptFormerWebGhosts`；`former_web_ghosts.ts` |
| Save 启停对齐（P2） | Save 返回 `pending+disabled` 且设备该 id 仍在跑 → 自动 Start；未跑保持停用 | `RemoteManagement.doSaveNetworkConfig`；`shouldAutoStartAfterDisabledSave` |
| 删除失败可重试（P0-2） | 设备先删失败则保留 DB；toast + Retry 确认再调 `delete_network` | `isRetryableDeleteApiError`；`offerRetryOnDeleteFailure` |

### 4.1 历史缺陷（已修）：曾用 `R` 当 prune keep 集

**根因（当初）**：桌面 GUI 冷启动时 `disabled_inst_ids`（`D`）来自空的 `GUIStorage` ⇒ `knownIds ≈ R`。若按 `R` prune，会误删「控制台仅停用、不在跑」的 web 缓存行（场景 I/J2）。

**修复（已落地，方案 a）**：控制台列表 **W** 经心跳下发；GUI prune **只认 W，不并 R**。**未选**方案 b（设备 toml 扫 `D`）作为 prune 主判据。

**补充修复（同轮）**：`PersistentConfig::is_disabled()` 落地，`GUIStorage` 镜像 `enabled_networks` → `db_disabled_inst_ids`（`D′`）语义可用。这是**残留徽章**（「库停用仍在跑」）的必要项，与 prune 主判据（W）正交；`D`（差集）与 `D′`（storage 自记）不可混用（见 §2.1）。

仍开放：离线 tombstone / 补偿队列（明确不做）；P0–P2 待人工回归。

---

## 5. 目标方案（分层落地）

### P0 — 删除 / 停用可收敛（正确性）

1. 控制台 Delete：设备 RPC 失败时 UI / API **明确失败**（现有：先设备后库，保持）。
2. 失败码稳定、可重试（**已落地**）：后端整单失败保留 DB；前端删除失败对 `rpc_timeout` / `client_not_found` / tunnel 等弹出 **Retry**（`offerRetryOnDeleteFailure`）。
3. 心跳：库已无该 Web 行、设备仍报 Web 源运行 → **stale cleanup**（已有）；确保 metas 失败时 fallback 不得误删「库内 disabled 仍在跑」的实例（keep=全 Web 行，已落地）。
4. **GUI prune 以控制台列表 `W` 为准（§4.1）**：**代码已落地**。`id ∈ W` 必留；`id ∉ W` 且 `source==web` 才删；`W` 无效则不 prune。

验收：

- [x] 在线删 Web：设备 purge + DB 无行；GUI 在 W 有效后缓存无该 `web` id（只认 W，不依赖 `R`）—*代码路径*；待人工回归。
- [x] **控制台仅停用某 Web 行 → GUI 连上后该行仍在缓存**（场景 I）—*代码路径*；待人工回归。
- [x] **W 无效 → 不 prune**（J1 类 fail-safe）；**空 R 但 id∈W → 不停用 web 行**（J2）—*代码路径*；待人工回归。
- [x] 删 RPC 超时：DB 仍在（整单失败）+ 显式重试入口 — *代码路径*；待人工回归。
- [x] Save 接管 disabled 后下一心跳：**不得**硬删仍在跑的实例（stale keep=全 Web，已落地）。

### P1 — 残留可见 + 二次权威（体验 / 企业）

1. **控制台设备页**（**代码已落地**）：list 返回 `db_inst_ids` / `db_disabled_inst_ids`；实例「仅设备残留」或「库已停用仍在跑」→ 徽章 + 动作：`清除设备副本` / `接管到控制台` / `忽略`（`RemoteManagement`，仅 `hostSurface=web-console`）。
2. **误标 `user`/`legacy` 的前 Web 幽灵**（**代码已落地**）：W 有效后，对本地 user/legacy 且设备 meta 仍为 Web、且 id∉W 的行 **提示**清除；**禁止默认静默删纯本地草稿**。
3. Web 源残留确认后硬清（`keep=false`）；User 源确认后清或接管；库停设备跑 → 清除=Stop，接管=Start。

> 注：原「GUI prune 第二权威」已上移 P0（§5 P0-4）——它是正确性问题，不仅是体验。

验收：

- [x] 误标 `user` 的前 Web 幽灵可被提示清除，默认不静默删纯本地草稿 — *代码路径*；待人工回归。
- [x] 运维能在控制台看见并处理设备孤儿（徽章 + 清除/接管/忽略）— *代码路径*；待人工回归。

### P2 — Save / 启停与运行态对齐

1. Save 接管时若设备该 id 仍在跑：**自动启用（Start）**（`doSaveNetworkConfig`：`pending+disabled` 且 `wasRunning` → `update_network_instance_state(false)`）。未在跑则保持 disabled（`newNetwork` / 停用再存）。P1 残留条仍可作兜底。
2. 文档与 tip：Start ≠ 仅唤醒（`start_network_tip` 已写明）；破坏性覆盖二次确认已按产品去掉 footer `confirm_start_overwrite`——以代码为准。

验收：

- [x] Save 接管后控制台启用态与设备运行态对齐（自动 Start）— *代码路径*；待人工回归。失败时 toast「仍在跑 / 对齐失败」+ P1 残留条。

### 明确不做（除非另立项）

- 启动时无差别「控制台配置整包覆盖全部本地」。
- 把所有 `user` / `legacy` 当幽灵删除。
- 本期离线 tombstone / 上线补删（见删除权文档 §5）。

---

## 6. 与现有子系统的关系

```text
控制台 Delete (Web)
  → 设备 RPC keep=false → 成功再删 SQLite
  → 失败：整单失败（可重试）

心跳 Full reconcile
  → stale cleanup：running Web − 库内全部 Web ids → keep=false 清孤儿
  → EnabledOnly：补跑 / patch / 条件 overwrite（不碰 User）

GUI 重连
  → 拉取 W = 控制台该机器全部 Web 行 id（P0；拉不到 → 不 prune）
  → prune 缓存：仅 `source==web && id ∉ W` 才删（**只认 W，不并 R**）
     · 并 R 会把「控制台已删、设备仍跑」的幽灵留在 GUI；设备孤儿交给心跳 stale cleanup
  → list 的 R/D 只作运行态展示 / lastInstanceId，不参与 web 缓存判定
```

| 文档 | 分工 |
|------|------|
| [`config-delete-authority.md`](./config-delete-authority.md) | **谁可以删**（bound + Web → 仅控制台） |
| [`console-save-and-push-semantics.md`](./console-save-and-push-semantics.md) | Save / Push / disabled 插入语义 |
| **本文** | 删完之后 **本地还剩什么、怎么收敛** |
| [`client-web-console-interaction.md`](./client-web-console-interaction.md) | 超时、Start 覆盖、会话抖动 |

---

## 7. 决策记录

| 日期 | 决策 |
|------|------|
| 2026-10-12 | 残留按 Web / User 分层；不以「启动全量覆盖」解决幽灵 |
| 2026-10-12 | 战术：设备 list 成功后 prune GUI `web` 缓存；list 失败不得空集 prune |
| 2026-10-12 | stale cleanup keep 集 = 全部 Web 行（含 disabled），修复 Save 接管后误杀 |
| 2026-10-12 | 企业级下一步：控制台列表二次权威 + 残留 UI + 删除失败可重试；Save 启停对齐为 P2 |
| 2026-10-12 | 不把 User/legacy 草稿纳入自动硬删 |
| 2026-10-12 | **复核修正**：区分 R/D/W/L 三个已知集（§2.1）；识别 prune「knownIds 只有 R」缺陷（§4.1）；把「控制台列表二次权威」由 P1 上移 **P0**；新增场景 I/J 与回归验收 |
| 2026-10-12 | prune 判定**只认 W**（不 `W∪R`）；场景 J 拆成 J1（list 失败）/ J2（空 R）；接 W 前停用按 R 的自动 prune（`initWithMode` 已落地） |
| 2026-10-12 | **P0-4 落地**：心跳下发 W → 客户端缓存 → `GetConfigServerStatus` → GUI `maybePruneWebConfigsFromConsoleW`；§4.1 方案 a |
| 2026-10-12 | **P1 落地**：list 增补 `db_inst_ids`/`db_disabled_inst_ids`；控制台残留徽章与清除/接管/忽略；GUI 误标前 Web 幽灵提示清除 |
| 2026-10-12 | **P2 落地**：Save `pending+disabled` 且设备仍在跑 → 自动 Start 对齐；停用未跑不 Start；tip 标明 Start≠仅唤醒 |
| 2026-10-12 | **P0-2 落地**：删除失败（timeout/offline/tunnel/db）显式 Retry；非离线 tombstone 队列 |
| 2026-10-12 | **复核优化**：会话建立前 / 结束后清空客户端 W 缓存；`!connected` 时 status 强制 `valid=false`，避免重连心跳前用旧 W prune；Start upsert 先于 run；list 一次 All 派生 `db_disabled` |

---

## 8. 修订方式

1. P0/P1 落地后勾选 §5 验收，并把稳定行为写入 `current/desktop-gui-and-config-server.md`（或 web-managed-config 短节）。
2. 若恢复或再次取消 Start 覆盖确认，同步 [`client-web-console-interaction.md`](./client-web-console-interaction.md) §5 与决策表。
3. 离线 tombstone 若立项，先改 [`config-delete-authority.md`](./config-delete-authority.md)，再回链本文场景 C。
