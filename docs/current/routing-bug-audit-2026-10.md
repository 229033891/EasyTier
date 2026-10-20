# 节点间选路审查（2026-10）

## Status

- Status: **Current**（审查结论 + 首批修复代码在工作区；⚠️ **待合入 / 待验收**后方可当可依赖现状；其余条目待决）
- 最近审阅：2026-10-20（复核修补：**B7 合并改为对账期**（热更新不再被冻结）、**B7 合并的 `never` / `domains` 硬失败**、**B19 补上热更新路径的校验**、**B20 dataplane 传 flow key**、过期注释与文档待验收标注）；2026-10-19（**第三轮：实施修复 + 实测复核 + 策略路由层核查**。首轮静态审查：OSPF 路由表 / 连接选择 / 数据面出口三路并行；补审：中转 vs 直连，新增 B14–B16；二次复核：B14 撤回、B15 更正；**本轮：按 §5 顺序实施修复（B1/B2 部分/B5/B6/B7/B8/B9/B11/B13/B18/B19/B20 + B10 部分），用实测逐条复核，修正了本文档里三处经不起复核的说法，并核查了 `select_by` 三种策略路由模式的实现**，见 §1e、§1f 与各条的「修复」栏）
- 范围：节点间选路的三个层次 —— **OSPF 路由表**（下一跳计算）、**连接选择**（同一 peer 的多条 PeerConn 怎么选）、**数据面出口**（包交给哪个 peer / 哪个出口节点）
- 方法：**静态代码审查 + 针对性实测**。凡「确认」条目均已逐行核对到具体逻辑行；无法用代码定性的降级为「存疑」；**可复现的行为差异一律写成回归用例跑过**（本轮新增 26 条用例，见 §6）
- 审查时基线：`git HEAD = 51ef4424`；修复代码仍在工作区未合入；前端 `vitest` 28 文件 / 241 用例全过
- 关联：[`peer-connections.md`](./peer-connections.md)（default_conn 与多链路）、[`traffic-steering.md`](./traffic-steering.md)（出口与子网代理）、[`../roadmap/code-review-fixes-2026-10.md`](../roadmap/code-review-fixes-2026-10.md)（CR-01…CR-15）、[`../roadmap/unified-routing-design.md`](../roadmap/unified-routing-design.md)（统一路由模型）、[`../roadmap/live-next-hop.md`](../roadmap/live-next-hop.md)（比较规则契约）
- 索引：[`../README.md`](../README.md)

---

## 0. 摘要

**状态列含义**：✅ 已修（本轮落地 + 有回归用例）｜🟡 部分修（只改了能明确判定的那一半）｜⏸ 待决（修法需要产品/协议语义决策，本轮不动）｜⏸ 待设计（修法本身需要先设计）｜❌ 撤回

| 编号 | 严重度 | 层次 | 状态 | 一句话 |
|---|---|---|---|---|
| **B1** | 🔴 高 | OSPF | ✅ 已修 | 修复前：地址/出口归属的决胜**只看跳数、从不看代价** ⇒ 便宜的出口可能输给贵的；平局时归属由 `DashMap` 迭代顺序决定（跨节点不一致） |
| **B2** | 🔴 高 | OSPF | 🟡 部分修 | **peer 进程重启后，其邻居会拒绝该 peer 的 OSPF 更新**。**已修：不再中止整批**（第三方路由恢复）；**未修：该 peer 自己那条仍冻结 61 分钟**；真·冒用仍会中止整批（安全边界保留） |
| **B3** | 🟠 中 | 数据面 | ⏸ 待设计 | ICMP 代理的 NAT key **不含源** → 共享出口上回包被别的 peer 抢走（兼：可被有意劫持）。**注：原写的修法「key 加字段」不可行，见该条** |
| **B4** | 🟠 中 | 数据面 | ⏸ 待决 | `enable_exit_node` **没被当许可门** → 只配了子网代理的节点会替别人出网 |
| **B5** | 🟠 中 | OSPF | ✅ 已修 | 并发重建时 `peer_infos` / `next_hop_alt_map` 缺 version 守卫 → 旧版本数据覆盖新版本 |
| **B6** | 🟠 中 | OSPF | ✅ 已修 | `get_next_hops()` 绕过 `suppressed_peer_ids` → 被抑制的 peer 仍能通过备用跳被选中 |
| **B7** | 🟠 中 | 数据面 | ✅ 已修 | `exit_nodes` 展开的 `/0` 规则在 LPM 索引里被**整条丢弃**（设计要求"合并"，实现是"二选一"） |
| **B8** | 🟡 低 | OSPF | ✅ 已修 | `edge_cost_from_i32` 声称的"上限"根本不存在 → 真实边权与策略惩罚两段可能重叠 |
| **B9** | 🟠 中 | 连接选择 | ✅ 已修 | 修复前：**丢 1 个控制包 → 质量分 +0.04（代价 +40）**，且滞留 100 个 ping 周期（可达 17 分钟） |
| **B10** | 🟠 中 | 连接选择 | 🟡 部分修 | 修复前：未完成首次 ping 往返的 conn 被上报为**代价 1**（全网最便宜边权）。**已修代价侧（→ 500）；conn 选择侧的"未测量"建模仍未做** |
| **B11** | 🟠 中 | 连接选择 | ✅ 已修 | 滞回基线是**进程级 static** + `retain` 跨 peer map 互清 → 配了外网后发布防抖**完全失效** |
| **B12** | 🟡 低 | 连接选择 | ⏸ 待决 | 发布死区只有绝对项、没有相对项。**注：原写的修法与本条自己的"后果"描述矛盾，见该条** |
| **B13** | 🟡 低 | 连接选择 | ✅ 已修 | 丢包率用 `as u32` 截断 → 29% / 57% / 58% 被系统性低报 1 个百分点 |
| **B14** | ❌ **撤回** | 数据面 | ❌ 撤回 | 「直连可用却走中转」是**误判** —— 项目有测试钉死的契约「成本为主键、跳数只破平局」，本条建议的修法会打破该测试 |
| **B15** | 🟡 低 | 数据面 | ⏸ 待决 | `forward_counter > 2` 会**静默清掉** `latency_first` ⇒ latency-first 只在浅路径生效，与 GUI「最低延迟」口径不符 |
| **B16** | 🟡 低 | 数据面 | ⏸ 待决 | 转发节点用**发包方**盖的 `latency_first` 位选路由表，不是本地配置 ⇒ 本地策略对过境流量不具权威性 |
| **B17** | 🔴 高 | 数据面 | ⏸ 待设计 | **最后一跳（出口节点 → 网格外目标）的代价在协议里没有传播路径** ⇒ 出口选择永远看不到"谁离目标更近" |
| **B18** | 🟠 中 | 策略路由 | ✅ 已修 | 修复前：`select_by = quality` 的打分**只算直连链路** ⇒ 只靠中转可达的候选恒排最后，`degraded` 模式下还会被直接排除 |
| **B19** | 🟡 低 | 策略路由 | ✅ 已修 | 修复前：`failover_when = never` 时候选集只剩第 1 个 ⇒ `select_by` **静默失效**，且配置校验不拦 |
| **B20** | 🟠 中 | 策略路由 | ✅ 已修 | 修复前：`balance` 规则下 wrapped-proxy 旁路校验**判错 peer**（没有 flow key 时按 slot 0 回答，而这条流可能哈希到另一个） |

另有 **4 个存疑项**（S2–S5）与 **20 余项"设计有意为之"**（见 §3、§4）。**净确认 19 条**（B1–B20 去掉撤回的 B14；本轮新增 B17–B20）。**本轮已落地 11 条 + 2 条部分修**：完整修好 B1、B5、B6、B7、B8、B9、B11、B13、B18、B19、B20；部分修 B2、B10（各自只剩一小半，见对应条目）。

---

## 1. 确认的 bug

### B1 ✅ 已修 — 地址/出口归属的决胜只看跳数，从不看代价

**位置**（符号名）：`peer_ospf_route.rs` → `is_new_peer_better`（修复前的判据块在 `:797-824`）
**影响**：`:888-926`（`ipv4_peer_id_map` / `ipv6_peer_id_map` / `ipv6_public_addr_lease`）、`:947+`（`cidr_peer_id_map` / `cidr_v6_peer_id_map`）→ `get_peer_id_by_ipv4` / `get_peer_id_for_proxy` / `get_peer_id_for_default_route_proxy` → `peer_manager.rs:3544`、`:3576-3582`（默认路由代理）

**原判据（修复前，`:806-815`、`:823`）**：

```rust
let old_next_hop = self.get_next_hop(old_peer.peer_id);
let new_next_hop = item.value();
let better = match old_next_hop {
    None => true,
    Some(old) => {
        old.path_len != new_next_hop.path_len
            && old.path_len > new_next_hop.path_len      // ← 只比跳数
    }
};
if !better {
    return false;                                       // ← 平局在这里就返回了
}
// CRITICAL (determinism): ... Break the tie on the peer id ...
old_peer.peer_id > peer_id_and_version.peer_id          // ← 平局永远走不到
```

**根因（三层，逐层加深的缺陷）**：

1. **判据里没有代价**。`OspfNextHopInfo` 同时带 `path_latency`（累计代价）和 `path_len`（跳数），归属仲裁**只用了后者**。⇒ 谁离我更近（更便宜）完全不影响归属。
2. **跳数严格更少时还要再过一道 peer_id 门槛**。`better` 为真之后还要求 `old_peer.peer_id > new_peer.peer_id` 才替换 ⇒ **跳数明显更少（更近）的 peer，只要 peer_id 数值更大就拿不到归属**。
3. **注释承诺的"平局用 peer_id 破平局"是死代码**。注释写的是"when two peers claim the same address and sit at the *same* distance … Break the tie on the peer id"，但 `path_len` 相等时 `better` 为 `false` ⇒ `:813-815` 直接 `return false` ⇒ **peer_id 比较在它声称要修的那个场景里根本不可达**。这正是 `5-F8` 那个"修复"写错了分支。

**触发条件**：两个 peer 声明同一个 IPv4 / IPv6 / public-lease 地址，或同一条 proxy CIDR。**在网格外出口这一层，这不是边缘场景** —— 多个节点同时做出口节点时，它们都会声明 `0.0.0.0/0`（见 B17），归属必然要仲裁。

**后果（实测确证，见 §1e）**：

1. **选错出口**。A 直连 B 100ms、A 直连 C 50ms（B、C 都是出口节点）——两者都 1 跳，**代价完全不影响归属**。
2. **贵的直连 peer 可以保住归属**。A—B 直连 1000ms，A—C 1ms + C—B 1ms（到 B 共 2 跳、2ms）：实测归属落在 **B**（1000ms 的那个），2ms 的 C 拿不到。
3. **跨节点不一致**。归属由 `self.next_hop_map.iter()`（`DashMap`）的迭代顺序决定，而分片顺序来自该表**创建时**随机生成的哈希种子 ⇒ 同一个网络里 A、B、C 三张表**可以给出不同的归属**。

**⚠️ 修正本文档此前的两处说法**（本轮实测后订正）：

| 此前写法 | 订正 |
|---|---|
| "平局时归属**随机翻转**……跨重建不稳定 ⇒ 地址归属在两次重建之间随机翻转" | ❌ **不成立**。实测：同一张表连续 16 次重建，归属**保持不变**。`next_hop_map` 是表的长生命周期字段，重建时原地更新（`entry().and_modify()`），哈希种子在**表创建**时固定一次。⇒ 随机性来自**表实例**（跨进程 / 跨节点），**不是**跨重建抖动 |
| "归属判据只比跳数 ⇒ 更近的 peer 可能输给更远的" | ✅ 成立，但**表述不准**："更近"要区分两个含义。**跳数更少**的 peer 会输给 peer_id 更大的那个；**代价更低**的 peer 则**完全不被考虑**（无论跳数） |

**修复（已落地）**：把归属仲裁改成与下一跳表**同一套字典序**（`live-next-hop.md` §1.2：`成本 → 跳数 → 稳定身份键`）：

```rust
let Some(old) = old_next_hop else { return true };   // 旧记录不可达 → 新来的接管
if old.path_latency != new_next_hop.path_latency {
    return new_next_hop.path_latency < old.path_latency;   // 代价优先
}
if old.path_len != new_next_hop.path_len {
    return new_next_hop.path_len < old.path_len;           // 跳数次之
}
old_peer.peer_id > peer_id_and_version.peer_id             // 最后才用 peer_id 破平局
```

这样 peer_id 决胜**真的**落在了它该落的分支（完全等距时），代价成为主键，跳数退化为第二键。

**回归用例**（`peer_ospf_route_tests.rs`）：

- `exit_ownership_follows_the_edge_cost` —— 对调两个出口的边代价，归属必须跟着换（修复前**恒定不变**）。
- `an_expensive_direct_peer_does_not_keep_the_default_route` —— 1000ms 直连 vs 2ms 两跳，归属必须是 2ms 的那个。

**置信度**：确认（代码事实 + 上述两条用例实测）。

---

### B2 🔴 部分修 — peer 进程重启后，其邻居会长期拒绝该 peer 的全部 OSPF 更新

**位置**（符号名）：`peer_ospf_route.rs` → `check_duplicate_peer_id`（判定）、`update_peer_infos`（修复前用 `?` 中止整批，写锁在判定之后才取）。修复前行号：判定 `:1665-1676`、中止 `:1693-1706`、写锁 `:1708`
**放大点**：`:1713-1719` → `:4784`（`do_sync_route_info` 早退，连 `conn_info` 都不再处理）、`:1766-1781` → `:1828-1846`（conn 行的版本闸门同样拒绝）

**修复前的判定（`check_duplicate_peer_id`）**：

```rust
} else if info.peer_id == dst_peer_id {
    let Some(dst_peer_route_id) = dst_peer_route_id else { return Ok(()); };
    if dst_peer_route_id != info.peer_route_id
        && info.version < self.get_peer_info_version_with_default(info.peer_id)
    {
        return Err(Error::DuplicatePeerId);      // ← 中止整批
    }
}
```

**触发条件**：**任一 peer 进程重启。** 三个前提均已核实：

| 前提 | 证据 |
|---|---|
| `peer_route_id` 每进程随机 | `peer_ospf_route.rs:2884` `my_peer_route_id: rand::random()` |
| 重启后自身 version 从 1 重新开始 | `update_my_peer_info` `:1946` `new_version = old.map(\|x\| x.version).unwrap_or(0) + 1`；新进程 `peer_infos` 为空 ⇒ `old = None` ⇒ `new_version = 1` |
| 邻居仍存旧 route id + 旧 version | `synced_route_info.peer_infos` 只在 `remove_peers` 时清除；`remove_peer` **只有单测在调**（`peer_ospf_route_tests.rs:1099`），生产无「重连即清除」路径 |

**后果（修复前 / 修复后）**：

| | 修复前 | 修复后（本轮） |
|---|---|---|
| 邻居对"发送方自己的那条 `RoutePeerInfo`"求值 | `route_id` 不同（真）且 `version` 变低（真）⇒ `Err(DuplicatePeerId)` | 判定为"**发送方重启**"⇒ **跳过该条**，不报错 |
| 整个 `update_peer_infos` 批次 | **中止**（`?` 在写锁之前，前面已遍历的条目也不落库） | **继续处理** ⇒ 发送方转发的**第三方路由**正常落库 ✅ |
| `do_sync_route_info` 的后续 | 提前返回 ⇒ `conn_info`（连通关系）、`foreign_network`、路由表重建**全都没跑** | 正常执行 ✅ |
| 邻居对该 peer **自己**的记录（route id / version / IPv4 / proxy CIDR） | 冻结在重启前 | **仍冻结**（本轮未改）⇒ 要等清理窗口 |
| 恢复窗口 | `clear_expired_peer`（`:3946-3957`）的清理条件是 `d > 3660s`（`REMOVE_DEAD_PEER_INFO_AFTER`，≈61 分钟）**或**（`d > 90s` 且该 peer 在拓扑里不可达）。**快速重启（崩溃后数秒内自愈）时 peer 已重新可达 ⇒ 90s 分支不触发 ⇒ 只能等 61 分钟。** | 同左（未改） |

⇒ **本轮止住的是"一个节点重启 ⇒ 整个网络的路由被连坐丢弃"**（最大伤害面）。**该 peer 自己那条记录仍然冻结 61 分钟**（它的 IPv4 / proxy CIDR / 连通关系停在重启前）—— 多数情况下这些值重启后并不变，所以实际影响小；但**路由表里该 peer 的版本号与 route id 是旧的**这件事仍在。

**根因**：判定"冒用"时用"route id 变了 + version 变低"（修复前 `:1670-1672`），但**重启与冒用在这两个字段上不可区分** —— `peer_route_id` 每进程随机，重启必然同时满足这两个条件。

**修复（🟡 部分落地）**：

**已做（低风险的一半）**：把"发送方自己那条被判为冒用"从**中止整批**降级为**跳过该条**。

- `check_duplicate_peer_id` 的返回类型从 `Result<(), Error>` 改为 `Result<bool, Error>`：`Ok(false)` = "跳过这条，但继续处理整批"；`Err` = 中止整批。
- **第一个分支（`info.peer_id == my_peer_id`，即"有人广播我的 peer id"）保持 `Err(DuplicatePeerId)` 不变** —— 那是 CR-07/CR-08 的安全加固，不能削弱。
- `update_peer_infos` 在 `Ok(false)` 时 `continue`，跳过该条。

**未做（需要设计）**：完整修复 = 识别"这是同一节点的重启"⇒ 清掉本地旧记录并接受新的 route id + version（强制重学习）。**难点在于判据必须与发送会话绑定**：

| 现成字段 | 能不能用 |
|---|---|
| `RoutePeerInfo.noise_static_pubkey` | ❌ **自声明**的（peer 自己在广播里填，接收方直接塞进信任列表 `:1385-1393`），**没有**与发送会话的 Noise 身份绑定 ⇒ 冒用者可以抄 |
| `RoutePeerInfo.inst_id`（进程 UUID） | ❌ 重启就变，但同样是**明文广播**的 ⇒ 冒用者能抄 |
| 会话层（peer_rpc）的远端 Noise 静态公钥 | ✅ 可信，但 `SyncRouteSession`（`:2417`）**没有**这个字段，要从 `PeerRpcManager` 那侧取 ⇒ 跨层改动 |

**回归用例**：

- `a_restarting_sender_only_skips_its_own_stale_record` —— 发送方重启后，同一批里**第三方** peer 的路由**必须落库**；发送方自己那条**保持旧值**（跳过而非应用）。
- `a_peer_advertising_our_own_id_still_aborts_the_batch` —— **真正的冒用仍然中止整批**，且**不部分应用**（安全边界没被削弱）。

**置信度**：确认。三个前提 + 中止路径 + 恢复窗口均已逐行核对。

---

### B3 🟠 — ICMP 代理的 NAT key 不含源，同出口的 peer 会互抢回包

**位置**：`easytier-core/src/gateway/proxy/icmp_proxy_engine.rs:31-36`（key 定义）、`:128-142`（写表）、`:163-170`（查表并 `remove`）

```rust
struct IcmpNatKey {
    real_destination: Ipv4Addr,
    identifier: u16,
    sequence: u16,
}          // ← 没有 source_peer_id / source_ip
```

但 value 里**存了** `source_peer_id` / `source_ip`（`:40-44`）—— 源信息本来就在，只是没进 key。

**关键佐证：这是同族三个引擎里唯一的异类。**

| 引擎 | NAT key | 含源？ |
|---|---|---|
| `icmp_proxy_engine.rs:32` | `{real_destination, identifier, sequence}` | ❌ |
| `udp_proxy_engine.rs:26` | `{src_socket, dst_socket}` | ✅ |
| `tcp_proxy_engine.rs:58` | `{src, mapped_dst}` | ✅ |

**触发条件**：两个 peer 共用同一个出口节点，向**同一真实目的**发 ICMP Echo，且 **identifier + sequence 相同**。这不是罕见巧合 —— `golang.org/x/net/icmp` 默认 `ID: 0`；很多自研 raw-socket ping 也用固定 ident；即便 ident 随机（iputils ping 取随机 16-bit），两个 peer 各 1/65536 概率撞上后，因为双方 **seq 从 0 同步递增**，碰撞是**粘性**的 —— 之后每一次 ping 都互相覆盖。

**后果**：后到的请求覆盖先到者的 NAT 条目。回包到达时 `nat_table.remove(key)` 取到的永远是**最后写入者**：

- 先到的 peer 的 Echo Reply 被交给**另一个 peer**（其 IP 栈按 ident+seq 匹配，会当成自己的应答接受，payload 却是对方的）
- 被抢走那一份在引擎里 `remove` 返回 `None` → `return Vec::new()` → **直接丢弃**
- ⇒ **先到的 peer 永久 ping 不通**（超时），且**无日志无计数**

**兼安全性问题**：key 不含源 ⇒ 一个 peer 可以**故意**用受害者的 ident+seq 发包，抢走受害者的 ICMP 回包。

**⚠️ 本轮复核：原写的修法「把 `source_peer_id` 加进 key」**不可行**——回包路径根本拿不到源。**

`handle_socket_response(peer_ip, packet)` 收到的是 socket 读到的完整 IPv4 包。ICMP Echo Reply **不回显源地址**（源是我们发给 Internet 的那个 NAT 地址），所以：

- `peer_ip` 是**回包的源**（Internet 主机），不是发起方；
- 发起方信息只在 NAT 表的 value 里（`source_peer_id` / `source_ip`），而查表用的是 `(real_destination, identifier, sequence)`；
- 且**所有 peer 的 ICMP 请求共用同一个 socket**（`icmp_proxy_service.rs:216-225` 每次 `socket.send(...)` 用同一把锁取出的 socket），所以回包的目标地址也无法区分发起方。

⇒ 往 key 里加字段只能**避免覆盖**（写入时不再静默覆盖别人的条目），但**回包仍然无法消歧**（`(D, I, S)` 命中多条时依然只能取一条）。

**可行的修法（需设计，本轮未做）**，二选一：

| 方案 | 做法 | 代价 |
|---|---|---|
| **A. 改写 identifier** | 出口节点为每个 ICMP flow 分配本节点唯一的 `ident`（改写后发往 Internet），回包时按改写后的 `ident` 精确查表，再把 `ident` 还原成原值发给 peer | 需重算 ICMP checksum；`u16` 空间（65536 个并发 flow）要处理绕回；部分场景对 ident 敏感 |
| **B. 源地址 NAT** | 给每个源 peer 分配**不同的本地源 IP**，回包的目标 IP 即 NAT 源地址，可据此反查发起方 | 需要 socket 运行时支持按源地址发送；改动面比 A 大，但语义最干净（与 UDP/TCP 的 key 对齐） |

**即便不选 A/B，也应先做的一步**：把 `nat_table.insert` 的**静默覆盖**改成"冲突时不覆盖并记 warn"——这不解决串台，但至少不再无声吞掉别人的回包，且让问题可观测。

**置信度**：确认（代码事实）；触发概率很可能。**修法需设计评审**，故本轮未动。

---

### B4 🟠 — `enable_exit_node` 没被当"许可"用

**位置**：`icmp_proxy_engine.rs:70`（总闸）与 `:95`（逐包）；`udp_proxy_engine.rs:198` 与 `:226-228`；`tcp_proxy_engine.rs:241` 与 `:560-577`

**总闸**（`icmp_proxy_engine.rs:70`，三个引擎同构）：

```rust
if self.cidr_table.is_empty() && !context.enable_exit_node && !context.no_tun {
    return IcmpProxyAction::Pass;
}
```

**逐包判定**（`icmp_proxy_engine.rs:95`）：

```rust
if real_destination.is_none() && !header.is_exit_node() && !is_local_no_tun {
    return IcmpProxyAction::Pass;
}
```

**触发条件**：节点 B 配了 `proxy_cidrs`（哪怕只是一条子网代理），但 **没有**开 `enable_exit_node`；节点 A 把出口指向 B（A 侧 `get_msg_dst_peer_ipv4` 打 `is_exit_node=true`）。

**后果**：B 仍会把 A 的**任意互联网目标**（8.8.8.8 等）用 B 自己的 IP NAT 出去。`enable_exit_node` 在整个代码库里**只**出现在这一处，是 **OR 条件**而非许可门 ⇒ "我不是出口节点"这个设置**不生效**，B 事实上仍在替第三方承载出网流量。

**与文档直接冲突** —— `docs/current/traffic-steering.md` 三处都承诺"丢弃"：

| 位置 | 原文 |
|---|---|
| `:54` | 带 `exit_node` 标志的包：需 `enable_exit_node`（或宿主强制）才由 TCP/UDP/ICMP 代理接 |
| `:156` | 被指向的 peer **必须开启**…否则该 peer 会**丢弃**这些包（不会静默回退到其它路径） |
| `:222` | 目标 peer 未开 `enable_exit_node` → 该 peer **丢弃**这些包 —— 排障先查此项 |

"丢弃"只在 B 完全是裸节点（无 `proxy_cidrs` 也未开出口）时才发生。**在合规/策略场景下这是实打实的绕过。**

**修复**：把总闸拆成「引擎是否启用」与「是否允许承接出口流量」两层，后者在 `is_exit_node` 分支上单独要求 `ctx.enable_exit_node`，使未开 `enable_exit_node` 的节点对带标志包一律 `Pass`。

**置信度**：确认（代码行为 + 文档口径）。**是否算 bug**取决于"网关模式"是否就允许这种 OR 语义 —— 倾向于算 bug，因为当前文档把它写成了许可开关。

---

### B5 🟠 — 并发重建时 `peer_infos` / `next_hop_alt_map` 缺 version 守卫

**位置**（符号名）：`peer_ospf_route.rs` → `build_from_snapshot_with_unusable` 里的 `peer_infos` 写入（修复前是无条件 `insert`）、`gen_next_hop_map_with_least_cost` 里的 `next_hop_alt_map` 清理（修复前是无条件 `remove`）。修复前行号：`:790` / `:667`
**对照**：`:642-649`（`next_hop_map` **有** `if x.version < version` 守卫，行为正确）

**触发条件**：`update_route_table()` 会被两个不同任务并发调用 —— ① `update_my_peer_info_routine`（1s 循环任务）；② OSPF RPC 处理任务（`do_sync_route_info`）。两次调用之间若发生一次 `synced_route_info.version.inc()`，两次 `build_from_snapshot_with_unusable` 就会拿到**不同 version 的快照**，而该函数全程不持互斥锁（`OspfRouteTable` 字段全是 `DashMap`/`ArcSwap`，只能保证内存安全，不能保证"新版本不被旧版本覆盖"）。

**后果**：

- 旧版本构建（V1）后完成时，`:790` 的**无条件 insert** 会用 V1 的 `RoutePeerInfo` 覆盖 V2 已写入的同名条目 ⇒ `get_peer_info()` / `list_routes()` 短暂对外播报**陈旧的 IP / proxy CIDR / feature_flag**
- `:667` 的 `remove` 同样无条件：V1 构建认为"该 dst 没有等代价备用跳"时，会把 V2 刚写入的 `next_hop_alt_map` 条目删掉 ⇒ **已建立链路的等代价备用下一跳消失**（`live-next-hop.md` 的 Phase B/D 回退能力被静默削弱）

触发窗口不大但真实存在（1s 循环 + RPC 回调），症状是"偶发路由回退 / 状态页显示旧地址"，极难定位。

**修复（✅ 已落地）**：

- `:790` 的 `peer_infos.insert` 改为带守卫的 `entry().and_modify()`：**仅当新条目更新时才覆盖**。判据是两段式：
  1. **先比 `inst_id`**（`route_peer_inst_id`）—— 它是**进程实例 id**，peer 进程重启就变。`RoutePeerInfo.version` 是 **peer 自己**的计数器，**重启后从 1 重新开始** ⇒ 单看 version 会把"重启后的新信息"误判成"更旧"而永久拒收。⇒ `inst_id` 不同 ⇒ 直接接受。
  2. **同一实例内比 `version`**（peer 自己单调递增，`peer_ospf_route.rs:1946` `new_version = old.version + 1`）⇒ 能给同一个 peer 的两条记录排序。
- `:667` 的 `next_hop_alt_map.remove` 改为 `remove_if(|_, entry| entry.version <= version)`：**只删自己（或更旧）那条**，并发重建写入的更新记录不会被误删。

**置信度**：确认（缺守卫是客观事实；并发触发条件已逐条核对调用点）。

---

### B6 🟠 — `get_next_hops()` 绕过 `suppressed_peer_ids`

**位置**（符号名）：`peer_ospf_route.rs` → `get_next_hop`（命中 `suppressed_peer_ids` ⇒ `None`）vs `get_next_hops`（修复前只过滤了主跳）。修复前行号：`:352-357` / `:376-392`

```rust
pub fn get_next_hop(&self, dst_peer_id: PeerId) -> Option<OspfNextHopInfo> {
    if self.suppressed_peer_ids.contains_key(&dst_peer_id) {
        return None;
    }
    self.get_topology_next_hop(dst_peer_id)
}

pub fn get_next_hops(&self, dst_peer_id: PeerId) -> NextHopCandidates {
    let mut ret = SmallVec::new();
    if let Some(primary) = self.get_next_hop(dst_peer_id) {   // ← 只有 primary 走了抑制检查
        ret.push(primary.next_hop_peer_id);
    }
    ...
    if let Some(entry) = self.next_hop_alt_map.get(&dst_peer_id) && entry.version >= cur_version {
        for hop in entry.hops.iter() { ... }                    // ← 备用跳不做任何抑制过滤
    }
    ret
}
```

**触发条件**：某个 peer 因"不可复用凭据重复"被选举机制抑制（`suppressed_non_reusable_credential_peers`），同时它在拓扑里存在**等代价**的备用首跳。

**后果**：`get_next_hops` 返回的候选里含该被抑制 peer 的**备用首跳**。发送路径 `conn/peer_map.rs`（`get_gateway_peer_id` → `get_next_hops_with_policy`）会选中它们 ⇒ **流量被转发到一个按策略应被排除的目标 peer**，与"excluded from forwarding until owner election selects them"的语义冲突（抑制只在 primary 生效，备用跳漏网）。

**修复（✅ 已落地）**：在 `get_next_hops` 遍历**备用跳**时同样过滤 `suppressed_peer_ids`。`get_next_hop` 已经过滤了主跳，备用跳必须遵守同一条规则，否则被抑制的 peer 仍能作为备用首跳被选中。

**置信度**：确认（两条读取路径的代码差异是客观的；被抑制 peer 仍会进入图并成为 alt 条目目的也已核对）。

---

### B7 ✅ 已修 — `exit_nodes` 展开的 `/0` 规则在 LPM 索引里被整条丢弃

**位置**（符号名）：`local_route_table.rs` → `rebuild_index`（同前缀去重）、`build_rules_from_config`（追加 sugar 规则）

**规则构造**（`:760-779`）：显式 `[[policy_routes]]` 先 push，**sugar `/0` 最后 push**。

**索引去重**（`:547-561`）：

```rust
entries.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1))...);
for (len, network, _strict, cidr, idx) in entries {
    if last == Some((len, network)) {
        continue;   // ← 后来者直接丢弃
    }
    ...
}
```

显式 `/0` 与 sugar `/0` 的 `(len, network)` 都是 `(0, 0)` ⇒ 排序后显式在前 ⇒ **sugar 被 `continue` 掉**。

**设计要求是"合并"** —— `docs/roadmap/unified-routing-design.md:1414`「仍相同 → **合并候选 peer 列表**（先后者在前）」、`:1432`「`exit_nodes` 展开的 `/0` 与显式 `/0` 规则 → **合并候选列表，显式在前**」。实现只有"丢弃后来者"。

**后果**：

1. 显式 `/0` 完全吃掉 `exit_nodes` —— 出口节点不再是该规则的候选，规则挂掉时不会切到它。`if_all_unavailable = closed` 时更明显：显式规则候选全挂 → 规则 `Degraded` → `Block` → **直接丢包**，而明明可达的 `exit_nodes` 不会被用。`select_by = quality` / `balance` 时出口节点不参与排序/分摊
2. 被丢弃的 sugar 规则**仍是死规则**：`match_rule` 永远返回不了它的 idx，但 `l2_contributing_cidrs`（`:276-285`）遍历的是 `snap.rules` 而非索引 ⇒ **L2 仍然采纳它**。同一条规则在 L2/L3 给出不同答案

**修复（✅ 已落地）**：把合并从**构造期**挪到**对账期**。

**为什么必须挪**：构造期合并（把 `exit_nodes` 直接塞进 `rule.peers`）会**冻结**出口节点——`build_local_route_table` 的注释就写着 "Do **not** bake exit_nodes into sugar rules here — that would freeze hot-patched exit peers on construct-time A"。而 `update_exit_nodes`（`peer_manager.rs`）**只改列表、不重建策略表** ⇒ 构造期合并会让 `config_patch` 的热更新**永久失效**。两条约束（设计文档要求"合并" vs 零回归约束要求"不冻结"）只有把合并放到对账期才能同时满足。

⇒ **注意**：这条约束对**两条**路径都成立 —— 显式 `/0` 规则和 sugar `/0` 规则。第一轮修复只处理了显式路径（sugar 仍然是构造期烤进去的），第二轮复核才发现 sugar 路径同样被冻结（详见下面的 ⚠️ 框）。

**做法**：

- `RouteRule` 新增 `merge_exit_nodes: bool` —— 规则只**记录**"我要 exit nodes"，不把它们塞进自己的 `peers`。
- `build_rules_from_config` 找到显式 `/0` 规则 ⇒ 置 `merge_exit_nodes = true`，**不**单独 push sugar 规则 ⇒ 整个表里只有**一条** `/0` 规则 ⇒ L3 索引与 L2 贡献集（`l2_contributing_cidrs` 遍历 `rules` 而不是索引）**给出同一个答案**（原来的后果 2 一并消失）。
- `merged_candidates(rule, exit_nodes)`（纯函数，可单测）负责合并：**显式候选在前，`exit_nodes` 在后**，去重。
- 对账循环 `reconcile_local_routes_once` **每轮**用**当前**的 `exit_nodes` 构造候选 ⇒ 热补丁在下一个 tick（≤1s）生效。
- **没有**显式 `/0` 规则 ⇒ 建一条 **sugar** `/0` 规则，但它的 `peers` **是空的**、`merge_exit_nodes = true` ⇒ 候选**完全**由对账期从**活** `exit_nodes` 生成。

> ⚠️ **sugar 路径同样必须走对账期**（第二轮复核补上）。原先 sugar 规则是 `peers: exit_nodes.to_vec()`（**构造期烤进去**）+ `merge_exit_nodes: false` ⇒ **热更新仍被冻结**：`update_exit_nodes` 只换活列表、不重建表，而 L3 次序 3.5 会先命中这条 sugar `/0` ⇒ 活列表（次序 4）**根本轮不到** ⇒ 热改出口列表对策略表完全无效。
> ⇒ 现在 sugar 与显式 `/0` **走同一套机制**，两条路径的热更新都 ≤1s 生效。
> ⇒ 为此 `validate_route_rule` 的"至少一个 peer"检查对 `merge_exit_nodes` 的规则**放宽**（显式规则在标记该标志**之前**就已通过校验，因此不受影响）。

> ⚠️ **标志也必须无条件**（第三轮复核补上）。前两轮只把**候选列表**变成活的，`merge_exit_nodes` **标志**却仍按"构造期 `exit_nodes` 非空"决定 ⇒ **实例启动时没有出口、之后用 `config_patch` 加上出口** ⇒ 标志永远是 `false` ⇒ 策略表（**构造一次、之后不重建**）永远看不到那个出口。
> ⇒ 现在标志**无条件**置位（显式 `/0` 与 sugar 都一样），候选列表每轮从**活** `exit_nodes` 生成 ⇒ 启动时没有出口、之后热补丁加上，也能在下个 tick（≤1s）生效。
> ⇒ 代价：`failover_when = "never"` 的默认路由规则**现在总是被拒**（即使当前没有 `exit_nodes`）—— 因为出口可能稍后被加上，那时合并会静默失效。

**配置期的硬失败**（全部按"合并后"的候选数判定）：

| 条件 | 结果 |
|---|---|
| 合并后候选数 > `MAX_PEERS_PER_RULE`（16） | **加载失败并报错**（不静默截断 —— 静默丢掉候选正是本条要消除的那类"配了不生效"） |
| 合并后候选数 > 1 且 `if_all_unavailable = closed` 且未 `require_confirmed` | **加载失败并报错**（单 peer 的 `closed` 规则原本合法，合并后会变成"closed + 多 peer"） |
| 显式 `/0` 规则**同时带 `domains`** | **加载失败并报错** —— 否则 `exit_nodes` 会一并成为**域名规则**的候选，静默放宽规则语义 |
| 显式 `/0` 规则 `failover_when = "never"` | **加载失败并报错** —— `"never"` 只保留**第一个**候选，合并进来的 `exit_nodes` **永远不会被选中**。这正是 B19 要消除的那类"配了却静默不生效"（`never` 本身合法，只有**要合并 `exit_nodes`** 时才冲突） |

**回归用例**（10 条）：

- `exit_nodes_merge_into_an_explicit_default_route_rule` —— 合并成**一条**规则，且 `peers` **不**含 `exit_nodes`（它是对账期合的，不是烤进去的）。
- `the_merged_candidates_follow_the_live_exit_nodes` —— 换一份 `exit_nodes`，合并结果跟着换 ⇒ **热更新语义**。
- `the_exit_nodes_sugar_rule_follows_the_live_exit_nodes` —— **sugar 规则**的候选来自活 `exit_nodes`，`peers` 为空 ⇒ 它的热更新也不被冻结。
- `a_rule_without_the_merge_flag_keeps_its_own_candidates` —— **非默认路由**的规则永远看不到 `exit_nodes`。
- `a_default_route_rule_with_domains_cannot_be_combined_with_exit_nodes` —— 带 `domains` 的默认路由规则 + `exit_nodes` ⇒ 硬失败。
- `merging_exit_nodes_into_a_never_rule_is_rejected` —— `failover_when = "never"` 的默认路由规则 + `exit_nodes` ⇒ 硬失败（合并进来的出口节点永远不会被选中）。
- `a_toml_config_with_an_illegal_policy_route_is_rejected` —— `TomlConfig` 形态（**热更新路径用的就是它**）的非法组合同样被拒。
- `merging_exit_nodes_into_a_closed_rule_requires_confirmation` / `a_confirmed_closed_rule_still_loads_with_the_exit_nodes_merged_in` —— `closed` 合并后未确认 ⇒ 硬失败；确认后可加载。
- `the_merged_default_route_rule_is_reachable_and_agrees_with_l2` —— 合并后的规则**能被查表命中**，且 L2 贡献集与 L3 一致。
- `exit_nodes_without_an_explicit_default_route_keep_their_own_rule` —— 没有显式 `/0` 时，sugar 规则单独一条、候选来自活 `exit_nodes`。

**置信度**：确认（代码与设计的偏差是确定的；修复后与 `unified-routing-design.md` §5.3 的"合并候选列表，显式在前"一致，且不再冻结热补丁）。

---

### B8 🟡 — `edge_cost_from_i32` 声称的"上限"根本不存在

**位置**（符号名）：`peer_ospf_route.rs` → `edge_cost_from_i32`、`AVOID_RELAY_COST` 常量、`gen_next_hop_map_with_least_hop` 里的 `normalize_edge_cost`；`conn/conn_select.rs` → `OSPF_EDGE_COST_MAX`

```rust
/// ...the ceiling keeps a peer-reported `i32::MAX` from dominating the whole graph
/// as a de-facto veto.
match u32::try_from(cost) {
    Ok(v) => usize::try_from(v).unwrap_or(AVOID_RELAY_COST).max(MIN_EDGE_COST),
    Err(_) => MIN_EDGE_COST,
}
```

注释说的 ceiling **在代码里不存在**：`u32::try_from(i32::MAX)` 成功 ⇒ 返回 `2147483647`。

**触发条件**：peer-center 里存在某条 `DirectConnectedPeerInfo.latency_ms >= 1_000_000_000`。该字段是 `int32` 且**服务端不做任何钳制**（`peer_center/server.rs` 原样入库 `req.peer_infos`），只有发布端自己会 clamp 到 `OSPF_EDGE_COST_MAX`；被篡改/异常上报的客户端可以送 `i32::MAX`。

**后果**：

1. 该边的权重 `>= AVOID_RELAY_COST` ⇒ `normalize_edge_cost` 把它**误判成"策略惩罚边"**，在 LeastHop 表里按 `1e9+1` 计跳数 ⇒ 该边被**从所有最短跳路径中彻底剔除**（除非是唯一通路），而它在 LeastCost 表里只是"很贵"。**同一份权重在两张表里语义不同**
2. 又因为 `calculate_cost` 在缺方向数据时回退到**反向**代价，一个 peer 对自己某条链路的恶意上报会**同时污染反向边** ⇒ 该 peer 在全网从"可达"变成"不可达"

**根因**：`edge_cost_from_i32` 只做了"负数⇒`MIN_EDGE_COST`"的**下**钳制，缺上钳制；`AVOID_RELAY_COST` 的文档注释依赖"任何真实边权都远小于 1e9"这一**未被代码强制**的不变式。

**修复（✅ 已落地）**：在 `edge_cost_from_i32` 里按 `OSPF_EDGE_COST_MAX`（1 000 000）加上钳，让"真实边权"与"策略惩罚"（`AVOID_RELAY_COST` = 1e9）两段**永不重叠**。

安全性：`edge_cost_from_i32` 只在 `peer_ospf_route.rs:516` 用于**真实边权**；策略惩罚是之后**单独相加**的（`:529` 的 `OSPF_FUSE_COST_BONUS`、`:533` 的 `AVOID_RELAY_COST`），所以在这里加钳不会碰到惩罚段——钳后的真实边权 ≤ 1 000 000，加惩罚后仍 ≥ `AVOID_RELAY_COST`，`normalize_edge_cost` 的 `>= AVOID_RELAY_COST` 判据依然成立。

**置信度**：确认（注释与实现不符是客观的；触发需要恶意/异常上报，故严重度不高）。

---

## 1b. 确认的 bug（连接选择层：B9–B13）

### B9 🟠 — 丢 1 个控制包 → 质量分 +0.04（代价 +40），滞留 100 个 ping 周期

**位置**（符号名）：`peer_conn_ping.rs` → `pingpong`（loss 窗口 100 样本）、`record_latency(0/1)`、`loss_rate_stats.store`（本轮由 `as u32` 截断改为四舍五入）；`conn_select.rs` → `conn_quality_score`（`w_loss*norm_loss`）、`ospf_edge_cost_from_score`（`score*1000`）、`ConnSelectConfig::default`（`w_loss` 本轮由 4.0 改来）、`pick_default_conn` 的 `required_delta`；`conn_bond.rs` → `in_quality_band`

**触发条件**：一条已建立的 conn 上有 **1 次**保活 ping 失败（控制包丢 1 个），窗口内 100 个样本只丢 1 个。

**数字链（已逐步复算）**：

```
loss_rate = 1/100 = 0.01
  → w_loss(4.0, conn_select.rs:32，本轮改为 0.4) × 0.01 = 0.04
  → score × 1000 = +40 代价单位
```

对一条 10ms 链路，广告代价从 **10 变 50**（5 倍）；对 100ms 链路，100 → 140。

**后果**：

- **bond 质量档被击穿** —— `in_quality_band` 的档宽是 `max(相对 10%, 绝对 5ms)`；10ms 链路只有 ±5ms 档，+40 远超 ⇒ 该 conn **被踢出 bond 发送集**，其上的流被重指向兄弟 conn（内层 TCP/RDP 视作丢包）
- **单路径同理** —— `pick_default_conn` 里 incumbent 分数 +0.04，挑战者以 0.04 优势领先 ≥ `required_delta`（`max(0.1×score, 0.005)`）→ 连续 3 窗口后切走
- **滞留时间 100 个 ping 周期** —— 忙时 ping 间隔 1s → ~100s；空闲退避到 `ping_interval_max_sec=10s` → **~1000s ≈ 17 分钟**。即"丢 1 个控制包"把链路的广告代价和成员资格污染十几分钟

**根因**：丢包率被量化成 **1% 步长**，而 `w_loss=4` 经 `×1000` 放大后 **1% = 40 个代价单位**，而 RTT 项每毫秒才 1 个单位 —— 质量分实际是"**40 级台阶 + 毫秒级微调**"。100 样本固定窗口 + 1s~10s 的自适应 ping 间隔，使单次事件被放大并滞留两个数量级的时间。

> `docs/current/peer-connections.md:77` 写的"单次抖动不会污染判断"对 RTT/jitter 成立，**对 loss 不成立**。

**修复（✅ 已落地，采用本文档列出的第三条）**：

**先排除前两个选项 —— 经复核两者都不成立**：

| 选项 | 复核结论 |
|---|---|
| 丢包率改用 **EWMA** | ❌ 滞留**更长**。固定窗口里那个失败样本会在 100 个新样本后被挤出去（硬淘汰）；EWMA 是指数衰减，α=0.01 时单次失败的影响要 ~460 个周期才衰减到 1% 以下。EWMA 改善的是"无界窗口"的漂移，不是本条的问题 |
| 丢包率改用 **Laplace 平滑** `(fail+1)/(n+2)` | ❌ 单次事件影响**翻倍**：1 次失败 / 100 样本从 1.0% 变成 2/102 = 1.96%。平滑是往"有偏"方向走，与本条诉求相反 |

**根子上，"固定样本窗口 + 比例估计"这个模型里，"幅度 × 滞留时间"是个常数**：单次事件的代价 = `w_loss × 1000 / N`，滞留 = N 个周期。缩窗口会让幅度变大、恢复变快；改估计模型则两头不讨好。**唯一能真正降低单次事件影响的，是降低 `w_loss` 本身。**

采用第三条：`w_loss` 默认值 **4.0 → 0.4**（`conn_select.rs` 的 `ConnSelectConfig::default()`，以及 `config/toml.rs` 的 `conn_select_w_loss: 400 → 40`，两处必须同步）。

| 量 | 修复前 | 修复后 |
|---|---|---|
| 丢 1 个包（窗口 100 ⇒ 1% 丢包） | +40 代价单位 | **+4 代价单位** |
| 对比：发布死区 `OSPF_EDGE_COST_MIN_DELTA` | 20 | 20（**现在 +4 在死区内** ⇒ 不再触发一次全网边权重发布） |
| 对比：bond 质量档宽（≥5ms 等值） | 5 | 5（**现在 +4 在档内** ⇒ 不再击穿 bond 成员资格） |
| 20% 丢包（未熔断） | +800 | +80 |
| 20% 熔断阈值 | 20% | **20%（不变）** |

**回归用例**（`conn_select.rs`）：

- `a_single_lost_probe_stays_inside_the_publish_deadband` —— 1% 丢包造成的代价增量必须 < 发布死区。
- `a_lossy_link_still_costs_more_than_a_slow_clean_one` —— 降权后仍必须把"5ms+30% 丢包"排在"25ms+1% 丢包"之后（防止降权过头）。

**注**：显式配置了 `conn_select_w_loss = 400` 的用户行为**不变**（`from_flags` 取字面值），只有默认值变了。

**置信度**：确认（数值链可逐步复算；窗口滞留时长取决于 ping 间隔）。

---

### B10 🟠 — 未完成首次 ping 往返的 conn 被上报为代价 1（全网最小边权）

**位置**（符号名）：`conn_select.rs` → `score_conn`（`unverified_hole_punch` 判据）、`ospf_edge_cost_from_score`（下界）；`peer_map.rs` → `best_edge_cost`（修复前只过滤 `unverified_hole_punch`，本轮加了未测量分支）；`peer_manager.rs` → `set_is_hole_punched(!is_directly_connected)` 调用点

**触发条件**：任意新建的**直连或中继** conn（`is_directly_connected = true` ⇒ `is_hole_punched = false`）在收到第一个 pong 之前：此时 `latency_us = 0`、`loss_rate = 0`、`jitter = 0` ⇒ `score = 0` ⇒ `cost = round(0) = 0` ⇒ `clamp(1, 1e6)` ⇒ **1**。

**后果**：1 是 `edge_cost_from_i32` 的下界（`MIN_EDGE_COST = 1`），即**全图最便宜边权**。上报周期 5s（`peer_center/instance.rs`）会在该窗口内采到它：

- 健康链路：窗口 ≈ 1 个 RTT，影响小
- **断链链路**：首个 ping 2s 超时前一直 `latency_us = 0` ⇒ 窗口 ≈ 2s，采样命中概率约 40%。这段时间内全网（`latency_first=true` 默认走 LeastCost）把"经本节点到该 peer"当成**全图最优路径**；若该 conn 是黑洞，流量被吸进黑洞，直到下一次上报（≤5s）才被 +10000 的熔断代价纠正

**对照**：`unverified_hole_punch` 的设计意图（`core_peer.proto:39-42`："排除出 default_conn 选择与 OSPF 边权上报"）只对 hole-punch conn 生效，**直连/中继 conn 逃逸出了这个保护**。

**根因**："未测量"状态没有显式建模，只用 `latency_us == 0` 隐式表达，且该隐式只挂在 `is_hole_punched` 上；而 `clamp` 的下界 1 又把"未知"翻译成"最优"。

**修复（🟡 部分落地）**：

**已做（代价侧）**：`best_edge_cost`（`peer_map.rs`）现在把**未测量**的 conn 与**已测量**的 conn 区分开：质量分恰好为 0（即 `latency_us == 0` —— 最小实测 RTT 是 1µs，量级 1e-6，所以这个判据不会误伤真实测量）时，发布 `OSPF_EDGE_COST_UNMEASURED = 500` 而不是下界 1。⇒ 未测量的 conn 不再是**全网最便宜边权**，但仍然"乐观可用"（500 远小于熔断段的 10 000），不会把新建立的连接排除出路由。

**未做（conn 选择侧）**：`score_conn` 里的 `unverified_hole_punch = m.is_hole_punched && m.latency_us == 0` 仍只覆盖打洞 conn（`peer_manager.rs:2750` 用 `set_is_hole_punched(!is_directly_connected)` 设置 ⇒ **直连 conn 恒为 false**）。后果：一条**直连但未测量**的 conn 在 `pick_default_conn` 里会被当作"已验证"，优先于真正测量过的中继 conn。要修需要把"未测量"从 `is_hole_punched` 里解耦出来 —— 而 `unverified_hole_punch` 是 **protobuf 字段**（`core_peer.proto`），语义改名要动协议，故本轮未动。

**置信度**：确认（路径与数值确定）；**影响窗口窄**，严重度中。

---

### B11 🟠 — 滞回基线是进程级 static，`retain` 跨 peer map 互清 ⇒ 发布防抖完全失效

**位置**（符号名）：`peer_map.rs` → `PeerMap::ospf_edge_cost_baseline`（修复前是 `static LAST_OSPF_EDGE_COST: LazyLock<DashMap<PeerId, i32>>`）、`ospf_edge_cost_with_hysteresis`（读/写基线）、`retain_ospf_edge_cost_baseline`（`retain`）

```rust
// peer_map.rs:42 —— 进程级全局，只按 PeerId 作键
static LAST_OSPF_EDGE_COST: LazyLock<DashMap<PeerId, i32>> = LazyLock::new(DashMap::new);

// peer_map.rs:690
let last = LAST_OSPF_EDGE_COST.get(&peer).map(|v| *v);
let cost = apply_ospf_cost_hysteresis(raw_cost, last, OSPF_EDGE_COST_MIN_DELTA);
LAST_OSPF_EDGE_COST.insert(peer, cost);

// peer_map.rs:698-699 —— 注释说「我们不再通告的 peer」
LAST_OSPF_EDGE_COST.retain(|id, _| ret.direct_peers.contains_key(id));
```
**触发条件**：节点配置了至少一个外网。进程内存在两个 `direct_peer_info` 调用方，peer 集合不同：

| 调用方 | 位置 | peer 集合 |
|---|---|---|
| 主实例 | `peer_manager.rs:2483` `direct_peer_info(&[主 peer_map, foreign client peer map])` | 主 + 外网客户端 |
| 每个外网实例 | `foreign_network/mod.rs:161` `direct_peer_info(std::slice::from_ref(&self.peer_map))` | 仅该外网 |

PeerId 是 `random_peer_id()` 逐进程随机，两组集合基本不相交 ⇒ **任一方的 `retain` 都会删掉另一方的基线**。

**后果**：`last` 恒为 `None` ⇒ `apply_ospf_cost_hysteresis` 退化成恒等函数 ⇒ 发布的边权 = 原始测量值，**发布防抖（`|Δ| < 20` 不改写）彻底失效**。

叠加 B9 的 40 单位/1% 台阶 ⇒ 上报代价每次都变 ⇒ peer-center `report_can_be_skipped` 永远返回 false ⇒ 5s 一报 ⇒ **全网 SPF 被反复触发改路**。

**根因**：滞回基线的**生命周期**（全局 static）与它的**使用者**（per-peer-center 实例）不一致，且 `retain` 的作用域是"本次调用的 peer 集合"而不是"本实例负责的 peer 集合"。

**修复（✅ 已落地）**：滞回基线从进程级 `static` 改为 **`PeerMap` 实例级字段** `ospf_edge_cost_baseline`：

- `PeerMap::ospf_edge_cost_with_hysteresis(peer, raw)` —— 读自己的基线、算滞回、写回自己的基线。
- `PeerMap::retain_ospf_edge_cost_baseline(&advertised)` —— **只裁剪本实例**负责的 peer 集合。
- `direct_peer_info` 改为**按 peer map 逐个处理**：每个 map 用自己的基线算自己那些 peer 的滞回，并用**自己**广告到的集合裁剪自己的基线 ⇒ **一个 map 的 `retain` 不可能再清掉另一个 map 的基线**。
- "第一个能连上某 peer 的 map 拥有该 peer 的边权"这条语义保持不变（`ret.direct_peers.contains_key(&peer)` 时跳过），所以上报值仍与 `peer_maps` 的顺序无关。

**置信度**：确认。

---

### B12 🟡 — 发布死区只有绝对项、没有相对项

**位置**（符号名）：`conn_select.rs` → `apply_ospf_cost_hysteresis`

**触发条件**：边代价处于典型量级（10~50，即 10~50ms RTT）时发生劣化。

**后果**：死区是 `|raw - prev| < 20`（**绝对**）。实测：RTT 10ms（cost 10）劣化到 28ms（cost 28），`|28-10| = 18 < 20` ⇒ **仍上报 10** —— 即"链路慢了 2.8 倍"对全网不可见，直到代价 ≥30 才发布。**相对容差高达 40%~200%。**

**根因（本文档此前的表述需要修正）**：此前说"同仓另一处死区实现 `within_switch_band`（`peers/route/local_route_table.rs:493-499`）用的是相对项(10%) OR 绝对项(5 单位)，高代价时由相对项主导；发布端只抄了绝对项，两处口径不一致"。

**⚠️ 本轮复核发现：这两处本来就不该用同一公式。**

| | `within_switch_band` | `apply_ospf_cost_hysteresis` |
|---|---|---|
| 回答的问题 | 要不要把 incumbent **换成** best？ | 要不要**改写对外发布的**边权？ |
| 语义 | "保守，不切换" ⇒ 死区宜**小**（敏感） | "防抖" ⇒ 死区宜**大**（迟钝） |
| 取法 | 相对项 **OR** 绝对项 ⇒ 谁小用谁 | 只有绝对项 |

**此前给出的修法与本条自己的"后果"描述矛盾**：`|raw-prev| < max(min_delta, |prev| * 10%)`，代入本条举的例子（prev=10、raw=28）⇒ `max(20, 1) = 20` ⇒ `|18| < 20` ⇒ **仍然上报 10，劣化照样不可见**。这个修法实际做的是把**大代价**边的容差从 2% 提到 10%（让劣化更晚发布），与"要更早看见劣化"的诉求方向相反。

**真正的取舍**（要动就得先定这个）：

- 要让"10ms → 28ms"可见，死区必须 **< 18** ⇒ 绝对下限 20 已经太大，必须**降低绝对下限**或**改成纯相对**；代价是低代价边的发布会明显变频繁（每次 RTT 微抖都可能改写全网边权 → SPF 被反复触发）。
- 保留纯相对（`max(小绝对下限, |prev| * 10%)`）：10ms 链路死区 ≈ 5、100ms 链路 ≈ 10、1s 链路 ≈ 100 ⇒ 大链路的噪声被容忍，小链路的劣化可见；但**发布频率**对每条链路都上升。

**处置（⏸ 待决）**：本轮**不动**。理由：这是**发布端防抖强度**的产品语义决策，不是客观缺陷；改错会直接改变全网边权的发布频率 ⇒ SPF 抖动面远大于本条本身（🟡 低）。需要先定"发布端死区"的契约，再改。

**置信度**：确认（行为确定）。"是否算 bug"取决于设计意图 —— **死区方向本身没错**（见 §4），缺的是相对项。

---

### B13 🟡 — 丢包率用 `as u32` 截断，3 个取值被系统性低报

**位置**（符号名）：`peer_conn_ping.rs` → `pingpong` 里的 `loss_rate_stats.store`

```rust
self.loss_rate_stats.store((loss_rate_1 * 100.0) as u32, Ordering::Relaxed);
```

**触发条件**：窗口内失败数 / 样本数 ∈ {29, 57, 58} / 100。

**后果**：f64 下 `29/100*100 = 28.999999999999996` → 截断成 28（57→56、58→57）。已穷举 0..100 全部 101 个取值，**只有这 3 个出错**，说明是浮点表示导致的系统性偏低而非随机。真实 29% 丢包上报成 28%，score 少 0.04（4 个代价单位）；熔断阈值 0.20 不受影响 ⇒ 严重度低。

**根因**：用浮点百分比再截断，而不是直接存窗口内的失败次数。

**修复（✅ 已落地）**：改用四舍五入并夹住范围：

```rust
self.loss_rate_stats.store(
    (f64::from(loss_rate_1) * 100.0).clamp(0.0, 100.0).round() as u32,
    Ordering::Relaxed,
);
```

`clamp` 顺带把负数与 NaN 挡住（`as u32` 对它们是饱和转换，虽不 UB 但语义不清）。**未采用"直接存失败计数"** —— `loss_rate_stats` 是对外发布的百分数字段（`AtomicU32`），改成计数会动协议。

**置信度**：确认（已复算），严重度低。

---

## 1c. 存疑项补充：S5 — fast probe 降级位被两个信号双写

**位置**：`peer_conn_ping.rs:418-419`（保活成功 → `fast_probe_degraded/sustained = false`）vs `:510-518`（fast probe 每 333ms 按自己的 `misses` 重新置位）；`peer_conn.rs:1586-1591`（`loss_rate()` 在 degraded 时返回 1.0）；`peer_conn_ping.rs:95-98`（`fast_probe_timeout = clamp(3×RTT均值, 300ms, 2s)`）

**触发条件**：链路 RTT 抖动使 fast probe 连续 3 次超时（实际 RTT 超过 `3×RTT均值`），但保活（2s 固定超时）仍成功 —— 典型是"均值 60ms、偶发尖峰 400ms"的链路。

**后果**：`loss_rate()` 在 1.0 与正常值之间以保活周期（1~10s）摆动 ⇒ `fused` 反复翻转 ⇒ 边权在 `~cost` 与 `~cost+10000+4000` 之间摆动（发布死区 20 完全吸收不了）⇒ **全网 SPF 每次都被触发**；同一 conn 在 bond 集内/外反复进出 ⇒ 其上的流被反复重指向。

**根因**：降级结论位由**两个独立信号**共同写。保活的一次成功就能抹掉 fast probe 连续多轮 miss 得出的结论（`FAST_PROBE_SUSTAINED_ROUNDS` 的"不可打断的一串 miss"语义被破坏），而 fast probe 的 `misses` 计数又没有被保活归零，于是 333ms 后结论又被原样重建 —— 两个写者之间没有统一的"证据"定义。

**修复**：降级位只由 fast probe 自己的 `misses` 写；保活成功应当**归零 fast probe 的 `misses`**（而不是直接清结论位），使"保活成功"成为与 fast probe 同源的一票。

**置信度**：很可能（代码路径确定；触发依赖链路抖动幅度，未在现网取证）。

---

## 1d. 中转 / 直连补审（B14–B16）

> 本轮补审：**"内部节点之间的中转 vs 直连"**这条线。**初次结论有误，二次复核已推翻其中 B14**——详见该条。**B15 / B16 保留**（代码事实成立）。

### ❌ B14 —— 复核后**撤回**：「直连可用却走中转」是误判

> **2026-10-19 二次复核：本条撤回。** 原始主张（"默认 `latency_first = true` 走 `LeastCost` ⇒ 对已直连的 peer 没有直连优先，可被绕到第三方中转"，以及建议的修法"把『有可用直连』提升为**与 policy 无关的快速路径**"）**与项目既有的设计契约和守卫测试直接冲突**。原文字段保留在下面，便于追溯"为什么当初看起来像 bug"。

**位置**：`peers/conn/peer_map.rs:274-290`（关键行 `:288`）、`peers/peer_manager.rs:197`（`get_next_hop_policy`）、`config/toml.rs:168`（`latency_first: true`）、`peers/peer_manager.rs:4519-4546`（发包分支顺序）

```rust
// peers/conn/peer_map.rs:283-290
// LeastHop short-circuit: a direct conn is the fewest-hop path. ...
if matches!(policy, NextHopPolicy::LeastHop) && self.has_peer_with_live_conn(dst_peer_id) {
    return Some(dst_peer_id);   // ← 只有 LeastHop 会「有直连就直连」
}
```

**为什么这不是 bug（三条独立证据）**：

**证据 1 —— 有守卫测试，且测试注释点名要防的就是本条的修法。**
`easytier-core/src/peers/route/graph_algo.rs:373-398`：

```rust
/// The hop-count preference must stay *secondary*: a strictly cheaper
/// multi-hop path still wins. This is the guard against "prefer direct" being
/// over-applied — e.g. a ~13ms direct link vs a 2-hop path costing ~2.
#[test]
fn strictly_cheaper_multi_hop_path_still_wins() {
    graph.extend_with_edges([(a, d, 13), (a, b, 1), (b, d, 1)]);
    ...
    assert_eq!(first_hop[&d], (b, 2), "cost is primary; hops only break ties");
}
```

**本条建议的修法 1（直连优先、与 policy 无关）会让这个测试失败。** 测试注释里的 "the guard against 'prefer direct' being over-applied" 就是在防这件事。

**证据 2 —— 设计文档把比较规则写死为字典序，跳数是第二键。**
`docs/roadmap/live-next-hop.md:38`：

> | 2 | **跳数** `hops` | 等代价时取短路径——这就是「同质量优先直连」（直连恒为 1 跳） |

⇒ **`LeastCost` 表本来就偏向直连**，只是作为**等代价时的第二键**。`:530` 明确写「字典序 `成本 → 跳数 → 稳定身份键`」。

**证据 3 —— 文档明文禁止"直连无条件优先"，并有验收负例。**

| 位置 | 原文 |
|---|---|
| `live-next-hop.md:443` | **不误判代价**：一条 13ms 直连 vs 两跳各 <0.5ms 的中继 ⇒ **应选中继**。防止有人用「直连无条件优先」来「修」本问题（§5.4） |
| `live-next-hop.md:484` | **负例**：造「13ms 直连 + 两条各 <0.5ms 的两跳」，`route` 必须选**中继**（防止把「直连优先」过度应用） |
| `live-next-hop.md:461` | `sustainedly_degraded_own_edge_is_outpriced_by_a_costlier_relay`（**直连持续降级 ⇒ 25 倍贵的中继顶上**） |

**原始主张的事实层面哪些成立、哪些不成立**：

| 原始说法 | 复核结论 |
|---|---|
| 直连短路只写在 `LeastHop` 分支 | ✅ **成立**（`peer_map.rs:288` 确实带 `matches!(policy, NextHopPolicy::LeastHop)` 限定） |
| 默认 `latency_first = true` ⇒ 走 `LeastCost` | ✅ **成立**（`config/toml.rs:168` → `peer_manager.rs:197` → `context.rs:537` 的 `flags.latency_first && !flags.p2p_only`，默认 `p2p_only=false`） |
| 「两套 policy 行为不一致」 | ❌ **不成立**。`LeastHop` 短路只是**快路径**：直连恒为 1 跳，在 LeastHop（每条边代价归一为 1）下必胜，跳过查表不改变结果。**两张表的结果一致** |
| 「全仓未见说明 `LeastCost` 不该偏向直连的注释或测试」 | ❌ **事实错误**。`live-next-hop.md:38/443/459/461/484` 就是那份说明，且 `graph_algo.rs:377` 是专门的守卫测试 |
| 「直连可用却走中转」是缺陷 | ❌ **不成立**。绕行路径总代价更低时选中继，**正是契约要求的行为**（成本为主键） |
| 修复建议 1：直连作为与 policy 无关的快速路径 | ❌ **不能采纳**。会让 `strictly_cheaper_multi_hop_path_still_wins` 失败，并违反 `live-next-hop.md:443/484` 的验收契约 |

**唯一有残余价值的部分**（保留，但不作为 bug）：

- **无路径粘性**：`send_msg_internal` 逐包重新决策，没有 per-flow 钉住 ⇒ 两条路径代价交叉时同一条 TCP 流的包会中途换路。但这**不是新发现** —— `live-next-hop.md:451` 明确记录「切换阈值」方案**已撤回**，抖动交由**边级**迟滞 `apply_ospf_cost_hysteresis` 负责。**不过 B11 会让这条边级迟滞在"配了外网"时失效**（见 §5 交互提示），那属于 B11 的连带影响，不另开条目。
- **两处 policy 的短路逻辑不统一**，纯属可读性问题：若希望 `LeastCost` 也有等价快路径，应写成一个与 policy 无关的「直连可用且代价未被判定劣化」判断，**但那必须复用 B9/B10 修好后的代价语义**，且不得违反上述契约。

**处置**：撤回为「误判」，移入 §3（设计有意为之）。**不做代码改动。**

---

### B15 🟡 — `forward_counter > 2` 静默清掉 `latency_first`

**位置**：`peers/peer_manager.rs:4170-4173`（**全仓唯一一处**）

```rust
// peers/peer_manager.rs:4153-4175（转发收包分支内）
if to_peer_id != self.my_peer_id {
    if hdr.forward_counter > 7 { ... return; }        // ← 8 跳截断
    ...
    if hdr.forward_counter > 2 && hdr.is_latency_first() {
        tracing::trace!(?hdr, "set_latency_first false because too many hop");
        hdr.set_latency_first(false);                 // ← 本条
    }
    hdr.forward_counter += 1;                          // ← 自增在判定之后
}
```

**触发条件**：包经过的转发跳数超过 3。

**精确语义（修正原文的"转发计数 ≤ 2"）**：判定用的是**自增前**的 `forward_counter`，且 `+= 1` 在判定**之后**。逐跳推演（A 为源，B 为终点，路径 A→C→D→E→B）：

| 节点 | 收到的 `forward_counter` | `> 2`? | 自增后 |
|---|---|---|---|
| C（第 1 跳） | 0 | 否 | 1 |
| D（第 2 跳） | 1 | 否 | 2 |
| E（第 3 跳） | 2 | 否 | 3 |
| F（第 4 跳） | 3 | **是 ⇒ 清位** | 4 |

⇒ **latency-first 存活到第 3 跳，从第 4 个转发节点起被清除**，其后全程按 least-hop 选路。

**后果**：深路径**静默**退回 least-hop（只有 `trace!` 级日志）。对用户可见口径的影响：

- GUI「延迟」列在中转行显示的是 least-cost 表的 `path_latency`（见 `latency-measurement-and-display-audit.md` §3.2 / P1），而深路径实际走的是 least-hop 表 ⇒ **显示的路由代价不是正在用的那条路**
- 与 8 跳截断（`peer_manager.rs:4154`）是**两套不同阈值**：8 跳是截断，>2 是降级 latency-first

**已被记录的部分**（修正原文"此前未被任何文档记录"的说法）：`docs/roadmap/live-next-hop.md:141` 已写明——

> `forward_counter > 7`（`peer_manager.rs:3706`）只是**截断**，不是防环：一个本该立刻丢弃的包变成约 7–8 次跨节点转发（期间 `forward_counter > 2` 还会关掉 `latency_first`，`peer_manager.rs:3722`）。

（该处行号已漂移：`peer_manager.rs:3706` → `:4154`，`:3722` → `:4172`。）⇒ **本条不是"新发现"，而是"已记录事实的量化补全 + 行号校正 + 补上 GUI 显示口径的连带影响"。**

**修复**：把该降级条件与真实理由对齐（防环？防长路径抖动？），并在状态面暴露"本路径未按 latency-first 选路"，而不是静默。

**置信度**：确认（代码事实）。是否合理取决于降级动机，但**代码里没有注释说明为什么是 2**。

---

### B16 🟡 — 转发节点用发包方的 `latency_first` 位，而非本地配置

**位置**：盖位方 `peers/peer_manager.rs`（多处 `hdr.set_latency_first(packet_policy.latency_first)`，来源 `context.packet_policy()`）；读位方 `send_msg_internal` 的 `msg.peer_manager_header().unwrap().is_latency_first()`

**后果**：`is_latency_first` 是**源节点**按自己的 `context.packet_policy()` 盖在包头上的位，转发节点读包头决定查 least-cost 还是 least-hop 表 ⇒ **本地 `latency_first` 配置对过境流量不具权威性**。一台刻意配 `latency_first = false`（省电/省 CPU/求稳定）的节点，仍会按上游节点的位去做 least-cost 选路。

这与 `latency-measurement-and-display-audit.md` §4 的 P1 互为补充：那份文档指出**前端**无条件优先 least-cost 字段，本文指出**转发侧**也由包头位决定，两者叠加使"实际在用哪张表"在整条路径上都难以确定。

**修复**：明确该位的语义边界——要么承认它是端到端意图（则应让本地配置成为上限，例如 `local_latency_first && hdr.is_latency_first()`），要么改为逐跳由本地配置决定；两种都可以，但需要在文档里写死，并在状态面可观测。

**置信度**：确认（代码事实）。

---

## 1e. 直连 vs 中转、出口归属：实测结论（2026-10-19 第三轮）

> 本节回答一个具体问题：**两个节点之间有更快的中转时，系统走中转还是直连？**
> 结论分两层：**网内（目标是 mesh peer）是对的**；**网格外（目标是网外设备）是错的**。
> 全部结论都有可复跑的用例或代码路径支撑，不依赖推断。
>
> ⚠️ **本节的实测数据采集于 B1 修复之前**，用于证明"修复前是什么样"。**修复后的行为见 §1e.5**。网内结论（1e.2）不受修复影响。

### 1e.1 前提：三条容易混淆的"路径"

| 概念 | 含义 | 由谁决定 |
|---|---|---|
| **边代价** | 一条直连 conn 的质量编码代价 ≈ 实测 RTT（ms 当量） | `conn_select::ospf_edge_cost_from_score` |
| **下一跳表** | 到某个 mesh peer 走哪条路 | `peer_ospf_route.rs` 的 LeastCost / LeastHop 两张表 |
| **出口归属** | 一个**网外**目标由哪个 peer 代理 | `is_new_peer_better` 仲裁 `0.0.0.0/0` / proxy CIDR 的归属 |

⇒ **网内走 LeastCost 表（按总代价）**；**网外先选归属，再由 LeastCost 表决定怎么到那个归属 peer**。这两步是**独立**的。

### 1e.2 网内：直连 vs 中转 —— **正确**

实测（`peer_ospf_route_tests.rs`，`FixedEdgeCosts` 固定边权）：

| 用例 | 边权 | 实际选出 | `path_len` | `path_latency` | 判定 |
|---|---|---|---|---|---|
| CASE 1.1 | A—B 100ms，A—C 50ms，B—C 100ms | **直连 B** | 1 | 100 | ✅ 直连 100 < 中转 150 |
| CASE 1.2 | A—B 100ms，A—C 50ms，B—C 20ms | **中转 A→C→B** | 2 | 70 | ✅ 中转 70 < 直连 100 |

**判定完全符合 `live-next-hop.md` 的契约**（`:443`「一条 13ms 直连 vs 两跳各 <0.5ms 的中继 ⇒ **应选中继**」）。**这同时是 B14 撤回的直接依据** —— 「直连可用却走中转」不是缺陷。

**用例**：`direct_edge_wins_when_the_relay_is_more_expensive`、`cheaper_multi_hop_relay_wins_over_the_direct_edge`。

**A1→B1 的完整数据路径**（CASE 1.2）：

1. A1 的包进 A 的 TUN，dst = B1 的虚拟 IP；
2. `get_msg_dst_peer_ipv4(B1)` → `get_peer_id_by_ipv4` 命中 `ipv4_peer_id_map` ⇒ dst = B（唯一归属，无争用）；
3. `send_msg_internal`：`get_gateway_peer_id(B, LeastCost)` 返回 **C**（≠ B，未被 `.filter()` 滤掉）⇒ 走 `relay_peer_map.send_msg(msg, B, LeastCost)`；
4. A 与 B 经 C 完成**中转握手**，密文交给 C；
5. C 收包（`to_peer_id = B ≠ C` ⇒ 转发分支）⇒ C 自己的 LeastCost 表算出到 B 是**直连** ⇒ C 直发 B。

⇒ **最终链路 A→C→B**。注意"到 B 的传输路径"与"B1 归属"是两个独立决策：即使归属落在 B，**到 B 的传输仍然可以经 C 中转**（本例中 C 到 B 是直连）。

**延迟口径**：`path_latency = 70` 是**两个 RTT 之和**（A↔C 50ms + C↔B 20ms），单向数据延迟约 35ms；直连时 `path_latency = 100` 对应单向约 50ms。

### 1e.3 网外：出口归属 —— **错，且与代价无关**

前提：局域网外设备 D1；B、C 都能到 D1（各自经自己的物理出口）。**A 不知道 B→D1、C→D1 各是多少** —— D1 不是 mesh peer，它的代价进不了任何一条边。

| 用例 | A→B | A→C | B→D1 | C→D1 | **最优** | **修复前实际** |
|---|---|---|---|---|---|---|
| CASE 2.1 | 100 | 50 | 100 | 100 | **C**（150） | 归属与代价**无关**（可能 B=200） |
| CASE 2.2 | 100 | 150 | 100 | 100 | **B**（200） | 归属与代价**无关** |
| CASE 2.3 | 100 | 150 | 100 | **20** | **C**（170） | **B**（200）—— 所有机制都会选错 |

**实测证据**（`FixedEdgeCosts` + 真实 `OspfRouteTable`）：

- 同一张表上把 A→B / A→C 的边权**对调**，归属**纹丝不动** ⇒ 归属判据**从不看代价**（这正是 B1）。
- A—B 直连 1000ms、A—C 1ms + C—B 1ms（到 B 共 2 跳、2ms）时，归属落在 **B** ⇒ **1000ms 的直连 peer 保住了出口，2ms 的两跳路径拿不到**。
- 归属由 `next_hop_map`（`DashMap`）的迭代顺序决定，顺序来自**表创建时**随机生成的哈希种子 ⇒ **同一网络里不同节点可以给出不同的归属**。

**出口归属一共四条机制路径，代价的参与情况**：

| 机制 | 判据 | 看代价？ | 看最后一跳？ |
|---|---|---|---|
| `exit_nodes` 配置列表 | **列表顺序**（`peer_map.rs:405-431`） | ❌ | ❌ |
| policy route `select_by = priority` | 候选列表顺序 | ❌ | ❌ |
| policy route `select_by = quality` | `peer_edge_cost` = **A→该 peer 的直连边**代价 | ⚠️ 只看第一跳 | ❌ |
| policy route `select_by = balance` | 流哈希轮询 | ❌ | ❌ |
| 对端广告的 `0.0.0.0/0` | `is_new_peer_better`（**B1**，修复前只看跳数） | ❌ | ❌ |

⇒ **CASE 2.3 是真正的错误答案**：唯一"会看代价"的机制（`select_by = quality`）比的是 **A→B 的直连边**（100 < 150）⇒ 选 B ⇒ 端到端 200ms，而走 C 只要 170ms。

**修复后**：`is_new_peer_better` 改成成本感知字典序后，归属会跟随 **A→各出口** 的代价（CASE 2.1 → C、CASE 2.2 → B）—— 但 **CASE 2.3 仍然错**（A→B 100 < A→C 150 ⇒ 选 B），因为**最后一跳的代价在协议里没有传播路径**。

### 1e.4 B17 🔴 — 最后一跳（出口节点 → 网格外目标）的代价没有传播路径

**这不是某一行代码的笔误，是协议层的结构性缺口。**

| 位置（符号名） | 事实 |
|---|---|
| `peer_manager.rs` → `peer_edge_cost` | `best_edge_cost(list_peer_conns(peer_id))` = **A 到该 peer 的直连边**代价（不含中转路径，更不含最后一跳） |
| `peer_manager.rs` → quality 打分 | `select_by = quality` 用的是**整条路径**代价（`peer_path_cost`，本轮 B18 改过），直连边代价只作兜底 |
| `peer_map.rs` → `resolve_exit_node_peers` / `resolve_exit_node_peer` | = **配置列表顺序** |
| `local_route_table.rs` → `pick_balance` | `select_by = balance` = 流哈希 |
| `peer_ospf_route.rs` → `get_peer_id_for_default_route_proxy` | 归属来自 `cidr_peer_id_map`（B1） |
| **协议层** | mesh peer 的路由信息里**没有**"我到某个网外目标有多远"这个量；`proxy_cidrs` 只声明"我代理这段地址"，不带代价 |

⇒ **任何出口选择机制都拿不到"出口节点 → D1"的距离** ⇒ "选离 D1 更近的那个出口"这件事**当前协议做不到**。

**可选路径（需设计评审）**：

1. **让出口节点把"到网外目标的代价"上报进路由信息**（扩展 `RoutePeerInfo`，例如按 proxy CIDR 附带一个代价）—— 协议扩展，跨版本兼容要设计；
2. **端到端探测**（出口节点主动测一次到目标的往返并发布）—— 正是此前被撤回的"出口端到端探测"，复杂度与稳定性收益当时判定不成比例；
3. **只做第一跳最优**（当前行为）—— 把"选哪个出口"降级为明确的**配置语义**（如 `exit_nodes` 列表顺序 = 优先级），并在 GUI 上如实显示"按优先级选择出口，不比较到目标的距离"。

**处置（⏸ 待设计）**：本轮**不改**。但**必须先在文档/UI 上把"出口是按配置优先级选的、不是按到目标的距离选的"这件事说清楚** —— 否则用户会以为系统选的是更近的出口。

**置信度**：确认（逐条核对到代码行）。

### 1e.5 修复后的行为（B1 落地后）

| 场景 | 修复前 | 修复后 |
|---|---|---|
| CASE 2.1（A→B 100 / A→C 50） | 归属与代价无关（可能 B） | **C**（第一跳代价低者胜）✅ |
| CASE 2.2（A→B 100 / A→C 150） | 归属与代价无关 | **B** ✅ |
| **CASE 2.3**（A→B 100 / A→C 150，C 离 D1 只有 20ms） | B（200ms） | **仍是 B（200ms）** ❌ —— 最后一跳的代价不可见（B17） |
| 归属的确定性 | 依赖 `DashMap` 迭代顺序 ⇒ **跨节点可能不一致** | 完全等距时按 `peer_id` 决胜 ⇒ **跨节点一致** ✅ |
| 网内 CASE 1.1 / 1.2 | 直连 / 中转 | **不变**（不经过归属仲裁） |

⇒ **可以对外说**："出口选择现在在**能看到的代价（第一跳）**上最优，且跨节点一致"。
⇒ **不能说**："出口选择已经按端到端代价最优" —— CASE 2.3 仍然错，根因是 B17。

---

## 1f. 策略路由（`select_by`）层的两条（2026-10-19 第三轮）

> 这一节来自一次针对"`select_by` 的 priority / quality / balance 到底实现了没有、是否合理"的核查。三种模式**都真实实现了**（不是占位），但有两个前提条件会让它们**静默退化**，另有实现层面的两处缺陷。

### 1f.0 三种模式的实现现状

| `select_by` | 实现位置（符号名） | 判据 | 切换行为 |
|---|---|---|---|
| `priority` | `peer_manager.rs` → `reconcile_local_routes_once` 的 `best` 匹配 | 候选列表里第一个"可用"的 | 优选恢复后**立刻**切回（列表顺序就是策略） |
| `quality` | 同上 | 候选里 cost 最小 | 需连续 `switch_windows`（默认 3）tick 超出切换带才切 |
| `balance` | 同上 + `local_route_table.rs` → `pick_balance` | 热路径 `flow_key % n` | 不切，候选集变了才重算 |

`parse_select_by`（`local_route_table.rs`）只接受 `priority|quality|balance`，拼错在配置校验时报错 ✓。

### 1f.1 两个前提条件（不满足 ⇒ 静默退化）

**① 只有 `[[policy_routes]]` 支持这三种；`exit_nodes` 不支持。**

`peer_manager.rs` → `build_local_route_table` 明确注释：

> Zero-regression: exit_nodes-only configs keep the empty table and step-4 live list (`update_exit_nodes`). Do **not** bake exit_nodes into sugar rules here — that would freeze hot-patched exit peers on construct-time A.

⇒ `exit_nodes` 永远走老路 `resolve_exit_node_peer`（`peer_map.rs`）= **配置列表顺序**，**没有 quality / balance**。

**② `failover_when` 决定候选集大小。**

```rust
// peer_manager.rs → reconcile_local_routes_once
let candidates: &[IpAddr] = match r.rule.failover_when {
    FailoverWhen::Never => r.rule.peers.get(..1).unwrap_or(&[]),   // ← 只取第 1 个
    FailoverWhen::Unreachable | FailoverWhen::Degraded => &r.rule.peers,
};
```

| `failover_when` | 候选集 | `select_by` 是否生效 |
|---|---|---|
| `unreachable`（**默认**，`config/api.rs:71`） | 全部 | ✅ |
| `degraded` | 全部 + 成本上限过滤 | ✅ |
| `never` | **只有第 1 个** | ❌ **完全失效** ⇒ **配置校验直接报错**（见 B19） |

### B18 ✅ 已修 — `select_by = quality` 的打分只算**直连**链路成本，经中转可达的候选恒垫底

**位置**（符号名）：`peer_map.rs` → `peer_edge_cost`、`peer_manager.rs` → quality 打分处、`local_route_table.rs` → `is_degraded_cost`

```rust
// peer_map.rs → peer_edge_cost（修复前）
pub(crate) async fn peer_edge_cost(&self, peer_id: PeerId) -> Option<i32> {
    best_edge_cost(self.list_peer_conns(peer_id).await)   // ← 只有直连 conn
}
// peer_manager.rs → quality 打分（修复前）
let raw = peers.peer_edge_cost(peer_id).await.unwrap_or(i32::MAX);
```

**触发条件**：某个 `policy_routes` 候选**只能经中转到达**（A 没有到它的直连 conn）。`list_peer_conns` 返回空 ⇒ `best_edge_cost` = `None` ⇒ cost = `i32::MAX`。

**后果**：

1. `select_by = quality` 下该候选**恒排最后** —— 哪怕它比其它候选好得多。
2. `failover_when = degraded` + `quality_degraded_cost` 下，`i32::MAX` **必然超过任何门槛** ⇒ 该候选被 `is_degraded_cost` **直接排除**，即使它是唯一能到目标的出口。
3. ⇒ **"按链路质量"实际是"按第一跳直连链路的质量"**，与 `traffic-steering.md` 原先的描述不符（本轮已同步更正）。

**根因**：策略层没有"到该 peer 的完整路径代价"这个来源。OSPF 表里**有**这个量（`path_latency`，含中转段），但 `Route` trait 当时只暴露了 `get_next_hop` / `get_next_hop_with_policy` / `get_next_hops_with_policy`，**都不返回 `path_latency`**。

**与 B17 同源**：整个系统缺一个"跨层可用的、含中转的路径代价"接口。

**修复（✅ 已落地）**：

1. **`Route` trait 新增 `get_path_cost_with_policy(peer_id, policy) -> Option<i32>`**（`route/mod.rs`），返回**整条路径**的累计代价（`OspfNextHopInfo::path_latency`，含中转段、含 relay-policy 惩罚）。默认实现返回 `None`（不跟踪路径代价的实现不受影响）。
2. `PeerRoute`（`Route` 的实现者）从 **least-cost 表**取该值（`peer_ospf_route.rs`）—— 最便宜的那张表正是"成本为主键"的那张。
3. `PeerMap::peer_path_cost(peer_id)` 遍历 `self.routes` 取该值。
4. `peer_manager.rs` 的 quality 打分改为 **先取路径代价**，**直连边代价作为兜底**（路由表还没知道这个 peer 的那几毫秒窗口）。

**回归用例**：`path_cost_covers_the_whole_path_including_relay_hops` —— A—B 直连 1000ms、A—C 1ms + C—B 1ms 时，到 B 的**路径代价是 2**（不是 1000）。

**与 B17 的关系**：B17 是"**最后一跳**（出口 → 网外目标）的代价"没有传播路径 —— **本条修不了它**。本条修的是"到某个**出口 peer**的路径代价"，那一层现在是对的。

**置信度**：确认（逐行核对 + 用例）。

### B19 ✅ 已修 — `failover_when = never` + `select_by ≠ priority` 静默失效，且配置校验不拦

**位置**（符号名）：`peer_manager.rs` → `reconcile_local_routes_once` 的候选集构造、 `local_route_table.rs` → `validate_policy_route_configs`（当时只做**单条**字段校验，无**跨字段组合**校验）

**触发条件**：配置 `select_by = "quality"`（或 `"balance"`）+ `failover_when = "never"`。

**后果**：候选集只剩 `peers` 的第 1 个 ⇒ `quality` / `balance` **没有任何东西可选** ⇒ 行为等同 `priority`。而配置**能通过校验**（不报错、不告警）⇒ 使用者以为配了质量优选 / 负载均摊，实际完全没有。

**修复（✅ 已落地）**：配置校验加一条**跨字段组合**规则 —— `failover_when = "never"` 且 `select_by ≠ "priority"` ⇒ **加载失败并报错**（与该模块其它组合校验一致，如 `quality_degraded_cost` 必须与 `failover_when = degraded` 成对）。落在 `PolicyRouteConfig::validate`（`local_route_table.rs`），**TOML 与 API 两条入口共用**（都走 `validate_policy_route_configs`）。

**⚠️ 顺带补上的一个洞：热更新路径原本不走这道校验。**

`config_patch` 的 `validate_candidate`（`management/full/config_patch.rs`）只做 `from_toml_with_host` + provider/capability 校验，**不调用** `validate_config_loader_policy_routes` ⇒ 通过 GUI 或 RPC 下发 `failover_when = "never"` + `select_by = "quality"` **能保存成功**，而运行期对账仍只取 `get(..1)` ⇒ **静默退化成 priority**。启动路径和 ValidateConfig RPC 拦得住，热更新路径拦不住。

⇒ `validate_candidate` 现在也调 `validate_config_loader_policy_routes(candidate)`。`TomlConfig` **本身就实现了 `ConfigLoader`**，所以直接传即可，不需要 `dump()` 往返（也就不受 `config-write` feature 影响）。

⚠️ 顺序：`validate_persist_and_commit_candidate` 内部先 `validate_candidate` 再 `persist_candidate_if_changed`，而 `instance.update_exit_nodes(...)` 在它**之后** ⇒ **非法配置不会被提交**，也不会先落盘再报错。

**回归用例**：`a_toml_config_with_an_illegal_policy_route_is_rejected`（`TomlConfig` 形态 —— 正是热更新路径用的那一种 —— 的非法组合同样被拒）。

**回归用例**：`failover_never_with_a_non_priority_select_by_is_rejected` —— `quality`/`balance` + `never` 被拒；`priority` + `never` 与其它组合仍合法。

**置信度**：确认（代码路径确定；触发只需一份配置）。

### B20 ✅ 已修 — `balance` 规则下 wrapped-proxy 旁路校验判错了 peer（原 S1）

> 本条**由存疑项 S1 升级为确认并修复**（2026-10-19 第三轮）。

**位置**：`peer_manager.rs`（`check_allow_wrapped_proxy_to_dst`）—— 修复前用 `self.local_route_table.lookup(v4, None)`，**`flow_key` 传的是 `None`**

**问题**：`select_by = balance` 的规则按**内层流 5-tuple 哈希**挑 peer。`pick_balance` 在 `flow_key = None` 时退化成 **slot 0** ⇒ 旁路校验回答的是**候选列表里第一个** peer 的 feature flag，而这条流实际可能被哈希到**另一个** peer。

**后果**：

- 误判为"允许"而实际 owner 禁止 ⇒ 流量被送到一个禁止旁路的 peer
- 误判为"禁止"而实际 owner 允许 ⇒ 放弃优化（安全，只是浪费）

**修复（✅ 已落地）**：

1. `LocalRouteTable::is_balance_ambiguous(ip)` —— 回答"这个 IP 是否命中一个**有多个候选**的 `balance` 规则"。
2. `check_allow_wrapped_proxy_to_dst` 增加 `flow_key: Option<u64>` 参数：
   - **有** flow_key ⇒ `lookup(v4, flow_key)` **精确**判断；
   - **没有** flow_key 且命中 `balance` 多候选 ⇒ **保守返回 false**（拒绝旁路）。宁可放弃优化，也不能判错对象。
3. `check_allow_kcp_to_dst` / `check_allow_quic_to_dst` 保持原签名（向后兼容），新增 `*_with_flow(dst_ip, flow_key)` 变体。
4. **入站路径**（`gateway/proxy/wrapped_transport/packet_plane.rs`）现在**算并传出** flow key（`zc_packet.payload()` 就是完整的内层 IPv4 包 ⇒ `flow_key_from_payload` 能解析出 5-tuple）。
5. **出站建连路径**（`gateway/dataplane/mod.rs`）：网关 KCP 有 `source_hint` + `dst_addr` ⇒ 用与 payload 哈希**位一致**的 `flow_key_from_ipv4_tuple` 传入；没有 source hint 时仍走保守分支（`None` ⇒ `balance` 多候选拒旁路）。（2026-10-20 复核修补）

**回归用例**：`balance_rule_with_several_candidates_is_ambiguous`（证明没有 flow key 时 `lookup` 会回答 slot 0，而 flow key 确实决定 peer）、`priority_rule_is_never_ambiguous`；`same_five_tuple_sticky_index` 钉死 tuple 助手与 payload 解析同键。

**代价（如实记录）**：无 `source_hint` 的出站建连（例如纯 public dataplane + PreferKcp）在 `balance` 多候选时仍拒旁路。网关路径与入站路径不受影响。

**置信度**：确认（代码路径确定）。

---

## 1g. 复核发现的遗留项（**本轮未修**，供后续处理）

> 2026-10-20 第四轮：对 `exit_nodes` 对账期合并这组改动做了一次**只读边界审查**（逐条核对到代码行）。下面 6 项**确实存在**，本轮**没有修**——多数是设计取舍或影响面超出本轮范围。每条给出触发条件与后果，方便排期。

| # | 遗留项 | 触发条件 | 后果 | 为何本轮不修 |
|---|---|---|---|---|
| **L1** | **balance 规则的分摊范围被 `exit_nodes` 静默扩大** | `select_by = balance` + 默认路由规则 + `exit_nodes` | `resolved_peers`（balance 的分摊列表）用的是**同一条 merged 列表** ⇒ 出口节点会一起参与 `flow_key % N` 均摊。行为符合字段注释，但**配置里看不出来** | 属**产品语义**：要么文档写明，要么让 balance 排除 exit_nodes。需产品拍板 |
| **L2** | **量纲混用** | `select_by = quality` | `peer_path_cost`（**OSPF 公布边权的累加**）与 `peer_edge_cost`（**本地实测 quality 分**）被放进**同一个** `scored` 向量用 `min_by_key` 比较，只有量纲近似一致、**没有归一化** | 两族数值都是"代价单位"（`ospf_edge_cost_from_score` 用的就是同一公式），但来源不同 ⇒ 需要确认这是否**有意为之** |
| **L3** | **`never` 的语义空洞** | `failover_when = never` + 唯一候选不可达 | `never` 的语义是"钉死第一个候选"，但该候选不可达时 `reachable` 为空 ⇒ 规则整体 `Degraded`（`closed` 时直接黑洞），而不是"钉住已失效的 incumbent" | 属 `never` 的**语义定义**问题，要先定"钉死一个死掉的 peer 该怎么办" |
| **L4** | **balance 的 `lookup` 完全忽略 `active_peer`** | `select_by = balance` | `lookup` 只读 `r.resolved_peers`；`ResolvedRule.active_peer` 的注释写着 "informational only"。当前对账**总是成对写入**两者，所以不可达；但这是**脆弱耦合** —— 将来任何让二者分叉的改动都会让 balance 规则静默变成 `Miss`/`Block` | 当前不可达 ⇒ 加注释与断言即可，不必改逻辑 |
| **L5** | **`route_algo = none` 时纯中转可达的出口拿不到分** | `route_algo = none` + 出口只能经中转到达 | `DisabledRoute` 没有实现 `Route::get_path_cost_with_policy` ⇒ `peer_path_cost` 返回 `None` ⇒ 回退到直连边代价（也没有）⇒ `i32::MAX` ⇒ `failover_when = degraded` 的规则会**丢弃该出口** | `route_algo = none` 本来就没有路由表 ⇒ 严格说"不可评估" ⇒ 保守丢弃是安全方向；但要不要让 `none` 模式直接禁用 `quality` 策略，需要产品确认 |
| **L6** | **管理面看不到合并后的有效候选** | 任何 | `policy_route_to_proto` 导出的是**配置层**的 `PolicyRouteConfig.peers`；sugar 规则根本不在配置层 ⇒ **合并后的实际候选列表在 GUI / CLI 上完全不可见**（用户看到"我只配了 A，出口却是 B"） | 需要在状态面/管理面补一个"生效候选"的视图，属**可观测性**工作，不是选路 bug |

**排期建议**（2026-10-20 讨论拍板）：

| 档次 | 项 | 理由 |
|---|---|---|
| **近期** | **L1、L6** | L1 是静默行为扩大（配置上看不见却改了 balance 分摊面）；L6 是可观测性缺口——补上"生效候选"视图后，L1 的产品拍板（文档写明 vs 排除 exit_nodes）也更好做 |
| **中期** | **L3、L5、L4** | L3 / L5 都要先定产品语义再动代码；L4 当前不可达，加注释与断言即可，不必改逻辑 |
| **长期** | **L2** | 两族代价都走同一 `ospf_edge_cost_from_score` 公式，量纲近似同源；短期风险被高估。真要归一化属于算法设计，不是热修 |

**已在本轮一并修掉的**（审查里发现、且改动足够小）：

| 审查发现 | 修法 |
|---|---|
| 候选按 **IP** 解析、不按 `PeerId` 去重 ⇒ 一个 v4 与一个 v4-mapped-v6 可能解析到同一 peer ⇒ 该 peer 在 balance 分摊里占两个槽 | 对账期对 `reachable` 按 `PeerId` 去重 |
| `is_balance_ambiguous` 用**观测态**（`resolved_peers`）回答**配置态**问题（这条规则是不是多候选）⇒ 首轮对账前会漏判 | 空列表也**视为可能歧义**（`len() != 1`）⇒ 保守方向 |
| `never` + `select_by ≠ priority` 的不变量只在 `PolicyRouteConfig::validate`，`ValidatedPolicyRoute` 字段全 `pub` ⇒ 可被绕过 | 同一不变量也放进 `validate_route_rule`（**防御纵深**） |
| `update_exit_nodes` 运行期**无校验** ⇒ 可绕过 `MAX_PEERS_PER_RULE` 与 `closed`+`require_confirmed` 确认（多出口黑洞保护） | `update_exit_nodes` 里按**当前规则**校验，违反则**拒绝更新 + warn**（`config_patch` 路径本已校验 ⇒ 这是第二道防线） |

---

## 2. 存疑项（需端到端复现才能定性）
| 编号 | 位置 | 可疑点 |
|---|---|---|
| **S2** | `peer_manager.rs:1060-1071`、`:3852-3863` | 自出口（`exit_nodes` 指向本机）会被自投递保护挡掉 ⇒ 包既不出网也不进 TUN ⇒ 落到 `unhandled packet` 静默丢弃。但 `peer_manager_tests.rs:889-907` 明确把这个语义钉死了，**可能是有意为之** |
| **S3** | `route/graph_algo.rs:209-263`、`:152-174` | `tight_edge_parents` 按 `DashMap` 派生的边序收集父节点，`full` 集合按 `max+1` 截断 ⇒ 某目标有 **> 2 个**等代价首跳时，保留哪两个会随边序变化 ⇒ 备用跳成员抖动（`sort_unstable` 只稳顺序不稳成员） |
| **S4** | `peer_ospf_route.rs:562-596` | `gen_next_hop_map_with_least_hop` 用**原图节点下标**建子图边。当前成立仅因 petgraph `Graph` 按 `0..n` 连续编号且子图按同序 `add_node` ⇒ **隐式耦合**，无断言保护 |
| **S5** | `peer_conn_ping.rs:418-419` vs `:510-518` | fast probe 降级位被"保活成功"清除、又被 fast probe 自己重新置起 ⇒ `loss_rate` 在 0 与 1.0 之间摆动 ⇒ 边权 ±10000 抖动。详见 §1c |

---

## 3. 明确排除的怀疑方向（不是 bug）

| 怀疑方向 | 结论 |
|---|---|
| LPM 匹配 `<` vs `<=`、等长 tie-break | **正确**。`local_route_table.rs:414-432` 用 `partition_point(network <= addr)` 二分，桶内同前缀只有一个可能命中的块；`:547-561` 的排序键 `(len, network, strict, idx)` 使"更长前缀 → closed → 更小索引"**完全确定** |
| 默认路由 `/0` 与更具体路由的优先级 | **正确**。`/32` 走 `by_ip` 恒胜；非 `/32` 按前缀长度从长到短扫桶 |
| IPv4 / IPv6 两表混用 | 无混用。策略表只吃 `Ipv4Addr`（IPv6 首期不支持是**已拍板**设计） |
| 观测 `/32` 与配置 CIDR 的优先级 | 观测 `/32` 进 `by_ip` 恒胜 LPM，符合"域名 IP 更具体"的语义；FIFO 淘汰会保护显式 `/32`，单测已覆盖 |
| 路由所有权（多节点各配 policy route） | 策略表是**纯本地**的，不存在跨节点冲突 |
| `disable_relay_data` 拒绝中转后丢包还是绕回 | **有意为之**：数据面永不硬丢，只做 OSPF 避让。文档亦已写明「**仅 OSPF 避让，不硬丢包**」（`peer_manager.rs` 收包处注释："Packets that still arrive for forwarding ... are forwarded, never hard-dropped"）。**本轮独立复核确认该结论成立**；尖锐之处在于配置名 `disable_relay_data` 承诺的比实现多 —— 声明「不中转」的节点**仍会真的转发过境数据**，对计费/合规场景给不出「无过境流量」的保证，属命名与口径问题而非代码缺陷 |
| 链路质量优先是否只看相邻跳 | **不成立**：`conn_select` 只算相邻 `PeerConn`，这是设计。**且「直连 vs 中转」的取舍已有固化契约**（字典序 `成本 → 跳数 → 稳定身份键`），绕行路径总代价更低时选中继是**规定行为**，不是缺陷 —— 见 B14（已撤回） |
| 「直连可用却走中转」是否是缺陷 | **不成立（B14 撤回）**。契约见 `live-next-hop.md:38`（跳数是第二键）、`:443`/`:484`（「13ms 直连 vs 两跳各 <0.5ms ⇒ **应选中继**」「防止有人用『直连无条件优先』来『修』本问题」）；守卫测试 `graph_algo.rs:377` `strictly_cheaper_multi_hop_path_still_wins`（注释原文："the guard against 'prefer direct' being over-applied"） |
| `is_exit_node` 标志在多跳转发时被错误保留/清除 | **正确**。转发时只改 `latency_first`、只 `forward_counter += 1`，标志位原样带过 |
| `forward_counter` 上限 / A→B→A 环路 | 上限 8 跳，是截断不是防环 —— `live-next-hop.md §4 P1` 已记录的既有设计 |
| `forward_counter > 2` 清 `latency_first` | **已记录**（`live-next-hop.md:141`，行号已漂移）。本轮**量化补全**了生效跳数并补上 GUI 显示口径的连带影响 ⇒ 见 B15 |
| 策略表更新与发包并发 | `ArcSwap` 快照读 + `rcu` 写，无撕裂读 |
| peer 掉线后策略表选到死 peer | `resolve_exit_node_peers` → `get_gateway_peer_id` → `select_next_hop` 对直连死 peer 会正确返回 `None` |
| `0.0.0.0/0` 兜底与 `exit_nodes` 顺序 | 顺序正确：policy(3.5) → exit_nodes(4) → 对端 `/0` 兜底(5) |
| `MIN_EDGE_COST = 1` 下限 | **有意为之**。避免零权边把 `first_hop` 记账退化成任意 tie-break |
| 两张路由表并存（LeastHop / LeastCost） | **有意为之**。每张表内部自洽，转发时按策略整表切换，不会互相回指成环 |
| 等代价备用跳只在"完全等代价"时提供 | **有意为之**（局部无环证明，更便宜/容差内的备用跳需 RFC 8402 LFA，明确不做） |
| 子网/地址映射每轮从零重建 | **有意为之**（为修"改 VIP 后旧地址永久可解析"） |
| `AVOID_RELAY_COST` 是"软否决"而非硬拒绝 | **有意为之**。注释明确写 "you never route through a refusing peer **when an alternative exists**"。代价：`disable_relay_data` 在拓扑只有一条路时形同虚设 —— 属语义/产品决策 |
| 滞回方向错、代价下降被死区卡死导致永不切到更好的路 | **不成立**。`apply_ospf_cost_hysteresis` 的比较对象始终是"已发布值"，raw 会收敛；`pick_default_conn` 的 `required_delta` 是"必须优于多少"的门槛而非零附近死区；incumbent 不可用时立即切；`within_switch_band` 只在 `current > best` 时生效，best 变好不会被卡（**缺的是相对项，见 B12，不是方向错**） |
| `clamp(1, 1e6)` 把 >1000ms 的高延迟截断成同一个值 | **不成立**。上界 1e6 代价 = 1000 秒 RTT；1s RTT → cost 1000，10s RTT → cost 10000，均不触顶。真正有问题的是**下界 1**（见 B10） |
| 量纲链不自洽（`latency_us/1e6`→秒→`×1000`→毫秒当量） | **自洽**。`score = rtt_s + 0.4·loss + jitter_s`，`cost = score×1000 = rtt_ms + 400·loss + jitter_ms`，与 `unified-routing-design.md` 的权重表一致。⚠️ `w_loss` 默认值在 2026-10-20 由 `4.0` 降为 `0.4`（B9）—— 系数 4000 → 400 |
| `seq` 是 u16 会回绕 | **不成立**。是 u32，且保活/快探针已用最高位分隔 seq 空间（`peer_conn_ping.rs:76-87`） |
| 15 样本窗口在样本变少/conn 重建时会给出离谱值 | **不成立**。`count` 只增不减、`sum` 是环内运行和；conn 重建时 `latency_stats`/`loss_rate_stats` 在 `PeerConn::new` 里新建，不复用 |
| 全部备选都 fused 时会抛异常或误选未验证 conn | **不成立**。`has_unfused` 为假时 fused conn 被保留；`cached_path_unusable` 的 `has_unfused_alternative` 为假时不失效缓存 —— 确实降级使用 |
| `pick_default_conn` 与 `best_edge_cost` 口径不一致导致"数据面走 A、路由表说 B" | **除 B10 / B11 外未发现实质不一致**。`best_edge_cost` 取 `min(cost)` 与 `pick_default_conn` 取 `min(score)` 等价（fuse 加成 10000 远大于任何合理代价）；`default_conn` 的额外约束只在"全部备选 fused"或"只有未验证打洞"时与 `best_edge_cost` 分歧 |
| 未验证 hole-punch 的排除逻辑在两处不一致 | **hole-punch 情形已被记录**：`docs/current/tailscale-gap-and-conflict-audit.md` §C8 明确"已复核，暂不改"（代价落到 500 兜底）。**但非 hole-punch（直连/中继）的"未测量"conn 逃逸出该保护**（见 B10） |
| `switch_windows` 迟滞不进真实数据面 | **是事实，但已文档化**。`peer_link_bond_count` 出厂默认 2 ⇒ `Peer::send_msg` 走 bond 集，`pick_default_conn` 只在 bond 集为空时兜底；`docs/current/peer-connections.md` §2 写明"单路径 = `bond_count ≤ 1`"。**属设计缺口而非新 bug**（但 bond 成员资格只有 10% 质量档、无 streak，因此 B9 的 +40 会直接踢人） |
| `quality_score` 经 f32 往返（`peer.rs:548`）导致与 f64 直算差 1 | 存在但 ≤1 个代价单位，不构成缺陷，仅记录 |

---

## 4. 与已有记录的对照

| 已有编号 | 内容 | 现状 |
|---|---|---|
| CR-08 | 对端可控 `version` 永久冻结他人路由 | 已用 `version_advance_is_plausible` 修复；**但叠加 B2 的"重启"场景后，冻结变成长期冻结** |
| CR-09 | 负代价 `i32 as usize` 下溢 | 已做负数/零值下钳；**但上钳缺失 ⇒ B8** |
| 5-F7 | `ipv4_peer_id_map` 只增不删 | 已修（每轮重建 + `ArcSwap`），无新问题 |
| **5-F8** | 同 VIP / 等距归属不确定 | **声称已修但修错分支 ⇒ B1**。**本轮已修**（归属改成成本感知字典序，peer_id 决胜回到它该在的分支） |
| 5-F6 | `build_route_info` / `build_conn_peer_list` 的 `break` | 已改为 `continue`，无新问题 |
| TD-1/4/5/7、unified-routing-*、live-next-hop | 热路径重构 / 统一路由模型 / 备用下一跳 | **B6** 与 `live-next-hop.md` 的备用跳语义有交叉但未被那几份文档记录；**B14 撤回**（`live-next-hop.md:38/443/484` 已有契约）；**B15 已记录**（`live-next-hop.md:141`，仅行号漂移） |
| tailscale-gap-and-conflict-audit §C8 | 未验证打洞 conn 的代价兜底 | **已复核，暂不改**。本文件不重复报告；**B10** 指出的是该保护在**非 hole-punch conn** 上的缺口 |
| peer-connections.md:77 | "单次抖动不会污染判断" | 该结论对 RTT/jitter 成立，**对 loss 不成立**（见 B9） |
| CR-10 | 畸形 Pong 打瘫链路 liveness | 已修；与本文件具体实现无冲突项 |

---

## 5. 修复顺序与本轮落地情况

### 5.1 本轮已完成（2026-10-19）

| 序 | 条目 | 改动要点 | 回归用例 |
|---|---|---|---|
| 1 | **B1** | 归属仲裁改成与下一跳表同一套字典序：`path_latency` → `path_len` → `peer_id` | `exit_ownership_follows_the_edge_cost`、`an_expensive_direct_peer_does_not_keep_the_default_route` |
| 2 | **B11** | 滞回基线从进程级 `static` 改为 `PeerMap` 实例级；`retain` 只裁剪本实例 | （间接，由 B9 用例覆盖发布频率） |
| 3 | **B9** | `w_loss` 默认 4.0 → 0.4（`conn_select.rs` + `config/toml.rs` 两处同步） | `a_single_lost_probe_stays_inside_the_publish_deadband`、`a_lossy_link_still_costs_more_than_a_slow_clean_one` |
| 4 | **B5** | `peer_infos` 写入加 version 守卫；`next_hop_alt_map` 改 `remove_if` | （并发路径，暂无直接用例） |
| 5 | **B6** | `get_next_hops` 的备用跳同样过滤 `suppressed_peer_ids` | （抑制路径，暂无直接用例） |
| 6 | **B8** | `edge_cost_from_i32` 按 `OSPF_EDGE_COST_MAX` 上钳 | （边界，暂无直接用例） |
| 7 | **B13** | 丢包率改四舍五入 + `clamp` | （数值，暂无直接用例） |
| 8 | **B10**（部分） | 未测量 conn 的发布代价 1 → `OSPF_EDGE_COST_UNMEASURED`(500) | （间接，由 B9/B1 用例覆盖） |
| 9 | **B18** | `quality` 打分改用 **OSPF 路径代价**（`Route::get_path_cost_with_policy`，含中转段） | `path_cost_covers_the_whole_path_including_relay_hops` |
| 10 | **B19** | 配置校验拦 `failover_when=never` + `select_by≠priority` | `failover_never_with_a_non_priority_select_by_is_rejected` |
| 11 | **B20** | wrapped-proxy 旁路校验带 flow key；无 flow key 且 balance 多候选 ⇒ 保守拒绝 | `balance_rule_with_several_candidates_is_ambiguous`、`priority_rule_is_never_ambiguous` |
| 12 | **B2**（部分） | 发送方重启时"被判为冒用"的那条从**中止整批**降级为**跳过该条**；真·冒用仍中止整批 | `a_restarting_sender_only_skips_its_own_stale_record`、`a_peer_advertising_our_own_id_still_aborts_the_batch` |
| 13 | **B7** | `exit_nodes` 的候选**并入**显式 `/0` 规则（显式在前），不再作为会被索引丢弃的第二条规则 | `exit_nodes_merge_into_an_explicit_default_route_rule` 等 4 条 |
| — | **B14** | **不做** —— 建议的修法会打破守卫测试 `graph_algo.rs:377` 与 `live-next-hop.md:443/484` 的验收契约；§1e.2 已用实测再次确认网内选路正确 | — |

**验证**：`cargo test -p easytier-core --lib` → **1072 passed / 0 failed / 1 ignored**；`cargo check -p easytier-core --all-targets` 通过；`cargo fmt --all -- --check` 通过。

#### ⚠️ 行为变更（升级即生效，仓库无 CHANGELOG ⇒ 记在这里）

本轮修复里有几处**改的是默认行为**，升级后不需要改配置就会变。逐条列清楚：

| # | 变更 | 影响面 | 缓解 |
|---|---|---|---|
| 1 | `conn_select_w_loss` 默认 **400 → 40** | **全网边权尺度变了**：丢包项在代价里的占比降 10 倍。已发布的边权与新算的边权不同量纲，混跑时 OSPF 会看到一次"重标定" | 显式配了 `conn_select_w_loss = 400` 的用户行为不变；其它用户等价于接受一次边权重标定（**滚动升级期间尤其明显**） |
| 2 | `failover_when = "never"` + `select_by ≠ "priority"` ⇒ **启动直接失败** | 原本"静默按 priority 跑"，现在**加载失败** ⇒ 该实例起不来 | 需要把 `failover_when` 改成 `unreachable` / `degraded`（或把 `select_by` 改回 `priority`） |
| 3 | `quality_degraded_cost` 的语义从"**直连边**代价"变成"**整条路径**代价" | 按老口径标定的阈值会**提前触发 degraded**（路径代价含中转段，通常更大） | 该阈值必须**重新标定**；`select_by = quality` 的排序口径同样变了（现在是整条路径） |
| 4 | `exit_nodes` 从"配了显式 `/0` 规则时**不生效**"变成"**生效**" | 原本被丢弃的出口节点现在进入候选 ⇒ 出口选择可能改变 | 这是修 bug 的目标；但依赖"旧的不生效"的部署会看到出口变化 |
| 5 | 显式 `/0` 规则**同时带 `domains`** + `exit_nodes` ⇒ **加载失败** | 原本能加载的配置现在报错 | 拆成两条规则（一条纯 `/0`、一条纯 `domains`） |
| 6 | 显式 `/0` 规则 `failover_when = "never"` + `exit_nodes` ⇒ **加载失败** | 原本能加载的配置现在报错 | 改成 `failover_when = "unreachable"` |
| 7 | `exit_nodes` 与显式 `/0` 规则合并后候选数 > 16 ⇒ **加载失败** | 原本能加载的配置现在报错 | 减少候选或拆分规则 |
| 8 | **显式 `/0` 规则 + `failover_when = "never"` ⇒ 现在总是加载失败**（即使当前没有 `exit_nodes`） | 原本合法的配置现在报错 | 改成 `failover_when = "unreachable"`。**原因**：出口节点可能稍后被热补丁加上，那时合并会静默失效（`never` 只保留第一个候选） |
| 9 | 运行期 `exit_nodes` 变更若违反候选数上限 / `closed`+多 peer 未确认 ⇒ **被拒绝**（记 warn，不更新） | 原本静默生效的非法变更现在不生效 | 这是第二道防线（`config_patch` 路径本来就拦）；库嵌入方直接调 `update_exit_nodes` 时会收到 warn 日志 |

**升级建议**：先在测试环境跑一遍 `easytier-cli` 的配置校验（加载即校验），把 2 / 5 / 6 / 7 这几类"启动失败"提前暴露出来；1 / 3 / 4 属于运行期口径变化，需要观察一轮 SPF 收敛。

### 5.2 待办（按建议顺序）

| 序 | 条目 | 理由 |
|---|---|---|
| 1 | **B17** | 结构性缺口，先把"出口是按配置优先级选的、不是按到目标的距离选的"在文档/UI 上说清楚；协议扩展（让出口上报"到网外目标的代价"）需设计评审。**B18 只修好了"到出口 peer 的路径代价"这一层，修不了 B17** |
| 2 | **B3** | 同出口可用性 + 安全面。**修法需先设计**（改写 identifier / 源地址 NAT，见 §1e 与 B3 条目），不能只加 key 字段 |
| 3 | **B2**（剩余） | 完整修复 = 识别"这是**同一节点的重启**"⇒ 清掉本地旧记录、接受新的 route id + version（强制重学习）。**判据必须与会话绑定** —— `RoutePeerInfo` 里的 `noise_static_pubkey` / `inst_id` 都是**自声明**的（明文广播），冒用者能抄；可信来源是 peer_rpc 会话层的远端 Noise 静态公钥，而 `SyncRouteSession` 没有这个字段 ⇒ 跨层改动 |
| 4 | **B4** | 涉及语义变更，需先与 `unified-routing-design.md §4.2` 的"网关模式"口径对齐，再同步 `traffic-steering.md:54/156/222` |
| 5 | **B10**（剩余） | conn 选择侧的"未测量"建模要与 hole-punch 的 `unverified_hole_punch`（**protobuf 字段**）解耦，语义改名要动协议 |
| 7 | **B12** | **先定"发布端死区"的契约**再改：死区是"防抖"（宜大），`within_switch_band` 是"切换"（宜小），两者本不该同公式（见 B12 条目） |
| 8 | **B15 / B16** | 需先定「latency_first 是端到端意图还是逐跳策略」这个语义，再改；改前建议先在状态面暴露"实际用了哪张表" |
| — | S2–S5 | 先补端到端复现再决定 |

**交互提示 1**：B9 与 B11 有放大关系 —— B11 让发布防抖失效后，B9 的 +40 台阶每次都原样上报，SPF 被反复触发。因此 **B11 应在 B9 之前或同时修**。**本轮两者都已修**（B11 先、B9 后）。

**交互提示 2**：B11 还会**间接放大「深路径换路」**。普通内部转发**逐包**重新决策（`send_msg_internal`），没有 per-flow 钉住；抖动本应由**边级**迟滞 `apply_ospf_cost_hysteresis` 负责（`live-next-hop.md:451` 明确「切换阈值」方案已撤回、抖动交由边级迟滞负责），而 B11 让这条边级迟滞在"配了外网"时失效。⇒ **修 B11 同时修掉了 B14 里唯一有残余价值的那个观察**（无路径粘性的后果面）。

**交互提示 3（本轮新增）**：**B1 与 B17 是两件事，不要合并修**。B1 修好之后，CASE 2.1 / 2.2 的出口归属会变成"代价低者胜"（正确），但 **CASE 2.3 仍然错** —— 最后一跳的代价在协议里没有传播路径（B17）。⇒ 修完 B1 **不能**对外宣称"出口选择已经按代价最优"，只能宣称"在能看到的代价（第一跳）上最优"。

---

## 6. 复核方法

本文结论为**静态审查 + 针对性实测**产出。凡标「确认」者，均已逐行核对到具体逻辑行；**可复现的行为差异一律写成回归用例跑过**（本轮新增 26 条）。复核入口一律用**符号名**给出（绝对行号会随改动漂移，只在必要时附注）：

| 条目 | 复核点（符号名） |
|---|---|
| B1 | `peer_ospf_route.rs` → `is_new_peer_better`（本轮改为成本感知字典序）；修复前的判据见 §1 的「原判据」代码块 |
| B2 | `peer_ospf_route.rs` → `check_duplicate_peer_id`（本轮改为 `Result<bool, Error>`）、`update_peer_infos`（本轮在 `Ok(false)` 时跳过该条而非中止整批）、`clear_expired_peer`（61 分钟 / 90 秒清理窗口）、`PeerRouteServiceImpl::new`（`my_peer_route_id: rand::random()`）、`update_my_peer_info`（`new_version = old.version + 1`，新进程从 1 开始） |
| B3 | `icmp_proxy_engine.rs` → `IcmpNatKey` vs `udp_proxy_engine.rs` → `UdpNatKey` vs `tcp_proxy_engine.rs` 的 key；**回包拿不到源**：`icmp_proxy_service.rs` → 收包循环（socket 只给源地址）+ `handle_peer_packet`（所有 peer 共用同一 socket） |
| B4 | `icmp_proxy_engine.rs` → `handle_peer_packet` 的总闸与逐包判断；对照 `docs/current/traffic-steering.md` |
| B5 | `peer_ospf_route.rs` → `build_from_snapshot_with_unusable` 里的 `peer_infos` 写入（本轮加了 inst_id + version 两段守卫）、`gen_next_hop_map_with_least_cost` 里的 `next_hop_alt_map.remove_if`（本轮由无条件 `remove` 改来）；对照 `update_my_peer_info`（peer 自己递增 version） |
| B6 | `peer_ospf_route.rs` → `get_next_hop`（查抑制）vs `get_next_hops`（本轮已在备用跳处补上同样的过滤） |
| B7 | `local_route_table.rs` → `build_rules_from_config`（本轮改成**对账期**合并：`RouteRule::merge_exit_nodes` + `merged_candidates`）、`rebuild_index`（同前缀去重 —— 本轮之后同前缀只剩一条规则，无需再改）；对照 `docs/roadmap/unified-routing-design.md` §5.3「合并候选列表，显式在前」 |
| B8 | `peer_ospf_route.rs` → `edge_cost_from_i32`（本轮加了 `OSPF_EDGE_COST_MAX` 上钳）；惩罚段在 `build_peer_graph_from_snapshot` 里单独相加；`conn_select.rs` → `OSPF_EDGE_COST_MAX` |
| B9 | `peer_conn_ping.rs` → `pingpong`（窗口 100）、`record_latency(0/1)`、`loss_rate_stats.store`（本轮由 `as u32` 截断改为四舍五入）；`conn_select.rs` → `ConnSelectConfig::default`（`w_loss` 本轮由 4.0 改来）、`conn_quality_score`、`ospf_edge_cost_from_score` |
| B10 | `conn_select.rs` → `score_conn`（`unverified_hole_punch` 判据）、`OSPF_EDGE_COST_UNMEASURED`（本轮新增）；`peer_map.rs` → `best_edge_cost`（本轮加了未测量分支）；`peer_manager.rs` → `set_is_hole_punched` 调用点 |
| B11 | `peer_map.rs` → `PeerMap::ospf_edge_cost_baseline`（本轮由 static 改来）、`ospf_edge_cost_with_hysteresis`、`retain_ospf_edge_cost_baseline`、`direct_peer_info`（改为按 peer map 逐个处理）；两个调用方 `peer_manager.rs` → `list_peers` 与 `foreign_network/mod.rs` → `list_peers` |
| B12 | `conn_select.rs` → `apply_ospf_cost_hysteresis`；对照 `local_route_table.rs` → `within_switch_band` |
| B13 | `peer_conn_ping.rs` → `loss_rate_stats.store`（本轮由 `as u32` 截断改为四舍五入 + `clamp`） |
| B14 | **已撤回** —— 反证：`graph_algo.rs` → `strictly_cheaper_multi_hop_path_still_wins`（注释点名防 "prefer direct" 被过度应用）+ `live-next-hop.md` §1.2 / §5.2 / §5.4 / §7 / §12 |
| B15 | `peer_manager.rs` → 转发收包分支（**全仓唯一** `set_latency_first(false)`）；对照同处的 8 跳截断、`live-next-hop.md` |
| B16 | 盖位方 `peer_manager.rs` → `hdr.set_latency_first(packet_policy.latency_first)`（来源 `context.packet_policy()`）；读位方 `send_msg_internal` → `msg.peer_manager_header().unwrap().is_latency_first()` |
| B17 | `peer_manager.rs` → `peer_path_cost` / `peer_edge_cost`（整条路径 vs 直连边）、quality 打分处；`local_route_table.rs` → `is_degraded_cost`、`pick_balance`（balance）；`peer_map.rs` → `resolve_exit_node_peers`（列表顺序）；`peer_ospf_route.rs` → `get_peer_id_for_default_route_proxy`（归属来自 `cidr_peer_id_map`） |
| B18 | `route/mod.rs` → `Route::get_path_cost_with_policy`（本轮新增）、`peer_ospf_route.rs` → 同名实现（从 least-cost 表取 `path_latency`）、`peer_map.rs` → `peer_path_cost`、`peer_manager.rs` → quality 打分（直连边代价作兜底） |
| B19 | `local_route_table.rs` → `PolicyRouteConfig::validate`（本轮加了跨字段组合校验）、`peer_manager.rs` → `reconcile_local_routes_once` 的候选集构造（`.get(..1)`，是本条要拦的组合之所以无效的原因） |
| B20 | `peer_manager.rs` → `check_allow_wrapped_proxy_to_dst`（本轮加了 `flow_key` 参数与 balance 歧义保守分支）、`local_route_table.rs` → `is_balance_ambiguous`（本轮新增）；`gateway/proxy/wrapped_transport/packet_plane.rs`（入站路径传出 flow key；无 balance 规则时跳过哈希）、`gateway/dataplane/mod.rs`（建连路径用 socket 端点算 flow key） |

**本轮（第三轮：实施修复 + 实测复核）基点**：`git HEAD = 51ef4424`；工作区含 5 个文档（`docs/README.md`、本文件、`docs/current/latency-measurement-and-display-audit.md`、`docs/current/traffic-steering.md`、`docs/roadmap/unified-routing-design.md`）与 **16 个源文件**的改动（13 个 `.rs`：`peers/route/peer_ospf_route.rs`、`peers/route/peer_ospf_route_tests.rs`、`peers/route/local_route_table.rs`、`peers/route/mod.rs`、`peers/conn/peer_map.rs`、`peers/conn/conn_select.rs`、`peers/conn/peer_conn_ping.rs`、`peers/conn/conn_bond.rs`、`peers/peer_manager.rs`、`peers/bench_support.rs`、`gateway/proxy/wrapped_transport/packet_plane.rs`、`gateway/dataplane/mod.rs`、`config/toml.rs`；1 个 `.proto`；2 个 locale）。

**验证**：`cargo test -p easytier-core --lib` → **1072 passed / 0 failed / 1 ignored**；`cargo check -p easytier-core --all-targets` 通过；`cargo fmt --all -- --check` 通过。

**本轮新增的回归用例**（全部可单独跑）：

| 用例 | 位置 | 钉住什么 |
|---|---|---|
| `direct_edge_wins_when_the_relay_is_more_expensive` | `peer_ospf_route_tests.rs` | CASE 1.1：直连 100ms 胜过中转 150ms |
| `cheaper_multi_hop_relay_wins_over_the_direct_edge` | 同上 | CASE 1.2：中转 70ms 胜出直连 100ms（`live-next-hop.md:443` 契约的可执行版） |
| `exit_ownership_follows_the_edge_cost` | 同上 | B1：出口归属跟随代价（修复前**恒定不变**） |
| `an_expensive_direct_peer_does_not_keep_the_default_route` | 同上 | B1：1000ms 直连不得保住 2ms 两跳的默认路由 |
| `a_single_lost_probe_stays_inside_the_publish_deadband` | `conn_select.rs` | B9：丢 1 个包的代价增量 < 发布死区 |
| `a_lossy_link_still_costs_more_than_a_slow_clean_one` | 同上 | B9：降权后仍要区分"丢包"与"慢" |
| `a_restarted_peer_refreshes_its_record` | `peer_ospf_route_tests.rs` | B5：peer 重启（`inst_id` 变、version 回退）后记录必须被刷新，不能被守卫误判为"更旧"而永久保留 |
| `path_cost_covers_the_whole_path_including_relay_hops` | 同上 | B18：路径代价必须是**整条路径**的（经 C 中转到 B 是 2ms，不是直连的 1000ms） |
| `failover_never_with_a_non_priority_select_by_is_rejected` | `local_route_table.rs` | B19：`never` + `quality`/`balance` 必须被配置校验拒绝 |
| `balance_rule_with_several_candidates_is_ambiguous` | 同上 | B20：没有 flow key 时 `lookup` 回答 slot 0，而 flow key 确实决定 peer ⇒ 必须能报出"歧义" |
| `priority_rule_is_never_ambiguous` | 同上 | B20：`priority` 规则由对账循环决定，与 flow key 无关，永不歧义 |
| `a_restarting_sender_only_skips_its_own_stale_record` | `peer_ospf_route_tests.rs` | B2：发送方重启后，同批的**第三方** peer 必须落库；发送方自己那条保持旧值 |
| `a_peer_advertising_our_own_id_still_aborts_the_batch` | 同上 | B2：**真·冒用仍中止整批**且不部分应用（安全边界没被削弱） |
| `exit_nodes_merge_into_an_explicit_default_route_rule` | `local_route_table.rs` | B7：`exit_nodes` 必须**并入**显式 `/0` 规则的候选（显式在前），不能变成会被索引丢弃的第二条规则 |
| `the_merged_default_route_rule_is_reachable_and_agrees_with_l2` | 同上 | B7：合并后的规则能被查表命中，且 L2 贡献集与 L3 一致 |
| `exit_nodes_without_an_explicit_default_route_keep_their_own_rule` | 同上 | B7：没有显式 `/0` 时 sugar 规则照旧（零回归） |
| `merging_more_candidates_than_the_limit_is_rejected` | 同上 | B7：合并后超限 ⇒ 硬失败，不静默截断 |
| `merging_exit_nodes_into_a_closed_rule_requires_confirmation` | 同上 | B7：合并导致 closed+多 peer 未确认 ⇒ 硬失败 |
| `the_merged_candidates_follow_the_live_exit_nodes` | 同上 | B7：合并必须跟随**活**的 `exit_nodes`（热更新语义） |
| `the_exit_nodes_sugar_rule_follows_the_live_exit_nodes` | 同上 | B7：**sugar** `/0` 规则的候选也来自活 `exit_nodes`（它的 `peers` 为空）⇒ 它的热更新同样不被冻结 |
| `a_rule_without_the_merge_flag_keeps_its_own_candidates` | 同上 | B7：没有标记的规则永远看不到 `exit_nodes` |
| `a_default_route_rule_with_domains_cannot_be_combined_with_exit_nodes` | 同上 | B7：带 `domains` 的默认路由规则 + `exit_nodes` ⇒ 硬失败 |
| `merging_exit_nodes_into_a_never_rule_is_rejected` | 同上 | B7：`failover_when = "never"` 的默认路由规则不能合并 `exit_nodes` |
| `a_confirmed_closed_rule_still_loads_with_the_exit_nodes_merged_in` | 同上 | B7：确认后的 closed 规则仍能加载 |
| `the_default_route_takes_exit_nodes_even_when_there_are_none_yet` | 同上 | B7：`merge_exit_nodes` 标志必须**无条件**置位，否则"启动时无出口、之后热补丁加上"的出口永远不可见 |
| `a_never_default_route_is_rejected_even_without_exit_nodes` | 同上 | B7：`never` + 默认路由规则**总是**被拒（出口可能稍后被加上） |
| `a_toml_config_with_an_illegal_policy_route_is_rejected` | 同上 | B19：`TomlConfig` 形态（**热更新路径用的就是它**）的非法组合同样被拒 |

**未做**：端到端复现、真机压测、抓包取证。B15/B16、B2 剩余部分、B4、B17 仍为**纯静态代码核对**。除 B14（撤回，依据仓库内已有测试与文档，不依赖本次跑测）外，本文档的"确认"结论要么有上述用例支撑，要么有逐行核对支撑 —— **两者都未做到端到端复现**。
