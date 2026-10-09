# 连接稳定性优化 TODO

## Status

- Status: **Roadmap**（Checklist；P-UX + P0.1–P0.4 + P1.1–P1.3/P1.6–P1.8 + P2.1–P2.3 + P-AUTO.L1 已落地；下一步 P0.5 或 P1.5）
- 日期：2026-10-06
- 最近审阅：2026-10-09（P2.4 bonding Phase 2 标记已合入；安卓审计迁 Archive）
- 背景：对照 OpenVPN / IPsec 的「固定隧道 + 强保活」模型，梳理 EasyTier Mesh（多 PeerConn + 打洞 + 中继）的稳定性差距与可落地项；**另纳入 2026-10-06 用户反馈：高级选项互斥缺校验、长表单占空间、单协议配置失败后无智能回落**
- 相关 Current：[`../current/peer-connections.md`](../current/peer-connections.md)、[`../current/tunnels-and-transport.md`](../current/tunnels-and-transport.md)
- 相关 Roadmap：[`traffic-camouflage.md`](./traffic-camouflage.md)、[`multi-link-bonding.md`](./multi-link-bonding.md)（Phase 2a/2b 已合入；Phase 3 待做）、[`market-comparison-2026-10.md`](./market-comparison-2026-10.md)
- 索引：[`../README.md`](../README.md)

**硬约束（全文适用）**

- **不得假设 TCP/443 或 HTTPS 一定可用。** 部分网络禁 443、仅放行特定端口、或只允许 UDP/自定义 TCP。
- 所有「保底路径 / 中继 / 预设」必须支持 **自定义 scheme、host、port、path**（及等价 listener URL），443 / `wss` 只是**推荐示例**，不是唯一默认。
- 文档、UI、模板、健康检查均不得写死「只有 443 才能通」。

---

## 1. 问题摘要

| # | 现象 | 根因（相对 OpenVPN / IPsec） |
|---|------|------------------------------|
| S1 | P2P/打洞失败或被限后体验差 | 有中继能力，缺**可配置**的「失败必有回落路径」产品档 |
| S2 | 误以为多 PeerConn = 更稳/更快 | 发送只走 `default_conn`（冗余切换，非 bonding） |
| S3 | 弱网下 UDP 抖动、闪断 | 自适应 ping 较好，但失败阈值偏硬；部分环境需改走 TCP/`wss`/QUIC |
| S4 | TCP 打洞 idle 被中间设备掐断 | #2632 已合入：TCP hole-punch 1s ping cap；待验证 |
| S5 | 「学 OpenVPN 上 443」在部分环境无效 | **443 本身也可能不可用** → 必须自定义端口与协议 |
| S6 | 低延迟但高丢包/高抖动的路径仍被选中 | ~~`select_conn` / `latency_first` 几乎只看 RTT~~ → 同 peer 与 OSPF 边代价均已用综合质量分（P1.7 / P2.3） |
| S7 | 互斥/依赖选项可同时选中，误配后难排查 | `api_input.rs` 逐 flag 独立赋值、无互斥校验；`Config.vue:226-268` 平铺 Checkbox，无联动隐藏/禁用 |
| S8 | 用户只配一种协议（如 UDP），受限环境直接全失败 | 无 scheme 优先列表与自动降级；`default_protocol` 仅单值（`Config.vue:281-292`），失败仍重试同协议 |

EasyTier 优势（保持）：多 scheme、STUN/打洞、OSPF 选路、自适应 Ping（1s～32s）+ `liveness-echo-v1`、手动 peer **1s** 重连。  
优化方向是 **可配置保底 + 可调保活 + 多指标质量选路 + 清晰回落**，不是重做 IKE/OpenVPN 协议。

---

## 2. 设计原则：自定义优先

### 2.1 保底路径 = 用户声明的 URL，不是固定 443

| 项 | 要求 |
|----|------|
| Scheme | 至少支持现有：`tcp` / `udp` / `ws` / `wss` / `quic` / `wg`（及已启用的 `faketcp` 等） |
| 端口 | **任意合法端口**；示例可用 443，模板须同时给出非 443 示例（如 `8443`、`9443`、内网自选） |
| Host / Path | 域名或 IP；`ws`/`wss` 支持自定义 path |
| 多条备选 | 允许配置 **有序列表**（主路径失败按序尝试），而非单一硬编码地址 |
| 探测 | 健康检查 / 连通探测目标 = 用户配置的 URL，不探测「默认 443」 |

### 2.2 产品档语义（已简化，不再设档位）

2026-10-06 拍板：用户要的始终是两设备之间低延迟/低抖动/低丢包，不区分直连还是中转。
因此**不设路径档位**：全部已配置 peer URL 恒维持连接（含打洞），走哪条只看质量分
（§2.3 / P1.7；对端间 OSPF 边代价已同源编码，P2.3）。`disable_p2p` / `p2p_only` /
「允许作为中转节点」保留为硬约束（政策/效率）。`prefer_peer_relay`：UI 隐藏；
**不**并入 `lazy_p2p`；仅 TOML 兼容字段，可选启用 OSPF 对端中继拓扑投影。

「中继」在文案中指 **用户（或网络管理员）提供的可达端点**，不隐含公网 443。

### 2.3 链路质量：延迟 + 丢包 + 抖动（必须一起考虑）

**结论：要。** 只按延迟选路会在弱网/Wi‑Fi/跨运营商场景选到「很快但不稳」的路径（TCP 重传、卡顿、语音视频花屏），体感往往差于略高延迟但低丢包、低抖动的路径。

| 指标 | 今日 | 目标 |
|------|------|------|
| **延迟（RTT）** | `select_conn` 与 OSPF `path_latency` / peer-center `latency_ms` | 保留，作为综合分的一项 |
| **丢包率** | Ping 算 `loss_rate`；**已参与**同 peer `select_conn` **与** OSPF 边代价（P2.3） | 已落地 |
| **抖动（Jitter）** | `WindowLatency::get_jitter_us`；**已参与**同 peer `select_conn` **与** OSPF 边代价（P2.3） | 已落地 |

推荐综合分（草案，权重可配；数值仅示意）：

```text
score = w_lat * norm(rtt)
      + w_loss * norm(loss_rate)      // 丢包权重要高于「略差一点的 RTT」
      + w_jitter * norm(jitter)
```

硬规则建议：

1. **丢包熔断阈值**：`loss_rate` 超过可配上限时，该 PeerConn **禁止成为 `default_conn`**（有替代路径时），避免「低延迟高丢包」饿死业务。
2. **防抖切换（hysteresis）**：新路径综合分需优于当前路径超过阈值（或连续 N 个探测窗口更优）才切换，避免抖动本身导致路径来回切。
3. **分层落地**：同 peer `select_conn`（P1.7）与跨 peer OSPF 边代价（P2.3）均已用综合质量分；OSPF 侧靠发布 hysteresis + 上报节流防震荡。
4. **不对标** 企业 SD-WAN 包级 DMPO；首期只做 **选路/选 conn 的质量分**，不做按包喷洒。

与 [`market-comparison-2026-10.md`](./market-comparison-2026-10.md)「链路质量选路（时延/丢包）」增强期、以及 bonding 文「按链路质量加权」一致；**稳定性 TODO 将其从「可选增强」升为明确交付项**。

### 2.4 与 `traffic-camouflage.md` 的关系

- 该文的「`wss` + 443 + 合法证书」视为 **推荐范式之一**。
- 本文将其升格为：**同一套回落机制，端点完全自定义**；443 不可用时换端口/换 scheme 仍须达标。
- 深度 TLS 指纹 / REALITY 仍属增强（Backlog），不阻塞本 TODO 的 P0。

### 2.5 配置正确性原则（新增草案，对应 S7）

互斥一律「可保存但要显式确认」不如「直接禁选 + 文案解释」；依赖一律「隐藏或禁用被置灰项」，不让用户填无效值：

| 组 | 规则（草案） | UI 行为 |
|----|--------------|---------|
| `disable_p2p` vs `p2p_only` / `lazy_p2p` / `need_p2p` | `disable_p2p=true` 时后三者无意义（`need_p2p` 仅对 `lazy_p2p` 对端有效） | 从严：禁用并 tooltip 说明（已拍板） |
| `p2p_only` vs `latency_first` | `p2p_only` 下 `latency_first` 被代码强制忽略（`peers/context.rs:534`） | 禁用 `latency_first` |
| `p2p_only` vs `disable_relay_data` / `prefer_peer_relay` / 中继白名单 | 已不要中转 | 禁用/隐藏中继相关 |
| `disable_tcp_hole_punching` + `disable_udp_hole_punching` 全关 | 只能走中转/手动 peer | 强提示横幅，不禁存 |
| `enable_kcp_proxy` + `disable_kcp_input` 同机 | 只发不收，合法但多为误配 | warn，不禁存 |
| `enable_kcp_proxy` + `enable_quic_proxy` 全开 | 可共存但双倍开销 | 建议单选，提示 |
| `disable_ipv6=true` | `ipv6_public_addr_auto/provider/prefix` 无效 | 隐藏整组 |
| `no_tun=true` | `dev_name/mtu` 无效；`exit_nodes` 默认路由不装；MagicDNS 不自动接线 | 禁用 `dev_name/mtu`，`exit_nodes`/DNS 处提示 |
| `disable_encryption=true` | `encryption_algorithm` 无效（今日仅 `disabled`，见 `Config.vue:780`） | 隐藏算法下拉 |
| `bind_device` 在纯虚拟/容器 | 可能无物理网卡 | 保留，但帮助注明 |

全网一致性参数（`encryption_algorithm` / `data_compress_algo` / `disable_encryption` / 网密）不得单端自动切换，只做一致性校验提示。

### 2.6 智能优选原则（新增草案，对应 S8）

**结论：分层做，先保底回落，再质量选路，最后才是主动探测推荐；不做全参数暴力组合。**

| 参数类 | 能否单端自动试 | 说明 |
|--------|---------------|------|
| 全网一致 | 否 | 加密/压缩/网密两端不一致直接连不通，只能提示 |
| 单端可试 | 是 | scheme 顺序（tcp/udp/ws/wss/quic/wg）、打洞开关、KCP/QUIC 代理、`listener/mapped`、中继 vs 直连、MTU |
| 质量信号 | 观测 | RTT + `loss_rate`（已有）+ 新增 jitter；综合分 + 熔断 + hysteresis（沿用 §2.3） |

阶段含义：L1 保底降级（无感）、L2 同 peer 质量选路（无感）、L3 主动探测后「推荐 + 一键应用」（需用户确认）、L4 全自动切换（默认不做，需明确产品需求才立项）。

---

## 3. TODO 清单

### P0 — 连通保底（自定义端点）

- [x] **P0.1** 定义配置模型（已简化，无档位）：保底/中继 **URL 列表**（`[[peer]]` / `peer_urls` / `public_server_url`，scheme/host/port/path）；**不**新增第二套 URL 系统
  - 拨号策略恒全量：全部已配置 URL 维持连接 + 按需打洞；选路只看质量分
  - 硬约束保留：`disable_p2p` / `p2p_only` / 允许中转；`prefer_peer_relay` UI 隐藏、不抑制打洞，TOML 可选 OSPF 投影
  - `ConnectionPathTier` / `config/connection_path.rs` 已删除（未上线直接删；proto `reserved`）
- [x] **P0.2** 回落状态机（已简化，随档位一并删除）：`connectivity/fallback` 已删除（未上线直接删）
  - ManualConnector 恢复恒全量重拨（全部 connectors + 按需打洞）；`select_conn` 质量分 + hysteresis 承担选优与防抖
  - 状态面回退为 connector url + status（`Connector.fallback_*`、`current_fallback_index` / `fallback_reason` / tier 字段已删）
  - `p2p_policy_flags.lazy_p2p` 仅跟 `lazy_p2p` 标志，不与 `prefer_peer_relay` 耦合
  - scheme 矩阵：已由 P-AUTO.L1 / P1.3 落地（`default_protocol` CSV × 手动 URL 改写）
- [x] **P0.3** 文档与模板：同时提供「443 示例」与「自定义端口示例」；写明 **443 不一定可用**
  - Current：`tunnels-and-transport.md` / `peer-connections.md` 双端口示例 + 有序列表
  - 用户面：`README.md` / `README_CN.md`、Web `initial_nodes_help`、CLI `peers` 帮助
  - 模板：`script/install.sh`、magisk / android-jni / mini 示例注释
- [x] **P0.4** GUI / Web：中继地址为自由输入（URL），端口不默认锁死 443；占位符展示多种 scheme（无档位控件）
  - `Config.vue`：基础设置仅保留「允许作为中转节点」正向开关（`disable_relay_data` 取反）；`disable_p2p` 为硬约束保留在高级设置
  - `initial_nodes_help` / placeholder 含 443 与自定义端口示例；全部地址恒维持连接，按质量自动选路
- [ ] **P0.5** 验收：在 **非 443** 的 `tcp://` / `wss://host:自定义端口/path` 上稳定组网；禁 UDP 环境可用用户指定的 TCP/`wss` 保底
  - 清单：① `wss://host:8443/et` 组网；② 同场景换 `tcp://host:5000`；③ 禁 UDP 主机仅用 TCP/`wss` peer 列表

### P1 — 保活与弱网策略可调

- [x] **P1.1** Ping 失败关连接阈值可配置（`flags.ping_fail_close_count`，默认 5；`peer_conn_ping.rs` / `api_input` / TOML）；弱网可调高（如 8～12）
- [x] **P1.2** `ping_interval_max_sec` 可配置（默认 32；与 per-conn TCP 打洞 1s cap 取更严）；自适应下限仍为 1s 踢拍
- [x] **P1.3** 弱网/策略预设：**优先 scheme 列表**可自定义（例：`wss,tcp,quic,udp`），失败按序降级——**不写死「先 443」**（= P-AUTO.L1）
- [ ] **P1.4** 验证 #2632：TCP/非 UDP 打洞连接 1s ping 在目标环境不再因 idle 掉线（`cargo test` / 现网抽样）
- [ ] **P1.5** 可选：短时抖动宽限（如短暂丢包不立即拆 conn），避免比 OpenVPN 更「神经质」的闪断
- [x] **P1.6** 度量：在现有 RTT 窗口上增加 **jitter** 统计，并与已有 `loss_rate` 一并暴露到 `PeerConnStats` / 状态面（`WindowLatency::get_jitter_us`；proto `jitter_us=6`；Status 表展示）
- [x] **P1.7** `select_conn` 改为 **综合质量分**（延迟 + 丢包 + 抖动），含丢包熔断阈值与切换 hysteresis；默认丢包权重大于纯 RTT（`peers/conn/conn_select.rs`）
  - 默认：`w_lat=1` / `w_loss=4` / `w_jitter=1`；loss fuse 20%；switch margin 10% + abs 0.005 × 2 窗口
  - Flags 透参：`conn_select_w_*`（百分制权重）、`conn_select_loss_fuse_pct`、`conn_select_switch_*`（全 0 = 内置默认，否则按字面值，允许权重为 0）；TOML / NetworkConfig / `ConnSelectConfig::from_flags`
  - 非目标（P1.7 当时）：未改 OSPF 代价（后由 P2.3 落地）；未做 GUI 调权
- [x] **P1.8** 单元验收（代码级）：「低 RTT + 高丢包」选后者；hysteresis 单窗口不切（`conn_select` 单测）
  - 现网双路径抽样仍建议人工确认；不阻塞 Flags 透参合入
- [ ] **P1.8b**（可选）现网/仿真双路径验收清单

### P2 — 可观测、路由代价与带宽

标记说明：`[x]` 已完成；`[ ]` 未开始；`[~]` 部分完成（子项注明剩余工作与跟踪去处）。

- [x] **P2.1** 状态面明确：各 PeerConn、`default_conn`、质量分分项（rtt/loss/jitter）、是否仅冗余
  - `PeerConnInfo.quality_score` / `quality_fused`（与 `select_conn` 同源）；Status「质量分」列 + tooltip（★ default / · 热备 / `!` 熔断 / `+N` 热备数）
  - Connector 存活仍走既有 `ListConnector`（CLI）；Web Status 本项未嵌入 connector 列表
- [x] **P2.2** 文档/UI：多连接 ≠ 已聚合带宽（对齐 Current `peer-connections.md`）
  - **文档（2026-10-07）**：Current §2/§4「热备 ≠ 聚合」；[`multi-link-bonding.md`](./multi-link-bonding.md) Phase 1 文案与异质优先语义
  - **UI（2026-10-07）**：Status「质量分」列头 `path_quality_help`；Config `peer_link_bond_count_help` 明确默认 1 = 热备非叠带宽（只描述 Current）
- [x] **P2.3** 将丢包/抖动（综合分）纳入 OSPF / `latency_first` 边代价；带防震荡与可观测
  - 发布端 `direct_peer_info`：`quality_score`→`DirectConnectedPeerInfo.latency_ms`（`score*1000` + 熔断加成）；发布 hysteresis `min_delta=20`
  - Dijkstra / peer-center `RouteCostCalculator` 不变（仍读 `latency_ms`）；零 loss/jitter 时量级≈原 RTT ms
  - 可观测：Status `path_latency*` 在 LeastCost 下反映质量代价；PeerConn 质量分列（P2.1）
- [x] **P2.4** [`multi-link-bonding.md`](./multi-link-bonding.md) Phase 2a/2b（按流哈希；异质优先 / 同质补齐；默认 N=1）**已合入**；Phase 3（出口/`bind_device` 多样性）仍见该文
- [ ] **P2.5** 丢包场景下 KCP/QUIC proxy 的启用策略产品化（可配置，非隐性默认）

### P-UX — 配置面正确性与密度（对应 S7；纯前端，低风险）

- [x] **P-UX.1** 互斥/依赖规则落地：按 §2.5 表实现禁用/隐藏/warn；**老配置读入只提示、不强制改值**（已拍板）
  - 改动面：`easytier-web/frontend-lib/src/components/Config.vue`、`modules/configConflicts.ts`、`types/network.ts`、`locales/cn.yaml + en.yaml`（`xxx_conflict_help`）
  - 验收：`disable_p2p+p2p_only`、`disable_ipv6+ipv6_auto`、`no_tun+dev/mtu`、`disable_encryption+算法` 四组不再能“无提示”保存
- [x] **P-UX.2** 紧凑布局（桌面 4 列×2 行 / 断点 **760px** 回单列，已拍板）：`默认连接协议/加密算法/数据压缩/SO_MARK` 一行 4 列；`主机名/TUN名称/MTU/接收限速` 一行 4 列
  - 改动面：同上 `Config.vue`，复用 VPN Portal 双列 grid 模式；`Select` 与 `InputNumber` 对齐、`placeholder` 不截断需验证
  - 非目标：不改字段语义与后端
- [x] **P-UX.3** 帮助文案只描述 Current，不引用本 Roadmap 草案（遵守 `docs/README.md` 约定）
- [x] **P-UX.4** 开关控件与正向展示统一（2026-10-08）：5 处 `ToggleButton`（VPN Portal / 网络白名单 / 自定义路由 / socks5 / 共享 IPv6 子网）改 `ToggleSwitch`；负逻辑字段由 2 个扩到 **9 个**走 `inverted` 正向展示（`disable_p2p` / `disable_kcp_input` / `disable_quic_input` / `disable_tcp|udp|sym_hole_punching` / `disable_upnp` / `disable_ipv6` / `disable_encryption`），字段名与后端语义不变
  - 约定与踩坑见 [`../current/desktop-gui-and-config-server.md`](../current/desktop-gui-and-config-server.md) §4（反转后冲突文案要同步 `configConflicts.ts` 的 `*_help` key）

### P-AUTO — 智能探测与优选（新增草案，对应 S8；分 L1-L4）

- [x] **P-AUTO.L1** 保底降级（无感）：`default_protocol: string` 扩展为有序 scheme 优先列表（如 `wss,tcp,quic,udp`），失败按序尝试；禁 UDP 环境自动落到用户指定的 TCP/`wss`
  - 兼容：单值老配置视为长度 1 列表；与 P0.1 为叠加关系（URL 列表×scheme 矩阵，已拍板）
  - 落地：`config/protocol_preference.rs`；Direct 按列表排序（含 wg/faketcp）；Manual reconnect 仅在 tcp/udp/ws/wss/quic 间改写 scheme（保留源有效端口；wg/faketcp 不改写）；Config MultiSelect + CSV；`ListConnector.url`=配置、`active_url`=获胜拨号；remove 可按任一 URL 匹配；failover 有总超时预算
- [x] **P-AUTO.L2** 质量选路（无感）：即 P1.6-P1.8，不重复
- [ ] **P-AUTO.L3** 主动探测 + 推荐（一键应用，需确认）：后台按候选 scheme/代理组合建连试测，记录 RTT/loss/jitter 到 `peer_conn_history` + 状态面，给出“推荐配置” diff
  - 约束：限频限并发、默认关（已拍板）、需用户 opt-in；不试全网一致性参数；对称 NAT 生日攻击式探测默认禁
  - 验收：双路径“低 RTT 高丢包 vs 略高 RTT 低丢包”下推荐后者，并可一键应用
- [ ] **P-AUTO.L4** 全自动切换：默认不做；仅在 L3 有明确产品需求且误切率可接受时再立项

### P3 — 增强 / Backlog

- [ ] **P3.1** TLS 外观增强（证书策略、ALPN 等）——仍基于用户自定义端口与域名
- [ ] **P3.2** 深度指纹 / pluggable transport（不承诺对抗主动探测）
- [ ] **P3.3** 与出口导流（轨道 T）联动时的 underlay 排除：保底 URL 的 host:port 必须可加入排除列表（端口自定义）
- [ ] **P3.4** 业务感知权重（如实时流提高抖动权重）——仅在有明确产品需求时再做

---

## 4. 非目标

- 不重做 OpenVPN 或 IPsec/IKE 协议栈。
- 不把「必须像访问 443 网站」写成验收前提。
- 不做域前置（domain fronting）依赖。
- 首期不对标 VeloCloud DMPO / 企业多 WAN 硬件 SD-WAN。
- **P-UX 不改字段语义与后端；P-AUTO.L3/L4 不做全参数暴力组合，不自动改全网一致性参数。**

---

## 5. 验收对照（摘要）

| 场景 | 期望 |
|------|------|
| 用户配置 `wss://relay.example:8443/et` | 可走该 URL，不要求 443 |
| 用户配置 `tcp://10.0.0.2:5000` | 同上 |
| 环境阻断 443 但放行自定义端口 | 只要配置指向可达端口即可通 |
| 环境阻断 UDP | 可通过用户指定的 TCP/`wss`/QUIC 建连，不依赖打洞 |
| 多 URL 配置 | 全部维持连接，按质量分自动选择，无档位概念 |
| 多 PeerConn | 仍可热备切换；**不等于**聚合带宽；bonding 见专题文，启用后按流分摊且成员异质优先 |
| 低 RTT 高丢包 vs 略高 RTT 低丢包 | 默认选后者（综合分）；超过丢包阈值不得占 `default_conn` |
| 质量分接近、RTT 抖动 | 不因单次探测频繁切换（hysteresis） |
| 文案 / 模板 | 出现「自定义端口」说明；无「仅支持 443」表述 |
| P-UX 互斥四组 | 无提示保存被拦截或 warn；老配置读入不强制改值 |
| P-UX 紧凑布局 | 桌面端 8 字段占 2 行；窄屏回单列不断行截断 |
| P-AUTO.L1 | UDP 被阻断时自动落到列表中 TCP/`wss`，状态可查哪一条生效 |
| P-AUTO.L3 | 给出推荐 diff，一键应用后复测分数提升；默认关闭，需 opt-in |

---

## 6. 建议实施顺序（草案，待拍板）

```text
P-UX（纯前端，可并行先行）
P1.6 stats jitter/loss 上报 + P1.1/P1.2 透参
  -> P0.1 全量拨号 + 质量选路（无档位；P0.2 FallbackController 已删）
  -> P0.3/P0.4 文档与 UI
  -> P0.5 非 443 验收
并行：P1.4 #2632 验证
随后：P1.3 / P-AUTO.L1 scheme 优先列表（**已完成**）；P1.7-P1.8 / L2 质量分（**已完成**）
再后：P2 可观测 / OSPF / bonding / proxy（P2.1–P2.3 **已完成**）
最后：P-AUTO.L3 探测推荐；P-AUTO.L4 默认不启动；P0.5 现网验收
```

拍板前可与 [`discussion-proposal-2026-10.md`](./discussion-proposal-2026-10.md) 轨道 C（连通保底）对齐；**本文强调自定义端点后，轨道 C 叙事应从「wss/443 范式」改为「可配置保底 URL + 可选 443 推荐」**。

---

## 7. 勾选记录

| 项 | 状态 | 备注 |
|----|------|------|
| P0.* | P0.1–P0.4 **已完成（档位已简化删除）**；P0.5 验收未开始 | 全量拨号 + 质量选路 + 文档/模板 |
| P1.* | P1.1–P1.3/P1.6–P1.8（单元）**已完成**；P0.5/P1.8b 现网验收、P1.4–P1.5 未开始 | 质量分 Flags 透参已接；P1.3=L1 |
| P2.* | P2.1–P2.4 **已完成**；P2.5 未开始 | bonding Phase 2 见 [`multi-link-bonding.md`](./multi-link-bonding.md)；Phase 3 仍开放 |
| P3.* | Backlog | |
| P-UX.* | **已完成** | 老配置只提示；断点 760px；`configConflicts.ts` + Config 紧凑布局 |
| P-AUTO.* | L1/L2 **已完成**；L3/L4 未开始 | L3 需 opt-in；L4 默认不做 |

---

## 8. 安卓 VPN 生命周期（来自 [`../archive/android-vpn-connection-audit-2026-10-07.md`](../archive/android-vpn-connection-audit-2026-10-07.md)）

与本文 S1–S8（协议/选路）**正交**；审计 A1–A14 / A3 / A9 可落地项已修复。细节见 Archive 审计文；现场 playbook 见 [`../ops/android-startup-auto-stop.md`](../ops/android-startup-auto-stop.md)。

**勾选**

| 项 | 状态 |
|----|------|
| A3 Rust watchdog（孤儿停 VPN + tick） | **已完成（最小闭环）** — `easytier-gui/src-tauri/src/android_vpn_watchdog.rs` |
| A3 后台无 WebView 自动拉起 VPN | 未做（需新鲜 virtual IP；避免盲启动） |
| A9 网络切换 | **已完成** — Kotlin NetworkCallback + `setUnderlyingNetworks`；Rust 关 peer conn 重拨；启动假切换见 Ops `android-startup-auto-stop.md` §6（同 netId 去重 + 5s 宽限期） |

---

## 9. 待决问题（讨论用）

1. ~~§2.5 互斥 / 老配置~~：**已拍板** — 从严禁选；老配置读入**只提示、不强制改值**。
2. ~~紧凑布局 / 断点~~：**已拍板** — 桌面 4 列×2 行（8 字段占 2 行）；移动端断点 **760px** 回单列。
3. ~~URL×scheme~~：**已拍板** — 叠加（URL 列表×scheme 矩阵）。
4. ~~L3 默认~~：**已拍板** — L3 默认关。剩余：探测频率/并发上限定多少？对称 NAT 生日攻击类探测是否永远禁止自动触发（建议是）？
5. 质量分权重默认值谁来定？丢包熔断阈值默认多少？是否允许用户手动调权重？
6. 本文拍板后是否需要回写 [`discussion-proposal-2026-10.md`](./discussion-proposal-2026-10.md) 轨道 C 叙事（从 wss/443 改为可配置保底 URL）？

