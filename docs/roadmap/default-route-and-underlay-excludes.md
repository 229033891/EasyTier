# 默认路由 Phase 2（`/1`+`/1` 加固）

## Status

- Status: **Roadmap**（按需；Phase 1 / 1b D+ **已落地**）
- 最近审阅：2026-10-06
- **现状 SoT**：[`../current/traffic-steering.md`](../current/traffic-steering.md) §3
- **方案全文留档**：[`../archive/default-route-and-underlay-excludes-2026-10.md`](../archive/default-route-and-underlay-excludes-2026-10.md)
- **相关**：[`traffic-steering-vNext.md`](./traffic-steering-vNext.md)
- 索引：[`../README.md`](../README.md)

---

## 已落地（勿再排期）

| 项 | 去向 |
|----|------|
| 期望集合过滤对端 `/0`、逃生阀 `allow_peer_default_without_exit`、exclude 门控 | Current |
| Android / OHOS / GUI 同步过滤对端 `/0` | Current |
| 桌面 DNS 绑物理默认网卡 | Current |

一句话（已采纳）：**需要全隧道才装会生效的默认路由；不需要就不装；排除路由只服务这条真默认路由。**

---

## Phase 2（开放 · 按需）

**触发条件**：现场出现「物理默认 metric 异常，导致低 metric TUN `/0` 抢不过物理默认」。

**改动**：

- exit 默认路由改为 `0.0.0.0/1` + `128.0.0.0/1`（及 IPv6 等价覆盖），不依赖 metric（对齐 OpenVPN `def1` / WG 常见做法）
- 仍保持 underlay exclude **仅在**将装 TUN 默认时门控
- 全平台（桌面 `apply_route_changes`、Android VpnService、OHOS 聚合）各测一遍

**不做**：无现场证据前全面切 `/1`+`/1`（双路由 + 测试成本高，换简单与性能）。

**验收**：物理 metric 异常宿主上，开 exit 后公网仍经 TUN；关 exit / 出口不可达后恢复物理默认；exclude 仍先于默认装上。
