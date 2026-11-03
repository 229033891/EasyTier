# 隧道出口公平队列（Egress Fair Queue）变更说明

**Status: Current** ｜ 最近审阅：2026-11-03 ｜ 关联：`docs/current/p2p-direct-stability-audit.md`、`peers/forward_egress.rs`（中转侧 per-dst lane，本变更的前置参照）、[`relay-hop-penalty.md`](./relay-hop-penalty.md)（同版放开的发版级升级说明 §7）

---

## 0. 解决什么问题

RDP 等交互式应用对**尾延迟**敏感。ET 的选路层（质量优先 + `relay_hop_penalty`）和连接层（conn_select 迟滞、bond flow 级分流）已经保证交互流走最优路径，但**隧道出口的发送队列是纯 FIFO**，大流量（文件传输、备份）会把交互小包压在队列尾部。本变更把出口调度从「单 FIFO」改为「按流公平」，并**缓解**（非消除）源头侧的串行发送桥——单流打满其 worker 通道后读循环仍会暂停，见 §2.3 的量化边界。

### 0.1 既有设施与本变更的边界

`peers/forward_egress.rs` 已解决**中转流量的跨目的地**队头阻塞（per-`dst_peer_id` lane + 独立 drain 任务）。本变更解决它没覆盖的两层：

1. **同连接内**：目的相同的多条流共享一个 32 槽 FIFO ring（`tunnel/mpsc.rs`），慢隧道上 bulk 流占满 ring 时，交互流最多被拖一个 `EGRESS_WAIT`（100ms，`peer_conn.rs` `send_msg`）粒度；
2. **源头串行桥**：TUN 读取（`virtual_nic.rs` `do_forward_nic_to_peers_task`）和 smoltcp→peer 桥（`gateway/dataplane/stack.rs`）都是**单任务串行** `read → send().await` 循环——任何一个 conn 的 ring 满会把整个源平面的所有流一起卡住。

---

## 1. 修改了什么

| 项 | 内容 |
|---|---|
| 新 flag | `enable_egress_fair_queue: bool`（**默认 true**），关闭后 PeerConn 出口回退为原 FIFO 直发行为 |
| 新模块 | `peers/conn/egress_fq.rs`：`EgressFairQueue`（per-flow 分桶调度）+ `FlowHashDispatcher`（按流 hash 分片的并行 worker 池） |
| PeerConn 出口 | ring 满时不再直接排队等待，改为进入 per-flow 分桶，桶间轮询发送 |
| TUN 读取桥 | 单任务 → 4 个 worker，按内层 IP 五元组 hash 分片（**同流恒定同 worker，保序**） |
| smoltcp 桥 | 同上改造 |

---

## 2. 设计

### 2.1 EgressFairQueue（PeerConn 出口）

- **桶布局**：1 个优先桶（容量 32，与旧单 ring 总深度对齐，避免路由收敛/RPC 突发比过去更早触发 BufferFull）+ 8 个数据流桶（每桶容量 32，与旧单 ring 的单流可占深度对齐）。**数据面判定收敛在 `packet::is_data_packet`**（`packet/mod.rs`）：`Data`、`ForeignNetworkPacket`、`KcpSrc/KcpDst`、`QuicSrc/QuicDst`、`DataWithKcpSrcModified/DataWithQuicSrcModified` 都算数据面——KCP/QUIC 代理流、跨网转发流属于大流量，必须进流桶而不是挤占优先桶；其余（PingPong、路由、RPC、握手）进优先桶。不可解析头（握手早期包）按控制处理。
- **flow key**：复用 bond 的既有实现 `msg.bond_flow_key().unwrap_or_else(|| flow_key_from_payload(msg.payload()))`（`peers/conn/conn_bond.rs`，内层 IP 五元组 hash）——bond 启用时每包已算好，零重复计算；不可解析 payload 退化为按前缀 hash，行为安全。
- **调度**：单 drain 任务，每轮先清空优先桶，再对 8 个数据桶**轮询各取一个包**发送（`sink.send().await`）。drain 空闲时 park 在**队列级** `work_arrived` Notify 上（任一桶入包即唤醒）；桶级 `Notify` 只服务 push 侧（等空位）。
- **背压语义（与旧路径对齐但非逐字相同）**：push 侧 `try_send`-优先、满则带 100ms 上限等待，超时返回 `BufferFull`（router 计数、conn 不因 BufferFull 拆除——这部分与现行为一致）。**深度差异要明说**：单流可占深度不变（32），但 conn 总排队上限从 32 提高到 `32+8×32=288`；慢链路上的 bulk 流因此少丢包、少 TCP 重传（这是修复吞吐回退，不是新增缓冲）。每桶自带 `Notify`（`notify_one` 无等待者时存 permit，唤醒无竞态）；`close()` 先 `notify_waiters` 唤醒所有停等方、再按 `Notify` 补一个 permit-storing `notify_one`，兜住「已过 closed 检查、尚未注册 notified」的窗口期 pusher。
- **关闭语义**：`sink.send` 失败（Shutdown）→ 置 closed 并唤醒所有等待的 push 方，错误向上传播路径与现状一致。drain 任务挂在 `PeerConn::tasks`（JoinSet）下：连接 drop → abort → 干净退出；**不允许** detached spawn（队列空时 drain park 在 Notify 上，detached 版本会随连接 churn 泄漏任务与 sink 句柄）。

### 2.2 不变量（守卫测试钉死）

1. **同流严格 FIFO**：同一 flow 的包进同一个桶，桶内 FIFO，跨桶绝不移动——单流不乱序（乱序对 TCP 是 RTO 灾难）。
2. **bond 选择不受影响**：bond 在 `Peer::send_msg` 层先选 conn（纯 hash），FQ 只在该 conn 内部重排——两层正交。
3. **公平性**：交互流所在桶与 bulk 流所在桶按轮询交替出包——bulk 独占发送机会被结构性排除。

### 2.3 FlowHashDispatcher（源头并行桥）

- 4 个 worker，每个持有独立 `mpsc` 通道（cap 128）与发送循环；分发器按 flow hash 选 worker（`key % 4`），**同流恒定同 worker** → 保序不变量 1 自动成立。
- 背压：某 worker 通道满 → `dispatch().await` 挂起 → 上游（TUN 读取/stack 出口）自然停。**要诚实说明边界**：一条 bulk 流塞满自己 worker 的 128 槽后，读循环仍会暂停、同平面的其它流一起停——**平面级 HOL 只是按流分片后推迟（约 128 包）并收窄，没有被消除**。拥塞时 4×128 的额外排队（约 700KB@MTU1500）是实打实的 bufferbloat，与「降低尾延迟」的目标存在权衡；这正是本节保留按流保序、不做 per-worker 溢出再分流的理由（保序优先）。
- 发送动作本身（`send_msg_by_ip` / `send_ip_packet`）本就被 router lanes、proxy、vpn_portal 多任务并发调用，并发安全无需改动。

---

## 3. 影响面

### 3.1 代码

| 文件 | 改动 |
|---|---|
| `easytier-proto/proto/common.proto` | `FlagsInConfig` 新增 `bool enable_egress_fair_queue = 60` |
| `easytier-proto/proto/api_manage.proto` | `NetworkConfig` 新增 `optional bool enable_egress_fair_queue = 99`（防 Full 下发清零，同 relay_hop_penalty 先例） |
| `easytier-core/src/config/toml.rs` | 默认 `true` + `define_flags_diff` 登记 |
| `easytier-core/src/config/api_input.rs` | Full→loader 回填、TOML→proto 回显、`FORM_MANAGED_FLAG_FIELDS` 登记 |
| `easytier-core/src/peers/conn/egress_fq.rs` | 新模块：EgressFairQueue + FlowHashDispatcher |
| `easytier-core/src/peers/conn/peer_conn.rs` | 构造时按 flag 建 FQ 并 spawn drain 任务；`send_msg` 分流 |
| `easytier/src/instance/virtual_nic.rs` | TUN 读取改 dispatcher + 4 worker |
| `easytier-core/src/gateway/dataplane/stack.rs` | smoltcp 桥改 dispatcher + 4 worker |

### 3.2 行为

- **纯本节点行为**，无协议改动、无全网协调，可按节点灰度。**但回退是部分的**：`enable_egress_fair_queue = false` 只把 **PeerConn 出口**退回原 FIFO 直发；TUN 读取与 smoltcp 桥的 dispatcher 改造是**无条件生效**的（没有独立 flag）。文档 §1/§3.1 中「回退」均只对 PeerConn 成立。
- **出厂默认 true ⇒ 升级即行为变更**（不需要改配置）。与同版放开的 `relay_hop_penalty`（默认 0→5）**兼容性质不同**：本 flag 是纯本节点行为、可逐节点灰度；`relay_hop_penalty` 改的是全网选路，必须全网统一。两者的发版级升级说明合并在 [`relay-hop-penalty.md` §7](./relay-hop-penalty.md)。
- flag 在 **conn 建立时**读取（conn 生命周期内固定）；改配置后新连接生效。
- 慢/拥塞隧道上的交互流尾延迟改善；LAN（µs 级排空）行为基本不变。
- 内存增量：每 conn 最多 32+8×32=288 个排队包（旧路径 32，≈45KB@MTU1500 → ≈410KB），加上 dispatcher 4×128（≈750KB）。经验上限可按 `conn 数 × 288` 估算；对 128MB 级设备（OpenWrt 等）conn 数通常为个位数，总量可控，但仍建议低配节点灰度时留意。**可观测性**：conn 建立且 FQ 启用时打一条 `debug`（`conn_id`、`peer_id_hint`、优先桶/流桶容量、桶数），可直接判断「FQ 是否在这条 conn 上生效」；`push` 在等待预算内没排上队时按「第 1 次 / 2 的幂 / 每 1000 次」稀疏打 `debug`，并累计 per-conn 的 `buffer_full_drops`——上游 `BufferFull` 计数只能说明「有 conn 饱和」，这一条补上「是哪条 conn」。**仍缺**：各桶实时深度、指标导出、conn 级共享预算选项，属后续项。

### 3.3 不做的事（明确排除）

- per-flow 优先级/流量分类（QoS 语义）——FQ 只保证公平，不定义谁更重要；
- forward_egress lane 内的多流 FQ（二期，中转目的内混合流的进一步公平化）；
- UDP-ring/NIC 背压改造（P-AUDIT.6 既定拒绝项，不翻案）。

---

## 4. 验证

单元测试（`peers/conn/egress_fq.rs` `mod tests`，全部 `fair_queue_*` / `flow_hash_dispatcher_*` 命名）：

- `fair_queue_priority_lane_bypasses_data_backlog`——控制包先于全部数据桶出队；
- `fair_queue_preserves_per_flow_ordering`——同流严格 FIFO；
- `fair_queue_interleaves_flows`——新流不被旧流 backlog 拖住（公平性）;
- `fair_queue_buffer_full_after_bounded_wait`——满桶在等待预算内返回 `BufferFull`，且计入该 conn 的 `buffer_full_drops`；
- `fair_queue_drain_delivers_and_fails_fast_after_sink_death`——**P0 守卫**：drain 任务真实投递每个包；sink 死亡后 push 迅速返回 `Shutdown`（而非挂起/BufferFull/静默丢弃）；
- `flow_hash_dispatcher_keeps_flow_affinity_and_order`——同流恒定同 worker 且保序。

集成测试（`peers/tests.rs`）：

- `peer_conn_egress_fair_queue_delivers_all_packets_in_order`——**FQ 接线守卫**：flag 显式置 true 走真实 PeerConn（本文件其余测试的 `FlagsInConfig::default()` 均为 false、走旧直写路径），经 PeerMap 收发 40 包全部到达、无多余包。若 drain 任务再被构造期 drop（P0 回归），push 填满 lane、包不再到达，该测试即失败。

**剩余覆盖缺口**：k 流公平性在真实 PeerConn 上的冒烟（单测层已有 `fair_queue_interleaves_flows`，PeerConn 级待补）；sink 关闭后集成路径的 Shutdown 传播（单测 `fair_queue_drain_delivers_and_fails_fast_after_sink_death` 已覆盖单元层）。

编译/本地验证（Windows 本机，`CARGO_TARGET_DIR` 指向短路径以避开 MSVC `c1.dll` 调试记录限制）：

- `cargo fmt --all -- --check`——通过；
- `cargo clippy -p easytier-core --all-targets --features proxy-smoltcp-stack -- -D warnings`——通过（覆盖 `egress_fq.rs`、`peer_conn.rs`、`dataplane/stack.rs`、`dataplane/tests.rs`）；
- `cargo test -p easytier-core --lib --features proxy-smoltcp-stack`——1388 passed / 0 failed / 2 ignored；
- `cargo check -p easytier --all-targets --features full`——通过（`virtual_nic.rs` 的 dispatcher 改造）。

特性归属要写清：`full` 是 `easytier` 包的特性，`easytier-core` 侧对应的是 `proxy-smoltcp-stack`——写成 `cargo clippy -p easytier-core --features full` 会直接报 `package 'easytier-core' does not contain this feature: full`。工作区级 `--all` 在 Windows 上无法通过（`easytier-ohos-core` 依赖 `std::os::unix`/`std::os::fd`，仅 Unix；`easytier-gui` 存在与本次无关的既有 lint），因此本地以 per-crate 命令为准，CI（ET Test：Linux 上 `clippy --all-targets --features full --all` + nextest）是最终裁决。
