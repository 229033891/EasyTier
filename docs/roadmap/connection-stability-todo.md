# 连接稳定性优化 TODO

## Status

- Status: **Roadmap**（Checklist；尚未按本文改代码）
- 日期：2026-10-06
- 最近审阅：2026-10-06（可行性复核：选路只看 RTT、`loss_rate` 已算未参选、`jitter` 零命中、保活/阈值全硬编码、`prefer_peer_relay` 仅 OSPF 投影；结论总体可行，实施顺序微调见 §6）
- 背景：对照 OpenVPN / IPsec 的「固定隧道 + 强保活」模型，梳理 EasyTier Mesh（多 PeerConn + 打洞 + 中继）的稳定性差距与可落地项
- 相关 Current：[`../current/peer-connections.md`](../current/peer-connections.md)、[`../current/tunnels-and-transport.md`](../current/tunnels-and-transport.md)
- 相关 Roadmap：[`traffic-camouflage.md`](./traffic-camouflage.md)、[`multi-link-bonding.md`](./multi-link-bonding.md)、[`market-comparison-2026-10.md`](./market-comparison-2026-10.md)、[`upstream-port-todo.md`](./upstream-port-todo.md)（#2632 TCP 打洞 1s ping）
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
| S4 | TCP 打洞 idle 被中间设备掐断 | 上游 #2632 已移植 1s ping cap；待验证 |
| S5 | 「学 OpenVPN 上 443」在部分环境无效 | **443 本身也可能不可用** → 必须自定义端口与协议 |
| S6 | 低延迟但高丢包/高抖动的路径仍被选中 | `select_conn` / `latency_first` **几乎只看 RTT**；`loss_rate` 已统计却未参与选路；**抖动未单独度量** |

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

### 2.2 产品档语义（草案）

名称待定；行为必须与端口无关：

| 档位 | 行为 |
|------|------|
| **直连优先** | 打洞/直连优先；失败后回落到用户配置的中继/备选 URL 列表 |
| **偏好中继** | 优先使用用户配置的中继 URL；直连为辅或延迟尝试 |
| **仅中继** | 不打洞 / 不直连（或严格禁用），只走配置的中继列表 |

「中继」在文案中指 **用户（或网络管理员）提供的可达端点**，不隐含公网 443。

### 2.3 链路质量：延迟 + 丢包 + 抖动（必须一起考虑）

**结论：要。** 只按延迟选路会在弱网/Wi‑Fi/跨运营商场景选到「很快但不稳」的路径（TCP 重传、卡顿、语音视频花屏），体感往往差于略高延迟但低丢包、低抖动的路径。

| 指标 | 今日 | 目标 |
|------|------|------|
| **延迟（RTT）** | `select_conn` / OSPF `path_latency` 主依据 | 保留，作为综合分的一项 |
| **丢包率** | Ping 已算 `loss_rate`，写入 `PeerConnStats`，**不参与选路** | 进入 PeerConn 选择与（可选）路由代价 |
| **抖动（Jitter）** | **未单独统计** | 由 RTT 滑动窗口算标准差或连续 RTT 差的均值；进入综合分 |

推荐综合分（草案，权重可配；数值仅示意）：

```text
score = w_lat * norm(rtt)
      + w_loss * norm(loss_rate)      // 丢包权重要高于「略差一点的 RTT」
      + w_jitter * norm(jitter)
```

硬规则建议：

1. **丢包熔断阈值**：`loss_rate` 超过可配上限时，该 PeerConn **禁止成为 `default_conn`**（有替代路径时），避免「低延迟高丢包」饿死业务。
2. **防抖切换（hysteresis）**：新路径综合分需优于当前路径超过阈值（或连续 N 个探测窗口更优）才切换，避免抖动本身导致路径来回切。
3. **分层落地**：先改 **同 peer 多 PeerConn 的 `select_conn`**（数据已在本地）；再评估是否把丢包/抖动编进 OSPF 代价（影响面更大，易震荡）。
4. **不对标** 企业 SD-WAN 包级 DMPO；首期只做 **选路/选 conn 的质量分**，不做按包喷洒。

与 [`market-comparison-2026-10.md`](./market-comparison-2026-10.md)「链路质量选路（时延/丢包）」增强期、以及 bonding 文「按链路质量加权」一致；**稳定性 TODO 将其从「可选增强」升为明确交付项**。

### 2.4 与 `traffic-camouflage.md` 的关系

- 该文的「`wss` + 443 + 合法证书」视为 **推荐范式之一**。
- 本文将其升格为：**同一套回落机制，端点完全自定义**；443 不可用时换端口/换 scheme 仍须达标。
- 深度 TLS 指纹 / REALITY 仍属增强（Backlog），不阻塞本 TODO 的 P0。

---

## 3. TODO 清单

### P0 — 连通保底（自定义端点）

- [ ] **P0.1** 定义配置模型：保底/中继 **URL 列表**（有序）+ 档位（直连优先 / 偏好中继 / 仅中继）
  - 字段须能表达 scheme/host/port/path；禁止仅 `use_https_443: bool` 一类开关
  - 与现有 manual peer / shared node / foreign network 配置对齐或明确映射（避免两套互相打架）
  - 与现有散装开关明确映射：`disable_p2p` / `p2p_only` / `disable_relay_data` / `prefer_peer_relay`（后者今日仅 OSPF 投影，见 `peer_ospf_route.rs:2494-2518`）不得与新档位语义冲突；老开关保留为兼容层，新档位为唯一真相源
- [ ] **P0.2** 实现回落状态机：当前路径不可用时按列表与档位切换；切换可观测（日志 + 状态面）
  - 新建 `FallbackController`（健康探测 + 按序降级 + hysteresis + 状态面 `current_fallback_index/reason`）；现状 `ManualConnector` 为 `DashSet` 全量轮询、`RelayPeerMap` 声明简化，均无统一编排
  - 健康信号依赖 P1.6（jitter/loss 上报）与 P1.1/P1.2（阈值可配），建议先行落地再做本项（见 §6）
- [ ] **P0.3** 文档与模板：同时提供「443 示例」与「自定义端口示例」；写明 **443 不一定可用**
- [ ] **P0.4** GUI / Web（若改配置面）：中继地址为自由输入（URL），端口不默认锁死 443；占位符展示多种 scheme
- [ ] **P0.5** 验收：在 **非 443** 的 `tcp://` / `wss://host:自定义端口/path` 上，仅中继档可稳定组网；禁 UDP 环境可用用户指定的 TCP/`wss` 保底

### P1 — 保活与弱网策略可调

- [ ] **P1.1** Ping 失败关连接阈值可配置（今日 `peer_conn_ping.rs:322-331` 硬编码连续 5 次；`three_node.rs:1651-1654` 有集成预期，改动须同步更新）；文档给出弱网建议值
- [ ] **P1.2** `ping_max_interval` / 自适应上下限可配置（默认保持 1s～32s；`peer_conn.rs:299,402,457-474` 今日 `pub(crate)` 硬编码，TCP 打洞等场景可单独 cap）
- [ ] **P1.3** 弱网/策略预设：**优先 scheme 列表**可自定义（例：`wss,tcp,quic,udp`），失败按序降级——**不写死「先 443」**
- [ ] **P1.4** 验证 #2632：TCP/非 UDP 打洞连接 1s ping 在目标环境不再因 idle 掉线（`cargo test` / 现网抽样）
- [ ] **P1.5** 可选：短时抖动宽限（如短暂丢包不立即拆 conn），避免比 OpenVPN 更「神经质」的闪断
- [ ] **P1.6** 度量：在现有 RTT 窗口上增加 **jitter** 统计，并与已有 `loss_rate` 一并暴露到 `PeerConnStats` / 状态面（今日 `WindowLatency` 仅均值、`jitter` 全仓零命中；`loss_rate` 在 `PeerConnInfo` 已上报但 `select_conn` 未读）
- [ ] **P1.7** `select_conn` 改为 **综合质量分**（延迟 + 丢包 + 抖动），含丢包熔断阈值与切换 hysteresis；权重可配，默认丢包权重大于纯 RTT（先改同 peer 多 PeerConn 的 `select_conn`，`peer.rs:213-237`；OSPF 代价放 P2.3 评估，避免震荡）
- [ ] **P1.8** 验收：构造「低 RTT + 高丢包」vs「略高 RTT + 低丢包」双路径时，默认选后者；路径抖动时不频繁来回切

### P2 — 可观测、路由代价与带宽

- [ ] **P2.1** 状态面明确：各 PeerConn、`default_conn`、质量分分项（rtt/loss/jitter）、是否仅冗余、当前是否走保底列表中的哪一条
- [ ] **P2.2** 文档/UI：多连接 ≠ 已聚合带宽（对齐 Current `peer-connections.md`）
- [ ] **P2.3** 评估将丢包/抖动（或综合分）纳入 OSPF / `latency_first` 代价；若做，必须带防震荡与可观测
- [ ] **P2.4** 按需推进 [`multi-link-bonding.md`](./multi-link-bonding.md)（按流哈希；坏链路按质量熔断）；默认 N=1
- [ ] **P2.5** 丢包场景下 KCP/QUIC proxy 的启用策略产品化（可配置，非隐性默认）

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

---

## 5. 验收对照（摘要）

| 场景 | 期望 |
|------|------|
| 用户配置 `wss://relay.example:8443/et` | 保底/仅中继档可走该 URL，不要求 443 |
| 用户配置 `tcp://10.0.0.2:5000` | 同上 |
| 环境阻断 443 但放行自定义端口 | 只要配置指向可达端口即可通 |
| 环境阻断 UDP | 可通过用户指定的 TCP/`wss`/QUIC 保底，不依赖打洞 |
| 直连优先 + 列表中继 | 打洞失败后落到列表中下一项，状态可查 |
| 多 PeerConn | 仍可热备切换；bonding 仅在 P2 启用后加带宽 |
| 低 RTT 高丢包 vs 略高 RTT 低丢包 | 默认选后者（综合分）；超过丢包阈值不得占 `default_conn` |
| 质量分接近、RTT 抖动 | 不因单次探测频繁切换（hysteresis） |
| 文案 / 模板 | 出现「自定义端口」说明；无「仅支持 443」表述 |

---

## 6. 建议实施顺序

```text
P1.6 stats+jitter/loss上报+P1.1/P1.2透参  <- 先行，给P0.2提供健康信号
  -> P0.1 配置模型+老开关映射
  -> P0.2 回落状态机FallbackController
  -> P0.3/P0.4 文档与UI
  -> P0.5 非443验收
并行P1.4#2632验证
随后P1.3 scheme优先列表
      P1.7-P1.8 质量分选路  <- 依赖P1.6
再后P2 可观测/OSPF代价评估/bonding/proxy策略
```

拍板前可与 [`discussion-proposal-2026-10.md`](./discussion-proposal-2026-10.md) 轨道 C（连通保底）对齐；**本文强调自定义端点后，轨道 C 叙事应从「wss/443 范式」改为「可配置保底 URL + 可选 443 推荐」**。

---

## 7. 勾选记录

| 项 | 状态 | 备注 |
|----|------|------|
| P0.* | 未开始 | |
| P1.* | 未开始 | P1.4 依赖 #2632；P1.7-P1.8 为质量选路 |
| P2.* | 未开始 | OSPF 多指标 / bonding 细节见专题文 |
| P3.* | Backlog | |
