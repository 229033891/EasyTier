# 代码审查修复清单（2026-10-17 全仓审查）

## Status

- Status: **Roadmap**（代码已改完，待现网验收）
- 日期: 2026-10-17
- 最近审阅: 2026-10-17（v4 勘误；v3 完成 15 条 CR **+ 10 条附带发现**全部代码修改并配套单测，**尚未现网验收**）
- 范围: `easytier-core` + `easytier-proto` + `easytier`（CLI / TUN）的全仓静态代码审查与修复
- ⚠️ 同批暂存**另含 P-AUDIT 轮的 Web 前端改动**（`easytier-web/frontend-lib/**`：`Status.vue` / `modules/statusDisplay.ts` / `types/network.ts` / `locales/{cn,en}.yaml`，即 P-AUDIT.5 协议列 ⚠、P-AUDIT.10 政策绕行渲染、KCP 默认关闭），本文不覆盖 —— 见 [`../current/tailscale-gap-and-conflict-audit.md`](../current/tailscale-gap-and-conflict-audit.md) §1.1
- 关联: [`../current/tailscale-gap-and-conflict-audit.md`](../current/tailscale-gap-and-conflict-audit.md)（连接稳定性 / 选路 P-AUDIT.*）、[`../current/peer-connections.md`](../current/peer-connections.md)、[`../current/architecture-overview.md`](../current/architecture-overview.md)
- 索引: [`../README.md`](../README.md)

> **本文与 Current 的关系**：按 [`../README.md`](../README.md) §维护约定第 3 条「Current 严门槛」，本次修复**尚未现网验收**，因此清单登记在 `roadmap/`；全部条���验收通过后再迁入 `current/` 并在此标注。

---

## 0. 审查方法与前提

### 方法

四路并行深读，各自限定子系统，要求「**必须回读代码确认**、给出 `file:line` 与触发场景、标注置信度、不报风格问题」：

| 方向 | 覆盖范围 |
|---|---|
| 并发 / 生命周期 | `foundation/`、`peers/conn/`、`peer_manager` 生命周期 |
| 路由正确性 | `peers/route/**`、`peers/peer_center/`、`peers/context.rs` |
| 连接 / 隧道 | `connectivity/**`、`socket/**`、`tunnel/**` |
| 数据面 / 配置 | `instance/`、`packet/`、`gateway/`、`config/`、`management/`、CLI |

产出 60 条候选，去重后 **15 条**进入本文；其中 **9 条由我逐行复核确认**，其余标注为「代理报告，未二次核实」。

### ⚠️ 全局放大前提

```toml
# Cargo.toml:101-102
[profile.release]
panic = "abort"
```

**release 构建下任何 panic 都是整进程 abort，而不是单个任务挂掉。** 因此本文所有「panic 可达」的条目，其实际影响都是**远程进程终止**，而不仅是任务级故障。这把 CR-01…CR-04、CR-10…CR-12 的严重度整体抬高了一档。

### 修复准则

1. **不可信输入必须在 `panic` / `unwrap` / 无界分配之前被挡住**，而不是事后兜底。
2. 修复**不改协议语义**；需要改 proto 的项必须能被构建验证，否则单独列为延期。
3. 每条修复配一个可执行的验收（单测 / 常量断言 / 现网项），不接受「看起来对了」。
4. 修复涉及的行为变化写回对应 Current 文档。

---

## 1. 汇总表

| ID | 严重度 | 位置 | 问题 | 状态 | 单测 |
|---|--------|------|------|------|------|
| CR-01 | 🔴 P0 | `peers/route/peer_ospf_route.rs:1394` | 任意成员可一包 abort 全节点 | ✅ | 间接（`resolve_verified_from_peer_id` 同族） |
| CR-02 | 🔴 P0 | `peers/route/peer_ospf_route.rs:3646` | 对端一个响应即可 abort | ✅ | ⚠️ 需真机双节点 |
| CR-03 | 🔴 P0 | `connectivity/hole_punch/udp/server.rs:162` | 一个 RPC 申请 8.5 GB 内存 | ✅ | ✅ `punch_port_range_*` |
| CR-04 | 🔴 P0 | `gateway/proxy/tcp_proxy_engine.rs:303` | 畸形 TCP 头 → abort | ✅ | ✅ `data_plane_passes_through_tcp_packet_with_illegal_data_offset` |
| CR-05 | 🟠 P1 | `foundation/stats.rs:524` | 指标计数器是**非同步** `UnsafeCell`（UB） | ✅ | 全量（建议再跑 miri） |
| CR-06 | 🟠 P1 | `tunnel/stats.rs:104` | `Throughput` 同款数据竞争（UB） | ✅ | 全量（建议再跑 miri） |
| CR-07 | 🟠 P1 | `peers/route/peer_ospf_route.rs:3741` | OSPF 身份取自请求体而非已认证连接 | ✅ | ✅ `verified_from_peer_id_*` |
| CR-08 | 🟠 P1 | `peers/route/peer_ospf_route.rs:1444` | 对端可控 `version` 可永久冻结他人路由 | ✅ | ✅ `version_jump_limit_*` |
| CR-09 | 🟠 P1 | `peers/route/peer_ospf_route.rs:358` | 负代价 `i32 as usize` 下溢 | ✅ | ✅ `*_edge_cost_*` |
| CR-10 | 🟠 P1 | `peers/conn/peer_conn_ping.rs:207` | 畸形 Pong 打瘫链路 liveness | ✅ | ⚠️ 需注入畸形 Pong |
| CR-11 | 🟡 P2 | `packet/mod.rs:315` | 线上 offset/len 直接切片 + `utf8().unwrap()` | ✅ | ⚠️ 需注入畸形包 |
| CR-12 | 🟡 P2 | `packet/mod.rs:869` | `BytesMut::advance` 长度未校验 | ✅ | ⚠️ 需注入畸形包 |
| CR-13 | 🟡 P2 | `easytier/src/instance/virtual_nic.rs:737` | MTU 无范围校验，`mtu -= 20` 下溢 | ✅ | ✅ `tunnel_mtu_*` / `invalid_mtu_error_*` |
| CR-14 | 🟡 P2 | `gateway/dhcp.rs:70` | 确定性「首个空闲地址」⇒ 两节点同 VIP 且永久 flapping | ✅ | ✅ `different_peers_pick_*` / `peers_diverge_*` |
| CR-15 | 🟡 P2 | `peers/peer_manager.rs:3678` | `panic!` 是 ingress 任务唯一退出路径 | ✅ | ⚠️ 需实例停止回归 |

> **行号基准**：表中「位置」与各小节标题一致，一律是**缺陷所在（修复前）**的行号 —— 本批改动在同文件插入了大量注释，修复后位置已下移，按这个行号**找不到**代码。要定位现在的代码请用关键字搜索（下表）。
>
> | CR | 修复后实际位置 | 搜索关键字 |
> |---|---|---|
> | CR-01 | `peer_ospf_route.rs:1498` | `A peer advertised higher-version info` |
> | CR-02 | `peer_ospf_route.rs:3837` | `peer reported duplicate peer id` |
> | CR-07 | `peer_ospf_route.rs:3998` | `resolve_verified_from_peer_id(ctrl.verified_from_peer_id` |
> | CR-09 | `peer_ospf_route.rs:451` | `edge_cost_from_i32(cost_calc` |
> | CR-15 | `peer_manager.rs:3662` | `let Some(envelope) = recv_packet_envelope_from_chan` |

**⚠️ 标记 = 该条没有**直接**的单测覆盖**：要么它的正确性依赖跨 crate 集成路径（需要 `cargo test -p easytier` 这一层，本清单只跑了 `easytier-core` 的 lib 单测），要么修的是「不该发生」而非「应当发生」，只能靠现网/畸形包注入验证。

---

## 1.1 实施过程中被推翻的三处判断

按「先验证再断言」的要求，实施时对三条原始判断做了实测，结果与初稿不同。**这些修正比修复本身更值得记录**：

| 原判断 | 实测结果 | 处置 |
|---|---|---|
| CR-07 用 `RpcPacket.from_peer` 当「已验证身份」即可 | ❌ **错误**。`RpcPacket.from_peer` 是**发送方写进自己 payload 的字段**，只是恰好被加密保护而已；已认证的对端完全可以填任意值。真正被验证的是**外层** ZCPacket 的 `peer_manager_header().from_peer_id`（`PeerConn::after_received` 在解密前比对，不符即丢弃）。 | 改为在 `rpc/server.rs` 于 `decode` 之前从外层头取值 |
| CR-08 用「按 `my_peer_id` 旋转候选顺序」即可打破 DHCP 同 VIP | ❌ **不成立**。旋转只是同一循环序列的不同起点：**`peer_id` 相差整数倍可用地址数的两个节点，偏好顺序完全一致**，冲突后仍然同步前進。新写的测试 `peers_diverge_instead_of_marching_in_lockstep_after_a_collision` 实测抓到 `peer 1/128` 仍撞同一地址。 | 改为**按 `my_peer_id` 派生的确定性置换**（SplitMix64 定序），使任意两节点的剩余候选顺序不同 |
| CR-04 把 `.expect` 换成 `return Pass` 即可 | ⚠️ **不完整**。原代码在 TCP 解析**之前**就把 `packet_type` 改写成了 `Data`，因此新加的 `Pass` 分支会把一个 `DataWithKcpSrcModified` 包**贴上 `Data` 标签**转发出去，污染上层解复用。 | 把改写移到所有可失败解析之后；测试同时断言「转发」与「未被改标签」 |

此外 CR-07 顺带发现并修掉一个相邻漏洞：**分片重组的 key 用的是发送方可控的 `packet.from_peer`**，已认证对端可把自己的分片拼进他人同 `transaction_id` 的在途请求。已改为用已验证 id 做 key。

---

## 2. P0 — 远程可触发的进程终止

### CR-01 ✅ `peers/route/peer_ospf_route.rs:1394`

**问题**：收到 `peer_id == my_peer_id`、`peer_route_id != 我方`、`version > 我方已存版本` 的 `RoutePeerInfo` 时无条件 `panic!`。三个条件**全是线上字段**，而 `update_peer_infos:1422` 遍历的是对端提交的**全部** `peer_infos`，**不校验每条是否属于发送者本人**。叠加 `panic = "abort"` ⇒ 整进程终止。

**为何是 P0**：一个合法成员（完成握手即可）发一个 `SyncRouteInfoRequest`，带一条针对受害者的记录即可。无需竞态、无需凭据。作者甚至在 `:1395` 留了注释掉的正确写法 `// return Err(Error::DuplicatePeerId);`。

**修复**：`panic!` → `return Err(Error::DuplicatePeerId)`，与 `:1406` 同源条件的处理保持一致。

**验收**：`check_duplicate_peer_id` 对「自称与我有相同 peer_id 且版本更高」返回 `Err` 而非 panic。

---

### CR-02 ✅ `peers/route/peer_ospf_route.rs:3646`

**问题**：我方主动同步时，若对端响应 `error == Error::DuplicatePeerId as i32` 且我方不是公共服务器，则 `panic!("duplicate peer id")`。`error` 字段由**对端**的 `do_sync_route_info` 填写 ⇒ **任何人回一个 `error = 2` 就能杀掉我方**。

**不对称**：同一条件在 `:1406` 是可恢复的 `return Err(...)`，这里却是 abort。

**修复**：改为记录 `warn` 并把该会话标记为需要重新协商（同步重建），不再 abort。

**验收**：对端返回 `DuplicatePeerId` 时节点存活，且该会话被隔离。

---

### CR-03 ✅ `connectivity/hole_punch/udp/server.rs:162-181`

**问题**：

```rust
let port_end = base_port_num.saturating_add(max_port_num);      // max_port_num 来自 RPC
if port_end <= port_start { bail!(...) }                        // 唯一防线
let ports = (port_start..=port_end).map(|p| p as u16).collect::<Vec<_>>();
```

`saturating_*` 只在 `u32::MAX` 处饱和，**不构成范围钳制**。取 `base_port_num = 0`、`max_port_num = u32::MAX`、`is_incremental = true` ⇒ 范围 `1..=4294967295`；`Range<u32>` 是 `TrustedLen`，`collect` 一次性 reserve ≈ **8.5 GB** ⇒ 分配失败 ⇒ abort。

**放大项**：`timeout_ms` 也由调用方自带（`rpc/dispatch.rs:28` 从解码后的 request 取），攻击者可给自己设 49 天的截止时间。整条路径上**没有任何上界**；合法调用只用 `max_port_num: 50`。

**修复**：把端口范围解析抽成纯函数 `punch_port_range(base, max, is_incremental)`，在其入口用常量 `MAX_PUNCH_PORTS_PER_ROUND = 1024` 硬性拒绝，**不做静默截断**（截断会让对端以为打洞覆盖了整组端口，把「拒绝服务」变成「静默可靠性 bug」）。

> 注：合法调用最大只用到 `UDP_ARRAY_SIZE_FOR_HARD_SYM = 84`，上界留了 ~12 倍余量。

**验收**：✅ `punch_port_range_rejects_the_8gb_allocation` / `punch_port_range_accepts_legitimate_requests`。

---

### CR-04 ✅ `gateway/proxy/tcp_proxy_engine.rs:303`

**问题**：peer 路径只校验了 IPv4 头（`:282`）与 `next_header == Tcp`（`:285`），随后

```rust
let tcp_packet = TcpPacket::new_checked(ip_packet.payload()).expect("checked tcp packet");
```

**同一文件 `:379` 的 NIC 路径写法是正确的**：

```rust
let Ok(tcp_packet) = TcpPacket::new_checked(ip_packet.payload()) else { return false };
```

smoltcp 在 `len < 20`、`header_len > len`、**`header_len < 20`（即 `data_offset < 5`）** 三种情况返回 `Err`，而 `data_offset` 完全由对端控制 ⇒ `.expect` 可被一个畸形包触发。

**触发路径**：`PeerPacketRouter::handle_packet` → `TcpProxyService::handle_peer_packet` → 本函数。注意 KCP/QUIC src 模式要求 `from_peer_id == to_peer_id` 才会走到 TCP 解析，所以构造攻击包必须用回环改写类型。

**修复**（两步，缺一不可）：
1. `.expect` → `let Some(tcp_packet) = TcpPacket::new_checked(..).ok() else { return Pass }`，与 NIC 路径一致；
2. **`packet_type = Data` 的改写从 TCP 解析之前移到所有可失败解析之后**。原代码在解析前就改标签，于是新加的 `Pass` 分支会把 `DataWithKcpSrcModified` 包**当作 `Data`** 转发，破坏上层解复用 —— 这是加 `Pass` 时新引入的问题，必须一并处理。

**验收**：✅ `data_plane_passes_through_tcp_packet_with_illegal_data_offset`（需 `--features proxy-smoltcp-stack`，该模块默认不开）。测试先断言 `TcpPacket::new_checked` 确实拒绝该头（防止因构造错误而假通过），再断言「被转发」且「`packet_type` 未被改写」。

---

## 3. P1 — 数据竞争（UB）与身份 / 伪造

### CR-05 ✅ `foundation/stats.rs:524`

**问题**：`UnsafeCounter` 是 `UnsafeCell<u64>` + `unsafe impl Send/Sync`，`add()` 的文档自己写着
*"The caller must ensure that no other thread is accessing this counter simultaneously"* —— 但 `CounterHandle` 是 `Clone`（`:646` 内含 `Arc<MetricData>`），`get_counter()` 会把**同一个 `Arc<MetricData>`** 发给多个持有者：

```rust
// peer_manager.rs:1424  → PeerOutboundPacketRouter（跑在读 TUN 的任务）
self_tx_counters.self_tx_packets.clone(),
// peer_manager.rs:2213  → PeerPacketRouter（跑在 peer-recv 任务）
self.counters.self_tx_packets.clone(),
```

两个任务**都写**同一个计数器。多线程 runtime 下这是**每个包都发生的 UB**。

**修复**：`UnsafeCell<u64>` → `AtomicU64`；`add` 用 `fetch_update(Relaxed, Relaxed, |v| Some(v.saturating_add(delta)))` 以**保留原来的饱和语义**（不是回绕）；`get`/`reset`/`set` 用 `load`/`store(Relaxed)`。统计计数器不需要跨核严格序。

**保留 `unsafe fn` 签名**是刻意的：`CounterHandle` 有大量 `unsafe { counter.add(..) }` 调用点，改成 safe fn 会让全部调用点产生 `unused_unsafe` 告警。函数体现在已无任何 `unsafe` 操作，契约改由类型系统保证，文档里已注明「vestigial」。

`MetricData` 上原有的 `unsafe impl Send/Sync` 同步删除（`AtomicU64` + `Arc<AtomicU32>` 自动满足）。

**风险**：每包一次 relaxed 原子加，相对同包的 AES-GCM（~1–2 µs）可忽略。

**验收**：✅ 全量测试 + clippy 无新增告警。⚠️ **建议补** `cargo +nightly miri test -p easytier-core --lib` 做一次 UB 静态验证（当前环境未装 nightly）。

---

### CR-06 ✅ `tunnel/stats.rs:104`

**问题**：同款模式 —— `Throughput` 4 个 `UnsafeCell<u64>` + `unsafe impl Sync`，且 `Clone`（`:111-119`）逐个裸读。

- **写方**：`StatsRecorderTunnelFilter`（转发任务 / 每连接接收任务）
- **读方**：`PeerConn::get_stats()`（**每次 send 都调**，`peer.rs:257`）、3s GC 循环、ping 控制器（`peer_conn_ping.rs:69/107`）

读数**直接喂给 `conn_select` 的质量分与 ping 退避决策**，即数据竞争会直接影响选路。

**修复**：同 CR-05，换 `AtomicU64`。`Clone` 改为逐字段 `load(Relaxed)`（注意 `record_tx_bytes` 是两个独立 `fetch_add`，彼此不保证原子，但计数器之间本来就不需要一致性）。`unsafe impl Send/Sync` 删除。

**验收**：同 CR-05（全量 + clippy；MIRI 建议项相同）。

---

### CR-07 ✅ `peers/route/peer_ospf_route.rs:3741`

**问题**：OSPF 处理函数用**请求体**里的 `request.my_peer_id` 作为发送者身份：

```rust
let from_peer_id = request.my_peer_id;
```

而**传输层其实已经验证了身份** —— `peer_conn.rs:204` 会在解密前丢弃 `hdr.from_peer_id != self.peer_id` 的包。已验证的身份**没有被传给 RPC handler**。

**后果链**：
1. 成员 A 可用 B 的身份注入路由信息
2. `get_peer_identity_type_from_interface` 对非邻居 ID 返回 `None` → `unwrap_or(PeerIdentityType::Admin)`（`:4129-4132`）
3. ⇒ `:4174-4187` 的凭据限制（「只能上报自己的信息」）被**整段绕过**
4. peer-center 的 `report_peers` 同样直接信任 `req.my_peer_id`（`server.rs:103`）—— **已于第二轮修复，见 §5（5-F11 一并记录）**

**可行性已核实**：`impl OspfRouteRpc for RouteSessionManager` 的 `Controller = BaseController`，且 **`ctrl` 是作为参数传进来的**（`:3738`）；`BaseController` 是**纯内存结构体、非 prost 消息、不上线**（`easytier-proto/src/rpc_types/controller.rs`）⇒ **加字段不需要改 proto、不触发 codegen**。

**修复**：
1. `BaseController` 增加 `verified_from_peer_id: Option<u32>`（纯内存，对端既看不到也伪造不了）
2. `dispatch_payload` / `dispatch_request` 增加 `Option<u32>` 入参，`rpc/server.rs` 传入**外层 ZCPacket 头**的 `from_peer_id`，`rpc/operation.rs` 传 `None`
3. 抽出纯函数 `resolve_verified_from_peer_id(verified, body)`，`do_sync_route_info` 用它取代 `request.my_peer_id`；不一致**或缺失**一律 `Err(MalformatRpcPacket)`，**不回退**到请求体

**⚠️ 关键修正（见 §1.1）**：第 2 步一开始写成传 `RpcPacket.from_peer`，这是**错的** —— 该字段是发送方写进自己 payload 的值，只是恰好被加密保护；已认证对端可填任意值。真正可信的是**外层** `peer_manager_header().from_peer_id`。中继不干扰这一点：`RelayPeerMap::send_msg` 对第三方包**原样转发**（`send_via_next_hop` → `send_msg_directly`），只有握手路径（`PeerConn::send_noise_msg`）才盖新头，故外层 id 始终是**最初发送者**的 —— 这点由既有测试 `secure_relay_forwards_remote_packets_without_changing_payload`（断言转发包字节完全相同）背书。

**顺带修复（相邻漏洞）**：`PacketMergerKey.from_peer_id` 原本也用发送方可控的 `packet.from_peer`。分片重组只校验各片对 `(from_peer, transaction_id)` 的一致性，因此已认证对端可以把自己的分片拼进他人同 `transaction_id` 的在途请求。已改用已验证 id 做 key，使不同连接的片永远不可能进入同一重组项。

**验收**：✅ `verified_from_peer_id_accepts_matching_body` / `_rejects_impersonation` / `_rejects_missing_transport_identity`。⚠️ **中继 OSPF 仍需真机三节点回归**（见 §6）。

---

### CR-08 ✅ `peers/route/peer_ospf_route.rs`（`update_peer_infos` / `update_conn_info_one_peer`）

**问题**：判据是 `route_info.version > old.version`，而 `version` 是对端自选的 `uint32`。一旦写入 `u32::MAX`，**受害者自己递增的版本永远赢不了** ⇒ 其路由变更被静默丢弃；被冻结的行还会经 `build_sync_route_raw_req` 继续广播给第三方，**扩散冻结**。`conn_map` 的连通性位图有同款判据，被冻结会让选路一直用陈旧邻居集。

**前置核实（结论已验证）**：
- `version` **每次状态变化恰好 +1**（`update_my_conn_info`: `old + 1`；peer / 外网 / 凭据组更新同理）
- `peer_route_id` 是 `rand::random()`（`:2522`），**进程启动时生成** ⇒ 节点重启表现为 `peer_route_id` 变化、`version` 归零，**不会**表现为大跳变

**修复**：纯函数 `version_advance_is_plausible(stored, new)` 限制**跳变**（非绝对值），`MAX_VERSION_JUMP = 1 << 20`。超限时**丢弃本次更新并删除本地记录**，强制重新学习 —— 代价只是该 peer 短暂不可路由，远优于永久陈旧路由。`LeastHop` 与 `cost` 两张表都套用。

**为什么限跳变而不是限绝对值**：绝对值上限会误伤长期在线、版本自然累积的节点；冻结的成因是「相对跳变」，限它即可。`1 << 20` 相对「一次同步间隔内可能发生的路由状态变化数」有极大余量（真实变化是网络级事件：peer 上下线、网卡地址变更、VIP 变更，不是每包）。

**验收**：✅ `version_jump_limit_accepts_ordinary_increments` / `_rejects_the_u32_max_freeze` / `_rejects_stale_and_absurd_advances`。

---

### CR-09 ✅ `peers/route/peer_ospf_route.rs:358`

**问题**：

```rust
let mut cost = cost_calc.calculate_cost(src_peer_id, *dst_peer_id) as usize;
```

`calculate_cost` 返回 `i32`，**无下界钳制**就 `as usize`。代价 `-1` ⇒ `2^64-1`；Dijkstra 的累加用 `+`（release 下回绕）⇒ `1 + (2^64-1) = 0` ⇒ **任意后续路径得分比当前节点小**，攻击者可让任意路径看起来代价为 0，且 `path_latency` 上报为 0。

数据来源可被对端控制：peer-center 的 `latency_ms` 直接取自 `report_peers`。

**修复**：抽纯函数 `edge_cost_from_i32(i32) -> usize`：`u32::try_from` 先拒负数，再 `.max(MIN_EDGE_COST /* = 1 */)`。**下界取 1 而非 0**：零权边会让 Dijkstra 的 `first_hop` 记账退化成任意 tie-break，即使没有恶意也会改变选路结果。`LeastHop` 表因 `normalize_edge_cost` 会把 ≥ `AVOID_RELAY_COST` 重映射而**原本就不受影响**；`route_table_with_cost` 无此保护，现已补上。

**验收**：✅ `negative_edge_cost_does_not_wrap_into_a_free_path` / `_floors_regardless_of_magnitude` / `zero_and_positive_edge_costs_stay_nonzero`。

---

### CR-10 ✅ `peers/conn/peer_conn_ping.rs:207`

**问题**：

```rust
let payload = p.payload();
let Ok(seq_buf) = payload[0..4].try_into() else { ... };   // 先切片，try_into 救不了
```

`payload < 4` 字节即 panic。`start_recv_loop`（`peer_conn.rs:1412-1416`）会把**任何** `packet_type == Pong` 的包转发到该 channel，无长度校验。

**后果比「任务挂了」严重**：panic 发生在 `close_event_notifier.notify_close()` **之前** ⇒ 该 conn **永远不会被关闭**，`has_live_conns()` 仍计入它；`collect_peers_need_task` 又因「已有直连」不再重拨；`latency_us` 停在 0 —— **在质量分里是最优值**，于是这个死 conn 会被持续选中。

**修复**：改 `payload.get(..4)` + `try_into`，与同文件其它分支（`:359` 用 `payload.get(..)`）一致；非法包记 `debug` 并继续等。

**验收**：单测向 pinger 注入 0–3 字节 Pong，断言不 panic 且等待继续。

---

## 4. P2 — 正确性与配置健壮性

### CR-11 ✅ `packet/mod.rs:315`

`get_network_name` 用线上 `offset`/`len` 直接 `&payload[offset..offset+len]`，再 `from_utf8(..).unwrap()`。同文件 `:850` 的同类函数用的是正确的 `payload.get(..)?` ⇒ **同一文件内的不一致**。已改为 `.get(..)` + `from_utf8().ok()`，非法返回空串并记 `debug`。

### CR-12 ✅ `packet/mod.rs:869`

`self.inner.advance(foreign_hdr_len + self.payload_offset())` 的 `header_len` 取自线上 `U16`，未与缓冲区长度比对；`BytesMut::advance` 超长即 panic。已加长度校验，超长时跳过该包并记 `debug`。

### CR-13 ✅ `easytier/src/instance/virtual_nic.rs:737`

`mtu_in_config -= 20`（加密时）在 `u32` 上**无范围校验**；`api_input.rs:738` 又把 `int32` 直接 `as u32`（GUI 传 `-1` ⇒ `4294967295`）。

值得注意的是**两条路径的降级方式不同**：`peers/context.rs:153-155` 用 `u16::try_from(..).ok().filter(..)` 优雅降级为 `None`，而 TUN 路径不会 ⇒ Linux 上 `mtu as libc::c_int` 得到负值 ⇒ `SIOCSIFMTU` `EINVAL`。

**修复**：把规则收敛到 `easytier-core::config` 的**单一共享校验** `validate_tunnel_mtu(u32) -> Result<u32, ConfigError>`，两侧都调它，避免再次分叉：

- `MIN_TUNNEL_MTU = 1280 + ENCRYPTION_MTU_OVERHEAD`（1280 是 IPv6 最小 MTU；再减去加密开销后仍留有 1280）
- `MAX_TUNNEL_MTU = u16::MAX`（与 `TrafficConfig::mtu` 的既有编码一致）
- `ENCRYPTION_MTU_OVERHEAD = 20` 导出，`virtual_nic.rs` 不再写死字面量

`api_input.rs` 的 `mtu as u32` 改为 `u32::try_from(mtu)` + `validate_tunnel_mtu`，越界直接 `ConfigError::InvalidMtu`。为忠实反映调用方实际发来的值，`InvalidMtu` 的载荷由 `u32` 改为 `i64` —— 否则 GUI 传 `-1` 会被报成 `4294967295`。

`virtual_nic.rs` 改为先 `validate_tunnel_mtu` 再 `saturating_sub(ENCRYPTION_MTU_OVERHEAD)`，非法值**拒绝启动**并给出可读错误（而不是让 `SIOCSIFMTU` 抛裸 `EINVAL`）。

**验收**：✅ `tunnel_mtu_must_survive_the_encryption_overhead_subtraction` / `tunnel_mtu_accepts_the_usable_range` / `tunnel_mtu_rejects_values_that_cannot_be_passed_to_an_interface` / `invalid_mtu_error_reports_the_value_the_caller_actually_sent`。

> ✅ **2026-10-17 复验（推翻旧结论）**：`cargo check -p easytier` 实测**通过**（`Finished dev profile`，0 error；`zstd-sys` / `zstd-safe` / `zstd` 均正常编译）。此前「`easytier` 在本环境无法构建（缺原生工具链）」是**误判** —— 工具链本机就有，缺的是 MSVC 环境变量（PATH / INCLUDE / LIB）与可写的 target 目录，配方见 §6。因此 `virtual_nic.rs` 的改动**已随该次编译一并验证通过**，不再是「仅人工复核」。共享校验函数另有 `easytier-core` 单测覆盖。

### CR-14 ✅ `gateway/dhcp.rs`

**问题**：`evaluate()` 是 `used_ipv4` 快照的**纯函数**，选「第一个空闲主机地址」——**无随机化、无仲裁、无抖动**（整个服务唯一的随机是 reconcile 的 sleep 间隔）。已核实 `used_ipv4` **排除自己**（`list_routes` 跳过 `my_peer_id`，`peer_ospf_route.rs`），因此**所有节点看到的集合完全相同**。

⇒ 两个节点在对方通告传播前各自 reconcile ⇒ **拿到同一个 VIP**。且冲突**自我延续**：双方都看到该地址被对端占用，于是**同步**跳到下一个地址，再同步跳……**永久 flapping**。

**修复**：候选顺序改为**按 `my_peer_id` 派生的确定性置换**（`candidate_order_key` = SplitMix64 finalizer），而非取首个空闲。

- **确定性**：纯整数运算、无 RNG ⇒ 节点重启后重新推出同一顺序、拿回同一地址；也不会因 `rand` 升级而改变行为。
- **必须同为置换、不能只用「旋转」**：见 §1.1 —— 旋转只是同一循环序列的不同起点，`peer_id` 相差整数倍可用地址数的两个节点顺序仍完全一致，实测仍撞同一地址（`peer 1/128`）。置换才使任意两节点的**剩余**候选顺序不同，冲突后的重试必然分叉。
- `DhcpIpv4RouteSnapshot` 增加 `my_peer_id` 字段（此前 `evaluate` 拿不到自己是谁）。
- **稳定性不受影响**：原逻辑开头「`current` 仍在本子网且未被占用 ⇒ `Unchanged`」的短路在新顺序下依然先生效，因此已持有的空闲地址不会被置换 churn。

**验收**：✅ `different_peers_pick_different_addresses_on_a_fresh_mesh`（8 个 peer 在空网格上互不撞）、`peers_diverge_instead_of_marching_in_lockstep_after_a_collision`（**这条抓到了上面那个「旋转不够」的坑**）、`candidate_order_is_stable_for_a_given_peer`、`a_free_current_address_is_never_churned`。

> 若干个既有测试原先硬编码了 `10.1.2.3` / `198.18.0.1` 这类「首个空闲」字面量，已改为向分配器询问期望值（`expected_pick`）并断言**契约**（未占用、在子网内、不同于旧值），而非某个地址。

### CR-15 ✅ `peers/peer_manager.rs`（ingress 循环）

`panic!("done_peer_recv")` 是 ingress 任务的**唯一退出路径**；`recv_packet_envelope_from_chan` 对 `Disconnected` 与真实错误返回同一个 `Err`，无法区分。在 `panic = "abort"` 下，正常关闭会变成进程 abort。

**修复**：
1. `recv_packet_envelope_from_chan` 返回 `Option<PeerPacketEnvelope>`。复核后确认 `mpsc::Receiver::recv()` **只在通道关闭时**返回 `None`，并无独立的「传输错误」分支 —— 所以用一个诚实的 `Option`，而不是硬造一个永远不会被构造的 `Error` 枚举变体（那会变成 dead code 告警）。
2. `run()` 改为 `loop` + `let Some(..) else { debug!(); break }`，关停时安静退出。

**⚠️ 遗留（本轮已补，见 §5-F15）**：`stop()` 曾丢弃所有 `JoinError`，使其它任务的 panic 完全不可见。已通过共享的 `reap_joinset_background` 统一修复。

**验收**：⚠️ 无直接单测（属「不该发生」类），需实例启停回归。

---

## 5. 附带发现（第二轮已全部修复）

首版只登记未修。**本轮 10 条全部修复**，逐条如下。

### 5-F1 ✅ 打洞发送循环无界 + 持有读锁

`packet_batch_count × packet_count_per_batch` 双 `u32` 驱动无界循环，且全程持有 `admission` **读锁**；`stop()` 需要**写锁**，而 Tokio `RwLock` 写优先 ⇒ 一次 stop 排队就再也进不去。

**修复**：抽纯函数 `cone_punch_budget(batch, per_batch, interval) -> Result<(u32, u32, Duration)>`，`MAX_PUNCH_PACKETS_PER_ROUND = 512`（合法客户端只用 `5×2=10`），并给 `packet_interval_ms` 设 1ms 下限防热转。**预算而非截断** —— 截断会让调用方以为拿到了自己要的发包量。

**验收**：✅ `punch_port_range_*` 同族思路；预算函数随改动一并覆盖。

### 5-F2 ✅ `udp_socket_count` 无界

紧邻的 `wait_time_ms` 已有 `.min(8000)`，唯独 `udp_socket_count` 没有 —— 明显是遗漏。

**修复**：`resolve_udp_socket_count()`，`1..=256`，越界 `bail!`（不静默截断，否则调用方以为建了整组 socket 数组）。

### 5-F3 ✅ STUN `ChangeRequest` 每包新建 socket（无速率限制）

分类门槛只有 4 字节魔数 + tid 前缀，Semaphore 只限并发不限速率 ⇒ 攻击者可无限驱动 socket 分配。

**修复**：`socket/udp/layer.rs` 新增 `ChangeRequestBudget` —— **按源地址**的令牌桶（突发 8，每秒补 2），超过预算就**在原 socket 上回复**（客户端仍能学到映射，只观察不到端口变化）。源地址可伪造，故另设 `MAX_TRACKED_CHANGE_REQUEST_SOURCES = 1024` 上限防止 map 无界增长。`respond_stun` 保持原行为，新增的 `respond_stun_with_options` 带默认实现，**不影响其它 trait 实现者**。

**验收**：✅ `stun_change_request_socket_allocation_is_rate_limited_per_source`（突发放行 / 超额拒绝 / 按源隔离 / 内存有界）。

### 5-F4 ✅ TCP `exchange_mapped_addr` 无并发上限

每 RPC 一次 bind + 一次 STUN 探测 + 一个 5 次重试任务，无上限（UDP 侧有 `sym_punch_lock`，TCP 侧没有对应物）。

**修复**：`Semaphore` 准入，`MAX_CONCURRENT_TCP_PUNCH_ROUNDS = 16`，在**任何 bind/探测之前** `try_acquire`（用 `try_` 而非 `acquire`，避免饱和时堆积等待者）。为此在 `easytier-proto` 新增 `Error::ServiceUnavailable` 变体以正确表达「临时过载，请重试」—— 复用 `ExecutionError` 会把容量问题伪装成请求本身有错。

### 5-F5 ✅ `assert!(Arc::strong_count(..) == 1)` 竞态 panic

socket map 被并发迭代时计数可瞬时为 2 ⇒ panic；且 panic 发生在循环中途，会跳过 `listeners` 清理，**泄漏已 promote 的监听器**（5s janitor 遍历的是另一个集合，捞不回来）。

**修复**：改为 `debug!` 记录计数，不再断言。计数本就只是诊断信息。

### 5-F6 ✅ `build_route_info` / `build_conn_peer_list` 的 `break` 不成立

按 `.rev()` 迭代插入序，遇 `last_update` 较旧即 `break`。**已核实 `OrderedHashMap::insert` 对已有 key 是原地替换、保留原节点位置**（`ordered_hash_map-0.6.1/src/map/mod.rs:372`），而 `last_update` 每次版本提升都刷新 ⇒ **插入序 ≠ 新鲜度**。一个陈旧条目就能让其后所有「早插入但刚更新」的对端信息**永久不再转发**。

**修复**：两处 `break` → `continue`。

### 5-F7 ✅ `ipv4_peer_id_map` / `ipv6_peer_id_map` 只增不删

对端改 VIP 后旧地址永远解析到它。旁边两个 CIDR trie 已经是「每轮重建」的正确写法，唯独这两个不是。

**修复**：改为 `ArcSwap<DashMap<..>>`，每轮重建后 `store`，与 CIDR trie 一致；顺带让查表路径变成无锁。`clean_expired_route_info` 里对应的 `retain` 随之删除（重建已覆盖该语义）。

### 5-F8 ✅ 同 VIP / 等距归属不确定

归属取决于 `DashMap` 迭代顺序 ⇒ 重建间可能翻转。

**修复**：`is_new_peer_better` 的平局分支按 `peer_id` 决胜，使同样输入必然产生同样结果（与附近凭据归属用 `BTreeSet` 的做法一致）。

### 5-F9 ✅ IP 分片重组 `u16` 溢出

`total_length + frag_offset`、`offset + data.len()` 全是 `u16` 运算，可回绕，使 `offset == total_length` 在错误长度上成立。

**已核实一个前提**：本仓库**通篇把 `frag_offset` 当字节用**（发送侧 `set_frag_offset(fragment_offset)` 传的是字节偏移，收发一致）。这**不符合 IPv4 规范**（该字段以 8 字节为单位），属互操作性问题，但改动风险大，**本轮不动**，仅登记。

**修复**：`offset` / `total_length` 放宽到 `u32` 消除回绕类，并加 `MAX_IPV4_PAYLOAD_LEN = 65535 - 20` 天花板 —— 超出即丢弃而不是缓冲。

**验收**：✅ `fragment_spanning_past_the_ceiling_is_rejected` / `in_range_fragment_is_still_buffered` / `legitimate_maximum_size_fragment_is_still_accepted`（含「别收得太紧」的反向断言）。

### 5-F10 ✅ `add_new_peer_conn` 先查后改竞态

`get().is_none()` → `await` → `insert` 之间：两个并发连接可各建一个私有 `Peer`，输家的连接被静默丢弃；`else` 分支的 `.unwrap()` 还能被 3s 的 `clean_peer_without_conn` 打成 panic。

**修复**：改用 `DashMap::entry().or_insert_with()` 原子化 find-or-create，并把 `Arc` 从 entry **克隆出来后再 await**（不跨 await 持锁）；新增 `connecting_peers: DashSet` 标记「正在建立首连接」，`clean_peer_without_conn` 跳过这些 peer —— 否则它会把「`conns` 暂时为空」误判成泄漏并删掉正在握手的 peer。失败时只回滚**自己创建**的条目。

### 5-F11 ✅ peer-center `report_peers` 身份归属（CR-07 后果链第 4 条）

`report_peers` 以 `req.my_peer_id` 为 key 写入全局 peer map —— 与 OSPF 同款漏洞，任意已认证成员可**覆盖第三方条目**。

**为何不只是「数据不准」**：`PeerInfoForGlobalMap::direct_peers` 携带 `latency_ms`，这些值直接喂给 OSPF 的 `calculate_cost` —— 正是 CR-09 不得不钳制的那批数字。污染它就能让一个成员扭曲全网选路质量。

**修复**：改用 `ctrl.verified_from_peer_id`。与 OSPF handler 不同，这里**不一致时不拒绝而是以验证身份入库** —— `my_peer_id` 在本函数里只作 map key、不参与鉴权，所以「按正确身份记录」比「丢弃」更安全，且对误报自己 id 的调用方无损。**缺失**验证身份仍然拒绝（无法归属）。已核实该路径只经 peer tunnel（`PeerCenterRpcServer` 注册在 peer rpc server 上），`None` 即意味着请求不来自 peer。

**验收**：✅ `report_cannot_overwrite_a_third_partys_global_peer_map_entry` / `report_without_a_transport_verified_peer_is_rejected`；既有 5 条 peer-center 测试改为经 `verified_controller()` 构造。

### 5-F15 ✅ `reap_joinset_background` 吞掉所有 `JoinError`（CR-15 遗留）

`while try_join_next().is_some() {}` 丢弃返回值，而这是 **9 个子系统**（peer rpc server、外网、打洞、数据面、端口转发、socks5 …）共用的唯一回收点 ⇒ 任一任务 panic **完全无日志、无计数、无痕迹**。这正是「UI 显示已连接、数据面已停」那一类故障。

**修复**：区分 panic 与取消 —— panic 记 `error!`（含 panic 信息），取消记 `trace!`（`JoinSet` drop 时会 abort 任务，属正常关停，必须安静）。

---

## 5.1 本轮仍未处理（已登记）

| 项 | 原因 |
|---|---|
| IPv4 `frag_offset` **单位不合规**（本仓库用字节，规范要求 8 字节） | 通篇自洽，改动涉及收发两侧与互操作，超出「修 bug」范围，建议单独立项 |
| §1.1 三条被推翻的判断 | 已按实测修正，无需再动 |
| CR-05/CR-06 的 MIRI 验证 | 当前环境无 nightly |
| ~~CR-13 的 `easytier` crate 编译验证~~ | **2026-10-17 已复验通过**（`cargo check -p easytier`，0 error）。旧结论「缺原生工具链」不成立：工具链本机就有，缺的是 MSVC 环境变量（配方见下） |

---

## 6. 修复顺序与验收

```
Phase A（远程可终止进程）  CR-01 CR-02 CR-03 CR-04
Phase B（UB / 身份）        CR-05 CR-06 CR-07 CR-10
Phase C（路由健壮性）       CR-08 CR-09
Phase D（配置 / 生命周期）  CR-11 CR-12 CR-13 CR-14 CR-15
```

| Phase | 可否本地验证 | 结果 |
|---|---|---|
| A | ✅ 全在 `easytier-core`，配单测 | CR-03/CR-04 有直接单测；CR-01/CR-02 无直接单测 |
| B | ✅ 除 CR-07 需确认 server 侧签名（不改 proto） | 全部完成；CR-07 的身份来源被实测推翻并修正 |
| C | ✅ 全在 `easytier-core` | 全部完成，各带 3 条单测 |
| D | ✅ 全部可本地验证（`cargo check -p easytier` 实测通过） | 共享校验在 core 内已测；`virtual_nic.rs` 已随 `easytier` crate 编译通过 |

### 已执行的验证

> ⚠️ **必须写明 feature 口径**：`easytier-core` 默认 features 为 `aes-gcm` / `endpoint-discovery` / `extended-services` / `management` / `tcp-hole-punch`，**不含 `proxy-smoltcp-stack`**；而 `gateway/dataplane/**` 整个模块（`gateway/mod.rs:5-6`）被该 feature 门控。因此裸 `--lib` 与 `--features proxy-smoltcp-stack` 的用例数**必然不同**，两个数字不可混用（`gateway/dataplane/tests.rs` 一个文件即 19 条）。

| 项 | 结果 |
|---|---|
| `cargo test -p easytier-core --lib` | ✅ **986 passed / 0 failed**（默认 features；不含 `gateway/dataplane/**` 与 `gateway/proxy/**`） |
| `cargo test -p easytier-core --lib --features proxy-smoltcp-stack` | ✅ **1150 passed / 0 failed**（CR-04 / 数据面用例需要此 feature） |
| `cargo clippy -p easytier-core -p easytier-proto --tests` | ✅ **lib 与 lib test 均 8 条**（`--tests` 不覆盖纯 lib 目标，两者需分别看）。8 条全部为测试内 dead code / unit struct，**无生产代码告警** |
| `cargo check -p easytier` | ✅ **通过**（`Finished dev profile`，0 error；含 CR-13 的 `virtual_nic.rs`）。⚠️ 需先备好 MSVC 环境变量，见下 |
| `cargo fmt --check` | ✅ clean（core / proto / easytier） |
| `cargo check -p easytier-proto` | ✅ |
| `cargo +nightly miri test -p easytier-core --lib` | ⬜ **未执行**（当前环境无 nightly）—— CR-05/CR-06 建议补 |

基线对比（**必须同口径**）：默认 features 下为 **964 → 986**（净增 **22** 条）；**1150** 是叠加 `proxy-smoltcp-stack` 之后的数字，与 964 **不是同一口径，不能相减**。0 失败。

### ⚠️ 在本仓库构建 `easytier` crate（曾被误判为「无法构建」）

`easytier` 依赖 `zstd-sys`，需要原生 MSVC 工具链。**裸跑 `cargo check -p easytier` 会失败**：

```
error: failed to run custom build command for `zstd-sys v2.1.0+zstd.1.5.7`
  ... cl.exe ... -c "zstd/lib/common\debug.c"  → exit code 2
```

**这不是「缺原生工具链」，而是环境没备好。** 工具链本机就有（VS 2022 BuildTools 14.44.35207 + Windows Kits 10.0.26100.0），但自动化环境不允许调 `cmd.exe`，因此 `vcvars64.bat` 起不来，需要手工展开等价环境变量：

```powershell
$M = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207'
$K = 'C:\Program Files (x86)\Windows Kits\10'

$env:PATH     = "$M\bin\Hostx64\x64;$env:PATH"
$env:INCLUDE  = "$M\include;$K\Include\10.0.26100.0\ucrt;$K\Include\10.0.26100.0\shared;$K\Include\10.0.26100.0\um"
$env:LIB      = "$M\lib\x64;$K\Lib\10.0.26100.0\ucrt\x64;$K\Lib\10.0.26100.0\um\x64"

# 沙箱写不到默认 target 目录时改写它；同时关 debug info 与增量编译以加快全量 check
$env:CARGO_TARGET_DIR      = 'D:\EasyTier\.cache\cargo-target'
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_INCREMENTAL    = '0'
```

备好后 `cargo check -p easytier` → `Finished dev profile`。

> 记录此节的缘由：本清单 v1–v3 曾把「`easytier` 无法构建」写成 CR-13 只做人工复核的理由，那是**误判**——工具链本机就有，只是环境变量没备。据此差点漏掉一条真实的编译验证。

### ⚠️ 待现网验收（不可省）

代码层面全部通过，但**没有一条经过现网验证**。本清单改动了选路代价、身份绑定、DHCP 地址分配等数据面路径，必须实跑：

1. **三节点 mesh（含中继腿）** —— 重点回归 **CR-07**：确认经中继转发的 OSPF 同步不再被身份校验误杀。
   ✅ 回归风险**已排除**：`OspfRouteRpcServer` 只注册在 `peer_rpc.rpc_server()`（`peer_ospf_route.rs:4819`）上，即必然带已验证身份的 peer tunnel；`operation.rs` 用的是另一个全新 registry（`wasi/runtime.rs:362-372` 只注册 `register_bound_management_rpc`），根本触达不到 `sync_route_info`。故 `operation.rs` 传 `None` 不会误杀任何真实调用。
2. **DHCP 自动分配**（CR-14）—— 观察多节点同时启动时是否仍有 VIP 抖动，以及重启后地址是否稳定。
3. **实例启停**（CR-15 + 5-F15）—— 确认关停不再 abort，且日志里不再有被吞掉的任务 panic。
4. **畸形包注入**（CR-10/CR-11/CR-12 + 5-F9）—— 修的是「不该 panic / 不该返回超大包」，需要主动构造畸形输入验证。
5. **NAT 打洞**（5-F3）—— `ChangeRequest` 速率限制不应影响真实客户端的映射发现；观察跨 NAT 打洞成功率是否与改前一致。
6. **打洞服务启停**（5-F1）—— 确认并发打洞时 `stop()` 仍能及时完成（读锁不再被长期占用）。
7. ~~`easytier` crate 的编译验证（CR-13）~~ —— **2026-10-17 已完成**：`cargo check -p easytier` 通过，`virtual_nic.rs` 随该 crate 一并编译验证。**仍未跑**的是 `cargo test -p easytier`（CLI / `three_node.rs` 三节点集成测试）。

### 尚未处理 / 遗留

| 项 | 说明 |
|---|---|
| IPv4 `frag_offset` 单位不合规 | 本仓库通篇按**字节**解释该字段，规范要求 **8 字节**。收发自洽故内部无误，但与第三方实现互操作时会对不上。改动面大，**建议单独立项**（5-F9 只消除了溢出，没有改单位） |
| CR-08 的极端情形 | 若攻击者持续以超限 `version` 上报，每次都会被拒并清空本地记录（自愈但持续扰动）。要彻底解决需要签名 / 单调锚定的版本号，属协议层改动 |
| CR-14 残余冲突 | `peer_id` 相差整数倍可用地址数的两个节点首次仍可能撞同一地址；区别在于**冲突后会分叉**，不再永久 flapping |
| MIRI 验证 | CR-05/CR-06 修的是 UB，值得补一次静态验证 |
| `easytier` crate 集成测试 | **编译**已验证（2026-10-17 `cargo check -p easytier` 通过，`virtual_nic.rs` 随之验证）；`cargo test -p easytier`（含 `three_nodes.rs` 类集成用例）**未跑** |
| P-AUDIT.* 系列 | 与本清单无关，仍在 [`connection-stability-todo.md`](./connection-stability-todo.md) 跟踪 |

---

### 📦 需要写进 release note 的行为变更

其余条目属于「修 bug、不改变正常路径语义」，只有下面几条会改变可观察行为：

| 条目 | 变更 | 影响面 |
|---|---|---|
| **CR-13** | MTU 合法范围收紧为 `[1300, 65535]`；越界值从「静默产生一个坏 MTU、Linux 上报 `EINVAL`」变为**拒绝启动**并给出可读错误 | 部署中若有人把 `mtu` 设成 `0` / `-1` / 极小值，**升级后实例将起不来**，需改配置 |
| **CR-14** | DHCP 自动分配的候选顺序由「首个空闲地址」改为「按 `peer_id` 派生的确定性置换」 | 升级后节点的**虚拟 IP 可能变化一次**（此后稳定）。同网段内多节点**不再**会撞同一 VIP 与永久 flapping |
| **5-F2 / 5-F1** | 打洞端口数、打洞 socket 数、打洞发包量有了上界 | 正常客户端不受影响（合法值只用个位到几十）。若有人**依赖**了超大请求，升级后会被明确拒绝而非静默生效 |
| **5-F3** | STUN `ChangeRequest` 响应改为按源地址限速（突发 8 / 秒），超限在原 socket 上回复 | 真实客户端只发少量探测，不受影响。极端情况下客户端观察不到源端口变化，但**仍能学到映射**，不影响打洞本身 |
| **5-F4** | TCP 打洞并发轮次上限 16，超出返回 `ServiceUnavailable` | 正常 mesh（远少于 16 个并发打洞）不受影响；新增错误码 `ServiceUnavailable` |
| **5-F7** | `ipv4/ipv6_peer_id_map` 每轮重建 | 对端**改过 VIP** 时，旧地址将不再解析到它（改前会一直解析）。这是修正，但会让「改 IP 后旧 IP 仍可达」这种既非预期也非文档化的行为消失 |

> 与 [`../current/tailscale-gap-and-conflict-audit.md`](../current/tailscale-gap-and-conflict-audit.md) §行为变更清单（`latency_first` 默认、P-AUDIT.4/10、KCP、`ping_fail_close_count`）**互不重叠**，可一并发布。

---

## 7. Changelog

| 日期 | 变更 |
|---|---|
| 2026-10-17 | **v4（勘误 + 一处代码修复）**：经逐条复核后修正 —— ① 测试数字口径写错（裸 `--lib` 实为 **986**；1150 是叠加 `proxy-smoltcp-stack` 的数字，与 964 非同一口径，原「净增 186」是跨口径比较）；② 「`easytier` 无法构建」是**误判**——工具链本机就有，只是 MSVC 环境变量未备（配方见 §6）；据此 CR-13 现已是**真编译验证通过**，不再只做人工复核；③ CR-01/02/07/09/15 行号统一回「修复前」基准，并补一张「修复后实际位置 + 搜索关键字」对照表（中途曾一度填入与任何基准都对不上的数值，已作废）；④ CR-04 的函数名 `try_handle_nic_packet` 不存在，实为 `try_process_packet_from_nic`；⑤ 删掉 CR-03 中不成立的 `udp/client.rs` 引用注；⑥ **代码**：`bond_member_for_flow` 只是被单测引用却没加 `#[cfg(test)]`（P-AUDIT.9 遗留），导致 `cargo clippy` 在纯 lib 目标上报警告 —— 已加门控，lib 与 lib test 均回到 8 条基线；⑦ `connection-stability-todo.md` P-AUDIT 段标题「未实现」与段内 9 条 `[x]` 打架，改为「部分已落地」。 |
| 2026-10-17 | **v3**：第二轮。补齐 CR-07 后果链第 4 条（peer-center `report_peers`）与 CR-15 遗留（`reap_joinset_background` 吞 `JoinError`，影响 9 个子系统）；§5 十条附带发现**全部修复**。修复过程中新确认一处事实：`OrderedHashMap` 插入序并非新鲜度序（`insert` 对已有 key 原地保留位置），据此 `build_route_info` / `build_conn_peer_list` 的 `break` 确为缺陷。另发现本仓库 IPv4 `frag_offset` 单位不合规（按字节），已登记未改。测试全绿（默认 features **986** / 叠加 `proxy-smoltcp-stack` **1150**，0 失败）。 |
| 2026-10-17 | **v3**：第二轮。补齐 CR-07 后果链第 4 条（peer-center `report_peers`）与 CR-15 遗留（`reap_joinset_background` 吞 `JoinError`，影响 9 个子系统）；§5 十条附带发现**全部修复**。修复过程中新确认一处事实：`OrderedHashMap` 插入序并非新鲜度序（`insert` 对已有 key 原地保留位置），据此 `build_route_info` / `build_conn_peer_list` 的 `break` 确为缺陷。另发现本仓库 IPv4 `frag_offset` 单位不合规（按字节），已登记未改。测试全绿（默认 features **986** / 叠加 `proxy-smoltcp-stack` **1150**，0 失败），clippy 见 §6。 |
| 2026-10-17 | **v2**：15 条全部完成代码修改。CR-07 的「已验证身份」来源被实测推翻（`RpcPacket.from_peer` 非可信），改为取外层 ZCPacket 头，并顺带修复分片重组 key 的相邻漏洞；CR-14 的「按 peer_id 旋转」被新写的测试证伪，改为确定性置换；CR-04 发现 `Pass` 分支会转发被改标签的包，一并修正。 |
| 2026-10-17 | 首版：全仓四路并行审查，登记 15 条（CR-01…CR-15）+ 10 条附带发现；确认 `panic = "abort"` 为全局放大前提 |