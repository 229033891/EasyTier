# 节点间连接（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-17（新增五小节：「探测与切换的时间常数」「`latency_first` 管的是哪一层」「多协议怎么被拨出来」「协议顺序到底管什么」「本机监听 vs 协议顺序」—— 澄清缓存/熔断/迟滞三层机制、`latency_first` 与 `select_conn` 的分工、**`peer_link_bond_count=1` 会连带关掉多协议拨号**、**协议顺序只定优先级不排除**、**bond 会截断拨号链**、以及监听与协议顺序方向相反；同步补进 GUI `peer_link_bond_count` / `default_protocol` 中英文提示；`latency_first` core/GUI 统一为 `true`；打洞 conn 保活 TCP 1s / **UDP 10s**；可用性闸门 `has_peer_with_live_conn`；**Bond 改一致哈希**；`AVOID_RELAY_COST` 有限化 + `path_latency` 如实上报 + Web「政策绕行」；GUI KCP 默认关闭）；2026-10-16（交叉链接 `tailscale-gap-and-conflict-audit`）；2026-10-09；2026-10-08；2026-10-07
- 范围：同一对 peer 之间的 `PeerConn` / 默认发送路径
- 多链路聚合（异质优先）：出厂默认 `bond_count=2`；见 §2 与 [`../roadmap/multi-link-bonding.md`](../roadmap/multi-link-bonding.md)
- 连接稳定性（质量选路 / 保底）见：[`../roadmap/connection-stability-todo.md`](../roadmap/connection-stability-todo.md)
- UI 已连 vs VIP/RDP 不通（bug vs 设计）见：[`tailscale-gap-and-conflict-audit.md`](./tailscale-gap-and-conflict-audit.md)
- 隧道 scheme 与伪装差距见：[`tunnels-and-transport.md`](./tunnels-and-transport.md)
- 索引：[`../README.md`](../README.md)

本文只描述 **代码今天做什么**。

---

## 1. 术语

| 术语 | 含义 |
|------|------|
| **Peer** | 对端节点（按 `peer_id`） |
| **PeerConn** | 一条具体底层隧道（TCP / UDP / QUIC / WG / ring 等），绑定一条 `Tunnel` |
| **default_conn** | 该 Peer 上缓存的「当前用于发送」的那条 PeerConn |

同一对 peer 之间 **可以同时存在多条 PeerConn**（多 listener、多协议、打洞 + 直连、重连残留等）。  
安全侧可跨多条 PeerConn 复用 Peer 级会话（见 `easytier/docs/peer_conn_secure_mode_v3.md`）。

---

## 2. 发送路径：默认单出口；可选 bonding

实现：`easytier-core` → `peers::conn::Peer::select_conn` / `send_msg` / `conn_bond`。

**单路径**（`flags.peer_link_bond_count ≤ 1`）：

1. 若已有缓存的 `default_conn` 且仍可用 → **所有 `send_msg` 走这一条**。  
2. 否则在存活连接中按**综合质量分**（延迟 + 丢包 + 抖动）挑选一条（打洞连接在 ping 未确认前不会抢流量；高丢包路径可被熔断），写入 `default_conn`。  
3. **不会**按包或按流把流量分摊到多条 PeerConn 上。

**出厂默认 / 新建配置**（`peer_link_bond_count` 默认 **2**，`peer_link_replica_fill_max` 默认 **2**；可调 1–5）：

1. 在已存活 PeerConn 上按质量门 + **同质量档内异质优先**（协议 / remote）+ **同档**同质补齐选出至多 N 条 bond 成员（见 Roadmap）。  
2. **质量优先于凑满 N**：档外（明显更差，含高丢包/抖动拉高质量分）路径**不进** bond 发送集；连接仍可保留为热备，好路径不可用时再接管。补链判据看**发送集缺口**（`bond_set_size < N`）而非存活条数——档外热备不阻塞补链；存活上限为 `N + 2`（至多 7，与发送集硬顶 5 分开），避免为凑数无限拨号，且 `N=5` 时仍保留档外余量。  
3. `send_msg` **按内层 IP 五元组哈希**到成员之一（sticky key 在压缩/加密前计算，避免密文按包喷洒）；同流不跨 conn。  
4. Phase 2b：直连 peer 在 bond 未满时继续参与 direct 拨号（沿用既有退避与周期）；**不**按包喷洒；单流仍受单条隧道上限。  
5. 出口/`bind_device` 多样性属 Phase 3，今日不宣称双宽带自动拆流。

因此：

- CLI / 状态里可能看到 `peer_conn_count > 1`（含未进 bond 的热备）；热备仍有 ping/保活流量。  
- 出厂默认 `bond_count=2` 时，**同档**多并行流可按流分摊；档外不参与分摊；设为 1 时有效吞吐仍受单条 `default_conn` 限制。  
- 多连接仍承担 **路径冗余、选优、故障切换**；单流（如单个 RDP）仍粘在一条 conn 上。

### 2.1 链路度量（今日）

| 指标 | 来源 | 是否参与 `select_conn` | 状态面 |
|------|------|------------------------|--------|
| **RTT**（`latency_us`） | `WindowLatency` 窗口均值 | **是**（质量分一项） | `PeerConnStats.latency_us` |
| **丢包**（`loss_rate`） | Ping 0/1 窗口均值 | **是**（质量分 + 熔断） | `PeerConnInfo.loss_rate` |
| **抖动**（`jitter_us`） | 同 RTT 窗口的连续样本绝对差均值 | **是**（质量分一项） | `PeerConnStats.jitter_us` |
| **质量分**（`quality_score`） | `select_conn` 综合分（越低越好） | 状态面「质量分」列 + tooltip 分项 | `PeerConnInfo.quality_score` / `quality_fused`；★=`default_conn_id` |

同 peer 多 PeerConn 的 `select_conn`（`peers/conn/conn_select.rs`）使用综合质量分（默认 `w_lat=1` / `w_loss=4` / `w_jitter=1`），丢包超过熔断阈值（默认 20%）时在有替代路径时禁止成为 `default_conn`；切换需相对边际（默认 10%）**且**绝对分差（默认 0.005）连续窗口（默认 3）。已关闭的 PeerConn 不参与选路。权重/阈值可通过 `flags.conn_select_*` 覆盖（百分制权重；0 = 默认）。

#### 探测与切换的时间常数

**探测是定时的，不是「实时测速」。** 延迟来自主动 ping/pong 往返（`peers/conn/peer_conn_ping.rs`）：

| 机制 | 值 | 说明 |
|------|-----|------|
| 底层 tick | 固定 **1s** | `interval(Duration::from_secs(1))`，`PingIntervalController` |
| 发送间隔 | `2^backoff_idx` 秒，上限 `max_interval` | 稳态**逐步退避**（1→2→4→…），最坏 10s 一轮（2026-10 起的出厂默认；此前 32s） |
| 退避归零 | `loss_counter > 0`（丢包）**或** tx 涨而 rx 没涨（单向） | 一旦异常立刻回到 1s |
| 打洞链路额外收紧 | TCP/FakeTCP **1s**、UDP **10s** | 取 `min(cap, ping_interval_max_sec)` |

打分用的不是瞬时值：RTT 与抖动取 **15 样本**滑动窗口（`WindowLatency::new(15)`，`peer_conn.rs:408`），丢包取 **100 样本**窗口（`peer_conn_ping.rs:271`）。故单次抖动不会污染判断。**链路空闲时探测频率最低**，这是省流量，不是探测变慢。

**切换不是立即切换，但要分清两层**（`peers/conn/peer.rs:242-280`）：

1. **缓存快路径**：每个包先读 `default_conn`，命中即用，**不重新打分**。仅当缓存被清空才重算 `select_conn`。
2. **缓存只被两种事件清空**（`cached_conn_should_invalidate`）：
   - `is_closed()` —— 连接真的断了（close 事件里直接置 `None`，`peer.rs:104-114`）
   - **熔断**（`loss_rate > loss_fuse`）**且**存在「已验证、未熔断」的替代路径
   轻微变差**不会**清缓存。
3. **重算时才走迟滞**（`pick_default_conn:240-247`）：新路需便宜 ≥ `max(相对 10%, 绝对 0.005)`，且 `better_streak` 连续累加到 `switch_windows`（默认 3）才切；优势不足则 streak 清零。**当前路已熔断/不可用时跳过全部阈值与计数，立即切**（`:230-234`）。

> ⚠️ **因此「连续 3 个窗口」的准确含义**：streak 是在**每次缓存清空后重新打分**时累加的，不是靠一个定时器驱动。所以在没有 close / 熔断事件时，缓存会一直命中、根本不会重新打分 —— 换言之，**链路「变差」会被迟滞挡住，「断掉」会立刻换**。一条**只是变慢但未熔断**的链路不会被主动切走，除非它先达到熔断阈值。
>
> 调 `conn_select_switch_windows` 到 1–2 可提高 responsiveness，代价是更容易在相近路径间抖动（flapping）。

> **为什么这样设计**：三层机制（缓存 + 熔断才清 + 迟滞）都指向**防 flapping**。快速切换会把在途流量的路径改掉，内层 TCP/RDP 视作丢包，反而更卡 —— 这与最初报告的「慢/卡 RDP」直接相关。同类问题见 P-AUDIT.9（bond 成员改一致哈希：原按位置哈希，成员重排会把无关的流重指向另一条 conn）。

#### `latency_first` 管的是哪一层（与 §2.1 的 `select_conn` 不是一回事）

两处都用「质量」，但作用对象和时间常数都不同，**排障时别混**：

| | 同 peer 多 tunnel 选路（§2.1） | `latency_first`（跨 peer OSPF 选路） |
|---|---|---|
| **决定什么** | 这几条直连里**走哪一条** | **下一跳交给哪个 peer**（直连腿 vs 中继腿 vs 更远的绕行） |
| **粒度** | 单 peer 内 | 全网拓扑 |
| **决策频率** | **每包**（命中缓存则不重算） | 按路由表重算周期，非每包 |
| **开关位置** | `flags.conn_select_*`（权重 / 阈值，GUI 未暴露） | `latency_first`（GUI 高级设置 →「质量优先选路」） |
| **关掉的后果** | 无（默认即质量选路） | 回到**最少跳**：有直连就永不比较中继 |

**开启后的实际收益**：直连腿与中继腿按**质量边代价公平竞争**。典型受益场景 —— 某节点的直连是移动网络（高延迟高丢包），而经另一节点中继反而更稳；LeastHop 会死守那条烂直连，LeastCost 会走中继。代价是**可能多一跳**，且**依赖各节点持续上报链路质量**（peer-center `report_peers`，约 60s 节流），控制面开销略增。

**三条既有的约束**：

- **与「仅 P2P」互斥**：开了 `p2p_only` 就禁止中转，质量优先选路失去意义，GUI 会直接拦下并提示（`configConflicts.ts` / `p2p_only_blocks_latency_first_help`）。
- **同网内需统一**：不同节点各自为政会让 OSPF 的边代价口径不一致，可能选出反直觉的路径。
- **不影响同 peer 内的 tunnel 选择**：`select_conn` 由 `conn_select_*` 参数控制，与 `latency_first` 正交。

### 2.1.1 Web / GUI 状态表（今日）

- 节点信息表对 **每条直连 PeerConn 一行**（`flattenPeerConnRows`）：协议、本端/对端地址、RTT、抖动、丢包、质量分、上下行按链路展示；`default_conn` 整行加粗；同 peer 多行共用灰白斑马底色分组。
- 本机行与无直连隧道的中转 peer 仍各占一行。
- Web「对端连接历史」另有延迟 / **丢包** / **抖动** / 流量趋势（按 peer 聚合采样，非 per-conn）；仅配置服务器形态有库表。详情见 Archive 文。
- Web 状态页 `CollectNetworkInfo` 约 **2s** 一轮（配置模式跳过 info、列表约 6s）；历史采样优先按 heartbeat 运行中实例定向 collect，减轻与配置/心跳的隧道争用（见 Archive §6）。

跨 peer 的 OSPF：配置键 `latency_first`（UI：「质量优先选路」）**出厂默认 `true`**（2026-10-17 起 core / GUI / proto 三处默认统一为质量优先）→ 用 LeastCost（最小化质量边代价，可多跳）；关闭时用 LeastHop（先最少跳，同跳再比代价）。边代价由同一质量分编码进 peer-center `DirectConnectedPeerInfo.latency_ms`（`round(score*1000)`，熔断路径另加固定加成；发布端 `|Δ|<20` 不改写，配合约 60s 上报节流）。零丢包/抖动时量级仍≈ RTT 毫秒，与历史行为兼容。

#### 多协议怎么被拨出来（`default_protocol` × `peer_link_bond_count` 的耦合）

**协议不是「选一个」，而是「能并行挂几条就挂几条，再按质量分选」。** 拨号主循环在 `connectivity/direct/mod.rs:482-503`：

```rust
while !available_listeners.is_empty() {
    let current_scheme = available_listeners.last().scheme();
    // 把当前 scheme 的所有 listener 一起拨
    self.spawn_direct_connect_tasks(...).await;
    let _ = tasks.join_all().await;        // 等这一批全部完成
    if has_directly_connected_conn(dst) && !peer_needs_bond_fill(dst) {
        return Ok(());                     // ← 连上且 bond 满了才收工
    }
}
// 只有这一批全军覆没，才试下一种 scheme
```

**因此 `default_protocol`（默认 `"udp,tcp"`）与 `peer_link_bond_count`（默认 2）是耦合的**，因为 `peer_needs_bond_fill` 的第一个判断就是 `if !bond.enabled() { return false }`（`peer_manager.rs:1890`），而 `bond.enabled()` 即 `bond_count > 1`（`conn_bond.rs:73`）：

| `peer_link_bond_count` | 实际行为 |
|---|---|
| **> 1（默认 2）** | 逐个 scheme **并行拨满**。默认配置下 udp + tcp 两条 tunnel 同时挂在同一 peer 上，`select_conn` 按质量分选 default，`pick_bond_set` 还带**协议多样性偏好**（`conn_bond.rs:280` 对未覆盖过的 scheme 加分），故 udp/tcp 质量接近时优先保留不同协议的成员 |
| **= 1** | `bond.enabled()` 为 false ⇒ `peer_needs_bond_fill` 恒 false ⇒ **第一条协议连上就 `return`**，`default_protocol` 里的后续协议**根本不会被拨**。此时只剩单协议，没有质量择优 |

**⚠️ 两个必须知道的限制**：

1. **把 bond 调成 1 会连带关掉多协议择优**，这是最容易踩的坑 —— 很多人手动调 1 是为了「减少连接数」，但同时也失去了 udp 被 QoS 限速时的自动降级。若目的是省资源，应保持 2（差异隧道只是热备、平时不额外占带宽），或用 `lazy_p2p` 减少无业务时的背景打洞。
2. **协议补齐只在「bond 未满」时发生，不因质量变差而重新拨号**。`collect_peers_need_task`（`direct/mod.rs:352-353`）的条件是「无直连 **或** 需要 bond 补齐」；bond 一旦填满，该 peer 就从候选里消失，5s 扫描循环不再看它。所以：已有 udp 变慢时，`select_conn` 只能在**现存 conn 集合**里选 —— 若集合里只有 udp，没有 tcp 可选。ET 不会因为「udp 变差」去补一条 tcp。**缓解手段是让 bond 保持 ≥2，把第二种协议常备着**，而不是指望质量变化触发重新拨号。

> **为什么 `true` 是有意义的开关**：`latency_first = false` 时，`get_gateway_peer_id`（`peers/conn/peer_map.rs:281-283`）对 LeastHop 直接短路——只要目标有直连 `PeerConn` 就返回目标自身，**中继腿永不参与竞争**，即使它质量更好。`true` 才允许 OSPF 在直连与中继之间按代价择优（`peer_manager.rs` `send_msg_internal` 的 `latency_first_gateway` 分支）。
> **升级注意**：未显式写 `latency_first` 的既有 TOML/CLI 配置，升级后会从 LeastHop 变成 LeastCost，即跨 peer 选路结果可能改变（中继可能胜出）。需要旧行为请显式写 `latency_first = false`。

#### 协议顺序（`default_protocol`）到底管什么

**它只管「优先级」，不管「能不能连」** —— 这是最常被误解的一点。`config/protocol_preference.rs:66-71`：

```rust
pub fn protocol_preference_sort_key(preference: &[String], scheme: &str) -> u32 {
    match preference.iter().position(|item| item == scheme) {
        Some(index) => (preference.len() - index) as u32,  // 越靠前分越高
        None => 0,                                          // 未列出 → 排最后，不是排除
    }
}
```

可参与排序的 scheme 共 7 个（`PREFERENCE_SCHEMES`）：`tcp` / `udp` / `ws` / `wss` / `quic` / `wg` / `faketcp`。`ring` 是 rendezvous 专用，两个 connector 都会过滤掉；未识别的 token 会被丢弃并 `warn`（`parse_protocol_preference:44-46`）。留空 / 全无效 → 回落到 `["udp","tcp"]`。

> **⚠️ 但 bond 会截断拨号链**：即使顺序里排了 N 个 scheme，只要前 `bond_count` 个拨成功，`peer_needs_bond_fill` 就变 false，主循环 `return`。所以**顺序表里第 3 个及以后的方案通常拨不到** —— 它只在前面失败时才有机会。例：`udp,tcp,ws` + `bond_count=2` ⇒ udp+tcp 成功即止，`ws` 不会尝试。

**为什么默认值是 `udp,tcp` 且 udp 在前**（`protocol_preference.rs:6-13`）：在**出厂默认开启**的传输里，udp 是唯一**不自带重传**的 —— tcp / ws / wss / quic / faketcp 都重传，内层跑 TCP（RDP/SSH/HTTP）时叠加成 TCP-over-TCP，轻微丢包即卡顿。tcp 保留在第二位以保住「UDP 被阻断时自动落到 TCP」。**wg** 同样是纯 UDP（boringtun），但包头开销更大且需双方都宣告，故默认 opt-in。

**因此不建议把 wg 排在 udp 前面**：它与 udp 底层同为 UDP，放首位等于让 bond 的两条隧道走同一种传输，而 `pick_bond_set` 的多样性分类是按 scheme 字符串（`DiversityClass.scheme`）判定的，会误判成「已覆盖两种协议」。需要 wg 优先时写 `udp,wg,tcp`（udp 打洞成功率最高 → wg 次之 → tcp 兜底）优于 `wg,udp,tcp`。

#### 本机监听地址 vs 协议顺序（方向相反，别弄混）

| 配置 | 决定什么 | 作用域 |
|---|---|---|
| **本机监听地址** | **别人**能用什么协议连**我** | 我 → 广播出去（`direct/mod.rs:1096-1102`：`get_ip_list` 把 `mapped_listeners` + `running_listeners` **全部**广播） |
| **协议顺序** | **我**用什么协议去连**别人** | 我 → 按对端广播的 listener 排序后拨 |

所以「要不要多加协议」取决于**对端监听了什么**，不是越多越好：

| 情况 | 建议 |
|---|---|
| 双方都是默认 tcp + udp | 维持默认即可，不必加 |
| 已知有对端只监听 `wg` / `quic` | 加进**本机监听**，否则连不上它 |
| 需要伪装成 HTTPS 流量穿透审查 | 加 `wss`（占443） |
| 对端在严格防火墙后（仅放行 443） | 加 `wss`，否则 udp/tcp 皆被封 |

**不建议盲目全加**：每加一个就多开一个对外端口（攻击面），且部分方案（`quic` 默认 11012、`faketcp` 默认 11013）在国内运营商链路上未必通畅，加了可能只是噪音。端口默认值为 tcp/udp `11010`、wg `11011`、quic `11012`、faketcp `11013`（`source_effective_port:82-95`）。

> **手动 peer URL 的额外规则**：`preference_candidate_urls`（`:142-166`）只做**改写式降级**，且只覆盖 `REWRITEABLE_SCHEMES`（tcp/udp/ws/wss/quic）；`wg` / `faketcp` 因传输指纹不同**永不被 URL 改写**。且**配置 URL 永远排第一** —— 协议顺序只作失败后的兜底，不会覆盖你显式指定的 scheme。

### 2.2 拨号与选路（今日：全量拨号 + 质量选路，无档位）

| 项 | 今日 |
|----|------|
| **初始节点 URL** | `[[peer]]` / `peer_urls` / `public_server_url`（完整 tunnel URL），全部恒维持连接 |
| **保活 / 拆连** | `ping_interval_max_sec`（**默认 10s** 2026-10 起、此前 32s，自适应退避上限；GUI 与 CLI 2026-10 起均可配置）+ `ping_fail_close_count`（**默认 10**，连续失败多少次才拆 conn）。两个默认值都集中在 `config::toml::DEFAULT_PING_*`，`PeerConn` 的 `0 → 未设置` 回落值也引用同一常量，避免漂移。<br>per-conn 更严的 ping 上限：**TCP/FakeTCP 打洞 1s**（#2632）、**UDP 打洞 10s**（NAT UDP 老化常见 ~30s）；取 `min(cap, ping_interval_max_sec)`，**只收紧不放宽**。<br>**为什么拆连阈值是 10**：拆完要重建（~5s 任务循环 + 2–6s 打洞 + 5s 握手），重建代价远大于故障窗口 ⇒ 短暂抖动就拆-建是亏的。失败后退避重置为 1s，每轮约 3s，10 轮 ≈ 30s 判死。真死链路会晚 ~15s 发现，但两种情况重建都要 ~30s，**总恢复时间几乎不变**。<br>**另有独立于拆连的快速活性探测**（Phase F，见 [`../roadmap/live-next-hop.md`](../roadmap/live-next-hop.md) §5.6）：协商 `liveness-echo-v1` 后以 333ms × 连续 3 次未往返（≈1.9s）判定「不往返」，**只把结果写成丢包率**（供 OSPF 按代价改路、并让状态面停止显示「连通 / 丢包 0%」），**不参与拆连**——上面的拆连时间常数与容错因此完全不变 |
| **可用性闸门** | 转发前校验目标**有 live conn**（`PeerMap::has_peer_with_live_conn`），而非仅在 map 里存在（`has_peer` = `contains_key`，最后一条 conn 断开后条目仍在）。否则一个已死的直连/网关条目会先匹配上分支，包在 `Peer::send_msg` 里失败并**不再落到可用的中继腿**——恰好是「该由中继兜底」的时刻。选路 `get_gateway_peer_id` 也把同一判据用在**路由点名的那个下一跳**上：死条目不再被选中，改试候选列表里的下一个，全部不可用则返回空让调用方尽早失败（`select_next_hop`）。**注意边界**：它不会去搜一个不在路由表里的替代下一跳——跨节点挑一个「还活着的 peer」会造成环路与误投；「本机 peer 表里没有」也**只作兜底**（该情形既可能是跨网网关，也可能是条目已被回收的死下一跳，只有调用方的 `foreign_network_client.has_next_hop` 能区分），见 [`../roadmap/live-next-hop.md`](../roadmap/live-next-hop.md) §4、Phase A |
| **硬约束** | `disable_p2p`（不主动直连）/ `p2p_only`（绝不中转）/ 允许中转开关（`disable_relay_data` 取反：**仅 OSPF 避让，不硬丢包**） |
| **遗留可选** | `prefer_peer_relay`：UI 隐藏；**不**抑制打洞；仅 TOML 可启用 OSPF 对端中继拓扑投影（与质量分正交） |
| **选优** | 同 peer 按 `select_conn` 综合质量分（RTT+loss+jitter）+ 熔断 + 迟滞；热路径命中缓存时也校验 `closed`/fuse（fused incumbent 暴露窗口 ≈20s → 0）。对端间 OSPF 由 `latency_first`（**默认 `true`**，质量优先选路）选 LeastCost，可跨跳换质量 —— 直连与中继腿按代价公平竞争。`lazy_p2p` 减少无业务时的背景打洞 |
| **Bond 选流** | `peer_link_bond_count=2` 出厂；成员集由 `pick_bond_set` 按质量档 + 异质优先 + 副本补齐选出（**fused / 未验证打洞不进发送集**）。流→成员用**一致哈希**（对每成员 `conn_id` 求哈希取最小），因此成员集变化时**只有「自己的成员离开」或「新成员胜出」才迁移**，纯重排导致 **0 个流迁移**。⚠️ 成员仍按**哈希**分摊而非「永远走最优」——bonding 的价值是聚合吞吐，压成单最优等于删能力。⚠️ **设为 1 会连带关掉多协议拨号**（见上文「多协议怎么被拨出来」） |
| **政策绕行可见性** | `disable_relay_data` / 中继白名单不匹配的节点，其出边加 `AVOID_RELAY_COST = 1e9`（约 1000× `OSPF_EDGE_COST_MAX`；**有限且 `saturating_add`**，仍是事实否决）。`path_latency` **如实上报**该惩罚（不再 `% AVOID_RELAY_COST` 抹除），故**远大于实测质量值**的 `path_latency` 即表示「该路径含拒载中转的节点」；Web 渲染为「政策绕行 / Policy detour」而非天文数字。⚠️ proto 尚无「哪个中间跳拒载」字段，只能看出「被绕行」看不出「谁拒载」 |
| **协议偏好** | `flags.default_protocol` 为有序 CSV（**默认 `udp,tcp`**；老单值=长度 1；可含 `wg`/`faketcp`）。**为什么 udp 在前**：tcp/ws/wss/quic/faketcp 都自带重传，内层 TCP（RDP/SSH/HTTP）会退化成 TCP over TCP，轻微丢包即卡顿；`udp` 是**默认开启项中**唯一不叠加重传的常用传输（`wg` 也是纯 UDP，但需双方都宣告，故保持 opt-in）。`tcp` 留第二位以保住「UDP 被阻断时自动落到 TCP」。直连按列表排序对端已宣告监听；手动 peer URL **先按配置的 scheme 拨号**，失败后才在 tcp/udp/ws/wss/quic 间改写 scheme 降级（保留源有效端口：显式或原 scheme 默认，如 wss→tcp 保 443；`wg`/`faketcp`/`ring` 不改写）。`ListConnector.url` 恒为配置 URL；`active_url` 为当前存活/降级获胜拨号 URL；remove 可按二者任一匹配 |
| **Web 控件** | 高级设置 →「连接」：「质量优先选路」(`latency_first`)、`disable_p2p` **正向展示为「允许 P2P 直连」**、「允许作为中转节点」(`disable_relay_data` 取反，同为 `inverted` 机制，字段名不变)、「协议顺序」(`default_protocol`)——配置页开关约定见 [`desktop-gui-and-config-server.md`](./desktop-gui-and-config-server.md) §4 |

示例（有序列表；**443 不一定可用**，按实际可达端口填写）：

```toml
[[peer]]
uri = "tcp://home-relay.internal:5000"
[[peer]]
uri = "wss://relay.example.com:8443/et"
[[peer]]
uri = "wss://relay.example.com/et"   # 隐式 443；仅当网络放行 443 时作为备选
```

---

## 3. 与运营商「单连接限速」的关系

部分网络对单条 TCP/UDP 流有带宽上限。  
`bond_count=1` 时，即使有多条 PeerConn，数据面仍只使用 `default_conn`，**无法**叠带宽。  
出厂默认 `peer_link_bond_count=2`（或更高）时，**多并行流**有机会超过单连接人为限速；**单条大象流**（含单个 RDP）仍粘在一条 conn 上。

---

## 4. 缺口（相对完整 bonding 愿景）

| 缺口 | 说明 | 状态 |
|------|------|------|
| 主动维持 N 条隧道 | Phase 2b | **已合入**（bond 未满继续 dial；无进展时让出调度，避免空转） |
| 状态面「in bond set」 | Phase 2b | **已合入**（`in_bond_set` / `bond_class`；CLI / GUI） |
| 出口 / `bind_device` 多样性 | Phase 3 | 未实现 |
| 产品预期 | — | 出厂默认 `bond_count=2`（按流分摊）；勿写「双宽带自动拆流」或「单流必翻倍」 |

---

## 5. 验收对照（现状）

| 场景 | 期望（今天） |
|------|----------------|
| 两节点仅一条存活隧道 | 全部流量走该隧道 |
| `bond_count=1`，多条存活隧道 | 发送仍只走 `default_conn`；热备 ≠ 聚合 |
| `bond_count=N>1`，已有异质存活隧道 | 仅同质量档进发送集并按流哈希分摊（至多 N）；档外热备不参与分摊；同档内优先协议/remote 多样 |
| 默认隧道断开（`bond_count=1`） | 重新 `select_conn`，切到另一条存活连接（若有） |
| `bond_count=N>1`，直连未满 N | direct 继续拨号补齐；无进展则下轮再试 |
| 双宽带 / `bind_device` 异质 | **未实现**（Phase 3） |

> 质量选路与 bonding 2a 单元测试在 `conn_select` / `conn_bond`。非 443 保底（P0.5）与双路径现网抽样（P1.8b）见 [`../roadmap/connection-stability-todo.md`](../roadmap/connection-stability-todo.md)。
