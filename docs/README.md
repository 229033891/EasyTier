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
| [tunnel-ip-flow.md](./current/tunnel-ip-flow.md) | **隧道级 IP 流量可观测性（现状）**：每条隧道承载哪些目的/源 IP；**字节口径 = 内层 IP 载荷字节，非链路字节**；精确 IP 双重闸门；采样=估算值；控制台历史（60s / 7 天） |
| [tunnel-ip-flow-fixes.md](./current/tunnel-ip-flow-fixes.md) | **隧道级 IP 流量可观测性 —— 缺陷修复（批次 A，待验收）**：F-1 运行页 tip 未转义节点上报字符串 / F-2 历史查询缺时间上界 / F-3 `configure` 私有化 / F-4 关闭时不 spawn 定时任务；附 F-5–F-8「明确不做」及理由 |
| [latency-measurement-and-display-audit.md](./current/latency-measurement-and-display-audit.md) | **延迟测量与展示审查**（2 个真实测量点 + 派生链；「延迟」列双口径等缺陷；展示侧修法以专题下钻 §5 为准；出口端到端探测已撤回；关联选路修复**已合入、待验收**） |
| [latency-scope-and-relay-visibility.md](./current/latency-scope-and-relay-visibility.md) | **「延迟」vs「VIP可达」口径边界**（Q5/§3.9；两表六种分叉；**§5 序1–5/7/11 前端已改、待验收**；P0×5 / P1×5 / P2×2） |
| [routing-bug-audit-2026-10.md](./current/routing-bug-audit-2026-10.md) | **节点间选路审查**（净确认 **19** 条 + 4 存疑；修复**工作区待合入 / 待验收**：已落地 11 条 —— **B1**/**B5**/**B6**/**B7**/**B8**/**B9**/**B11**/**B13**/**B18**/**B19**/**B20**；部分修 **B2**/**B10**；B3/B4/B17/B12/B15/B16 待决；B14 撤回；见文内 §1e/§1f 与 §5.1 的行为变更/迁移提示） |
| [p2p-direct-throughput-audit.md](./current/p2p-direct-throughput-audit.md) | **P2P 直连吞吐瓶颈审查**（逐行核实出站通路；3 处瓶颈 / 3 处已排除；未压测） |
| [p2p-direct-stability-audit.md](./current/p2p-direct-stability-audit.md) | **P2P 直连稳定性审查**（RDP 卡顿五机制 + 配置层缓解清单；混淆因素待排除） |
| [tailscale-gap-and-conflict-audit.md](./current/tailscale-gap-and-conflict-audit.md) | **vs Tailscale 代码层审查**（Bug/冲突 C1–C6 vs 设计 S1–S5；UI 已连≠VIP；P-AUDIT 修复清单未落地） |
| [packet-loss-investigation-2026-10.md](./current/packet-loss-investigation-2026-10.md) | **现网偶发丢包实测记录**（三轮 ping 原始数据；证伪「ET 控制面」「bond 切路」两个假设；证实代理 TUN 与 ET 隧道**双向互抢路由**；尖峰落点待双侧取证） |
| [console-and-client-interaction-performance.md](./current/console-and-client-interaction-performance.md) | **控制台与客户端交互性能审查**（Web 控制台 / 安卓 GUI 交互慢的根因；OpenWrt LuCI 见外部仓文档） |
| [tunnels-and-transport.md](./current/tunnels-and-transport.md) | 隧道 scheme、打洞/中继与伪装差距 |
| [magic-dns.md](./current/magic-dns.md) | MagicDNS / DnsConfig / OS 接线与覆盖信号（现状 SoT） |
| [magic-dns-manual-wiring.md](./current/magic-dns-manual-wiring.md) | Linux 非 systemd / OpenWrt MagicDNS 手工接线 |
| [dns-policy-audit.md](./current/dns-policy-audit.md) | **域名策略（DnsConfig）审查**（逐行核对全链路；3 处 BUG / 4 处冲突 / 9 项优化；静态核查未复现） |
| [windows-wintun-interface-missing.md](./current/windows-wintun-interface-missing.md) | **Windows WinTun 网卡「找不到」**：`find_interface_index` 失配被当致命错误 → 打死绑网卡的 socket 创建 + ERROR 海啸；根因与修复（P1–P3b 已修，P4 记录在案 / 已登记 P-WIN.1） |
| [windows-installer-reboot-audit.md](./current/windows-installer-reboot-audit.md) | **Windows 安装器重启提示**：Server 不提示 / Win11 提示的根因（NSIS `/REBOOTOK` + `.bak` 残留）；实测推翻「提权不足」旧结论；修复分 A 止血 / B 机器级安装 / C 合规 |
| [message-popup-audit.md](./current/message-popup-audit.md) | **消息弹窗审查**：约 180 个 toast/confirm；冗余（同一事实两处播报 / 5s 轮询无节流 / `t('error')` 键缺失）与缺失（重置密码、清空日志无确认；轮询失败静默）；第一批 10 项已收敛，余项 P1–P4 待决策 |

## Roadmap — 待实现

| 文档 | 说明 |
|------|------|
| [discussion-proposal-2026-10.md](./roadmap/discussion-proposal-2026-10.md) | **路线整合讨论稿**（排序 / αβγ 节奏 / 待决清单） |
| [market-comparison-2026-10.md](./roadmap/market-comparison-2026-10.md) | **市场对比**（Tailscale / ZeroTier / 蒲公英 / 华为 / VeloCloud） |
| [traffic-steering-vNext.md](./roadmap/traffic-steering-vNext.md) | 出口默认路由、路由所有权、与域名导流统一设计 |
| [default-route-and-underlay-excludes.md](./roadmap/default-route-and-underlay-excludes.md) | 默认路由 **Phase 2**（`/1`+`/1`）；Phase 1 论证已归档 |
| [domain-proxy.md](./roadmap/domain-proxy.md) | 域名驱动子网代理 + DNS 答案同步（⚠️ **方案已被统一路由模型取代**，保留作历史论证） |
| [unified-routing-design.md](./roadmap/unified-routing-design.md) | **统一路由模型**（**P1–P6 已落地**，仅剩跨平台验证；尽力分流非零泄漏） |
| [routing-tech-debt.md](./roadmap/routing-tech-debt.md) | **路由热路径技术债**（P-T1 已完成可上线；TD-1/4/5/7 评估后暂不实施） |
| [unified-routing-conflicts.md](./roadmap/unified-routing-conflicts.md) | **统一路由 × 现有功能冲突分析**（已核查：2 处设计缺陷 / 3 处实现约束 / 3 处无冲突） |
| [dns-policy.md](./roadmap/dns-policy.md) | DNS 策略**剩余缺口**（现状见 Current `magic-dns.md`） |
| [multi-link-bonding.md](./roadmap/multi-link-bonding.md) | 多 PeerConn 聚合：Phase 2a/2b **已合入**（出厂默认 `bond_count=2`）；Phase 3 出口/`bind_device` 多样性待做 |
| [tunnel-ip-flow-observability.md](./roadmap/tunnel-ip-flow-observability.md) | **隧道级 IP 流量可观测性**：每条隧道在跑哪些目的/源 IP；日志/事件/遥测三通道是否合并（结论：语义 3 类、物理 2 通道）；本机 vs 控制台可见性分维度规划；**Phase 0 实测 + Phase 1 记账内核 + Phase 2 协议/UI + Phase 3 控制台配置项/历史落库/图表 已落地**（仅剩 Phase 4 告警，本期不做；剩余风险见 §17.6） |
| [traffic-camouflage.md](./roadmap/traffic-camouflage.md) | 传输伪装 / 抗识别（wss 范式与 TLS 外观，Draft） |
| [traffic-camouflage-phase-a-todo.md](./roadmap/traffic-camouflage-phase-a-todo.md) | Phase A TODO：中继 + wss:443 Ops/状态（**暂不实现**） |
| [connection-stability-todo.md](./roadmap/connection-stability-todo.md) | 连接稳定性优化 TODO（相对 OpenVPN/IPsec；**端点可自定义，不假设 443**） |
| [relay-forward-egress-queue.md](./roadmap/relay-forward-egress-queue.md) | 中继转发每目的 peer 有界出口队列（消共享 router HOL；代码已落地待现网） |
| [stability-abc-plan.md](./roadmap/stability-abc-plan.md) | **稳定性 A/B/C 施工计划**（A=P1.5 拆连雪崩防护；B=配置引导；C=现网验收；不对标 DERP） |
| [live-next-hop.md](./roadmap/live-next-hop.md) | **已建立线路才转发**（**方案重议**：首版改法不被接受，无方向性 ⇒ 环路；目标语义=已建立+在通往目的的路径上+无环；比较规则=字典序 `成本→跳数→稳定身份键`；Phase A/C/F/F2/B/D 已落地待现网，**B/D 限等代价旁路**） |
| [code-review-fixes-2026-10.md](./roadmap/code-review-fixes-2026-10.md) | **全仓代码审查修复清单**（15 条 CR-01…CR-15 + 10 条附带发现；`panic=abort` 放大前提；分 Phase A–D） |
| [web-evolution.md](./roadmap/web-evolution.md) | easytier-web 演进约束与阶段 |
| [web-console-runtime-diagnostics.md](./roadmap/web-console-runtime-diagnostics.md) | 控制台侧栏：**系统诊断** + **运行日志**（预定 vs 实际 / ring buffer；MVP 已实现待验收） |
| [web-console-log-persistence.md](./roadmap/web-console-log-persistence.md) | 控制台**日志本地留存**：默认 `./logs`+warn 滚动（可写性预检降级）+ 运行日志页读文件筛选（A/B/C 已落地，待验收） |
| [client-web-console-interaction.md](./roadmap/client-web-console-interaction.md) | 客户端 ↔ 控制台交互：超时 / Start 覆盖 / 会话抖动 / 协议降级（P0–P2）+ 交互性能审查（§9；优先 S4/S5） |
| [poll-to-push-roi.md](./roadmap/poll-to-push-roi.md) | **轮询→推送 ROI**：P0 GUI 1s 事件化；P1 状态页混合；§4.4 desired 下发另立；心跳/安卓纯推不做 |
| [tray-run-state.md](./roadmap/tray-run-state.md) | **托盘运行态语义**：判据从「客户端已连接」改为按实例聚合（灰=无网在跑 / 黄=启动中 / 绿=在网 / 红=需介入）；代码已落地，**待真机验收** |
| [gui-connected-vs-reachable.md](./roadmap/gui-connected-vs-reachable.md) | **已连接 vs VIP 可达**：**按链路**数据面 ICMP 探测（每条 PeerConn 各测），行级 Tag 按链路、`[M/N]` 按节点；代码已落地，**待现网验收**（GUI/`frontend-lib` 一期；LuCI 二期） |
| [config-delete-authority.md](./roadmap/config-delete-authority.md) | 配置删除权：bound+Web 仅控制台；客户端删门与停用保留 |
| [local-config-residue.md](./roadmap/local-config-residue.md) | 控制台删除后本地/GUI 残留：分层收敛（P0–P2 代码已落地；待人工回归） |
| [config-sync-per-end.md](./roadmap/config-sync-per-end.md) | 配置同步**各端一致性核验**：删除权 / W 权威 / 残留 UI 覆盖矩阵（有意不统一；gui 含 Android 已闭环） |
| [console-save-and-push-semantics.md](./roadmap/console-save-and-push-semantics.md) | 控制台 Save→Push：落库 / 启停 / reconcile 语义 |
| [github-release-install.md](./roadmap/github-release-install.md) | GitHub/GHCR 安装升级方案 |
| [config-vs-run-pages.md](./roadmap/config-vs-run-pages.md) | **配置页 / 运行页**（主体已落地；可选「保存并运行」待做） |
| [credential-pin-preference-failover.md](./roadmap/credential-pin-preference-failover.md) | Admin pin 被 preference failover 绕过（§4.1 3+2 + §4.6 客户端身份命中已落地；responder 待开工；待 CI） |

## Ops — 运维

| 文档 | 说明 |
|------|------|
| [web-build-deploy.md](./ops/web-build-deploy.md) | Web 打包与 Win/Linux 部署 |
| [windows-build-pack.md](./ops/windows-build-pack.md) | Windows 一键打包：GUI NSIS（含 `ET-cli`）+ 无头 `ET-windows-*`（对齐 CI） |
| [windows-msvc-local-build.md](./ops/windows-msvc-local-build.md) | Windows 本机 MSVC：已装 Build Tools 仍编不过时，先 vcvars 再 cargo |
| [web-upgrade.md](./ops/web-upgrade.md) | Web 现网升级与保库 |
| [deploy-install.md](./ops/deploy-install.md) | 安装脚本用法 |
| [docker-compose-deploy.md](./ops/docker-compose-deploy.md) | Docker Compose：节点 / 控制台 / 同时启动（SSH 命令） |
| [ohos-downstream-builds.md](./ops/ohos-downstream-builds.md) | OHOS 下游构建 |
| [release-version-bump.md](./ops/release-version-bump.md) | **版本号 bump 必改清单**（10 文件 / 17 处）+ 发布分支惯例 |
| [android-startup-auto-stop.md](./ops/android-startup-auto-stop.md) | 安卓启动后马上自动停：现象 / 日志定性 / §6 同 netId 去重+宽限期（待新包验收）/ 清数据与重装 |
| [ci-failure-log.md](./ops/ci-failure-log.md) | **CI/ET Test 失败追加日志** + 提交前自检（clippy / trait 导入等） |
| [doc-update-after-task.md](./ops/doc-update-after-task.md) | **任务收尾文档决策表**（Current 严门槛 + 按类型最少更新集） |

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
2. 从 Roadmap 开工前，先把 Status 改为 Accepted / In progress；Current 仅预留**已知缺口**（勿把未实现方案写成现状）。
3. **Current 严门槛**：代码落地但未达「可依赖」（待联调 / 待现网）→ 只更新绑定 Roadmap；行为已可依赖后再改对应 `current/`，Roadmap 标剩余缺口 / Superseded / Archive。细则与决策表见 [`ops/doc-update-after-task.md`](./ops/doc-update-after-task.md)。
4. 对外与 Current/Ops/Roadmap 文遵守上文「品牌与溯源」硬约束；涉及 `main`/`dev` 分支同步的历史跟踪放 `archive/`。
5. **引用验证结果必须写明口径**（跑的是哪个 feature 组合 / 哪条命令）—— 见下节「本地构建与验证」的 feature 门控说明。裸数字（如「1150 passed」）不带口径就是错误信息。

---

## 本地构建与验证

> **结论先行**：本机**可以**完整跑 `cargo check` / `cargo test` / `cargo clippy` / `cargo fmt`。
> 历史上多次出现「本环境编不了、缺原生工具链（`zstd-sys`）」的判断 —— 那是**沙箱未禁用**时的现象，**不是真的缺工具链**。2026-10-17 已实测推翻：`cargo check -p easytier` 通过（`zstd-sys` / `zstd-safe` / `zstd` 均正常编译）。
> 引用旧文档时注意：`docs/roadmap/code-review-fixes-2026-10.md` 与 `docs/current/tailscale-gap-and-conflict-audit.md` 曾以此为「不做某项改动」的理由，该理由已不成立。

### 前提（两条缺一不可）

1. **禁用沙箱** —— 调用 Bash 工具时传 `dangerouslyDisableSandbox: true`。
   沙箱会拦 `reg.exe`（使 `vcvars64.bat` 失败），并让写 `CARGO_TARGET_DIR` 报 `os error 5（拒绝访问）`。
2. **注入 MSVC 环境** —— 普通终端跑一次 `vcvars64.bat`（或从「x64 Native Tools Command Prompt」启动）即可；**自动化工具里的 Bash 禁止调 `cmd.exe`**，只能手写下面这几个变量：

```bash
export MSVC="/c/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/VC/Tools/MSVC/14.44.35207"
export PATH="$PATH:$MSVC/bin/Hostx64/x64"
export INCLUDE="C:\\Program Files (x86)\\Microsoft Visual Studio\\2022\\BuildTools\\VC\\Tools\\MSVC\\14.44.35207\\include;C:\\Program Files (x86)\\Windows Kits\\10\\Include\\10.0.26100.0\\ucrt;C:\\Program Files (x86)\\Windows Kits\\10\\Include\\10.0.26100.0\\shared;C:\\Program Files (x86)\\Windows Kits\\10\\Include\\10.0.26100.0\\um"
export LIB="C:\\Program Files (x86)\\Microsoft Visual Studio\\2022\\BuildTools\\VC\\Tools\\MSVC\\14.44.35207\\lib\\x64;C:\\Program Files (x86)\\Windows Kits\\10\\Lib\\10.0.26100.0\\ucrt\\x64;C:\\Program Files (x86)\\Windows Kits\\10\\Lib\\10.0.26100.0\\um\\x64"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR='D:\EasyTier\.cache\cargo-target'
```

- `14.44.35207` / `10.0.26100.0` 随本机安装变化，按实际目录替换。
- 用 **Bash 工具**跑 cargo；PowerShell 工具对 `.exe` 走管道会报 `CantActivateDocumentInPipeline`。
- `CARGO_PROFILE_DEV_DEBUG=0` 省掉调试信息，编译更快、占盘更小。

### 常用命令

| 目的 | 命令 |
|---|---|
| 类型检查（core） | `cargo check -p easytier-core` |
| 类型检查（**全链路**，含 TUN / CLI） | `cargo check -p easytier` |
| 单测（**默认 features**） | `cargo test -p easytier-core --lib` → **986** |
| 单测（含数据面 / TCP 代理） | `cargo test -p easytier-core --lib --features proxy-smoltcp-stack` → **1150** |
| lint | `cargo clippy -p easytier-core -p easytier-proto --tests` |
| 格式（CI 会卡） | `cargo fmt --check` |
| 前端单测 | `cd easytier-web/frontend-lib && node node_modules/vitest/vitest.mjs run --config vitest.config.ts` → **192** |

### ⚠️ feature 门控：引用测试数**必须**写明口径

`easytier-core` 默认 features = `aes-gcm` / `endpoint-discovery` / `extended-services` / `management` / `tcp-hole-punch`，**不含** `proxy-smoltcp-stack`，而 `gateway/dataplane/**` **整个模块**（`gateway/mod.rs`）被它门控。于是：

- 裸 `--lib` → **986** passed
- 叠加 `--features proxy-smoltcp-stack` → **1150** passed（`gateway/dataplane/tests.rs` 一个文件就 19 条）

两者**不是同一口径，不能相减**。同理，`proxy-packet` 门控的代码（`gateway/magic_dns/{packet,observe}.rs`、`instance/packet_plane` 的 `register_magic_dns_resolver` 等）在裸 `cargo check -p easytier-core` 下**静默跳过、不报错** —— 改到这些文件时必须显式加 `--features proxy-packet`，或直接 `cargo check -p easytier`（该 crate 经 `quic`/`kcp`/`icmp-proxy` 默认开 `proxy-packet`）。

### ⛔ 打包 / 出安装包：**必须**走 `script/` 里的脚本，不要手搓命令

> **硬约束：本地打包一律从 `script/` 目录用现成入口，禁止自己拼 `cargo build` / `pnpm tauri build` 序列。**
> 手搓几乎必然失败，而且失败点（MSVC `D8050`、esbuild 临时目录、WebView2 引导包、wasm 工具链）看起来像"环境坏了"，
> 实际是脚本已经替你处理掉的坑。**2026-10-23 已踩过一次**：直接 `cargo test` 反复卡在
> `cl: 命令行 error D8050: 无法执行 c1.dll` —— 那正是脚本用 `CARGO_PROFILE_DEV_DEBUG=0` + 短 `CARGO_TARGET_DIR`
> 规避掉的问题（见 `script/build-common.ps1` `Initialize-EasytierWindowsCargoEnv`）。

**目录名是单数 `script\`，不是 `scripts\`**（仓库根目录没有 `scripts/`，那是 `frontend-lib` 内部的目录）。

| 场景 | 入口 |
|---|---|
| 完整 Windows 包（GUI NSIS + 无头 zip） | `script\easytier-windows-fast.cmd`（日常）/ `-release.cmd`（正式）/ `-debug.cmd` |
| 只要无头包（最快） | `.\script\build-easytier-windows.ps1 -Profile Fast -SkipGui` |
| 只要 NSIS | `.\script\build-easytier-windows.ps1 -SkipHeadless` |
| 仅 Web embed | `script\easytier-web-fast.cmd` / `-release.cmd` / `-debug.cmd` |
| 提交前自检（fmt + clippy + features） | `script\easytier-test-fast.cmd` / `-full.cmd` |

- 脚本会自己设置 `CARGO_TARGET_DIR=.cache/cargo-target`、`CARGO_PROFILE_DEV_DEBUG=0`、可写 `TEMP`，并从 Git 目录补 `perl`。
- 输出在 `artifacts\`，日志在 `artifacts\logs\`（失败先看这里，不要靠猜）。
- 需要 GUI/NSIS 时脚本会在**编译前**检查 WebView2 引导包，缺失立刻失败并打印下载地址。
- 细节见 [`ops/windows-build-pack.md`](./ops/windows-build-pack.md)；MSVC 相关见 [`ops/windows-msvc-local-build.md`](./ops/windows-msvc-local-build.md)。

> 上文的 `cargo` / `node` 命令只用于**改代码后的快速校验**（check / test / clippy / fmt），
> **不能**用于产出安装包。两者不要混用。

### 已知坑

| 现象 | 处理 |
|---|---|
| `failed to write …\.fingerprint\…\lib-*.json` + `拒绝访问。(os error 5)` | 删掉那个 `.json` 后重跑即可（偶发文件锁，非权限配置问题） |
| `cl: 命令行 error D8050: 无法执行 …\c1.dll: 未能将命令行放入调试记录中` | MSVC 在 `-Z7` + 长路径下写 PDB 失败。**不要**改用 `--release` 瞎试：设 `CARGO_PROFILE_DEV_DEBUG=0` 并用短 `CARGO_TARGET_DIR`（如 `.cache/cargo-target`）；打包场景直接用 `script\` 脚本（已内置）。详见上文「⛔ 打包」 |
| `--features full` 失败：`openssl-sys: Command 'perl' not found` | `full` 含 `openssl-crypto`，要从源码构建 OpenSSL。CI 的 `cargo clippy --all-targets --features full -- -D warnings` **本地无法复现**；要跑需先装 Strawberry Perl |
| 默认 features 下出现一批「never used」告警 | 多数是 feature 门控造成的**假死代码**，**不要盲删** |

### 前端 / proto（不必走 pnpm）

- 类型检查：`node easytier-web/frontend-lib/node_modules/vue-tsc/bin/vue-tsc.js -b`
- 单测：`node easytier-web/frontend-lib/node_modules/vitest/vitest.mjs run --config vitest.config.ts`
- proto TS 生成：`node easytier-web/frontend-lib/scripts/codegen-proto.mjs`（注意在 `frontend-lib/` 下，**不是**仓库根的 `scripts/`；`@protobuf-ts/protoc` 自带 protoc，无需系统安装）
- Rust 侧 proto：由 `easytier-proto/build/main.rs` 自动下载 protoc（Windows 走 GitHub release）
- 不跑 cargo 时的替代校验：`rustfmt --edition 2024 --check <file>`（edition 必须写 2024，否则 let-chains 会误报「only allowed in Rust 2024」）

