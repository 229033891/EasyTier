# EasyTier 文档索引

本目录按用途分层。**改代码时以 Current 为准；拍板未实现能力时改 Roadmap。**

| 分区 | 含义 | 何时更新 |
|------|------|----------|
| [`current/`](./current/) | 代码今天做什么（含已知缺口） | 行为变更的同一 PR |
| [`roadmap/`](./roadmap/) | 待实现决策、阶段与验收 | 立项 / 改语义时 |
| [`ops/`](./ops/) | 构建、部署、升级 | 发布/安装流程变更时 |
| [`archive/`](./archive/) | 历史验证与 benchmark | 一般不再当规划依据 |

每篇文首应有：`Status: Current | Roadmap | Ops | Archive`，以及最近审阅日期。

产品帮助文案（GUI/Web i18n）只描述 **Current** 行为，不引用 Roadmap 草案。

---

## Current — 现状

| 文档 | 说明 |
|------|------|
| [architecture.md](./current/architecture.md) | 可移植 core / host / proto 边界（架构 SoT） |
| [product-map.md](./current/product-map.md) | 产品与 crate / 二进制命名地图 |
| [traffic-steering.md](./current/traffic-steering.md) | 出口节点、子网代理、系统路由：**真实行为与缺口** |
| [data-plane.md](./current/data-plane.md) | DataPlaneRuntime / FFI / Go / WASI（现状摘要；长文在 Archive） |
| [web-managed-config.md](./current/web-managed-config.md) | Web managed config Full/PATCH 接收与 Session 收敛 |
| [socket-protection.md](./current/socket-protection.md) | Host VPN-bypass / `need_protect` 契约 |
| [peer-connections.md](./current/peer-connections.md) | 节点间多 PeerConn 与单 `default_conn` 发送路径 |
| [tunnels-and-transport.md](./current/tunnels-and-transport.md) | 隧道 scheme、打洞/中继与伪装差距 |

## Roadmap — 待实现

| 文档 | 说明 |
|------|------|
| [discussion-proposal-2026-10.md](./roadmap/discussion-proposal-2026-10.md) | **路线整合讨论稿**（排序 / αβγ 节奏 / 待决清单） |
| [market-comparison-2026-10.md](./roadmap/market-comparison-2026-10.md) | **市场对比**（Tailscale / ZeroTier / 蒲公英 / 华为 / VeloCloud） |
| [traffic-steering-vNext.md](./roadmap/traffic-steering-vNext.md) | 出口默认路由、路由所有权、与域名导流统一设计 |
| [default-route-and-underlay-excludes.md](./roadmap/default-route-and-underlay-excludes.md) | 默认路由 / underlay 排除：与 WG·OpenVPN 对比及方案 D |
| [domain-proxy.md](./roadmap/domain-proxy.md) | 域名驱动子网代理 + DNS 答案同步 |
| [multi-link-bonding.md](./roadmap/multi-link-bonding.md) | 多 PeerConn 并行分摊以提升带宽（Draft） |
| [traffic-camouflage.md](./roadmap/traffic-camouflage.md) | 传输伪装 / 抗识别（wss 范式与 TLS 外观，Draft） |
| [web-evolution.md](./roadmap/web-evolution.md) | easytier-web 演进约束与阶段 |
| [github-release-install.md](./roadmap/github-release-install.md) | GitHub/GHCR 安装升级方案 |

## Ops — 运维

| 文档 | 说明 |
|------|------|
| [web-build-deploy.md](./ops/web-build-deploy.md) | Web 打包与 Win/Linux 部署 |
| [web-upgrade.md](./ops/web-upgrade.md) | Web 现网升级与保库 |
| [deploy-install.md](./ops/deploy-install.md) | 安装脚本用法 |
| [ohos-downstream-builds.md](./ops/ohos-downstream-builds.md) | OHOS 下游构建 |

## Archive — 历史记录

验证与 benchmark 见 [`archive/`](./archive/)。含：

| 文档 | 说明 |
|------|------|
| [data-plane-runtime-plan.md](./archive/data-plane-runtime-plan.md) | DataPlane 原实现计划全文（已由 Current 摘要替代日常阅读） |

---

## 维护约定

1. Roadmap 文若仍写「不实现 / Draft」，不得当作已交付功能依据。
2. 从 Roadmap 开工前，先把 Status 改为 Accepted / In progress，并在 Current 中预留或同步缺口说明。
3. 功能落地后：更新 Current；Roadmap 标记 Superseded 或删减为「剩余缺口」。
