# 已建立线路上的转发（下一跳的方向性、可用性与无环备份）

## Status

- Status: **Roadmap**（**方案重议**：首版改法不被接受，见 §4；**Phase A、C、F、F2、B/D 已落地（工作区，待现网）**。其中 **B/D 只覆盖等代价旁路**，不等代价备份 + 真 LFA 未做，见 §5.1/§5.2）
- 最近审阅：2026-10-17
- 证据：[`../current/packet-loss-investigation-2026-10.md`](../current/packet-loss-investigation-2026-10.md) §10
- 相关 Current：[`../current/peer-connections.md`](../current/peer-connections.md)、[`../current/tailscale-gap-and-conflict-audit.md`](../current/tailscale-gap-and-conflict-audit.md)（C3 / P-AUDIT.4）
- 索引：[`../README.md`](../README.md)

> 本文只写**目标语义与改动方案**。首版改法仍在工作区未提交且不编译，**不进入 `current/`**（[`../ops/doc-update-after-task.md`](../ops/doc-update-after-task.md) §1）。发送路径的现状描述仍以 `current/` 为准。

---

## 1. 目标语义

数据包只从**源与目的之间已经建立的、且位于通往目的的最优路径上**的链路发出。

### 1.1 候选资格

候选下一跳 X 必须同时满足三条，缺一不可：

| # | 条件 | 含义 | 今天有没有 |
|---|---|---|---|
| 1 | **方向性** | X 在「我 → 目的」的最优/次优路径上，且 X 的路径**不经过我** | ❌ 无数据 |
| 2 | **可用性** | 我与 X 之间的 conn 已建立（`has_peer_with_live_conn`） | ✅ P-AUDIT.4 |
| 3 | **无环性** | 切到 X 不会把我拽进环路（LFA 判据，见 §5.2） | ❌ 无数据 |

**只有 (2) 而没有 (1)(3)，等价于「挑一个还活着的 X」，那不是选路**——这正是首版改法的问题（§2、§4）。

### 1.2 比较规则：候选合格之后按什么排序

合格候选之间用**确定性字典序**排序，不允许由 Dijkstra 的到达顺序决定：

| 顺序 | 键 | 理由 |
|---|---|---|
| 1 | **路径代价** `cost`（ms 当量） | 主目标：走质量最好的那条。**严格优先，不设余量** |
| 2 | **跳数** `hops` | 等代价时取短路径——这就是「同质量优先直连」（直连恒为 1 跳） |
| 3 | **稳定身份键**（peer id） | 代价、跳数都相同时的最终兜底，保证重建之间不翻面 |

四条硬约束：

1. **比较对象必须是「到目的的整条路径」的代价**，不是「到我的这条链路」的质量。后者就是首版改法 `relay_quality` 干的事（§2）。
2. **「质量」必须新鲜**。只比「上次测到的数」就有窗口——§10 正是在**选路完全正确**的前提下黑洞的（那条路径当时实测更快，只是远端腿已死却还在被广告）。见 §5.4、§5.6。
3. **不做「切换阈值」**（初版曾提议复用 `OSPF_EDGE_COST_MIN_DELTA = 20`，**已撤回**）：20 个 ms 当量会直接否掉 §7 的验收用例（13ms 直连 vs 两跳各 <0.5ms，只差 11），也与「否则走质量最好」相冲突。抖动由**边级迟滞**负责（`apply_ospf_cost_hysteresis`，`conn_select.rs:147-152`）：单条边的上报代价在 20 以内不跳变，路径比较保持严格即可。
4. **tie-break 必须落在「重建之间稳定的身份」上**（peer id），**不能**用节点下标：`build_peer_graph_from_snapshot` 的节点编号来自 `DashMap` 迭代顺序（`peer_ospf_route.rs:399-413`），每次重建都会变。

**为什么「同质量优先直连」在改动前既无规范也无保证**：`dijkstra_with_first_hop` 只在 `next_score < ent.get()`（严格小于）时更新，等代价时**先到先得**，即由 `BinaryHeap` 的内部与加边顺序决定。这一点本轮**已实测证实**——新增用例 `equal_cost_equal_hops_is_stable_across_edge_order` 在只比较 `(cost, hops)` 的中间版本上**失败**，两次仅改变加边顺序的构建给出了不同的首跳。所以要把它变成契约，必须补上第 3 个键（§5.5）。

### 1.3 分层职责：「选路」不建立链路

| 层 | 位置 | 干什么 | 会不会建链 |
|---|---|---|---|
| 建链 | `connectivity/`（`direct/`、`hole_punch/`） | 决定拨给谁、何时拨 | **会** |
| 选路 | OSPF `peer_ospf_route.rs` | 从**已有**拓扑 + 边权算下一跳 | **不会**（纯计算） |
| 转发 | `send_msg_internal` → `select_conn` | 把包交给选中的**已开** conn | **不会** |

因此本方案的硬规则是：**转发路径零建链**。更好的下一跳由建链层先拨出来，OSPF 随后才可能在代价上偏向它。

反馈通道（`connectivity/direct/mod.rs:323-358`）：

```text
候选集 = peer_manager.get_route().list_routes()        // ← OSPF 路由表
触发   = 后台（lazy_p2p 默认 false ⇒ 对所有已知 peer 都拨）
       || should_try_p2p_with_peer(...) && has_recent_traffic(peer_id)   // 有流量就拨
跳过   = 已有直连（除非 bond 需要补隧道）
```

⇒ 默认配置下「先建好再切换」**成立且不需要谁专门去要**：建链层会对每个已知可达 peer 尝试直连。

> 转发路径唯一的窄例外：`RelayPeerMap::send_msg`（`relay_peer_map.rs:226-262`）对**本机发出**的包、且 **secure mode** 开启时，会先 `ensure_session`；会话未就绪则把包**缓冲**（每 peer 上限 32，`:264-270`）等握手完成。**转发包不建会话**（`:232-239`）。这是逻辑会话握手，不是重拨隧道。

#### 已知缺口：`lazy_p2p = true` 时没有「更便宜」这个理由

反馈是 **PeerId 级**的（`has_recent_traffic(peer_id)`），不是**目的地级**的，而且**不含「这条更便宜」**：

- 建链层会问「我在跟这个 peer 通信吗」→ 会 → 拨。
- 建链层**不会**问「哪条路径更便宜」→ 它没有这个信息。

默认 `lazy_p2p = false` 掩盖了这个缺口；一旦开启，只有 `need_p2p` 的对端或有过流量的对端会被拨，此时「OSPF 认为某条直连更便宜、但它还不存在」**没有任何通道去要它**。单独记为已知缺口，与 §6 的 Phase 正交。

其余边界（不变）：

- 发送路径**不新拨隧道**。
- 同一条 peer 上的多条 conn 仍由 `select_conn` 在**已建立**的 conn 之间选，那一层不动。
- 拓扑清扫（90s 路由保留 / 60s sweep）与保活阈值（连续失败 10 次）都不动。

---

## 2. 首版改法实际做到的（`peer_map.rs`，**已从工作区移除**）

> 本节保留为复核记录。该实现已全部回退，**不在当前工作区里**；`Phase A` 只保留了其中「对被点名的那个下一跳查活连接」这一条（§5.3、§6）。

调用链：`send_msg_internal`（`peer_manager.rs:4055`）→ `get_gateway_peer_id`（`peer_map.rs:316`）→ `choose_sendable_gateway`。

判定输入 `SendableGateway`（8 个字段）：`dst` / `my_id` / `policy` / `ospf_next` / `dst_live` / `hop_live` / `hop_in_peer_map` / `fallback_live`。其中 `fallback_live` 是调用方在本机**先算好**再传进来的。

兜底候选 `best_established_relay`（`peer_map.rs:387-400`）：

```text
for peer_id in list_peers_with_conn() {        // 我的全部活 peer，无任何方向过滤
    if peer_id == dst || peer_id == my || peer_id == exclude { continue }
    score = relay_quality(peer_id)             // 我↔它的质量分
    best = min(score)
}
```

`relay_quality`（`peer_map.rs:365-384`）= 我对该 peer 的 `min(quality_score)`（跳过未验证打洞）。

**关键事实**：`dst` 只出现在排除条件里，**从不参与候选筛选或排序**；整个函数没有一次「X 能不能到 dst」的查询。所以排序键是**「到我的链路质量」**，不是**「到目的的路径代价」**。

所以首版改法实现的是 **条件 (2) 单独成立**，而 §1 要求 (1)+(2)+(3)。

---

## 3. 先修的事实问题

| 编号 | 事实 | 锚点 |
|---|---|---|
| F-1 | ~~**不编译**：`relay_quality` 用 `f64` 比较 `PeerConnInfo.quality_score`，该字段是 protobuf `float`（`f32`）~~ **✅ 已消除**：该实现已回退，Phase A 不引入 `quality_score` 比较 | `peer_map.rs:373-374`（已移除）；`core_peer.proto:36` |
| F-2 | 正确写法在同文件已有先例，Phase B/D 若要用质量分请照此写 | `peer_map.rs:701` `f64::from(conn.quality_score)` |
| F-3 | 文档口径已漂移：两处审计仍写「选路用 `has_peer` / 短路保持不变」，与 `peer-connections.md` 的新说法矛盾 | `tailscale-gap-and-conflict-audit.md:207`、`connection-stability-todo.md:166`（**本批文档已同步**：改回与已发布行为一致，并指向本文） |

F-1 已随回退消失；`cargo test -p easytier-core --lib`（991 项）+ `cargo clippy` + `cargo fmt --check` 在 Phase A 后均通过。

---

## 4. 问题点（为什么首版改法不被接受）

### P1 — 引入 A↔B 转发环路（阻塞）

两跳线型 **A — B — C**，B→C 的 conn 已死。按 `handle_packet` → `send_msg_internal` 的分支推演：

```text
A: dst_live(C)=false, ospf_next=B, hop_live=true                → 发 B    （合理）
B: dst_live(C)=false, ospf_next=C 且 hop_in_peer_map=true,
   hop_live=false → fallback(exclude=C) → 唯一活 peer 是 A       → 发回 A  ❌
A: 再收到 → 又选 B ……                                        ↺
```

- `forward_counter > 7`（`peer_manager.rs:3706`）只是**截断**，不是防环：一个本该立刻丢弃的包变成约 7–8 次跨节点转发（期间 `forward_counter > 2` 还会关掉 `latency_first`，`peer_manager.rs:3722`）。
- OSPF 死记录 90s 被清掉后**更糟**：`ospf_next = None` ⇒ `foreign_hop = false` ⇒ 仍进 fallback ⇒ **B 继续往 A 甩包**。
- 对照改动前：B 在 LeastHop 下 `has_peer(C)` 为真返回 C，`has_peer_with_live_conn(C)` 为假 ⇒ `warn!` + 立即 `RouteError`。**1 次转发、明确日志，严格优于新逻辑。**
- 现场旁证：`packet-loss-investigation` §10 记录的「转发跳数超过 7 被丢」日志说明这条截断路径确实在现网发生。

### P2 — 误投：候选没有路径校验

`best_established_relay` 只要求「有活连接」，不要求「有路径到目的」。命中的 peer 若没有到 dst 的路由，包会在对端被丢（`send_msg_internal` 末尾的 `no gateway for peer`），丢包位置从本机移到对端，可靠性没有提升，只多了一次转发与 TTL 消耗。

### P3 — 修不到 §10 的主故障模式

§10.2/§10.3 的场景是 **DEV 侧 shdx 有活连接**（否则 P-AUDIT.4 的闸门早已挡掉、退回直连分支），是 **shdx 那一端**丢了包。新逻辑在 DEV 侧的判断是 `hop_live(shdx) = true` ⇒ **照样选 shdx**，黑洞不变。

首版改法只在「中间节点自己的直连死了、且恰好有第三个 peer 能到 dst」时偶然生效；而那种情况下 OSPF 本来就会把那个 peer 算成下一跳。真正缺的是**快速活性探测**（§5.6 / Phase F），不是本机再挑一个活 peer。

### P4 — 热路径开销与副作用

- `best_established_relay` 是 O(peer 数)，且每包调用；触发条件（`!dst_live && !hop_live && !foreign_hop`）**恰好是故障窗口**，即调用频率最高的时刻。
- `relay_quality` → `list_peer_conns` → `peer.rs:475-481` 会**顺带 `close_peer_conn` 关连接**，并且每 peer clone 全部 `Arc<PeerConn>` + 逐个算分。在发送热路径上做带 mutation 的全表快照，风险与开销都不可接受。

### P5 — 策略双份真相

「要不要算 fallback」「什么算 foreign hop」在调用方（`peer_map.rs:342-350`）和被调方（`choose_sendable_gateway`）各判一遍，规则重复实现，已存在分叉风险。

### P6 — 可观测性倒退

兜底命中时没有任何日志或计数；原来那条 `cannot send msg to peer through gateway` 会消失，而 `connection-stability-todo.md` 的验收方式正依赖这条 warn 的计数。

### P7 — 调用方语义被连带改动

`resolve_exit_node_peers`（`peer_map.rs:481`）、`need_relay_by_foreign_network`（`:663-674`）都只用 `.is_some()` 判断可用性。新逻辑下前者会过滤掉条目在但无活连接的出口候选（大概率是想要的），后者在同一边角从 `Ok(false)` 变 `Err(RouteError)`——两处都未评估、未测试。

### P8 — 推翻了既有设计决定，却只改了一半文档

`tailscale-gap-and-conflict-audit.md:207` 明确写过该短路「**保持不变**（它回答的是拓扑问题）」，`connection-stability-todo.md:166` 也写「选路仍用 `has_peer`」。首版改法推翻了它，却只改了 `peer-connections.md`，另两处未同步而留下互相矛盾的记录。（本批文档已把三处口径对齐，见 §3 F-3。）

### P9 — 「不在 peer 表里」被当成可发：兜底下一跳反被兜底遮住（Phase A 自引入）

`next_hop_is_sendable` 的规则是「`hop == 我` ⇒ 可发；否则 `不在表里 || 有活连接` ⇒ 可发」。后半句把**跨网网关**（本机确实没有条目，属设计如此）和**已被回收的死下一跳**（`clean_peer_without_conn` → `close_peer` 摘掉条目，路由条目却还留着）当成同一件事，于是后者会被立刻返回。

后果不是「选了一个错跳」这么轻：按主下一跳优先返回，**后面代价相同、且已经连上的等代价备选根本不再被看**，包随后在 `send_msg_internal` 的网关闸门处因「既无活连接、也非跨网网关」被丢掉。也就是说 Phase B/D 的兜底**恰好在它唯一有意义的场景（主下一跳连条目都不剩了）里失效**。

改动前没有备选概念，丢掉的结果一样，所以这不是旧缺陷被暴露，而是**新加的兜底被自己的资格判据挡掉**——比不做 B/D 更坏，因为表里明明有一条能走的路。已改为分档（§5.3），T3 只作兜底。

---

## 5. 方案

### 5.1 OSPF 输出候选下一跳（方向性的数据来源）— ✅ 已落地（**等代价**范围）

现状路由表**每个目的只存一条**下一跳：`next_hop_map: DashMap<PeerId, OspfNextHopInfo>`；两条建表路径（LeastHop 的显式子图与 LeastCost 的全图）都在 `gen_next_hop_map_with_least_cost` 里只取 Dijkstra 的一条结果，等代价的多条候选在那一刻被丢掉。

**落地范围与初版的差异（重要）**：初版写的是「按 `path_latency` 升序保留前 K 条」，实现时**收缩为「只保留等代价（ECMP）候选」**。理由见 §5.2——只有等代价那一种情形能用本地数据证明无环；「次优但仍在余量内」需要邻居到目的地自己的距离，那是 RFC 8402 的 LFA 计算，本地没有。代价后果：

- 有等代价旁路时：主用一跳死掉可**立刻**切换（不必等拓扑收敛）；
- 没有等代价旁路时（含 §10 那种「13ms 直连 vs 1ms 两跳中继」的不等代价情形）：走 F/F2 的**代价膨胀**路径改路（~10s），不是本地瞬切。

实现：

| 项 | 内容 |
|---|---|
| 候选计算 | `graph_algo::equal_cost_first_hops`：取最短路径 DAG（紧边 `dist(u)+w==dist(v)`）从 dst 反向闭包，凡 `start` 的直接后继即为等代价首跳；上限 `MAX_ALTERNATE_NEXT_HOPS = 2` |
| 存储 | `next_hop_alt_map: DashMap<PeerId, AltNextHopEntry{hops, version}>`，版本与 `next_hop_map_version` 同源；`clean_expired_route_info` 一并清理 |
| 对外 | `get_next_hop_with_policy` **不变**（只返回首选，`list_routes` / `foreign_network` / `resolve_exit_node_*` 不受影响）；新增 `Route::get_next_hops_with_policy`（默认实现退化为单条，其它实现零改动） |
| 顺序 | 首选在前，其余按 **peer id 排序**（反向闭包的遍历顺序跟随图内部次序，必须显式定序才能跨重建稳定） |

### 5.2 无环判据（无环性的数据来源）— ✅ 已落地（等代价的**证明**，非 LFA）

初版提了「去掉边 (我,X) 后 X 仍能到 dst」与「真 LFA `cost(X→dst) < cost(我→X)+cost(我→dst)`」两条。实现时发现：**等代价这一情形根本不需要额外判据，可以直接证明无环**。

设 `us = start`，`X` 为候选首跳。

- `X` 是**等代价**首跳 ⟺ `dist(us,dst) = w(us,X) + dist(X,dst)` … (☆)
- `X` 若把包送回 `us`，则 `dist(X,dst) = w(X,us) + dist(us,dst)` … (★)
- 把 (★) 代入 (☆)：`w(us,X) + w(X,us) = 0` ⇒ 与正权矛盾。**故等代价候选不可能回环**。

这条证明只用**本地图**，不需要邻居的路由表。反过来，「次优/余量内」的候选做同样的代入会留下正的残差，堵住它必须知道邻居到 dst 的距离——即 RFC 8402 LFA，**本轮不做**，这也是 §5.1 收缩范围的直接原因。

- 仍然成立：判据必须在**建表时**算，不能在 `peer_map` 发送时算（发送时没有拓扑图，也不该在每包路径上跑图算法）。
- 未落地的部分记为后续项：要支持不等代价备份，需要按目的地做反向距离（每次重建 N 次反向 Dijkstra，或缓存 `dist(X,dst)`），并加 LFA 不等式。

### 5.3 发送路径改动 — ✅ 已落地

`get_gateway_peer_id` 现在：

1. 取候选列表 `route.get_next_hops_with_policy(dst, policy)`（首选 + 等代价备选，顺序由路由给出）。
2. **按顺序**套 `select_next_hop`（纯函数 + 惰性 `probe`，见 Phase A），分三档：
   - **T1 自发**（`hop == my_id`）；
   - **T2 本地可发**：条目在 **且** ≥1 条活连接（`has_peer` 只是 `contains_key`，最后一条 conn 断了条目还在，所以活连接判据不可省）；
   - **T3 无本地条目**：**仅作兜底**。
3. 取第一个 T1/T2 命中；T2 落空的候选逐个 `debug!` 记录（保留 P6 的可观测性）。
4. 全部候选都不可用时，才返回第一个 T3 候选；连 T3 都没有 ⇒ `None`，保持「明确失败」，**不打洞、不猜**。

- 「另一条已建立的线」= **候选列表里的下一个**，不是全表扫描，也不是任意活 peer。
- 首版引入的 `best_established_relay` / `relay_quality` 已删除（P2/P4/P5 一并消失）。

#### T3 为什么只能兜底（2026-10-17 修正，见 §4 P9）

从 `PeerMap` 内部看，下面两件事**长得一模一样**（都是「本机 peer 表里没有」）：

1. **跨网网关**——本来就没有本机条目，发送时由 `foreign_network_client.has_next_hop` 接手（调用点的 `NOTIC`）；
2. **已被回收的死下一跳**——`clean_peer_without_conn` → `close_peer` 会把条目从 peer 表摘掉，而命名它的路由条目还要留 `REMOVE_UNREACHABLE_PEER_INFO_AFTER` 那么久。

只有调用方能区分二者（`foreign_network_client.has_next_hop`）。首版实现把「不在表里」直接当可发，于是**回收掉的主下一跳会立刻被返回，把已经连上的等代价备选遮住**，包随后在发送侧因「既无活连接、也非跨网网关」被丢掉——**兜底恰好在最需要它的时候失效**（正是 Phase B/D 要解决的场景）。T3 降级为兜底后：本地可发候选永远优先；实在没有才走 T3，跨网路由行为与改动前一致，而一个陈旧下一跳退化为改动前同样的「告警 + 丢弃」，不引入新失败模式。

### 5.4 度量本身是对的，缺的是新鲜度与确定性

> **本节修正了本文初版的一处误判。** 初版把 §10.2 的「代价倒挂」当成根因之一，证据不支持。

| 检查 | 结论 |
|---|---|
| 边权语义 | `cost = round(score*1000)`，`score = 1.0·rtt_s + 4.0·loss + 1.0·jitter_s`（`conn_select.rs:97-107`、`123-129`）⇒ **ms 当量**，丢包按 4 倍计。基线版本直接上报真实时延，本仓换成这个是**等价量纲的推广**（多了丢包 / 抖动权重），不是压缩 |
| 13ms 直连 | `round(0.013*1000) = 13`，**没有被压缩** |
| 中继路径 cost 2 | 两跳各 cost 1 ⇒ 每跳实测 <0.5ms ⇒ **该路径确实比 13ms 直连快** |
| `clamp(1, …)` | 只影响亚毫秒 LAN 边，那本来就是 1ms 量级的等价物 |
| 未验证打洞边 | 已被 `best_edge_cost` 排除（`peer_map.rs:700`） |

⇒ **§10 那次选路是正确的**：它选了当时实测更快的那条（~1ms vs 13ms）。黑洞的成因是 **shdx→home-pc 那条边已经死了却还在被广告**（保活连续失败 10 次 ≈30s；`REMOVE_UNREACHABLE_PEER_INFO_AFTER` 90s；清扫 60s）。

所以本方案要补的**不是**「改代价公式」，而是：

1. **新鲜度**：让死掉的边尽快被探测到并从拓扑消失（§5.6 / Phase F）。这是 §10 的直接解。
2. **确定性比较 + 切换阈值**（§1.2 / §5.5）：减少无谓地切到中继腿，并消除等代价抖动。
3. 字段名 `latency_ms` 装的是代价——保留为可读性债务（wire 兼容），不改。

### 5.5 比较规则落地（Phase C，✅ 已落地）

`dijkstra_with_first_hop`（`graph_algo.rs`）改为对**字典序键**做松弛与堆排序：

```text
key = (cost, hops, tie_key(first_hop))
```

- **堆键同构**：`MinScored((cost, hops, tie_key), node)`。键是全序，且路径延拓对它是单调的，所以「首次弹出即可定型」仍然成立——`visited` 跳过不会把更差的跳数锁死。
- **`tie_key` 由调用方注入**：新增形参 `tie_key: impl FnMut(G::NodeId) -> TK`（`TK: Ord + Copy`）。调用方传**peer id**（`peer_ospf_route.rs:545` 的 `|n| graph.node_weight(n).copied().unwrap_or_default()`），而不是节点下标——理由见 §1.2 约束 4。
- **`(cost, hops)` 单独不够**（已实测）：本轮先实现了只比 `(cost, hops)` 的版本，`equal_cost_equal_hops_is_stable_across_edge_order` 立即失败，于是补上第 3 个键。这是本轮唯一一次「先写错、被测试挡住」的地方，保留记录。
- 返回值不变：`(costs, first_hop)`，`first_hop` 仍是 `(首跳, 跳数)`，`gen_next_hop_map_with_least_cost` 与 `list_routes` 的对外语义不受影响。
- **不引入切换阈值**（§1.2 约束 3）。

### 5.6 新鲜度：快速活性探测 + 度量驱动降级（Phase F）

**空窗可以分解**：`黑洞时长 ≈ 探测延迟 + 收敛延迟`。当前 = 保活 ≈30s（`ping_interval_max_sec` 10s × `ping_fail_close_count` 10）+ 路由保留 90s + 清扫 60s ≈ **2–2.5 分钟**，与 §10 实测吻合。

#### 现状：liveness echo 已有，但没有亚秒检测

`PeerConnLiveness`（feature `liveness-echo-v1`，`peer_conn_liveness.rs:10`、`42-43`）挂在**同一个周期性 ping** 上：`start_probe`（`peer_conn_ping.rs:305`）/ `wait_for_echo`（`:246`）/ `finish_probe`（`:318`）。所以它的探测速度仍被保活节奏限住，**不存在亚秒级失效检测**。

#### 降级通道本来就是通的，缺的只是探测速度

`ospf_edge_cost_from_score` 在 `loss > loss_fuse`（默认 0.20）时加 `OSPF_FUSE_COST_BONUS = 10_000`（`conn_select.rs:115`、`125-127`）⇒ **代价被顶高 → 下一次 SPF 自动改路**。

⇒ 也就是说：**只要 loss 立刻反映到边权、且边权能立刻分发出去，§10 那次「1ms 中继 vs 13ms 直连」的选择会在一个 SPF 周期内被自动推翻**，既不需要新建链路，也不需要数据面参与。缺的是两件事：①「30s 才看得出 loss 变了」；②**边权分发本身受中心 60s 跳过 / 15s 拉取限制**（见下文「传播链路的现实约束」）。

#### 改法（✅ 已落地）

1. **BFD 式快速探测**（思路同 RFC 5880）：独立于业务保活的短间隔探测，只在小包上跑。
2. 探测结果**立即**变成边权 ⇒ 走**已有的 fused-cost 通路**改路，不新增机制。
3. **兼容硬约束**：沿用 `PeerFeatureFlag` 协商（`liveness-echo-v1`）。**不单方面加快节奏**，否则未协商的老节点会被打成假死。
4. 探测变快之后，**90s 路由保留 / 60s 清扫仍不缩短**（§1 的「不动清扫」结论不变，且现在更有理由不动——失效由探测处理，不靠清扫兜底）。

**实现（`peer_conn_ping.rs` / `peer_conn.rs`）**

| 项 | 取值 | 位置 |
|---|---|---|
| 探测间隔 | 333ms | `FAST_PROBE_INTERVAL` |
| 判死 | 连续 3 次未往返 | `FAST_PROBE_MISSES_TO_DEGRADE` |
| 单次等待 | `clamp(3 × 实测 RTT, 300ms, 2s)` | `fast_probe_timeout` |
| 最坏检测 | ≈ 3 × (333ms + 300ms) ≈ **1.9s** | 由 `fast_probe_timeout_scales_with_rtt_and_is_clamped` 断言 |

- 探测只写 `PeerConn::fast_probe_degraded`（`AtomicBool`）；`loss_rate()` 在它置位时返回 **1.0**，于是 `score_conn` 判定 fused → `ospf_edge_cost_from_score` +`OSPF_FUSE_COST_BONUS` → 边权重新上报 → 下一次 SPF 改路。
- **绝不拆连**：不碰 `loss_counter`、不碰 `ping_fail_close_count`，所以「拆完要重建 ~30s」那套容错时间常数完全不变（§8 边界）。
- **门禁**：`PeerConnLiveness::is_enabled()`（协商结果）。未协商时循环空转并**清掉**可能残留的降级位，绝不参与判定。
- 每 conn 一个额外任务，随 conn 关闭退出（`close_notify.is_closed()`）。
- 副作用（正向）：状态面不再对一条正在静默黑洞的 conn 显示「连通、丢包 0%」（§10.1 的抱怨）。

**实现中发现并修掉的一个坑：两条探测流的 seq 会互相冒充**

`ctrl_resp_sender` 是 **broadcast**，`Pong` 只按 seq 相等匹配。保活与快速探测若共用 0 起的 seq 空间，A 的 Pong 会被 B 收下当作自己的应答——**假成功，正好掩盖这个探测要报告的失败**。所以把 u32 拆开：保活占低 31 位，快速探测占最高位（`FAST_PROBE_SEQ_BIT` / `keepalive_seq` / `fast_probe_seq`），并用 `keepalive_and_fast_probe_sequences_never_collide` 守住。

**成本与已知限制**

- 50 conn × 333ms × ~30B ≈ **4.5 KB/s**，全部在控制面，**每包零开销**。
- **高 RTT 限制**：单次等待被 2s 上限截断，所以 RTT 秒级的链路上检测会慢于 1.9s。这是探测的固有限制（不可能比 RTT 更快得出结论），不是可调参数问题。
- 目前**没有配置开关**：门禁就是 `liveness-echo-v1` 协商。若现网反馈控制面开销偏大，再补一个 flag（属独立小改动）。

#### ⚠️ 传播链路的现实约束（2026-10-17 复核发现，**阻塞 F 的收益**）

「探测快」只解决**检测**。「让别节点改路」还要把新的边权送出去，而**边权根本不走 OSPF 的 1s 点对点同步**：

| 环节 | 事实 | 锚点 |
|---|---|---|
| 边权来源 | `cost_calc` **只**读 peer-center 的 `global_peer_map`，不看本地 conn 状态 | `peer_center/instance.rs:379-385` |
| 上报（我 → 中心） | 作业 5s 一跳，但**只有 peer *集合* 变化或距上次上报 ≥60s 才真发**；`peer_list` 只取 `direct_peers.keys()`，**不含代价** ⇒ **纯代价变化最多 60s 才上报一次** | `peer_center/instance.rs:327-336` |
| 分发（中心 → 我） | 轮询 **15s**（摘要未变也 15s） | `peer_center/instance.rs:279-300` |
| OSPF 点对点同步 | 1s，但只带**拓扑（谁连谁）与各自的 *路由表***，不带边权 | `peer_ospf_route.rs:4125`、`400-457` |

⇒ 端到端现实值 = **检测 1.9s + 上报 + 拉取 ≤15s + SPF**。修复「上报」之前，纯代价变化的实发间隔受 60s 心跳限制。

**处置（2026-10-17，方案 ① 已落地）**：上报判据从「只比 peer 集合」改为**比 `peer → cost` 快照**（`instance.rs::report_can_be_skipped`，`ReportedPeerCosts = BTreeMap<PeerId, i32>`）。中心端本来就无条件重算摘要、且摘要**含 `latency_ms`**（`server.rs:86`、`:158`；服务端已有 `peer_latency_change_invalidates_digest` 用例），所以代价变化现在能立刻走完「上报 5s 跳动 → 摘要变 → 拉取」这条路。

- **代价变化 → 上报**：现在**最迟一个 5s 跳动**（此前 ≤60s）。
- **端到端（修后）**：1.9s 检测 + ≤5s 上报 + ≤15s 拉取 ⇒ **典型 ~10s，最坏 ~22s**。
- **仍存的地板 = 15s 拉取轮询**（`instance.rs:279-300`）：这是下一个可调的杠杆，但它直接影响中心负载，建议**先实测再动**。
- 代价若**只变但未超迟滞**（`OSPF_EDGE_COST_MIN_DELTA = 20`）不会进快照 ⇒ 不会触发上报，符合预期。

**为什么不是方案 ②（探测结论改「拓扑存在性」）**：那会让别的节点把一条「降级但仍可用」的路直接从表里删掉，失去最后手段表达；且①已经足够，②只在需要更快（<5s）时才考虑。

**本阶段能确定的收益**：检测从「拆连才可见（≈15s）」压到 **1.9s**，且不再等 `ping_fail_close_count`；配合①后，改路时间由分钟级降到**十秒级**。

> **为什么不用「中间节点回报 send 失败」**：那个 hop 凭什么知道送不出去？只有靠它自己的超时/保活，**与源侧的检测延迟是同一个量级**，所以它**治不了 §10**，只是多一条通道。真正能压缩空窗的只有本地快速探测 + 快速收敛。

### 5.7 重建成本基线（B/D 的度量 + LFA 的 go/no-go 依据）

**测量工具**（零行为变更，`#[ignore]` 故不进 CI）：

```bash
cargo test -p easytier-core --lib -- --ignored --nocapture route_rebuild_cost
```

`graph_algo.rs::tests::route_rebuild_cost_baseline` 构造**对称合成 mesh**（n 节点、每节点 k 个出邻居、双向、**全部权重 1**）。全 1 权重是候选计算的**最坏情形**：所有等长路径都是最短路径，紧边 DAG 最宽。

| n | k | deg | 一次 Dijkstra | 候选计算 | 比值 | LFA 上限估计 (deg+1)×dij |
|---|---|---|---|---|---|---|
| 10 | 2 | 4 | 74µs | 58µs | 0.78× | 0.37ms |
| 30 | 2 | 4 | 131µs | 122µs | 0.94× | 0.65ms |
| 30 | 5 | 10 | 209µs | 119µs | 0.57× | 2.3ms |
| 100 | 5 | 10 | 646µs | 432µs | 0.67× | 7.1ms |
| 100 | 10 | 20 | 1.23ms | 498µs | 0.40× | 25.9ms |
| 300 | 10 | 20 | 3.38ms | **1.72ms** | **0.51×** | **70.9ms** |

**结论 1（B/D 成本可接受）**：候选计算稳定在 **0.4–0.94× 一次 Dijkstra**，且随规模**线性**。每次重建建两张表 ⇒ 总成本约从 2 次 Dijkstra 当量升到 **~2.5–3 次**，可接受。

> **过程记录（一次自我发现的回归）**：初版是「**每个目的地**反向走一遍祖先集」，即 O(目的地数 × 节点数) = O(V²)。实测 n=300 时为 **7.4×（24.9ms）**——是我引入的回归。改成 §5.1 所述的**一次递增距离合并遍历**后为 **0.51×（1.72ms）**，快 14 倍且不再超线性。**这条基线本身就抓出了这个回归。**

**结论 2（全量 LFA 的 go/no-go）**：LFA 需要每个直接邻居到各目的地的距离 ⇒ 重建成本上限 ≈ `(deg + 1) × 一次 Dijkstra`，而**每次重建要建两张表**：

| 规模 | 每次重建估计（两张表） | 判断 |
|---|---|---|
| n ≤ 30，deg ≤ 10 | ≤ ~5ms | ✅ 可接受 |
| n = 100，deg = 10 | ~14ms | ⚠️ 临界 |
| n = 100，deg = 20 | ~52ms | ❌ 偏重 |
| n = 300，deg = 20 | ~142ms | ❌ 不可接受 |

⇒ **全量 LFA 只在小/中规模可承受；大规模必须按需计算**（只对「无直连 + ≥2 个活邻居」的目标算），否则不做。**本轮决定：不做全量 LFA**，理由见 §5.2（收益窄、对 §10 无效）+ 上表的成本天花板。等现网验收显示仍有「第一跳抖动 + 无直连」的实例，再回来按需实现。

---

## 6. 分阶段

| Phase | 内容 | 规模 | 每包开销 |
|---|---|---|---|
| **A**（✅ 已落地，待现网） | 去掉随机兜底与 `relay_quality` / `best_established_relay`；可用性判定回到 `send_msg_internal` 既有位置，`get_gateway_peer_id` 只对「OSPF 命中的那一个 hop」查 live；修 F-1 编译错误；补负例测试（候选不可用必须尽早失败且不跨节点转发） | 小，纯收敛 | 无 |
| **C**（✅ 已落地） | 比较规则落地：字典序 `(成本, 跳数, 稳定身份键)`；**不含切换阈值**（§1.2 约束 3、§5.5） | 小，与 B 正交 | **零**（只在 SPF 时算） |
| **B**（✅ 已落地，**等代价范围**） | OSPF 输出**等代价**候选下一跳（§5.1）+ 发送侧按候选顺序选（§5.3） | 中 | 每包 ≤3 次 `has_peer_with_live_conn`，可忽略 |
| **D**（✅ 已落地，**等代价范围**） | 无环性用**等代价的本地证明**代替 RFC 8402 LFA（§5.2）；不等代价备份（需反向距离）**未做**——go/no-go 见 §5.7 | 小（证明免费） | 零（建表时算） |
| **F**（✅ 已落地） | **快速活性探测 + 度量驱动降级**（§5.6）—— 只改度量，不拆连。检测 1.9s | 中，≈4.5 KB/s | 零 |
| **F2**（✅ 已落地） | **上报判据纳入代价**（`report_can_be_skipped`）：让纯代价变化即时上报，否则 F 的检测速度无法转成改路速度 | 小，一行级判据 + 5 条用例 | 无（上报仍是 5s 跳动） |
| ~~E~~（长期可选） | 数据面路径反馈（中间节点回报 send 失败）。**降级**：非标准做法，且下界仍受限于本地探测速度（见 §5.6 末），F 之后若仍有残留场景再评估 | 大 | 高（失败抖动时） |

Phase A 不是「修复」，是**回滚到已知正确行为**；Phase C 小而正交、零开销、可独立先上；Phase B/D 把「已建立 + 在路径上 + 无环」补成可实现形态；**Phase F 才是 §10 的直接解**。

---

## 7. 验收

正向：

- 直连活着、OSPF 指向一个无活连接的更低代价 hop：Ping 仍走直连，时延保持直连量级。
- 该 hop 的 conn 建立完成之后：后续包切到它，中间没有「先切过去再等握手」的空窗。
- **不误判代价**：一条 13ms 直连 vs 两跳各 <0.5ms 的中继 ⇒ **应选中继**。防止有人用「直连无条件优先」来「修」本问题（§5.4）。
- **快速降级（Phase F + F2）**：某条边的 loss 突增后，**检测 ≤2s**；端到端改路 = 检测 + 上报(≤5s 跳动) + 拉取(≤15s) + SPF ⇒ **典型 ~10s，最坏 ~22s**（见 §5.6）。→ 单测已覆盖检测预算、降级位与上报判据；端到端属现网验收项。

负向（首版改法缺的）：

- **无环路**：3 节点 A—B—C 且 B→C 死、A↔B 活，断言不出现 B→A 回送；包在**第一跳真正发不出去的那一点**失败（`no gateway for peer`，debug 级），不在节点间往返。
- **无路径不入选**：候选集不含「无路径到目的」的 peer。
- **确定性**：构造两条等代价路径并连续重建多轮，下一跳稳定（不再随机）。→ Phase C 已用 `equal_cost_*` 两条用例覆盖「等代价按跳数」与「完全相同按稳定身份键」。
- ~~**不抖**：直连与中继代价差小于阈值时不切；超过阈值才切。~~ **已撤回**：阈值与验收「13ms 直连 vs 两跳各 <0.5ms 应选中继」冲突；抖动改由边级迟滞负责（§1.2 约束 3）。
- **探测兼容（Phase F）**：未协商 `liveness-echo-v1` 的老节点**不会**被加快节奏打成假死。
- 直连最后一条 conn 已拆、peer 条目仍在、另有已连接 peer：失败次数落到秒级，而不是间隔 2 秒的约 70 次（这是现网可观测的终点）。

构建（Phase A + C + F + F2 + B/D 后实测）：

- `cargo test -p easytier-core --lib` → **1011 passed / 0 failed**；`cargo clippy -p easytier-core --lib` 与 `cargo fmt --check -p easytier-core` 干净。
- **Phase A 已覆盖**（`select_next_hop_tests`，`peer_map.rs`）：自己恒可发；活 hop 可选；**死 hop 被拒并顺延到下一候选**；**回收掉的主下一跳不遮住活的备选**（P9 回归）；实在没有本地可发候选时仍返回第一个无本地条目的候选（跨网网关行为不变）；全死 ⇒ `None`；`probe` 在首个可用候选处短路（每包热路径，不允许无条件探测）。
- **Phase C 已覆盖**（`graph_algo.rs::tests`）：`equal_cost_prefers_the_fewer_hop_path`（等代价取少跳 —— 「同质量优先直连」）、`strictly_cheaper_multi_hop_path_still_wins`（**成本仍是主键**：13 vs 2 走中继，防止把「直连优先」过度应用）、`equal_cost_equal_hops_is_stable_across_edge_order`（完全相同时不随加边顺序翻面）。
- **Phase F 已覆盖**（`peer_conn_ping.rs::tests` / `peer_conn.rs::tests`）：`fast_probe_timeout_scales_with_rtt_and_is_clamped`（含「3 次未往返 < 2s」的预算断言）、`keepalive_and_fast_probe_sequences_never_collide`（两条探测流 seq 不重叠）、`fast_probe_degraded_flag_drives_reported_loss`（降级位 → `loss_rate()==1.0`，且 **conn 仍不关闭**）。
- **Phase F2 已覆盖**（`peer_center/instance.rs::tests`）：`reports_when_only_a_cost_changed`（纯代价变化必须上报）、集合变化/心跳/中心切换各自的应报行为。
- **Phase B/D 已覆盖**：`graph_algo.rs::tests` 的 `equal_cost_alternates_are_all_reported`（菱形两条都报）、`costlier_paths_are_not_alternates`（**次优不作候选**）、`direct_edge_wins_the_tie_and_the_detour_is_the_alternate`；`peer_ospf_route_tests.rs` 的 `equal_cost_diamond_offers_every_first_hop`（路由表级：两个候选且首选在前）、`single_path_topology_has_no_alternates`。另有测量工具 `route_rebuild_cost_baseline`（`#[ignore]`，见 §5.7）。
- **仍待覆盖**：端到端「主用 conn 死 → 立刻切到等代价备选」的集成验证（需 Phase 拓扑 harness 或现网）；以及**不等代价备份 + 真 LFA**——**本轮决定不做**，go/no-go 见 §5.7。

---

### 7.1 现网验收清单

**前置**：≥2 节点且其中一个是 peer-center；把日志级别开到 **debug**（实例启动参数 `--console-log-level debug`，或 `easytier-cli logger`）——Phase A 的跳过日志是 **debug** 级，看不到就等于没测。

**T1 — 死网关：不被选中，也不阻塞备选**（Phase A）
拓扑 A—B—C，让 B→C 的 conn 断开（拔 B 网线 / 拆 conn）。在 A 上观察：

| 期望 | 说明 |
|---|---|
| ✓ 出现 `route names a next hop with no live conn, skipping it` | debug 级 |
| ✗ 不再出现 `cannot send msg to peer through gateway` | 该 warn 已不在此路径（见 §4 P6） |
| ✗ `forward counter exceed, drop packet` 不增长 | 无 A↔B 往返（首版改法的 P1 环路） |
| ✓ 包在 **B** 侧以 `no gateway for peer` 结束 | 而不是网络里往返 7 跳 |

**T2 — 确定性与不误判代价**（Phase C）
对称 mesh（两条同代价路到同一目的）：`easytier-cli -p <admin> route` 连续 10 次，**首跳必须完全一致**。
**负例**：造「13ms 直连 + 两条各 <0.5ms 的两跳」，`route` 必须选**中继**（防止有人把「直连优先」过度应用）。

**T3 — 快速降级（复现 §10）**（Phase F + F2）
拓扑 DEV—shdx—home-pc，且 DEV↔home-pc 有直连。**在 shdx 上**让 shdx→home-pc 断开，DEV↔shdx 保持：

| 观测点 | 位置 | 期望时限 |
|---|---|---|
| 探测告警 | shdx 日志 `fast liveness probe: link not round-tripping...`（warn） | **≤2s**，且**只出现一次**（仅状态跃迁） |
| 度量 | `easytier-cli peer` 该 conn 的 loss | **≤2s** 变 100% |
| 边权 | `easytier-cli peer-center` 中 shdx→home-pc 的 `latency_ms` | ≤~10s（最坏 ~22s）跳到 1e4 量级 |
| 改路 | DEV 上 `easytier-cli route`（home-pc） | 同上时间内改走直连 |
| 恢复 | 恢复 shdx→home-pc | 日志 `... round-trips again`（info）；**conn 未被拆** |

**负例（拆连语义未变）**：30s 内抖动 3 次（每次 <10s 恢复）⇒ conn **不应被拆**。

**T4 — 等代价备选瞬切**（Phase B/D）
菱形拓扑，拆掉首选 hop 的 conn ⇒ **第一个包**就走备选；`route` 表内容不变。

**T5 — 兼容**
与**未升级**的老版本节点互联 ⇒ conn 不掉、loss 不跳 100%、探测节奏不变（未协商 `liveness-echo-v1` ⇒ 快速探测空转）。

**T6 — 资源**
CPU：先跑 §5.7 的 harness 拿基线，再看实例占用（注意快速探测 3 次/s × **每条 conn**）。
中心：观察 `report_peers` 频率（代价变化现在会即时上报，见 Phase F2），确认中心 RPC 速率可接受。

**T7 — 回滚顺序**（每步后 `cargo test -p easytier-core --lib` 应保持全绿）
纯文档：`docs/**`（5 个文件）。
代码：`peer_center/instance.rs`（F2）→ `peer_conn*.rs`（F）→ `peer_ospf_route.rs` + `graph_algo.rs` + `route/mod.rs`（B/D）→ `peer_map.rs`（A）。

---

## 8. 边界 / 不做

- 不为单个 Ping 或单条 TCP 重建隧道。
- 不把保活失败阈值或 90s 清扫改短。
- 不在发送热路径上跑图算法或做全表 conn 快照。
- 不以「任意活 peer」充当「通往目的的链路」。

---

## 9. Changelog

| 日期 | 内容 |
|---|---|
| 2026-10-17 | 初版：更好的下一跳先由现有拨号建立，连上后再切换；无活连接的 hop 不被选中。 |
| 2026-10-17 | **方案重议**。代码复核后首版改法不被接受：无方向性 ⇒ A↔B 环路（P1）+ 误投（P2）；修不到 §10 主故障（P3）；热路径 O(N) 且 `list_peer_conns` 带 mutation（P4）；策略双份真相（P5）；可观测性倒退（P6）；调用方语义连带变更（P7）；未同步另两处审计（P8）；不编译（F-1）。目标语义收敛为「已建立 + 在通往目的的路径上 + 无环」，方案与分阶段见 §5、§6。 |
| 2026-10-17 | 补 §1.2 比较规则（字典序 `成本 → 跳数 → 直连优先` + 切换阈值；并说明今天等代价是**随机**的、会抖）、§1.3 三层职责（**选路不建链**；建链反馈通道 = `list_routes()` + `has_recent_traffic`；`lazy_p2p=true` 时缺「更便宜」这条理由）、§5.5 比较规则落地。**修正 §5.4**：初版把「代价倒挂」当根因是错的——边权是 ms 当量（`round(score*1000)`），§10 那次选路其实**正确**，黑洞成因是**死边仍在被广告（健康信息过期）**。分阶段新增 Phase C（比较规则，与 B 正交）。 |
| 2026-10-17 | 按「企业级标准 / 长远性能」重排方案：新增 **§5.6 新鲜度 = BFD 式快速活性探测 + 度量驱动降级（Phase F）**；**Phase E（数据面路径反馈）降级为长期可选**——它非行业标准做法，且下界仍受限于本地探测速度（那个 hop 只能靠自己的超时知道失败，与源侧同量级），治不了 §10。关键事实：`liveness-echo-v1` 已有但挂在同一周期 ping 上（无亚秒检测），而**降级通路本来就通**（`OSPF_FUSE_COST_BONUS` 在 loss > `loss_fuse` 时顶高代价 → 下个 SPF 自动改路），缺的只是探测速度。分阶段加「每包开销」列；§7 加快速降级与探测兼容两条验收。 |
| 2026-10-17 | **交付前复查（3 处）**：① **LeastHop 短路仍有洞**——`has_peer(dst) && LeastHop` 只看条目存在，死条目会**遮住路由表及其等代价备选**，而 `latency_first=false` 的包（含转发超过 2 跳后被关掉 latency_first 的包）正好走这条 ⇒ 改为 `has_peer_with_live_conn(dst)`，对活的目的行为不变、只是不再遮蔽。② **快速探测的降级位缺自愈**——只由探测任务置位，若该任务消失会永久停在 100% ⇒ 改为**保活成功时也清位**（任何一次成功往返都作为独立第二意见）。③ `tie_key` 每次松弛会被调用多次，补上「必须纯函数」的契约说明。另记录**企业级缺口**（未做，需在后续阶段补）：新路径只有日志、无计数器；快速探测节奏/备选数量上限无配置项；B/D 与 F/F2 无运行时 kill switch；性能基线是 `#[ignore]` 工具、CI 无性能闸门；无端到端集成用例。 |
| 2026-10-17 | **§5.7 重建成本基线 + 修掉自己引入的回归**：加 `#[ignore]` 测量工具（合成 mesh、全 1 权重=最坏情形）。首测即抓出初版候选计算是「每目的地各走一遍」⇒ O(V²)，n=300 时 **7.4×（24.9ms）**；改为**一次递增距离合并遍历**后 **0.51×（1.72ms）**，线性。**全量 LFA 的 go/no-go 也就此有了数字**：上限 ≈ `(deg+1)×Dijkstra`，每次重建两张表 ⇒ n≤30 可接受（≤5ms）、n=100 临界（14ms）、n=300/deg=20 不可接受（~142ms）。**决定本轮不做全量 LFA**（收益窄 + 对 §10 无效 + 成本天花板），等现网验收显示仍有「第一跳抖动 + 无直连」实例再按需实现。 |
| 2026-10-17 | **Phase B/D 落地（代码，等代价范围）**：新增 `graph_algo::equal_cost_alternates_for_all`（最短路径紧边 DAG，按**递增距离**一次遍历合并各节点的首跳集合），路由表新增 `next_hop_alt_map`（版本同 `next_hop_map_version`），`Route` 新增 `get_next_hops_with_policy`（默认实现退化为单条，其它实现零改动），发送侧按候选顺序取第一个可用（保留逐候选 `debug!`）。**范围收缩并说明原因**：只做**等代价**候选，因为这是唯一能用本地数据**证明无环**的情形——等代价 ⟹ `w(us,X)+w(X,us)=0`，与正权矛盾；「次优/余量内」需要邻居到目的地的距离（RFC 8402 LFA），未做。后果：无等代价旁路时（含 §10 的不等代价场景）改路仍走 F/F2 的代价膨胀（~10s），不是本地瞬切。**实现中撞到并修掉一个 bug**：反向闭包用 `seen` 去重会把「第二次到达 start」吞掉，导致只剩一个备选（被新用例挡下）。1007 项通过。 |
| 2026-10-17 | **Phase F2 落地（代码，方案 ①）**：peer-center 上报判据从「只比 peer 集合」改为比 **`peer → cost` 快照**（`instance.rs::report_can_be_skipped` + `ReportedPeerCosts`），并抽出纯函数便于测试（5 条用例，含「只改代价必须上报」这条回归）。中心端无需改动：它本来就无条件重算摘要、摘要**含 `latency_ms`**（`server.rs:86`、`:158`）。效果：代价变化由「≤60s 心跳」变为「≤5s 跳动」⇒ 端到端改路 **典型 ~10s / 最坏 ~22s**。**仍存地板 = 15s 拉取轮询**，建议先实测再动（影响中心负载）。**不采用方案 ②**（改拓扑存在性）：会让别的节点把「降级但仍可用」的路直接删掉，失去最后手段表达。1002 项测试通过。 |
| 2026-10-17 | **Phase F 落地后的复核（发现阻塞项）**：探测本身正确（`fast_probe` / 降级位 / seq 拆分均已单测覆盖），但**「改路」所需的时间不由探测决定**——边权只来自 peer-center 的 `global_peer_map`（`peer_center/instance.rs:379-385`），而上报作业**只在 peer 集合变化或距上次 ≥60s 时才发**（`instance.rs:327-336` 的 `peer_list` 只取 key），分发轮询 **15s**（`instance.rs:279-300`）。故端到端 = 检测 1.9s + 上报 + 拉取 ≈ **典型 15–25s / 最坏 75s**，**撤回「≤2s 改路」的说法**（§5.6 新增「传播链路的现实约束」，§7 与 §6 同步）。 |
| 2026-10-17 | **Phase F 落地（代码，待现网）**：新增 BFD 式快速活性探测（`peer_conn_ping.rs::fast_probe`，333ms × 3 次未往返 ≈ **1.9s** 判死，单次等待 `clamp(3×RTT, 300ms, 2s)`）。它**只写度量**（`PeerConn::fast_probe_degraded` → `loss_rate()==1.0` → `score_conn` fused → `ospf_edge_cost_from_score` +`OSPF_FUSE_COST_BONUS` → 边权重报 → 下次 SPF 改路），**不拆连**，`ping_fail_close_count` 时间常数不变。门禁 = `liveness-echo-v1` 协商（`PeerConnLiveness::is_enabled`），未协商则空转并清位。**实现中发现并修掉的一个坑**：保活与快速探测共用 0 起 seq 时，broadcast 的 `Pong` 会被另一条流当作自己的应答（**假成功，正好掩盖要报告的失败**），故把 u32 拆开（保活低 31 位 / 快速探测最高位）。成本 ≈4.5 KB/s，全在控制面。997 项测试通过。 |
| 2026-10-17 | **Phase C 落地（代码）**：`dijkstra_with_first_hop`（`graph_algo.rs`）改为按字典序键 `(cost, hops, tie_key(first_hop))` 做松弛与堆排序，新增 `tie_key` 形参由调用方注入 **peer id**（不能用节点下标——每次重建都会变）。**撤回初版的「切换阈值」建议**：20 个 ms 当量会否掉 §7 的验收用例（13ms 直连 vs 两跳各 <0.5ms，差 11），也与「否则走质量最好」冲突；抖动由边级迟滞负责。**过程记录**：先只比 `(cost, hops)` 的版本被新用例 `equal_cost_equal_hops_is_stable_across_edge_order` **实测挡住**（改加边顺序结果不同），因此补上第 3 个键——这也把 §1.2 里「等代价会抖」从推断变成了实证。5 条 `graph_algo` 用例 + 全量 993 项通过，clippy/fmt 干净。 |
| 2026-10-17 | **Phase A 落地（代码，待现网）**：回退首版实现（`choose_sendable_gateway` / `relay_quality` / `best_established_relay` / 兜底 relay 全部移除），改为**只对被点名的那个下一跳查活连接**——新纯函数 `next_hop_is_sendable(hop, my_id, hop_in_peer_map, hop_live)` + 5 条单测；死 hop 不再被选中（改为 `debug!` 记录后继续找下一条 route）。语义边界：**不搜索替代下一跳**（跨节点的兜底就是 P1/P2 的环路与误投），跨网网关与本机无条目的 peer 行为与改动前一致。实测 `cargo test -p easytier-core --lib` 991 通过、clippy/fmt 干净。 |
| 2026-10-17 | **P9 修正（Phase A 的资格判据自伤）**：`next_hop_is_sendable` 的「不在本表 ⇒ 可发」把**跨网网关**（本机无条目属设计如此）和**条目已被 `clean_peer_without_conn` 回收的死下一跳**混为一谈，于是后者被立刻返回、**把已连上的等代价备选遮住**，包在 `send_msg_internal` 的网关闸门处被丢——B/D 的兜底恰好在唯一有意义的场景里失效（比不做 B/D 更坏：表里明明有能走的路）。改为 `select_next_hop(my_id, candidates, probe)`：**T1 自发 / T2 本地有活连接优先，T3「无本地条目」仅作兜底**（只有调用方能用 `foreign_network_client.has_next_hop` 区分二者）。跨网路由行为与改动前一致，陈旧下一跳退化为改动前同样的「告警 + 丢弃」。`probe` 做成惰性回调以保住每包热路径的短路（有用例断言只探测 1 次）。单测 5 条 → 9 条（含这条回归），全量 1011 项通过。 |
