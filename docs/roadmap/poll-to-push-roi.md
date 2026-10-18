# 轮询 → 推送：高收益 / 可控风险选型

## Status

- Status: **Roadmap**（**P0 代码已落地**，待 GUI 验收；P1 / §4.4 未实现）
- 日期：2026-10-18
- 最近审阅：2026-10-18（**P0 代码已落地**，待 Windows/Android GUI 验收；§4.4 另立不变）
- 归属：[`web-evolution.md`](./web-evolution.md) 阶段 B（控制台体验 / REST）
- 范围：把「定期拉取状态 / 配置视图」改成事件推送（或推拉混合）时，**哪些值得做、哪些不要动**
- 不覆盖：数据面转发性能、OSPF 选路本身（另见 [`connection-stability-todo.md`](./connection-stability-todo.md) P-AUDIT）；移动端 L2 **下发**见 §4.4（不进 P0）
- 相关 Current：[`../current/console-and-client-interaction-performance.md`](../current/console-and-client-interaction-performance.md)（交互延迟现状）、[`../current/traffic-steering.md`](../current/traffic-steering.md)（`desired_proxy_cidrs` / 平台装路；§4.4）
- 相关 Roadmap：[`client-web-console-interaction.md`](./client-web-console-interaction.md)（§9 / 阶段 G）、[`console-save-and-push-semantics.md`](./console-save-and-push-semantics.md)（Save→notify 已落地）
- 索引：[`../README.md`](../README.md)

---

## 1. 一句话结论

| 层 | 结论 |
|----|------|
| **配置热路径（控制台 → 设备）** | **已推送**：Save 落库后 `notify` 立刻 reconcile，不卡在 3.5s 心跳。心跳只保活 + 兜底，**不要改成纯推**。 |
| **真正还在「空转拉」的** | ~~GUI 外壳 1s~~ → **P0 已改**事件 + 30s 兜底；远程管理页 **5s** 全量快照、列表页 REST 轮询（P1）。 |
| **优先做（P0 已落地）** | 连接态 / 实例生命周期 / 配置已同步事件化；GUI 验收见 §4.1。 |
| **另立项** | 移动端 `desired_proxy_cidrs` **下发**事件化（§4.4）；`collect_network_info` 流式差分（§4.3）。 |
| **暂缓 / 不做** | 砍心跳、安卓 VPN 纯事件、Peer Center 60s 改推（§4.3 / §5）。 |

---

## 2. 现状地图（按「还能再推吗」）

### 2.1 已经是推（或推 + 周期兜底）

| 能力 | 机制 | 周期兜底 | 说明 |
|------|------|----------|------|
| 控制台 Save → 设备热重载 | DB bump + `notify` 唤醒 reconcile | 心跳 Full/Patch | 见 `console-save-and-push-semantics.md`；**不做**设备 pull RPC |
| 托管覆盖写回 GUI | Tauri `managed_config_synced_overwrite` | — | `manager.rs` emit；UI toast / 编辑器刷新 |
| 实例事件（DHCP / proxy_cidr / TUN） | `subscribe_native_instance_event` → emit | — | 已有范式，可复用 |
| 暂存包重发（pending flush） | `PeerInfoWakes.pending_flush` Notify | 20ms tick | P-AUDIT.12；重试发生在宿主进程内，不跨端 |
| 桌面 L2 路由安装（desired 集合的**计算**） | `PeerInfoWakes.proxy_cidr` Notify | 1s tick | P-AUDIT.11；tick 仅兜底。⚠️ 集合本身**下发仍靠宿主轮询** `NetworkInstanceRunningInfo`，不是推送 —— 见 §2.2 注 / §4.4 |

### 2.2 仍在定期拉（本文候选）

| ID | 位置 | 周期 | 拉取内容 | 单次成本感 | 用户可感知延迟 |
|----|------|------|----------|------------|----------------|
| **A** | `index.vue` `configServerTimer` | ~~1s~~ → **30s**（P0 已改） | `getConfigServerStatus`（含 W / prune） | 跨 Tauri/RPC，低频变更 | 事件路径 <200ms；漏事件时最多 30s |
| **B** | `index.vue` client/service timer | ~~1s~~ → **30s**（P0 已改） | `isClientRunning` / `getServiceStatus` | 同上 | tray / 「远程客户端」态 |
| **C** | `RemoteManagement` `STATUS_POLL_MS` | **5s** | `collect_network_info` / list ids / get_config 漂移 | **最重**（全量 peer+route；安卓再 + Binder） | 状态页/脏配置提示 |
| **D** | Web `DeviceList` / `NetworkList` 等 | **5s**（阶段 G 已从 2–3s 放宽） | `/machines` + `/devices` | 随设备数线性 | 控制台列表在线态 |
| **E** | `Status.vue` / `NetworkChart` | **2s** | 本地累计字节算速率 | 轻（不打重 RPC 时可留） | 速率曲线 |
| **F** | `mobile_vpn` 后台 sync | **10s** | VPN/路由 reconcile | 中；Doze 下 JS 不可靠 | 后台路由对齐 |
| **G** | `web_client` heartbeat | **3.5s** 默认 | 会话 + revision + W | 会话通道固定成本 | 离线检测 / 漏 notify 兜底 |
| **H** | Peer Center `report_peers` | **~60s** | 成本图上报 | 低敏感 | 几乎无 UX |

> 周期数字以代码为准；Current 性能文里被本文 P0 直接改动过、**待验收后改写**的地方不止「2s」：还有 `console-and-client-interaction-performance.md` 的 `index.vue:1192` 1 秒轮询（§2 第 63 行）、A1b / A5 的「1 秒级轮询」（§4.2）与建议 4 / 5（把 prune 与 T2/T3 的 1s IPC 改事件驱动）。见该文文首指针。
>
> **C 的配置页 ≈15s 不是同一定时器的另一种配置**：它走 `CONFIG_MODE_LIST_POLL_EVERY = 3` 这条**独立条件分支**（`pollTick % 3 === 0`），即每 3 个 5s tick 才跑一次 list/get_config；状态页本身仍是 5s。改这两条分支要分别动（对应 §4.1 P0-3）。
>
> **本表不含 `desired_proxy_cidrs`（候选 ID = L）**：它是 proto field 16，宿主按各自节奏轮询 `NetworkInstanceRunningInfo` 取回（安卓 GUI / OHOS / JNI 见 `traffic-steering.md` §3 平台表；JNI 约 3s）。§2.1 里事件化的只是**它的计算**（`resolve_proxy_cidrs` 随 peer-info 立即重算；桌面随即装 OS 路由），**跨进程下发仍是轮询**。要做近实时 → **§4.4 另立项**，不进本文 P0（与 GUI 1s 空转是不同面）。

---

## 3. ROI 矩阵（收益 × 风险）

评分：**收益** = 减 IPC/RPC 次数 + 体感延迟下降；**风险** = 漏事件、双端一致性、协议/安卓生命周期复杂度。

| 优先级 | 候选 | 收益 | 风险 | 建议 | 理由 |
|--------|------|------|------|------|------|
| **P0-1** | **A** config-server 状态事件化 | 高 | 低 | **已落地（待验收）** | `wait_changed` + Tauri emit；service 模式仍靠 30s 兜底 |
| **P0-2** | **B** 进程/服务运行态事件化 | 高 | 低 | **已落地（待验收）** | `client_rpc_state_changed` + post_run/vpn_stop；30s 兜底 |
| **P0-3** | **C′** 配置已同步 / revision 变更推到打开的编辑器 | 中高 | 低 | **已有** | Patch 与 overwrite 均已 emit；RemoteManagement 已 reload/脏提示（不改名） |
| **P1-1** | **C** 状态页：保留 poll，**hidden 跳过 + 事件唤醒立刻一轮** | 中 | 低～中 | **做** | 不全改推送；吞吐/拓扑仍需采样；与阶段 G 的 5s 叠加 |
| **P1-2** | **D** 列表页：SSE/websocket 或「机器上下线」轻量推 | 中 | 中 | **评估后做** | 设备多时收益大；要控制台侧会话钩子，范围大于 GUI |
| **P2** | **E** 速率图 | 低 | 低 | **保持采样** | 速率本质是差分采样；推送无意义 |
| **—** | **F** 安卓 VPN 去周期 | 中（理论） | **高** | **不做纯推** | WebView/Doze 冻 JS；已有 Rust `android_vpn_watchdog`；应加强 native tick，而非删周期 |
| **—** | **G** 心跳改纯推 | 低（配置已 notify） | **高** | **不做** | 保活 / 半开会话 / 漏通知兜底；文档已禁止对打的 pull |
| **—** | **H** Peer Center | 低 | 中 | **不做** | 聚合上报，延迟不敏感 |
| **—** | **L** 移动端 `desired_proxy_cidrs` 下发事件化 | 中（路由收敛） | 中 | **另立项（§4.4）** | 计算已 Notify；缺的是宿主侧推送。可复用 `proxy_cidrs_updated`；勿与 P0 GUI 空转混做 |
| **—** | `collect_network_info` 流式差分协议 | 很高（长期） | **高** | **另立项** | 要改 proto、兼容旧端、快照一致性；不进本篇 P0 |

```text
        高收益
          │
  P0-1/2  │  P1-1 混合
          │
   P0-3   │  P1-2 列表 / L 下发
          │
  E 采样  │  F/G/H 不动
          └────────────────── 高风险 →
```

> 图中用**优先级 ID**（与上表一致）；对应 §2.2 的候选 ID：**P0-1=A**（config-server 状态）、**P0-2=B**（进程/服务运行态）、**P0-3=C′**（配置已同步）、**P1-1=C**（状态页）、**P1-2=D**（列表页）、**P2=E**、**不做=F/G/H**、**另立=L**（desired 下发）。`C` 与 `C′` 是同一层的两个不同面：`C` 是全量状态采样，`C′` 是离散的配置同步事件。

---

## 4. 推荐落地（按阶段）

### 4.1 Phase P0 — GUI 外壳去 1s 空转（只升 gui + 少量 core 钩子）

**目标**：前台正常时，A/B 从「每秒 RPC」变为「变更才推」；保留 **慢速兜底 poll**（如 15–30s）防漏事件。

| 步骤 | 做法 | 锚点（易漂移） |
|------|------|----------------|
| P0-1a | ✅ core：`generation` + `Notify`；`mark_*` / `clear` / host 变更 / W store·clear → bump | `config_server_status.rs`；`web_client.rs` |
| P0-1b | ✅ Tauri：`wait_changed` → `emit("config_server_status_changed")`（空 payload，前端再 `get_config_server_status`）；`index.vue` listen。前端 refresh：**单飞 + dirty 再跑**（避免末次 bump 被吃掉） | `lib.rs` `spawn_config_server_status_watch`；`event.ts` |
| P0-1c | ✅ 原 1s timer → **30s 兜底** + 事件即时刷新；`waitForConfigServerOutcome` 仍自循环 refresh（未改），与事件路径共用同一单飞/dirty | `index.vue` |
| P0-2a | ✅ `init_rpc_connection` 建立/拆掉 tunnel 时 `emit("client_rpc_state_changed")`；`post_run_network_instance` / `vpn_service_stop` 桥到 window。⚠️ 隧道**自行**断连不发事件，只靠 30s 兜底 | `lib.rs`；`event.ts` |
| P0-2b | ✅ `isClientRunning` 1s → 事件 + **30s** 兜底 | `index.vue` 第二段 `onMounted` |
| P0-3 | ✅ **已有**：Patch 与 overwrite 均 emit `managed_config_synced_overwrite`；RemoteManagement 已 reload/脏提示。**不改名** | `manager.rs`；`RemoteManagement.vue` |

**明确不做（本 Phase）**：改 heartbeat 间隔；动安卓 VPN 10s；改 `collect_network_info` 载荷。

**验收**

- [x] 代码：generation/Notify + Tauri watch + 30s 兜底 + RPC state emit（**待现网体感**）
- [ ] 稳态前台：A/B 相关 IPC ≤ 约 2–4 次/分钟（仅兜底），连接成功后 UI **<200ms** 内反映（事件路径）。
- [ ] 断线 / 服务停止 / W 变化后 prune 行为与今日一致（无漏 prune、无误 prune）。
- [ ] `waitForConfigServerOutcome` 仍能在超时内区分 connected / failed / disabled。
- [ ] Windows + Android GUI 各测一遍（Android 事件在前台；后台仍靠兜底 + 既有 watchdog）。

### 4.2 Phase P1 — 状态页「事件唤醒 + 慢轮询」（frontend-lib，两端共享）

**目标**：不消灭 `collect_network_info` 采样，但减少无效轮次、缩短「拓扑已变但 UI 还要等 5s」的尾延迟。

| 步骤 | 做法 |
|------|------|
| P1-1a | 已有 `document.hidden` 跳过则确认 RemoteManagement / Status 全覆盖 |
| P1-1b | 订阅轻量信号：`PeerAdded`/`PeerRemoved` / `proxy_cidrs_updated` / `dhcp_ip_changed`（GUI 已有部分）→ `pollTick` 立刻跑一轮（仍走 single-flight） |
| P1-1c | 配置模式：revision/synced 事件优先于 `CONFIG_MODE_LIST_POLL_EVERY` |
| P1-2 | （可选）控制台列表：机器 session bind/unbind 时推一条轻量「online 变更」，列表页降到 15–30s 全量 |

**验收**

- [ ] 隐藏页时无 collect 风暴；回到前台立即一轮。
- [ ] 加/删 peer 后状态页延迟 **明显低于** 固定 5s 上限（事件路径）。
- [ ] 与阶段 G S10–S13 单飞 / 15s probe 超时无回归。

### 4.3 明确不进本路线图的「大件」

| 项 | 原因 |
|----|------|
| Heartbeat → 纯服务端推配置 | 保活与「配置通道」绑在同一会话；Save 已 notify；纯推无法替代半开检测 |
| `collect_network_info` 增量/流式 | 收益大但协议+兼容面大 → 单独立项（可挂 `web-evolution`） |
| 安卓去掉一切周期 sync | 与 Doze/WebView 事实冲突；方向应是 **native 周期 + 前台事件**，不是删周期 |

### 4.4 另立项：移动端 `desired_proxy_cidrs` 下发（候选 L）

**问题拆分**（勿与 §2.1「计算已事件化」混淆）：

| 段 | 今日 | 要否进本文 P0 |
|----|------|----------------|
| Core 内 `resolve_proxy_cidrs` + 桌面 `apply_route_changes` | Notify + 1s 兜底（P-AUDIT.11） | 否（已做） |
| 集合写入 `NetworkInstanceRunningInfo.desired_proxy_cidrs`（#16） | 随 collect / 宿主读 RunningInfo | 否 |
| **宿主**（Android GUI / JNI / OHOS）读 #16 并装 VPN/TUN 路由 | **轮询** RunningInfo（JNI ≈3s；GUI 跟 RemoteManagement / VPN sync） | **否 → 本小节** |

**方向（草案，开干前再收口）**

1. Core 在 desired 集合相对上次快照变化时，复用或扩展已有实例事件（桌面已有 `proxy_cidrs_updated` emit 范式）。
2. 宿主 listen → 立刻 `get_network_info` / 读 RunningInfo 一次并装路由；**保留**低频 native 周期（安卓 Doze / watchdog），禁止「纯事件、无 tick」。
3. 与 P0-1/2 解耦：本项动的是 **L2 路由收敛延迟**，不是 GUI 外壳 1s IPC；可挂 `traffic-steering` / connection-stability，不必阻塞本文 P0。

**验收（立项时再细化）**

- [ ] peer proxy_cidr / policy 变更后，移动端装路延迟明显低于固定 3s/5s 轮询上限（事件路径）。
- [ ] 旧 core（无 #16）回退路径不变；后台/Doze 下仍靠 native 周期收敛，无永久陈旧路由。

---

## 5. 非目标

- 不把 Peer Center / OSPF / ping 自适应改成「全事件、无 tick」。
- 不引入设备主动 pull 托管配置的新 RPC（与 Save→Push 语义冲突）。
- 不在未过 Current §1 门槛前，把「已事件化」写进 `docs/current/` 正文。
- 不把 §4.4（desired **下发**）塞进本文 P0 实现范围。

---

## 6. 与既有文档的分工

| 文档 | 管什么 |
|------|--------|
| **本文** | 轮询→推送的 **选型 / ROI / 排期** |
| `console-and-client-interaction-performance.md` | 交互慢的 **现状事实**（验收前勿当「已改完」） |
| `client-web-console-interaction.md` | 超时、覆盖、会话、阶段 G 放宽轮询等 **已排交互项** |
| `console-save-and-push-semantics.md` | 配置 **Save→notify** 语义（热路径已推） |
| `traffic-steering.md`（Current） | L2 desired / 各端装路；§4.4 开干时对照平台表 |

阶段 G 已把部分 poll **放宽**（2s→5s）；本文 P0/P1 是下一步 **减次数 + 事件补延迟**，不是再加一轮「继续放宽间隔」了事。§4.4 不挡 P0。

---

## 7. Changelog

| 日期 | 说明 |
|------|------|
| 2026-10-18 | 首版：P0 GUI 1s 事件化；P1 状态页混合；心跳/安卓纯推/差分协议列为非目标或另立 |
| 2026-10-18 | 补 §4.4 / 候选 L：`desired_proxy_cidrs` 计算≠下发；移动端近实时另立项，不进 P0 |
| 2026-10-18 | **P0 落地**：`wait_changed` + GUI emit/30s；`client_rpc_state_changed`；P0-3 核实已有；待 GUI 验收 |
| 2026-10-18 | **审查修正**：前端 refresh 加**单飞合并**（事件突发与 `waitForConfigServerOutcome` 不再重叠）；§2.2 A/B 周期 1s→30s 同步；`post_run` 正名为 `post_run_network_instance`；注明隧道自行断连不发事件 |
| 2026-10-18 | 单飞补 **dirty 再跑**（末次 bump 不丢）；Current 性能文链改「P0 已落地，待验收」 |
