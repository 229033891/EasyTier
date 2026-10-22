# 隧道级 IP 流量可观测性（哪条隧道在跑哪些 IP）

## Status

- Status: **Roadmap / Phase 1 + Phase 2 已落地（待 Phase 3：控制台配置项与历史落库）**（D1-D16 全部拍板；§13 为 Phase 1 实现级附录；§14 为 Phase 0 实测结果；§15/§16 为实施记录）
- 最近审阅：2026-10-22
- 已定决策：**D5（采集默认关）**、**D8/D9（精确 IP 默认不出节点；`flow_view` 缺省 MASKED）**、**D10（挂 `PeerConnInfo.flows` 字段）**、**D11（速率节点侧固定窗口算）**、D12 不审计 / D13 独立开关 / D14 前缀可配 / D15 支持 managed config / D16 本期不做告警；完整表见 §8
- 归属：运行页（`Status.vue`）可观测性；关联 [`current/peer-connections.md`](../current/peer-connections.md)（多 PeerConn 与 `default_conn`）、[`current/traffic-steering.md`](../current/traffic-steering.md)（出口/子网代理语义）、[`roadmap/web-console-log-persistence.md`](./web-console-log-persistence.md)（控制台日志留存）、[`roadmap/poll-to-push-roi.md`](./poll-to-push-roi.md)（轮询→推送）
- 范围：`easytier-core`（记账）+ `easytier-proto`（字段）+ `easytier-web/frontend-lib`（运行页展示）+ `easytier-web`（可选历史采样）
- 索引：[`../README.md`](../README.md)

> ⚠️ 本文中的行号是**代码锚点**，会漂移；判断行为请以 `current/` 文档与代码为准。

---

## 1. 背景与问题陈述

用户在「运行界面」提出的诉求有两层：

1. **节点级**：现在运行页只有隧道级的累计上下行（`Status.vue` 的「上传 / 下载」两列），看不到「这条隧道现在在跟哪些 IP 通信」。
2. **隧道级（本次目标）**：明确想知道 **每条隧道（PeerConn）** 上：
   - **发往** 哪些目的 IP 的流量（TX 方向）
   - **收到** 哪些源 IP 的流量（RX 方向）

附加诉求（同期讨论）：把这类信息落到日志里，也是一个可接受选项。

### 1.1 今天为什么看不到

| 现象 | 代码锚点 |
|------|----------|
| 只有隧道级字节计数，无任何 IP 维度 | `easytier-core/src/tunnel/stats.rs` `Throughput`（`tx_bytes/rx_bytes/tx_packets/rx_packets`） |
| 计数在隧道 filter 里累加，但 **payload 已加密**，无法解析 IP | `easytier-core/src/tunnel/filter.rs` `StatsRecorderTunnelFilter::before_send/after_received`；`easytier-core/src/peers/conn/peer_conn.rs` 过滤器链顺序（session 加密在前、统计在后） |
| 加密发生在进隧道之前 | `easytier-core/src/peers/peer_manager.rs:4338` `try_compress_and_encrypt` → 内部 `encryptor.encrypt`（**非 secure 模式同样加密**，见同文件 `try_compress_and_encrypt` 实现 `:3668`） |
| peer 维度统计按 **instance id** 聚合，不按 IP | `easytier-core/src/peers/traffic_metrics.rs` `LogicalTrafficMetrics::build_peer_counters`；`per_peer` 私有且**无可读访问口** |
| 全仓唯一的 per-IP 指标是 TCP 代理的**连接计数**，与隧道无关 | `easytier-core/src/gateway/proxy/service.rs` `MetricName::TcpProxyConnect` + `LabelType::DstIp/MappedDstIp` |

### 1.2 硬约束（决定实现位置）

> **隧道层看不到内层 IP。** 内层 IP 包在 `peer_manager` 就被加密，之后 tunnel filter 只见到密文。
> ⇒ 「每条隧道跑哪些 IP」的记账**必须在 `peer_manager` 层完成**，不能放在 tunnel filter。

---

## 2. 目标与非目标

### 2.1 目标

| # | 目标 |
|---|------|
| G1 | 每条 PeerConn 可见：发往哪些目的 IP、收到哪些源 IP、各自字节/包数 |
| G2 | 归属到**具体隧道**（不是只到 peer），因为一个 peer 可能有多条 PeerConn 且 bond 会分摊流量 |
| G3 | 开销可控：默认关闭；开启时有采样、上限、TTL；不引入新的硬件要求 |
| G4 | 呈现通道清晰：**本机可见**与**控制台可见**分开决策、可分别开关 |
| G5 | 不破坏现有通道语义：不污染事件通道、不把逐包日志打进日志文件 |

### 2.2 非目标

- 不做中继/转发流量的**源 IP** 可见性（见 §4.3 语义边界）
- 不做逐包日志（成本不可接受，见 §7.3）
- 不为控制台新增 websocket / webhook 通道（webhook 现在只承载节点生命周期，见 §6.4）
- 不在本期实现「按 IP 的限速 / 阻断」（那属于 ACL，见 `easytier-core/src/peers/acl/`）

---

## 3. 现状盘点：日志 / 事件 / 遥测三条通道

这是本次讨论必须先拍板的前置问题：**新增的流量信息应该落在哪条通道上**。

| | ① tracing 日志 | ② 事件日志 | ③ 遥测（结构化时序） |
|---|---|---|---|
| 载体 | `easytier/src/common/log/mod.rs` 自研 `Logger`（非 tracing-subscriber） | `easytier-core/src/events.rs` `CoreEvent` → `easytier/src/common/global_ctx.rs` `GlobalCtxEvent` → `easytier/src/instance/runtime_host/event_journal.rs` | `easytier-proto/proto/api_instance.proto` + 控制台 SQLite |
| 落点 | stdout/stderr + 滚动文件（`easytier.log` + 3 归档，单文件 ≤100MB，≤400MB 目录上限，`easytier/src/common/log/file.rs`）+ 内存环（**仅 web console 启用**，`easytier-web/src/main.rs:525` `enable_memory_buffer(1000)`） | broadcast（**容量 16**，`global_ctx.rs:194`）→ EventJournal（**只留 20 条 JSON 字符串**，`event_journal.rs:70`）→ `NetworkInstanceRunningInfo.events` → GUI + 控制台 `Status.vue` 事件面板（`frontend-lib/src/components/HumanEvent.vue`） | `easytier-web/src/db/entity/peer_conn_history.rs`（60s 采样、7 天保留） |
| 频率承受 | 高 | **低**（一次状态变化一次） | 中（采样） |
| 结构 | 自由文本 | 枚举 | 定长数值 |
| 生命周期 | 文件滚动 ≈3 天 | **易失**，重启即空 | 天/周级 |
| 谁能看 | 本机（GUI 读文件；控制台只读**自己进程**的环） | 授权用户（随节点运行信息轮询下发） | 控制台授权用户 |
| 现状是否已有 per-tunnel 数据 | 有（字节/包/延迟/抖动） | 无 | 有（`peer_conn_history`：rx/tx 累计、延迟、丢包、抖动、隧道类型） |

### 3.1 结论：语义保持三类，物理收敛为两条通道

**不建议**把三者合并为一个存储。三者的量级、生命周期、结构、审计要求都不同，硬合并的代价是具体的：

| 合并方向 | 代价 |
|---|---|
| 全塞进事件 | 20 条容量 + 16 槽 broadcast 撑不住高频；必然 `Lagged`（GUI 已需专门处理 `event_lagged`）；JSON 字符串无法聚合查询 |
| 全塞进日志文件 | 无法排序/画图/留存；日志滚动 3 天；会吃光 400MB 预算；控制台读文件走「读文件+审计」路径，不适合当数据源 |
| 全塞进控制台 DB | 高频写 SQLite；节点侧不该背 DB；无控制台场景全废 |

**建议的最终形态（2 个物理通道 × 3 类语义）**：

```
通道 A：本机诊断流（tracing）
  ├─ 普通日志
  ├─ 流量聚合日志（CORE::FLOW，每 5-10s 一行，可关）      ← 新增
  └─ 事件的文本降级（已有：easytier/src/instance/cli_event_logger.rs）

通道 B：结构化状态 + 遥测（proto 轮询 + 控制台 DB）
  ├─ peers / routes / conns（已有）
  ├─ events 数组（已有，只放低频状态变化）
  ├─ 隧道流量实时 top-N（新增）                            ← 新增
  └─ 隧道流量历史（新增，采样落库）                        ← 可选
```

**值得统一的只有横切关注点，不是存储**：target 命名与等级过滤（已有 `TargetFilter`）、脱敏（见 §6.5）、可见性开关语义。

---

## 4. 语义定义（必须先拍板，否则实现会打架）

### 4.1 「某条隧道上的流量」的确切含义

- **TX 方向**：本节点**发出**、经该 PeerConn 承载的 IP 包，其**目的 IP**。
- **RX 方向**：本节点**收到**、经该 PeerConn 承载的 IP 包，其**源 IP**。
- 两者都是**链路层视角**：被中继转发的包在中间节点上「收到」的源 IP 是真实源，但那条隧道只是中继链路。

### 4.2 直连 / 中继 / 转发的区分

| 场景 | 本节点视角 | 可归属到哪条隧道 |
|---|---|---|
| 本节点 ↔ 对端直连 | TX/RX 都在直连隧道上 | 直连 PeerConn |
| 本节点做**中继**（转发别人的包） | 只有 RX 侧能看到该包（明文未解密时看不到，见 §4.3） | 中继链路的 PeerConn |
| 对端不是直连、经中继 | TX 走中继 PeerConn | 中继 PeerConn（**不是**最终目的 peer 的隧道） |

### 4.3 中继侧看不到内层 IP（重要限制）

- 出站：明文 IP 只存在于**发起节点**（`peer_manager` 加密前）。
- 入站：**终点节点**在 `peer_manager` 解密后可见明文（`peer_manager.rs` RX 分支的 decrypt → decompress → ACL 处理顺序）。
- 中继节点转发时**不解密网络层**（只做压缩 + 会话层处理），因此**中继无法给出被转发流量的真实源/目的 IP**。
  ⇒ 中继场景只能得到「下一跳是谁 + 字节数」。
- 要让中继也可见，需要在转发路径额外解密一次（成本：一次对称解密 ≈ 1-3 GB/s/核），本期不做。

### 4.4 累计值 vs 速率

- 现有 `rx_bytes/tx_bytes` 是**累计计数**。速率必须由两次采样差分得到。
- 差分必须处理**计数器归零**（PeerConn 重建后从 0 开始）→ 归零时重置基线，不得算出负速率或巨大尖峰。

### 4.5 采样放大

若启用抽样（§5.3），字节数按抽样比例放大，UI 必须标注为**估算值**。

---

## 5. 方案设计

### 5.1 数据结构（core 新增）

建议新增 `easytier-core/src/peers/flow_table.rs`：

```rust
pub struct FlowKey {           // Hash + Eq
    pub conn_id: PeerConnId,   // 归属隧道
    pub peer_id: PeerId,       // 对端节点（冗余，便于按节点聚合/过滤）
    pub ip: IpAddr,            // 目的 IP（TX）或源 IP（RX）
    pub direction: FlowDirection, // Tx | Rx
}

pub struct FlowEntry {
    pub bytes: AtomicU64,
    pub packets: AtomicU64,
    pub last_seen_ms: AtomicU64,
    pub sampled: AtomicBool,   // 是否命中抽样，用于标注估算
}

pub struct FlowTable {
    flows: DashMap<FlowKey, Arc<FlowEntry>>,
    max_per_conn: usize,       // 默认 256
    max_total: usize,          // 默认 8192
    ttl: Duration,             // 默认 10min
    sample_rate: u32,          // 默认 1（全采），可设 1/8
}
```

约束：
- 用 `DashMap`（仓库既有惯例：`traffic_metrics.rs`、`acl/processor.rs`、`gateway/dataplane/flow.rs`）；**不要**用 `std::sync::Mutex<HashMap>`（全局锁会成为热点）。
- 计数用 `AtomicU64` 避免在锁内做加法。
- 超限淘汰：LRU 或「最久未见」近似（用 `last_seen_ms` 扫描淘汰即可，量级小）。

### 5.2 记账点（TX / RX）

**RX（收）——改动最小：**

- `PeerPacketEnvelope` **已经**携带 `PeerPacketIngress::Peer { peer_id, conn_id, origin }`（`easytier-core/src/peers/mod.rs`），但 `peer_manager` 的接收循环里 `let (ret, _) = envelope.into_parts();` **把它丢掉了**。
  ⇒ 改动：`handle_packet` 接收 ingress 参数即可拿到**确切隧道**。
- 记账时机：解密 + 解压之后（此时 payload 是明文 IP 包），复用现成解析器
  `easytier-core/src/peers/acl/filter.rs` `parse_ip_packet`（或 `easytier-core/src/instance/packet_io.rs` 的同名函数）。

**TX（发）——需要把目的 IP 带到选路完成处：**

- `peer_manager.rs:4290` 处已经在算 `flow_key_from_payload(msg.payload())`（**每包已解析 IP 头**，5 元组 hash），提取目的 IP **几乎零额外成本**。
- 但**具体隧道**是在更下游才选定的：`easytier-core/src/peers/conn/peer.rs` `Peer::send_msg` 里按 bond 一致性 hash / `default_conn` 挑 PeerConn。
- ⇒ 建议照抄现有做法：给 `ZCPacket` 增加一个**本地字段**（与现有 `bond_flow_key` 同性质，不上线传输），例如 `flow_dst_ip: Option<IpAddr>`；在 `Peer::send_msg` 选定 conn 后记账。

> 备选（更省事、精度略低）：只在 `peer_manager` 记 **(下一跳 peer, 目的 IP)**，不区分同一 peer 的多条隧道。bond 场景下会失真。

### 5.3 采样 / 上限 / GC

| 项 | 建议默认 | 依据 |
|---|---|---|
| **总开关** `flow_stats_enabled` | **false（默认关）** | D5：关闭时零开销、零采集 |
| 抽样 | 1（全采），提供 `1/2`、`1/8` | §6.3 性能模型 |
| 每 conn 条目上限 | 256 | 内存可控（§6.2） |
| 全局条目上限 | 8192 | 同上 |
| TTL | 10 分钟无流量即淘汰 | 避免长跑后无限增长 |
| GC 周期 | 30s（挂 `PeerMaintenanceTasks`，与 `spawn_recent_traffic_gc_routine` 同节奏） | `peer_manager.rs` 维护任务组 |

### 5.4 协议与展示

- `easytier-proto/proto/api_instance.proto`：新增
  ```
  message PeerConnFlow {
    string conn_id = 1;
    uint32 peer_id = 2;
    string ip = 3;              // 目的 IP（TX）或源 IP（RX）；控制台视图下为聚合前缀
    uint32 direction = 4;       // 0 = tx, 1 = rx
    uint64 tx_bytes = 5;
    uint64 rx_bytes = 6;
    uint64 packets = 7;
    uint64 last_seen_ms = 8;
    bool sampled = 9;           // true = 估算值
    uint32 prefix_len = 10;     // 0 = 精确 IP；>0 = 按前缀聚合（控制台默认形态，见 D8）
    uint32 flow_count = 11;     // 被聚合进本条目的不同 IP 数（prefix_len > 0 时有效）
  }
  ```
  并在 `PeerConnInfo` 追加 `repeated PeerConnFlow flows = <新字段号>`（或旁挂 `ListPeerFlows` RPC，二选一见 §9 Q4）。
- **掩码聚合在节点侧完成**（D8/D9）：精确 IP **不出节点**。下发时由节点把同前缀条目合并为一条（`prefix_len > 0`、`flow_count > 1`），控制台拿到的默认是前缀级而非精确 IP。
  - 前缀长度取值：IPv4 默认 **/24**、IPv6 默认 **/64**（需按部署的虚拟网段掩码语义确认，IPv6 /64 可能过大 → 必要时降到 /56 或 /48）。
  - 视图形态由**请求参数**决定：节点运行信息请求追加 `flow_view`（`EXACT` / `MASKED`，缺省 `MASKED`）。
    - 本机 GUI（`easytier-gui` → 本地管理 RPC）显式请求 `EXACT` ⇒ **本机可见精确 IP**；
    - 控制台（`easytier-web` → 节点 `CollectNetworkInfo`）使用缺省 `MASKED` ⇒ **控制台默认拿不到精确 IP**，需另行显式开启（Q1 后续项）。
  - ⚠️ **暴露面**：`EXACT` 视图随「谁能调用节点管理 RPC」而暴露。管理 RPC **默认仅监听 localhost**，另有可选 CIDR 白名单（`easytier/src/rpc_service/api.rs` `ApiRpcServer::with_whitelist` / `set_whitelist`）。一旦把白名单放开到局域网，精确 IP 也随之可达 ⇒ 放开白名单时需提示，或要求 `EXACT` 视图额外开关（见 Q7）。
- **关键：不需要新建传输通道。** 控制台本来就通过 `CollectNetworkInfo` 轮询节点运行信息（`easytier-web/src/restful/network.rs`），GUI 走同一条 RPC（`easytier-gui/src/composables/backend.ts`）。proto 加字段 → **控制台与 GUI 同时生效**。
- 前端：`easytier-web/frontend-lib/src/components/Status.vue` 节点表增加「流量明细」展开行（**该组件被 Web 控制台与桌面 GUI 共用**，`frontend-lib/src/components/RemoteManagement.vue` 挂载），i18n 键加在 `frontend-lib/src/locales/cn.yaml` / `en.yaml`。
- **必须做 top-N 截断**：全量 5k 条 × 48B ≈ 240KB/次，5s 轮询 ≈ 48KB/s/实例，多实例会成为控制台负担。截断后 20 隧道 × top-10 ≈ 10KB/次 ≈ 2KB/s/实例。

### 5.5 控制台历史（可选，后置）

仿 `easytier-web/src/peer_history.rs`（60s 采样、单设备 20s 超时、7 天保留）：

- 新表 `flow_history`（与 `peer_conn_history` 同构：machine_id / instance_id / conn_id / peer_id / ip / direction / bytes / sampled_at）
- REST：`GET /api/v1/machines/{machine-id}/flow-history/{inst-id}`
- 前端：复用 `frontend-lib/src/components/PeerConnHistoryChart.vue` 的图表形态

---

## 6. 性能与硬件评估

### 6.1 每包增量开销（估算）

新增工作 = ①解析 IP 头 ②查/插 hash 表 ③原子加计数。

| 项 | 估算 |
|---|---|
| IP 头解析（IPv4 固定偏移 / IPv6 走扩展头） | ~20-30 ns |
| `DashMap` 查找 + `entry()` 插入（摊销） | ~50-100 ns（热点 key 有分片竞争） |
| 原子加 | ~5 ns |
| **合计** | **~80-150 ns/包** |

对照现状：每包已有对称加密（1500B 包约 0.5-1.5 µs）+ SipHash 5 元组 ⇒ 增量约为加密开销的 **10%**。

RX 侧解析成本更高（当前 RX 不做 IP 解析，除非启用 ACL，见 `acl/filter.rs` `process_packet_with_acl` 的 `acl_enabled` 早退）；TX 侧几乎免费（解析已在做）。

### 6.2 内存

条目 ≈ `conn_id(16) + peer_id(4) + ip(16) + 计数(24) + 时间戳(8)` ≈ 64-80B，加 map 开销约 **~120 B/条**。

| 规模 | 估算 |
|---|---|
| 20 隧道 × 64 IP | 1.3k 条 ≈ **0.16 MB** |
| 20 隧道 × 256 IP（每 conn 上限） | 5.1k 条 ≈ **0.6 MB** |
| 全局上限 8192 条 | ≈ **1 MB** |

⇒ 内存开销可忽略（相对 GUI 进程数百 MB 级别）。

### 6.3 吞吐影响

| 场景 | 包率 | 增量 CPU（不抽样） | 增量 CPU（1/8 抽样） |
|---|---|---|---|
| 100 Mbps | ~8 kpps | <0.2% 单核 | ~0.03% |
| 1 Gbps | ~83 kpps | ~1% 单核 | ~0.15% |
| 10 Gbps（大量小包） | ~830 kpps | ~8-12% 单核 ⚠️ | ~1-1.5% |

⇒ 高包率场景**必须**抽样（1/8 时统计误差 <12%）。

### 6.4 硬件要求结论

- **无特殊硬件要求**；纯 CPU/内存开销。
- 但低端设备（如 MIPS 软路由）应能**完全关闭**该功能 → 做成**默认关闭**的开关，关闭时零开销。
- 锁：只用 `DashMap` 分片 + 原子计数，避免全局 `Mutex`。
- GC：30s 一次 `retain`/扫描，万级条目可忽略。

---

## 7. 呈现通道与可见性规划

### 7.1 分维度可见性矩阵（建议，待拍板）

| 内容粒度 | 本机 | 控制台 | 说明 |
|---|---|---|---|
| 逐包 / 逐流原始明细 | ✅ 仅 `debug`，**默认关** | ❌ | 量太大；且泄露内部拓扑 |
| 每隧道 **top-N（建议 N=10）目的/源 IP + 字节 + 速率** | ✅ **精确 IP** | ✅ **掩码网段**（D8） | 这就是本次诉求的正解；体积可控 |
| 隧道级汇总（总上下行、活跃流数） | ✅ | ✅ | 已有 `PeerConnStats`，加两个字段即可 |
| 历史趋势（天/周级曲线） | ❌（本地文件滚动足够） | ✅ 落库（掩码级） | 见 §5.5 |
| 异常告警（新增大流量 / 流激增 / 异常目的地） | ✅ | ✅ 走**事件**通道 | 事件通道只放这类**低频状态变化**；必须限流（broadcast 仅 16 槽） |

> ⚠️ 前提：**整个功能默认关**（D5，`flow_stats_enabled=false`）。未开启时本机与控制台都看不到任何流量明细 —— 开启入口需要同时覆盖：节点本地配置 / CLI 参数 / GUI 开关（受管节点则走 managed config 下发，本期是否支持见 §9 Q9）。

### 7.2 本机侧的呈现

- **日志**：新增 target `CORE::FLOW`（走 `easytier/src/common/log/mod.rs` 的 `__log__!` category 形式）。
  ⚠️ 不要用裸 `tracing::` 调用：控制台进程默认 console targets 只有 `["CORE","easytier_web"]`（`easytier-web/src/main.rs:505`），`easytier_core::*` 在 stdout 默认 Off（文件 sink 仍以 warn+ 捕获）。
- **节奏**：每 5-10s 输出一行聚合，例如
  `conn=3f2a… peer=10.0.0.7 tx=1.2MB/s dst=10.0.0.9:443 rx=340KB/s src=10.0.0.9`
- GUI 侧可见性：GUI 没有独立日志页，靠 `frontend-lib/src/components/LoggingSettingsDialog.vue` 读本地日志文件 → 新日志天然可见（需保证文件级别 ≥ info 才能看到 `CORE::FLOW` 的 info 行）。

### 7.3 为什么不能逐包写日志

83 kpps 下逐包 `tracing::info!` 意味着每秒数万行、几十 MB/s 的格式化 + 文件 IO，足以把隧道打满并吃光 400MB 日志预算。
⇒ 只允许：**聚合输出**（默认 5-10s 一行）+ **新流首见**（去重后每流一行，可选 debug 级）。

### 7.4 控制台侧：不新增传输通道

- 控制台**没有** websocket 推送通道（浏览器侧一律 REST 轮询，`frontend/src/modules/usePollingList.ts`）。
- webhook 只承载节点生命周期（`easytier-web/src/webhook.rs`：`validate-token` / `node-connected` / `node-disconnected`），**不适合**承载流量负载。
- ⇒ 控制台侧只有两条路：
  1. **复用现有 `CollectNetworkInfo` 轮询**（推荐）：proto 加字段即到；实时性 = 前端轮询节奏（运行页 5s）。
  2. **新增 `flow_history` 采样落库**（后置）：历史曲线。

### 7.5 隐私与权限（**D8 已定**）

**已定（D8，2026-10-22）：精确 IP 默认不上送控制台；控制台只收掩码网段；本机可见精确 IP。**

- 控制台是多租户（users / groups / permissions）。节点日志**目前不可经 RPC 读取**（`easytier-core/src/management/full/logger_rpc.rs` 只能设级别）——把流量明细送到控制台，等于**新开一个能看见节点内部 IP 拓扑的面**。D8 的作用就是把这一面**默认关起来**。
- 实现要求（对应 §5.4）：
  1. **掩码在节点侧完成**，精确 IP 不出节点；
  2. 视图形态由请求参数 `flow_view` 决定，缺省 `MASKED`；
  3. 本机 GUI 显式请求 `EXACT`；控制台保持缺省；
  4. 打开「控制台看精确 IP」必须是**显式动作**（控制台侧开关 + 节点侧开关，二者都要开）。
- 本机日志里的 IP **不脱敏**（运维排障需要精确值），但要意识到：本机日志文件可被控制台管理员读（`easytier-web/src/restful/admin_diagnostics.rs` 的 `/admin/logs/file`，且该读取有审计行）——**控制台读的是控制台自己的日志文件，不是节点日志文件**；节点日志不经 RPC 暴露（除 `flow_view=EXACT` 这条新通道，需按上面第 4 条双重开关约束）。
- 现状脱敏能力：`easytier-web/src/restful/admin_diagnostics.rs` `redact_log_message` 只 mask `user_token`；若将来需要，可扩展为可配置掩码项（本期不做）。

---

## 8. 已定设计决策（D1-D16，全部已决 2026-10-22）

| # | 决策 | 理由 |
|---|------|------|
| D1 | 流量**不进事件通道**（EventJournal 保持 20 条、broadcast 保持 16 槽语义不变） | 事件是低频状态变化；流量是持续状态，量级不匹配 |
| D2 | 流量**不进 tracing 逐包日志**；只做 5-10s 聚合行（`CORE::FLOW`） | 逐包日志会打满 IO 与磁盘预算 |
| D3 | 记账在 `peer_manager` 层，**不放 tunnel filter** | 隧道层 payload 已加密，解析不到 IP |
| D4 | RX 复用 `PeerPacketIngress.conn_id`；TX 给 `ZCPacket` 增加本地字段携带目的 IP | ingress 已有 conn_id，TX 侧照抄 `bond_flow_key` 既有模式 |
| D5 | **采集本身默认关闭**（节点侧开关 `flow_stats_enabled`，默认 `false`）：关闭时**零开销**（不解析、不记账、不占内存）；开启时才付出 §6 的成本。相关参数一并可配：`flow_stats_sample_rate`（默认 1）、`flow_stats_max_per_conn`（默认 256）、`flow_stats_max_total`（默认 8192）、`flow_mask_prefix_len`（掩码前缀长度） | 低端设备（MIPS 软路由等）必须能完全零开销；默认开启也会带来「未声明的采集」合规争议 |
| D6 | 控制台侧**不新增传输通道**，复用 `CollectNetworkInfo` 轮询 + 可选采样落库 | 现有通道已足够，避免引入 websocket/webhook 面 |
| D7 | 结构化通道与日志通道**共用同一份内存表**，只是呈现形态不同 | 避免两份计数器口径漂移 |
| D8 | **精确 IP 默认不上送控制台**：控制台默认只收**掩码网段**（IPv4 /24、IPv6 /64，聚合在节点侧完成）；**本机 GUI / 本机日志保留精确 IP**（2026-10-22 决策） | 控制台是多租户，节点内部 IP 拓扑属敏感面；本机排障需要精确值 |
| D9 | 视图形态由**请求参数** `flow_view`（`EXACT`/`MASKED`，缺省 `MASKED`）决定；开启控制台精确视图需**控制台侧 + 节点侧双重显式开关** | 精确 IP 不出节点；把「谁能调用节点管理 RPC」与「能否看精确 IP」解耦 |
| D10 | 流量明细**挂在 `PeerConnInfo.flows` 字段**，随现有 `CollectNetworkInfo` 轮询自动下发（Q4=a） | 零额外 RPC 往返；GUI 与控制台同一条 RPC；protobuf3 空数组不占字节 ⇒ 未开启时响应体积与今天完全一致。**降级条件**：实测单实例增量 > 20KB 时改为独立 RPC `ListPeerFlows` |
| D11 | **速率在节点侧按固定窗口（1s）计算**并随下发（Q6=a）；消费侧不做差分 | 速率是「用户看到的事实」，必须唯一口径；固定窗口让速率与谁在轮询、多久轮询无关；计数器归零/PeerConn 重建只有一处处理 |
| D12 | 流量明细**不做读取审计**（Q2=a），与 `peer_conn_history` 一致 | 本期不引入新审计表；审计另立需求 |
| D13 | 管理 RPC **白名单放开不放行 `EXACT`**（Q7=b）：`EXACT` 需**节点侧独立开关**（`flow_stats_allow_exact_ip`，默认 false）+ 请求侧显式 `flow_view=EXACT` | 白名单是唯一的横向暴露路径，不能自动带上精确 IP |
| D14 | 掩码前缀长度**可配**（Q8=b）：`flow_mask_prefix_len_v4`（默认 24）/ `flow_mask_prefix_len_v6`（默认 64） | IPv6 /64 可能过粗，需按部署网段调整 |
| D15 | 开关支持**本地配置 + managed config 下发**（Q9=b） | 受管节点若不能远程开启，功能对控制台用户等于不可用 |
| D16 | **本期不做告警**（Q5=a）：不新增 `GlobalCtxEvent` 变体 | 事件通道 broadcast 仅 16 槽、journal 仅 20 条，告警必须限流；留待有明确诉求再做 |

---

## 9. 待决问题（Open Questions）

| # | 问题 | 选项 | 影响 |
|---|---|---|---|
| ~~Q1~~ | 控制台是否默认上送**精确 IP**？ | ✅ **已决 2026-10-22：否（默认关）** → 见 D8 / D9 | 精确 IP 默认不出节点；控制台只见掩码网段；本机可见精确 IP |
| ~~Q2~~ | 流量数据是否需要**审计**（谁在何时看了哪条隧道的流量）？ | ✅ **已决 2026-10-22：不审计** → 见 D12 | 不引入新审计表，与 `peer_conn_history` 一致 |
| ~~Q3~~ | **采集本身**的默认开关状态（与 D8 的「精确 IP 是否外传」是两件事） | ✅ **已决 2026-10-22：默认关** → 见 D5 | 默认零开销、零采集；需显式开启（节点侧开关 `flow_stats_enabled`） |
| ~~Q4~~ | 协议形态：挂在 `PeerConnInfo.flows` 还是独立 RPC `ListPeerFlows`？ | ✅ **已决 2026-10-22：挂字段** → 见 D10 | 零额外往返；空数组不占字节；>20KB 则降级为独立 RPC |
| ~~Q5~~ | 是否需要**告警**（如单隧道流量突增、N 个新目的地）？ | ✅ **已决 2026-10-22：本期不做** → 见 D16 | 不动事件通道语义；留待有明确诉求再做 |
| ~~Q6~~ | 速率是否要在节点侧算好再下发？ | ✅ **已决 2026-10-22：节点侧固定窗口算** → 见 D11 | 唯一口径；与轮询节奏解耦 |
| ~~Q7~~ | `EXACT` 视图的**暴露面**如何管：管理 RPC 白名单放开到局域网时是否自动允许精确 IP？ | ✅ **已决 2026-10-22：仍需独立开关** → 见 D13 | 白名单 ≠ 精确 IP 授权 |
| ~~Q8~~ | 掩码前缀长度是否需要**按网络实例**可配（IPv6 /64 可能过粗） | ✅ **已决 2026-10-22：可配** → 见 D14 | 默认 IPv4 /24、IPv6 /64 |
| ~~Q9~~ | 开关的**下发路径**：本地配置 / CLI 参数 / GUI 开关 / managed config（受管节点由控制台下发） | ✅ **已决 2026-10-22：本地 + managed config** → 见 D15 | 受管节点必须可远程开启 |

---

## 10. 分阶段计划与验收

| 阶段 | 内容 | 验收标准 |
|---|---|---|
| **Phase 0** | 零改码验证：`tcpdump -i <tun>` / Windows `pktmon` 抓虚拟网卡，确认「哪些 IP」的实际分布与规模 | ✅ **已完成**（2026-10-22，两轮 60s/300s 实测，结论见 §14） |
| **Phase 1** | core：`FlowTable` + TX/RX 记账 + 开关/抽样/上限/TTL/GC + 节点侧速率窗口（实现级设计见 **§13**） | ✅ **已落地**（2026-10-22，见 §15） |
| **Phase 2** | proto 字段（`PeerConnInfo.flows` + `flow_view`）+ codegen + `Status.vue` 展开行 + i18n + `[flow_stats]` 配置 | ✅ **已落地**（2026-10-22，见 §16）；控制台视图为**掩码级**、本机/CLI 视图为**精确级**（需节点 `allow_exact_ip=true`） |
| **Phase 3**（可选） | 控制台 `flow_history` 采样落库 + 图表 | 7 天保留；单实例采样开销 <1% CPU；落库内容为掩码级 |
| **Phase 4**（可选） | 告警（走事件通道 + 限流，D16 本期不做） | 限流生效，`event_lagged` 不再被触发 |

---

## 11. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 高包率场景 CPU 上升 | 默认关闭 + 抽样 + 上限；Phase 0 先量真实包率 |
| 内存无界增长 | 每 conn / 全局双上限 + TTL + GC |
| hash 表热点竞争 | `DashMap` 分片；必要时按 conn 再分片 |
| 控制台轮询体积膨胀 | 强制 top-N 截断 + 只发活跃流 |
| 内部 IP 拓扑泄露 | 默认关闭 + 掩码策略（Q1）+ 权限沿用设备授权 |
| 与现有 `PeerConnStats` 口径不一致 | 两者语义不同需在 UI 标注（链路级 vs 逻辑级）；文档写清 |
| 计数器归零导致速率尖峰 | 速率在节点侧固定窗口算（D11）；PeerConn 重建 = 新 conn_id = 新条目，不污染旧条目 |

---

## 12. 相关文档与代码索引

**文档**
- [`current/peer-connections.md`](../current/peer-connections.md) — 多 PeerConn、`default_conn`、bond
- [`current/traffic-steering.md`](../current/traffic-steering.md) — 出口节点 / 子网代理（影响「目的 IP」语义）
- [`current/architecture.md`](../current/architecture.md) §Logging — 日志体系现状
- [`current/message-popup-audit.md`](../current/message-popup-audit.md) — 弹窗不是日志通道的设计意图
- [`roadmap/web-console-log-persistence.md`](./web-console-log-persistence.md) — 控制台日志留存
- [`roadmap/poll-to-push-roi.md`](./poll-to-push-roi.md) — 轮询→推送（未来可省流量轮询体积）

**代码（core）**
- `easytier-core/src/peers/peer_manager.rs` — TX 记账点（`:4290` flow key、`:4338` 加密、fanout）、RX 记账点（ingress 丢弃处、decrypt/decompress 后）
- `easytier-core/src/peers/mod.rs` — `PeerPacketEnvelope` / `PeerPacketIngress`（含 `conn_id`）
- `easytier-core/src/peers/conn/peer.rs` — `Peer::send_msg`（隧道选定处）
- `easytier-core/src/peers/conn/peer_conn.rs` — `start_recv_loop`（ingress 产生处）、`get_stats`
- `easytier-core/src/tunnel/stats.rs` / `src/tunnel/filter.rs` — 现有隧道级计数（**不可加 IP 维度**的原因）
- `easytier-core/src/peers/acl/filter.rs` — 现成 IP 解析器（可复用）
- `easytier-core/src/peers/traffic_metrics.rs` — 现有 peer/instance 级流量指标

**代码（proto / host / web）**
- `easytier-proto/proto/api_instance.proto` — `PeerConnInfo` / `PeerConnStats` / `NetworkInstanceRunningInfo`
- `easytier/src/common/log/mod.rs` — `Logger` / `MemoryLogBuffer` / `__log__!` target 规则
- `easytier/src/common/global_ctx.rs` — `GlobalCtxEvent` / broadcast 容量 16
- `easytier/src/instance/runtime_host/event_journal.rs` — EventJournal（20 条）
- `easytier/src/instance/cli_event_logger.rs` — 事件→日志的既有降级路径
- `easytier-web/src/peer_history.rs` + `src/db/entity/peer_conn_history.rs` — 采样落库的既有范式
- `easytier-web/frontend-lib/src/components/Status.vue` — 运行页节点表（上传/下载列所在）
- `easytier-web/frontend-lib/src/components/HumanEvent.vue` + `types/network.ts` — 事件渲染与前端枚举

---

## 13. Phase 1 实现级附录（core 记账层）

> 本节是**开工前的落点清单**，字段号与函数名以开工时的代码为准（行号会漂移，语义与职责不变）。

### 13.1 配置项落点

`easytier-proto/proto/api_manage.proto` → `NetworkInstanceConfig`（现有字段到 `90`，新增从 **91** 起）：

```proto
optional bool   flow_stats_enabled            = 91;  // 默认 false（D5）
optional uint32 flow_stats_sample_rate        = 92;  // 默认 1（可 2 / 8）
optional uint32 flow_stats_max_per_conn       = 93;  // 默认 256
optional uint32 flow_stats_max_total          = 94;  // 默认 8192
optional uint32 flow_mask_prefix_len_v4        = 95;  // 默认 24（D14）
optional uint32 flow_mask_prefix_len_v6        = 96;  // 默认 64（D14）
optional bool   flow_stats_allow_exact_ip     = 97;  // 默认 false（D13：节点侧精确 IP 授权）
```

- core 侧读取：`easytier-core/src/instance/config.rs`（与 `peer_link_bond_count` 等同级）。
- 下发路径（D15）：本地 TOML / GUI 开关 / managed config（`easytier-web` 的 managed config 下发链路，沿用既有字段的 PATCH 语义）。

### 13.2 新增文件：`easytier-core/src/peers/flow_table.rs`

```rust
pub enum FlowDirection { Tx, Rx }

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlowKey {
    pub conn_id: PeerConnId,
    pub peer_id: PeerId,
    pub ip: IpAddr,
    pub dir: FlowDirection,
}

pub struct FlowEntry {
    pub total_bytes: AtomicU64,
    pub total_packets: AtomicU64,
    pub window_bytes: AtomicU64,   // 当前 1s 窗口增量
    pub rate_bps: AtomicU64,       // 上一个窗口的字节数（D11：节点侧算）
    pub first_seen_ms: AtomicU64,
    pub last_seen_ms: AtomicU64,
    pub sampled: AtomicBool,       // 该条目含抽样包 → UI 标注「估算」
}

pub struct FlowTableConfig {
    pub enabled: bool,
    pub sample_rate: u32,          // 1 = 全采
    pub max_per_conn: usize,
    pub max_total: usize,
    pub ttl: Duration,
}

pub struct FlowTable {
    flows: DashMap<FlowKey, Arc<FlowEntry>>,
    cfg: FlowTableConfig,
}

impl FlowTable {
    /// 热路径：仅在 enabled 时做一次哈希查找 + 计数；关闭时立即返回（零开销）。
    pub fn record(&self, key: FlowKey, bytes: u64, packets: u64, now_ms: u64);

    /// 1s tick：把 window_bytes 换出为 rate_bps（D11 的固定窗口）。
    pub fn rotate_rate_window(&self);

    /// GC：TTL 淘汰 + 每 conn 上限淘汰 + 全局上限淘汰；配合 shrink_dashmap。
    pub fn gc(&self, now_ms: u64);

    /// 下发视图：掩码聚合（D8）+ top-N 截断（D10）+ 速率携带（D11）。
    pub fn snapshot(&self, view: FlowView, top_n: usize) -> Vec<FlowSnapshot>;
}

pub enum FlowView { Exact, Masked { v4_prefix: u8, v6_prefix: u8 } }
```

要点：
- `record()` 里做抽样判定（`sample_rate`）+ 上限判定；**上限判定必须 O(1) 摊销**（不要每次遍历）→ 用 `per_conn_counts: DashMap<PeerConnId, usize>` 计数，超限才触发一次淘汰扫描。
- `snapshot()` 的掩码聚合：把同 `(conn_id, dir, prefix)` 的条目合并（字节/包求和，`flow_count` = 合并条目数），**再**做 top-N 截断。
- `FlowTable` 挂在 `PeerManagerCore` 上（与 `traffic_metrics` 同级），通过 `Arc<FlowTable>` 下发给 `Peer`/`PeerConn`。

### 13.3 TX 记账点

1. **一次解析，两个用途**：`peer_manager.rs` 出站路径已在算 `flow_key_from_payload(msg.payload())`（每包解析 IP 头）。
   - `easytier-core/src/peers/conn/conn_bond.rs` 内已有 `parse_ip_flow_key(payload) -> Option<IpFlowKey>`（`IpFlowKey = (u8, [u8;16], [u8;16], u16, u16)`），把它提升为 `pub(crate)` 或新增
     `pub(crate) fn parse_inner_ip_addrs(payload: &[u8]) -> Option<(IpAddr, IpAddr)>`（返回 src/dst）。
   - ⇒ **不做第二次解析**。
2. **把目的 IP 带到选路完成处**：给 `ZCPacket` 增加**本地字段**（与既有 `bond_flow_key` 同性质，不上线传输）：
   ```rust
   // easytier-core/src/packet/mod.rs
   pub fn set_flow_dst_ip(&mut self, ip: IpAddr);
   pub fn flow_dst_ip(&self) -> Option<IpAddr>;
   ```
   在出站路径设置 `msg.set_flow_dst_ip(dst_ip)`。
3. **在选定隧道后记账**：`easytier-core/src/peers/conn/peer.rs` `Peer::send_msg` 有**三条**出口（bond 命中 / `default_conn` / `select_conn()`），三处都要记账 → 收敛为一个私有 helper：
   ```rust
   async fn send_on(&self, conn: &ArcPeerConn, msg: ZCPacket) -> Result<(), Error> {
       if let (Some(dst), Some(tbl)) = (msg.flow_dst_ip(), self.flow_table.as_ref()) {
           tbl.record(FlowKey { conn_id: conn.get_conn_id(), peer_id: self.peer_node_id, ip: dst, dir: FlowDirection::Tx },
                      msg.buf_len() as u64, 1, now_ms);
       }
       conn.send_msg(msg).await
   }
   ```
   ⚠️ 记账必须发生在 `conn.send_msg` **之前**（`msg` 会被 move）。

### 13.4 RX 记账点

1. **别丢 ingress**：`easytier-core/src/peers/peer_manager.rs` 接收循环
   ```rust
   // 现状：let (ret, _) = envelope.into_parts();
   let (ret, ingress) = envelope.into_parts();
   self.handle_packet(ret, ingress).await;
   ```
   `handle_packet` 增加 `ingress: PeerPacketIngress` 参数。
2. **记账时机**：只在 `to_peer_id == my_peer_id` 分支、**解密 + 解压之后**（此时是明文 IP 包）。
   - 复用解析器：`easytier-core/src/peers/acl/filter.rs` 的 `parse_ip_packet`（当前是模块私有）→ 提升为 `pub(crate)`，或下沉到 `easytier-core/src/foundation/` 供 ACL 与 flow 共用（推荐后者，避免两处解析逻辑漂移）。
   - 取 `src_ip` 作为 RX 方向的 IP（§4.1 语义）。
3. **归属**：`PeerPacketIngress::Peer { peer_id, conn_id, .. }` → 直接得到隧道。
4. **转发分支不记**：`to_peer_id != my_peer_id` 时不解密网络层（§4.3）→ 不产生 RX 条目。
5. `PeerPacketIngress::Local`（本机注入）→ 不记。

### 13.5 速率窗口与 GC 任务

- 挂在 `PeerMaintenanceTasks::spawn_into`（`peer_manager.rs` 维护任务组），新增两个周期任务：
  | 任务 | 周期 | 动作 |
  |---|---|---|
  | 速率窗口 | **1s** | `flow_table.rotate_rate_window()` |
  | 流量 GC | **30s** | `flow_table.gc(now_ms)`（TTL + 上限 + `shrink_dashmap`） |
- 周期与既有节奏对齐参考：recent-traffic GC 30s、pending flush 20ms、credential GC 1s（`peer_manager.rs` 维护任务组）。

### 13.6 proto 变更

`easytier-proto/proto/api_instance.proto`：

```proto
// PeerConnInfo 现有字段到 19，新增 20
repeated PeerConnFlow flows = 20;

message PeerConnFlow {
  string conn_id    = 1;
  uint32 peer_id    = 2;
  string ip         = 3;   // TX=目的 IP，RX=源 IP；MASKED 视图下为聚合前缀
  uint32 direction  = 4;   // 0 = tx, 1 = rx
  uint64 tx_bytes   = 5;
  uint64 rx_bytes   = 6;
  uint64 packets    = 7;
  uint64 rate_bps   = 8;   // D11：节点侧 1s 窗口速率
  uint64 last_seen_ms = 9;
  bool   sampled    = 10;  // true = 含抽样包，数值为估算
  uint32 prefix_len = 11;  // 0 = 精确 IP；>0 = 掩码聚合前缀长度（D8/D14）
  uint32 flow_count = 12;  // 被聚合进本条目的不同 IP 数（prefix_len > 0 时有效）
}
```

`easytier-proto/proto/api_manage.proto`：

```proto
message CollectNetworkInfoRequest {
  repeated common.UUID inst_ids = 1;
  optional FlowView flow_view = 2;   // 缺省 = MASKED（D9）
}
enum FlowView {
  FLOW_VIEW_UNSPECIFIED = 0;   // 视为 MASKED
  FLOW_VIEW_EXACT      = 1;    // 需要节点侧 flow_stats_allow_exact_ip = true（D13）
  FLOW_VIEW_MASKED     = 2;
}
```

> `flow_view` 是**请求级**参数：控制台用缺省（MASKED）；本机 GUI 显式传 `EXACT`，且节点侧 `flow_stats_allow_exact_ip` 为 true 时才生效，否则**静默降级为 MASKED**（并在响应里体现 `prefix_len > 0`，前端据此提示）。

### 13.7 下发路径（填字段的两处）

`NetworkInstanceRunningInfo` 的 `peer_route_pairs[].peer.conns[]` 与 `peers[].conns[]` **都要填**（运行页用的是 `peer_route_pairs`）：
- `easytier-core/src/management/instance_rpc/mod.rs` `PeerManageRpc::list_peer`
- `easytier-core/src/management/full/instance_info.rs` `network_instance_running_info`

建议抽一个公共 helper：`fn fill_conn_flows(conns: &mut Vec<PeerConnInfo>, table: &FlowTable, view: FlowView)`，两处共用，避免口径漂移。

### 13.8 前端改动点

| 文件 | 改动 |
|---|---|
| `easytier-web/frontend-lib/src/types/network.ts` | `PeerConnInfo.flows?: PeerConnFlow[]` + 新增 `PeerConnFlow` 类型 |
| `easytier-web/frontend-lib/src/components/Status.vue` | 节点表新增「流量明细」展开行（每条隧道下挂子表：方向 / IP(或前缀) / 字节 / 速率 / 估算标记） |
| `easytier-web/frontend-lib/src/locales/cn.yaml` / `en.yaml` | 新增列名与提示文案键 |
| `easytier-web/frontend-lib/src/generated/proto/api_instance.ts` | codegen 产物（`node scripts/codegen-proto.mjs`） |

> `Status.vue` 被 **Web 控制台与桌面 GUI 共用** ⇒ 一次改动两端生效；控制台侧因为走 MASKED 视图，展示的是前缀级。

### 13.9 单测清单（`easytier-core`）

| # | 用例 | 断言 |
|---|---|---|
| 1 | 开关关闭 | `record()` 不产生条目、不分配（表为空且 `len()==0`） |
| 2 | TX 归属 | 记账落在**实际选中**的 conn（bond 命中 / default_conn / select_conn 三条路径各一例） |
| 3 | RX 归属 | 记账落在 `ingress.conn_id`（直连隧道） |
| 4 | 转发包 | 转发分支不产生 RX 条目（不解密） |
| 5 | 抽样 | `sample_rate=8` 时条目数 ≈ 全采的 1/8（统计意义，带容差） |
| 6 | 每 conn 上限 | 超过 `max_per_conn` 后条目数被压回上限（最旧的先淘汰） |
| 7 | 全局上限 | 超过 `max_total` 后总量不增长 |
| 8 | TTL | 超过 TTL 的条目被 GC 清除 |
| 9 | 掩码聚合 | `MASKED` 视图：返回 `prefix_len>0`、同前缀合并、`flow_count` 正确；`EXACT` 视图：返回精确 IP |
| 10 | 速率窗口 | tick 后 `rate_bps` ≈ 上一窗口字节数；空闲窗口归零；**PeerConn 重建后不产生尖峰** |
| 11 | 关闭降级 | 节点侧 `flow_stats_allow_exact_ip=false` 时，请求 `EXACT` 返回 `prefix_len>0`（静默降级） |

**验证口径**（引用测试数必须带口径，见 `docs/README.md`）：`cargo test -p easytier-core --lib` = 默认 features；`--features proxy-smoltcp-stack` 会多出 `gateway/dataplane` 的用例，**两个口径不可相减**。

### 13.10 工作量与回滚

| 项 | 估时 | 回滚方式 |
|---|---|---|
| core（FlowTable + 两处记账 + 任务 + 单测） | ~2 天 | `flow_stats_enabled=false`（默认）→ 零开销；代码可整体 revert |
| proto + codegen | ~0.5 天 | 新增字段向后兼容，旧端忽略 |
| 前端（`Status.vue` + i18n + 类型） | ~1 天 | 字段缺失时降级为不显示展开行 |
| 控制台历史落库（Phase 3） | ~1 天 | 独立表，不影响运行期 |

### 13.11 Phase 0 的验证清单（**已执行，结果见 §14**）

1. `tcpdump -i <tun> -nn`（Linux）/ `pktmon`（Windows）抓 5-10 分钟真实流量；
2. 统计**去重目的 IP 基数**与**每条隧道的目的 IP 基数**分布（P50/P95/P99）；
3. 确认是否需要按 /24 之外的前缀聚合（IPv6 场景）；
4. 确认采集开关的**下发路径**（受管 vs 本地）是否满足使用场景。

> 若 Phase 0 显示「每隧道目的 IP 基数」很小（如 <32），可考虑**放弃采样**（`sample_rate=1`）以换取精确值。

---

## 14. Phase 0 实测结果（2026-10-22，本机 DEV 节点）

> 环境：Windows 11 + EasyTier GUI（`ET.exe`，`--rpc-portal 127.0.0.1:15999`，`--secure-mode`），
> 实例 `et`（`cd57f3d0-…`），虚拟网卡 `et Tunnel`（Wintun），虚拟 IP `10.10.10.101/24`，
> `enable_exit_node = true`，`source = "web"`（**受管节点**），通过 `et.scmec.vip:11010` 接入。
> 采集方式：`pktmon --comp 1`（仅 `et Tunnel`）→ `etl2pcap` → 自写 pcapng 解析。
> ⚠️ `pktmon` 的 pcapng 声明 `LINKTYPE_ETHERNET`，但隧道网卡写的是**裸 IP 帧**（无以太网头）→ 解析必须自动识别，否则 100% 解析失败。

### 14.1 两轮实测

**第一轮（60 秒）**

| 指标 | 值 |
|---|---|
| 抓到的 IP 包 | 2,008（1.59 MB）；协议 TCP 1,977 / ICMP 28 / UDP 3 |
| 方向拆分 | TX（本节点发出）1,337 包 / 1.51 MB；RX（到本节点）671 包 / 87.5 KB |
| **TX 去重目的 IP** | **11** |
| TX 去重 /24 前缀 | 4（压缩 2.8×） |
| **RX 去重源 IP** | **10** |
| 每目的 IP 包数 | P50=3、P95=817、max=817 |
| IPv6 流量 | **0**（网口上只有 link-local `fe80::`） |

**第二轮（300 秒，覆盖一次突发传输）**

| 指标 | 值 |
|---|---|
| 抓到的 IP 包 | 27,532（22.5 MB）；TCP 27,301 / ICMP 130 / UDP 101 |
| 方向拆分 | TX 18,466 包 / 21.26 MB；RX 9,066 包 / 1.26 MB（平均 ≈ 92 pps，0.58 Mbps） |
| **TX 去重目的 IP** | **14**（第一轮 11 → 第二轮 14，**基数收敛极快**） |
| TX 去重 /24 前缀 | 3（压缩 **4.7×**） |
| **RX 去重源 IP** | **13** |
| 每目的 IP 包数 | **P50=8、P95=8,419、max=9,888**（极度长尾） |
| 目的 IP 分类 | 子网代理网段 5 个 / 直连 peer 虚拟 IP **9** 个 / 公网出口 0（窗口内无外网访问） |

**目的 IP 的三类分布（按本机路由表归类）**

| 类别 | 包数（第二轮） | 去重目的 IP |
|---|---|---|
| 子网代理网段（`172.65.90.20/30`、`192.168.11.0/24`） | 18,401 | 5 |
| 直连 peer 虚拟 IP（`10.10.10.x`） | 65 | 9 |
| 公网 / 出口节点（`20.205.243.166` = GitHub，第一轮观测到） | 0 / 18 | 1 |

### 14.2 结论（对设计的影响）

| # | 结论 | 对设计的影响 |
|---|---|---|
| C1 | **目的 IP 基数极小且收敛快**（60s: 11 → 300s: 14） | ⇒ **可以放弃采样**：`sample_rate=1`（全采）即可；`flow_stats_sample_rate` 降级为**高包率场景的备用档**；`max_per_conn=256` 远高于实测（全局 14） |
| C2 | **/24 掩码在小基数下会丢信息**（14 → 3，压缩 4.7×；14 个不同主机塌成 3 个前缀） | ⇒ 掩码聚合**必须带 `flow_count`**（合并条目数），否则控制台看到的「一个网段」无法区分是 1 个还是 14 个目的地。IPv6 /64 默认值**待观察**（本次 0 流量） |
| C3 | **存在公网出口流量**（第一轮目的 IP = `20.205.243.166` = GitHub） | ⇒ 目的 IP **不都是内网地址**；`/24` 掩码对公网 /32 主机会退化成「该 /24」，信息量与精确 IP 差距不大 ⇒ 掩码策略对公网 IP 收益低，但仍应保持默认关闭精确 IP（D8 不变） |
| C4 | 本机是**出口节点**（`enable_exit_node=true`），存在**转发**流量 | ⇒ 印证 §4.3：本节点视角只覆盖「自己发起 + 自己终结」的流量；转发包不解密 → 不产生条目。UI 需提示该口径 |
| C5 | 该节点是**受管节点**（`source = "web"`） | ⇒ 印证 D15（开关必须支持 managed config 下发）为**必需项**而非可选项 |
| C6 | 管理 RPC 仅监听 `127.0.0.1:15999` | ⇒ 与 D13 一致：`EXACT` 视图的暴露面默认仅本机 |
| C7 | **流量长尾极端**：两个目的地承载 99% 流量（9,888 + 8,419 / 18,466 包） | ⇒ `top-N=10` 截断几乎无损失；但 UI 应**突出 top 几条**，并用长条图/对数刻度呈现，否则小流量目的地视觉上不可见 |
| C8 | 单个目的地在 5 分钟内产生 **11.6 MB**（突发传输） | ⇒ 速率字段需支持 **MB/s 级**；D11 的 **1 s 固定窗口**足以捕捉突发（窗口内平均 ≈ 2.3 MB/s），无需更细窗口 |
| C9 | 观测到 **9 个不同的直连 peer 虚拟 IP** 被访问 | ⇒ 组网规模 ≥ 9 节点；**单条直连隧道的目的 IP 基数 ≈ 1**（对端 VIP），出口/代理隧道才承载多个目的地 ⇒ 每 conn 上限 256 毫无压力 |

### 14.3 遗留（Phase 0 未覆盖）

- **每条隧道各自的目的 IP 基数**无法从抓包得到（抓包不区分隧道）→ 需 Phase 1 上线后用真实数据复核；
  按 C1/C9 的量级（全局 14、直连隧道 ≈ 1）推断，单条隧道应 ≤ 3 个 ⇒ 上限 256 无压力。
- **IPv6 目的 IP 基数**：本次无 v6 流量，需在有 v6 网络的环境复测（影响 `flow_mask_prefix_len_v6` 默认值）。
- **高包率场景**：本次平均仅 92 pps（峰值 ~200 pps），远未触及 §6.3 的性能拐点 ⇒ §6 的 CPU 估算仍为**理论值**，需在 1 Gbps 场景复测（届时才需要 `sample_rate=8`）。


---

## 15. Phase 1 实施记录（2026-10-22）

### 15.1 已落地的代码

| 文件 | 改动 |
|---|---|
| `easytier-core/src/peers/flow_table.rs`（**新增**） | `FlowTable` / `FlowKey` / `FlowEntry` / `FlowDirection` / `FlowView` / `FlowTableConfig` / `FlowSnapshot`；`record()` / `record_rx()` / `rotate_rate_window()` / `gc()` / `snapshot()` / `configure()`；`parse_inner_ips()` / `mask_ip()` |
| `easytier-core/src/peers/mod.rs` | 注册 `flow_table` 模块 |
| `easytier-core/src/packet/mod.rs` | `ZCPacket::flow_meta`（**本地字段，不上线**）+ `FlowMeta { dst_ip, inner_len }` |
| `easytier-core/src/peers/conn/peer.rs` | `Peer` 持有 `Arc<FlowTable>`；`Peer::send_msg` 的**三条出口**（bond / `default_conn` / `select_conn`）统一在选定隧道后调用 `record_tx_flow()` |
| `easytier-core/src/peers/conn/peer_map.rs` | `PeerMap::new` 接收并下发 `FlowTable` |
| `easytier-core/src/peers/peer_manager.rs` | 出站路径在**加密前**把 `FlowMeta` 挂到包上；接收循环保留 `PeerPacketIngress`（原来被丢弃）→ `handle_packet(ret, ingress)`；解密解压后 `record_rx_flow()`；`flow_table()` / `configure_flow_stats()` 访问器；维护任务新增 **1s 速率窗口**与 **30s GC** |
| `easytier-core/src/peers/foreign_network/{client,mod}.rs` | 外网 PeerMap 传入（禁用的）独立表 |
| `easytier-core/src/peers/bench_support.rs`、`peers/tests.rs` | 适配新的 `PeerMap::new` 签名 |

### 15.2 与 §13 设计的偏差（已记录）

| # | 偏差 | 原因 |
|---|---|---|
| Δ1 | 字节口径定为 **inner IP 字节**（而非链路字节） | 流量语义是「IP 流量」；链路字节已由 `PeerConnStats` 覆盖。UI 必须标注两种口径不同 |
| Δ2 | `FlowTable` 配置用**原子量**而非 `Mutex<FlowTableConfig>` | 热路径（每包）只读开关与上限，不能有锁 |
| Δ3 | 每条隧道的条目数用 `per_conn_counts` 计数，上限判定 O(1) | 避免每包扫描整表 |
| Δ4 | TX 记账点在 **`Peer::send_msg` 选定隧道之后** | 只有这里才知道**具体**是哪条 PeerConn 承载了包（bond 一致性 hash 会把流分散到不同 conn） |
| Δ5 | RX 记账在**解密 + 解压之后** | 此时才是明文内层 IP 包；转发包不解密 → 不计入（与 §4.3 一致） |
| Δ6 | `FlowTable::record` 用 `DashMap::entry` 显式区分「新建/命中」 | 只有**新建条目**才需要检查上限，避免每包做上限判定 |

### 15.3 验证结果（口径见 `docs/README.md`）

| 命令 | 结果 |
|---|---|
| `cargo test -p easytier-core --lib`（默认 features） | **1119 passed / 0 failed / 1 ignored**（其中新增 14 条 flow 相关用例） |
| `cargo test -p easytier-core --lib --features proxy-smoltcp-stack` | **1283 passed / 0 failed / 1 ignored**（两个口径**不可相减**，见 `docs/README.md`「feature 门控」） |
| `cargo check -p easytier`（CLI/GUI 宿主 crate） | 通过 |
| `cargo check -p easytier-core --features proxy-smoltcp-stack` | 通过 |
| `cargo clippy -p easytier-core --lib` | 无告警 |
| `cargo clippy -p easytier-core --tests` | 仅 8 条**既有**告警（`instance/tests.rs` / `operation_broker.rs` / `tunnel/encrypt` / `wrapped_transport`，与本次改动无关） |
| `cargo fmt -p easytier-core` | 已格式化 |

新增用例清单（对应 §13.9）：
1. `disabled_table_records_nothing` —— 关闭时零记账零条目（用例 1）
2. `tx_and_rx_are_attributed_to_their_tunnel`（用例 2/3）
3. `counters_accumulate_per_ip`
4. `sampling_keeps_a_subset_and_marks_estimate`（用例 5）
5. `per_conn_cap_is_enforced`（用例 6）
6. `global_cap_is_enforced`（用例 7）
7. `ttl_drops_expired_entries`（用例 8）
8. `masked_view_merges_prefixes_and_counts_flows`（用例 9）
9. `rate_window_is_independent_of_polling_cadence`（用例 10）
10. `rx_records_the_source_ip_of_the_inner_packet` / `rx_ignores_non_ip_payloads`
11. `snapshot_is_truncated_and_sorted_by_bytes`
12. `parse_inner_ips_handles_v4_and_v6` / `mask_ip_truncates_to_prefix`
13. `tx_flow_is_attributed_to_the_selected_tunnel`（**端到端**：真实 `Peer::send_msg` 路径 + 内存隧道对端，断言记账落在正确的 PeerConn 上）

### 15.4 Phase 1 尚未覆盖（属 Phase 2）

| 项 | 说明 |
|---|---|
| 配置项（`flow_stats_enabled` 等 6 个） | 尚未进 TOML/proto 配置模型；core 侧只有 `configure_flow_stats(FlowTableConfig)` 运行期入口（**默认关闭**） |
| 协议下发（`PeerConnInfo.flows` + `flow_view`） | 未加 proto 字段 ⇒ 外部暂时看不到数据 |
| 控制台 / GUI 展示 | 未做 |
| RX 端到端（跨节点） | 目前只做了单元级验证（`record_rx`）；跨节点 RX 需要完整 peer manager 集成测试（Phase 2 补） |

---

## 16. Phase 2 实施记录（2026-10-22，配置 + 协议 + 前端）

### 16.1 协议（`easytier-proto`）

| 改动 | 位置 |
|---|---|
| `message PeerConnFlow`（conn_id / peer_id / ip / direction / bytes / packets / rate_bytes_per_sec / last_seen_ms / sampled / prefix_len / flow_count） | `proto/api_instance.proto` |
| `enum FlowView { UNSPECIFIED, EXACT, MASKED }` | `proto/api_instance.proto` |
| `PeerConnInfo.flows = 20` | `proto/api_instance.proto` |
| `ListPeerRequest.flow_view = 2` | `proto/api_instance.proto` |
| `CollectNetworkInfoRequest.flow_view = 2` | `proto/api_manage.proto` |
| `NetworkConfig.flow_stats_* = 91..97`（含 `flow_stats_allow_exact_ip`） | `proto/api_manage.proto` |

> `ip` 用**字符串**而不是 `common.IpAddr`（oneof）：掩码视图下它本来就是「前缀」，字符串在前端可直接显示，省掉 oneof 的位运算。
> `flows` 只挂在 **api 层**（`api.instance.PeerConnInfo`），**不进 peer-to-peer 线协议**（`core_peer.proto` 未动）。

### 16.2 core

| 文件 | 改动 |
|---|---|
| `src/management/flow_view.rs`（**新增**） | `resolve_flow_view()` / `resolve_flow_view_with_prefix()` / `fill_conn_flows()`；**双重开关**落地：只有「调用方显式要 EXACT」**且**「节点 `flow_stats_allow_exact_ip=true`」才返回精确 IP，否则一律 MASKED（D9/D13） |
| `src/instance/management.rs` | `flow_table()` / `flow_stats_allow_exact_ip()` 访问器 |
| `src/management/instance_rpc/mod.rs` | `list_peer` 填充 `flows` |
| `src/management/full/instance_info.rs` | `network_instance_running_info(instance, flow_view)` 填充 `flows`（`peers[]` 与 `peer_route_pairs[]` 两处都填） |
| `src/management/full/process_rpc.rs` | `CollectNetworkInfoRequest.flow_view` 透传 |
| `src/management/full/remote_client.rs` | 本机宿主（桌面 GUI）请求 **EXACT**（节点仍可降级） |
| `src/config/toml.rs` | TOML 模型新增 `[flow_stats]` 表 + `FlowStatsConfig` + `ConfigLoader::get/set_flow_stats` |
| `src/config/api_input.rs` | proto ↔ TOML 双向映射（**只有 proto 明确带字段时才写回**，避免老控制面把本地 `[flow_stats]` 抹掉） |
| `src/instance/config.rs` | `CoreInstanceConfig` → `PortablePeerManagerConfig.flow_stats` |
| `src/peers/peer_manager.rs` | `PortablePeerManagerConfig.flow_stats` → 建表时生效 |

### 16.3 前端（`easytier-web/frontend-lib`，Web 控制台与桌面 GUI 共用）

| 文件 | 改动 |
|---|---|
| `src/types/network.ts` | `PeerConnFlow` 类型 + `PeerConnInfo.flows?` |
| `src/components/Status.vue` | 节点表新增 **「IP流量」列**（紧凑摘要：前 2 个目的地 + `+N`，悬停出全部明细）；新增**展开行**（方向 / IP / 流量 / 速率 / 估算标记）；无流量时整列与展开按钮**不渲染** |
| `src/locales/cn.yaml` / `en.yaml` | `status.flow_detail` / `flow_direction` / `flow_ip` / `flow_bytes` / `flow_rate` / `flow_tx` / `flow_rx` / `flow_estimated` |

### 16.4 调用方的视图选择

| 调用方 | `flow_view` | 效果 |
|---|---|---|
| Web 控制台（`easytier-web`） | 不传（= MASKED） | 精确 IP **不出节点**（D8） |
| 桌面 GUI（`easytier-core` 宿主助手） | EXACT | 本机可见精确 IP（前提：节点 `flow_stats_allow_exact_ip=true`） |
| CLI（`easytier-cli peer`） | EXACT | 同上（本地排障）；表格新增 `flows` 列，显示该节点最忙的两条流（如 `↑10.0.0.9 ↓10.0.0.5`） |

### 16.5 开启方式（TOML）

```toml
[flow_stats]
enabled = true                 # 总开关（默认 false）
sample_rate = 1                # 1 = 全采；高包率场景可设 8
max_per_conn = 256
max_total = 8192
mask_prefix_len_v4 = 24
mask_prefix_len_v6 = 64
allow_exact_ip = false         # 允许本机/CLI 看到精确 IP（D13）
```

### 16.6 已知缺口

- **控制台配置界面**没有 `[flow_stats]` 的表单项（proto 字段已就绪，managed config 可读写，但 GUI 表单未做）→ Phase 3。
- **控制台历史落库**（`flow_history` 表 + 曲线）未做 → Phase 3。
- **RX 跨节点端到端**用例仍缺（目前只有单元级 `record_rx` 覆盖）→ Phase 3。
- 掩码前缀长度默认 /24（IPv6 /64 可能过粗，实测无 v6 流量）→ 见 §14.3。
