# 连接稳定性优化：A / B / C 实施计划

## Status

- Status: **Roadmap**（A/B **代码已落地**；下一步仅 **C 现网验收**）
- 日期：2026-10-24
- 最近审阅：2026-10-24（A/B 合入工作区；本机 cargo 单测按用户指示跳过，B4 vitest 已绿）
- 背景：对照 Tailscale 稳定性差距分析后，决定**不做**客户端「对端连接历史」、**不做** DERP / 会话模型重写；先落地高杠杆的拆连雪崩防护与产品面引导，再做现网验收。
- 关联 Current：[`../current/tailscale-gap-and-conflict-audit.md`](../current/tailscale-gap-and-conflict-audit.md)、[`../current/p2p-direct-stability-audit.md`](../current/p2p-direct-stability-audit.md)、[`../current/peer-connections.md`](../current/peer-connections.md)、[`../current/packet-loss-investigation-2026-10.md`](../current/packet-loss-investigation-2026-10.md)
- 关联 Roadmap：[`connection-stability-todo.md`](./connection-stability-todo.md)（P1.5 / P-AUDIT.* / P0.5）
- 索引：[`../README.md`](../README.md)

---

## 1. 目标与非目标

### 1.1 目标

缩小 EasyTier 相对 Tailscale 在 **RDP/SSH 等长连接交互流量**上的体感差距，优先砍掉「短暂抖动 → 成簇拆连 → 重建冻结」。

| 档 | 名称 | 一句话 |
|----|------|--------|
| **A** | 拆连雪崩防护（P1.5） | 同 peer **仍有可用替代 conn 时不拆光**；至少保护「最后一条」 |
| **B** | 产品 / 配置引导 | 让用户在 GUI 能调保活参数，并看懂「交互场景」推荐（不静默改出厂默认） |
| **C** | 现网验收 | 用日志 + ping 证明 A/B 与既有 P-AUDIT 合入项真有效 |

### 1.2 非目标（本计划明确不做）

| 项 | 理由 |
|----|------|
| 客户端本地「对端连接历史」 | 控制台已有；本地采样+落库成本高、非刚需 |
| 厂商级 DERP / 会话永不拆 | 架构级，另立产品 |
| 发送环满改阻塞背压（P-AUDIT.6） | 死锁/活锁风险，审计已否决 |
| 出厂把 `peer_link_bond_count` 改为 1 | 删聚合能力；与「协议顺序第二档兜底」耦合（见 Current 帮助文案） |
| 把偶发 200–500ms 尖峰一律算作本轮必解 | 现网已证伪「ET 控制面 / bond 切路」为主因；C 只做观测与排除 |

---

## 2. 顺序与工作量

```
A（core，3–7 人日）→ B（前端/文案，1–3 人日，可与 A 尾声并行）→ C（现网，2–5 人日）
```

| 档 | 估时 | 风险 | 依赖 |
|----|------|------|------|
| A | 3–7 人日 | **中**：判据写错 → 拖死链路或仍雪崩 | 读懂 `peer_conn_ping` / `Peer` 关连路径 |
| B | 1–3 人日 | **低**：默认值勿静默改；帮助只写 Current | A 不阻塞 B 的「核实/补文案」部分 |
| C | 2–5 人日 | **低**（方法学已有） | A 合入后再做「雪崩」对比；其余可先跑基线 |

合计日历时间大约 **1–2 周**（含单测与一轮现网）。

---

## 3. 档 A — 拆连雪崩防护（P1.5）

### 3.1 问题（已核实）

- 代码：`easytier-core/src/peers/conn/peer_conn_ping.rs` 连续失败达 `ping_fail_close_count`（出厂 **10**）即拆该 `PeerConn`。
- 现网：同一 peer 的多条 conn 曾在约 **19s** 内被全部拆掉（`loss_rate` 递增）——缺「保留最后一条」保护。
- 设计轴（Current §9）：**存活性**判据是「还有没有别的可用路」，不是「有没有中继」。

出厂把阈值 5→10 **已降低**误拆频率，但**多 conn 雪崩**仍可能在窗口内拆光。

### 3.2 拍板语义（施工必须遵守）

1. **有替代路径时**：某条 conn 达到失败阈值 → 允许拆该条（换路），**不得**在短窗口内拆光同 peer 全部活 conn。
2. **无替代路径时**（该 peer 仅剩这一条活 conn）：
   - **默认**：**不因 ping 失败阈值拆掉最后一条**（避免硬中断 + 重建冻结）；继续保活探测，等恢复或上层另有策略。
   - **例外（可选后续）**：显式配置「允许拆最后一条」或独立超时（本档 **不做** 新 flag，除非联调证明必须）。
3. 「替代」定义（与选路质量轴对齐、避免把未验证打洞当活路）：
   - 同 `Peer` 上另有 `!is_closed()` 的 conn；
   - 且满足与 `is_unfused_verified` **同一谓词**（未验证打洞 **不算** 替代），避免「拆完只剩半吊子打洞」。
4. **不要**做成「无条件延长 `ping_fail_close_count`」——真死链路会拖更久；杠杆是 **条件拆**，不是全局更钝。

### 3.3 建议改动面

| 区域 | 文件（预期） | 做什么 |
|------|--------------|--------|
| 关连决策 | `peers/conn/peer_conn_ping.rs` 及调用关连处 | 达阈值时查询「peer 是否还有已验证未熔断的其它 conn」；若否 → skip close（打 debug/计数） |
| Peer 查询 | `peers/conn/peer.rs` / `peer_map.rs` | 提供只读查询：`has_alternative_live_conn(except: PeerConnId) -> bool`（命名以实现为准） |
| 度量 | metrics 或既有计数 | 建议：`peer_conn_close_skipped_last`（或等价）——宽限生效次数，便于 C 验收 |
| 单测 | `peers/conn/*` 测试 | ① 双 conn：一条失败可拆；② 单 conn：达阈值不拆；③ 唯一替代是 unverified hole-punch → 视为无替代、不拆（或按拍板：仍保护最后一条） |

> 实现时 **禁止** 在热路径 `clone` 大配置；沿用 `PeerContext` 快照惯例。

### 3.4 验收（代码级，合入前门禁）

- [ ] `cargo test -p easytier-core --lib` 相关单测全绿（**2026-10-24 本机跳过**；合入/CI 前仍须跑）
- [x] 单测覆盖：双路径可拆劣质 / 单路径不拆最后一条 / unverified 不算替代（`may_tear_down_requires_verified_unfused_alternative`）
- [x] 日志：触发宽限时有可 grep 的 debug（勿每包 warn）
- [x] Release note 草稿：见 §7「A release note」

### 3.6 Release note 草稿（发版说明用）

> **连接稳定性**：同 Peer 仅剩最后一条可用隧道时，保活失败**不再拆光**该路径（避免短暂抖动后整段冻结）；若仍有其它已验证、未熔断的替代隧道，仍可拆掉劣质连接。交互场景下可参考配置帮助中 UDP / bond / ping 阈值说明。

### 3.5 风险与回滚

| 风险 | 缓解 |
|------|------|
| 真死最后一条拖成僵尸 | C 阶段用「拔线」场景测；若不可接受再加「最后一条最大挂起秒数」flag（另开 PR） |
| 与 bond / fuse 交互 | 替代判定复用 `is_unfused_verified`，与 P-AUDIT.1 一致 |
| 回归「该拆不拆」导致 UI 长期绿但不通 | 最后一条仍应持续 ping；状态面已有 VIP 可达探测（正交）——C 用 ping 对齐 |

回滚：单 PR 可逆；保留 feature 门控 **非必须**（语义清晰则默认开）。

---

## 4. 档 B — 产品 / 配置引导

### 4.1 现状核实（2026-10-24）

下列项在代码里**已基本落地**，本档以「补齐缺口 + 引导」为主，避免重复劳动：

| 项 | 状态 | 锚点 |
|----|------|------|
| GUI：`ping_fail_close_count` / `ping_interval_max_sec` | **已有** | `Config.vue`；`network.ts` 默认模板 10/10；i18n help |
| 状态页 TCP-over-TCP ⚠ | **已有** | P-AUDIT.5 / `Status.vue` |
| `default_protocol` / `peer_link_bond_count` help | **已有**且已写清耦合 | `locales/cn.yaml` / `en.yaml` |

旧审计里的 **P-AUDIT.5b「GUI 无控件」** 视为 **过时**；B 验收时勾「已关闭 5b」即可。

### 4.2 本档要做的事

| ID | 项 | 说明 | 估时 |
|----|-----|------|------|
| **B1** | 关闭 / 勘误 P-AUDIT.5b | 在 `tailscale-gap-and-conflict-audit.md` 与 `connection-stability-todo.md` 将 5b 标为已落地；避免后续误开工 | 0.5 人日 |
| **B2** | 交互场景「推荐配置」文案（只描述 Current） | 在配置页相关 help 或短提示中明确三条推荐（**不改出厂默认**）：① UDP 确认可用时可将协议顺序收为 **`udp`**（避免 TCP-over-TCP）；② 对照实验可暂设聚合数 **1**（知悉会关掉多协议拨号兜底）；③ 弱网可调高拆连失败次数 | 0.5–1 人日 |
| **B3** | （可选）配置页「场景提示」静态块 | 非预设一键改值；仅提示 + 链到字段。若嫌吵可只做 B2 | 0.5–1 人日 |
| **B4** | 默认模板一致性抽检 | 新建网络：`ping_*=10`、`default_protocol` 与 core `gen_default_flags` 一致；`vitest` 覆盖关键断言 | 0.5 人日 |

### 4.3 明确不做（B）

- 出厂改 `default_protocol` 为仅 `udp`（禁 UDP 环境会直接挂）。
- 出厂改 `peer_link_bond_count=1`。
- 一键「应用交互预设」自动写库（易误伤托管配置；若要做升为单独产品需求）。

### 4.4 验收（B）

- [ ] 文档勘误：5b 已关闭
- [ ] 中/英 help 或场景提示含「交互 / RDP」指引，且**不引用本 Roadmap 名**
- [ ] `frontend-lib` 相关 vitest 通过
- [ ] 人工：配置页能看到 ping 两字段 + 协议顺序说明含 TCP-over-TCP 警告

---

## 5. 档 C — 现网验收

### 5.1 方法学硬约束

1. **不要只看状态页**（2s 轮询会漏秒级事件）；用 `ping` + ET 日志时间对齐。
2. ET 日志时间戳多为 **UTC**，对齐本地时注意时区。
3. 先排除环境：本机代理 TUN 抢 ET 外层、ET `proxy_cidrs` 反抢代理出口（见 `packet-loss-investigation-2026-10.md`）。

### 5.2 清单

#### C0 — 环境基线（每次现网前）

- [ ] `Find-NetRoute`（或等价）确认到 ET 公网节点出口 **不是** 代理 TUN
- [ ] 记录：`latency_first`、`default_protocol`、`peer_link_bond_count`、`ping_fail_close_count`
- [ ] 记录内核/GUI 版本与 commit

#### C1 — 既有合入项（A 之前也可跑）

| 项 | 做法 | 期望 |
|----|------|------|
| S1b NAT 保活 | 空闲 >30s 后 VIP ping | 不应再出现「映射老化式」长黑洞（改前 ~30s+） |
| P-AUDIT.7 | 短抖动丢 1–2 个保活 | 不应轻易拆连（阈值 10） |
| P-AUDIT.4 / DEF | 坏直连 + 有中继 | 业务应能改走中继而非死等坏直连 |
| P-AUDIT.5 | 全 tcp/quic 隧道 | 状态页协议列 ⚠ |
| P0.5 | 非 443 `tcp`/`wss` 保底 | 禁 UDP 主机仍可组网（见 `connection-stability-todo` P0.5） |

#### C2 — 档 A 专项（合入后必做）

- [ ] **雪崩对照**：弱网或故意丢包下，`Select-String 'closing the connection'`（或仓库实际文案）——同一 peer **数秒内拆光多条** 应显著减少或消失
- [ ] **最后一条**：仅单 conn 时，ping 失败达阈值 → 进程日志出现 skip/宽限计数；**不应**立即进入完整重建窗口（除非后续加了「最后一条超时」）
- [ ] **双 conn**：人为弄坏一条 → 允许拆坏条，另一条继续承载 RDP/ping
- [ ] 指标：`peer_conn_close_skipped_last`（若已加）在单路径抖动时上升

#### C3 — 档 B 专项

- [ ] 按帮助将协议改为仅 `udp`（UDP 可用环境）→ RDP 无 ⚠、体感不劣于改前
- [ ] 聚合数改 1 时，UI/help 提示「多协议拨号被截断」行为符合 Current

#### C4 — 明确不纳入本轮通过标准

- 偶发 200–500ms 尖峰且 ET 日志空白 → 记入 `packet-loss-investigation`，**不阻断** A/B 合入

### 5.3 C 产出物

- [ ] 在本文件 §7 勾选表更新日期与简短结论（通过 / 有条件通过 / 打回）
- [ ] 若失败：附日志片段 + 配置快照；开新 issue/续写 Current，不直接改 A 语义拍板

---

## 6. 与总清单的映射

| 本计划 | `connection-stability-todo.md` / Current |
|--------|------------------------------------------|
| A | **P1.5**（存活性轴；「无替代才保护」） |
| B1 | **P-AUDIT.5b** → 标完成 |
| B2–B4 | 产品帮助 / 模板一致性（非新 P 编号） |
| C1 | P0.5、P-AUDIT.1/4/5/7、S1b、DEF 现网验收 |
| C2 | P1.5 现网验收 |

---

## 7. 勾选记录

| 项 | 状态 | 日期 / 备注 |
|----|------|-------------|
| A 设计拍板（本文 §3.2） | **已拍板** | 2026-10-24 |
| A 代码 + 单测 | **代码已合入工作区** | 2026-10-24：闸门 + `build_ping_fail_close_gate`；单测已写；**本机 cargo 跳过**，CI/出包前补跑 |
| A release note | **草稿见 §3.6** | 短暂抖动不拆光唯一 PeerConn；有已验证替代仍可拆劣质 conn |
| B1 文档勘误 5b | **已完成** | 2026-10-24 |
| B2/B3 交互引导文案 | **B2 已完成**（B3 场景块跳过） | 2026-10-24：`default_protocol` / `peer_link_bond_count` / `ping_fail_close_count` 中英文 help |
| B4 模板 vitest | **已通过** | 2026-10-24：`default-config-parity` 6/6 绿（含 ping 默认 10/10） |
| C0 环境基线 | 未开始 | |
| C1 既有项现网 | 未开始 | |
| C2 A 专项现网 | 未开始 | |
| C3 B 专项 | 未开始 | |

---

## 8. 施工顺序建议（给执行者）

1. 读 Current `tailscale-gap-and-conflict-audit.md` §5.1 / §9 与本文 §3.2。  
2. 实现 A + 单测；本地 `cargo test -p easytier-core --lib`。  
3. 并行或随后做 B1–B4。  
4. 出包后跑 C0→C1→C2→C3；结果写回 §7。  
5. **通过后**再考虑「最后一条最大挂起秒数」等增强——不在本计划默认范围。
