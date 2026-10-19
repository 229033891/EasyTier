# GUI：已连接 vs 路由可达

## Status

- Status: **代码已落地，待现网验收**（一期 GUI/`frontend-lib` 已完成；验收项见 §5，未全绿前不写入 `docs/current/`）
- 最近审阅：2026-10-18（企业级可行性复审 + 实现后缺陷复审）
- 索引：[`../README.md`](../README.md)
- 优先级：**桌面 / 安卓 GUI（共用 `frontend-lib`）一期**；**LuCI 二期**
- 关联：
  - [`packet-loss-investigation-2026-10.md`](../current/packet-loss-investigation-2026-10.md)（§10：GUI「直连 / 丢包 0%」≠ 业务 Ping 通）
  - [`live-next-hop.md`](./live-next-hop.md)（§5.6 Phase F：控制面快速活性探测；**不是** VIP 数据面可达）
  - [`tray-run-state.md`](./tray-run-state.md)（托盘颜色语义；**本期不改托盘**）
  - [`poll-to-push-roi.md`](./poll-to-push-roi.md)（状态页 `collect_network_info` 成本；探测结果随快照捎带，不另开高频 RPC）
- 实现位置：[`vip_probe.rs`](../../easytier-core/src/instance/vip_probe.rs)（探测器）、`instance_info.rs`（快照填充）、`statusDisplay.ts` + `Status.vue`（展示）

---

## 0. 复审结论（2026-10-18）

| 维度 | 结论 |
|------|------|
| 问题是否真实 | **成立**。与丢包调查 §10、Phase F 注释一致：保活 / fast_probe 改的是 **PeerConn 边权与 loss 展示**，仍不是用户 ICMP/VIP 可达性。 |
| 产品是否该做 | **该做**。企业运维需要「会话在线」与「业务可达」分栏；单一「已连接」会误导排障。 |
| 一期范围是否可交付 | **可交付**，但须遵守下文契约（三态、默认开、禁 ping 语义、成本上限、与 Phase F 边界）。 |
| 是否已够「企业级规格」 | **原稿偏薄**。已在本文补齐：精确命名、状态机、探测契约、成本/安全、兼容、验收与明确不做。 |

---

## 1. 问题

现状 UI 把多层状态揉成「已连接」：

| 实际层次 | 现有信号 | 用户误读 |
|----------|----------|----------|
| 进程 / 实例在跑 | `network_running`、托盘绿 | 「组网通了」 |
| Peer 控制面保活 | `peer_route_pairs` 延迟 / 丢包、`peer_count`（cn：**已连接**） | 「对端通了就能访问」 |
| 数据面 VIP / 业务路径 | **几乎无独立展示** | 列表在线但 ping / 业务不通时以为 UI 骗人 |

根因：PeerConn keepalive / Phase F `fast_probe` 走**控制面**（已有 conn 上 ping）；业务流量走 **TUN + 路由下一跳**。两者可分叉（见丢包调查 §10：本机直连保活绿，包却在中继下一跳黑洞）。

OpenWrt LuCI 同样只展示进程 / CLI 表，本期**不改 LuCI**，二期复用同一探测字段或 CLI。

---

## 2. 命名与目标语义（一期做死）

### 2.1 对外文案（避免「路由可达」歧义）

工程与 UI **首选**：「**VIP 可达**」/ `vip_reachable`（en: **VIP reachable**）。

- 「路由可达」易被理解成「路由表有条目」或「OSPF 有下一跳」——**不够**；有路由项仍可能黑洞。
- 一期定义：**本节点经 EasyTier 数据面路径，对对端虚拟 IPv4 完成一次成功的 Echo 往返**（见 §3 探针类型）。
- 摘要仍可保留口语「可达」，tooltip / 帮助写清 = VIP 数据面探测，≠ 控制面延迟/丢包，≠ 代理网段。
- **不经过本机 TUN / OS 路由表**：探针直接进 mesh 数据面（见 §2.5）。所以它证明的是「**组网内**到该 VIP 的转发路径」，**不是**「本机操作系统此刻能 ping 通」。主机侧 TUN 路由故障不在本特性观测范围内。

### 2.2 状态表

| UI 文案 | 含义 | 数据源 |
|---------|------|--------|
| 已连接 | Peer 在 `peer_route_pairs` 且有控制面路径 | **现有**（不变） |
| 可达 | 最近一次 VIP 数据面探测 **成功** | **新增** |
| 不可达 | Peer 已连接，最近一次探测 **明确失败/超时**（含对端禁 ping） | **新增** |
| 探测中 | 已调度、尚无终态 | **新增** |
| 未知 | 探测关闭、无 VIP、`no_tun`、或尚无调度 | **新增** |

摘要与标题分数：**按链路统计**。`M` = 探测成功的**链路**数，`N` = 参与探测的**链路**总数。  
与每行 VIP Tag **同一单位**：主路坏、备路通时显示 `[1/2]`，不会被凑成节点级的「1 个可达」。

链路的口径（与 §2.5 一致）：

- 一条 **live PeerConn** 计 1 条；
- 中继 / 无直连 conn 的节点，后端发一条 `conn_id` 为空的**节点级条目**，**也计 1 条**——否则全中继网络会显示 `[0/0]`（等价「无数据」），而实际上每个中继都通。

**禁止**：探测失败时把「已连接」改成断开。  
**禁止**：用控制面 `loss_rate` / `lat_ms` / Phase F `fast_probe_degraded` 填充「可达」列。

```text
PeerConn keepalive / fast_probe ──► UI「已连接」+ 延迟/丢包（控制面）
VIP dataplane probe             ──► UI「可达 / 不可达 / 探测中 / 未知」
可同时成立：已连接=true 且 可达=false
```

### 2.3 UI 分层（已拍板）

**分数一律简洁：`[M/N]`**（方括号可选；文案**不**再写「已连接」「VIP 可达」前缀）。  
约定：`M` = VIP 探测成功的**链路**数，`N` = **探测器实际探测过的链路数**（`peer_vip_reachability` 记录数，含中继节点级条目）。

> **为什么 `N` 用记录数而不是表格行数**：`M` 只可能来自探测记录。若把"行数"当分母，一行若没有探测记录，它就进了 `N` 却永远无法贡献 `M`，徽章在完全健康的网络上也永远到不了 `[N/N]`。
>
> 二者本可不等：`collect_links` 枚举的是**路由表**，某 peer 的路由下没有可枚举的 PeerConn（中继 / 无直连）会坍缩成 **1 条节点级记录**，而表格仍按 conn 逐行显示。
>
> 为消除"徽章 18 / 表格 19"的观感矛盾，这类行**行内标为「未探测」**（区别于「未知」），Tooltip 说明"未被纳入本轮探测范围，不计入 N"。**表格自己标出哪一行不在 N 内**，两个数就不再冲突。

| 位置 | 展示 | 说明 |
|------|------|------|
| **最上方**（实例态旁） | 运行态旁一枚 Tag：`[M/N]` | 颜色：`M===N∧N>0` 绿；**有链路仍未给出结论**（`probing` / `unknown`，如探测关、`no_tun`、无 VIP）→ 灰；其余（含 M=0）橙；`N===0` 隐藏。**无 `[?/N]` 形态**：`hasData=false` 只可能来自旧内核（字段缺失，此时 N 也未知），直接隐藏。Tooltip：`VIP 可达链路 M / 链路总数 N`（及与控制面延迟/丢包的区别） |
| **节点信息**面板标题 | 与最上方**同形** Badge：`[M/N]` | 同一套计数与颜色；Tooltip 同上。**不再并排显示行数徽章**——两个无标签数字并排只会让人问「为什么不一样」 |
| **每一行** | 独立「VIP」Tag：**可达时只显示 rtt**（`7ms`）/ 不可达 / 探测中 / 未知 / **未探测** | 与延迟、丢包、代价列分开；不合并成单一健康度灯。**可达不带「可达」二字**：数字本身就是数据面已应答的证据，结论交给 Tag 颜色与「VIP」列头；仅当后端报可达却缺 `rtt_ms` 时才回退显示「可达」。**未知必须带原因**（`未知 · 探测已关闭` / `未知 · 本机尚未分配虚拟 IP` / `未知 · no_tun 模式不支持探测`），否则整屏「未知」无法排障 |

排障读法：

```text
最上方：  运行中  [5/18]       （探测器实际探了 18 条，其中 5 条通）
节点信息：节点信息 [5/18]；表内 19 行，18 行有 VIP Tag，1 行标「未探测」
表内延迟/丢包：控制面（与 VIP 列独立）
```

两处 `[M/N]` 数字必须一致（同一快照）。最上方负责一眼扫过；节点信息标题重复同一分数，方便收起顶栏后仍可见。
### 2.4 与 Phase F 的边界（必须写进实现评审）

| | Phase F（已有） | 本特性（拟做） |
|--|----------------|----------------|
| 目的 | 加快死边发现 → 改 OSPF 代价 | 告诉用户「VIP 业务路径是否通」 |
| 载体 | PeerConn 控制 ping | 数据面 VIP Echo（经 `send_probe_msg_on_conn` / packet plane） |
| 失败动作 | 抬边权 / 不拆连 | **仅展示**；不改选路、不拆连 |
| GUI 列 | 已有延迟/丢包 | **新列**，不可合并 |

#### 2.4.1 定位锁定：VIP 探测永远只观测，不承担保活

**已决策（2026-10-18），不再复议。** 曾有提议把探测频率提到「能保活链路」的量级（1–5s、全链路覆盖），以此改善 NAT 映射存活与切换速度。**否决**，理由如下：

| 论据 | 事实 |
|---|---|
| NAT 保活已在别处完成 | `DEFAULT_PING_INTERVAL_MAX_SEC = 10`，源码注释写明这是 **NAT-safe** 值，专门为避免「直接连接空闲时、用户流量在保活间隙被黑洞」而下调（原 32s 会超过约 30s 的运营商 NAT UDP 寿命） |
| 探测本来就更慢 | 现有 20s 周期**慢于**控制面 10s ping，加密探测对 NAT 映射存活无净增益，属重复劳动 |
| **切换根本不读这个信号** | `pick_default_conn` 的输入是控制面 pong 得到的 `ConnMetrics`（`latency_us`/`loss_rate`/`jitter_us`）。VIP 探测结果不参与任何切路判据，**加密探测对切换时机零影响** |
| 加密探测有副作用 | 探测包与用户数据共用同一条 conn 与发送环：加剧拥塞；探测丢包可能计入对端统计而**加速**误判；空闲时制造「假流量」干扰 lazy-p2p |

**因此**：

- `send_probe_msg_on_conn` / `send_probe_msg_by_ip` **必须**继续跳过 `mark_recent_traffic`（P2P / lazy-p2p 控制面信号）、用户流量统计与不可路由缓冲——这不是疏忽，是定位；
- 任何「让 VIP 探测影响选路 / 拆连 / 流量统计」的改动都属于**改变特性定位**，必须另立文档并重新评估，不得以调参形式夹带；
- 若要改善**切换速度**，正确的改动面是 `pick_default_conn` 的 `switch_windows` / `switch_margin`、故障判定阈值，或改用 **bond 多路径并行**；这些都与本特性无关，见 §2.4.2。

#### 2.4.2 切换时延的正确归属（供排障对照，不是本特性职责）

用户看到「已连接却卡顿」时，容易怀疑是探测太慢。实际时间线由控制面决定：

| 阶段 | 默认值 | 位置 |
|---|---|---|
| 控制 ping 间隔上限 | 10s（打洞 conn 更密，TCP 1s） | `DEFAULT_PING_INTERVAL_MAX_SEC` |
| 判定路径死亡（触发拆连） | `ping_fail_close_count = 10` × ~3s ≈ **30s** | `DEFAULT_PING_FAIL_CLOSE_COUNT` |
| 重建连接成本 | 约 30s（建连 5s + 打洞 2–6s + 握手 5s） | 同上注释 |
| **切换到备路** | fast probe 连丢 3 次（`FAST_PROBE_MISSES_TO_DEGRADE`，333ms 间隔）≈ **2s** → 发布 100% loss → loss fuse 命中 → 缓存立即失效 | `cached_path_unusable` |
| 主动换到更优路径 | 优势需 `switch_windows = 3` 个窗口**连续**保持，且超过 `max(incumbent×0.10, 0.005)`；窗口 = `default_conn` 缓存清空周期 **5s**，故约 **15s** | `pick_default_conn` + `peer.rs` 清空任务 |

> 结论：**断链换路约 2s**（用户最能感知的场景，已足够）；「主路活着但变差」才需要 15s，且下限受 5s 硬编码周期限制，仅调 `switch_windows = 1` 最快只能到 5s。

一句话：**「发现坏路快」靠 loss fuse，「主动换好路」才看 `switch_windows`；VIP 探测两边都不参与。**

### 2.5 探测粒度：按链路，不按节点

**这是本特性最容易被实现走偏的一点，写死在此。**

| | 做法 | 为什么 |
|---|---|---|
| **探测单位** | **每条 PeerConn 各发一个 probe**（`send_probe_msg_on_conn`，强制指定 `conn_id`） | 若交给 OSPF / `select_conn` 选路，主路已死时会被备路静默接管 → 「主路坏」永远看不出来 |
| **行级 Tag** | 按该行的 `conn_id` 取结果 | 一行 = 一条链路，Tag 就该回答「这条链路通不通」 |
| **`[M/N]` 汇总** | **同样按链路**：M = 通了的链路数，N = **探测器实际探测的链路数** | 汇总与行级 Tag 必须是同一单位，否则「每行都红、汇总却绿」会让人怀疑面板在对数（见下） |
| **中继 / 无直连 conn 的节点** | 退化为**按节点**探测一次（`conn_id` 为空），走路由路径；**该条目计 1 条链路** | 这类节点没有可命名的链路；不计入会让全中继网络显示 `[0/0]`（假象） |

推论（必须理解，否则会误判展示）：

- **主路坏、备路通 → `[1/2]`**：汇总如实反映「一半链路可用」，而不是把节点算成「可用」把坏链路藏掉；
- **行级与汇总方向一致**：行级红绿分布可以直接与 `[M/N]` 对上，二者不会互相打架；
- **`[M/N]` 的 N ≥ 节点数**：多隧道节点的 N 按链路数计，这是有意的；
- **N 与「表格行数」可能不等**（本例 18 vs 19）：差额行在表内标为「**未探测**」，因此不构成矛盾——看清哪个数是什么单位即可；
- 控制面延迟/丢包列仍各按自己的 conn 显示，与 VIP 列互不覆盖。

---

## 3. 探测契约（企业级必填）

### 3.1 探针类型（一期定稿）

1. **首选**：经核心数据面注入的 **ICMP Echo Request/Reply**（源=本机 VIP，目的=对端 VIP），走 mesh 转发路径（见 §2.5 粒度说明）。  
2. **若平台/feature 无法发 ICMP**：允许等价 **UDP/自定义 echo**（须走同一条数据面注入路径），但 UI tooltip 必须写「非 ICMP」；**不得**退回控制面 Ping。  
3. **超时**：单次 **3s**；**1 次超时即不可达**，恢复需 **1 次成功**（简单、可测）。

### 3.2 节奏与成本

| 项 | 一期默认 | 理由 |
|----|----------|------|
| 间隔 | **20s** | 弱于状态页 5s 轮询；避免 N 链路打满 |
| 并发 | 每实例 **最多同时 3** 个探测，**按链路轮转** | 上限只管并发、不管覆盖：链路数 > 3 时按游标轮询，`ceil(L/3)` 轮内覆盖全部 |
| 捎带 | 结果写入 `CollectNetworkInfo` 快照 | **不新增**高频 GUI RPC（对齐 poll-to-push） |
| 可关 | `flags.disable_vip_reachability_probe`（默认开）；关则全员「未知」+ `disabled` | 企业可禁探测流量；弱机/合规环境可关 |
| `no_tun` | 标「未知」+ 原因 `no_tun` | 无 TUN 时 ICMP 语义不同，避免假阳性 |

成本粗算：L 条链路、20s 间隔、≈46B echo ⇒ 量级 **十~百 B/s**，远低于业务。

**metrics（已落地）**：`vip_reachability_probes_sent` / `_reached` / `_timed_out` / `_send_failed` 四个计数器，便于现网观察探测成功率与超时量。

### 3.3 安全与误报

- 对端 **禁 ICMP** → 显示「不可达」或文案「探测失败（可能禁 ping）」；**不得**推断「隧道断开」。  
- ACL / 防火墙丢 echo → 同上。  
- 探测包不得触发拆连、不得写入 `ping_fail_close_count`。  
- 探测走**专用发送路径**（`send_probe_msg_on_conn` / `send_probe_msg_by_ip`）：不写 `recent_traffic`（P2P/lazy-p2p 控制面信号）、不计入用户流量统计、不进入不可路由缓冲。  
- **回包归属校验**：`ident` → `(目标 peer, seq, 发送时刻)`；回包必须来自**被探测的那个 peer** 且 `seq` 一致才认。仅凭魔数即可冒充的路径已被堵死（单测 `reply_is_claimed_only_from_the_probed_peer`）。  
- Proto 字段向后兼容：旧 GUI 忽略新字段；新 GUI 遇旧 core → 全「未知」。

### 3.4 字段（Proto）

挂在 `NetworkInstanceRunningInfo` 的 **`repeated PeerVipReachability peer_vip_reachability = 17`**（独立 repeated，避免撑爆 `PeerRoutePair` 语义）：

| 字段 | 说明 |
|------|------|
| `peer_id` | 节点 |
| `conn_id` | **该结果所属的 PeerConn**（对齐 `PeerConnInfo.conn_id`）；空 = 节点级条目（中继/无直连链路） |
| `state` | enum：`UNKNOWN` / `PROBING` / `REACHABLE` / `UNREACHABLE` |
| `rtt_ms` | 成功时可选 |
| `checked_at_unix_ms` | |
| `error` | 短机器可读：`timeout` / `send_failed` / `no_vip` / `disabled` / `no_tun` |

**每链路一条**。前端先按 `peer_id` 归组，再用 `conn_id` 取行级结果，`conn_id` 为空时作为节点级兜底。

---

## 4. 实现（已落地）

### 4.1 Core + Proto

- `easytier-proto`：§3.4 字段 + `FlagsInConfig.disable_vip_reachability_probe`。  
- `easytier-core`：探测器 [`vip_probe.rs`](../../easytier-core/src/instance/vip_probe.rs)；回包过滤挂在 peer packet pipeline；结果经 [`instance_info.rs`](../../easytier-core/src/management/full/instance_info.rs) 送入快照。  
- 任务生命周期：随实例启动拉起，`stop_components` 中先 `stop_vip_probe()`（abort + await），**不会跨重启泄漏**。
- 配置：`disable_vip_reachability_probe`（默认 `false`）。

### 4.2 共用前端（GUI 一期）

- [`Status.vue`](../../easytier-web/frontend-lib/src/components/Status.vue)、[`statusDisplay.ts`](../../easytier-web/frontend-lib/src/modules/statusDisplay.ts)、[`RemoteManagement.vue`](../../easytier-web/frontend-lib/src/components/RemoteManagement.vue)（实例态旁 `[M/N]`）  
- [`cn.yaml`](../../easytier-web/frontend-lib/src/locales/cn.yaml) / [`en.yaml`](../../easytier-web/frontend-lib/src/locales/en.yaml)  
- 行内 Tag：绿可达（带 rtt）/ **橙**不可达 / 灰未知·探测中；摘要 `[M/N]`；tooltip 边界说明。  
- Windows GUI / Android / 嵌入 Status 的控制台共用，不各写一套。

### 4.3 LuCI（二期）

- 读同一字段或 `easytier-cli` 扩展；**一期不改** `luci-app-easytier`。

---

## 5. 验收清单（实现后勾选；未勾前不进 Current）

功能：

- [ ] Peer 保活正常、**人为阻断某条链路的 VIP 转发** → GUI：该行 **已连接 + 不可达**，同 peer 其他链路仍「可达」，`[M/N]` **按链路少 1**（如 `[2/3]`）
- [ ] 所有链路都被阻断 → 该节点所有行「不可达」，`[M/N]` 中对应链路数全部计入不可达
- [ ] 恢复后 ≤2 个探测周期内 → **可达**，RTT 有值
- [ ] 关闭探测（`disable_vip_reachability_probe`）→ 全「未知」+ 原因 `disabled`（快照仍逐 peer 返回条目，不是空数组），已连接列不变
- [ ] 旧 core + 新 GUI → 「未知」，不崩
- [ ] Phase F 已把控制面 loss 打满时，若 VIP echo 仍通 → **可达**（证明两列独立）
- [ ] 链路数 > 3 时，连续两轮后所有链路都被探测过（验证轮转覆盖）
- [ ] 中继节点（无直连 conn）→ 出一行「可达/不可达」，且 `[M/N]` 的 N **包含**该节点级条目

> **口径说明**：验收针对 **mesh 转发路径**，**不**包含「本机 OS TUN 路由被破坏」场景——探针不经过 TUN（§2.1 / §2.5），主机侧路由故障由其他信号覆盖。

工程：

- [x] core：ICMP 组包、回包归属/防冒充、超时结算、按链路快照、gc、disabled 语义、**发送头标志（`not_send_to_tun`/`no_proxy` 仅用于自投递）** 单测（`vip_probe` 9 条 + `peer_manager` 3 条）
- [x] `frontend-lib`：类型检查 + VIP 纯函数单测（徽章映射 / 行级 join / **汇总按链路** / 中继节点级条目）
- [ ] 桌面 GUI + 安卓各至少一次真机矩阵
- [ ] （二期）LuCI 同语义

未达「可依赖」前：只更新本 Roadmap（勾选 +「代码已落地，待现网」），**禁止**写入 `docs/current/`。

---

## 6. 非目标（一期）

- 代理网段 / 任意 TCP·UDP 端口探测（企业若要「业务探测」另立 SLO 文档）
- 控制面 `loss_rate` / `lat_ms` / Phase F 冒充 VIP 可达
- 托盘随「无可达 peer」变色（另立项，避免与 [`tray-run-state.md`](./tray-run-state.md) 缠在一起）
- OpenWrt LuCI 一期改版
- 探测驱动自动改路 / 拆连（本特性只观测）

---

## 7. 风险与开放项（实现前可关死）

| 风险 | 处置（一期默认） |
|------|------------------|
| 对端禁 ping → 假「不可达」 | 文案标明探测失败；高级设置可关探测 |
| 中继黑洞（§10）本机 VIP echo 也可能失败 | **正确**：应显示不可达；与用户 ping 一致 |
| Android / `no_tun` | 未知 + 原因；不强行 ICMP |
| 状态页 5s 轮询变重 | 结果已在快照内；探测器独立 20s，不跟 5s 齐射 |
| 与「路由表有条目」混淆 | UI 禁用「路由可达」作主标题，用「VIP 可达」 |

---

## 8. Changelog

| 日期 | 说明 |
|------|------|
| 2026-10-18 | 立项：GUI 优先区分已连接与 VIP 数据面可达；LuCI 二期 |
| 2026-10-18 | 企业级复审：补命名、与 Phase F 边界、探测契约、成本/安全、兼容与验收；主文案改为「VIP 可达」 |
| 2026-10-18 | UI：最上方与节点信息标题统一简洁 `[M/N]`（不分写「已连接」）；分母=节点；含义仅 Tooltip |
| 2026-10-18 | UI 复议：曾拟顶栏 `已连接 N · VIP M/N`、节点信息标题 `VIP M/N`；**已撤销**，最终定稿为统一 `[M/N]`（见 §2.3），行内仍保留 VIP Tag |
| 2026-10-18 | **一期落地**：`vip_probe.rs` 探测器 + proto 字段 + 前端 `[M/N]`/行级 Tag；补 metrics、配置开关、`no_tun` |
| 2026-10-18 | **缺陷复审后修正**：① 目标截断加轮转游标（>3 时不再只探前 3 个）；② 任务纳入实例生命周期，重启不泄漏；③ 探测改走专用发送路径，不污染 `recent_traffic` 与用户流量；④ 回包按 `(peer, seq)` 校验归属，堵冒充；⑤ 每轮重取本机 VIP（DHCP 自愈） |
| 2026-10-18 | **探测粒度改为按链路**（§2.5）：每条 PeerConn 各发一个 probe（`send_probe_msg_on_conn`），行级 Tag 按 `conn_id` 取值；新增 `PeerVipReachability.conn_id` |
| 2026-10-18 | **验收口径澄清**：探针不经本机 TUN/OS 路由表，§5 中「TUN 路由错误」改为「阻断某条链路的 VIP 转发」；状态改为「代码已落地，待现网验收」 |
| 2026-10-18 | **汇总口径改为按链路**（撤销本次会话早期的「按节点」定稿）：`[M/N]` 的 M/N 均为**链路**数，与行级 Tag 同单位；主路坏备路通显示 `[1/2]`。中继节点的节点级条目**计 1 条**（避免全中继网络显示 `[0/0]`）。Tooltip 文案同步为「VIP 可达链路 M / 链路总数 N」 |
| 2026-10-18 | **P0 修复（按链路探测恒超时）**：`send_probe_msg_on_conn` 漏了 `to_peer_id == my_peer_id` 守卫，把 `not_send_to_tun` + `no_proxy` 也打给了发往**远端**的探测包——远端 `NicPacketProcessor` 因 `is_not_send_to_tun` 不投 TUN、`IcmpProxyEngine` 因 `is_no_proxy` 不应答，两个回包来源同时被堵，所有按链路探测恒「不可达」。判定逻辑抽为 `needs_self_delivery_guard`（自投递守卫），用户流量路径与两条探测路径**共用**，并补 3 条单测。 |
| 2026-10-18 | **P1 修复（disabled 语义）**：`instance_info.rs` 不再把关闭探测短路成 `Vec::new()`，改由 `snapshot_reachability` 逐 peer 输出「未知 + `disabled`」，与 §5 一致；原先的 disabled 分支不再死代码。 |
| 2026-10-18 | **P2 清理**：删除 `snapshot_reachability` 中只写不读的 `seen_peers`；前端移除不可达的 `[?/N]` 分支（`hasData=false ⟺ N 未知`，此时直接隐藏徽章），改为「有链路尚未给出结论 → 灰色 `[0/N]`」；§2.3/§5 文案与单测计数同步（`vip_probe` 9 + `peer_manager` 3）。 |
| 2026-10-18 | **定位锁定：永久纯观测，不承担保活（§2.4.1）**。评估过「加密探测以保活链路 / 加速切换」，**否决**：① NAT 保活已由 `ping_interval_max_sec=10s` 控制面 ping 覆盖（探测 20s 更慢，无增益）；② `pick_default_conn` 只读控制面 `ConnMetrics`，探测结果**不参与任何切路判据**，加密对切换时机零影响；③ 加密会与用户流量争抢同一条 conn、探测丢包可能加速误判、空闲期制造假流量干扰 lazy-p2p。同步在 `vip_probe.rs` 模块注释与 `PROBE_INTERVAL` 上写死「不要调快」的理由，并新增 §2.4.2 说明切换时延的真实归属。 |
| 2026-10-18 | **`N` 定为「实际探测链路数」，新增「未探测」状态**：上一版曾把 `N` 改为表格行数，但那会让"无探测记录的行"进入 `N` 却永远无法贡献 `M`，健康网络也到不了 `[N/N]`。现 `N` 回到记录数（与 `M` 同源），并由 `vipReachabilityIndex.wasProbed` 区分「从未探测」（行内标『未探测』+ Tooltip 说明「不计入 N」）与「旧内核无数据」（仍为『未知』）。两个数字的差额由表格自身标注，不再靠并排数字解释。 |
| 2026-10-18 | **（中间方案，已被上行推翻，仅留档）** 徽章分母曾改为「表格行数」：实测出现「徽章 `16` / 旁边 `19` / Tooltip 又说 `18`」的三数矛盾——后端记录数（枚举路由表）与表格行数（枚举 PeerConn）本可不等。**该方案已撤销**，见上行「`N` 定为实际探测链路数」；`vipLinkTotal` 现显式忽略行数参数。**其中一项仍然有效并保留至今**：节点信息面板标题**不再并排显示裸行数徽章**（重复且易误解），同步删除 `peer_info_rows_tip` / `peer_info_rows_only_tip` 两条 i18n。 |
| 2026-10-18 | **未知态必须显示原因**：`vipStateTip` 原只在 `unreachable` 时输出 `error`，导致整屏「未知」无法排障。现 `unknown` 也显示原因，并直接拼进行内 Tag（`未知 · 本机尚未分配虚拟 IP`），覆盖 `disabled` / `no_vip` / `no_tun` / `timeout` / `send_failed`。 |
| 2026-10-18 | **切换时延补齐实测窗口**：`default_conn` 缓存清空周期为 **5s**（`peer.rs`），故 `switch_windows=3` ≈ 15s；断链换路由 fast probe（3×333ms）驱动，约 **2s** 即可切换。 |
| 2026-10-19 | **P0 修复（现网全「未知」/`[0/N]`）**：Tauri 经 pbjson 把 `PeerVipState` 序列成字符串 `"PEER_VIP_STATE_*"`（`state==0` 时甚至省略字段），前端却用数字 `0–3` 比较 → 每行都落成「未知」、徽章恒 `[0/N]`。新增 `normalizeVipState`；`peer_id` 也做 `Number` 强制（字符串键会导致「未探测」假象）。 |
| 2026-10-19 | **可达行只显示 rtt**：行内 Tag 由 `可达 7ms` 改为 `7ms`——数字本身就是数据面应答的证据，结论交给 Tag 颜色与「VIP」列头，省掉两个字的宽度。后端报可达但缺 `rtt_ms` 时仍回退显示「可达」，避免出现空 Tag。`不可达` / `未探测` / `未知 · 原因` 一律不变。 |
