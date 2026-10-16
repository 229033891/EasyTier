# 客户端 ↔ Web 控制台交互优化

## Status

- Status: **Roadmap**
- 最近审阅：2026-10-16（阶段 G S10–S12 代码已落地，待现网验收；P0 A–D + P1 + §9 S1–S9 仍待现网观察）
- 归属：[`web-evolution.md`](./web-evolution.md) 阶段 B（控制台体验 / REST）+ 客户端连接可靠性
- 相关 Current：[`../current/desktop-gui-and-config-server.md`](../current/desktop-gui-and-config-server.md)、[`../current/web-managed-config.md`](../current/web-managed-config.md)、[`../current/console-and-client-interaction-performance.md`](../current/console-and-client-interaction-performance.md)
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
| 重试退避 | 拨号失败指数退避；会话结束轻度升级（1→2→4→8s）+ 抖动，会话稳定运行 ≥15s 后计数清零 | `web_client_routine`：`next_backoff` / `session_end_reconnect_delay` / `note_session_end`（`STABLE_SESSION_UPTIME`） |
| 会话索引 | **机器维度已是主索引**（`user_clients_map`）；`ClientInfo.previous_client_urls` 保留换端口旧 URL；`client_sessions` 仍按 `client_url` 键 | `storage.rs`；`ClientManager::get_session_by_machine_id` / `list_machine_by_user_id` |
| 会话接管 | 按 `session_epoch` 单调接管，旧 epoch 不覆盖新 epoch | `storage.rs` `bind_managed_runtime_state` / `update_session_client` / `is_session_superseded` |
| 心跳 | 间隔/超时可由服务端策略下发；客户端有钳制 | `HeartbeatPolicy`；`web_client` heartbeat 任务 |
| 控制台启动网络 | `overwrite: true`，配置来自 **SQLite** | `RemoteClientManager` 默认实现：`handle_update_network_state` / `handle_run_network_instance_with_source`；节点侧 `process_rpc::run_network_instance` |
| 心跳自动补跑 | `overwrite: false`（已健康可跳过） | `runtime_revision` / reconcile |
| 管理 RPC 超时 | 轻量查询 5000ms；**collect 读路径 15000ms（S11）**；写操作 60000ms | `rpc_timeout.rs`：fast 5s / probe 15s（`collect_network_info` + soft-start 健康探针）/ slow 60s（run/delete/patch 等）；proxy-rpc 同分级（`restful/rpc.rs`） |
| 断开会话 | 断开即停掉该机器**全部**存活会话（含 NAT 换端口后残留的旧 URL 隧道） | `ClientManager::disconnect_session_by_machine_id`；REST `DELETE /api/.../session/{user}/{machine}` |

说明：服务端默认双听已缓解**新装** scheme 不匹配；客户端另有 `udp↔tcp` 运行时降级。`ws`/`wss` 写错或双向都不通时仍会失败。

---

## 3. 问题分级与对策

### 3.1 P0 — 正确性 / 现网痛点

#### P0-1 启动网络强制覆盖客户端配置

- **现象**：控制台 Start / Run 曾总是 `overwrite: true`，用库内配置写回客户端实例文件并重启路径；与设备是否已在跑、GUI/本地是否在编辑无关。
- **对比**：心跳 reconcile 补跑多用 `overwrite: false`。
- **现状（已落地）**：
  1. `ClientManager::handle_update_network_state`：实例已在跑且无 error、managed revision 允许时 **软启动**（只改 DB disabled，不 `run`/`overwrite`）；否则仍 `overwrite: true` 并作废 applied revision。REST 不再在 update-state 前后无条件 invalidate。
  2. 前端：footer「启动」**已不再**弹 `confirm_start_overwrite`（2026-10-12 按产品要求去掉）；重跑仍确认 `confirm_rerun_network`；User→Web 接管仍确认。详见 [`local-config-residue.md`](./local-config-residue.md) §5 P2。
  3. tip 文案写清：权威在 web-owned 场景是控制台库，Start ≠「仅唤醒」。
- **剩余**：现网联调；`run_network` 显式重建路径仍为 `overwrite: true`（重跑有确认）。
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
| P1-4 | 重连退避 | 会话结束轻度升级（1→2→4→8s，带抖动）+ 15s 稳定运行后计数清零 | `web_client.rs` `note_session_end` / `STABLE_SESSION_UPTIME` |
| P1-5 | 错误码收敛 | `managed_config_invalid`→`validation`（保留服务端文案）；兜底 `internal_error` |

---

### 3.3 P2 — 锦上添花

| ID | 项 |
|----|----|
| P2-1 | Token 在路径中的脱敏（日志、文档示例、进程列表说明） |
| P2-2 | 避免日志/webhook 打印完整含 `user_token` 的 HeartbeatRequest |
| P2-3 | Nginx 示例：API 口与 config-server `:22020` 分清（见 [`../ops/deploy-install.md`](../ops/deploy-install.md)） |
| P2-4 | 系统诊断二期：本机 udp/tcp 自环拨测（见 diagnostics 文档阶段 2） |

> 另有 2026-10-12 交互性能/健壮性审查新增的性能项（S1–S9），单列于 **§9**，避免与本章「功能可靠性」混排。

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
- [x] ~~强制覆盖需确认（footer `confirm_start_overwrite`）~~ → **2026-10-12 已去掉**；`doStartNetwork()` 直接启。重跑确认与 User 接管确认仍保留。
- [ ] （剩余）`confirmRunNetwork`（`Config` 内嵌 Run 按钮的 rerun/首次 Run 确认分支，含 `confirm_rerun_network` 文案）当前不可达——两处 `<Config>` 均传 `hide-run-button="true"`，而 `run-network` 唯一发射点被 `v-if="!hideRunButton"` 关闭；且 sticky footer 的 Run 会调到该函数。需决定：清理死分支，或恢复内嵌 Run 入口。
- [ ] 本地残留收敛：见 [`local-config-residue.md`](./local-config-residue.md)（P0–P2 代码已落地；待人工回归）。

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

**阶段 F（§9 交互性能，2026-10-12 审查后新增；顺序 = 建议落地序）**

- [x] S4：`get_session_by_machine_id` 建 `machine_id → Session` 反向索引，去兜底全表扫（web）。
- [x] S5：Dashboard `/summary` 轮询 2s → 5s（前端）。
- [x] S1：`handle_get_network_metas` miss 路径一次 `list_network_configs` 批量回填，再 per-id RPC（core）。
- [x] S2：`cleanup_stale_web_source_instances` 部分成功 `forget_many` + 始终记 `last_desired`（web）。
- [x] S3：reconcile / prepare 读路径超时提到 `MANAGED_RPC_PROBE=15s`（web）。
- [x] S6：未绑定 session 120s idle TTL 回收（web）。
- [x] S7：`notifier.send` 失败补 debug 日志（web）。
- [x] S8：`networkMetaCache` `onUnmounted` 显式清空（前端）。
- [x] S9：注释澄清 idle tick 与 wait 路径不并行（GUI；无行为变更）。

**阶段 G（隧道争用 / 轮询叠加，2026-10-16；依据 Reno 安卓 HAR + 性能审查 F1/F12）**

- [x] S10：状态页 `CollectNetworkInfo` 单飞（`collectNetworkInfoSingleFlight` + generation 丢弃过期结果）；`NetworkList` VIP 用 `collectNetworkInfoSingleFlightForKeys(instIds)` 按实例 UUID 登记，与状态页共享 key（遇 in-flight 则本轮跳过）。**待现网验收**：安卓状态页同 URL pending ≤1。补测：单飞 coalesce / 批量登记 / 状态遇批量则等待后再取 / **工厂非 Promise 返回或同步抛错不泄漏 key**（`collect-network-info-single-flight.spec.ts`）+ RemoteManagement generation 丢过期（`remote-management-config.spec.ts` S10 用例）。复核中发现并修复：`factory()` 返回非 Promise（如 stub）或同步抛错时 `.then` 同步异常会让 key 永久占位、后续 collect 死锁——改用 `invokeFactory` 兜底为 `Promise.resolve`/`Promise.reject` 并保证结算清理。
- [x] S11：只读 `collect_network_info` 改用 `MANAGED_RPC_PROBE=15s`（移出 60s 慢路径；`rpc_timeout.rs` + `handle_collect_network_info`）。**待现网验收**：不可达约 ≤15s 失败；写操作仍 60s；**弱网但可达场景下 15s 不误报**（collect 正常 p50 ≈ 84ms，裕量大，仍列入验收观察项）。
- [x] S12：`RemoteManagement` `STATUS_POLL_MS` 2s → 5s（与 Dashboard S5 对齐）。**待现网验收**：状态刷新体感可接受。
- [x] S13：设备/网络**列表页**轮询 2s/3s → 5s（F2/F4、性能文建议#9；`DeviceList`、`NetworkList` `usePollingList` interval、`DeviceManagement` `PeriodicTask`、`usePollingList` 默认值同步 2000→5000）。这是 1.har 中 53 分钟 178 对 `machines`+`devices`（每对约 8.9KB）的来源，属当前最大的重复交互。**待现网验收**：列表页刷新体感可接受。

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
| 2026-10-12 | 去掉 footer `confirm_start_overwrite`；残留收敛方案见 `local-config-residue.md` |
| 2026-10-10 | 阶段 C 落地：`previous_client_urls` + `get_session` 多 URL/扫描兜底；`list_machine` 合并活会话 URL |
| 2026-10-10 | 阶段 D 落地：config-server 拨号 `udp↔tcp` 自动降级（纯客户端；`ws`/`wss` 不改写；广告字段未做） |
| 2026-10-10 | 阶段 E 落地：P1-1～P1-5（脏草稿提示、冲突一键重载、Secure 阶段可观测、会话结束抖动退避、managed-config 错误码） |
| 2026-10-10 | P1 加固：revision 轻量探针、冲突展示 revision、scheme 粘滞、会话结束退避升级、validation 错误 kind、load generation 竞态修复 |
| 2026-10-10 | 审阅修复：unauthorized 映射；诊断/日志错误态；revision 探针 generation+unmount；connecting_detail 独立字段；admin logs token 脱敏；DeviceManagement `:key` |
| 2026-10-10 | 复核修正（§5 精确化）：Start 确认经 footer 可达、`confirmRunNetwork` 死代码待决；P1-2 对话框就绪但控制台路由无冲突产生源；P1-1 补 null 基线跃迁剩余项；`connecting_detail` 为 proto 新增可选字段（`GetConfigServerStatusResponse` #4，前后兼容，不属必选协议变更） |
| 2026-10-11 | 修复复核发现的三处问题：① webhook 建联漏写 `route_bind`，NAT 换端口后旧隧道被路由选中（补 `set_route_bind` + 回归测试）；② `disconnect_session_by_machine_id` 只停最高 epoch 会话，导致 `DELETE` 返回 204 但旧 URL 隧道残留、下次 RPC 回落旧隧道（改为停该机器全部存活会话，候选 URL 上绑定到其他机器的会话跳过）；③ reconcile 慢 RPC 改用 60s 慢控制器（`slow_rpc_controller`），文档 §2「管理 RPC 超时」行与 P1-4 退避描述同步（15s 稳定后计数清零） |
| 2026-10-16 | 阶段 G（S10–S13）：状态页 collect 单飞、读超时 15s、状态轮询 5s、设备/网络列表页轮询 5s；见 §5 阶段 G；待现网验收 |

---

## 8. 修订方式

1. 实现前以本文优先级为准；具体 API/字段在 PR 中微调。
2. 落地一项勾选 §5，并视需要把稳定行为写入 `current/`（如 desktop-gui / web-managed-config）。
3. 若需改节点必选协议，先更新 [`web-evolution.md`](./web-evolution.md) 再开发。

---

## 9. 交互性能 / 健壮性审查（2026-10-12）

对本节所列路径做了一次「控制台 ↔ 客户端」静态走查（不含数据面）。**结论：无正确性 bug（不会掉配置、不会错删）；问题按落地性价比分层，而非一律「功能 P1」。**

**落地序（与 §5 阶段 F 一致）**：先做 **S4 + S5**（只升 web/前端、现网即感）；再排 **S1–S3**（规模上来才痛）；**S6–S9** 为轻量观察。

### 9.1 优先落地（高性价比）

#### S4 `get_session_by_machine_id` 兜底全表扫

- **位置**：`easytier-web/src/client_manager/mod.rs` `get_session_by_machine_id`（约 L466-476 的 `for entry in self.client_sessions.iter()`）；`list_machine_by_user_id`（约 L553）对每台设备调一次。
- **现象**：路由未命中授权 URL / previous URLs 时退化为全表扫描；单次 `/summary` 为 **O(设备 × 会话)**（非 O(会话²)）。
- **建议**：建反向索引 `machine_id → Session`，兜底从全表扫降到 O(1)。
- **改动面**：仅 web。

#### S5 Dashboard `/summary` 前台 2s 轮询

- **位置**：`easytier-web/frontend/src/components/Dashboard.vue`（`PeriodicTask(..., 2000)`，约 L35）。
- **现象**：前台每 2s 拉一次；`PeriodicTask` 已有 `document.hidden` 跳过（`utils.ts:545`），后台标签页不请求。
- **建议**：放宽到 5~10s（单行改动）；与 S4 叠加时收益更明显。
- **改动面**：仅前端。

### 9.2 规模敏感项（次排期）

#### S1 `handle_get_network_metas` 的 N+1 回退

- **位置**：`easytier-core/src/management/full/remote_client.rs` `handle_get_network_metas`（约 L255-301）。
- **现象**：先批量 `list_network_instance_meta`；**未命中的实例逐个**走 `handle_get_network_config_with_source`（每次 = 1 次设备 RPC + 可能 1 次 storage 查询）。
- **对比**：`handle_list_network_instance_ids`（同文件约 L122）只做 1 次 RPC + 1 次 storage，**无 N+1** —— 说明该放大是 metas 路径特有。
- **触发条件**：设备上「实例存在但 meta 未缓存」时，一次控制台调用放大为 N 次 RPC。
- **建议**：回退改为批量预取（一次 `list_network_config_with_source` 批量接口），或让 meta 列表本身携带 config 摘要。
- **落地方式（2026-10-12）**：miss 集合先走 **一次** `get_storage().list_network_configs(identify, ListNetworkProps::All)` 批量回填，仍 miss 的才 per-id RPC。⚠️ **语义差异**：旧 `handle_get_network_config_with_source` 是「**RPC 运行时 config 优先**，storage 兜底」，批量回填走的是 **storage**（`network_name` 与 `source` 取 storage 行的 `get_runtime_network_config_source()`）。对「GUI 改了 dev_name 但 storage 未落」或「storage 行 `network_name` 与运行时不一致」的场景，回填结果与旧行为不同——**当前按 storage 语义接受**（该路径本就是 meta 未命中时的兜底，且 storage 对 managed 配置权威）。
- **改动面**：`easytier-core` 管理侧（节点/GUI，**不在**「只升 web」路径）；若走批量 RPC 需先冻结接口（见 [`web-evolution.md`](./web-evolution.md)）。

#### S2 stale-delete 部分失败 → 本机会话下轮反复跑 Full cleanup

- **位置**：`easytier-web/src/client_manager/session/runtime_revision.rs` `cleanup_stale_web_source_instances` 尾部（约 L839-841）。

  ```rust
  if !outcome.has_failed {
      cache.cleaned_web_source_instances = true;
      cache.last_desired_web_inst_ids = Some(db_web_inst_ids);
  }
  ```

- **现象**：删除是**一次**批量 RPC；若 RPC 失败或 `undeleted_instance_ids` 非空则 `record_failure`，缓存位 `cleaned_web_source_instances` 置不上。稳态无 dirty patch 时 scope 本就多为 Full（约 L460），于是**该机器**下一轮心跳会再次跑完整 stale cleanup（不是扫「全部设备」）。
- **性质**：「部分失败放大为本机会话级反复 Full cleanup」，与失败实例数无关。
- **建议**：拆细失败粒度 —— 已成功删除的实例照常 `forget_many` + 更新 `last_desired_web_inst_ids`，只对 `undeleted_instance_ids` 保留重试；或将「缓存位」与「本轮是否完全成功」解耦。
- **改动面**：仅 web（`runtime_revision.rs`），符合「只升 web」。

#### S3 reconcile 读路径超时与写路径不统一（5s vs 60s）

- **位置**：读/探测路径用 `MANAGED_RPC_FAST=5s`（`client_manager/rpc_timeout.rs`；`mod.rs` 约 745/749/1251），写路径用 `MANAGED_RPC_SLOW=60s`（`slow_rpc_controller`）；另有 `MANAGED_RPC_PROBE=15s`。
- **现象**：同一设备链路上，读侧 5s 就断，跨网/NAT 场景更易超时误判。
- **与 S2 的关系（勿过度归因）**：meta list 失败时常走 intersection 回退，orphan 集可能为空，反而**仍会置上** cache bit；stale **删除**本身已用 60s。S3 主要伤害是读路径假阴性 / 体验抖动，**不是**「必然触发 S2 全量重扫」。
- **建议**：读侧区分「本机快路径」与「跨网慢路径」；跨网读至少提到 `MANAGED_RPC_PROBE=15s`，或复用设备侧缓存的 meta 而非实时拉。
- **改动面**：仅 web。

### 9.3 轻量 / 观察项（不急）

| ID | 位置 | 问题 | 影响 |
|----|------|------|------|
| S6 | `easytier-web/src/client_manager/mod.rs` `client_sessions`（约 L212） | `DashMap` **无空闲 TTL**，仅约 L268 `remove_if` 清理非运行/被取代会话 | 旧会话未及时回收时长驻内存 |
| S7 | `easytier-web/src/client_manager/session.rs`（约 L578/1075/1220/1487/1510 等） | `let _ = notifier.send(...)` 吞掉发送错误 | 通知丢失无日志，排障困难 |
| S8 | `easytier-web/frontend-lib/src/components/RemoteManagement.vue`（`networkMetaCache`，约 L414） | 运行期靠当前实例列表 prune（约 L554）；**`onUnmounted` 不停清 cache**（只停定时器 / 作废 generation） | 长会话或实例集膨胀时内存缓慢上涨 |
| S9 | `easytier-gui/src/pages/index.vue`（`configServerIdleProbeTick`，约 L320） | 仅在 **无 URL 且未启用** 的 idle 探测路径自增；`waitForConfigServerOutcome`（有 URL、500ms 等待）**不会**与 idle 路径并行自增同一 counter | 最坏是 idle 探测节流不准，无害 |

**注意（S4/S5 辨正）**：

- `get_heartbeat_requests`（`mod.rs` 约 793）本身是 **O(1)**（按 `client_url` 直接 `client_sessions.get`），**不是**全表扫。`/summary` 的成本在 `list_machine_by_user_id` → **每台设备**调 `get_session_by_machine_id` 时的兜底全表扫描（S4）。
- `Dashboard.vue` 的 `PeriodicTask` 已带 `document.hidden` 跳过；S5 只是「前台 2s 一轮」。

### 9.4 已确认无问题的点

- **持锁跨 `await`**：未发现（会话数据用 `RwLock` 短临界区）。
- **丢中间心跳导致丢配置**：否 —— `recv_latest_heartbeat`（`runtime_revision.rs` 约 L25-57）是 `try_recv` **drain 到最新**，语义正确。
- **重试风暴**：否 —— 拨号有指数退避 + 会话结束轻度升级（见 §2「重试退避」）。
- **`/summary` 心跳读取**：`get_heartbeat_requests` 为 O(1)，非瓶颈（见上「注意」）。

### 9.5 决策记录（追加）

| 日期 | 决策 |
|------|------|
| 2026-10-12 | 交互性能/健壮性静态走查：确认无正确性 bug；记 S1–S9（见上分层） |
| 2026-10-12 | 辨正：`get_heartbeat_requests` O(1)；`PeriodicTask` 已有 `document.hidden`；S4 为 O(设备 × 会话) |
| 2026-10-12 | 落地序：先 S4 反向索引 + S5 放宽轮询（只升 web/前端）；再 S1–S3；S6–S9 观察 |
| 2026-10-12 | 文档纠偏：勿将 S1–S3 标成「功能 P1」而与「先做 S4/S5」矛盾；S3 不必然触发 S2；S2 范围为本机会话而非全部设备；S8 无 onUnmounted 清 cache；S9 idle 与 wait 路径不并行自增 |
| 2026-10-12 | §9 S1–S9 代码落地（阶段 F 勾选） |
| 2026-10-12 | 复核（S1–S9 落地后）：修 `runtime_revision.rs` 两处 UTF-8 注释损坏（`§8.1.4`→`?8.1.4`、`—` 破损 + 注释被挤成一行）；`lifecycle_tests.rs` 3 处 rustfmt DIFF 已格式化为 0 diff；S2 的 `should_delete − retained` 逻辑经 `remain_inst_ids` 服务端语义核实无误；S1 记录 storage 回填 vs 旧 RPC 优先的语义差异（按 storage 接受） |
| 2026-10-16 | 阶段 G：S10 状态页 collect 单飞 + NetworkList 避让；S11 collect 读超时 60s→15s probe；S12 状态轮询 2s→5s；S13 列表页轮询 2s/3s→5s（F2）。依据 `temp/1.har`/`2.har`（Reno 安卓）。Current 性能文仅更新审阅指针，验收后再改 F12/F1/F2 正文 |
