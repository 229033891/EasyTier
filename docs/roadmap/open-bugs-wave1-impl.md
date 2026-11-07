# Wave 1 开放缺陷施工方案（复核后）

## Status

- Status: **Roadmap**（2026-11-07 对照 HEAD 代码复核完成；本篇为施工绑定文档）
- 最近审阅：2026-11-07
- 基线：`dev` / 2.7.11
- 上游绑定：
  - [`lan-direct-underlay-guard.md`](./lan-direct-underlay-guard.md)（LAN；本文 §2 修正其接入点漂移）
  - [`../current/routing-bug-audit-2026-10.md`](../current/routing-bug-audit-2026-10.md)（B3/B4/B17）
  - [`../current/traffic-steering.md`](../current/traffic-steering.md)（B4 契约已写、B17 口径）
- 索引：[`../README.md`](../README.md)

---

## 0. 复核结论（先评再做）

### 0.1 总判

| 条目 | 方案是否合理 | 可否按原文开工 | 处置 |
|---|---|---|---|
| LAN L1+L2+L0 | **方向正确**；两处结构性漂移必须先改方案 | **否（须按 §2 修正后）** | 修正接入点后施工 |
| LAN L4/L3 | 合理；本波不做 L3 | L4 可跟；L3 延后 | Wave 1b / 3 |
| B3 冲突不覆盖+warn | **合理**（可观测小步） | 是 | Track A |
| B3 完整 A/B | 合理但未选型 | **否** | Track C，本波不做 |
| B4 许可门 | **合理**；Current 已承诺，代码未兑现 | 是（对齐文档） | Track A |
| B17 文档/UI | **合理** | 是 | Track A |
| B2/B10/B15/B16/B17 协议 | 需设计 | **否** | Track C |

### 0.2 LAN 原文必须修正的两点（阻塞）

1. **L1 剪枝不能只挂在 `packet_plane` 包装上。**  
   桌面 desired 主要由 `cidr_monitor::collect_proxy_cidr_state` → `ProxyCidrsUpdated` 驱动（`cidr_monitor.rs`），`virtual_nic` 事件路径**不**再经 `packet_plane` 收集。只剪 packet_plane → 事件路径仍装未剪枝前缀。  
   **正确 choke point**：在 `collect_proxy_cidr_state`（`easytier-core/src/gateway/proxy/cidr_monitor.rs`）对 `resolve_proxy_cidrs` 结果做剪枝；`local_nets` 用共享 `ArcSwap`（或等价）供 monitor 读取。

2. **L2 `needs_excludes` 不能只改 tick 门闩（~1944）。**  
   真正决定是否装 `/32` 的是 `sync_proxy_cidrs_with_underlay_excludes` 内 `will_have_tun_default`（~1280）。只改 tick → 非 `/0` 的 `/22` 永远不装 exclude。  
   **必须同时改**：install 门闩（~1280）与 tick 门闩（~1944），或抽成同一谓词。

### 0.3 其它复核要点

- L1 ** alone 不够**：`/22 − 本机 /24` 仍可能留下覆盖对端物理 IP 的 `/24` → **L2 `/32` 必做**。
- `local_nets` **今天不存在**；`is_ip_in_same_network` 是 overlay VIP 网段，不能当物理 LAN。
- Windows 接口枚举仍缺 TUN 过滤；采集时必须 `exclude_ifname`（Wintun 名）。
- underlay exclude **会**收集 RFC1918（原文 §4 B.2「放行」表述反了）——已在 Current underlay 对齐，LAN 文需勘误。
- B4：改完后「仅有 proxy_cidrs、未开 enable_exit_node」的节点**不再**为带 `is_exit_node` 的包做互联网 NAT；子网代理 CIDR 命中路径保留。需 changelog。

---

## 1. 本波范围（Track A + Track B）

### Track A — 低风险 / 契约对齐（约 1–2 人日）

| ID | 内容 | 验收 |
|---|---|---|
| A1 | B17：`traffic-steering.md` + GUI/CLI help 明确「出口代价=到网关 peer，不含出口→公网目标」 | 文案可检索；无「按到目标距离选出口」歧义 |
| A2 | B3：ICMP NAT 同 key 异源不覆盖 + warn；同源可刷新 | 单测：异源冲突保留首条；同源刷新；不发无 NAT 的 socket |
| A3 | B4：三引擎 `is_exit_node` 分支要求 `enable_exit_node`（`no_tun` 本机路径豁免） | 单测：有 cidr 无 exit 标志 → Pass；开标志 → 仍代理 |

**本波不做**：B3 ident 改写 / 源 IP NAT；B2 全修；B17 协议。

### Track B — LAN 核心（约 4–6 人日）

| 层 | 内容 | 验收 |
|---|---|---|
| L0 | `is_easytier_managed_ipv4` + direct/manual 过滤 | 候选/bind 不含托管 IPv4 |
| L1 | `subtract_ipv4` + `collect_proxy_cidr_state` 剪枝 + `local_nets` 注入 | `/22−/24` 单测；`/0` 不剪；monitor 路径也剪 |
| L2 | `needs_excludes` = `/0` **或** desired 覆盖 off-link underlay IP；on-link 不装 | `/22` 场景装对端 `/32`；`/0` 回归不变 |

L4（声明卫生）→ Wave 1b；L3（绑网卡）→ 与 underlay 合并延后。

---

## 2. LAN 修正后施工设计（覆盖原文接入点）

### 2.1 L1 — 剪枝（修正版）

**算法**：同 [`lan-direct-underlay-guard.md`](./lan-direct-underlay-guard.md) §5.1（`/0` 不剪；重叠则 subtract；空则丢弃并 warn）。

**模块**：`easytier-core/src/gateway/proxy/proxy_cidr_prune.rs`（与 `cidr_monitor` 同层，便于 core 单测）。

**`local_nets` 数据流**：

```text
easytier desktop virtual_nic only
  run_proxy_cidrs_route_updater  // virtual_nic.rs 启动路径；Android/mobile 不跑
  collect_local_ipv4_networks(exclude_tun_ifname)
       │  全平台：run_in_namespace_opt（spawn_blocking → setns → current-thread runtime → 同步 show()）
       │  失败（枚举失败 / runtime 建失败 / JoinError）→ None，绝不用 expect 打死 updater
       ▼
  CorePacketPlane::set_local_ipv4_networks(...)
       // ArcSwap<Option<Vec>>；None=未注入，Some=[]=已知无物理网段
       // collect 失败 → Option::None → set 保持旧值（勿写成 Some([])）
       // 变化时 notify proxy_cidr_wake，monitor 快路径比对 Arc ptr
       │
       ▼
  cidr_monitor::collect_proxy_cidr_state
       → resolve_proxy_cidrs(...)
       → prune_proxy_cidrs(raw, holes)   // ← 唯一剪枝点（holes=物理+VIP）
       → diff / ProxyCidrsUpdated / desired_proxy_cidrs
```

**覆盖范围**：L1 物理网段注入与 L2 on-link 判定**仅桌面**生效——采集挂在 `run_proxy_cidrs_route_updater`（`virtual_nic.rs` 启动处）。Android / mobile / 其它非桌面宿主永远 `local_ipv4_networks = None`，L2 只保留「有 TUN `/0` 才装 underlay `/32`」的旧行为；**不要**写成「移动端也修了」。

**采集**：`collect_local_ipv4_networks` → `Option<Vec>`（`None`=枚举/Join 失败，`set_*` 保持旧值）；Windows **必须**按 TUN ifname 排除；`prefix < 8` 跳过（**60s** 窗内至多一条 warn）。注入：`virtual_nic` **首次 `proxy_cidr_diff` 前先采**，之后每 5s。线程跃迁在采集入口；底层 `collect_interfaces_*` 同步枚举。

**禁止**：只在 `packet_plane::collect_proxy_cidr_*` 包装层剪枝而不改 `collect_proxy_cidr_state`。

### 2.2 L2 — underlay `/32`（修正版）

统一谓词（伪代码）：

```rust
fn needs_underlay_excludes(
    effective_desired: &BTreeSet<Ipv4Cidr>, // 已剪枝
    underlay_ips: &BTreeSet<IpAddr>,
    local_nets: Option<&[Ipv4Cidr]>, // None=未知 → 仅 /0 门闩；Some=[]=全 off-link
) -> bool {
    if effective_desired.iter().any(|c| c.network_length() == 0) {
        return true;
    }
    let Some(local_nets) = local_nets else { return false };
    underlay_ips.iter().any(|ip| {
        let IpAddr::V4(v4) = ip else { return false };
        let covered = effective_desired.iter().any(|c| c.contains(v4));
        let on_link = local_nets.iter().any(|n| n.contains(v4));
        covered && !on_link
    })
}
```

接入：

1. `sync_proxy_cidrs_with_underlay_excludes`（~1280）用该谓词替代 `will_have_tun_default` 单条件。  
2. tick 门闩（~1944）用同一谓词（可保留「已安装 `/0`」以免拆除窗口漏 exclude）。

### 2.3 L0 — 候选净化

- `PeerManagerCore::is_easytier_managed_ipv4`（仿 IPv6）。  
- `direct/mod.rs` 候选生成后过滤；`should_deny_target` 拒托管目标。  
- `manual/mod.rs` `collect_bind_addrs` 过滤源 IPv4。

### 2.4 测试（最低集）

1. `subtract_ipv4` 单测：`/22−/24`、全覆盖、无交、`/0` 不剪。  
2. B3/B4 引擎单测。  
3. L2：on-link 不装、off-link 覆盖要装、条件消失拆除。  
4. 回归：仅 `/0` 时 exclude 行为与今日一致。  
5. （有环境时）事故复现：推送 `/22` → 直连稳定、`same network`≈0。

---

## 3. Track A 施工细节

### A1 B17 口径

- `docs/current/traffic-steering.md`：§2 或 Known-limit 增一句——选出口看配置顺序 / 到**网关 peer** 的路径代价，**协议不传播「出口→公网目标」最后一跳**（B17）。  
- `easytier-web/frontend-lib/src/locales/{cn,en}.yaml`：`exit_nodes_help`、`policy_routes_select_by_help` 各加一句。  
- `easytier/locales/app.yml`：`exit_nodes` help 同步。

### A2 B3

文件：`icmp_proxy_engine.rs` `handle_peer_packet` NAT insert。  
- `entry` API：occupied 且 `source_peer_id` 不同 → warn，**不**覆盖，**不** `SendToSocket`。  
- 同源 → 刷新 `started_at`（可选）并继续发送。

### A3 B4

| 文件 | 改动 |
|---|---|
| `icmp_proxy_engine.rs` | `is_exit_node && !enable_exit_node && !is_local_no_tun` → Pass（cidr 命中仍走） |
| `udp_proxy_engine.rs` | 非 cidr 的 exit 臂要求 `enable_exit_node` |
| `tcp_proxy_engine.rs` | `real_dst_ip_for_mode` 中 `is_exit_node` 与 `enable_exit_node` 与 |

changelog：行为变严，与 Current `traffic-steering.md` 对齐。

---

## 4. 施工顺序（按文档开工）

1. **A1 → A2 → A3**（快、可独立合入）  
2. **L0**（独立、低耦合）  
3. **L1 算法模块 + 单测** → 接入 `collect_proxy_cidr_state` + `local_nets` 注入  
4. **L2** 统一谓词接入两处门闩  
5. 文档收尾：本篇勾选；更新 `lan-direct-underlay-guard.md` §5 接入点；**未现网验收前不把 LAN 新行为写入 Current 可依赖口径**（Roadmap「已落地待验收」）

---

## 5. 明确不做（本波）

- B2 会话绑定全修、B3 ident/源 NAT、B10 proto、B15/B16、B17 协议扩展、LAN L3  
- 两端同前缀 LAN 自动仲裁（继续 known limitation + mapped CIDR）

---

## 6. 验收勾选（施工中勾）

- [x] A1 B17 文档/i18n  
- [x] A2 B3 冲突 warn + 单测  
- [x] A3 B4 三引擎许可门 + 单测  
- [x] L0 managed IPv4 过滤（含 IPv4 skip `debug!` 对称 IPv6）  
- [x] L1 prune 模块 + monitor 接入 + local_nets（启动先采 + set 变化 wake + Arc 快路径）  
- [x] L2 needs_excludes 双门闩 + `Option` 未知/空区分 + 单测（on-link / off-link / unknown / `/0`）  
- [x] `lan-direct-underlay-guard.md` 接入点勘误  
- [x] P0/P1 复核修复：prefix&lt;8 跳过（限频）、入口级 `spawn_blocking`、ICMP 冲突限频、manual 全删/部分剪 分文案  
- [x] fmt：`cargo fmt --all --check` 本机通过（2026-11-07 复核后）  
- [ ] lockfile `metadata --locked` + clippy `easytier-core`（**本机 toolchain 无 cargo-clippy**，须 CI 或另机；**按请求跳过** `easytier` 打包路径）  
- [ ] 现网验收：推送覆盖性 `/22` 后直连稳定（待用户机）  
- [ ] L2 拆除窗口端到端（条件消失后 exclude 拆净；单测覆盖谓词，系统拆除仍靠 tick）

---

## 7. 发版说明草稿（可进 Release notes）

**行为变更（B4）**

- 仅配置了 `proxy_cidrs`、**未**开启 `enable_exit_node` 的节点，不再为带 `exit_node` 标志的包做互联网 NAT。
- 迁移：出口机必须显式打开 `enable_exit_node`；纯子网代理节点勿再被列入对端 `exit_nodes`。
- 详见 [`../current/traffic-steering.md`](../current/traffic-steering.md) §2.1（E3）。

**LAN 直连加固（L0/L1/L2）— 预发 / RC 口径**

- 代码已合入本分支，用于缓解「覆盖性 proxy CIDR 劫持 underlay 直连」类问题。
- **范围：桌面 `virtual_nic` 路径**；非桌面宿主不注入 `local_nets`，L2 仅 `/0` 旧门闩。
- **尚未做现网 `/22` 冒烟**；发版说明应写「待现网确认 / 建议在复现拓扑验证」，**不要**写「2026-11-04 事故已永久关闭」。
- 临时运维缓解仍有效：对端物理 IP 加 `/32` 指向物理网关，或勿下发覆盖本机 LAN 的过宽 proxy CIDR。

**可观测小步（B3）**

- ICMP NAT 异源同 key 冲突时不再静默覆盖；60s 窗内至多一条 warn、其余 debug，拒绝后者；完整消歧（改写 ident / 源 NAT）未做。

---

## Changelog

- 2026-11-07：初版。代码复核：L1/L2 接入点漂移、RFC1918 表述、L1 alone 不足；锁定 Track A+B 范围与修正后设计。
- 2026-11-07：Track A + LAN L0/L1/L2 **代码已落地**（待现网验收）；剪枝 choke=`collect_proxy_cidr_state`；L2 改 install+tick 两门闩。
- 2026-11-07：增 §7 发版说明草稿；`traffic-steering.md` §2.1 记 B4/E3 升级影响。
- 2026-11-07：提交前自检：`cargo fmt --check` + `metadata --locked` 通过；clippy 曾在**另一环境**跑过 `easytier-core`（本机 1.95.0-msvc **未装** `cargo-clippy`，不可在此机复放）；**跳过** `-p easytier` 打包 clippy。
- 2026-11-07：复核 P0–P2 修复——L1 启动先采 + set 变化 wake + monitor Arc 快路径；`prefix<8` 跳过；`Option<&[Ipv4Cidr]>`；入口级 `spawn_blocking`（底层同步枚举，保 netns）；ICMP/宽掩码限频；L2 谓词单测；manual 全删/部分剪分文案；IPv4 skip debug；文档标明仅桌面。
- 2026-11-07：提交建议拆开：`feat(gateway)` L1/L2、`feat(connectivity)` L0、`fix(gateway)` B3/B4、`docs(roadmap)`、memory 勿进代码提交。
- 2026-11-07：复核 P2——采集失败区分（`Option`，`set_*` 保持旧值）、去 `expect`（`JoinError` 降级）、限频改 60s 时间窗；抽 `run_in_namespace_opt` 统一 netns 阻塞模式，`collect_local_ipv4_networks` 合并 Linux/非 Linux 分支。
