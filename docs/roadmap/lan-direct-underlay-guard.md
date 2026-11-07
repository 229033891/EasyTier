# 同网段直连加固：接收侧路由剪枝、下垫面钉扎与声明侧卫生

## Status

- Status: **Roadmap**（L0/L1/L2 **代码已落地；现网 `/22` 冒烟未做** → 发版口径写「待现网确认」，**勿**宣称事故已永久关闭。L4/L3 未做。施工绑定 [`open-bugs-wave1-impl.md`](./open-bugs-wave1-impl.md) §7）
- 最近审阅：2026-11-07（接入点勘误 + Wave1 落地 + 发版口径）
- 背景：2026-11-04 现网事故——同局域网跨网段节点（192.168.11.124 ↔ 192.168.10.101）直连反复建拆，OSPF 稳定绕行中转节点。诊断结论见 §2。
- 相关 Roadmap：[`open-bugs-wave1-impl.md`](./open-bugs-wave1-impl.md)（Wave 1 施工）、[`default-route-and-underlay-excludes.md`](./default-route-and-underlay-excludes.md)（underlay 排除思想同源）、[`unified-routing-design.md`](./unified-routing-design.md)（路由所有权）、[`multi-link-bonding.md`](./multi-link-bonding.md)（Phase 3 `bind_device` 多样性）、[`connection-stability-todo.md`](./connection-stability-todo.md)、[`stability-abc-plan.md`](./stability-abc-plan.md)（liveness 稳定性总纲）、[`gui-connected-vs-reachable.md`](./gui-connected-vs-reachable.md)
- 相关 Current：[`../current/peer-connections.md`](../current/peer-connections.md)、[`../current/traffic-steering.md`](../current/traffic-steering.md)、[`../current/underlay.md`](../current/underlay.md)、[`../current/data-plane.md`](../current/data-plane.md)
- 索引：[`../README.md`](../README.md)

---

## 1. 问题陈述

用户直觉（2026-11-04 讨论）：**NAT 类型描述的是公网侧打洞能力；同一局域网的设备直连根本不经过 NAT，不应被 NAT 语义牵连。**

代码核实：该直觉成立。

- LAN 直连（DirectConnector，`easytier-core/src/connectivity/direct/mod.rs`）**不读 NAT 类型**；NAT 只门控打洞——`get_punch_hole_method`（`easytier-core/src/connectivity/hole_punch/udp/common.rs:81-137`）中 HardSymmetric ↔ 对称 → `None`（`:128-130`），HardSymmetric ↔ cone → `SymToCone`（`:131-133`）。
- 因此「symmetric 阻止同网段直连」在现有代码中**不成立**；事故根因是 §3 的 overlay 路由劫持，与 NAT 无关。

## 2. 事故诊断摘要（2026-11-04）

时间线（源：现网 `easytier.log`，本机 192.168.11.124，GUI 托管实例）：

| 时间 | 事件 |
|---|---|
| 01:13:58 | config-server 推送 proxy CIDRs：`added=[172.16.0.0/16, 172.25.0.0/24, 192.168.8.0/22]` |
| 01:14:00 | 直连连接开始 1.5–12s 周期建拆：`fast liveness probe: link not round-tripping, misses=3`（握手成功、RTT 691µs 的连接随后判死） |
| 全程 | `tunnel src 10.10.10.25 is from the same network` 拒绝 **351 次**；`timed out waiting for first handshake packet` 162 次 |
| 01:23:41 | proxy CIDRs 移除（含 192.168.8.0/22） |
| 01:28:04 | 再次加回 → 震荡继续 |

根因：**`192.168.8.0/22` 覆盖 192.168.8.0–192.168.11.255，把两台节点的物理网段（10.x 与 11.x）全部纳入 TUN 路由**。本机网段靠 OS connected 路由（最长前缀）侥幸不受影响，但**对端物理地址 192.168.10.101 落在 /22 内且只能经默认路由到达**，于是直连流量进 TUN、源地址变为虚拟 IP，pong 回不到原连接，conn 级 liveness 判死，OSPF 弃用直连边。

已排除：跨网段防火墙（路径为纯交换）、双实例（GUI + core 为正常进程结构；日志中同机多 peer_id 系重启后 peer_id 重新随机生成）。

## 3. 根因与不变式

- 不变式（本次复核确认）：**只要本机路由表里存在一条「比物理默认路由更具体、且包含对端 underlay 地址」的 overlay 路由，直连 / pong / 握手回包就会被抢进 TUN**。与谁宣告、宣告粒度无关。
- 推论：修在**接收侧**才有效。声明侧只能降低发生概率，无法保护未升级的对端——`proxy_networks` 由属主节点自报并经 OSPF 同步（`easytier-core/src/peers/route/peer_ospf_route.rs:1194-1195`），接收方无法假设对端是哪个版本。
- 推论：不能整条丢弃宣告。`192.168.8.0/22` 里 `192.168.10.0/24` 是对端 LAN 的合法可达路径，整条丢弃会把「子网代理」这个核心能力一起修没。

## 4. 原方案复核（A–D）

| 原项 | 复核结论 | 依据 |
|---|---|---|
| **A** 直连候选含虚拟 IPv4 | **成立** | `direct/mod.rs:539-557`：listener 0.0.0.0 时 IPv4 候选 = `interface_ipv4s.chain(public_ipv4)`，无托管地址过滤；IPv6 有 `is_easytier_managed_ipv6`（`peer_manager.rs:2265-2274`），IPv4 无对应函数。`collect_bind_addrs`（`manual/mod.rs:1137-1168`）更把全部 `interface_ipv4s` 当源地址返回 → `tunnel src 10.10.10.25` 的来源。`should_deny_target`（`direct/mod.rs:532-536`）只拒「目标 == 本地 IP 且端口相同」，拦不住。 |
| **B.1** 拨号绑物理接口 | **方向对，机制描述有误** | 现有 `bind_device: true`（`direct/mod.rs:710-723`）只是把 `interface_ipv4s` 当源地址，**不是** `IP_UNICAST_IF` / `SO_BINDTODEVICE`。真正的出口钉扎在 `easytier/src/tunnel/common.rs:125-157` 与 `easytier/src/arch/windows.rs:108-172`（`set_ip_unicast_if`），需要「本地 IP → 接口名」的管道，而 `host.interface_addrs()` 只给 IP（`connector_host.rs:242-259`）。 |
| **B.2** /32 排除路由 | **机制已存在，触发条件不符** | `collect_underlay_exclude_ips`（`easytier-core/src/gateway/proxy/underlay_exclude.rs:16-49`）已采集 peer tunnel underlay IP + STUN IP；`should_exclude_ip`（`:70-84`）**会纳入 RFC1918**（不是「放行跳过」——2026-11-07 勘误）。安装器 `reconcile_underlay_exclude_routes`（`virtual_nic.rs:1556-1660`）已实现物理默认网关解析、变更重装、失败重试。触发条件过窄：install 门闩 `sync_proxy_cidrs_with_underlay_excludes`（~1280）与 tick（~1944）都只认 TUN `/0`——**本次 /22 不触发**。 |
| **C** liveness 闪断放大 | **已被现行代码覆盖，按原文施工 = 重复施工** | `peer_conn_ping.rs`：`FAST_PROBE_INTERVAL` 333ms（:95）、`FAST_PROBE_MISSES_TO_DEGRADE = 3`（:103）、`FAST_PROBE_RECOVER_ROUNDS = 3`（:119）、`FAST_PROBE_SUSTAINED_ROUNDS = 9`（:135）；恢复迟滞已实现（:147-155）；seq 隔离 `FAST_PROBE_SEQ_BIT`（:246）、Pong 按 seq 匹配（:526-527）；`:392-406` 注释逐字记录了「keepalive 直写 `store(false)` 绕过门控」的旧 bug 及修复。`ctrl_resp_sender` 是 `PeerConn` 字段（`peer_conn.rs:313`），每 conn 一个 broadcast，「跨 conn 串扰」不成立。 |
| **D** NAT 语义解耦 | **层级错位** | `nat_type` 是 peer 级展示（`easytier/src/easytier-cli/main.rs:1549`、`:1628`、`:1654`），GUI 对端表没有 NAT 列，不存在「每条边一个 NAT 值」需要解耦。降级为 P3 文案项。 |

结论：诊断扎实，但原方案把主修放在声明侧、且 B 的机制描述与代码不符。下文按 **L1 → L2 → L0 → L4 → L3** 重排。

## 5. 修复设计

小节按施工顺序排列（L1 → L2 → L0 → L4 → L3）；层号表示依赖与优先级，不对应小节编号。

### 5.1 L1 接收侧路由剪枝（核心，P0）

目标：本机安装 proxy 路由前，先减去**本机物理网段 + 本机虚拟网段**，只剪重叠部分。

规则：

1. **`/0` 永不剪**。`virtual_nic.rs:1163`、`:1178` 的 exit-node 度量逻辑依赖 `network_length() == 0`；且 `/0` 减一个 `/24` 会拆成 24 条具体路由，默认路由语义丢失。本机 LAN 由 connected 路由天然胜出，无需剪。
2. 前缀包含本机网段 → **减去**该网段（`192.168.8.0/22` − `192.168.11.0/24` = `192.168.8.0/23` + `192.168.10.0/24`），而非整条丢弃。
3. 剪完为空 → 丢弃该前缀并 warn（带 peer_id）。
4. 本机**虚拟网段**也进集合：对端宣告 `10.0.0.0/8` 会连 overlay 自己一起劫持，当前无任何防护。

接入点（**2026-11-07 修正**：唯一剪枝点必须在 monitor 收集路径，不能只挂 packet_plane 包装）：

- **`easytier-core/src/gateway/proxy/cidr_monitor.rs` → `collect_proxy_cidr_state`**：`resolve_proxy_cidrs` 之后剪枝，再 diff / 发 `ProxyCidrsUpdated`。桌面 `virtual_nic` 主要吃事件里的 `added/removed`，**绕过** `packet_plane` 包装；只剪 packet_plane 会装回未剪枝前缀。
- 新增 `CorePacketPlane::set_local_ipv4_networks`（内部 `ArcSwap<Option<Vec>>`）：`None`=未注入，`Some`=已知集合。变化时 `proxy_cidr_wake.notify_one()`；monitor 快路径比对 Arc，避免「只写不动」导致 L1 永不重跑。
- L1：holes 空（未注入且无 VIP）→ 不剪（兼容）；L2：`None` 仅触发 `/0` 门闩，**不**把未知当全 off-link。
- **覆盖范围：仅桌面**。注入入口是 `run_proxy_cidrs_route_updater`（`virtual_nic.rs` 启动路径）；Android/mobile 等不跑该 updater → 永远 `None`，L2 退回「仅 `/0`」旧行为。
- 网段采集：`collect_local_ipv4_networks` → `Option<Vec>`；枚举/`JoinError` 失败返回 `None`，`set_local_ipv4_networks(None)` **保持旧值**（避免 WMI 抖动写成 `Some([])` 触发 L2 路由抖）。Windows **必须按 TUN ifname 排除**；`prefix < 8` 跳过（60s 限频 warn）。线程跃迁在采集入口；底层同步 `show()`。
- 注入时机：`run_proxy_cidrs_route_updater` **首次 `proxy_cidr_diff` 前先采**，之后每 5s（`Instant`）重采。
- ⚠️ **L1 alone 不够**：剪掉本机 `/24` 后仍可能留下覆盖对端物理 IP 的子前缀 → **必须配 L2**。

算法（新增 `easytier-core/src/gateway/proxy/proxy_cidr_prune.rs`）：

```rust
pub fn subtract_ipv4(base: cidr::Ipv4Cidr, holes: &[cidr::Ipv4Cidr]) -> Vec<cidr::Ipv4Cidr>;

fn subtract_one(base: Ipv4Cidr, hole: Ipv4Cidr) -> Vec<Ipv4Cidr> {
    if !overlaps(base, hole) { return vec![base]; }
    if hole.contains(&base) { return vec![]; }
    if base.network_length() >= 32 { return vec![]; }
    let (lo, hi) = split(base);            // 前缀 +1 的两半
    let mut out = subtract_one(lo, hole);
    out.extend(subtract_one(hi, hole));
    out
}
```

复杂度 O(32 × holes)，可忽略；`cidr` crate 无内建减法，需自写并配单测。

### 5.2 L2 下垫面 /32 钉扎（P0，复用现成机制）

判据从「有没有 TUN /0」改为「**有没有 peer 下垫面地址被某个（已剪枝）desired 前缀覆盖**」。

**2026-11-07 修正**：必须改 **两处**（或抽成同一谓词），只改 tick 不够：

1. **Install 门闩**（权威）：`sync_proxy_cidrs_with_underlay_excludes` 内原 `will_have_tun_default`（~1280）——真正决定是否 `reconcile_underlay_exclude_routes`。
2. **Tick 门闩**（~1944）：避免无谓词时跳过 `underlay_exclude_ips()` 采集；可 OR「已安装集合含 `/0`」以免拆除窗口漏 exclude。

```rust
let needs_excludes = desired.iter().any(|c| c.network_length() == 0)
    || underlay_ips.iter().any(|ip| {
        desired.iter().any(|c| c.contains(ip)) && !local_nets.iter().any(|n| n.contains(ip))
    });
```

两个必须加的例外：

- **on-link 例外**：下垫面地址落在本机物理 `local_nets` 内时不装 `/32`（`local_nets` 需 L1 新建；勿与 overlay `is_ip_in_same_network` 混淆）。
- **本地虚拟 IP 例外**：`should_exclude_ip` 已用 `is_local_virtual_ip` 排除（`underlay_exclude.rs:75`），保持。

拆除路径已存在：`enable_excludes == false` 分支会删掉已装 `/32`（`virtual_nic.rs:1567-1581`），条件变假即自动清理，幂等。

> 注意：`/32` 不需要「权重压过 /22」——最长前缀匹配天然生效，只要路由存在即可。

### 5.3 L0 直连候选净化（P1，最便宜）

共用同一谓词（仿 `is_easytier_managed_ipv6`，`peer_manager.rs:2265-2274`）：

```rust
pub async fn is_easytier_managed_ipv4(&self, ip: Ipv4Addr) -> bool {
    self.is_local_virtual_ip(&IpAddr::V4(ip)) || context.is_ip_in_same_network(&IpAddr::V4(ip))
}
```

三处接入：

- `direct/mod.rs:539-557` 候选生成后过滤；
- `direct/mod.rs:532-536` `should_deny_target` 加同一判定；
- `manual/mod.rs:1137-1168` `collect_bind_addrs` 过滤源地址（修复 `tunnel src 10.10.10.25`）。

`check_resolved_remote_addr_not_from_virtual_network`（`peer_manager.rs:1474-1500`）语义正确（防环路），**不改**。

### 5.4 L4 声明侧卫生与冲突检测（P2，辅助）

1. **宣告前自检**：候选前缀包含本机物理网段 → warning + 不宣告（不阻塞启动）。core 层拿不到接口列表，建议复用 L1 注入的 `local_nets`，在 `collect_proxy_cidrs` 出口统一判定；config-server 推送同样经此路径（`config_patch.rs:28`、`:80`、`:474-501`）。
2. **冲突检测**（当前完全缺失）：两 peer 宣告同前缀 → warn + `easytier-cli route` 标记 ambiguous；部分重叠只告警不改写。
3. **mapped 校验补齐**：`toml.rs:1219-1225` 只校验了 real/mapped 前缀长度一致；还需校验 mapped 不与虚拟网段、其他 proxy 的 real/mapped 前缀重叠。
4. 运维收紧：`allowed_proxy_cidrs` 是 credential 级子集白名单（`peer_ospf_route.rs:4883-4908`，为空时清空全部宣告），文档应将其写成「收紧到 /24 粒度」的运维动作。

### 5.5 L3 出口绑定（P2，可延后）

B.1 的真实缺口是「本地 IP → 接口名」：需要在 `direct/mod.rs:710-723` 按目的地址解析接口名，灌入 `UdpBindOptions.bind_device`（`easytier/src/socket/udp.rs:145-147`），由 `setup_socket_for_win`（`arch/windows.rs:174-198`）经 `set_ip_unicast_if` 生效。与 [`../current/underlay.md`](../current/underlay.md):124 的「套接字绑定物理网卡并排除已注册 TUN ifname」合并施工，不为本次事故单独赶工。

## 6. 重叠网段与 mapped CIDR

两端 LAN 同网段（如都是 `192.168.1.0/24`）时，声明净化**无效**：不宣告 = 访问不到；宣告 = 与本地 connected 路由同前缀，胜负取决于 metric（`implicit_route_metric` 对非 `/0` 返回 9000，`easytier/src/common/ifcfg/mod.rs:53-59`；Windows 侧 `ifcfg/windows.rs:193-216`），结果静默错误。

唯一正确解是 **mapped CIDR**（已实现，开箱可用）：

| 环节 | 实现 |
|---|---|
| 配置 | `proxy_networks = ["192.168.1.0/24->10.100.1.0/24"]`，解析 `config/api_input.rs:86-101`；字段 `ProxyNetworkConfig { real, mapped }`（`config/mod.rs:282-285`，TOML `toml.rs:627-631`） |
| 宣告 | 通告 mapped 前缀：`context.rs:850` `config_ipv4_cidr(proxy.mapped.as_ref().unwrap_or(&proxy.real))` |
| 数据面 | `gateway/proxy/cidr_table.rs:92-105` `lookup_v4` 反写，调用点 `tcp_proxy_engine.rs:569`、`udp_proxy_engine.rs:224`、`wrapped_tcp_proxy.rs:62`、`icmp_proxy_engine.rs:93` |
| 兜底 | overlay 数据面不读本机路由表：`get_msg_dst_peer_ipv4`（`peer_manager.rs:4215-4237`）按 VIP → `local_route_table.lookup`，故端口转发 / SOCKS5 在同网段下仍可用（`gateway/port_forward.rs:442`） |

已知限制：两端都声明同一 real 前缀时，OSPF 两个同前缀候选，选谁取决于代价，**不可预期**——列入 known limitation，不做自动仲裁。

## 7. 生命周期、幂等与可观测

- **刷新**：DHCP 续约 / Wi-Fi 漫游 / 热点切换会让本机网段变化，desired 必须跟着变；剪枝集合重算后由同一 tick 的 diff 完成 add/remove。
- **幂等**：`installed` 记录实际装上的（已剪枝）前缀，desired 记录本 tick 剪枝结果，两者同 tick 计算。
- **日志**：`proxy cidr {cidr} pruned by local network {net} -> {parts}`，每前缀每次变更打一次；整条丢弃 warn 带 peer_id。
- **状态**：`ProxyCidrRouteSyncStatus`（`packet_plane.rs:101-111`）增加 `pruned: Vec<(Ipv4Cidr, Ipv4Cidr)>`，`easytier-cli route` 与 GUI 展示，避免「访问不到对端某段」变成玄学。
- **逃生阀**：`--no-proxy-cidr-prune`（默认开），给「就是要用 overlay 覆盖本地网段」的极端用户。

## 8. 测试计划

1. 单测 `subtract`：`/22` − `/24` → 2 条；`/24` − `/32` → 8 条；完全覆盖 → 空；无交 → 原样；`/0` 不剪。
2. 单测 `needs_excludes`：on-link 不装、off-link 且被覆盖装、条件消失后拆除。
3. 集成（扩展 `easytier/src/tests/three_node.rs:4573-4640` 现有 `ProxyNetworkPatch` 用例）：同 LAN 跨网段两节点 + 一方宣告 `/22`，断言 60s 内直连不重建、`same network` 日志为 0、本机 LAN 不被劫持、对端 LAN 仍可达。
4. 回归：exit node `/0` 场景 exclude 行为与今天逐字一致（最易被 L2 改坏）。
5. 现网：`grep -c "is from the same network"` 归零；`fast liveness probe` 不再周期出现。

## 9. 排期

| 层 | 内容 | 人日 | 依赖 |
|---|---|---|---|
| L1 | `proxy_cidr_prune` + 采集 + 注入 + 单测 | 2–3 | 无 |
| L2 | exclude 判据扩展 + on-link 例外 | 1–2 | L1（共用 local_nets） |
| L0 | `is_easytier_managed_ipv4` + 三处过滤 | 0.5–1 | 无 |
| L4 | 声明自检 + 冲突检测 + mapped 校验 | 2–3 | L1 |
| L3 | 出口绑定（与 underlay.md 合并） | 3–5 | 无 |

核心（L1+L2+L0）4–6 人日即可消掉现网反复建拆；L4 防复发，L3 长期架构。

## 10. 验收标准

| 项 | 验收标准（可量化） |
|---|---|
| L1 | 推送覆盖性 proxy CIDR 后：本机路由表只出现剪枝后的前缀；`pruned` 列表可查；本机 LAN ping 不中断 |
| L2 | peer 下垫面 `/32` 指向物理网关（on-link 场景不出现）；`/32` 存在时 `same network` 日志为 0 |
| L0 | 候选列表不含任何 peer 虚拟 IP；`tunnel src ... is from the same network` 归零 |
| L4 | 覆盖自身物理网段的宣告被拒并告警；同前缀冲突在 CLI 可见 |
| 综合 | 事故复现环境（推送 `/22`）下直连 conn 连续稳定 ≥ 10 min，`not round-tripping` 不出现 |

运维侧临时缓解（不依赖代码）：为跨网段对端物理 IP 添加 `/32` 静态路由指向物理网关（最长前缀即生效，无需「权重压过」）；或避免下发覆盖节点自身物理网段的 proxy CIDR。

## 11. 待决

- `/32` 排除路由的安装方：**已结论——客户端本地安装**（复用 `underlay_exclude`），不需要控制台下发 exclude 列表；路由所有权语义与 [`unified-routing-design.md`](./unified-routing-design.md) 对齐时只需确认「本地 /32 不参与 overlay 路由计算」。
- 两端 LAN 同前缀：列入 known limitation，指引用户改用 mapped CIDR，不做自动仲裁。
- peer_id 每次重启随机再生成（本次诊断确认）是否保留：GUI 历史对比、控制台审计均受影响，另立议题，与本问题解耦。
