# Web 状态面 / 连接历史 / 配置加载（2026-10-09）

## Status

- Status: **Archive**（2026-10-09 已落地；日常以 Current 为准）
- 日期：2026-10-09
- 最近审阅：2026-10-09
- 背景：Web 控制台节点表无法直观看到「两节点间几条链路」；对端连接历史缺丢包/抖动曲线；打开配置页偶发偏慢。
- 相关 Current：[`../current/peer-connections.md`](../current/peer-connections.md)、[`../current/web-managed-config.md`](../current/web-managed-config.md)
- 索引：[`../README.md`](../README.md)

---

## 1. 落地摘要

| # | 需求 | 结果 |
|---|------|------|
| 1 | 连接历史加 **丢包** 曲线 | 前端展示已有 `loss_rate`（库表/API 原有） |
| 2 | 连接历史加 **抖动** | 迁移 `jitter_us` + 采样 + API + 图表 |
| 3 | 配置加载偏慢 | 读配置改为 **本地存储优先**；配置页降低实例列表轮询；并行拉 metas |
| 4 | 节点表 **一链路一行** | `flattenPeerConnRows`；本端/对端地址与按链路指标 |
| 5 | GUI 显示连接历史 | **不做**（继续用 Web 控制台） |

---

## 2. 行为说明

### 2.1 对端连接历史（Web only）

- 入口：设备管理 → 节点详情 →「对端连接历史」（`api.get_peer_conn_history` 存在时）。
- 曲线顺序：延迟 → **丢包率** → **抖动** → 流量。
- 采样：默认约 60s；按 peer **聚合**（优先 default conn），不是按每条 PeerConn 存历史。
- 升级后旧采样的 `jitter_us` 为 `-1`（未知），图表该段断线；新采样后才有抖动点。
- **必须跑迁移** `m20261009_000012_peer_conn_history_jitter`，否则历史查询 SQL 会因缺列失败。

### 2.2 节点信息表（一链路一行）

- `Status.vue` 数据源：`flattenPeerConnRows(peer_route_pairs)`。
- 多隧道 peer 拆多行；本机 / 仅中转（无直连 conn）仍一行。
- 新增列：**本端地址**；主机名旁「主/备」；质量分按行显示 `★` / `·`（不再用汇总 `+N`）。
- 角标为**链路行数**；多于节点数时 tooltip：`N 节点 · M 链路`。
- 上下行按**单链路**累计字节；节点级合计不再在该列叠加（无 conn 的中转行仍可能为空）。

### 2.3 配置加载

- **不要**在 trait 默认里做全局存储优先：会打坏 GUI Windows `persist_runtime_dev_name`（运行时网卡名回写依赖 RPC）。
- **Web only**：`ClientManager` 覆盖 `handle_get_network_config_with_source` → **SQLite 优先**，无行再 RPC。
- **GUI**：保留 trait 默认 **RPC 优先** → 存储兜底。
- 权威说明见 Current：[`../current/desktop-gui-and-config-server.md`](../current/desktop-gui-and-config-server.md) §2.5、[`../current/web-managed-config.md`](../current/web-managed-config.md) 文首。
- 配置页 `list_network_instance_ids` 约每 6s 一次（`STATUS_POLL_MS=2s` × `CONFIG_MODE_LIST_POLL_EVERY=3`），且配置模式跳过 `get_network_info` 轮询。
- 已加载同 `instance_id` 草稿时，`ensureConfigModeEditing` 不再重复拉配置。

---

## 3. 代码锚点

| 区域 | 路径 |
|------|------|
| 历史采样 | `easytier-web/src/peer_history.rs` |
| 历史查询 API | `easytier-web/src/restful/peer_history.rs` |
| 迁移 | `easytier-web/src/migrator/m20261009_000012_peer_conn_history_jitter.rs` |
| `get_jitter_us` | `easytier-proto/src/api.rs`（`PeerRoutePair`） |
| 读配置 RPC 优先（GUI 默认） | `easytier-core/.../remote_client.rs` |
| 读配置存储优先（Web 覆盖） | `easytier-web/.../client_manager/mod.rs` |
| 扁平化行 | `frontend-lib/src/modules/statusDisplay.ts` → `flattenPeerConnRows` |
| 状态表 | `frontend-lib/src/components/Status.vue` |
| 历史图 | `frontend-lib/src/components/PeerConnHistoryChart.vue` |
| 配置页轮询 | `frontend-lib/src/components/RemoteManagement.vue` |

测试：`peer-history.spec.ts`、`status-display.spec.ts`、`peer-conn-history-chart.spec.ts`（实例切换重挂 canvas 时重建 Chart）、`remote-management-config.spec.ts`（覆盖配置页脏/保存等；**不含** `pollTick` 轮询与 Web 存储优先 RPC 路径的专项单测）。

---

## 4. 复查与已修问题（2026-10-09）

| 项 | 结论 | 处理 |
|----|------|------|
| `default_conn_id` 过期时无行标 ★ | 真实边缘 bug | `hasPreferred` 回退到首条 conn |
| `pollTick` 声明在 watch 之后 | 当前无 `immediate`，运行时安全；易踩 TDZ | 提前到文件前部声明 |
| 历史空数据后 Chart 实例残留 | 预存问题，加丢包/抖动后更明显 | `destroyCharts()` 在无 peers 时清理 |
| `jitter_us` 类型过严 | 旧后端缺字段 | API 类型改为可选 |
| 读配置全局存储优先 | **P1**：打断 GUI `persist_runtime_dev_name` | 已改为 **仅 Web 覆盖**；GUI 仍 RPC 优先 |
| Web 存储优先覆盖设备未同步改动 | P2 语义 | Current §2.5 / web-managed-config 文首写清权威 |
| 历史仍按 peer 聚合 | 非 bug | 与「一链路一行」实时表粒度不同，已知 |
| 空 class `peer-hostname-cell` | 小 | 已去掉 |
| 配置页仍每秒调 `loadCurrentNetworkInfo` | 小 | 配置模式跳过该调用 |
| 切换实例后历史图空白 | 既有：清 `data` 卸载 canvas 后 Chart 句柄未清，`initCharts` 跳过 | `destroyCharts()` 在清 data / load 失败处；刷新不销毁以免闪 |
| public-server hostname 无 tooltip | 扁平化后标签分支丢了 | `Status.vue` 标签容器补回 tooltip |
| `connLossRate` 缺字段显示 0% | 与延迟/抖动不一致 | 缺字段返回空串 |
| pathQuality tip 文案两处重复 | 改文案易漏 | `formatPathQualityTipLine` 共用 |

### 已知限制（非本次缺陷）

- GUI 无连接历史（无 Web 库表）；面板靠可选 API 隐藏。
- 历史抖动需升级 Web 并等待新采样。
- 配置页若只在设备改、未回写 Web 库，编辑页显示库内版本。

---

## 5. 验收清单

- [x] `vitest`：`peer-history` / `status-display` / `peer-conn-history-chart` / `remote-management-config`
- [ ] 升级 easytier-web，确认迁移写入 `jitter_us` 列
- [ ] Web：多隧道 peer 拆行，主/备与本端/对端地址正确
- [ ] Web：历史图四条曲线；旧数据抖动可断线
- [ ] Web：配置页打开不再长时间停在「正在加载网络配置」（设备在线且库有配置时）

---

## 6. 控制台 ↔ 设备隧道争用缓解（同日续）

**瓶颈**：状态页 `CollectNetworkInfo`、列表 `list_machines`、历史采样全量 collect、heartbeat reconcile 共用同一 BidirectRpc 隧道；原状态页约 1Hz 最伤配置加载与采样。

| 改动 | 位置 | 效果 |
|------|------|------|
| 状态页轮询 1s → **2s**；配置模式列表 tick 保持约 6s | `RemoteManagement.vue`（`STATUS_POLL_MS`） | 减半 CollectNetworkInfo |
| 设备列表 / Dashboard / `usePollingList` 默认 **2s**（设备管理页已 3s；网络列表已 2s） | `DeviceList.vue`、`Dashboard.vue`、`usePollingList.ts` | 减轻 list_machines 与状态争用 |
| 历史采样优先按 heartbeat **running_network_instances** 定向 collect；空则跳过；无 heartbeat 再全量 | `peer_history.rs` | 少采停用实例，缩短采样 RPC |

未做（可选后续）：同机 Concurrent CollectNetworkInfo 合并、RPC 优先级队列、采样器并行限流。
