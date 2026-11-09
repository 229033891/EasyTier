# 高并发下隧道停摆与丢包：完整分析（2026-11-07 ~ 2026-11-09）

> 本文是一次**代码评审 + 多轮现网实测 + 根因定位**的完整记录，不是设计文档，也不是施工单。目的是：
>
> 1. 固化 `0473c945` 及其后续修复的评审结论与每一轮复核发现的缺陷；
> 2. 固化可复现的测量方法、原始数据路径与**方法论陷阱**；
> 3. 明确哪些假设已被**证伪**，哪些结论已被**修正口径**；
> 4. 给出当前唯一未修的架构级缺陷的代码级根因、证据链与候选修复方案；
> 5. 提供可执行的验收判据，供后续"打包验证"使用。
>
> **本文不修改任何代码，不编译，不 push。** 本文记录的修复方案均为待决策项（§8）。
>
> 相关：[`packet-loss-investigation-2026-10.md`](./packet-loss-investigation-2026-10.md)、[`egress-fair-queue.md`](./egress-fair-queue.md)、[`../roadmap/live-next-hop.md`](../roadmap/live-next-hop.md)（Phase F 快速探测）、[`../roadmap/open-bugs-wave1-impl.md`](../roadmap/open-bugs-wave1-impl.md)（Wave 1 复核结论）。

---

## Status

| 项 | 状态 |
|---|---|
| 评审对象 | `0473c945`（Wave 1 Track A + LAN L0/L1/L2 加固）、`a15e51d2`（快速探测四值判定）及其间全部复核修复 |
| 代码评审 | **通过**。阻塞项 2 项 + P0/P1/P2 共 9 项已修；告警治理 5 处已收敛；门禁状态见 §2.4 |
| 探测误判（旧缺陷） | **已修且验证**：三值→四值判定 + 预算棘轮 + `DeliveryExcuse`；25 个单测通过 |
| 转发场景 `carried_data` | **结论：判定不改，只加观测**。`egress_drops` 诊断字段已落地，理由见 §2.5 |
| **新发现的架构级缺陷** | **已修复并验证**（2026-11-09 16:40）：根因是**出向控制面饥饿**，不是入向 128 队列自锁。修复见 §13.3，验证见 §13.5；§6 保留为**被证伪**的排查记录 |
| 缺陷严重度 | **P0 / 企业级不可接受**（数据面持续饱和 → 控制面被饿死 → 双向误判 → 拆连重建） |
| 实测数据 | `target/tmp/etmon6/`、`target/tmp/t10run/`；主日志 `%LOCALAPPDATA%\com.kkrainbow.easytier\logs\easytier.log` |
| 约束 | 按用户要求：跳过打包、跳过编译、不 push；验证以静态复核 + `cargo fmt --check` + 独立逻辑单测 + 现网压测为主（门禁状态另有更正，见 §2.4）。**2026-11-09 起该约束已解除**，见 §12.0 |
| **2026-11-09 追加** | 已完成**纯插桩改动**（3 文件，未提交）并准备好取证脚本；**待办测试 T1~T9** 与**勘误 13 条**见 **§12** |
| **2026-11-09 16:40 定案** | **T1 取证完成，根因证实为「出向控制面饥饿」；P0 修复已落地、通过门禁、部署到两端并实测通过。** 见 **§13**。§0 / §6 / §8 / §11 的旧定性均以 §13 为准 |

> **⚠️ 先读 §13。** 2026-11-09 16:40 的实测**推翻了 §6 的「入向 128 队列自锁」假设**：真因是出向 32 槽队列在满速下长期 100% 占用，**控制面 `try_send` 被数据面 `send().await` 饿死**。修复只有 15 行，两端部署后停摆归零。§0 / §6 / §8 / §11 保留原文作为排查过程记录。

> **阅读本文前先看 §12。** 2026-11-09 的补充评审新增了两类内容：
> 1. **§12.1 待办测试清单（T1~T9）** —— 本文的结论目前还缺一次直接观测（§6 是「最强假设」而非「已证实的根因」），T1 就是把它钉死的那一次实验；
> 2. **§12.2 勘误 13 条** —— 正文中有 13 处表述需要更新（最要紧的是 §4.4 的速率口径、§5.5 的 0.97Gbps 疑虑、§6 的定性、§8.2/§8.3 的方案细节）。正文对应位置已就地加了 **13 处** `勘误 §12.2-#n` 标记（其中 #4 在 §6 开头与 §11 末尾各标一次，#13 与 §11 末尾那条合并）。

---

## 0. 执行摘要

**一句话结论**：这一轮改动的**评审对象是干净的**——Wave 1 加固、告警治理、快速探测的误判修复都达到企业级标准且经多轮复核；但改完之后用干净链路做的高并发压测，暴露出一个**与这些改动无关的、既有的架构级缺陷**：EasyTier 的收包循环把「批量数据投递」和「控制面 Pong 应答」放在同一个可能无限阻塞的循环里，队列一满就双向自锁，表现为整条隧道突然全黑十几到二十几秒，随后自愈。

> **⚠️ 已修正（2026-11-09 16:40）**：上面这段对**现象**的描写仍然准确，但**根因定性已被实测推翻**。
>
> 真因不是入向 128 队列自锁，而是**出向队列的控制面饥饿**：满速下 32 槽出向队列持续 100% 占用，数据面靠 `send().await` 每次 ≤5ms 挤进去，控制面的 `try_send` 则**每一轮都失败**；累积 30~40 秒后被判 `LocalCongestion` 并拆连。决定性数据：`.16` 在停摆期间 `feeds` 8.8 万包/秒（≈112MB/s 线速）、`max_feed_ms=1~3ms`、`writer_idle_ms=0`，唯独 `enq_full` 每秒 +3（保活全灭）。
>
> 修复只有约 15 行（保活队满时有界等待 50ms），两端部署后**停摆 0 次、超时 0 条**，150s × 12 流吞吐 **94.12MB/s**（旧版最好 85.62）。完整证据、修复与验证见 **§13**；下面三条要点中的**第 3 条（根因假设）随之作废**。

三条要点：

1. **不是网络问题**。决定性证据：隧道 `10.10.10.16` 连续 27 次 ping 超时（约 26 秒全黑）的同时，物理地址 `192.168.11.124` 908/908 全通、RTT 0–3ms；本机网卡 242 次采样 discard/error 全 0（期间收发 15.1GB）。主机与底层链路 100% 健康。
2. **不是探测误判**。旧缺陷是「拥塞被误判成链路死亡」，本缺陷反过来是「链路真的死了」：`rx_delta=0`、`tx_delta` 只剩探测包自身、`egress_drops=0`，日志签名是真黑洞而非预算不足。四值判定、预算棘轮、`DeliveryOnly` 在停摆时都拿不到任何投递证据，因此救不了。
3. **根因假设在共享队列**（**最强假设，待 T1 确认**，见 §12.2-#4）。全局唯一的 `mpsc::channel(128)`（`peers/mod.rs:120`）在 90MB/s 下只有约 **1.8ms** 缓冲；队满后 `send_peer_packet_to_chan` 退化为 `.send().await`（`peers/mod.rs:132-136`），直接卡住 `start_recv_loop`（`peer_conn.rs:1418`）。而**Ping 的 Pong 应答也在同一个循环内**（`:1545-1564`，且 `sink.send().await` 无超时）。循环一停 → 不读 socket、不回 Pong → 对端判我们死、我们对端也判死 → 降级 → 剔边 → 拆连重建。

**建议动作**（2026-11-09 勘误后口径，与 §12.0 一致）：**先 T1 取证**（半天插桩 + 一次定向复现，判定「本机自锁 / 对端不发 / 上游丢弃」）→ 再 **A1+A2+A3 同批**做 P0 止血（收包循环任何分支都不得因下游容量而 `await`，控制面优先、数据面 tail-drop 并计量）→ 三处验收 → 消费端吞吐整改 → B-lite → 交叉归因。**不建议**放宽探测阈值、加大 128 队列、或用 `tx` 判活——理由见 §8.6。

---

## 1. 范围、版本与环境

### 1.1 时间线

| 时间 | 事件 |
|---|---|
| 2026-11-07 | 评审 `0473c945`（19 文件，+710/−50）；发现 2 项阻塞 + P0/P1/P2 共 9 项；用户分三轮修复，AI 逐轮复核 |
| 2026-11-08 上午 | 三节点空载 + 并发压测；发现快速探测把拥塞误判为 100% loss（业务 103/103 成功） |
| 2026-11-08 11:00 | v1 修复 `path_carried_data`；当日改为企业级四值判定 + 预算棘轮 |
| 2026-11-08 下午 | 用户提示 `.10/.20` 有带宽限制；改测局域网 `.201`，链路完全干净 |
| 2026-11-08 傍晚 | 复核发现 `DeliveryExcuse` 棘轮是死代码（参数收了没读），修复并补反证实验；`a15e51d2` 落地 |
| 2026-11-08 19:13 | 用户重新打包并安装（本机 / .201 / .10 / .20 / .16 二进制时间戳一致） |
| 2026-11-09 00:54–01:30 | 对 `10.10.10.201` 与 `10.10.10.16` 双目标压测；.201 干净，.16 复现 5 次隧道停摆 |

### 1.2 评审与运行版本

| 项 | 值 |
|---|---|
| 评审提交 | `0473c945 docs(roadmap): 更新 LAN 直连 underlay 加固提案与开放缺陷施工方案` |
| 当前 HEAD | `a15e51d2 fix(core): 快速探测改四值判定与预算棘轮，拥塞不再误报丢包`（2026-11-08 18:27 +0800） |
| 版本号 | 2.7.11（`0f2989f0`） |
| 实测二进制 | `ET.exe` 2026-11-08 19:13 构建；日志含 `budget_ms` / `next_budget_floor_ms` / `rx_delta` / `egress_drops` 字段，确认改动在跑的包里 |

### 1.3 拓扑

| 节点 | ET 虚拟地址 | 物理地址 | 隧道 | 备注 |
|---|---|---|---|---|
| 本机 DEV | `10.10.10.101/24` | `192.168.10.101/24` | — | Windows，GUI 托管 easytier-core，RPC `127.0.0.1:15999`；10Gbps vmxnet3 |
| APP-SERVER | `10.10.10.10` | `192.168.10.x` | tcp p2p，6.03ms | **有带宽限制**（写入 ~1.45MB/s），Web 服务 :8001 |
| DB | `10.10.10.20` | — | udp p2p，5.74ms | **有带宽限制**（写入 ~4.01MB/s） |
| 对照节点 | `10.10.10.201` | `192.168.10.x` 同 L2 | wg / LAN p2p，0.3ms | SMB C$；磁盘顺序读 ~87–120MB/s 是吞吐天花板 |
| **故障节点** | `10.10.10.16` | **`192.168.11.124`** | **udp p2p**（日志确证 `udp://192.168.11.124:11010`） | IIS 静态文件 `:80`；到本机**无下一跳**（同 L2，0ms） |

`et` 虚拟网卡 MTU = 1280（1200B 负载可通，1300B 不通）。`.16` 的 peer id = `1959083652`，本机 peer id = `3427831975`。

### 1.4 约束与方法边界

- 用户明确要求：**跳过打包、跳过编译、不要 push 代码**；因此早期验证以静态复核 + `cargo fmt --all -- --check` + 把纯逻辑抽到独立 crate `target/tmp/probecheck` 真实编译跑单测为主。后来环境限制解除，补跑了真门禁（见 §2.4 口径更正）。
  - **2026-11-09 更新（勘误 §12.2-#1）**：该约束已解除。已落地**纯插桩**改动 3 处（行为零变更、未提交），并用真门禁验证：`cargo check -p easytier-core --lib` exit 0、`cargo clippy -p easytier-core -- -D warnings` exit 0、`cargo fmt --all -- --check` 干净、`cargo test -p easytier-core --lib peer_conn_ping` **25 passed / 0 failed**。取证脚本见 `target/tmp/stallprobe/`。
- PowerShell 执行策略禁止直接运行 `.ps1`，统一用 `[scriptblock]::Create((Get-Content -Raw ...))` 调用。
- 所有结论以**可复现的原始数据**为准；本文对早期 memory 记录中的口径偏差做了显式修正（§10.2）。

---

## 2. 第一部分：代码评审与修复复核

### 2.1 `0473c945` 评审结论

总判：**方向正确，但两处结构性漂移使原方案不能直接开工**，必须修正接入点后施工（`open-bugs-wave1-impl.md` §0）。

| 条目 | 判断 | 处置 |
|---|---|---|
| LAN L1+L2+L0 | 方向正确；两处结构性漂移必须先改方案 | 否（须按 §2 修正后） |
| B3 冲突不覆盖 + warn / B4 许可门 / B17 文档 i18n | 合理 | Track A，可开工 |
| B3 完整 A/B、B2/B10/B15/B16/B17 协议 | 需设计 | 本波不做 |

**两项阻塞缺陷**（评审发现，已修）：

1. **L1 剪枝挂错 choke point**。桌面 desired 主要由 `cidr_monitor::collect_proxy_cidr_state` → `ProxyCidrsUpdated` 驱动，`virtual_nic` 事件路径**不**再经 `packet_plane` 收集；只剪 `packet_plane` → 事件路径仍会装未剪枝前缀。正确做法是在 `cidr_monitor.rs` 对 `resolve_proxy_cidrs` 结果剪枝，`local_nets` 用共享 `ArcSwap` 供 monitor 读取。
2. **L2 `needs_excludes` 只改了 tick 门闩**。真正决定是否装 `/32` 的是 `sync_proxy_cidrs_with_underlay_excludes` 内的 `will_have_tun_default`；只改 tick → 非 `/0` 的 `/22` 永远不装 exclude。必须同时改 install 门闩与 tick 门闩，或抽成同一谓词。

**其余评审要点**：L1 alone 不够（`/22 − 本机 /24` 仍可能覆盖对端物理 IP）；`local_nets` 当时不存在，`is_ip_in_same_network` 是 overlay VIP 不能当物理 LAN；Windows 接口枚举缺 TUN 过滤（需 `exclude_ifname`）；underlay exclude **会**收集 RFC1918（原文表述反了，已勘误）；B4 改完后「仅有 proxy_cidrs、未开 exit node」的节点不再为 `is_exit_node` 包做互联网 NAT（需 changelog）。

**P0/P1/P2 复核修复**（用户分三轮修，AI 逐轮复核）：

| 级别 | 问题 | 修复 |
|---|---|---|
| P0 | L1 启动未先采、monitor 无快路径 | 启动先采 + set 变化 wake + monitor `Arc` 快路径 |
| P0 | `prefix<8` 超宽掩码 | 跳过 + 60s 限频 warn |
| P1 | 未知 vs 空无法区分 | `Option<&[Ipv4Cidr]>`；L2 单测补在 `packet_plane` 末尾 |
| P1 | 同步接口枚举放在 async 上下文 | 入口级 `spawn_blocking`，底层保持同步 `show` 以保 netns 亲和 |
| P1 | ICMP 冲突日志刷屏 | 60s 限频 warn |
| P2 | manual 全删/部分剪文案不分、IPv4 skip 级别 | 分文案；IPv4 skip 改 `debug!` 对称 IPv6；仅桌面 |
| P2 | 采集失败 `expect` 直接 panic | 返回 `Option::None`，`set_*` 保持旧值；`JoinError` 降级不 panic updater |
| P2 | netns 阻塞模式重复 | 抽出无条件 `run_in_namespace_opt`（spawn_blocking + setns + current-thread runtime，`JoinError`/建 runtime 失败 → `None`），`run_in_namespace` 改为薄封装 `.expect`；Linux/非 Linux 双分支合并，净减 74 行 |

**验收勾选现状**：A1/A2/A3、L0、L1、L2、文档勘误、P0/P1/P2 全部已勾；**仍空**的是「现网验收：推送覆盖性 `/22` 后直连稳定」与「L2 拆除窗口端到端」。原因见 §10.3：本机 LAN `192.168.10.0/24` 与现有 proxy CIDR 无重叠，L1/L2 路径**本轮一次都没被触发**，只证明了「无退化」，不能证明剪枝逻辑正确。

### 2.2 告警治理

实测发现 4 类既有告警噪声，均已治理（不改判定语义）：

| 位置 | 改动 |
|---|---|
| `hole_punch/udp/client.rs` | 5 处重试循环内 `error!` → `debug!`，清掉 47 条 ERROR；汇总结果由 `connector.rs` 的 `hole punching failed` 承担 |
| `tunnel/encrypt/ring.rs` | `packet is already encrypted` 由 `warn!(?packet)` 改 `debug!(payload_len=...)`，不再把整段密文写日志 |
| `peers/peer_manager.rs::try_compress_and_encrypt` | 入口若已加密直接返回，避免对密文再压缩 |
| `management/full/web_client.rs` | 心跳首次瞬态失败降 `debug!`，第 2 次起仍 `warn!`（31 次全部 attempt=1 且重试成功）；两分支字段提到局部变量消除双份维护 |
| `hole_punch/udp/binding.rs` | UPnP 映射失败 `warn!` → `info!`（有明确 fallback） |

### 2.3 快速探测误判：从布尔合并到四值判定

**旧缺陷（2026-11-08 上午实测）**：`fast_probe_timeout = clamp(3 × EWMA RTT, 300ms, 2s)`，而**超时不产生 RTT 样本** ⇒ EWMA 学不到后起的队列 ⇒ 预算钉死 300ms。`.10` 在 4 路负载下隧道排队 656ms ≫ 300ms，于是连续 miss。

时间线（02:52）：`02:52:17.758` 报 `link not round-tripping`（rtt_us=6008，正是 6ms 的 .10）→ `02:52:21–31` 遥测 `loss=100%` → `02:52:26.339` OSPF 把该边从 SPF 剔除 → `02:52:30.346` 恢复 cost → `02:52:34.308` 探测恢复。**反证**：这 18 秒内业务侧 103 次请求 103 次全部成功，链路一直是通的。

另有保活补不上的问题：`PingIntervalController` 空闲退避 `1<<backoff_idx`（`max_backoff_idx=5` ⇒ 最长 32s 一次），探测决策的 ~2s 窗口内 `roundtrip_epoch` 常常不变。

**修复演进（四轮）**：

| 版本 | 做法 | 被否定的原因 |
|---|---|---|
| v1 | `path_carried_data = rx_after > rx_before`；`roundtrip_proven = epoch_advanced \|\| path_carried_data` | 把「单向有入向字节」和「完成往返」等价，三个缺陷：① 本机每 333ms 会收到对端 pinger 的探测 ping，本身计入 `rx_bytes`，单向故障时该值几乎恒真 ⇒ 本机判活、对端判死，流量持续打进黑洞且不改道；② `DeliveryOnly` 直接 `hits+1`，恢复门槛从 3 轮缩到 2 轮；③ 预算不自适应，队列只要再涨就复发 |
| v2 | `enum RoundOutcome { Miss, Inconclusive, DeliveryOnly, Proven }`；`DeliveryEvidence { rx_delta, budget }`；预算棘轮 | 首版 `DeliveryExcuse` 形同虚设，见 v3 |
| v3 | `DeliveryExcuse { spent }` 每 episode 闩锁；`Inconclusive` 既不花也不还 | 复核发现 `excuses_a_timeout(&self, excuse)` **收了 `excuse` 却没读它**，判定仍是 `carried_data && budget < CEILING`，与 v2 逐字等价 |
| v4 | 棘轮真正生效 + `rx_delta/tx_delta/egress_drops` 诊断字段 | 当前形态 |

**v3 那个缺陷值得单列**，它是本轮评审中最有价值的一条教训：

> 编译器直接报了 `unused variable: excuse`，但上一轮记录的「21 passed」里**没有一条能发现它**。反证实验：把 `!excuse.spent` 换成 `true` 复现旧行为，`a_one_way_outage_is_published_even_though_the_floor_falls_back` → **FAILED**（单向故障永不发布降级）。另发现「`floor >= queue`」的断言只是因为循环跑了 30 轮（偶数）才成立，改成 29 轮就红；已改为验性质（首次 `Proven` 必须落在第 3 轮 + 新增 `max_budget ≥ queue`）而非轮次奇偶。
>
> **教训：新增测试通过 ≠ 新增机制生效。凡是「加了参数 / 棘轮 / 闸门」的改动，必须有一条消费该状态的用例，并用一次对照实验（把状态置为常量）确认它会变红。**

**当前代码形态**（`easytier-core/src/peers/conn/peer_conn_ping.rs`）：

| 位置 | 内容 |
|---|---|
| `:95` | `FAST_PROBE_INTERVAL = 333ms` |
| `:105` | `FAST_PROBE_MISSES_TO_DEGRADE = 3`（发布 100% loss） |
| `:127` | `FAST_PROBE_RECOVER_ROUNDS = 3`（解除降级需要 3 轮 `Proven`） |
| `:143` | `FAST_PROBE_SUSTAINED_ROUNDS = 9`（本地 SPF 剔边） |
| `:172-185` | `RoundOutcome` 四值枚举 |
| `:189-235` | `DeliveryEvidence { rx_delta, tx_delta, egress_drops, budget }`；`carried_data()` / `excuses_a_timeout()` |
| `:260` | `DeliveryExcuse { spent }`，仅 `Proven` 清零 |
| 常量 | `FAST_PROBE_TIMEOUT_MIN=300ms` / `MAX=2s` / `FAST_PROBE_EXCUSE_MAX_BUDGET=1200ms` |

判定语义：

- `Proven`（探测 pong / 保活 pong / liveness echo，一轮只算一次）：`misses=0, hits+1`，`budget_floor` 减半回落至 300ms。
- `DeliveryOnly`（超时但有入向字节、预算 < 1200ms、且本 episode 未花掉豁免）：`misses=0`，**`hits` 不动**，`budget_floor` 翻倍（上限 2s）。
- `Inconclusive`（`LocalCongestion`，ping 没出本机）：两个计数器与棘轮都**不动**。
- `Miss`：`misses+1, hits=0`，`budget_floor` 减半。

**设计不变量**：入向字节可以结束「判死」的连续 miss，但**永远不能解除一次已发布的降级**（那需要双向往返证据）。否则单向故障会靠持续推来的字节永远占着 SPF 里的边。

### 2.4 门禁状态与口径更正

早期记录「`cargo check` 不可用」**已过期**：`script/build-common.ps1` 的 `Import-EasytierMsvcEnvironment` 会在入口自动 `call vcvars64`，`ring` 的 C 构建不再缺 MSVC 环境变量。实测：

| 门禁 | 结果 |
|---|---|
| `cargo check -p easytier-core --lib` | **exit 0**（仅 2 条既有 `inject_self_delivered_nic_packet` dead_code 警告，后已加 `#[cfg(feature = "proxy-packet")]` 消除） |
| `cargo check -p easytier-core --features proxy-packet` | exit 0 |
| `cargo test -p easytier-core --lib peer_conn_ping` | **25 passed / 0 failed**（不再需要外挂 `probecheck` 抽取件） |
| `cargo clippy -p easytier-core -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | 干净（曾有 6 处未格式化，已 `cargo fmt --all` 修好） |
| 完整 clippy `easytier` 包 | 本机跑不通：`aws-lc-sys` 的 `stdalign_check.c` 在 MSVC `-WX` 下报错，属原生构建环境问题，交 CI Linux 门禁 / `script\easytier-test.cmd fast` |

### 2.5 转发场景 `carried_data` 恒为 false：结论是「判定不改，只加观测」

用户观察到的现象成立：06:44 那条降级对应 peer `2962473162`，remote 是 `udp://192.168.10.96`（LAN 直连，不是 .10）。本机在给它**转发**流量（同期 1 条 `Tunnel(BufferFull)` 转发丢包），但 `throughput_stats.rx_bytes()` 只统计入向，转发出去的流量只增 `tx`，所以判定逻辑对 relay 连接是瞎的。

**结论：不能用 `tx` 判活**，理由是安全方向问题：

- 对端变黑洞时本机 `tx_bytes` 照样增长（写本地 socket buffer 成功，直到缓冲满）。用 `tx` 豁免超时 = **永不判死**，流量持续打向死路并一直占着路由表，比「偶尔误剔边」严重得多。
- relay 场景其实不缺证据：pong 本身就是 `rx`，探测成功即 `Proven`，根本走不到 `carried_data` 分支。
- 本机出口已满由 `try_send` → `BufferFull` → `LocalCongestion`（`Inconclusive`）覆盖（`peer_conn_ping.rs:617` 附近）。

**已落地的观测增强**（4 文件，+92/−14）：

| 文件 | 改动 |
|---|---|
| `tunnel/stats.rs` | `Throughput` 新增 `egress_drops: AtomicU64` + `egress_drops()` / `record_egress_drop()`，`Clone`/`Default` 同步。放在 `Throughput` 而非 fq 内部，是因为判定侧需要看见它 |
| `peers/conn/egress_fq.rs` | `EgressFairQueue` 新增 `throughput: Option<Arc<Throughput>>`；`new()` 保持零参（5 处测试不动），新增 `with_throughput()` builder；`note_buffer_full()` 同时 `record_egress_drop()` |
| `peers/conn/peer_conn.rs` | 构造 fq 时 `.with_throughput(throughput.clone())`（`:420`）；`send_msg` 的直连出向路径（`enable_egress_fair_queue=false`）也补 `record_egress_drop()`，否则该配置下「`drops>0` ⇒ 本机出口饱和」永远不成立 |
| `peers/conn/peer_conn_ping.rs` | `DeliveryEvidence.carried_data: bool` → `rx_delta: u64`，新增 `tx_delta` / `egress_drops`，**均为观测字段，不参与判定**；`publishing degraded loss` 与 `degradation sustained` 两条 warn 输出 `rx_delta` / `tx_delta` / `egress_drops` |

四种日志组合的含义（写进代码注释）：`rx=0,tx=0,drops=0` 真黑洞；`rx=0,tx>0,drops=0` 灌黑洞；`rx>0` 预算不足；`drops>0` 本机出口饱和。

**这批字段在本次 .16 故障中直接发挥了作用**：`rx_delta=0 tx_delta=40 egress_drops=0` 一行就把「本机出口饱和」和「预算不足」同时排除，见 §5.4。

---

## 3. 实测方法（可复现）

### 3.1 工具链与脚本

| 脚本 | 用途 |
|---|---|
| `target/tmp/etmon6/smb_raw.py` | SMB 直读，`FILE_FLAG_NO_BUFFERING` + `VirtualAlloc` 页对齐缓冲 |
| `target/tmp/etmon6/smbload.py` | SMB 多流压测（线程池 + 每流独立文件句柄） |
| `target/tmp/etmon6/httpload.py` | HTTP keep-alive 多流压测（`http.client` 单进程 + 线程池），支持 SMB/HTTP 混合 |
| `target/tmp/etmon6/dualping.ps1` | **双路 ping**：同时探 `10.10.10.16`（隧道）与 `192.168.11.124`（物理），250ms 一轮 |
| `target/tmp/etmon6/ping_201.ps1` / `ping3.ps1` | 对照节点 ICMP 采样 |
| `target/tmp/etmon6/nic_probe.ps1` | 本机网卡 rx/tx discard/error 计数器采样 |
| `target/tmp/etmon6/srv_probe2.ps1` | 远程采 .201 磁盘队列 / 读延迟 |
| `target/tmp/etmon6/logwatch.py` / `analyze6.py` | 主日志事件抽取、CSV 统计 |
| `target/tmp/t10run/` | `.10` 高并发 4/8/16 流压测产物 |

原始数据：`target/tmp/etmon6/run{1,3,4,5,6,7,8,9,10}/blk_*.csv`（字段 `ts_utc,stream,bytes,ms,err`）、`dual.csv`、`nic.csv`、`phases.log`、`probe_events.log`；主日志 `%LOCALAPPDATA%\com.kkrainbow.easytier\logs\easytier.log`。

### 3.2 方法论陷阱（都是踩过的坑，后续复现必须避开）

1. **SMB 客户端缓存会造出假吞吐**：普通 `open().read()` 读 `\\201\C$` 命中 Windows SMB client cache，测出 **1883MB/s** 的假数据。必须用 `FILE_FLAG_NO_BUFFERING` + 页对齐缓冲，真实单流约 40MB/s。
2. **MSYS fork 并发死锁**：每请求 fork 一次 `date/cut/awk` 的脚本，在 MSYS 并发下会死锁（实测 `date` 卡死，4 条流里 2 条在 2s/17s 处停住），据此得出的「卡顿」是测量工具造成的。改为 Python 单进程 + 线程池 + keep-alive。
3. **PowerShell 执行策略**：禁止直接执行 `.ps1`，用 `[scriptblock]::Create((Get-Content -Raw ...))`。
4. **ICMP 是否被代理**：ping 网段内不存在的地址 `10.10.10.250/99` → 100% 丢失，确认 ping 走真实隧道、未被 ICMP proxy 伪造应答，结果可信。
5. **`.201:80` 是 http.sys（Microsoft-HTTPAPI）无静态资源**，故 .201 只能走 SMB（445/3389 开放）；`.16` 的 IIS 有 `iisstart.png`（98 757 B）可走 HTTP keep-alive。
6. **`.10`/`.20` 有带宽限制**（用户明确指出）：写入分别只有 ~1.45MB/s / ~4.01MB/s。这两条链路上观察到的丢包与 p99 膨胀**不能用来评价 EasyTier**，必须换到无限制的 LAN 节点。
7. **双路 ping 的采样耦合**：`dualping.ps1` 每轮对两个目标各发一次 ping，隧道目标超时会同步占用 1000ms，物理目标的采样间隔随之拉长。因此「两者样本数相同」不能推出「采样密度相同」；但物理目标 **908/908 全 ok** 这一事实不受影响。
8. **分析脚本正则**：早期 `analyze.py` 漏掉 `rx=150.08 kB` 中的空格，单流 OUT 路径也指错过，均已修正。

### 3.3 双路判别法

同时 ping 隧道 VIP 与对端物理 IP，是本次定位的决定性方法：

- 两者同时丢包 ⇒ 主机 / 物理链路 / 网卡问题；
- **仅隧道丢包、物理全通 ⇒ ET 数据面内部问题**；
- 仅隧道延迟膨胀而物理平稳 ⇒ ET 排队（bufferbloat）。

再叠加本机网卡 discard/error 计数器与 `tunnel_buffer_full` / `data_plane_no_dst_peer_drops`，可把故障面收敛到 ET 进程内。

---

## 4. 实测数据全景

### 4.1 空载基线（2026-11-08，`2.7.11-abba9d83`）

- 12 分钟 4264 个 ICMP 样本，`.10` / `.20` 两条链路丢包 **0**；p50=p90=p99=6ms，最大值 20ms / 14ms 各 1 次（0.07%）。
- 路由表 0 次变更、peer 会话集合 0 次变更、网卡 discard/error 全 0。
- ET 遥测 141 次采样：cost=p2p 恒定、loss=0.0%、lat 波动 0.06ms；TCP 建连探针 141/141 成功。

### 4.2 `.10` / `.20`：限速链路，仅作背景

| 场景 | 现象 |
|---|---|
| 4 路并发下载打 `.20` | `.20` 小包丢 10.7%（17/159）、大包 3.1%；同期 `.10` 丢 0%；ET 遥测同步出现 1.0% loss |
| 单流下载 40s | 两边均 0% 丢包 ⇒ 丢包与并发压力相关 |
| 吞吐硬顶 | `.20` 在 2/4/16 路均为 1.08–1.09MB/s；`.10` 在 2/4 路 0.46–0.47MB/s，16 路反跌到 0.31MB/s |
| 拥塞形态 | `.20` 纯 tail drop（p50/p99 恒 6ms，无延迟膨胀）；`.10` bufferbloat（4 路 ICMP p99 663ms，16 路转大量丢包，大包丢 57%） |
| 新包复测（`.10`，4/8/16 流各 60s） | 业务 406/406、376/378、389/390 成功；ping 500 样本 **11.0% 丢包**，p50 20ms / p99 616ms；**探测零误判**，`.10` 的 conn 全程无 `publishing degraded loss`、无剔边 |

`.10` 新版不再误判，**主因不是探测改动而是两端同步升级**：`egress_fq`（提交 `df95ab88`）让控制包走 `EGRESS_PRIORITY_CAP=32` 优先道，旧包 `.10` 侧的 pong 被自身 bulk 数据堵住才导致本机探测超时。这次测试中 `carried_data` **全部为 false**，`DeliveryOnly` 与 `budget_floor` 上浮**从未被触发**——探测改动是第二道闸，备用。

### 4.3 `.201`：完全干净的对照组

| 项目 | 数据 |
|---|---|
| SMB NO_BUFFERING 12 流 | **108.27 MB/s**（另一轮 113.87MB/s / 25 992 块 / 0 错误），p50 26.5ms、p99 64.9ms、max 913ms |
| ping `.201` | 1469 样本 **0 丢包**，p99 20ms |
| 探测事件 | **0 条** degraded / 0 次重建 |
| 913ms 毛刺归因 | `.201` 自身磁盘队列：SMB 881ms 卡顿 ↔ 磁盘读队列 10.55 / 读延迟 60.1ms；689/535/498ms 卡顿 ↔ 队列 11.57 / 读延迟 106.2ms，时间戳逐秒重合，同期 ping 反而降到 0ms（ICMP 不落盘） |

对照表（写入 256MB / 读吞吐，MB/s）：

| 节点 | 隧道 | 写 256MB | 单流读 | 4 流读 | 12 流读 |
|---|---|---|---|---|---|
| `.201` | udp p2p 0.3ms | 24.9 | 38.9 | 81.7 | **105.8** |
| `.20` | udp 5.8ms | 4.01 | — | — | — |
| `.10` | tcp 6.0ms | **1.45** | — | — | — |

### 4.4 `.16`：高并发核心数据

压测方式：IIS 静态文件 `http://10.10.10.16/iisstart.png`（98 757 B），`http.client` keep-alive 多线程循环。下表按 `blk_*.csv` **原始记录复算**（吞吐 = 窗口内成功字节 / 实测时长；延迟单位 ms；`err` 为脚本显式错误计数，长尾卡顿可能只体现在 `max` 而不计入 `err`）：

| 阶段 | 产物 | 流 | 时长 | MB/s | p50 | p99 | max | err |
|---|---|---|---|---|---|---|---|---|
| h4 | run6/blk_h4 | 4 | 45.0s | 93.31 | 3.8 | 10.5 | 68.9 | 0 |
| h6 | run7/blk_h6 | 6 | 40.0s | **100.22** | 5.6 | 11.9 | 331.1 | 0 |
| h8 | run7/blk_h8 | 8 | 40.0s | 89.35 | 7.8 | 20.1 | 397.2 | 0 |
| h9 | run8/blk_h9 | 9 | 60.0s | 90.75 | 8.5 | 22.2 | 668.8 | 0 |
| **h8long** | run8/blk_h8long | 8 | 120.0s | 65.75 | 9.5 | 25.7 | 677.4 | **8** |
| **h10** | run7/blk_h10 | 10 | 40.0s | 36.50 | 9.6 | 30.4 | **7352.6** | **10** |
| h12 (崩) | run6/blk_h12 | 12 | 59.8s | 61.51 | 13.9 | 45.2 | **9726.9** | 0 |
| **h12 (100s 全清)** | run9/blk_h12 | 12 | 100.0s | 88.65 | 12.5 | 24.3 | 275.6 | 0 |
| h12a | run10/blk_h12a | 12 | 75.0s | 85.67 | 12.9 | 28.5 | 410.6 | 0 |
| **h12b** | run10/blk_h12b | 12 | 62.2s | 90.82 | 12.3 | 24.2 | 349.0 | **12** |
| h12c | run10/blk_h12c | 12 | 75.0s | 78.01 | 12.3 | 30.5 | **7229.7** | 0 |

**关键观察：崩溃不由并发数决定，而由「持续负载 + 概率」决定**。12 流有跑满 100s 全清的（run9），也有 75s 内崩的（run10 h12b）；8 流跑 120s 会崩（h8long），跑 40s 不会（h8）。这符合「队列被填满」的随机性，不符合「固定阈值触发」。

> **勘误 §12.2-#2（口径问题，会削弱本条论证）**：上表的 `MB/s` 是「窗口内成功字节 ÷ 实测时长」，而窗口本身包含停摆（如 h10 的 36.50MB/s 里有 7 秒全黑）。**用含故障的窗口平均速率去论证「崩溃与速率不单调」是循环论证**：平均速率被故障本身压低了。方向性结论仍成立，但应改用 `blk_*.csv` 逐块时间戳重算 **1 秒粒度瞬时速率**，并补一列「崩前 1s 瞬时 MB/s」后重新表述。该重算纯离线，可立即做（T9）。

### 4.5 五次隧道停摆（主日志可查，全部同一 peer `1959083652` = `.16`）

| # | UTC 时间 | 阶段 | 首条 degraded | 剔边 | 拆连 | 重建 |
|---|---|---|---|---|---|---|
| 1 | 01:06:19 | h12/60s | 01:06:19.868 | — | 01:06:44（连接 removed） | 01:06:44.270 |
| 2 | 01:16:39 | h10 | 01:16:39.847 | 01:16:44.145 | 01:17:05.289 | 01:17:06.580 |
| 3 | 01:17:52 | h12 | 01:17:52.327 | 01:17:56.636 | 01:18:18.600 | 01:18:19.456 |
| 4 | 01:21:15 | h8long | 01:21:15.257 | 01:21:19.672 | 01:21:39.471 | 01:21:41.795 |
| 5 | 01:28:07 | h12b | 01:28:07.653 | 01:28:12.246 | 01:28:31.813 | 01:28:34.604 |

共同签名：**所有流同时卡死** → 探测 3 轮 `rx_delta=0` → 发布 degraded → 9 轮 sustained → 本地 SPF 剔边 → 约 24 秒后 `too many consecutive pingpong failures`（`loss_counter=13`、`fail_close_count=10`）拆连重建。

业务侧直接证据（run8 `blk_h8long.csv`）：`01:21:13.46 → 01:21:28.51` 出现 **15.05 秒的所有流同时无记录**空档，窗口内 8 条 `TimeoutError`。

---

## 5. 证据链：为什么确定是 ET 内部而非网络

### 5.1 决定性证据：双路 ping（run10，01:25:38–01:29:48）

| 目标 | 样本 | 成功 | 超时 | 成功样本最大 RTT |
|---|---|---|---|---|
| 隧道 `10.10.10.16` | 908 | 880 | **28 TimedOut** | 63ms |
| 物理 `192.168.11.124` | 908 | **908** | **0** | **3ms** |

连续全黑窗口：`01:28:05.925 – 01:28:31.644`，**27 次连续 TimedOut，约 25.7 秒**；窗口内物理侧全程 0–3ms。主机和底层链路 100% 健康，只有 ET 隧道死了。

### 5.2 恢复早于拆连 ⇒ 拆连不是恢复的原因

run8：业务数据面 `01:21:28.510` 恢复，强制拆连发生在 `01:21:39.471`，**早约 11 秒**。说明是队列自然排空后自愈，拆连只是降级逻辑的后续动作，不是解药。

### 5.3 排除项

| 假设 | 反证 |
|---|---|
| 本机网卡 / 驱动丢包 | 242 次采样，rx/tx discard 与 error **全 0**（期间收发 15.1GB） |
| 本机 ET 处理能力不足 | mix 阶段 HTTP(.16) + SMB(.201) 合计 ~115MB/s 无异常（run6/blk_mix 114.89MB/s，0 err）；若本机是瓶颈，.201 也应崩 |
| 物理链路 / 对端主机宕机 | 物理 ping 908/908 全通 |
| 两条链路互相干扰 | 201 与 16 同时压（mix）：仅 .16 出现 397ms 抖动，无崩溃；.201 全程 0 丢包 |
| 出口公平队列打满 | 日志 `egress_drops=0`，且 `egress_fq` 只作用于出向 `send_msg`，管不到入向共享通道 |
| 服务端 IIS / 磁盘 | 崩溃时**所有流同时**停，且物理 ping 正常；服务端磁盘问题不会让 ICMP 全黑 |

### 5.4 日志签名排除「预算不足」与「本机出口饱和」

```
01:28:07.653 WARN fast liveness probe: link not round-tripping, publishing degraded loss
  peer_id=1959083652 misses=3 rtt_us=Some(3581) budget_ms=300 next_budget_floor_ms=300
  rx_delta=0 tx_delta=164 egress_drops=0
01:28:11.467 WARN degradation sustained, local edge cost will be overridden ...
  misses=9 ... rx_delta=0 tx_delta=40 egress_drops=0
01:28:12.246 WARN local link state: connected peers that stopped round-tripping; pricing our edges
  out of the local SPF current={1959083652}
01:28:31.813 WARN too many consecutive pingpong failures, closing the connection
  loss_counter=13 fail_close_count=10 held_last_path=true hold_expired=true
01:28:34.604 INFO new peer connection added ... udp://192.168.11.124:11010
```

按 §2.5 定义的四种组合，`rx=0, tx=40（仅探测包自身）, drops=0` 属于「灌黑洞」：**没有任何入向投递证据，也不是本机出口饱和**。这正是 `DeliveryOnly` 无法介入的场景——它不是判定逻辑写错了，而是判定所需的证据在物理上不存在了。

> **勘误 §12.2-#8（这套图例的适用范围要收窄）**：`egress_drops` 只统计 fq 路径（`note_buffer_full`）与 `send_msg` 直连路径，而 **Ping 走 `sink.try_send`（`peer_conn_ping.rs:704`）旁路 fq，它的 `BufferFull` 不计入 `egress_drops`**。因此 `rx=0, tx>0, drops=0` 的正确解读是「本机 fq / `send_msg` 出口不饱和，且探测自身成功发出」，**不能**推广成「任何出口都不饱和」。`.16` 的结论不受影响（`tx_delta` 恰好证明探测包发出成功），但 relay / 低带宽场景不能照搬这条推断（→ T5）。

---

### 5.5 专项排查：「是不是触发了带宽上限？」

用户提出该质疑后做了专项复核。**结论：不是。** 速率是把队列压满的**必要压力**，但不是根因；且 ET 自身的带宽限速器根本没启用。

| 检查 | 结果 | 含义 |
|---|---|---|
| 物理网卡协商速率 | `Ethernet0`（vmxnet3）= **10 Gbps**；压测窗口 rx p50 0.70 / p99 0.95 / max **0.97 Gbps** | 只用了约 **9.7%**，物理链路远未饱和 |
| 网卡计数器 | `run7/nic.csv` 319 次采样，rx/tx discard、error **全 0**（窗口内 rx 累计约 20GB） | 本机 NIC 无溢出 |
| 物理 ICMP | `192.168.11.124` 908/908 全通，全程 **0–3ms** | 无排队延迟，排除 bufferbloat / 上游队列持续拥塞 |
| ET 内部限速 | 运行配置 `…\Roaming\com.kkrainbow.easytier\config.d\cd57f3d0….toml` **未设** `instance_recv_bps_limit` / `foreign_relay_bps_limit`（默认 `u64::MAX`）⇒ `recv_limiter()` 返回 `None`，限速分支不执行；主日志 0 条限速 / `BufferFull` / `recv error` | ET 层没有带宽闸门 |
| 限速器语义（即使启用） | `data_packet_payload_len` 对 Ping/Pong 返回 `None`（`traffic_kind` = `Control`）⇒ 控制包不计费；`try_consume` 失败只 `continue` 丢包，**不阻塞循环** | 限速器机制上无法造成「Pong 全丢 + 收包循环停摆」 |
| 对照节点 | `.201` 在 **113.87 MB/s**（物理约 0.96–1.0 Gbps）下 25 992 块 0 错误、ping 0 丢包、0 探测事件 | 同量级物理速率下不复现 |

**确实存在一个吞吐天花板**（约 90–100MB/s，4 流即达 93.31MB/s），用户观察到「被削平」是对的；但它与停摆不相关：

1. 撞天花板是**稳态削顶**，表现应是吞吐恒定 + 延迟随并发增长；实测却是「所有流同时 15–26 秒完全静默 → 自愈」，这是间歇性全停，不是限速曲线。
2. 4 流 93.31MB/s（已撞顶）跑 45s 零异常；12 流 85.67MB/s（未撞顶）反而崩；12 流 100s / 88.65MB/s 全程干净 ⇒ 崩溃与速率**不单调相关**，与「持续负载 + 概率」相关。
3. 唯一可疑点是 0.97Gbps 接近 1Gbps，若上游 vSwitch 对 ET 的 UDP 做 policing，ICMP 小包全通确实可以解释；但它解释不了「26 秒全黑后突然全通」「恢复与 ET 拆连/重建时刻重合」「01:21 恢复早于拆连 11 秒」，也解释不了本机日志 `rx_delta=0`（tunnel 层一个包都没收到）。

**仍需一锤定音的取证**（现有证据链只到「ET 进程内」，尚未区分本机 / 对端 / 上游）：

在停摆窗口对本机物理网卡抓包（`pktmon` / Wireshark，过滤 `udp.port == 11010`）：

- 物理层仍有双向 UDP 包、但 ET 不处理 ⇒ **本机 ET 收包循环自锁**（当前主假设，§6）；
- 只有我方发出、无对端回包 ⇒ 上游丢弃，或**对端 ET 进程**卡住（对端跑的是同一套 `start_recv_loop`，症状完全一致）；
- 物理层也完全静默 ⇒ 对端 / 上游问题。

配合对端 `10.10.10.16` 的 ET 日志（看它是否收到 Ping、是否回了 Pong）即可彻底三分。注意：物理 ping 全通只证明**对端主机内核网络栈**健康，**不能**证明对端 ET 进程健康。

> **勘误 §12.2-#3（「0.97Gbps 贴边」这一疑虑可以消解掉一半）**：
>
> - 0.97Gbps ≈ 121MB/s 线上字节；`mix` 阶段实测 114.89MB/s 载荷 × 1.052（1280B 载荷 + UDP/IP 28B + 以太网/前导 38B）≈ **0.966Gbps**。也就是说 **0.97 是载荷要求的结果，不是被钳在 1.0 的证据**——没有出现「rx 贴着 1.0Gbps 而载荷上不去」的钳位特征。
> - 但同一窗口内存在**约 15% 的缺口**：NIC rx 121MB/s 线上 vs `.16` 载荷峰值 100.22MB/s（≈105MB/s 线上）。这 15% 是「到了网卡、没被应用读到」，**首选解释是内核 UDP 收包缓冲溢出——也就是本机收包循环被 park 的预期副产品**。它支持 §6 的假设，而不是支持带宽封顶。
> - 建议把该缺口的取证列为 **T1 的一部分**（`netstat -s -p udp` 定时采样，见 `target/tmp/stallprobe/probe_sample.ps1`）：**比 `pktmon` 便宜得多，且能与全黑窗口逐秒对齐**。

---

## 6. 根因分析（代码级）

> **⛔ 本章结论已于 2026-11-09 16:40 被实测证伪，保留作为排查记录。以 §13 为准。**
>
> 三条决定性反证：
> 1. **循环不是停在投递点**：停摆期间 `send_peer_packet_to_chan` 的 `Full` 分支计时（≥20ms 报警）**三轮全部未触发**，Pong 的 `sink.send()` 计时也全部未触发；
> 2. **根本没有包到达**：停摆期间本机 NIC `rx = 0.00 MB/s`（连续 18 个采样）、内核 UDP 零包零错误。若是循环卡住导致缓冲溢出，`rx` 只会升高，不会归零；
> 3. **jam 在对端出向**：`.16` 每轮先报 `congested_ms=30000~40000`，而其 writer 全程 `writer_idle_ms=0`、`max_feed_ms=1~3ms`、`feeds` 8.8 万包/秒 ⇒ 队列是**满载**，不是**卡死**。控制面被数据面饿死，与入向队列无关。
>
> 本章第 3 条「控制面与数据面共用循环 ⇒ 双向自锁」的**部分洞察仍然成立**（控制面确实被数据面挤死），但**竞争的资源搞错了**：不是入向 128 队列，而是**出向 32 槽队列**。

> **勘误 §12.2-#4（定性需要降一档）**：本章建立的是**最强假设**，不是已证实的根因。证据链是「代码结构完全支持 + 排除法排除所有替代解释 + 因果链自洽」，但缺**一条直接观测**：没有 `ingress_drops`、没有循环心跳、没有抓包、没有对端日志，因此没有任何一条证据证明「确实发生了一次 park」。
>
> 本文 §2.3 自己立的标准是「新增测试通过 ≠ 新增机制生效，必须有一条消费该状态的用例 + 一次对照实验」。**同一标准应适用于本章**：结构成立 ≠ 缺陷被证实。定性与施工的风险定价都应改为「最强假设，待 T1 插桩确认」。这不改变结论方向，但决定了「若假设错了，A+B+C 是否白做」。

### 6.1 数据通路拓扑

```
[对端 .16] ──udp──> [tunnel stream] ──> start_recv_loop  (peer_conn.rs:1418)
                                          │
                       ┌──────────────────┼────────────────────┐
                       │ Ping  → 改 Pong → sink.send().await    │ ← 控制面，无超时
                       │ Pong  → broadcast::send                 │ ← 控制面
                       │ 其它  → send_peer_packet_to_chan(...).await
                       │           └─ try_send 失败 → .send().await（无限等待）
                       └──────────────────┼────────────────────┘
                                          ▼
                     PacketRecvChan  mpsc::channel(128)  ← 全局唯一，所有 conn 共享
                                          ▼
                     PeerRecv::run 单任务串行出队 (peer_manager.rs:4974)
                       → foreign network 查询 / ACL / flow_table / 解密解压 / 路由 / NIC 写入
```

### 6.2 三个结构性事实

1. **全局共享的浅队列**：`peers/mod.rs:119-122` 的 `create_packet_recv_chan()` 用 `tokio::sync::mpsc::channel(128)`；`peer_manager.rs:1670` **只创建一次**，然后 `packet_send.clone()` 分发给所有 peer conn（`:1673` PeerMap）。也就是说，一条 conn 的突发可以堵住**所有** conn 的控制包（跨 conn HoL blocking）。
2. **队满退化为无限阻塞**：`peers/mod.rs:124-139`：
   ```rust
   match sender.0.try_send(envelope) {
       Ok(()) => Ok(()),
       Err(TrySendError::Full(envelope)) => sender
           .0
           .send(envelope)      // ← 队满即 park 当前 task
           .await
           .map_err(|error| SendError(error.0.packet)),
       Err(TrySendError::Closed(envelope)) => Err(SendError(envelope.packet)),
   }
   ```
3. **控制面与批量数据共用同一个循环**：`peer_conn.rs:1418-1611` 的 `while let Some(ret) = stream.next().await`（循环体自 `:1521` 起）里，数据走上面的阻塞通道（`:1591-1596`），**Ping 的 Pong 应答也在循环内**（`:1545-1564`），且用 `sink.send(zc_packet).await`（无超时，不像出向 `send_msg` 有 `EGRESS_WAIT=100ms` 保护）。

**容量估算**：90MB/s ÷ 1280B/包 ≈ **7.0 万包/s**；128 个槽位只够约 **1.8ms**。消费端 `PeerRecv::run` 是单任务串行，任何一次 `await`（foreign network 查询、ACL、flow accounting、解密、路由查找、写 Wintun）停顿超过 2ms，队列就会被填满。Windows 上调度抖动到 2ms 完全正常——这解释了为什么崩溃是概率性的、且与并发数不单调相关。

> **勘误 §12.2-#5（这个估算的适用边界要说清，否则会被误读成「低带宽安全」）**：
> **队列时间常数 = 队列深度 ÷ pps，与 bps 无关。** 上式的 1.8ms 只在「大包 + 高带宽」时成立：
>
> | 场景 | pps | 128 槽的缓冲 |
> |---|---|---|
> | 1280B @ 90MB/s（`.16`） | ≈70 000 | **1.8ms** ← 出事点 |
> | 1280B @ 1.25MB/s（10Mb 大包） | ≈976 | 131ms（70× 安全） |
> | **64B @ 1.25MB/s（10Mb 小包：VoIP/RDP/小包业务）** | ≈20 000 | **6.4ms** ← 又危险 |
> | 64B @ 12.5MB/s（100Mb 小包） | ≈195 000 | **0.65ms** ← 比 `.16` 更糟 |
>
> 所以「10Mb 公网不会踩到」是错的：真正决定风险的是**包长分布与聚合 pps**，不是签约带宽。另外 128 槽是**全局唯一、所有 conn 共享**（`peer_manager.rs:1670`），网关/多分支节点必须按**聚合 pps** 估算（10 条 10Mb 分支 = 100Mb 聚合 + 数万 pps，直接回到 `.16` 量级）。

### 6.3 因果链（时序推演）

| 步 | 发生了什么 |
|---|---|
| 1 | 持续高 pps 入向 + 消费端一次 >2ms 停顿 ⇒ 128 槽队列打满 |
| 2 | 某条 conn 的 `send_peer_packet_to_chan` 进入 `.send().await`，**`start_recv_loop` 整体 park** |
| 3 | 该循环不再调用 `stream.next()` ⇒ 底层 socket/隧道缓冲溢出，内核开始丢包；同时 `rx_bytes` 停止增长 |
| 4 | 循环里的 Ping 也读不到 ⇒ **不回 Pong**；即使读到，出口 `sink.send().await` 也可能继续 park |
| 5 | 对端收不到我们的 Pong ⇒ 对端判我们死；我们同样收不到对端 Pong ⇒ **双向互相判死** |
| 6 | 快速探测 3 轮 `Miss` → 发布 degraded（约 1s）；9 轮 → 本地 SPF 剔边（约 4s）；保活累计 13 次失败 → 约 24s 后拆连重建 |
| 7 | 重建 / 队列自然排空 ⇒ 循环恢复 ⇒ 数据面自愈（有时早于拆连，见 §5.2） |

**这不是「拥塞被误判」，而是「拥塞造成了真实的、双向的、持续的控制面失联」**。业务流全部同时停摆、ICMP 全黑，是数据面真的不转发了，不是告警谎报。

### 6.4 为什么本轮所有修复都救不了

| 机制 | 为什么无效 |
|---|---|
| `RoundOutcome::DeliveryOnly` / `carried_data` | 需要 `rx_delta > 0`；停摆时收包循环根本不读 socket，计数器不涨 ⇒ 3 轮干净 `Miss` |
| 预算自适应 / `budget_floor` 翻倍 | 只在 `DeliveryOnly` 上翻倍，`Miss` 反而减半，最终钉死 300ms；队列长度不是探测预算能覆盖的问题 |
| `DeliveryExcuse` 棘轮 | 同上，没有任何入向字节来触发豁免 |
| `egress_fq` 控制包优先道 | 只作用于**出向** `send_msg`（`EGRESS_PRIORITY_CAP=32`），管不到**入向**共享 128 通道；日志 `egress_drops=0` 佐证 |
| `tx_delta` / `egress_drops` 诊断字段 | 设计上就不参与判定（§2.5），只用于把根因范围缩小；本次确实靠它们排除了出口饱和 |
| 拆连重建 | 是降级逻辑的**结果**，不是恢复原因（§5.2） |

一句话：**这些修复都在优化「如何判断一条链路是否还活着」，而缺陷在于「判断所依赖的控制包自己被数据挤死了」。判定层再精细也拿不到证据。**

### 6.5 为什么 `.201` 不复现（未验证假设，不能写成结论）

| 假设 | 支持 / 反证 | 验证方法 |
|---|---|---|
| 隧道类型差异：`.16` 是 udp p2p，`.201` 是 wg | `.16` 已由日志确证 udp；`.201` 的隧道类型需从 `new peer connection added` 复核 | 在同一台 `.201` 上用 `httpload.py` 12 流压测 |
| 业务形态差异：SMB（TCP 自带流控 + `NO_BUFFERING`）vs HTTP keep-alive 小文件（98 757 B，高频小突发） | 两者峰值吞吐接近（113.87 vs 100.22 MB/s），但包到达的突发性可能不同 | `.201` 跑 HTTP、`.16` 跑 SMB，交叉对照 |
| 对端瓶颈位置：`.201` 磁盘顺序读 ~87–120MB/s 是天花板，实际入向 pps 被服务端限速 | 与「崩溃需持续负载」一致 | 在 `.201` 上用内存盘 / 大文件消除磁盘瓶颈后重压 |
| 对端 ET 平台 / CPU / socket 缓冲差异 | 无数据 | 远程采两端 ET 进程的 CPU 与 socket 丢包计数 |

**当前不能排除数据路径差异**，因此 §8 的止血方案必须同时覆盖所有隧道类型，不能只针对 udp 打补丁。

---

## 7. 严重度与影响评估

| 维度 | 评估 |
|---|---|
| 触发条件 | 持续 **≈70k pps** 入向负载（1280B@90MB/s；小包场景几万 pps 即可触发，64B@100Mb ≈195k pps）+ 消费端一次 >2ms 停顿；概率性，与并发数不单调相关。决定量是**聚合 pps 而非 bps**，见 §12.2-#5 |
| 影响面 | **整条隧道全黑**（不只被打的那条流），同机上其他 conn 的控制包也会被全局 128 队列连坐；本机所有 peer 的 liveness 可能一起被误判 |
| 用户感知 | 业务请求 7–15 秒级卡顿 / 超时、RDP 类长连接假死、ICMP 全丢；随后自愈，极易被当成「网络抖动」而漏报 |
| 可恢复性 | 自愈，但代价是 SPF 剔边 + 拆连重建（约 24s 窗口），期间可能触发路由震荡 |
| 数据正确性 | 未发现数据损坏；SMB 25 992 块 0 错误 |
| 企业级差距 | ① 控制面与数据面未隔离；② 热路径存在无界 `await`；③ 入向丢弃不可观测（无计数、无告警）；④ 单点浅队列造成跨 conn HoL blocking；⑤ 故障自愈依赖拆连这种重动作 |

> **勘误 §12.2-#6（影响面少画了一条边）**：本章的因果链只画了「sent → 128 队列 → 消费端」这张单程图，缺一条**回边**：`PeerRecv::run` 在处理 foreign network 包时是**内联 `await` 出向发送**的（`peer_manager.rs:5236-5290` 的 `forward_foreign_network_packet` 与 `peer_map.send_msg`，后者最长 `EGRESS_WAIT=100ms`）。于是还存在第二个环：
>
> ```
> 入向消费端 await 出向 → 出向队列满/慢（≤100ms）→ 消费端停
>   → 入向 128 队列满 → recv loop park → 更堵
> ```
>
> `.16` 是单网络、无 foreign 流量，这个环不触发（不影响 §5 的结论）；但**relay / 多网络 / 网关型部署会触发**。这决定了 A 方案的验收不能只测 `.16`（见 T6）。同理，`get_origin_my_peer_id` 内部的 `self.routes.read().await`（`peer_map.rs:539`）在 OSPF 收敛期是隐藏争用点。
>
> 另外**低带宽场景有一条「症状相同、根因相反」的缺陷**：满载时 Ping 走 `sink.try_send`（`peer_conn_ping.rs:704`）会拿到 `BufferFull` → `LocalCongestion`，若持续满 30s，`LOCAL_CONGESTION_CLOSE_AFTER`（`:87`/`:854-862`）会主动拆连并打 `pingpong egress full for too long, closing the connection`。用户看到的还是「掉线一下又好了」，但**这里堵的是出向，不是入向**。详见 §12.2-#7。

对照企业级标准，前四项必须在架构层解决，不能靠调参。

---

## 8. 修复方案（供决策，尚未实施）

> **✅ 2026-11-09 16:40：实际实施的修复不是本章的方案 A/B，而是一处约 15 行的改动**，见 **§13.3**。
> 根因由「入向自锁」更正为「出向控制面饥饿」后，本章方案的取舍随之改变：
> - **方案 A（入向 tail-drop）**：针对的是被证伪的根因，**不实施**；
> - **方案 B（控制面独立通道）**：方向正确，但实测证明**不需要新通道**——保活在队满时有界等待 50ms 即可，实测 `enq_blocked_max_ms=5`（数据面等待上限），代价可忽略。作为后续优化保留，优先级从 P0 降为 P2。

### 8.1 设计原则

1. **控制面与批量数据物理隔离**：Pong / Ping / 路由 RPC 永远不排在数据后面。
2. **收包循环无阻塞**：`start_recv_loop` 的任何分支都不得因下游容量而 `await`；它必须持续 `stream.next()`。
3. **丢弃可观测**：任何主动丢弃都要计数（`ingress_drops`）、进日志、能告警；宁可丢包，不可停摆。
4. **检测语义不退化**：真黑洞 3 轮 degraded（≈1.9s）、9 轮剔边（≈5.7s）、单向故障 ≈4.7s 三条时间常数不得因修复而变长。
5. **可灰度 / 可回滚**：新行为置于已有配置项或新增开关之后，默认开启前可 A/B。

### 8.2 方案 A（P0，最小止血）：收包循环去阻塞

> **勘误 §12.2-#9（三处实现细节需要更新，否则照抄会编译不过 / 会踩坑）**：
> 1. 下面的代码示意**参数已经是对的**（`ingress: PeerPacketIngress` 与现签名一致），**唯一要改的是返回类型**：`SendError<ZCPacket>` **无法区分 `Full`(该丢) 与 `Closed`(该 break)**，所以 A1 不能照抄该签名，需要改成三态返回（如 `Sent / Dropped / Closed`）。文档只提到了 `is_err() => break` 这一个陷阱，这是第二个更隐蔽的。
> 2. A2 无需「确认 `MpscTunnelSender` 的 API」：`try_send` **已存在**（`tunnel/mpsc.rs:40-44`，返回 `TunnelError::BufferFull` / `Shutdown`），A2 是 3~5 行改动。因此 **A1 与 A2 应同批落地**——只修 A1 会留下「偶发停摆但 `ingress_drops=0`」的极难查残留（出向 sink 满同样会 park 同一个循环）。
> 3. A3 的计数器**不要放 `Throughput`**（那是 per-conn 语义，而被丢的包尚未归属任何 conn）。应放 `MetricName::DataPlane*` 计数器族，与 `DataPlaneNicChannelDrops` 并列。而且代码库已有**现成范式 + 现成测试范式**可直接照抄：`NicPacketProcessor`（`peer_manager.rs:1099-1126`，`try_send` + `inc()` + `debug!`）与 `full_host_packet_queue_does_not_block_peer_packet_processing`（`peer_manager_tests.rs:106`）。

**A1 — 数据投递改 tail-drop**：为热路径新增 `try_send_peer_packet_to_chan`，`Full` 时丢弃并计数，**不再 `.send().await`**。等价路由器 tail-drop：拥塞时丢在用户态、丢得可控，而不是把整条收包循环挂起。

```rust
// peers/mod.rs（示意，未落地）
pub(crate) fn try_send_peer_packet_to_chan(
    sender: &PacketRecvChan, packet: ZCPacket, ingress: PeerPacketIngress,
) -> Result<(), SendError<ZCPacket>> {
    let envelope = PeerPacketEnvelope::from_peer(packet, ingress);
    match sender.0.try_send(envelope) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(e)) => { /* ingress_drops += 1; 不 break 循环 */ Err(SendError(e.packet)) }
        Err(TrySendError::Closed(e)) => Err(SendError(e.packet)),
    }
}
```

> **实施陷阱**：`peer_conn.rs:1591-1596` 现有代码是 `if send_peer_packet_to_chan(...).await.is_err() { break; }`。改成 tail-drop 后**必须把「丢弃」和「通道关闭」区分开**——`Closed` 才 `break`，`Full` 只计数并继续循环。照抄现有 `is_err() => break` 会把 tail-drop 变成「一丢包就断连」，比原缺陷更糟。

**A2 — Pong 应答改非阻塞**：Ping→Pong 分支不得 `sink.send().await` 无超时等待。用 `try_send`（需确认 `MpscTunnelSender` 的 API）或 `timeout(EGRESS_WAIT, sink.send(...))`，超时/满则丢弃 + 计数 + `trace!`，循环继续。丢一个 Pong 的代价远小于停摆：Pong 幂等，下一轮探测会补上，且 `DeliveryOnly` / 3 轮门槛本来就容忍偶发丢失。

**A3 — 计量**：新增 `ingress_drops`（`Throughput` 或独立计数器），在 `ET-cli peer` / 日志中可见；`publishing degraded loss` 的 warn 增加该字段，使 `rx=0, ingress_drops>0` 能直接指明「本机入向队列饱和」。

| 优点 | 风险 |
|---|---|
| 改动集中在 2 个文件，直击根因，覆盖所有隧道类型 | 放弃现有背压：拥塞时 TCP 重传可能放大入向压力，需实测丢包率 |
| 立即消除「控制面被数据挤死」 | 丢 Pong 可能让对端多一次 miss；需验证不触发 3 轮误判 |
| 可与现有 `egress_drops` 形成入/出双向可观测 | 需要新增针对「队满不阻塞」的单测与压测回归 |

### 8.3 方案 B（P1，架构整改）：独立控制面通道

> **勘误 §12.2-#10（B 的定性写偏了，工作量被高估）**：读代码后可以确认——Ping→Pong 的应答**本来就在 `start_recv_loop` 里内联处理**，并不经过数据队列。所以它被卡住的原因不是「排在数据后面」，而是那两件事：① 循环被 park 了（A1 解决）；② 它自己的 `sink.send().await` 无超时会 park（A2 解决）。
>
> 因此 B 的**真正增量**不是「把 Pong 移出收包循环」，而是**让控制包走独立的出向通道**，不被 bulk 数据的出向队列堵住。这个差别直接把施工范围砍掉一大半：
>
> - **不需要动 tunnel 层**；
> - **不需要重新核对 `StatsRecorderTunnelFilter` 的 per-conn 计数前提**（那是出向统计，与入向无关，上文这句可以删）；
> - 只需 per-conn 一条 32 深度控制队列 + 一个 drain task。
>
> 建议把 B 重新定义为 **B-lite：控制面独立出向队列**，改动面从「中-大」降到「小-中」，工期约为原估的 1/3。它应当排在 A 之后、由 A 的实测数据决定是否真的必要。

把 Ping/Pong 完全移出 `start_recv_loop` 的数据通路：在 tunnel 层直接应答 Ping，或为控制包提供一条独立的高优先级 `PacketRecvChan`（独立容量、独立消费任务）。数据面拥塞在任何情况下都不影响 liveness 证据的收发。

- 语义最干净，彻底消除「共用循环」；
- 改动面较大，涉及 `peer_conn.rs`、`peer_manager.rs`、tunnel 层；需重新核对 `StatsRecorderTunnelFilter` 每 conn 独立计数器的前提（`peer_conn.rs:387`）。

### 8.4 方案 C（P1/P2）：消除跨 conn HoL blocking

全局 128 通道按 conn 分片（每 conn 独立队列 + 消费端多 worker 或加权公平出队），使一条 saturating conn 无法饿死其他 conn 的控制包。同时把容量调整为按 pps 推导（例如目标 100MB/s × 10ms 缓冲 ≈ 700 槽），而非固定 128。

> **单独加大 128 不是修复**：只是把触发阈值从 ~1.8ms 推到 ~10ms， Windows 调度抖动仍可越过，且放大 bufferbloat。**只能作为 C 的配套，不能单独上线。**

### 8.5 方案对比与推荐

| 方案 | 止血速度 | 改动面 | 根治程度 | 回归风险 |
|---|---|---|---|---|
| A（tail-drop + 非阻塞 Pong + 计量） | 快 | 小（2 文件） | 消除自锁，队列共享仍在 | 低-中（需实测丢包率） |
| B（独立控制面） | 中 | 中-大 | 根治控制面隔离 | 中 |
| C（分片 + 容量推导） | 慢 | 大 | 根治跨 conn HoL | 中 |

**推荐路径**：**先 A 止血并打包验证 → 再按 B + C 做架构整改**。A 之后即使 A 的丢包率不理想，也已把「隧道全黑 26 秒」降级为「可控的少量丢包」，风险量级下降一个数量级；B/C 再解决公平性与容量。

### 8.6 明确不建议的做法

| 做法 | 为什么不行 |
|---|---|
| 放宽 `FAST_PROBE_TIMEOUT_MIN` / `MISSES_TO_DEGRADE` / `fail_close_count` | 掩盖症状，真黑洞检测时间从 ≈1.9s 拉长到十几秒，且数据面仍然全黑 |
| 关闭快速探测或停止剔边 | 放弃 liveness 能力，回到 §2.3 之前的黑洞问题 |
| 用 `tx_delta > 0` 判活来「覆盖转发场景」 | 黑洞时 tx 照涨 ⇒ 永不判死（§2.5） |
| 仅把 `channel(128)` 调大 | 阈值游戏，不消除无界 `await`，且加重 bufferbloat |
| 靠拆连重建加速恢复 | 拆连是结果不是原因（§5.2），缩短只会制造更频繁的路由震荡 |

> **勘误 §12.2-#12（还要再否掉一个「看起来更便宜」的做法）**：「**加大 `SO_RCVBUF`** 代替 A1」同样不成立——它只是把内核侧的溢出点往后推几十毫秒，`start_recv_loop` 里的无界 `await` 一点没动，而且加重 bufferbloat。结论与「仅把 `channel(128)` 调大」同理：**只能作为 A 之后的观测项，不能作为修复**。

---

## 9. 验收方案（供「打包验证」使用）

### 9.1 复现脚本

```bash
# 1) httpload.py 只接受一个位置参数 outdir；目标硬编码在 HTTP_TARGET
#    (httpload.py:18 = 10.10.10.16:80/iisstart.png)，阶段计划硬编码在文件末尾 plan 列表
#    现有 plan：idle 30s → h4/45s → h12/60s → s12/60s → mix/60s
#    要跑「3 × 120s × 12 流」需把 plan 改成：
#    [("h12a", [("http", 12)], 120), ("h12b", [("http", 12)], 120),
#     ("h12c", [("http", 12)], 120)]
python target/tmp/etmon6/httpload.py run11
```

```powershell
# 2) 同期双路 ping（执行策略禁止直接跑 .ps1，用 scriptblock）
$sb = [scriptblock]::Create((Get-Content -Raw 'D:\EasyTier\target\tmp\etmon6\dualping.ps1'))
& $sb -DurationSec 480 -OutDir 'D:\EasyTier\target\tmp\etmon6\run11'
```

```bash
# 3) 同期采本机网卡计数器与主日志事件
#    nic_probe.ps1（同样用 scriptblock 调用）；日志直接 grep 主日志：
#    grep -E "publishing degraded loss|degradation sustained|too many consecutive" easytier.log
```

### 9.2 通过判据（P0 止血）

> **勘误 §12.2-#11（下面第 3、4 行在 A 落地后会互相冲突，需要先定量）**：A 的语义就是「拥塞时 tail-drop」，所以**吞吐必然下降、`ingress_drops` 必然增长**。而第 3 行要求「≥85MB/s 不得显著退化」、第 5 行又允许「`ingress_drops` 增长」——如果消费端能力不足（见 §4.4 与 §12.2-#5），这两条会同时不满足，验收就会卡成主观判断。**上线前先定死数字**，例如：`ingress_drops` < 0.1%、吞吐 ≥ 80MB/s、p99 ≤ 40ms。
>
> 另外本表缺两条判据，建议补入：
> - **tcp 隧道**（`.10`）：tail-drop 在 TCP 承载上会引发内层重传放大，必须单独验证不恶化（T3）；
> - **对端 `misses` 曲线**：A2 生效后仍可能丢 Pong，需确认对端 `misses` 稳定 ≤1、不触发 3 轮 degraded。

| 项 | 判据 |
|---|---|
| 稳定性 | 3 轮 × 120s × 12 流，无连续 >2s 全黑、业务无 >2s 空档；ICMP 丢包率 **< 0.1%**（**不写「0 丢包」**：A 的语义就是拥塞时 tail-drop，ICMP 走隧道必然可能被丢，见 §12.2-#11） |
| 控制面 | 压测窗口内 **0 条** `publishing degraded loss` / `degradation sustained` / `too many consecutive pingpong failures` / `pricing our edges` |
| 吞吐 | ≥ 85MB/s（基线 h12a 85.67、run9 88.65），不得因 tail-drop 显著退化 |
| 延迟 | p99 ≤ 40ms，`max` 不出现 >2s 长尾 |
| 可观测 | 若发生拥塞，`ingress_drops > 0` 且日志可见；**允许丢包计数增长，但不得伴随隧道全黑** |

### 9.3 回归判据（不得退化）

| 项 | 判据 |
|---|---|
| `.201` 对照组 | 12 流 SMB ≥ 105MB/s，25 000+ 块 0 错误，ping 0 丢包，0 探测事件 |
| 真黑洞检测 | 3 轮发布 degraded（≈1.9s）、9 轮本地剔边（≈5.7s）不变 |
| 单向故障 | ≈4.7s 发布降级（预算棘轮 + `DeliveryExcuse` 仍生效） |
| 拥塞不误报 | 663ms 队列场景 30 轮零误报（`a_663ms_queue_never_publishes_loss_under_the_adaptive_floor`） |
| 单元测试 | `cargo test -p easytier-core --lib peer_conn_ping` 25 passed + 新增「队满不阻塞循环」用例 |
| 静态门禁 | `cargo fmt --all -- --check`、`cargo check -p easytier-core --lib`、`cargo clippy -p easytier-core -- -D warnings` 全绿；完整 clippy 交 CI |

---

## 10. 遗留问题、待验证假设与口径修正

### 10.1 仍未处理

1. **观测层（显示问题，非数据面）**：`ET-cli peer` 的 `lat` 会冻结在最后一次 pong 值；`loss` 滑动窗口衰减极慢（`.20` 连续 55 个采样点恒 1.0%）。这两项会让人误读为「持续抖动」，本轮未改。
2. **L1/L2 现网未触发**：本机 LAN `192.168.10.0/24` 与现有 proxy CIDR（172.16/16、172.18.10.10/32、152.65.x/32、172.25.0.1/32）无重叠，日志中 underlay exclude / prune 记录均为 0。**只证明无退化，不证明剪枝正确**。需推送覆盖性 `/22` 后验收，以及验证条件消失后 `/32` exclude 拆净。
3. **真实拥塞下 `DeliveryOnly` 路径从未被触发**：`.201` 太干净，探测从未超时；`.10` 新版也全靠 `egress_fq` 优先道。该分支目前只有独立 crate / 单测覆盖，没有现网证据。可用 `tc netem delay` 或人工打满上行补测。
4. **`.201` 与 `.16` 差异未归因**（§6.5），需交叉压测。

> **勘误 §12.2-#7（本节要补两条本次新发现的缺陷 / 边界）**：
> 1. **低带宽满载 30s 会拆连（根因在出向，与 §6 不同）**：满载时 Ping 走 `sink.try_send`（`peer_conn_ping.rs:704`）拿到 `BufferFull` → `LocalCongestion`；若持续满 `LOCAL_CONGESTION_CLOSE_AFTER = 30s`（`:87`，判定在 `:854-862`），会主动拆连并打 `pingpong egress full for too long, closing the connection`。**用户感知与 `.16` 一样（掉线一下又好了），但这里堵的是出向**（→ T5）。不要把它和 §6 的入向自锁混为一谈。
> 2. **内核丢包判据**：`netstat -s -p udp` 的 `RcvbufErrors` 增长且与全黑窗口逐秒对齐 ⇒ 本机自锁的**正**证据（见 #3）。

### 10.2 口径修正（覆盖早期记录，以本文为准）

| 项 | 早期记录 | 修正后 |
|---|---|---|
| 单向故障检测时间 | 3.2s → 6.2s | **≈4.7s**（`Miss` 也把 `budget_floor` 减半，第 4/5 轮预算是 600/300ms 而非 1200ms：0.63+0.93+1.53+0.93+0.63）。「最坏 ≈1.9s」只对全黑洞成立；「全黑洞紧跟拥塞 ≈3.1s」 |
| `cargo check` 可用性 | 不可用（`ring` 缺 MSVC include） | **可用**，`build-common.ps1` 自动 `call vcvars64`；实测 exit 0，25 个单测通过 |
| run10 连续全黑时长 | 19 秒 / 19-of-30 丢包 | 按 `dual.csv` 原始记录复算：**27 次连续 TimedOut，01:28:05.925–01:28:31.644，约 25.7 秒**；全窗口 28/908 超时。早期数字来自较窄窗口统计 |
| 01:21 恢复 vs 拆连 | 早 9 秒 | 业务数据 `01:21:28.510` 恢复、拆连 `01:21:39.471`，**早约 11 秒** |
| 日志字段名 | `budget_floor_ms` | `next_budget_floor_ms`（打印时已是下一轮的 floor，与本轮 `budget_ms` 同名易误读） |
| `cargo fmt --check` 曾记录「通过」 | 通过 | 当时实际**不干净**（6 处），已格式化修好；现在干净 |

### 10.3 已证伪的假设（避免重复排查）

- 「.10/.20 的丢包是 EasyTier 缺陷」→ 证伪：两台有带宽限制；换 .201 后 108MB/s 零丢包零探测事件。
- 「SMB 1883MB/s 说明链路极快」→ 证伪：Windows SMB client cache 假象。
- 「.201 的 913ms 毛刺是隧道抖动」→ 证伪：远程磁盘队列逐秒对齐，同期 ping 反降 0ms。
- 「探测把拥塞误报成丢包是本次停摆的原因」→ 证伪：本次是真实数据面停摆，`rx_delta=0`，业务全停、ICMP 全黑。
- 「本机出口 `egress_fq` 打满导致停摆」→ 证伪：`egress_drops=0`，且 fq 只管出向。
- 「拆连重建让链路恢复」→ 证伪：恢复早于拆连约 11 秒。
- 「`.16` 停摆是触发了带宽上限」→ 证伪（§5.5）：10Gbps 网卡峰值仅用 0.97Gbps、物理 ICMP 908/908 全通 0–3ms、网卡 discard/error 全 0、ET 限速器未启用且机制上不会丢控制包、`.201` 在 113.87MB/s 同量级速率下零异常。**高 pps 是把 128 槽队列压满的必要压力，不是根因**。

---

## 11. 结论

1. `0473c945` 及其后续修复**符合企业级标准**：阻塞项与 P0/P1/P2 全部闭环，探测判定从布尔合并重构为四值 + 预算棘轮 + 单向故障闩锁，告警治理收敛，门禁（除完整 clippy 的本机原生构建环境问题外）全绿。评审中发现的「棘轮收了参数没读」是一次典型的「测试通过但机制未生效」，已用对照实验修正并固化为经验。
2. `carried_data` 对 relay 连接恒 false 是**真实存在的观测盲区**，但**不能用 `tx` 修补判定**；已落地 `egress_drops` 等纯诊断字段，且本次故障定位正是靠这些字段排除出口饱和，证明该决策正确。
3. 当前**唯一未修的 P0 缺陷**是入向共享 `channel(128)` + 队满 `.send().await` + 控制面与数据面共用收包循环造成的**数据流自锁 → 控制面连带死亡 → 双向误判 → 拆连**。它与本轮所有改动无关，是既有架构债；在高 pps、无带宽限制的 LAN 链路上可稳定概率性复现。
4. 建议按 §8.5 先做方案 A 止血、再按 §9 打包验证，之后推进 B/C 架构整改。**在 A 落地前，不应把「.16 停摆」记为已解决，也不应通过放宽探测参数来掩盖。**

> **2026-11-09 补充（勘误 §12.2-#4 / #13）**：第 3 条的「根因」应读作**最强假设**——因果模型与排除法都支持它，但尚缺一次直接观测（T1），因此在假设被证实之前不宜按单根因全面施工。第 4 条的执行顺序相应调整为：**T1 取证 →（A1+A2+A3 同批）→ 三处验收 → 消费端吞吐整改 → B-lite → 交叉归因**。完整清单见 §12.1。

> **✅ 2026-11-09 16:40 最终结论（覆盖以上全部定性）**
>
> - **真因**：出向 32 槽队列的**控制面饥饿**，不是第 3 条的入向 128 队列自锁。
> - **修复**：`peer_conn_ping.rs::do_pingpong_once` 的 `BufferFull` 分支改为 **50ms 有界等待后重试**，只有等待超时才判 `LocalCongestion`（约 15 行）。
> - **验证**：两端部署后 7 轮压测，**停摆 0 次、请求超时 0 条、`publishing degraded` 0 次、`egress full` 0 次**；150s × 12 流吞吐 **94.12 MB/s**（旧版最好 85.62），`max` 从 3046~9547ms 降到 **1020ms**。
> - **第 3 条中「控制面被数据面挤死」的部分洞察成立**，只是被挤的不是入向队列。详见 §13。

---

## 12. 2026-11-09 追加：待办测试清单与勘误

### 12.0 现状

| 项 | 状态 |
|---|---|
| §6 的根因定性 | **最强假设**（结构已证 + 替代解释已排除，缺一次直接观测）→ 见 §12.2-#4 |
| 插桩改动 | **已落地，未提交**：`peers/conn/peer_conn.rs`（收包循环心跳看门狗 + Pong 阻塞计时 + `stall_probe=true` 启动标记）、`peers/mod.rs`（`send_peer_packet_to_chan` 的 `Full` 分支 park 计时）。**行为零变更**（不改队列深度 / `send` 语义 / 探测阈值 / 任何超时） |
| 门禁 | `cargo check -p easytier-core --lib` exit 0；`cargo clippy -p easytier-core -- -D warnings` exit 0；`cargo fmt --all -- --check` 干净；`cargo test -p easytier-core --lib peer_conn_ping` **25 passed / 0 failed** |
| 取证脚本 | `target/tmp/stallprobe/probe_sample.ps1`（1 Hz 采 NIC 计数 + 内核 UDP 计数 + tail 主日志抓 probe 行）、`target/tmp/stallprobe/README.md`（操作与判据） |
| 下一个动作 | **T1**，不是改方案 |
| 明确不做 | 放宽探测阈值 / 单独加大 `channel(128)` / 只做 B+C 全量 / 再跑一轮无插桩压测 |

**为什么先测不改**：评审对象（`0473c945` + `a15e51d2`）是干净的；暴露出来的缺陷是既有架构债，其因果模型已由排除法支撑，但**没有一条证据证明「确实发生了一次 park」**。若假设方向错（例如实为对端自锁或上游丢弃），A+B+C 全部白做；而补这一环的成本是「半天插桩 + 一次定向复现」。

---

### 12.1 待办测试清单（T1~T9）

#### 12.1.1 T1（P0，已就绪）——插桩三分：本机自锁 / 对端不发 / 上游丢弃

| 项 | 内容 |
|---|---|
| 目的 | 闭合 §6 缺的那一环，决定后续修哪里 |
| 前置 | 用 `release-fast` 编译（**不能用 debug**：dev profile 无优化，单包路径慢 5–20×，会造出「队列必然满」的假象，或压不到 90MB/s 造出假阴性；且 `easytier-windows-debug` 是 `pnpm tauri dev`，不产出可分发件） |
| 自检 | 启动后日志出现 `start recving peer conn packet  stall_probe=true`，确认新包在跑 |
| 方法 | 三窗口：`probe_sample.ps1`（先起）→ `dualping.ps1` → `httpload.py run11`；**一次复现即足够** |
| 产物 | `target/tmp/stallprobe/run1/{net.csv, netstat_raw.log, probe_events.log}` + `target/tmp/etmon6/run11/dual.csv` + 该时段主日志 |

判定表：

| 观察组合 | 结论 | 后续 |
|---|---|---|
| `ingress channel full, recv loop parked` + `busy recv loop stopped iterating` + dualping 隧道同时超时 | **本机入向自锁坐实** | 立刻进 T7 前的 A1+A2 实施 |
| 只有 `pong reply blocked on a full tunnel sink` | 卡在 **Pong 出向**，A2 升为第一优先 | 同上，但优先级对调 |
| `busy recv loop stopped iterating`，但**无** `parked` 行 | 循环停在 `stream.next()`：**对端不发 / 上游丢弃** | 转 T2；此时只修本机无效 |
| 上述皆无，而 `netstat_raw.log` 的 UDP 计数在涨 | 包到了网卡、没到 ET（内核层丢） | 同时查 `SO_RCVBUF`；A1 仍要做 |
| `STALL PROBE` 出现但 dualping 全通、无 degraded | **阶段结束的假阳性**（上一秒还在 60k pps，本秒归零） | 忽略，不计为停摆 |

> 噪声说明：看门狗只在「bulk 忙态（≥1000 pkt/s）后连续 2s 零迭代」时报警，轻流量（RDP/Ctrl+C）不触发。真停摆与阶段结束假阳性的区分点：真停摆同时伴随 dualping 隧道连续超时 + 约 1s 后 `publishing degraded loss`。

#### 12.1.2 其余测试

| ID | 优先级 | 目的 | 方法 | 判据 | 状态 |
|---|---|---|---|---|---|
| **T2** | P0（条件） | 若 T1 判定「非本机」，在对端取证 | 把同一份 `release-fast` 包装到 `.16`，重复 T1 | 出现 `parked` ⇒ 对端自锁，两端都需升级 | 依赖 T1 结果 |
| **T3** | P0 | tail-drop 在 **TCP 承载**上是否被内层重传放大成正反馈 | `.10`（tcp p2p）压测，A 落地前后各一轮 | 丢包不随内层重传上升；吞吐不劣于基线 | 未开始 |
| **T4** | P1 | 低带宽**小包**是否仍会自锁（§12.2-#5） | 合成 64B × ~20k pps（≈10Mb）流量 | 无 `parked` / `stopped iterating`；若已落 A3 则 `ingress_drops` 稳定 | 未开始 |
| **T5** | P1 | 低带宽长期满载是否触发「30s 拆连」（§12.2-#7） | 10Mb 满载持续 >60s | 无 `pingpong egress full for too long` | 未开始 |
| **T6** | P1 | **跨方向自锁环**：入向消费端内联 `await` 出向（§12.2-#6） | relay / 多网络拓扑，foreign network 流量 | 不出现「消费端停 → 入向满」的级联 | 未开始 |
| **T7** | P0（A 后） | 止血验收 | `.16` / `.10` / `.201` 三处跑 §9.2 | 见 §9.2 + §12.2-#11 的量化上限 | 待 A 落地 |
| **T8** | P2 | 关掉 §6.5 的未归因（`.201` vs `.16`） | `.201` 跑 HTTP 小文件、`.16` 跑 SMB（可行时）交叉对照 | 四种组合中只要一种在 A 后仍停摆 ⇒ 存在第二机制 | 未开始 |
| **T9** | P2 | 修正 §4.4 的速率口径（§12.2-#2） | 用 `blk_*.csv` 逐块时间戳重算 1s 粒度瞬时速率 | 给出「崩前 1s 瞬时 MB/s」列，重新表述「非单调」 | **可立即做（纯离线）** |

> **测试纪律**（避免重复踩坑）：① 不再跑无插桩压测——同构证据已足够；② 记录**首个** `STALL PROBE` 行的时间戳，它是与 `dual.csv`、`net.csv` 对齐的锚点；③ 停摆窗口内保留 `netstat_raw.log`（中文本地化的计数器名在人工判读时是唯一可信来源）。

---

### 12.2 勘误与需要更新的表述（13 条）

**#1 §1.4 与 Status「约束」已过期。** 原文「跳过打包、跳过编译、不 push」与现状矛盾。应改为：约束已于 2026-11-09 解除，已落地纯插桩 3 处并跑真门禁（`cargo check` / `clippy -D warnings` / `fmt --check` / 25 单测全绿），仍未提交。

**#2 §4.4 表格的速率口径会削弱本表最关键的论证。** `MB/s` = 「窗口内成功字节 ÷ 实测时长」，而窗口含停摆（h10 的 36.50MB/s 里有 7 秒全黑）。用含故障的窗口平均速率论证「崩溃与速率不单调」是循环论证——平均速率被故障本身压低了。方向性结论成立，但须改用 1s 粒度瞬时速率重算并补「崩前 1s 瞬时 MB/s」列（→ **T9**）。

**#3 §5.5 的「0.97Gbps 贴边」疑虑可消解，但引出更有价值的线索。**
- 0.97Gbps ≈ 121MB/s 线上；`mix` 阶段 114.89MB/s 载荷 × 1.052 ≈ 0.966Gbps ⇒ **0.97 是载荷要求的结果，没有钳位特征**。
- 真正的异常是同一窗口的 **≈15% 缺口**（NIC rx 121MB/s vs `.16` 载荷 105MB/s 线上）：帧到了网卡却没被应用读到，**首选解释是内核 UDP 收包缓冲溢出，即 park 的预期副产品**。这支持 §6 而不是带宽封顶。
- 应把 `netstat -s -p udp` 采样列入 T1（比 `pktmon` 便宜、可逐秒对齐）。

**#4 §6 的定性从「根因已定位」降为「最强假设」。** 理由是本文 §2.3 自定的标准：结构成立 ≠ 缺陷被证实，必须有消费该状态的用例 + 对照实验。本章有排除法但无直接观测（无 `ingress_drops`、无心跳、无抓包、无对端日志）。建议把标题改为「根因假设与代码级证据链」，并在开头注明「待 T1 确认」。这不改变方向，但改变「假设错时 A/B/C 是否白做」的风险定价。

**#5 §6.2 的容量估算必须补 pps 边界，否则会被读成「低带宽安全」。** 时间常数 = 深度 ÷ **pps**，与 bps 无关：1280B@90MB/s = 1.8ms；1280B@10Mb = 131ms；**64B@10Mb = 6.4ms**；**64B@100Mb = 0.65ms（比 `.16` 更糟）**。同时 128 槽是全局共享（`peer_manager.rs:1670`），网关/多分支必须按**聚合 pps** 估算（10×10Mb 分支 = 100Mb + 数万 pps）。

**#6 §7 的影响面少画了一条边：跨方向自锁环。** `PeerRecv::run` 处理 foreign network 包时**内联 `await` 出向发送**（`peer_manager.rs:5236-5290`：`forward_foreign_network_packet` / `peer_map.send_msg`，后者最长 `EGRESS_WAIT=100ms`），于是存在「入向消费端等出向 → 出向满 → 消费端停 → 入向 128 满 → recv loop park」的第二个环。`.16` 不触发，但 relay/多网络/网关型部署会（→ **T6**）。另 `get_origin_my_peer_id` 内的 `routes.read().await`（`peer_map.rs:539`）在 OSPF 收敛期是隐藏争用点。

**#7 §10.1 应补两条本次新发现的缺陷/边界。**
1. **低带宽满载 30s 会拆连**：Ping 走 `sink.try_send`（`peer_conn_ping.rs:704`），满载拿到 `BufferFull` → `LocalCongestion`；`LOCAL_CONGESTION_CLOSE_AFTER = 30s`（`:87`, 判定在 `:854-862`）会主动拆连并打 `pingpong egress full for too long, closing the connection`。**症状与 `.16` 相同（掉线重建），但根因在出向**（→ **T5**）。
2. **内核丢包判据**：`RcvbufErrors` 增长且与全黑窗口对齐 ⇒ 本机自锁的正证据（见 #3）。

**#8 §2.5 / §5.4 的「四种组合」少一个边界说明。** `egress_drops` 只统计 fq 路径（`note_buffer_full`）与 `send_msg` 直连路径，而 **Ping 走的是 `sink.try_send`，旁路 fq，其 `BufferFull` 不计入 `egress_drops`**。因此 `rx=0, tx>0, drops=0` 的解读应为「本机 fq/`send_msg` 出口不饱和，且探测自身成功发出」，不能推广为「任何出口都不饱和」。这一点不影响 `.16` 的结论（`tx_delta` 正是探测包自身发出成功），但影响该图例的普适性。

**#9 §8.2 方案 A 的三处实现细节需更新。**
1. 代码示意的**参数已正确**（`send_peer_packet_to_chan(sender, packet, ingress)` 与 `peers/mod.rs:124` 一致），**要改的是返回类型**：`SendError<ZCPacket>` **无法区分 `Full`（该丢）与 `Closed`（该 break）**，A1 必须改三态返回（`Sent/Dropped/Closed`）。原文只提示了 `is_err() => break`，这是第二个更隐蔽的陷阱。
2. A2 无需「确认 `MpscTunnelSender` API」：`try_send` 已存在（`tunnel/mpsc.rs:40-44`）。**A1 与 A2 必须同批**，否则留下「偶发停摆但 `ingress_drops=0`」的残留。
3. A3 的计数器**不要放 `Throughput`**（per-conn 语义，而丢弃的包尚未归属任何 conn），应放 `MetricName::DataPlane*` 族，与 `DataPlaneNicChannelDrops` 并列；实现照抄现成范式 `NicPacketProcessor`（`peer_manager.rs:1099-1126`）与现成测试 `full_host_packet_queue_does_not_block_peer_packet_processing`（`peer_manager_tests.rs:106`）。

**#10 §8.3 方案 B 的定性写偏了，工作量被高估。** Ping→Pong 应答**本来就在 `start_recv_loop` 内联处理**，它被卡是「循环被 park」（A1 解）+「自身 `sink.send` 无超时」（A2 解），不是「排在数据后面」。所以 B 的真正增量是「控制包走独立**出向**通道」，可以砍掉 tunnel 层改动、也**不必**重新核对 `StatsRecorderTunnelFilter` 的 per-conn 计数前提（那是出向统计）。建议重定义为 **B-lite**，改动面「中-大」→「小-中」，工期约 1/3。

**#11 §9.2 的判据在 A 落地后会互相冲突，且缺两条。** A 的语义就是拥塞时 tail-drop ⇒ 吞吐必降、`ingress_drops` 必增；而表中同时要求「≥85MB/s 不得显著退化」与「允许 `ingress_drops` 增长」。上线前须定量（建议：`ingress_drops` < 0.1%、吞吐 ≥ 80MB/s、p99 ≤ 40ms）。并补：**tcp 隧道（`.10`）不恶化**、**对端 `misses` 稳定 ≤1**。

**#12 §8.6「不建议」表应补一条。** 「用加大 `SO_RCVBUF` 替代 A1」同样只是把 1.8ms 推迟到几十毫秒，不消除无界 `await`，且加重 bufferbloat——与「仅把 `channel(128)` 调大」同理。它只能作为 A 之后的观测项，不能作为修复。

**#13 §8.5 的推荐顺序需要调整。** 原文「A 止血 → 打包验证 → B+C」应改为：**T1 取证 →（A1+A2+A3 同批）→ 三处验收 → 消费端吞吐整改（批量出队 + 按实测停顿分布定容量）→ B-lite → 交叉归因（T8）**。理由：① 取证先行，避免假设错时整体白做；② 消费端吞吐整改是唯一能同时满足「不停摆」与「不掉吞吐」的改动，不能排到 B/C 之后（否则 §9.2 的吞吐判据过不了）；③ B-lite 的工期比原文估计小得多。

---

## 13. 2026-11-09 16:40 定案：根因、修复与验证

> 本章是本缺陷的**最终结论**，覆盖 §0 / §6 / §8 / §11 的旧定性。全部数据来自两端部署插桩版本后的现网压测，脚本与产物见 §13.7。

### 13.1 一句话

**根因是出向队列的控制面饥饿**：满速下 32 槽出向队列长期 100% 占用，数据面靠 `send().await` 每次 ≤5ms 挤进去，而控制面的 `try_send` **每一轮都失败**；累积 30~40 秒后被判为 `LocalCongestion` 拆连。修法是让保活在队满时**有界等待 50ms** 再重试。

### 13.2 决定性证据

#### 证据 1：循环确实停了，但不是停在投递点

停摆期间 4 次采样（两轮），`iters` 恒定不动，确认收包循环冻结：

| 时刻 (UTC) | `iters`（冻结值） | `last_busy_pps` |
|---|---|---|
| 05:46:00 | 3 406 378 | 63 957 |
| 05:47:24 | 3 965 010 | 81 609 |
| 06:06:24 | 3 605 898 | 33 971 |
| 06:07:46 | 3 476 799 | 82 279 |

但同期 `send_peer_packet_to_chan` 的 `Full` 分支计时（≥20ms 报警）**三轮全部未触发**，Pong 的 `sink.send().await` 计时也**全部未触发** ⇒ 循环不是停在 §6 假设的 `.send().await`。

#### 证据 2：停摆期间根本没有包到达

本机网卡逐秒采样（06:06 那一轮，MB/s）：

```
06:06:22  rx=112.8  06:06:23  rx=112.0
06:06:24  rx=  0.0  ← 归零
...
06:06:41  rx=  0.0
06:06:42  rx= 30.6  ← 恢复
06:06:43  rx= 53.3
```

同期内核 UDP 计数只有约 77~180 包/秒（ICMP/保活），**零错误、零丢弃**。若是「循环卡住 → 套接字缓冲溢出」，`rx` 只会升高，不可能归零。

#### 证据 3：真正的 jam 在对端出向

`.16` 的 `pingpong egress full for too long` 记录（`self=461269021` 即 `.16`）：

| 时刻 | `congested_ms` | 反推拥塞起点 |
|---|---|---|
| 05:47:58.395 | 39999 | 05:47:18.4 |
| 06:06:59.748 | 30000 | 06:06:29.7 |
| 06:08:23.116 | 38000 | 06:07:45.1 |

**全日志 10 次 egress-full 拆连全部在 `.16` 侧，本机 0 次**；且 `.16` 全程 0 次 `publishing degraded loss`——它从不认为链路坏，只知道「我发不出去」。

#### 证据 4：队列是「满载」，不是「卡死」

`.16` 活隧道（到本机 `:60391`）在停摆期间的 `EGRESS PROBE`，19 秒内逐秒：

```
writer_idle_ms     = 0~1        ← 写任务每时每刻都在跑
max_feed_ms        = 1~3        ← 每次 feed 瞬时完成，环从不阻塞
enq_blocked_max_ms = 5          ← 数据面 send() 最多等 5ms 就挤进去了
feeds              = 390 881 → 2 074 896   (+1.68M / 19s ≈ 88k 包/秒 ≈ 112MB/s)
enq_full           = 6 → 60     (≈3 次/秒 ≈ 保活的每一次 try_send 都失败)
```

**这就是根因的精确定性**：`feeds` 8.8 万包/秒 ≈ 1Gb 线速，写路径完全健康；但每一条保活都在 `try_send` 上撞墙。数据面用阻塞 `send()`（肯等），控制面用 `try_send`（不肯等）——队列 100% 占用时，前者每次 5ms 挤进去，后者**必然**饿死。

#### 证据 5：修复在满速下依然成立

部署修复后跑满速（94~101MB/s ≈ 1Gb 的 88%），`data_plane_tunnel_buffer_full` 累计 **19 941**——说明出向队列**照样经常满**（这正是旧版触发 `LocalCongestion` 的条件），但保活靠 50ms 有界等待挤进去了，**不再把「健康的饱和」误判成「链路故障」**。

### 13.3 修复

文件：`easytier-core/src/peers/conn/peer_conn_ping.rs` → `do_pingpong_once`

```rust
// 改前：队列一满就判拥塞（满速下 100% 失败）
Err(TunnelError::BufferFull) => return Ok(PingResponse::LocalCongestion),

// 改后：有界等 50ms，真挤不进去才判拥塞
Err(TunnelError::BufferFull) => {
    const CONTROL_EGRESS_WAIT: Duration = Duration::from_millis(50);
    // try_send 失败会消费掉 req，必须重建
    let req = Self::new_ping_packet(self.my_peer_id, self.peer_id, seq);
    match tokio::time::timeout(CONTROL_EGRESS_WAIT, self.sink.send(req)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Ok(PingResponse::LocalCongestion),
    }
}
```

**语义变化（只有一处）**：`LOCAL_CONGESTION_CLOSE_AFTER`（30s）的「满」从「瞬间满」变成「50ms 都拿不到槽」。阈值本身未改。

**没有改的**：数据面 `send()`、队列容量（32）、入向 128 队列、任何探测阈值、任何超时。**没有启用方案 A（入向 tail-drop），也没有实现方案 B（独立通道）**——实测证明不需要。

**配套插桩**（同一批，行为零变更，用于本次定位与后续观测）：

| 文件 | 内容 |
|---|---|
| `tunnel/egress_probe.rs`（新增） | 出向队列探针：`queued/enqueued/enq_full/enq_blocked/feeds/max_feed_ms/feed_age_ms/writer_idle_ms`；1 Hz 上报；`Weak` 注册表避免死隧道刷屏 |
| `tunnel/mpsc.rs` | `send()`/`try_send()` 埋点；`note_feed_begin()` 标记 feed 在飞 |
| `tunnel/filter.rs` | `INGRESS PROBE`：`inner_ready` / `swallowed` / `passed` 三计数，判定「过滤器是否吞光了包」 |
| `peers/conn/peer_conn.rs` | `STALL PROBE`：收包循环心跳看门狗 |
| `peers/mod.rs` | `send_peer_packet_to_chan` 的 `Full` 分支 park 计时 |

### 13.4 被证伪的假设

| # | 假设 | 证伪依据 |
|---|---|---|
| 1 | **入向共享 `channel(128)` 自锁**（原 §6 根因） | 停摆时 `send_peer_packet_to_chan` 的 `Full` 计时未触发；`ring_full`/内核零丢包；`rx` 归零而非升高 |
| 2 | **`start_recv_loop` 停在 `.send().await`** | 同上；且停摆期间 `iters` 冻结在 `stream.next()` 上，而该分支未被执行 |
| 3 | **对端完全停发** | 部分证伪：`.16` 一直在喂 8.8 万包/秒（见 §13.6 缺陷 B，包为何未到达仍未解） |
| 4 | **加大 128 队列 / 做入向 tail-drop 可解决** | 入向不是瓶颈（`udp_session_ring_full` 在旧版几乎不涨），方案 A 不实施 |
| 5 | **需要控制面独立通道（方案 B）** | 有界等待 50ms 已足够；`enq_blocked_max_ms=5` 说明拿到槽的代价极小 |

### 13.5 验证数据

#### 停摆指标（150s × 12 流，同一目标 `.16`）

| | 吞吐 | 超时 | `STALL PROBE` | `degraded` | 拆连 | `egress full` | max |
|---|---|---|---|---|---|---|---|
| 旧版（4 轮） | 66.6~85.6 | 12~24 | 每次停摆 6 行 | 每轮 4 次 | 每轮 2 次 | 持续 | 3046~9547ms |
| **新版（run4/5）** | 44.3 / 49.2 | **0** | **0** | **0** | **0** | **0** | 1110 / 2176ms |
| **新版（终轮 150s）** | **94.12** | **0** | **0** | **0** | **0** | **0** | **1020ms** |

终轮其它指标：`p50 10.7ms / p90 14.4ms / p99 67.5ms`，`>1s` 仅 1 次（旧版 12~26 次）。**吞吐比旧版最好成绩高 10%**。

#### 为什么 run4/run5 只有 44~49MB/s：测量对象时变，不是退化

同一版本、同样 12 流，连续 7 轮：

| 轮 | 时长 | 吞吐 | p50 | **p90** |
|---|---|---|---|---|
| A | 60s | **11.04** | 95.7ms | 135.1ms |
| B | 60s | 47.98 | 10.7ms | 80.6ms |
| C | 45s | 98.58 | 10.4ms | 12.9ms |
| D | 45s | 98.88 | 10.2ms | 13.1ms |
| E | 45s | 100.71 | 10.6ms | 12.4ms |
| F | 45s | 101.04 | 10.5ms | 12.8ms |
| G | 150s | 94.12 | 10.7ms | 14.4ms |

A 与 C 相隔不到 20 分钟，差 **9 倍**，期间**没有任何代码变化**。判读要点：**p50 几乎不变（10~12ms），差异全在 p90（14ms ↔ 135ms）**。按 Little 定律，12 条流的吞吐 ≈ 并发 ÷ 平均延迟，尾部一肥吞吐就腰斩。这是 `.16` 服务端尾延迟的波动，与隧道无关——隧道在同一次会话里能稳定跑满 100MB/s。

#### 分级加压（同一时刻，服务端处于慢相）

| 流数 | 吞吐 | p50 |
|---|---|---|
| 1 | 19.53MB/s | 2.0ms |
| 6 | 11.79MB/s | 46.2ms |
| 12 | 9.61MB/s | 106.5ms |

**流数越多越慢、延迟涨 50 倍**——这是服务端在并发下串行化的特征，不是隧道带宽封顶（那会是吞吐饱和、延迟不塌）。

### 13.6 遗留问题（均不阻塞本次定案）

| # | 现象 | 证据 | 状态 |
|---|---|---|---|
| **A** | **tunnel 对象泄漏，死后仍被喂数据** | `.16` 上同时存在 3 条到本机的隧道。本机 `06:06:50` 删除 `:60391` 后，`.16` 仍喂到约 `06:07:00`；`06:08:14` 删除 `:51341` 后仍喂到约 `06:08:23`；`:61650` 的 writer 已停 1465s 仍在逐秒上报 | 待修 |
| **B** | **`.16` 喂 8.8 万包/秒，本机 `rx=0`** | `send_to` 失败会 `close.close()` 并退出循环（`socket/udp/session.rs:896`），而该会话持续 19 秒未退出 ⇒ 写入「成功」。包去了哪里未解 | 待查 |
| C | 服务端尾延迟波动（p90 14↔46ms） | §13.5 七轮表 | 独立议题，与隧道无关 |
| D | `udp_session_ring_full` 54216 / `nic_channel_drops` 29552 | 满速下累计；**未造成任何请求失败**（err=0），被 TCP 重传吸收 | 优化项 |

### 13.7 复现与取证工具

| 路径 | 用途 |
|---|---|
| `target/tmp/stallprobe/t1_run.py` | 压测入口；`T1_STREAMS` / `T1_PHASE_SEC` 控制流数与时长 |
| `target/tmp/stallprobe/probe_sample.ps1` | 1 Hz 采网卡字节/包数、内核 UDP 计数、tail 主日志抓 probe 行 |
| `target/tmp/etmon6/dualping.ps1` | 隧道 ping + 物理 ping 双路对照 |
| `%LOCALAPPDATA%\com.kkrainbow.easytier\logs\easytier.log` | 主日志（`STALL PROBE` / `EGRESS PROBE` / `INGRESS PROBE` 均为 WARN 级） |

**取证注意事项**：`.16` 若保持 DEBUG 级，压测时 **11 秒写满 100MB**，停摆窗口会被轮转冲掉（本次第一份对端日志就是这样丢掉了两个完整停摆窗口）。**对端必须调成 INFO。**

---

## 附录 A：产物与路径索引

| 类别 | 路径 |
|---|---|
| **停摆取证（2026-11-09，T1）** | `target/tmp/stallprobe/README.md`（操作与判据）、`probe_sample.ps1`（1 Hz 采 NIC + 内核 UDP 计数 + tail 主日志）；产物 `target/tmp/stallprobe/run*/{net.csv,netstat_raw.log,probe_events.log}` |
| 压测脚本 | `target/tmp/etmon6/{smb_raw,smbload,httpload,analyze6,logwatch,stats_probe}.py`、`{dualping,ping_201,ping3,nic_probe,srv_probe2}.ps1` |
| 原始数据 | `target/tmp/etmon6/run{1,3,4,5,6,7,8,9,10}/blk_*.csv`、`run9|run10/dual.csv`、`run7/{nic.csv,probe_events.log,phases.log}`、各 run 的 `phases.log` |
| `.10` 压测 | `target/tmp/t10run/` |
| 主日志 | `C:\Users\Administrator\AppData\Local\com.kkrainbow.easytier\logs\easytier.log` |
| 逻辑验证 crate | `target/tmp/probecheck/`（早期门禁不可用时的替代，现已被真单测取代） |
| 相关文档 | `docs/roadmap/{open-bugs-wave1-impl,lan-direct-underlay-guard,live-next-hop}.md`、`docs/current/{packet-loss-investigation-2026-10,egress-fair-queue}.md` |

## 附录 B：关键代码位置索引

> **行号基线**：本地工作区 HEAD `a15e51d2` + **2026-11-09 未提交的插桩改动**（`peer_conn.rs` 增约 120 行、`peers/mod.rs` 增约 20 行）。插桩前的行号（如 Ping→Pong 在 `:1466-1472`）已全部作废；引用本表前先用 `grep -n` 复核。

| 位置 | 内容 |
|---|---|
| `peers/mod.rs:119-122` | `create_packet_recv_chan()`，`mpsc::channel(128)` |
| `peers/mod.rs:124-159` | `send_peer_packet_to_chan`，`try_send` → `.send().await` |
| `peers/mod.rs:172-180` | `recv_packet_envelope_from_chan` |
| `peers/peer_manager.rs:1670` | 全局唯一通道创建；`:1673` 分发给所有 conn |
| `peers/peer_manager.rs:4967-4997` | `PeerRecv::run` 单任务串行消费（`:4974` 出队、`:5168` 内联 `process_peer_packet`） |
| `peers/conn/peer_conn.rs:1418-1611` | `start_recv_loop`（数据 + Pong 共用循环；循环体自 `:1521` 起） |
| `peers/conn/peer_conn.rs:1545-1564` | Ping → Pong，`sink.send().await` 无超时 |
| `peers/conn/peer_conn.rs:1591-1596` | 数据投递，`.is_err() => break` 陷阱 |
| `peers/conn/peer_conn.rs:1685-1724` | `send_msg` 出向，`EGRESS_WAIT=100ms`（`:1695`），`record_egress_drop()` |
| `peers/conn/peer_conn.rs:420` | `EgressFairQueue::new().with_throughput(...)` |
| `peers/conn/egress_fq.rs:34` | `EGRESS_PRIORITY_CAP=32`；`:135/:153/:207` throughput 注入与丢弃计数 |
| `tunnel/stats.rs:139-204` | `Throughput.egress_drops` / `record_egress_drop()` / `rx_bytes` / `tx_bytes` |
| `peers/conn/peer_conn_ping.rs:95-270` | 快速探测常量、`RoundOutcome`、`DeliveryEvidence`、`DeliveryExcuse` |
| **`peers/conn/peer_conn.rs:1439-1512`** | **STALL PROBE 心跳看门狗**（待 T1 结论后决定去留）：`busy recv loop stopped iterating` / `resumed` |
| **`peers/conn/peer_conn.rs:1548-1560`** | **STALL PROBE**：Pong 出向阻塞计时 `pong reply blocked on a full tunnel sink` |
| **`peers/mod.rs:132-156`** | **STALL PROBE**：`send_peer_packet_to_chan` 的 `Full` 分支 park 计时 `ingress channel full, recv loop parked` |
| `tunnel/mpsc.rs:30-44` | `MpscTunnelSender::send`（`try_send` 失败退化为无界 `await`，与 `peers/mod.rs` 同一反模式）与已有 `try_send` |
| `peers/peer_manager.rs:1099-1126` | `NicPacketProcessor`：`try_send` + 计数 + `debug!` 的正确范式（A1/A3 照抄对象） |
| `peers/peer_manager.rs:5236-5290` | foreign network 转发：入向消费端**内联 await 出向**（跨方向自锁环，§12.2-#6） |
| `peers/conn/peer_conn_ping.rs:87, 704, 854-862` | `LOCAL_CONGESTION_CLOSE_AFTER=30s`、`sink.try_send` 旁路 fq、`pingpong egress full for too long` 拆连（§12.2-#7） |

## 附录 C：最小复现步骤

1. 确认两端均为 2.7.11 / `ET.exe` 2026-11-08 19:13 或更新构建。
2. 本机启动 `dualping.ps1`（隧道 `10.10.10.16` + 物理 `192.168.11.124`，≥120s）。
3. 本机运行 `httpload.py --host 10.10.10.16 --streams 8~12 --duration 120`，重复 3 轮。
4. 监视主日志中的 `publishing degraded loss` / `degradation sustained` / `too many consecutive pingpong failures`。
5. 判定：若出现「隧道连续 TimedOut ≥15s 且物理 0 丢包 + 业务全流同时无记录」，即复现本缺陷。

---

## Changelog

- **2026-11-09 16:40（定案，本版）**：**T1 取证完成，根因定案并已修复验证。** 新增 **§13** 承载最终结论：真因是**出向队列的控制面饥饿**（32 槽出向队列满速长期占满，数据面 `send().await` 每次 ≤5ms 挤入、控制面 `try_send` 每轮必败），**推翻了 §6 的入向 128 队列自锁假设**。修复为 `peer_conn_ping.rs::do_pingpong_once` 的 `BufferFull` 分支有界等待 50ms 后重试（约 15 行）。两端部署后 7 轮压测：停摆 0 次、超时 0 条、`degraded` 0 次、`egress full` 0 次；150s×12 流 **94.12MB/s**（旧版最好 85.62），max 3046~9547ms → **1020ms**。同步就地标注被证伪章节（§0 摘要、§6 开头、§8 开头、§11 末尾），并记录两项遗留缺陷：**A** tunnel 对象泄漏且死后仍被喂数据、**B** `.16` 喂 88k 包/秒而本机 `rx=0`（未解）。另记录「服务端尾延迟波动导致吞吐 11~101MB/s」的独立议题（§13.5）。

- 2026-11-09：初版。整合 11-07 评审复核、11-08 探测误判修复与 .201 对照实测、11-09 .201/.16 双目标压测与停摆根因定位；按原始 CSV 复算全部数据，修正 6 处早期口径偏差。
- 2026-11-09：新增 §5.5「带宽上限」专项排查（配置 / 限速器语义 / 网卡速率 / 对照组），并把该假设列入 §10.3 已证伪清单；补充区分「本机 ET / 对端 ET / 上游」的抓包取证方案。
- 2026-11-09（补充评审）：新增 **§12 待办测试清单（T1~T9）与勘误 13 条**；正文就地加了 9 处 `勘误 §12.2-#n` 标记。§6 定性由「根因已定位」下调为「最强假设，待 T1 插桩确认」。本轮另落地**纯插桩改动 3 处**（收包循环心跳看门狗、Pong/数据投递的等待时长计时、`stall_probe=true` 启动标记；行为零变更、未提交），门禁全绿（`cargo check` / `clippy -D warnings` / `fmt --check` / `peer_conn_ping` 25 单测）；取证脚本见 `target/tmp/stallprobe/`。新增发现：跨方向自锁环（§12.2-#6）、低带宽满载 30s 拆连（§12.2-#7）、`egress_drops` 的覆盖边界（§12.2-#8）、B 方案应缩为 B-lite（§12.2-#10）。
- 2026-11-09（一致性修订，仅改表述与行号、不动任何结论）：① **行号基线统一到插桩后的 HEAD**——§0 要点 3、§6.2 事实 3、§8.2 实施陷阱、附录 B 共 5 处由插桩前行号改为实测量值（Ping→Pong `1545-1564`、数据投递 `1591-1596`、`send_msg` `1685-1724`、`start_recv_loop` `1418-1611`、`recv_packet_envelope_from_chan` `172-180`），并在附录 B 头部加基线声明；② §0「建议动作」改为与 §12.0 一致的 **T1 先行**（原文仍是「先做 P0 止血」）；③ §7 触发条件由 `65–100MB/s` 改为 **≈70k pps**，与 §12.2-#5 口径统一；④ §8.2 / §12.2-#9.1 的「代码示意是旧签名」改为「参数已正确，只有返回类型要改」；⑤ §9.2 稳定性判据由「ICMP 0 丢包」改为「丢包率 < 0.1% 且无连续 >2s 全黑」，消除与 A 的 tail-drop 语义冲突；⑥ 补齐 §5.4(#8)、§8.6(#12)、§10.1(#7) 三处就地标记，正文标记数与 §12.2 的 13 条对齐；⑦ 修正 §0 一处错误交叉引用（§8.8 → §8.6）；⑧ §0 要点 3 的「根因在共享队列」改为「根因**假设**在共享队列（最强假设，待 T1 确认）」，与 §12.2-#4 的降档保持一致。
