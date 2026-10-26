# 隧道级 IP 流量可观测性 —— 缺陷修复（批次 A）

## Status

- Status: **Current**（本批次已实现，待验收）
- 最近更新：2026-10-26
- 行为基线：[`./tunnel-ip-flow.md`](./tunnel-ip-flow.md)（描述代码**今天做什么**，本文件只记录**本次要改什么**）
- 设计决策：[`../roadmap/tunnel-ip-flow-observability.md`](../roadmap/tunnel-ip-flow-observability.md)

本文件是**验收合同**：§1 的每一条都有「问题 / 证据 / 改法 / 验收方法 / 回滚」四段，§3 是可勾选的验收清单。凡不在 §1 的改动都算超范围。

---

## 0. 范围

### 本次修复（批次 A，4 项）

| ID | 问题 | 严重度 | 影响面 |
|----|------|--------|--------|
| **F-1** | 运行页 IP 流量 tip 把节点上报的 `ip` 拼进 `escape: false` 的 HTML | 低（信任边界） | 前端 1 文件 |
| **F-2** | `flow_history` 查询无时间上界，时钟超前的节点数据会污染所有窗口 | 低 | web 1 文件 |
| **F-3** | `FlowTable::configure()` 是公开的死 API，且一旦接到热更路径会立刻算错 8 倍 | 中（潜伏） | core 1 文件 |
| **F-4** | 关闭 `flow_stats` 时仍常驻 1 Hz + 1/30 Hz + 1/5 Hz 三个定时任务 | 低 | core 1 文件 |

### 明确**不做**（附理由，避免验收时误判为漏项）

| ID | 项 | 不做理由 |
|----|----|----------|
| **F-5** | 控制台历史速率曲线本质是「每分钟随机抓 1 秒」 | 这是**口径设计**而非缺陷（D11 的既定选择）。修它需要推翻 D11、改 4 处文档、改写 3 个测试并新增 4 个，工期 4–6h，且曲线形状对用户可见变化。**建议单独立项**：先出设计说明确认「首点无前驱」与「掩码视图新 IP 加入产生假尖峰」两个降级策略 |
| **F-6** | `rate_bytes_per_sec` 不按实际窗口长度归一 | 同上。做了 F-5（改用累计计数器差分）之后本项**自动失效**，现在单独做是无用功。且其真实影响只是运行页瞬时速率略有偏差，不是历史失真的主因 |
| **F-7** | `full/remote_client.rs:115-118` 无条件请求 `FlowView::Exact` | 改注释需要的**事实**（该 client 是否可能连远程节点）无法只从代码判定。在确认前动注释反而可能写出新的错误陈述 |
| **F-8** | `flow_table.rs:357-378` `record_rx()` 在 `record()` 判 `is_enabled()` 之前就调 `parse_inner_ips(payload)` | 防御性 nit，**当前无实际开销**：唯一调用方 `record_rx_flow()` 已先判 `is_enabled()`；把判断下移到 `record_rx` 只是让「关闭即零成本」不依赖调用方纪律。改它要动热路径参数顺序，收益不抵风险，**建议随 F-5 一起做**（届时反正要重写记账入口） |

---

## 1. 修复项

### F-1 运行页 IP 流量 tip 未转义节点上报的字符串

**问题**

`v-tooltip.top` 是 PrimeVue 指令，`{ value, escape: false }` 表示把 `value` 当 **HTML** 渲染。`rowFlowTip()` 用 `'<br>'` 拼接多行后返回 `escape: false`，其中 `flowIpLabel(flow)` 直接插值了节点上报的 `flow.ip` 字符串（proto `string` 字段），未做转义。

**证据**

- `easytier-web/frontend-lib/src/components/Status.vue:243-258` — `rowFlowTip()` 返回 `{ value: lines.join('<br>'), escape: false }`
- `easytier-web/frontend-lib/src/components/Status.vue:1285` — `v-tooltip.top="rowFlowTip(slotProps.data)"`
- 同文件 `:790-799` `vipStateTip` 路径**是**转义的，说明作者清楚这个坑；`easytier-web/frontend-lib/src/modules/statusDisplay.ts:903-906` 已有一个 `escapeHtml` 局部箭头函数，但**未导出**，`rowFlowTip` 用不到

**改法**

1. `statusDisplay.ts`：把 `vipStateTip()` 内部的局部 `escapeHtml` 提升为模块级 `export function escapeHtml(text: string): string`，`vipStateTip` 改为调用它（**行为零变化**）。
2. `Status.vue`：`import` 该函数，在 `rowFlowTip()` 里对 `flowIpLabel(flow)` 的**结果**转义。
3. **禁止**在 `flowIpLabel()` 内部转义 —— 它同时被 `rowFlowSummary()`（`:232`）调用，而那里走 `{{ }}` 文本插值，**Vue 会再转义一次**，会导致正常行显示成字面 `10.0.0.0&lt;/24`。这是本项唯一的坑。

**验收方法**

`easytier-web/frontend-lib/tests/status-display.spec.ts` 新增用例：
- `escapeHtml` 对 `&`、`<`、`>` 正确转义，且先转 `&` 再转 `<`/`>`（顺序错会产生 `&amp;lt;`）
- 对纯 IP 文本 `10.0.0.0/24 (3)` 是**恒等**（证明不会破坏正常显示）

**回滚**：`git revert` 该 commit（前端 2 文件 + 1 测试，无数据/协议影响）。

---

### F-2 `flow_history` 查询缺时间上界

**问题**

`SERIES_SQL` 与 `PEER_HOSTNAME_SQL` 只有 `sampled_at >= ?`，没有 `<= now`。节点时钟超前时，那些「未来」行会进入**每一次**查询的任何窗口，把曲线整体抬高。且 `cleanup_once` 按 `sampled_at < cutoff` 删，未来行永远不满足条件 → 被动等真实时间追上才自然过期。

**证据**

- `easytier-web/src/restful/flow_history.rs:88-104` — `SERIES_SQL` 的 `WHERE ... AND sampled_at >= ?`
- 同文件 `:113-127` — `PEER_HOSTNAME_SQL` 同一缺陷
- 同文件 `:155` — `let now = chrono::Utc::now().timestamp();` 已经算好了，只是没用在 WHERE 里

**改法**

两条 SQL 各加 `AND sampled_at <= ?`，handler 把已有的 `now` 绑进去（`SERIES_SQL` 参数数 6 → 7，`PEER_HOSTNAME_SQL` 4 → 5）。

**验收方法**

- 新增用例：seed 一条 `T0 + 1_000_000` 的未来行，用 `from = T0` / `to = T0 + 600` 查询，**断言该行不出现**，同时同窗口内的正常行仍返回。
- 既有 3 个用例（`series_keeps_tunnels_apart_and_averages_rate`、`series_filters_by_user_machine_and_window` + `run_series`）的参数个数必须同步补齐 —— 漏一处会在**第一个测试**就 panic（sea-orm 参数个数不匹配），不会溜到线上。

**已知残留（不修，写在这里以免被当 bug 报）**：未来行**不可见但仍占存储**。等真实时间追上 `sampled_at + 保留期` 后由 `cleanup_once` 正常回收，属自愈，不需要额外处理。

**回滚**：`git revert` 单文件。

---

### F-3 `FlowTable::configure()` 是公开死 API，且接上就错

**问题（两个叠加）**

1. **没有任何运行时调用点**：全仓 `.configure(` 只有 `flow_table.rs:220`（`FlowTable::new` 内部）一处。但函数是 `pub` 且 doc 写着 *"The host may call this when an instance configuration is reloaded."* —— 公开邀请别人去接。
2. **接了立刻算错**：`sampled` 是**条目级 sticky** 标记（`record()` 里 `entry.sampled.store(true)` 一旦置位不复位），而放大系数用**当前** `sample_rate`。先以 `sample_rate = 1` 跑出 1 GB（**真实值**），再把 `sample_rate` 调到 8：下一个被抽中的包置 `sampled = true`，随后整个 `total_bytes`（含那 1 GB 全量计数）被 ×8 → **虚高 8 倍**。反向 8→1 则低估 8 倍。

**证据**

- `easytier-core/src/peers/flow_table.rs:224-246` — `pub fn configure` + 上述 doc 注释
- `:318` — `record()` 中 `entry.sampled.store(true, ...)`
- `:381` — `summary()` 中 `let scale = if entry.sampled { sample_rate } else { 1 }`
- `:589` / `:666` — `snapshot()` 的 masked / exact 两处同一 `scale` 写法
- grep `.configure(` 全仓：仅 `:220` 一处调用

**改法**

按「`[flow_stats]` 仅构造期生效」收口，二合一：

1. `pub fn configure` → `fn configure`（**私有**）：任何将来想在运行时热更的调用会**编译失败**，把潜伏 bug 变成编译期错误。
2. 函数末尾清空累计状态（`flows` / `per_conn_counts` / `sample_cursor`），并把 doc 改成「仅构造期调用；重配会丢弃累计计数，因为放大系数依赖 `sample_rate`」。
   - 对现状**零影响**：唯一调用点 `new()` 里三张表本来就是空的，这次 clear 是空操作。
   - 但若将来有人重新 `pub` 它，clear 保证不会出现 ×8 的错误放大。
3. 明确记录：**改 `[flow_stats]` 需要重启网络实例**（实例重建会 `FlowTable::new(flow_stats)`），此时计数器本来就会归零。

**验收方法**

`easytier-core/src/peers/flow_table.rs` 的 `mod tests`（子模块可访问父模块私有项）新增用例：
- 记录若干条流 → 断言 `len() > 0` → 以不同 `sample_rate` 调用 `configure` → 断言 `is_empty()`，且新配置的 `config().sample_rate` 生效。

**回滚**：单文件 revert。无协议、无配置格式变化。

---

### F-4 关闭总开关时仍常驻三个定时任务

**问题**

`spawn_flow_stats_routines()` 不检查 `is_enabled()`，无条件 spawn 三个 task：1 Hz 速率窗口轮转、30 s GC、5 s `CORE::FLOW` 聚合日志。与 D5「关闭时零开销」的承诺不符 —— CPU 占用可忽略，但对 MIPS 软路由 / 电池设备是实打实的每秒唤醒。

**证据**

- `easytier-core/src/peers/peer_manager.rs:3600-3637` — 三个 `tasks.lock().await.spawn(...)`，函数入口无 `is_enabled()` 判断
- 第三个 task（5 s 日志）**内部**已有 `if !flow_table.is_enabled() { continue; }`，说明作者意识到要判，但只判了日志那一个，且是「醒过来再判」，没省掉唤醒

**改法**

函数入口加 `if !self.flow_table.is_enabled() { return; }`，并写清前提：

> 依赖 F-3 —— `configure` 私有化后 `enabled` 在 `FlowTable` 生命周期内**不可变**，因此「构造时关闭」等价于「永远关闭」，跳过 spawn 是安全的。

> **注意（F-3 / F-4 的耦合）**：若 F-3 选了「保留 `pub configure` 支持热更」，则 F-4 **不能**早退（否则热更打开后速率窗口不轮转、GC 不跑、表无限增长）。两项必须一起改，不能只改一项。

第三个 task 内已有的 `continue` 判断保留（belt-and-braces），注释同步更新为「整组任务在关闭时不会 spawn」。

**验收方法**

- 代码评审：`spawn_flow_stats_routines` 入口第一句是早退；三处 `spawn` 都在早退之后。
- 运行验证（手工）：`[flow_stats] enabled = false` 起节点，`RUST_LOG=info` 下**不应**出现 `target="CORE::FLOW"` 的 `flow aggregate` 日志。
- 回归：`enabled = true` 明确仍能看到该日志（每 5 s，且表非空时）。

**回滚**：单行 revert。

---

## 2. 改动清单

| 文件 | 项 | 预期改动量 |
|------|----|-----------|
| `easytier-web/frontend-lib/src/modules/statusDisplay.ts` | F-1 | 局部箭头 → 模块级导出函数；`vipStateTip` 改调用（行为不变） |
| `easytier-web/frontend-lib/src/components/Status.vue` | F-1 | import + `rowFlowTip` 内转义 1 处 |
| `easytier-web/frontend-lib/tests/status-display.spec.ts` | F-1 | +2 用例 |
| `easytier-web/src/restful/flow_history.rs` | F-2 | 2 条 SQL + handler 绑定 + 3 处测试参数 + 1 新用例 |
| `easytier-core/src/peers/flow_table.rs` | F-3 | `configure` 私有化 + 清表 + doc + 1 新用例 |
| `easytier-core/src/peers/peer_manager.rs` | F-4 | 入口早退 + 注释 |

不涉及：`easytier-proto`（无协议变化）、`deriving`/迁移（无表结构变化）、`easytier-cli`、`easytier-gui`、TOML 配置格式。

---

## 3. 验收清单

### 3.1 代码层

- [ ] **F-1** `Status.vue` 的 `flowIpLabel()` **未**被改动（转义只在 `rowFlowTip` 内），`rowFlowSummary()` 输出不变
- [ ] **F-1** `escapeHtml` 从 `statusDisplay.ts` 导出，`vipStateTip` 行为不变（既有 `vip-reachability.spec.ts` 全绿）
- [ ] **F-2** `SERIES_SQL` / `PEER_HOSTNAME_SQL` 均含 `sampled_at <= ?`
- [ ] **F-2** 所有绑定该 SQL 的地方参数个数一致（编译期不校验，靠测试兜）
- [ ] **F-3** `configure` 不再是 `pub`；grep `\.configure(` 只剩 `FlowTable::new` 内一处
- [ ] **F-3** `configure` 末尾清空 `flows` / `per_conn_counts` / `sample_cursor`
- [ ] **F-4** `spawn_flow_stats_routines` 首个语句是 `is_enabled()` 早退
- [ ] **F-4** 早退注释明确写出「依赖 F-3 的不可变性」

### 3.2 测试层

```bash
# 前端（快，约 20s）
pnpm -C easytier-web/frontend-lib vitest run tests/status-display.spec.ts tests/vip-reachability.spec.ts tests/flow-history.spec.ts

# 后端（首次为全量编译，冷依赖，约 10–25 min）
cargo test -p easytier-core --lib peers::flow_table
cargo test -p easytier-web --lib restful::flow_history
```

- [ ] `status-display.spec.ts` 全绿，新增 2 用例通过
- [ ] `vip-reachability.spec.ts` 全绿（F-1 未改 `vipStateTip` 语义）
- [ ] `flow_table` 全部用例通过，新增 `configure` 用例通过
- [ ] `flow_history` 全部用例通过，新增「未来行被过滤」用例通过

### 3.3 行为层（手工，可选）

- [ ] `[flow_stats] enabled = false` 起点：无 `CORE::FLOW` 日志
- [ ] `enabled = true` 且表非空：每 5 s 一条 `flow aggregate`
- [ ] 运行页 IP 流量列的 tip 显示与改动前**逐字符一致**（纯 IP 输入下转义是恒等变换）

---

## 4. 风险与残留

| 项 | 评估 |
|----|------|
| F-1 误在 `flowIpLabel` 内转义 | **唯一实质风险**，会造成显示回归（`10.0.0.0&lt;/24`）。已用「禁止清单」+ 恒等用例双重卡住 |
| F-1 回归（2026-10-25） | `a4829412` 改独立流量面板时删掉 tip 转义；`218f377b` 嵌回 peer 表 tip 时未恢复。**2026-10-26 已再修**：`rowFlowTip` 重新 `escapeHtml(flowIpLabel(flow))` |
| F-2 参数个数漏改 | 低。会在首个测试 panic，非线上 |
| F-3 私有化破坏外部调用者 | 已 grep 全仓确认仅 `new()` 一处；若某个未覆盖路径存在，会**编译失败**而非静默错误 |
| F-4 早退导致热更失效 | 仅在 F-3 未落实时成立；两项已绑定交付 |
| 残留：未来行占存储 | 自愈（真实时间追上后由 `cleanup_once` 回收），不额外处理 |
| 残留：`docs/current/tunnel-ip-flow.md` §1 未改 | **一致性缺口**。该文档记录 F-3/F-4 之后的新事实（`[flow_stats]` 仅构造期生效、关闭时无定时器），本批次**同步更新**，见 §5 |

---

## 5. 与现状文档的差异（已同步 ✅）

[`./tunnel-ip-flow.md`](./tunnel-ip-flow.md) 已按本批次落地后的事实更新：

- §1「默认开关」行：扩展为 **「关闭时整个 routine 组根本不 spawn（1s 速率窗口 / 30s `gc()` / 5s 聚合日志三个定时任务全部跳过）」**（F-4）。
- §2「配置」：新增 **「生效时机」** 段 —— `[flow_stats]` 仅实例构造时读取；控制台改配置走 `overwrite: true` **重建实例**（自动生效，但该实例全部隧道重连、flow 计数清零），TOML 启动的进程需手动重启实例（F-3）。
- §4「已知未闭环」：原文未提及 F-2 的缺陷，**无需删除**。

### 5.1 「改配置会重建实例」的依据（供验收核对）

`flow_stats_*` **不在** `easytier-proto` 的 `InstanceConfigPatch` 里，因此：

1. `runtime_reconcile.rs:103` `hot_patch_base()` 未清空任何 `flow_stats_*` 字段 → 差异保留在整表比对中；
2. `runtime_reconcile.rs:359` `if current_base != desired_base { return Ok(None); }` → 该差异无法用 patch 表达；
3. `runtime_reconcile.rs:555` 落到 `overwrite: true` → 实例重建。

这既是 F-3 私有化**不造成「配置静默无效」**的原因，也是「调 `sample_rate` 会中断全部隧道」这一运维代价的来源。**若将来把 `flow_stats_*` 加进 `InstanceConfigPatch` 做热更，则 F-3 与 F-4 必须同时回退为「notification-driven」方案**（见 F-4 的耦合警告）。
