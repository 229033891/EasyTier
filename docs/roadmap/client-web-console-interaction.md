# 客户端 ↔ Web 控制台交互优化

## Status

- Status: **Roadmap**
- 最近审阅：2026-10-10（P0 A–D + P1 已落地；P2 / 可选项另议）
- 归属：[`web-evolution.md`](./web-evolution.md) 阶段 B（控制台体验 / REST）+ 客户端连接可靠性
- 相关 Current：[`../current/desktop-gui-and-config-server.md`](../current/desktop-gui-and-config-server.md)、[`../current/web-managed-config.md`](../current/web-managed-config.md)
- 相关运维诊断：[`web-console-runtime-diagnostics.md`](./web-console-runtime-diagnostics.md)（控制台侧预定 vs 实际；**不替代**本文客户端交互项）
- 索引：[`../README.md`](../README.md)

---

## 1. 背景与目标

控制台与节点通过 config-server（默认 `:22020`）+ 控制台 REST/proxy-RPC 协作。现网已出现：

- 客户端 `tcp://` 连仅 UDP 监听的旧服务端 → 连接失败（10061/10060）
- 控制台「启动网络」覆盖客户端本地配置，与本地运行/编辑态无关
- UI「操作超时」（约 5s），设备侧可能仍在执行
- NAT 换源端口后设备间歇离线

目标：按优先级收敛 **正确性 → 可靠性 → UX**，减少假失败与静默覆盖；改动尽量可「只升 web」落地，协议 / 节点侧项单独标注（**不在**「只升 web」默认路径）。

---

## 2. 交互现状（摘要）

```text
Client (--config-server udp|tcp|ws://host:22020/<token>)
  → 拨号：配置 scheme 优先，失败后 udp↔tcp 自动降级（ws/wss 不改写）
  → WebClient 心跳（默认 3.5s / 15s）
  → easytier-web ClientManager（会话按 machine_id + session_epoch 接管；client_url 作路由键）
  → SQLite + heartbeat reconcile（Full/Patch → Run/Patch）
控制台 UI → REST → proxy-rpc / network API → 同一会话（BaseController 默认 5s）
```

说明：会话表主索引已是 `machine_id`（`user_clients_map`），`session_epoch` 保证同机器新会话单调接管；`client_url` 作路由键，NAT 换端口时由 `previous_client_urls` + 活会话扫描兜底（见 §3.1 P0-4）。

| 环节 | 关键行为 | 代码锚点（易漂移） |
|------|----------|-------------------|
| 拨号 | 配置 scheme 优先；失败后 `udp↔tcp` 降级（每候选约 9s） | `config_server_dial_candidates` / `dial_config_server_with_scheme_fallback`；`ConfigServerConnector`（native + WASI） |
| 服务端监听 | 默认 `udp,tcp` 同端口 | `easytier-web/src/main.rs` `--config-server-protocol` |
| 会话索引 | **机器维度已是主索引**（`user_clients_map`）；`ClientInfo.previous_client_urls` 保留换端口旧 URL；`client_sessions` 仍按 `client_url` 键 | `storage.rs`；`ClientManager::get_session_by_machine_id` / `list_machine_by_user_id` |
| 会话接管 | 按 `session_epoch` 单调接管，旧 epoch 不覆盖新 epoch | `storage.rs` `bind_managed_runtime_state` / `update_session_client` / `is_session_superseded` |
| 心跳 | 间隔/超时可由服务端策略下发；客户端有钳制 | `HeartbeatPolicy`；`web_client` heartbeat 任务 |
| 控制台启动网络 | `overwrite: true`，配置来自 **SQLite** | `RemoteClientManager` 默认实现：`handle_update_network_state` / `handle_run_network_instance_with_source`；节点侧 `process_rpc::run_network_instance` |
| 心跳自动补跑 | `overwrite: false`（已健康可跳过） | `runtime_revision` / reconcile |
| 管理 RPC 超时 | 默认 **5000ms** | `BaseController::default`；`restful/rpc.rs` proxy-rpc；`RemoteClientManager` 各 `handle_*` 亦用默认 5s |

说明：服务端默认双听已缓解**新装** scheme 不匹配；客户端另有 `udp↔tcp` 运行时降级。`ws`/`wss` 写错或双向都不通时仍会失败。

---

## 3. 问题分级与对策

### 3.1 P0 — 正确性 / 现网痛点

#### P0-1 启动网络强制覆盖客户端配置

- **现象**：控制台 Start / Run 曾总是 `overwrite: true`，用库内配置写回客户端实例文件并重启路径；与设备是否已在跑、GUI/本地是否在编辑无关。
- **对比**：心跳 reconcile 补跑多用 `overwrite: false`。
- **现状（已落地）**：
  1. `ClientManager::handle_update_network_state`：实例已在跑且无 error、managed revision 允许时 **软启动**（只改 DB disabled，不 `run`/`overwrite`）；否则仍 `overwrite: true` 并作废 applied revision。REST 不再在 update-state 前后无条件 invalidate。
  2. 前端 Start / 首次 Run：二次确认 `confirm_start_overwrite`；重跑确认文案标明强制覆盖。
  3. tip 文案写清：权威在 web-owned 场景是控制台库，Start ≠「仅唤醒」。
- **剩余**：现网联调；`run_network` 显式重建路径仍为 `overwrite: true`（有确认）。
- **兼容 / 落地路径**：节点侧已支持 `overwrite`；改动仅 web `ClientManager` 覆盖 + 前端（与 [`web-evolution.md`](./web-evolution.md)「只升 web」一致）。

#### P0-2 管理操作共用 5s RPC 超时

- **现象**：启停网络、写盘、TUN、重 reconcile 与轻量查询共用 `BaseController` 5s → UI「操作超时」，易误判失败并重复点击。
- **现状（已落地 service 层）**：错误码侧已分级 —— `convert_rpc_error` 稳定输出 `rpc_timeout` / `rpc_execution` / `rpc_tunnel` / `rpc_shutdown` / `rpc_error`（`restful/mod.rs`），`client_not_found` 另有独立 code；前端 `formatApiErrorDetail` / `classifyApiError` 已按 code 映射为可操作文案。**proxy-rpc** 按 method 分级（快 5s / 慢 60s，见 `client_manager/rpc_timeout.rs`）；`ClientManager` 已覆盖 `handle_run_network_instance_with_source` / `handle_update_network_state` / `handle_remove_network_instances` / `handle_collect_network_info` 使用慢超时。
- **剩余**：现网联调确认启停 >5s 不再假超时；确认「已超时请刷新」与「确定失败」两条路径都走到（`error_timeout` vs 其余 code）。
- **兼容**：仅超时策略；不改节点协议。已「只升 web」落地。

#### P0-3 客户端无协议自动降级

- **现象**：URL 单 scheme；对端只听另一种或防火墙只通一边时，退避重试同一 scheme，永不切换。
- **现状（客户端降级已落地）**：
  1. 拨号失败 / 单候选超时后尝试对端常见 scheme（**udp↔tcp**；`ws`/`wss`/`ring` 不改写）。
  2. 实现：`dial_config_server_with_scheme_fallback`（每候选约 9s，落在外层 20s 安全网内）；native `ConfigServerConnector` + WASI 共用。
- **剩余（可选）**：服务端心跳广告 `listening_schemes`（控制台诊断已有 `config_server_listening_schemes()`，**尚未**下发）；文档/诊断继续强调服务端双听。
- **兼容**：纯客户端运行时行为，**不改**配置 URL 格式、不改 web 协议；需发节点/GUI 生效。

#### P0-4 `client_url` 变化导致间歇离线

- **现象**：NAT 换源端口 → 新 `client_url` → 新会话；旧会话被 supersede；中间窗口 `list_machines` / 管理 RPC 见离线或 `client_not_found`。
- **现状（路由层已收敛）**：会话表按 machine_id 主索引 + `session_epoch` 接管（见 §2）。另已落地：
  1. `ClientInfo.previous_client_urls`：换端口时保留至多 2 个旧 URL；
  2. `get_session_by_machine_id`：authorized 主 URL → `route_client_urls`（含当前/旧 URL）→ 扫描仍在跑且已 bind 的会话；
  3. `list_machine_by_user_id`：若有活会话则返回其当前 URL（合并视图）。
- **剩余（可选）**：UI「重连中」态（前端目前无此状态）。
- **兼容**：仅 web 路由/查询层；客户端已带 `machine_id`。

---

### 3.2 P1 — 可靠性 / UX（**已落地**）

| ID | 项 | 现状 |
|----|----|------|
| P1-1 | 脏编辑 vs 服务端推送 | 脏草稿不静默覆盖；`GET managed-config-revision` 轻量探针；确认框一键重载 / 继续编辑 |
| P1-2 | CAS / revision 冲突 | 回传并展示 `current_config_revision`；保存/启停冲突一键重载 |
| P1-3 | Secure-mode 双拨 | `phase:*` 稳定码 + GUI i18n；secure 重拨 20s（覆盖 scheme fallback）；失败前缀 |
| P1-4 | 重连退避 | 会话结束轻度升级（1→2→4→8s）+ 抖动；拨号失败路径仍用指数退避 |
| P1-5 | 错误码收敛 | `managed_config_invalid`→`validation`（保留服务端文案）；兜底 `internal_error` |

---

### 3.3 P2 — 锦上添花

| ID | 项 |
|----|----|
| P2-1 | Token 在路径中的脱敏（日志、文档示例、进程列表说明） |
| P2-2 | 避免日志/webhook 打印完整含 `user_token` 的 HeartbeatRequest |
| P2-3 | Nginx 示例：API 口与 config-server `:22020` 分清（见 [`../ops/deploy-install.md`](../ops/deploy-install.md)） |
| P2-4 | 系统诊断二期：本机 udp/tcp 自环拨测（见 diagnostics 文档阶段 2） |

---

## 4. 建议排期

| 阶段 | 内容 | 预期收益 | 主要改动面 |
|------|------|----------|------------|
| **A** | P0-2 超时分级（**已落地**；待现网联调） | 减少假超时与双击启停 | web：`rpc_timeout.rs` + `restful/rpc.rs`；`ClientManager` 覆盖慢 `handle_*` |
| **B** | P0-1 Start/overwrite 语义（**已落地**；待现网联调） | 减少静默覆盖本地配置 | web：软启动 + UI 确认；节点已有 `overwrite` |
| **C** | P0-4 路由层空窗（**已落地**；UI 重连中可选） | 减轻 NAT 换端口闪断 | web：previous URLs + 会话扫描 + list 合并 |
| **D** | P0-3 协议降级（**已落地** udp↔tcp；广告可选） | 降低 scheme 写错全挂 | **节点/GUI**；`listening_schemes` 广告另议 |
| **E** | P1（**已落地**；待现网联调） | 冲突 UX、脏编辑、Secure 可观测、退避 | web + frontend-lib + 节点/GUI |

原则：

1. **A/B/C 可优先「只升 web」（+前端）** 验证；落地方式是 **ClientManager 覆盖** + REST/UI，不是改 `easytier-core` trait 默认实现。
2. **D / P1-3 / P1-4**：需发节点/GUI，**不在**「只升 web」默认路径；若仅运维双听 + 诊断，保持文档与安装默认即可。
3. 任何「节点必须识别否则失败」的新字段，须先改 [`web-evolution.md`](./web-evolution.md) 冻结接口策略。

---

## 5. 验收要点（摘要）

**阶段 A**

- [x] 慢操作（启停）超过 5s 不必然 UI 超时：proxy-rpc + `ClientManager` 慢 `handle_*` 使用 60s（`rpc_timeout.rs`）；现网仍需联调确认。
- [x] 轻量查询仍保持短超时（默认 5s）。
- [x] 错误码分级 + 前端 i18n 文案（`rpc_timeout` 等稳定 code 已落地）。

**阶段 B**

- [x] 设备已用相同 revision 在跑时，控制台「启动」不重写本地 toml（软启动仅改 disabled；待现网联调）。
- [x] 强制覆盖需确认：footer「启动」经 `startNetwork()` 二次确认（`confirm_start_overwrite`）后才调 `doStartNetwork()`；写盘行为可预期。
- [ ] （剩余）`confirmRunNetwork`（`Config` 内嵌 Run 按钮的 rerun/首次 Run 确认分支，含 `confirm_rerun_network` 文案）当前不可达——两处 `<Config>` 均传 `hide-run-button="true"`，而 `run-network` 唯一发射点被 `v-if="!hideRunButton"` 关闭；且 `saveAndRunNewNetwork` 唯一调用方即该函数。需决定：删除死代码，或恢复内嵌 Run 入口。

**阶段 C**

- [x] 路由兜底：previous URL + 活会话扫描；`list_machine` 优先活会话 URL（单测覆盖；待现网联调）。
- [x] 会话表按 `machine_id` 为主索引 + `session_epoch` 单调接管（已落地，见 §2）。
- [ ] （可选）UI 短暂断连显示「重连中」。

**阶段 D**

- [x] 服务端仅 UDP、客户端配置 `tcp://`（或反之）时自动降级（单测覆盖候选顺序与失败汇总；待现网联调）。
- [ ] （可选）心跳广告 `listening_schemes`，客户端只试广告列表。

**阶段 E**

- [x] P1-1：脏草稿不静默覆盖；`managed-config-revision` 轻量探针 + 确认一键重载（含 generation/unmount 守卫；`:key=deviceId` 防跨机串味）。
- [ ] P1-1（剩余）：基线为 null 时跳过比较——console 上次写入后行被清空（`clear_managed_config_revision`），若此后 webhook 推送新 revision，探针因 `baselineRev` 为空不提示。需决定是否补「null→非 null」跃迁提示。
- [x] P1-2：冲突对话框已实现，可展示 `current_config_revision` 并一键重载（`offerReloadOnRevisionConflict`；`utils.ts` 透出该字段并有单测）。
- [ ] P1-2（剩余）：`managed_config_revision_conflict` 仅内部 reconcile 路由经 `convert_managed_config_error` 产生（`network.rs:465,492`），控制台保存/启停/新建路由走 `convert_error` 永不产生该 code，浏览器端暂无触发源。需决定：给控制台路由接 CAS，或明确降级为纯防御性处理。
- [x] P1-3：Secure 阶段走独立 `connecting_detail`（非 last_error）+ GUI i18n。
- [x] P1-4：会话结束轻度升级退避 + 抖动。
- [x] P1-5：managed-config / unauthorized 稳定 code。

---

## 6. 非目标

- 不把「系统诊断」页当成客户端连通性拨测的替代（诊断对象是控制台进程）。
- 不在本文范围改 VPN 数据面、打洞、中继策略（见 `connection-stability-todo.md` 等）。
- 不默认改为「客户端 URL 多 scheme 列表」作为用户配置格式（易与现有单 URL 心智冲突）；降级应是运行时行为。

---

## 7. 决策记录

| 日期 | 决策 |
|------|------|
| 2026-10-10 | 立项：客户端 ↔ 控制台交互按 P0→P1→P2 排期；与控制台「系统诊断」文档分工 |
| 2026-10-10 | 服务端默认 `udp,tcp` 双听保留；客户端自动降级列为 P0-3，不替代运维双听 |
| 2026-10-10 | Start 覆盖与心跳 `overwrite:false` 语义对齐列为 P0-1 |
| 2026-10-10 | 建议实施顺序：超时分级 → Start/overwrite → 路由层空窗 → 协议降级 |
| 2026-10-10 | 复核：会话主索引已改为 `machine_id` + `session_epoch`，P0-4 降级为「路由层空窗」；P0-2 错误码/前端文案已落地，只差 service 层分级；P1-5 仅剩兜底分支 |
| 2026-10-10 | 复核修正：P0-1/P0-2「只升 web」= `ClientManager` 覆盖 `RemoteClientManager` 默认方法，不默认改 core；P1-4 收窄为会话结束后固定 1s；P0-3/P1-3/P1-4 标明属节点侧 |
| 2026-10-10 | 阶段 A 落地：proxy-rpc / 慢 `handle_*` 分级超时（5s / 60s，`client_manager/rpc_timeout.rs`） |
| 2026-10-10 | 阶段 B 落地：Start 软启动（健康+revision）跳过 overwrite；UI `confirm_start_overwrite` |
| 2026-10-10 | 阶段 C 落地：`previous_client_urls` + `get_session` 多 URL/扫描兜底；`list_machine` 合并活会话 URL |
| 2026-10-10 | 阶段 D 落地：config-server 拨号 `udp↔tcp` 自动降级（纯客户端；`ws`/`wss` 不改写；广告字段未做） |
| 2026-10-10 | 阶段 E 落地：P1-1～P1-5（脏草稿提示、冲突一键重载、Secure 阶段可观测、会话结束抖动退避、managed-config 错误码） |
| 2026-10-10 | P1 加固：revision 轻量探针、冲突展示 revision、scheme 粘滞、会话结束退避升级、validation 错误 kind、load generation 竞态修复 |
| 2026-10-10 | 审阅修复：unauthorized 映射；诊断/日志错误态；revision 探针 generation+unmount；connecting_detail 独立字段；admin logs token 脱敏；DeviceManagement `:key` |
| 2026-10-10 | 复核修正（§5 精确化）：Start 确认经 footer 可达、`confirmRunNetwork` 死代码待决；P1-2 对话框就绪但控制台路由无冲突产生源；P1-1 补 null 基线跃迁剩余项；`connecting_detail` 为 proto 新增可选字段（`GetConfigServerStatusResponse` #4，前后兼容，不属必选协议变更） |

---

## 8. 修订方式

1. 实现前以本文优先级为准；具体 API/字段在 PR 中微调。
2. 落地一项勾选 §5，并视需要把稳定行为写入 `current/`（如 desktop-gui / web-managed-config）。
3. 若需改节点必选协议，先更新 [`web-evolution.md`](./web-evolution.md) 再开发。
