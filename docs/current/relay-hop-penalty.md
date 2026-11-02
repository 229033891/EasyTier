# relay_hop_penalty（中继跳惩罚）变更说明

**Status: Current** ｜ 最近审阅：2026-11-02 ｜ 关联：`docs/current/routing-bug-audit-2026-10.md` §1d（B14 撤回）、`docs/roadmap/live-next-hop.md:443/484`（选路契约与负例）

---

## 0. 与既有选路契约的关系（先读这段）

`routing-bug-audit-2026-10.md` §1d 已把「直连可用却走中转」**撤回为误判**：成本为主键、跳数只破平局是**有守卫测试钉死的契约**（`graph_algo.rs:373` `strictly_cheaper_multi_hop_path_still_wins`），且有明文负例（`live-next-hop.md:443`：13ms 直连 vs 两跳各 <0.5ms ⇒ **应选中继**）。

**本变更不推翻该契约，但默认值不是 0**：出厂默认 `relay_hop_penalty = 5`（2026-11-02 定为默认，此前为 0）。这个取值经过与契约负例的标定——`live-next-hop.md:443` 的验收拓扑（13ms 直连 vs 两跳各 <0.5ms，路径代价 1+3=4）在默认惩罚下为 4+5=9 < 13，**中继仍然胜出，负例不翻转**；被翻转的只是「两跳和 7 vs 直连 9」这类毫秒级 marginal 胜出。要回到契约的原始严格形态（惩罚完全不参与），显式设 `relay_hop_penalty = 0`。它做的本质是把「中间节点不是免费管道」这一真实开销计入选路模型——计多少，部署者仍可通过该值调整。

**与负例的边界**：对 `live-next-hop.md:443` 的验收拓扑（13 vs 1+3=4），惩罚 ≥13 会翻转该负例。因此：

- 推荐值 **3-5**（覆盖毫秒级 marginal 胜出，离 13 尚远）；
- 惩罚 ≥13 的部署等于宣告「本网络放弃该负例所代表的契约场景」，这是配置决定而非默认行为——文档不鼓励，代码不禁止；
- 硬上界 `RELAY_HOP_PENALTY_MAX`（见 §3.1）防的是**语义破坏**（撞车策略否决带），不是替运维做质量判断。

---

## 1. 修改了什么

新增一个可选的路由配置项 **`relay_hop_penalty`**（中继跳惩罚）：

- 字段位置：`FlagsInConfig`（proto 字段号 59）、`NetworkConfig`（api_manage 字段号 98）；TOML 配置 `[flags]` 段或管理 API 均可设
- 类型：`uint32`；**单位 = OSPF 边代价单位（≈1ms RTT 当量，定义见 `common.proto` 字段 59 注释，单一出处）**
- 默认值：`5`（2026-11-02 起出厂默认；设 `0` 恢复历史无惩罚行为，见 §0 与负例的标定）
- 作用：OSPF LeastCost（`latency_first=true`，质量优先选路）计算路径代价时，**每经过一个中间转发节点，路径总代价额外加一次该惩罚值**
- 硬上界：超过 `RELAY_HOP_PENALTY_MAX`（= `OSPF_EDGE_COST_MAX / 8` = 125,000）的值由两层处理——**管理 API 边界直接拒绝并返回明确错误**（与 MTU 的 CR-13 风格一致，不做静默截断）；**TOML 路径不做启动校验**（与其余 `[flags]` 字段一致），越界值在 OSPF 读取处被 clamp 到上界（旧管理端直接推送原始 `FlagsInConfig` 也走这条兜底路径），warn 日志**按越界值去重**，同一错误值只报一次（见 §3.1）。控制台/`new_from_config` 回显的是**生效值**（已 clamp），因此越界 TOML 不会等到「保存配置」时才报错

TOML 用法（**全网节点必须统一**，见 §3.3；默认已是 5，此处示例为显式声明）：

```toml
[flags]
relay_hop_penalty = 5
```

**生效时机**：OSPF 主循环监听 runtime config 变更，收到变更后**带变更检测**——只有惩罚值相对上次生效值变化才 `version.inc()` + 重建路由表（runtime 变更通知同时被 DHCP 租约 / VPN portal / ACL 组刷新等无关路径触发，无条件重建会让对端白拉一次 delta）。配置改动无需重启进程，也不必等下一次路由同步。该分支的触发源与跳过逻辑为代码审查 + 人工验证（store 级通知单测未覆盖）。

### 代码改动清单

| 文件 | 改动 |
|---|---|
| `easytier-proto/proto/common.proto` | `FlagsInConfig` 新增 `uint32 relay_hop_penalty = 59;`（单位单一出处 + 上界说明） |
| `easytier-proto/proto/api_manage.proto` | `NetworkConfig` 新增 `optional uint32 relay_hop_penalty = 98;`——**必须**，否则 Full 全量下发会把 TOML 值静默清零（见 §3.2） |
| `easytier-core/src/config/toml.rs` | `gen_default_flags()` 默认 0；`define_flags_diff` 字段表同步 |
| `easytier-core/src/config/mod.rs` | `RELAY_HOP_PENALTY_MAX` + `clamp_relay_hop_penalty`（**定义在 config 层**，所有消费者单一出处；OSPF 侧编译期断言钉死与 `OSPF_EDGE_COST_MAX/8` 的推导关系） |
| `easytier-core/src/config/api_input.rs` | ① Full→TomlConfigLoader 路径回填该字段（**超界报错**）；② TOML→proto 回显**生效值**（clamp 后，保证控制台不会显示一个自己保存时会被拒绝的数）；③ `FORM_MANAGED_FLAG_FIELDS` 登记 |
| `easytier-core/src/peers/route/peer_ospf_route.rs` | 核心：建图循环对 `src != my` 的边 `saturating_add` 惩罚；`update_route_table` 每次重建从 `context.flags()` 读取（**签名未变**），兜底 clamp + 按值去重 warn；runtime 变更分支触发重建（**带变更检测**，惩罚未变跳过）；>0 时 debug 日志 |
| `easytier-core/src/peers/route/peer_ospf_route_tests.rs` | 新增 6 个测试（见 §5） |

---

## 2. 为什么修改：问题背景

### 2.1 实测发现的现象

在某生产网络（下文脱敏为 N1 / N2 / RELAY-1 等 6 节点 + 1 个公网中继）上排查「两个局域网节点之间流量不走直连、绕行第三方中转」时，用 `ET-cli route` 拿到实测路由表：

| 目标 | 直连边代价 | 经 RELAY-1 中继路径代价 | LeastCost 选中 |
|---|---|---|---|
| N2（APP 类节点） | 9（单边 RTT 7.9ms） | 7（N1↔RELAY-1 0.23ms + RELAY-1↔N2 ≈6.8ms） | **中继** |
| N3（DB 类节点） | 9 | 7 | **中继** |

即：**直连已经成功建立**（peer 表全部为 p2p、0% 丢包、防火墙/监听均正常），但质量优先选路按「边代价线性相加」比较后，2 跳中继路径以 7 < 9 的微弱优势胜出。

**这个结果是契约内的正确行为**（§0），不是缺陷。

### 2.2 现象背后的模型盲区（本变更的动机）

OSPF 现有代价模型是**纯边代价线性相加**：k 边路径代价 = k 条边质量分之和。模型有一个系统性盲区：

> **中间转发节点不是免费管道。** 每个中转节点要付出拆封/解密/再加密/调度/占用自身转发带宽的真实开销，这些开销不在任何一条边的质量分里，模型完全不计。

后果（在 §0 契约仍然成立的前提下）：

1. **毫秒级 marginal 胜出被系统性放大**——如上例，2 条快边之和恰好压过 1 条稍慢直连边（7 vs 9），但真实端到端体验里这 ~1ms 的理论优势很可能被中继节点的转发开销吃掉甚至反超；
2. **中继节点成为隐性单点依赖**——选路把流量集中到某个「恰好网络位置居中」的节点上；
3. **无法表达「同局域网应优先直连」这类业务偏好**——此前唯一手段是整体关闭 `latency_first`（LeastHop 恒选直连），但那样连「烂直连让位给好中继」的合理场景一并牺牲。

### 2.3 设计目标

- 能表达「每多一跳就多一份开销」，让 marginal 中继胜出可被翻转为直连；
- **不破坏质量选路的核心能力**：真正大的质量差距（如 100ms 烂直连 vs 30+30ms 中继路径）下，中继依然必须能赢；
- 默认关闭、向后兼容、无需全网协议升级。

---

## 3. 方案设计

### 3.1 数学模型与硬上界

惩罚放在**本节点路由计算的建图阶段**：对每条「出边起点不是自己」的边加固定惩罚。k 条边的路径恰有 k-1 个中间节点，惩罚自然累加 k-1 次：

```
直连（1 条边）：      cost = edge_cost                        （零惩罚）
2 跳中继（2 条边）：  cost = edge1 + edge2 + 1×penalty
3 跳（3 条边）：      cost = edge1 + edge2 + edge3 + 2×penalty
```

**硬上界 `RELAY_HOP_PENALTY_MAX = OSPF_EDGE_COST_MAX / 8 = 125,000`**：管理 API 边界（`config::api_input`）**拒绝并报错**；TOML（以及任何绕过该边界的原始 `FlagsInConfig`）在 OSPF 读取处被 clamp 到上界 + 按值去重 warn。理由——惩罚是无 clamp 的纯加法项，无上界的操作值会撞穿两条既有代价语义不变量：

| 操作值 | 后果 | 上界的实际效果 |
|---|---|---|
| > ~1e6 | 2 跳路径 `path_latency` 越过 `OSPF_EDGE_COST_MAX`，与「这条腿被政策强制」的既有判读撞车（展示/排障误读） | **有界但不完全消除**：3 跳 + 接近 1e6 的边仍可达 ~3.4e6（≤ ~1e7 量级）。合理取值（推荐 3-5）下不存在该重叠 |
| ≥ 1e9 | 普通边权越过 `AVOID_RELAY_COST`，被当成政策否决：`path_len` 统计报废，~3 跳时 `Route.cost`（i32）变负 | **完全消除**：任何 k 的最坏路径 `k×(1e6+125k)` 都远低于否决带 |

125,000 同时已 ~125,000 倍于任何有意义的质量差距——打到上界即「配置错误被转化为有界可诊断状态」（边界报错 / 读取处一次 warn），而非静默污染无关的代价语义。上界定义在 config 层（`config::RELAY_HOP_PENALTY_MAX`，所有消费者单一出处），OSPF 侧用编译期断言钉死它与 `OSPF_EDGE_COST_MAX / 8` 的推导关系防漂移。

### 3.2 为什么必须进 api_manage（Full 下发清零问题）

`api_input.rs` 的 Full→TomlConfigLoader 转换从 `gen_default_flags()` **重建** flags，只回填它认识的字段。若 `NetworkConfig` 不携带 `relay_hop_penalty`，则：

- **Full PUT / console 托管下发 → 该值被静默清零并持久化**，把 TOML 里手工配的值冲掉；
- PATCH 路径不受影响（只改给定字段）。

这是 `disable_vip_reachability_probe`（字段 58）已踩过的既有模式，新字段不继承该缺陷。回填时**超界直接报错**（拒绝而非截断，与 MTU 边界同风格），保证管理面下发的值永远合法。

### 3.3 部署纪律（必须统一）

惩罚是**每节点独立的选路权重**。各节点取值不一致时，不同节点对「最优路径」的判断不再自洽——这是经典的前缀树不自洽 / 微环来源。因此：

- **全网节点必须设置相同值**（不是「建议」）；
- 值只需在一处定义（proto 注释），改值走与 `latency_first` 相同的全网配置流程。

### 3.4 关键设计决策

**为什么加在计算端而不是发布端**：发布端不知道自己的边会不会被当 Transit 用；加在发布端会把策略污染进 wire 协议、逼全网同步升级。加在本节点计算侧：零协议改动、每节点 opt-in，`path_latency` 如实包含惩罚（与熔断加价 `OSPF_FUSE_COST_BONUS`、政策否决 `AVOID_RELAY_COST` 同位同模式）。

**实现纪律：纯加法。** 只做 `saturating_add`，严禁任何「从总代价里反解质量分量」的技巧（历史 bug 正出于此，见 `AVOID_RELAY_COST` 注释里的 CR-09 记录）。

---

## 4. 对各消费者的影响（准确口径）

| 消费者 | 影响 |
|---|---|
| **LeastCost 下一跳**（`latency_first=true`） | **核心目标场景**：marginal 中继胜出可翻转为直连 |
| **LeastHop 下一跳与跳数** | **不变**：`normalize_edge_cost` 把边压成 1；等跳路径中间节点数相同，惩罚是常数偏移，第二阶段比价不改变排序 |
| **LeastHop 表的 `path_latency`** | **含惩罚**（上报的是 total_cost）。该值是地址归属仲裁的主键之一（`is_new_peer_better` 先比 `path_latency` 再比 `path_len`，且对两张表都跑）——`latency_first=false` 的节点，VIP/凭据归属**可能**因惩罚而翻转（例：2 跳 7 vs 直连 9，惩罚 3 后 10 > 9，归属换给直连节点）。`ET-cli route` 的 least_hop 行也会 +（k−1）×惩罚 |
| **`get_path_cost_with_policy` / `list_routes` 展示** | 展示值含惩罚（设计使然：`path_latency` 如实反映本节点的选路输入） |
| **出口/代理候选排序**（`peer_manager.rs:563` 的 20 单位死区） | **推荐值 3-5 且路径跳数 ≤4 时实际无效**（(k−1)×3..5 < 20）；≥6 跳时 (k−1)×5 ≥ 25 会越过死区。不要指望用推荐值调出口排序 |

**可观测性**：惩罚 >0 时每次重建打 debug 日志（`relay_hop_penalty` 值）；运维侧对比方法是同一节点开关该值前后跑 `ET-cli route`，观察 `path_latency` 差值是否等于 (k−1)×惩罚。

---

## 5. 测试

| 测试（`peer_ospf_route_tests.rs`） | 钉死的行为 |
|---|---|
| `relay_hop_penalty_flips_marginal_relay_win_to_direct` | 惩罚 0 复现中继胜出基线；惩罚 3 翻转直连；100ms 烂直连仍输 30+30 中继；首跳无惩罚；LeastHop 选路不变 |
| `relay_hop_penalty_accumulates_per_intermediate_hop` | **k=3 路径累加 2×惩罚**（60+2×3=66，非 63）；惩罚 6 翻转 |
| `relay_hop_penalty_clamped_to_max_and_stays_below_policy_veto_band` | clamp 函数边界（u32::MAX → MAX，编译期断言钉住上界 < 1e6）；最坏全 1e6 边 + 上界惩罚的 3 跳路径 `path_latency` 远低于 `AVOID_RELAY_COST` |
| `avoid_relay_veto_still_dominates_under_hop_penalty` | 避让中继边在惩罚开启时仍被否决、惩罚不能「收买」政策拒绝；胜出路径留在 merit 带 |
| `least_hop_diamond_choice_unchanged_but_path_latency_includes_penalty` | 等跳菱形：惩罚 0/5 下选路不变；`path_latency` 含惩罚（6 → 6+5） |
| `service_route_rebuild_reads_and_clamps_penalty_from_context_flags` | **生产接线**：`update_route_table` 从 context flags 读取（penalty 0→2、3→5）；u32::MAX 在读取路径被 clamp（→ 2+MAX） |
| `relay_hop_penalty_survives_full_config_generation_with_boundary_check`（`config/api_input.rs`） | Full 下发不清零；TOML↔proto 回显；超界**报错**（u32::MAX → Err，错误信息含字段名）；边界值本身合法 |

验证（2026-11-02，本机）：`cargo check -p easytier-core --all-targets` 全绿；`cargo clippy -p easytier-core --all-targets -- -D warnings` 通过（CI 的 `-D warnings` 口径，另跑过 `--features browser-config`）；OSPF 路由套件 149 passed；config 测试 112 passed（加 `browser-config` 时 117 passed）。

---

## 6. 使用指南与已知边界

### 6.1 使用

```toml
# 全网各节点必须统一（出厂默认即 5，此块用于显式声明或覆盖）
[flags]
relay_hop_penalty = 5
```

- `0` = 关闭（恢复历史行为，惩罚完全不参与选路）；
- 默认 `5` 翻转毫秒级 marginal 中继胜出（如两跳和 7 vs 直连 9），同时保住契约负例（13 vs 4+5=9）；
- 真正大的质量差距下中继依然胜出；
- 运行时修改经管理 API / TOML 重载下发后由 OSPF 主循环显式重建生效；
- 管理面（`NetworkConfig.relay_hop_penalty`）已支持，console 托管网络可安全使用。

### 6.2 已知边界 / 待办

1. **GUI 表单未暴露**——TOML 与管理 API 已可用，GUI「高级设置」页待跟进（`FORM_MANAGED_FLAG_FIELDS` 已登记，浏览器侧表单加字段即可）；
2. 惩罚是**全局固定值**，不区分具体中继节点（按节点定价属后续演进）；
3. **LeastHop 节点的地址归属仲裁含惩罚**（§4），对归属敏感的部署应评估后开启；
4. 代码形状遗留：`build_peer_graph_from_snapshot` / `build_from_snapshot_with_unusable` 参数较多（`locally_unusable` + 惩罚可收拢为 `GraphBuildOptions`），功能正确，重构低优先级。

### 6.3 相关排查结论（非本变更范围）

- 该网络直连建立链路（防火墙、11010 监听、NAT 打洞）经实测全部正常，「走中继」纯属选路结果；
- 若业务上要求「有直连必走直连」，更简单的手段是 `latency_first = false`（LeastHop）；`relay_hop_penalty` 的定位是**保留质量选路能力的前提下，让运维能校正模型对中转开销的低估**。
