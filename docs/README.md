# EasyTier 文档索引

本目录按用途分层。**改代码时以 Current 为准；拍板未实现能力时改 Roadmap。**

其他 agent / 复查：**先读** [`current/system-overview.md`](./current/system-overview.md)，再按任务下钻专题。

| 分区 | 含义 | 何时更新 |
|------|------|----------|
| [`current/`](./current/) | 代码今天做什么（含已知缺口） | 行为变更的同一 PR |
| [`roadmap/`](./roadmap/) | 待实现决策、阶段与验收 | 立项 / 改语义时 |
| [`ops/`](./ops/) | 构建、部署、升级 | 发布/安装流程变更时 |
| [`archive/`](./archive/) | 历史验证与 benchmark | 一般不再当规划依据 |

每篇文首应有：`Status: Current | Roadmap | Ops | Archive`，以及最近审阅日期。

产品帮助文案（GUI/Web i18n）只描述 **Current** 行为，不引用 Roadmap 草案。

**品牌与溯源（硬约束）**：`current/`、`ops/`、`roadmap/`、根目录 `README*`，以及产品帮助链接（GUI/Web i18n）**不得**出现源项目仓库、官网、社区或旧镜像身份（例如第三方 org 名、源站域名、「上游 / upstream / fork of …」溯源表述）。本仓库自有地址（`229033891/EasyTier`、`ghcr.io/229033891/et`）与协议/DNS 语义上的「上游」除外。`archive/` 可保留历史上下文，但**不得当作规划依据**。

crate 内设计稿（非本索引）：`easytier/docs/`（凭据计划已标 Archive；`RelayPeerMap` / Secure Mode 规格已加 Status，行号可能漂移）。

---

## Current — 现状

| 文档 | 说明 |
|------|------|
| [system-overview.md](./current/system-overview.md) | **Agent 入口**：系统如何串起来、阅读顺序与代码锚点 |
| [architecture.md](./current/architecture.md) | 可移植 core / host / proto 边界（架构 SoT） |
| [architecture-overview.md](./current/architecture-overview.md) | **中文架构总览与稳定性评估**（分层/交付/配置权威/流量模型 + 压力点兜底评级 + 已核实偏差与缺陷） |
| [product-map.md](./current/product-map.md) | 产品与 crate / 二进制命名地图 |
| [traffic-steering.md](./current/traffic-steering.md) | 出口节点、子网代理、系统路由：**真实行为与缺口** |
| [underlay.md](./current/underlay.md) | **物理链路保活与黑洞防护**（排除路由 / DNS 绑定 / Android 网络变化；四种 underlay 消歧） |
| [data-plane.md](./current/data-plane.md) | DataPlaneRuntime / FFI / Go / WASI（现状摘要；长文在 Archive） |
| [web-managed-config.md](./current/web-managed-config.md) | Web managed config Full/PATCH 接收与 Session 收敛 |
| [desktop-gui-and-config-server.md](./current/desktop-gui-and-config-server.md) | 桌面 GUI / `ET-Gui` 进程模型与 config-server 回写路径 |
| [socket-protection.md](./current/socket-protection.md) | Host VPN-bypass / `need_protect` 契约 |
| [peer-connections.md](./current/peer-connections.md) | 节点间多 PeerConn 与单 `default_conn` 发送路径 |
| [tunnels-and-transport.md](./current/tunnels-and-transport.md) | 隧道 scheme、打洞/中继与伪装差距 |
| [magic-dns.md](./current/magic-dns.md) | MagicDNS / DnsConfig / OS 接线与覆盖信号（现状 SoT） |
| [magic-dns-manual-wiring.md](./current/magic-dns-manual-wiring.md) | Linux 非 systemd / OpenWrt MagicDNS 手工接线 |

## Roadmap — 待实现

| 文档 | 说明 |
|------|------|
| [discussion-proposal-2026-10.md](./roadmap/discussion-proposal-2026-10.md) | **路线整合讨论稿**（排序 / αβγ 节奏 / 待决清单） |
| [market-comparison-2026-10.md](./roadmap/market-comparison-2026-10.md) | **市场对比**（Tailscale / ZeroTier / 蒲公英 / 华为 / VeloCloud） |
| [traffic-steering-vNext.md](./roadmap/traffic-steering-vNext.md) | 出口默认路由、路由所有权、与域名导流统一设计 |
| [default-route-and-underlay-excludes.md](./roadmap/default-route-and-underlay-excludes.md) | 默认路由 **Phase 2**（`/1`+`/1`）；Phase 1 论证已归档 |
| [domain-proxy.md](./roadmap/domain-proxy.md) | 域名驱动子网代理 + DNS 答案同步 |
| [dns-policy.md](./roadmap/dns-policy.md) | DNS 策略**剩余缺口**（现状见 Current `magic-dns.md`） |
| [multi-link-bonding.md](./roadmap/multi-link-bonding.md) | 多 PeerConn 聚合：Phase 2a/2b **已合入**（默认 `bond_count=1`）；Phase 3 出口/`bind_device` 多样性待做 |
| [traffic-camouflage.md](./roadmap/traffic-camouflage.md) | 传输伪装 / 抗识别（wss 范式与 TLS 外观，Draft） |
| [connection-stability-todo.md](./roadmap/connection-stability-todo.md) | 连接稳定性优化 TODO（相对 OpenVPN/IPsec；**端点可自定义，不假设 443**） |
| [web-evolution.md](./roadmap/web-evolution.md) | easytier-web 演进约束与阶段 |
| [web-console-runtime-diagnostics.md](./roadmap/web-console-runtime-diagnostics.md) | 控制台侧栏：**系统诊断** + **运行日志**（预定 vs 实际 / ring buffer；MVP 已实现待验收） |
| [web-console-log-persistence.md](./roadmap/web-console-log-persistence.md) | 控制台**日志本地留存**：默认 `./logs`+warn 滚动（可写性预检降级）+ 运行日志页读文件筛选（A/B/C 已落地，待验收） |
| [client-web-console-interaction.md](./roadmap/client-web-console-interaction.md) | 客户端 ↔ 控制台交互：超时 / Start 覆盖 / 会话抖动 / 协议降级（P0–P2） |
| [github-release-install.md](./roadmap/github-release-install.md) | GitHub/GHCR 安装升级方案 |
| [config-vs-run-pages.md](./roadmap/config-vs-run-pages.md) | **配置页 / 运行页**（主体已落地；可选「保存并运行」待做） |
| [credential-pin-preference-failover.md](./roadmap/credential-pin-preference-failover.md) | Admin pin 被 preference failover 绕过（§4.1 3+2 + §4.6 客户端身份命中已落地；responder 待开工；待 CI） |

## Ops — 运维

| 文档 | 说明 |
|------|------|
| [web-build-deploy.md](./ops/web-build-deploy.md) | Web 打包与 Win/Linux 部署 |
| [windows-build-pack.md](./ops/windows-build-pack.md) | Windows 一键打包：GUI NSIS + 无头 `ET-windows-*`（对齐 CI） |
| [windows-msvc-local-build.md](./ops/windows-msvc-local-build.md) | Windows 本机 MSVC：已装 Build Tools 仍编不过时，先 vcvars 再 cargo |
| [web-upgrade.md](./ops/web-upgrade.md) | Web 现网升级与保库 |
| [deploy-install.md](./ops/deploy-install.md) | 安装脚本用法 |
| [docker-compose-deploy.md](./ops/docker-compose-deploy.md) | Docker Compose：节点 / 控制台 / 同时启动（SSH 命令） |
| [ohos-downstream-builds.md](./ops/ohos-downstream-builds.md) | OHOS 下游构建 |
| [release-version-bump.md](./ops/release-version-bump.md) | **版本号 bump 必改清单**（10 文件 / 17 处）+ 发布分支惯例 |
| [android-startup-auto-stop.md](./ops/android-startup-auto-stop.md) | 安卓启动后马上自动停：现象 / 日志定性 / §6 同 netId 去重+宽限期（待新包验收）/ 清数据与重装 |

## Archive — 历史记录

验证与 benchmark 见 [`archive/`](./archive/)。含：

| 文档 | 说明 |
|------|------|
| [data-plane-runtime-plan.md](./archive/data-plane-runtime-plan.md) | DataPlane 原实现计划全文（已由 Current 摘要替代日常阅读） |
| [default-route-and-underlay-excludes-2026-10.md](./archive/default-route-and-underlay-excludes-2026-10.md) | 默认路由 / underlay 方案 D+ 全文（已由 Current + Phase 2 薄页替代日常阅读） |
| [kcp-control-reliability-design-2026-09-14.md](./archive/kcp-control-reliability-design-2026-09-14.md) | KCP 控制报文可靠性设计 |
| [kcp-control-reliability-validation-2026-09-14.md](./archive/kcp-control-reliability-validation-2026-09-14.md) | KCP 控制报文可靠性验证 |
| [tcp-proxy-flow-key-validation-2026-09-13.md](./archive/tcp-proxy-flow-key-validation-2026-09-13.md) | TCP proxy flow-key 验证 |
| [tcp-proxy-half-close-validation-2026-09-14.md](./archive/tcp-proxy-half-close-validation-2026-09-14.md) | TCP proxy 半关闭验证 |
| [quic-proxy-memory-benchmark-2026-07-27.md](./archive/quic-proxy-memory-benchmark-2026-07-27.md) | QUIC proxy 内存 benchmark |
| [upstream-port-todo.md](./archive/upstream-port-todo.md) | `main` → `dev` cherry-pick 历史跟踪（P0–P2 已落袋；非日常规划） |
| [android-vpn-connection-audit-2026-10-07.md](./archive/android-vpn-connection-audit-2026-10-07.md) | 安卓 App 连接/卡断审查（A1–A14；代码侧已修） |
| [magic-dns-static-host-wildcard-audit-2026-10-09.md](./archive/magic-dns-static-host-wildcard-audit-2026-10-09.md) | MagicDNS 静态主机通配审查（D1–D8；第三轮已修 D2/D3/D4/D6-2，D5 有意保留） |

---

## 维护约定

1. Roadmap 文若仍写「不实现 / Draft」，不得当作已交付功能依据。
2. 从 Roadmap 开工前，先把 Status 改为 Accepted / In progress，并在 Current 中预留或同步缺口说明。
3. 功能落地后：更新 Current；Roadmap 标记 Superseded 或删减为「剩余缺口」。
4. 对外与 Current/Ops/Roadmap 文遵守上文「品牌与溯源」硬约束；涉及 `main`/`dev` 分支同步的历史跟踪放 `archive/`。
