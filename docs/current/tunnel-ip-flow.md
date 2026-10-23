# 隧道级 IP 流量可观测性（现状）

## Status

- Status: **Current**（Phase 1–3 可依赖行为；Phase 4 告警本期不做）
- 最近审阅：2026-10-23
- 设计决策与实施记录：[`../roadmap/tunnel-ip-flow-observability.md`](../roadmap/tunnel-ip-flow-observability.md)
- 关联：[`peer-connections.md`](./peer-connections.md)（多 PeerConn / `default_conn`）
- 索引：[`../README.md`](../README.md)

本文只描述 **代码今天做什么**。完整决策表（D1–D16）、开销实测与审查关闭项见 Roadmap。

---

## 1. 行为摘要

| 能力 | 现状 |
|------|------|
| 热路径记账 | `easytier-core` `peers::flow_table`：按 **PeerConn × IP × 方向** 计数；TX 记目的 IP，RX 记源 IP（内层 IP 明文侧）；**转发包不解密，不计入 RX** |
| 默认开关 | **`[flow_stats]` 默认关闭**；关闭时热路径仅一次 atomic 读 |
| 精确 IP | **双重闸门**：节点 `allow_exact_ip` **且** 调用方请求 `FlowView::Exact`；否则只出掩码聚合（缺省 MASKED） |
| 速率 | 节点侧固定约 1s 窗口算 `rate_bytes_per_sec`（D11）；**不**对累计 `bytes` 做差分求速 |
| 运行页 | `Status.vue`「IP流量」列：紧凑摘要 + tip；展开行明细 |
| CLI | `easytier-cli peer` 的 `flows` 列：按 **`rate_bytes_per_sec` 降序** 取 top-2（当前最忙，非累计最大） |
| 控制台历史 | `easytier-web` 采样器顺带写 `flow_history`（与 `CollectNetworkInfo` 同次 RPC，只收 MASKED）；API `/api/v1/machines/{machine-id}/flow-history/{inst-id}`（需登录）；曲线按窗口内总流量截断到 40 条 |

---

## 2. 配置（TOML / managed）

`[flow_stats]`（字段经 `NetworkConfig.flow_stats_*` 进出协议）：

- `enabled`（默认 `false`）
- `sample_rate`（≥1；1 = 全量）
- `max_per_conn` / `max_total` / TTL
- `mask_prefix_len_v4` / `mask_prefix_len_v6`（服务端将 `0` 钳制为 `1`，避免「填 0 绕过掩码」）
- `allow_exact_ip`（默认 `false`；与总开关解耦展示，关总开关时清零）

---

## 3. 存储

- 表 `flow_history`：`conn_id` 为 **PeerConnId UUID 文本**（勿按整数解析）。
- 迁移 `m20261022_000013` 建表；`m20261023_000014` 为早期 INTEGER 误型的重建修复（已推送环境保留两条，勿再 squash）。
- 单轮采样写入上限 `MAX_FLOW_ROWS_PER_SAMPLE = 4096`；查询侧 `MAX_SERIES = 40`。

---

## 4. 已知未闭环

| 项 | 说明 |
|----|------|
| Phase 4 告警 | 本期不做（D16） |
| R6 掩码默认 | v4 `/24`、v6 `/64` 缺 v6 现网复核；保持可配默认 |
| 10 Gbps 小包 | 全量采样约 ~10% 单核；高包率场景应提高 `sample_rate`（见 Roadmap §18） |
