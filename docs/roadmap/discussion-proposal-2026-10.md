# 路线整合讨论方案（2026-10）

## Status

- Status: **Roadmap / Discussion**（供拍板，**不是**已 Accepted 的实施计划）
- 日期：2026-10-03
- 最近审阅：2026-10-03
- 索引：[`../README.md`](../README.md)
- **市场对比（Tailscale / ZeroTier / 蒲公英 / 华为 / VeloCloud）**：[`market-comparison-2026-10.md`](./market-comparison-2026-10.md)
- 本文把已有 Current / Roadmap 收成一份「讨论稿」：问题、候选顺序、默认建议、待决问题。  
- **通过讨论并写入各专题 Status（Accepted / 暂缓）之前，不得按本文排期写代码。**

---

## 0. 市场对比后的推荐立场（2026-10-03）

完整矩阵与论据见 [`market-comparison-2026-10.md`](./market-comparison-2026-10.md)。讨论时建议先确认下列修订，再选 α/β/γ。

| 主题 | 对标 | 建议 |
|------|------|------|
| 出口语义 | Tailscale exit node | **坚持 W1**：客户端启用出口 = 装/卸默认路由；子网 LPM 优先 |
| 域名代理 | Tailscale App Connector | **坚持 W2**（接在 W1 后）；权威 DNS + 路由成对下发 |
| P2P 被限 | Tailscale DERP / 蒲公英转发分层 | **W0/W3a 定性为「中继保底」**，不是「隐身」；深度 TLS 指纹默认 Backlog |
| 多链路带宽 | ZeroTier Multipath（flow hash） | **W3b 可选**；默认 N=1；**不对标** VeloCloud DMPO / 华为站间多 WAN |
| 产品边界 | — | 近中期做 **软件 Mesh**，不承诺完整企业 SD-WAN 清单 |

**修订后的默认节奏**：仍推荐 **方案 α**，但把轨道 C 的叙事从「伪装专项」改为「连通保底（类 DERP）→ 产品预设 →（可选）指纹」。

---

## 1. 用户侧问题（讨论起点）

| # | 用户感知 | 根因（文档结论） | 专题文档 |
|---|----------|------------------|----------|
| P1 | 配了出口节点，本机却像没走 VPN | ~~`exit_nodes` 只做 L3 选路~~ → **已落地**：可解析 exit 时本机装 `/0`（D+） | [Current](../current/traffic-steering.md) / [vNext Phase B/C](./traffic-steering-vNext.md) |
| P2 | 路由加不上 / 删不掉 | L2 `apply_route_changes` 失败仍记账 | 同上 |
| P3 | 想用域名做子网代理，IP 对不上 | 无域名权威；B 侧 DNS 可能与 A 不一致 | [domain-proxy](./domain-proxy.md) |
| P4 | 单条连接被运营商限速，多连接也不快 | 默认 `bond_count=1` 仍单路径；`bond_count>1` 已按流分摊（Phase 2a/2b） | [multi-link-bonding](./multi-link-bonding.md) / [Current](../current/peer-connections.md) |
| P5 | P2P / 打洞易被限 | 缺「直连失败必有可用中继」的产品档；伪装/指纹为次要 | [traffic-camouflage](./traffic-camouflage.md) / [Current](../current/tunnels-and-transport.md) / [市场对比](./market-comparison-2026-10.md) |
| P6 | 控制台继续演进、只升 web | 节点协议短期冻结 | [web-evolution](./web-evolution.md) |
| P7 | 安装升级统一走 GitHub | 方案已有，增强项未做 | [github-release-install](./github-release-install.md) |

P1–P5 偏 **core 数据面 / 连接**；P6–P7 偏 **交付与控制台**，可并行。

---

## 2. 能力轨道（已有草案，不重复设计）

```text
轨道 T（导流）     出口默认路由 + 路由可靠 → 域名静态 → 域名动态
轨道 C（连通保底） 中继+wss/443 范式 → 全量拨号+质量选路（原定的直连/偏好中继/仅中继三档已删除，见 connection-stability-todo §2.2） →（可选）TLS 指纹
轨道 B（带宽）     可观测 → 按流哈希多隧道（ZeroTier 向）→ 加权/熔断
轨道 W（Web）      控制台 UX / 发布体验（不改 core）
轨道 D（交付）     Release/GHCR 安装升级打磨
```

专题细节以各 Roadmap 文为准；本文只讨论 **做不做、先做谁、默认答案是否采纳**。

---

## 3. 建议讨论的默认排序（提案，可改）

### 3.1 原则

1. **先结果可见、再增强**：能上网 / 能连上 / 能删路由，优先于「更快」和「更像网站」。  
2. **少改核心的先落地**：伪装 Phase A（文档+模板）几乎不改协议，可与 T 并行。  
3. **域名依赖导流模型**：域名代理接在导流 Phase A 之后（与 vNext 一致）。  
4. **Bonding 与伪装解耦**：一个解决限速，一个解决识别；勿捆成一个大需求。  
5. **Web / 交付不挡 core**：P6/P7 用独立节奏，不占用导流第一刀人力时可并行。

### 3.2 推荐时间盒（讨论用，非承诺）

| 波次 | 内容 | 主要改动面 | 依赖 |
|------|------|------------|------|
| **W0（立刻可做）** | 伪装 Phase A：中继+`wss:443` 部署清单；连接/隧道 Current 文案进 UI 帮助（不夸大） | docs / 少量 UI 文案 | 无 |
| **W1（core 第一刀）** | 导流 Phase A：出口默认路由所有权 + L2 失败重试 + 冲突提示 + 帮助文案对齐 Current→新语义 | core + host 路由 + i18n | 接受 vNext §11 |
| **W2** | 域名静态 MVP（domain-proxy Phase 1 / vNext Phase B） | core + MagicDNS + Web 配置项 | W1；挂网络实例 |
| **W3a / W3b（可并行）** | **a** 伪装产品预设 Phase B；**b** bonding MVP（按流哈希） | a: 配置/UI/策略；b: Peer 发送路径 | 各专题开放问题拍板 |
| **W4+** | 域名动态；伪装 TLS 指纹；bonding 加权；导流可观测 | 按需 | W2/W3 |

```text
W0 ──┬──► W1 ──► W2 ──► W4（域名动态等）
     │         ╲
     │          └──► W3a（伪装预设）∥ W3b（bonding）
     └──（全程）W / D 轨道并行
```

### 3.3 明确建议「本季度不启动」的项（除非讨论推翻）

- 域名动态解析、Fake-IP / 强 DNS 劫持  
- REALITY / 域前置 / 完整第三方代理兼容  
- bonding 单流拆包加速  
- 导流里「出口改成全网通告 `0.0.0.0/0`」  
- web-evolution 阶段 D（收窄 core 依赖）

---

## 4. 三套备选节奏（供二选一/三选一）

讨论时先选一条「主节奏」，再填开放问题。

### 方案 α — 「出口可用」优先（本文默认推荐）

- 顺序：W0 → W1 → W2 →（W3a ∥ W3b）  
- 适合：投诉集中在「出口没生效 / 路由脏」。  
- 代价：限速与抗识别改善靠后。

### 方案 β — 「连得上」优先

- 顺序：W0 加重（甚至提前做伪装 Phase B 预设）→ W1 → W3b → W2  
- 适合：主战场是 P2P 被限、必须先经 `wss` 中继。  
- 代价：出口语义债继续存在一段时间。

### 方案 γ — 「带宽」优先

- 顺序：W0 → bonding 可观测 + MVP → W1 → …  
- 适合：已能稳定直连，痛点是单连接限速。  
- 代价：连不上时 bonding 无意义；且动 Peer 发送路径有加密/乱序风险，宜在导流第一刀稳定后或并行专人做。

**提案默认：α**；若现场反馈「根本连不稳」，改为 **β**，W1 仍保留为紧随其后的 core 必做项。

---

## 5. 默认技术答案（沿用各文，供一次确认）

| 主题 | 默认答案 | 来源 |
|------|----------|------|
| 出口谁装默认路由 | 配置了 `exit_nodes` 的本机装/卸 | vNext |
| L3 优先级 | VIP → 代理 LPM → exit_nodes | Current / vNext |
| 对端宣告 `0.0.0.0/0` | LPM 优先；与本机 exit 并存时 UI 警告 | vNext |
| 域名权威 | 仅代理节点；CIDR + 答案成对下发 | domain-proxy |
| `proxy_domains` 挂载 | 网络实例级 | vNext 建议 |
| bonding MVP | 按流哈希，禁止同流跨 conn | multi-link-bonding |
| 伪装 MVP | 中继 + `wss` + 443 + 合法证书；不承诺隐身 | traffic-camouflage |
| 短期 web | 只改 web，冻结节点可见协议 | web-evolution |

若讨论否定某一行，应回写对应专题文档，而不是只改本文。

---

## 6. 待决问题清单（建议会上逐项勾选）

### 6.1 范围与节奏

- [ ] 主节奏选 α / β / γ？  
- [ ] W1（出口+路由可靠）是否定为 **下一 core 功能**？  
- [ ] W0（伪装部署文档）是否本周合并进 `ops/` 并链到 README？  
- [ ] bonding 与伪装 Phase B：并行还是串行？谁先？

### 6.2 导流 / 域名

- [ ] IPv6 `::/0`：W1 是否明确只做 v4？  
- [ ] `proxy_domains` 配置挂网络实例 — 确认或改节点全局？  
- [ ] Web 托管是否在 W2 同步出配置 UI，还是先 CLI/TOML？

### 6.3 连接

- [ ] bonding 默认 `N`（建议默认 1，高级选项 2–4）？是否接受「Tailscale 未做、我们做成差异化」？  
- [x] 中继档是否做成「直连优先 | 偏好中继 | 仅中继」？→ 已决议：**不做**（2026-10-06）。用户要的是两设备间低延迟/低抖动/低丢包，不分直连中转；拨号恒全量、选路看质量分。`ConnectionPathTier` 已删除，见 `connection-stability-todo.md` §2.2。  
- [ ] Phase C TLS 指纹：默认 Backlog，还是有强制合规场景必须立项？  
- [ ] 对外是否明确「不做完整 SD-WAN（华为/VeloCloud/蒲公英硬件清单）」？

### 6.4 文案与发布

- [ ] W1 合并前是否强制改 `exit_nodes_help` 等 i18n（避免继续夸大）？  
- [ ] 上述能力是否需要单独 changelog / 兼容说明（行为变更：装默认路由）？

---

## 7. 成功标准（讨论用切片）

| 波次 | 「做完」的最低标准 |
|------|-------------------|
| W0 | 新人按一篇 Ops 能搭 `wss:443` 中继组网；UI/文档不声称已网站伪装 |
| W1 | 只配 exit 即出现可删除的本机默认路由；路由失败会重试；帮助文案与行为一致 |
| W2 | 静态域名在覆盖 DNS 下命中代理；故意 DNS 不一致仍命中 |
| W3a | 一键/预设走 web_wss，无需手写冷门端口 |
| W3b | `N>1` 时多流总吞吐可超过单连接人为限速；单 TCP 流不因乱序明显变差 |

---

## 8. 风险与依赖（汇总）

| 风险 | 影响波次 | 缓解 |
|------|----------|------|
| 默认路由权限 / 各 OS metric | W1 | 平台表驱动；文档写清需管理员 |
| 路由 owner 与 `manual_routes` 交互 | W1 | 专题验收场景 5–6 |
| MagicDNS 覆盖未开启 | W2 | UI 提示；降级仅 CIDR |
| PeerSession / 乱序 | W3b | 按流哈希；对照 secure mode 文档 |
| 夸大抗审查 | W0–W3a | Current 措辞；Roadmap 不进帮助 |
| 人力并行过多 | 全部 | 选定 α/β/γ 后最多两条 core 热线 |

---

## 9. 建议的会议产出

会议结束时至少留下：

1. **主节奏**：α / β / γ（或改写版一句话）。  
2. **下一实现项**：唯一的 W1（或明确改为伪装/bonding）。  
3. **暂缓列表**：勾选 §3.3 / §6.3。  
4. **回写**：把结论写进各专题 `Status`（Accepted / Deferred）并改本文 Status 为 `Superseded by …` 或附「决议附录」。

---

## 10. 决议附录

| 日期 | 决议 | 备注 |
|------|------|------|
| 2026-10 | **W1 / Phase A 已落地**：出口可解析时本机装默认路由（D+）；现状见 Current `traffic-steering.md` | P1 关闭 |
| 2026-10 | **Bonding Phase 2a/2b 已合入**；默认 `bond_count=1` | P4 部分关闭；Phase 3 仍开放 |
| 2026-10 | **配置页/运行页主体已落地**（GUI + Web `mode`） | 见 [`config-vs-run-pages.md`](./config-vs-run-pages.md) |
| — | W2 域名代理、W0/W3a 伪装、W3b Phase 3 仍待拍板/实现 | 见各专题 Roadmap |

---

## 11. 结论（讨论稿默认立场）

1. **对标 Tailscale 导流语义**：先 W1（出口=默认路由）再 W2（App Connector 式域名）。  
2. **对标 DERP/蒲公英分层回落**：W0/W3a 做中继保底产品化；抗 DPI 指纹后置。  
3. **对标 ZeroTier、不对标 VeloCloud DMPO**：bonding 按流哈希、默认关，与导流解耦。  
4. **Web / GitHub 交付**继续并行，不塞进 W1。  
5. 详细对照表见 [`market-comparison-2026-10.md`](./market-comparison-2026-10.md)；拍板后回写各专题 Status。
