# 市场产品对比与方案启示（2026-10）

## Status

- Status: **Roadmap / Discussion**（对比分析材料，**不是**实施规格）
- 日期：2026-10-03
- 最近审阅：2026-10-09（矩阵「今天」列对齐 Current：出口 D+、bonding Phase 2）
- 索引：[`../README.md`](../README.md)
- 配套讨论稿：[`discussion-proposal-2026-10.md`](./discussion-proposal-2026-10.md)
- 用途：后续拍板时对照「成熟产品怎么做」与「EasyTier 该学什么 / 不该硬碰什么」

说明：下列能力描述来自各产品**公开文档 / 发行说明 / Issue**，随版本变化；对比侧重**产品语义与架构选型**，非性能测试报告。  
文中 **VELO** 按业界常指的 **VMware SD-WAN by VeloCloud（VeloCloud）** 理解；若指其它同名产品，决议时需更正命名。

---

## 1. 对比对象与定位

| 产品 | 定位（简） | 与 EasyTier 关系 |
|------|------------|------------------|
| **Tailscale** | 软件 Mesh VPN；控制面协调 + WireGuard；DERP/Peer Relay 保底 | 最接近「纯软件组网」对标 |
| **ZeroTier** | 软件虚拟以太网；可选 Multipath 多链路 | 多连接聚合的直接参照 |
| **蒲公英（贝锐）** | 软硬一体 SD-WAN / 异地组网；云端智能选路 + 转发 | 国内中小企业「能连、好用」对标 |
| **华为 SD-WAN / 乾坤** | 企业级 SD-WAN；应用识别 + 多链路智能选路 | 企业站间多上行天花板 |
| **VeloCloud（VELO）** | 企业 SD-WAN；DMPO 动态多路径优化、带宽聚合 | 多 WAN 聚合与业务策略天花板 |

EasyTier 当前更接近 **Tailscale / ZeroTier 一类软件 Mesh**，而非华为/VeloCloud/蒲公英硬件边缘的完整 SD-WAN。对比时避免用「全功能 SD-WAN」当短期验收标准。

---

## 2. 能力矩阵（相对 EasyTier 现状）

图例：● 成熟/主推　◐ 部分有或需高级版　○ 弱/无/社区诉求　△ EasyTier 现状　★ EasyTier Roadmap 已立项

| 能力维度 | Tailscale | ZeroTier | 蒲公英 | 华为 SD-WAN | VeloCloud | EasyTier 今天 | EasyTier 路线 |
|----------|-----------|----------|--------|-------------|-----------|---------------|---------------|
| 虚拟网互联 / Mesh | ● | ● | ● | ● | ● | △ | — |
| **出口节点 = 装默认路由** | ●（exit node） | ◐（需自行路由） | ●（上网/出口类能力，产品化） | ●（站点出口策略） | ● | △（可解析 exit 时本机装 `/0`；见 Current） | ★ 域名 Phase B/C |
| **子网/CIDR 代理** | ●（subnet router） | ● | ● | ● | ● | △（proxy_cidrs） | 保持 |
| **域名驱动导流** | ●（App Connector：域名→解析→通告路由 + split DNS） | ○ | ◐（偏应用/加速与策略，非开源同构） | ●（应用识别选路） | ●（应用策略） | ○ | ★ domain-proxy |
| **P2P 失败必有中继** | ●（Peer Relay → DERP/HTTPS） | ●（根服务器/叶） | ●（P2P→转发→强制转发） | ●（Hub/Gateway） | ●（VCG） | ◐（有中继/共享节点，缺「HTTPS 保底」产品档） | ★ camouflage W0/B |
| **传输伪装 / 抗识别** | ◐（DERP 走 HTTPS，主目标是连通非隐身） | ○ | ◐（自研协议+节点，非公开伪装栈） | ○ | ○ | ○（有 wss/faketcp，无产品档） | ★ 伪装；深度指纹 Backlog |
| **多链路加带宽** | ○（Issue 诉求；现单路径） | ●（Multipath：flow hash / stripe 等） | ●（多 WAN 负载/选路，偏站点） | ●（多链路负载均衡） | ●（DMPO 带宽聚合） | ◐（出厂默认 `bond_count=2` 按流分摊） | ★ Phase 3 出口多样 |
| 链路质量选路（时延/丢包） | ◐（选 exit/路径偏好） | ◐（multipath quality） | ●（智能选路卖点） | ● | ●（DMPO） | ◐（latency_first 等） | 增强期 |
| 云端零配置 / 硬件 | ◐（SaaS 控制面） | ◐ | ●（软硬+云） | ● | ● | ◐（easytier-web） | web-evolution |
| 开源 / 自托管控制面 | ●（Headscale 生态） | ◐ | ○ | ○ | ○ | ● | 保持差异化 |

---

## 3. 分主题：成熟做法 vs 启示

### 3.1 出口与子网（导流）— 首要对齐 Tailscale

**成熟做法**

- Tailscale 把能力拆成用户能懂的三块：  
  - **Exit node**：客户端启用后走 `0.0.0.0/0` / `::/0`（真·接管上网）  
  - **Subnet router**：通告 CIDR，其它节点装对应路由  
  - 二者叠加时：**更具体的子网/App 路由优先于 exit**  
- 路由要进系统表才算「生效」；控制面批准 + 客户端 accept-routes（Linux）等流程产品化。

**启示（建议采纳）**

1. EasyTier 导流 Phase A（本机因 `exit_nodes` 装/卸默认路由）与 Tailscale **语义一致**，应作为 **core 第一优先级**，不宜再摇摆成「仅 Peer 选路」。  
2. UI/帮助应改用「出口节点 / 子网代理」两套话术，避免「配了出口=已接管系统上网」的夸大（与 Current 缺口一致）。  
3. 与对端通告 `0.0.0.0/0` 并存时的 LPM 优先 + 警告，与 Tailscale「subnet/app 优先于 exit」同构，可保留。

**不建议**

- 首期不做华为/VeloCloud 级「按应用 DPI 选出口」；成本高，且偏离 Mesh 主线。

---

### 3.2 域名导流 — 对齐 Tailscale App Connector

**成熟做法**

- Tailscale **App Connector**：配置域名 → 连接器侧解析（DoH）→ **通告发现的 IP 路由** → 客户端对该域名走 **split DNS**，保证「解析到的 IP」与「路由通告」同源。  
- 可与 exit 并存；App 路由优先。

**启示（建议采纳）**

1. 现有 [`domain-proxy.md`](./domain-proxy.md)（权威在代理节点、CIDR+答案成对下发）与 App Connector **同构**，方向正确。  
2. 实现顺序仍应在 **出口/路由可靠（W1）之后**（与 Tailscale 先有 subnet/exit、再强化 connector 的产品成熟路径一致）。  
3. 产品叙述可借用「应用连接器 / 域名代理」心智，降低教育成本；配置挂 **网络实例** 便于 Web 托管（讨论稿已建议）。

**差异注意**

- Tailscale 控制面强、路由需批准；EasyTier 更偏实例配置下发——审批流首期可不做，但 **DNS 覆盖前提** 必须在 UI 写清（与草案一致）。

---

### 3.3 连通保底（P2P 被限）— 学 Tailscale/蒲公英的「分层回落」，伪装降级为增强

**成熟做法**

- Tailscale：直连优先 → **Peer Relay** → **DERP（HTTPS）**；卖点是「只要能出 HTTPS 就能通」，不是「完全不可识别」。  
- 蒲公英：P2P → 自动转发 → 强制转发；再叠加云节点智能选路。  
- 华为/VeloCloud：站点经 Gateway/Hub，不依赖终端互打洞。

**启示（建议调整优先级表述）**

1. **W0/伪装轨道的第一价值应写成「可靠中继（类 DERP）」**：公网中继 + `wss`/`443` + 合法证书 + 清晰回落策略。  
2. 「像正常上网 / 抗 DPI」是 **加分项（Phase C+）**，不宜当 MVP 验收；市场主力产品也未把「隐身」当 Mesh 主 KPI。  
3. 产品档建议：`直连优先 | 偏好中继 | 仅中继`（名称待定），对齐蒲公英三种传输模式心智。  
4. 深度 TLS 指纹 / REALITY 保持 Backlog，避免与开源 Mesh 主线抢工期。

这与原 [`traffic-camouflage.md`](./traffic-camouflage.md) Phase A→B→C 兼容，但 **讨论时应把 Phase A 定性为「连通保底」而非「混淆专项」**。

---

### 3.4 多链路带宽 — 学 ZeroTier Multipath；勿首期对标 VeloCloud DMPO

**成熟做法**

- **ZeroTier Multipath**：多物理/多路径 bonding；推荐 **balance-xor（按流哈希）** 以保护 TCP 有序；另有 active-backup、stripe 等。  
- **Tailscale**：公开 Issue 仍诉求多路径加吞吐，**现状单路径**——说明软件 Mesh 里 bonding 是差异化机会，但也可证明「没有也能成主流产品」。  
- **华为 / VeloCloud / 蒲公英**：多在 **站点多 WAN** 上做负载均衡、质量切换、甚至包级优化（DMPO）；假设边缘设备 + 多上行，与「两端软件进程多开几条隧道」不是同一层问题。

**启示（建议采纳）**

1. EasyTier bonding MVP **直接对齐 ZeroTier：按流哈希 + 维持 N 条隧道**；默认 `N=1`。  
2. 文档明确两层能力，避免用户预期错位：  
   - **Peer 多隧道聚合**（软件 Mesh，Roadmap bonding）  
   - **站点多 WAN SD-WAN**（硬件/边缘场景，**非** EasyTier 近期目标）  
3. 不要把 VeloCloud 级 DMPO/FEC/业务识别写入近季度承诺。

---

### 3.5 控制面与交付 — 学「云管 + 本地可跑」双轨

| 启示 | 来源 | EasyTier 落点 |
|------|------|----------------|
| 控制面协调路由/出口批准 | Tailscale | web-managed-config 已有基础；审批流可后置 |
| 软硬一体零开局 | 蒲公英 / 华为 / VeloCloud | 非软件主线；保持旁路/客户端 |
| 自托管 + 发布物清晰 | Tailscale / 开源生态 | github-release-install + web 独立升级 |

---

## 4. 综合推荐（供讨论稿采纳）

### 4.1 战略定位一句话

> EasyTier 近中期对标 **Tailscale 语义清晰度 + ZeroTier 可选多路径 + 蒲公英/Tailscale 式中继保底**；  
> **不对标** 华为/VeloCloud 完整 SD-WAN（应用 DPI、DMPO、多 WAN 硬件边缘）。

### 4.2 相对原讨论稿 α 的修正建议

| 项 | 原 α | 市场对比后建议 |
|----|------|----------------|
| W1 出口+路由可靠 | core 第一刀 | **保持**；与 Tailscale exit 对齐，证据最强 |
| W0 伪装文档 | 立刻可做 | **保持并改名表述**：强调「HTTPS/WSS 中继保底（类 DERP）」 |
| W2 域名 | W1 后 | **保持**；实现时显式对标 App Connector 语义 |
| W3a 伪装产品预设 | 与 bonding 并行 | **略升权**：做成「直连/偏好中继/仅中继」比 TLS 指纹更重要 |
| W3b bonding | 并行 | **保持可选并行**；MVP=ZeroTier flow-hash；不承诺 SD-WAN 聚合 |
| 深度混淆 Phase C | 可选 | **默认 Backlog**，除非合规场景强需求 |

### 4.3 更合适的「默认方案」摘要（修订版）

1. **导流**：学 Tailscale — exit 必须装默认路由；子网 LPM 优先；文案诚实。  
2. **域名**：学 App Connector — 代理侧权威 DNS + 路由通告成对；接在 W1 后。  
3. **连通**：学 Tailscale DERP / 蒲公英转发分层 — 先产品化中继保底，伪装指纹后置。  
4. **带宽**：学 ZeroTier Multipath — 按流哈希；出厂默认 `bond_count=2`；不对标 VeloCloud DMPO。  
5. **不做**：企业 SD-WAN 应用级智能选路全家桶（华为/VeloCloud 主场）。

---

## 5. 讨论时可用的对照题

1. 我们是否接受「对用户承诺的出口 = Tailscale exit 语义」？若否，差异写进 Current 的哪一句？  
2. 中继保底是否升为与 W1 同级的产品必达（即便少改核心、先 Ops/预设）？  
3. bonding 是「差异化卖点」还是「有限精力下的 P2」？（Tailscale 至今未做）  
4. 域名代理 UI 是否直接使用「应用连接器」类命名以降低教育成本？  
5. 是否明确对外宣称「不做完整 SD-WAN」，以免渠道按蒲公英/华为清单验收？

---

## 6. 参考链接（公开材料）

- Tailscale Exit nodes：https://tailscale.com/docs/features/exit-nodes  
- Tailscale App connectors：https://tailscale.com/docs/features/app-connectors  
- Tailscale DERP / connection types：https://tailscale.com/docs/reference/derp-servers  
- Tailscale multipath 诉求：https://github.com/tailscale/tailscale/issues/2256  
- ZeroTier Multipath：https://docs.zerotier.com/multipath/  
- 蒲公英 SD-WAN / 智能选路（贝锐官网与公开方案文）  
- 华为智能选路 / SD-WAN（support.huawei.com / 华为云乾坤文档）  
- VMware SD-WAN by VeloCloud DMPO（VMware 公开博客与文档）

---

## 7. 修订记录

| 日期 | 说明 |
|------|------|
| 2026-10-03 | 初版：纳入讨论材料，并回写 discussion-proposal 推荐修正 |
