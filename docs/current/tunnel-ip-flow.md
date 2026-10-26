# 隧道级 IP 流量可观测性（现状）

## Status

- Status: **Current**（Phase 1–3 可依赖行为；Phase 4 告警本期不做）
- 最近审阅：2026-10-26（运行页面板文案：实时流量 / 流量历史）
- 设计决策与实施记录：[`../roadmap/tunnel-ip-flow-observability.md`](../roadmap/tunnel-ip-flow-observability.md)
- 关联：[`peer-connections.md`](./peer-connections.md)（多 PeerConn / `default_conn`）
- 索引：[`../README.md`](../README.md)

本文只描述 **代码今天做什么**。完整决策表（D1–D16）、开销实测与审查关闭项见 Roadmap。

---

## 1. 行为摘要

| 能力 | 现状 |
|------|------|
| 热路径记账 | `easytier-core` `peers::flow_table`：按 **PeerConn × IP × 方向** 计数（`FlowKey` 另含 `peer_id`，因与 PeerConn 一一对应而冗余）；TX 记目的 IP，RX 记源 IP（内层 IP 明文侧） |
| 记账范围 | **只记本节点收发的流量**：RX 只认 `PacketType::Data` 且非本地注入（转发包在此处不解密，不计入）；TX 只认 outbound router 打过 `flow_meta` 的包，且仅在所选隧道 `send_msg` **成功之后**记账（sink 失败不抬 TX；转发方向没有该字段，同样不计入） |
| **字节口径** | **内层 IP 载荷字节** = 压缩/加密之前的 IP 包长度，**不是链路字节**。运行页「上传/下载」列来自 `PeerConnStats`（隧道 filter 处的链路字节），两者口径不同，**不可比、不可相减**，UI 必须分开标注 |
| 默认开关 | **`[flow_stats]` 默认关闭**；关闭时**整个 routine 组根本不 spawn** —— 1s 速率窗口、30s `gc()`、5s 聚合日志三个定时任务全部跳过（`spawn_flow_stats_routines` 入口早退）；且 `FlowTable::record()` 只做一次 relaxed atomic 读，无分配、无锁（整条包路径上会命中 2–3 次同一判断，量级不变） |
| 采样与估算 | `sample_rate = N > 1` 时按 **1/N 抽样**记账，条目置 `sampled` 标记；**读取时 ×N 放大**为估算值（UI 显示「估算」，i18n `flow_estimated`） |
| 精确 IP | **双重闸门**：节点 `allow_exact_ip` **且** 调用方请求 `FlowView::Exact`；否则只出掩码聚合（缺省 MASKED） |
| 速率 | 节点侧固定约 1s 窗口算 `rate_bytes_per_sec`（D11）；**不**对累计 `bytes` 做差分求速 |
| 回传条数 | 节点侧每隧道最多回传 **top-10**（`FLOW_TOP_N_PER_CONN`，按**累计 `bytes`** 降序）。运行页展开行**不会**超过 10 行 |
| 条目存活 | 无流量超过 `ttl`（固定 **600s，不可配**）的条目由 `gc()` 回收；`max_per_conn`（256）/ `max_total`（8192）限额另做兜底淘汰，按 `last_seen_ms` 最旧优先 |
| 运行页 | `Status.vue` 独立折叠面板「实时流量」（不嵌在 Peer 表内）：扁平表（对端 / 协议 / 方向 / IP / 字节 / 速率），按累计字节降序；支持对端/IP/方向本地搜索。IP 走 `{{ }}` 文本插值（勿在 `flowIpLabel` 内转义）。历史曲线仍是另一块「流量历史」面板 |
| CLI | `easytier-cli peer` 的 `flows` 列：按 **`rate_bytes_per_sec` 降序** 取 top-2（当前最忙，**非**累计最大 —— 与上表的 top-10 排序口径不同） |
| 控制台历史 | `easytier-web` 采样器顺带写 `flow_history`（与 `CollectNetworkInfo` 同次 RPC，只收 MASKED）；API `/api/v1/machines/{machine-id}/flow-history/{inst-id}`（需登录）；曲线按窗口内总流量截断到 40 条 |
| 历史节奏 | 每 **60s** 采样一次（`DEFAULT_SAMPLE_INTERVAL_SECS`），保留 **7 天**，清理任务每 10 分钟扫一次；节点没开 `flow_stats` 则零写入 |

---

## 2. 配置（TOML / managed）

`[flow_stats]`（经 `NetworkConfig` 的可选字段进出协议：`flow_stats_enabled` / `flow_stats_sample_rate` / `flow_stats_max_per_conn` / `flow_stats_max_total` / **`flow_mask_prefix_len_v4|v6`**（注意无 `stats` 前缀）/ `flow_stats_allow_exact_ip`；控制台未携带这些字段时保留本地原表）：

- `enabled`（默认 `false`）
- `sample_rate`（默认 `1` = 全量；≥1，见 §1「采样与估算」）
- `max_per_conn`（默认 `256`）/ `max_total`（默认 `8192`）
- `mask_prefix_len_v4` / `mask_prefix_len_v6`（默认 `24` / `64`；服务端将 `0` 钳制为 `1`，避免「填 0 绕过掩码」）
- `allow_exact_ip`（默认 `false`；与总开关解耦展示，关总开关时清零）

**生效时机**：`[flow_stats]` 仅在**网络实例构造时**读取（`FlowTable::new()` → `configure()`，后者是私有的一次性 API）。控制台改配置**不会**走热更 —— `flow_stats_*` 不在 `InstanceConfigPatch` 里，会落进 `hot_patch_base()` 的整表比对，一旦变化 `web_source_runtime_patch()` 就返回 `None` → `RunNetworkInstance { overwrite: true }` → **整个实例重建**。因此控制台侧「自动生效」，但代价是**该实例所有隧道重连、flow 计数从零开始**（调 `sample_rate` 时尤其要注意，见 §4 与 Roadmap §18.3）；用 TOML 直接启动的进程必须手动重启实例。

**不可配**：条目 TTL 固定 `600s`（`FlowTableConfig::default()` 的 `ttl`；`FlowStatsConfig` 无对应字段，proto 也没有）。想加速回收只能调小 `max_per_conn` / `max_total`。

---

## 3. 存储

- 表 `flow_history`：`conn_id` 为 **PeerConnId UUID 文本**（勿按整数解析）。
- 迁移 `m20261022_000013` 建表，**建表起就是 `conn_id TEXT`**；`m20261023_000014` 是为「曾用 `INTEGER` 建过表的老环境」补的 TEXT 重建 —— 对全新环境是一次 `TEXT→TEXT` 空表重建，**无实际作用**。两条都已在 `origin/dev`，保留即可，但不要以为 13 曾经是 INTEGER。
- 单轮采样写入上限 `MAX_FLOW_ROWS_PER_SAMPLE = 4096`（超出按累计字节降序保留并 `warn`）；查询侧 `MAX_SERIES = 40`。

---

## 4. 已知未闭环

| 项 | 说明 |
|----|------|
| Phase 4 告警 | 本期不做（D16） |
| R6 掩码默认 | v4 `/24`、v6 `/64` 缺 v6 现网复核；保持可配默认 |
| 10 Gbps 小包 | 全量采样约 ~10% 单核；高包率场景应提高 `sample_rate`（见 Roadmap §18） |
