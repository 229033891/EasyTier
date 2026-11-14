# underlay 排除路由：同网段过滤 + 清理按已安装集合

## Status

- Status: **In progress**（2026-11-14 现网**两台**机器复现；风险定级与工期拆分见 §8；**F1 + F2 代码已落地，待自测与现网验收** —— 未过 §6 验收前不进 `docs/current/`）
- 最近审阅：2026-11-14（晚间代码级复核补记见 §8.5）
- 复核补记：§8.5 —— via 快照、abort 发布粒度、存量残留一次性清理、定级细化；§4 / §5 / §6 已同步
- 代码锚点口径：§3/§4 的 `文件:行号` 是 2026-11-14 快照，**以函数名为准**，动手前用 `rg -n "<函数名>"` 复核行号是否漂移
- 背景：`shdx`（ImmortalWrt 路由器，子网代理 `172.16.0.0/16`）出现 `192.168.10.101/32 via 192.168.10.1`——同网段对端被钉到公司网关；ET 停止后路由残留。
- 现场口径：[`../ops/openwrt-node-runbook.md`](../ops/openwrt-node-runbook.md) §4
- 相关 Roadmap：[`lan-direct-underlay-guard.md`](./lan-direct-underlay-guard.md)（L2 下垫面钉扎，本文为其缺口）、[`default-route-and-underlay-excludes.md`](./default-route-and-underlay-excludes.md)、[`routing-tech-debt.md`](./routing-tech-debt.md)
- 相关 Current：[`../current/underlay.md`](../current/underlay.md)、[`../current/traffic-steering.md`](../current/traffic-steering.md)
- 索引：[`../README.md`](../README.md)

---

## 1. 问题陈述

underlay 排除机制的语义是：**TUN 装上了有效默认路由（或以 proxy CIDR 覆盖了某个对端的下垫面地址）时，把这些下垫面目的地钉一条更具体的 `/32`，让隧道自身的流量绕过 TUN。**

但当前实现有两个缺口：

- **G1（准确性）**：门闩 `needs_underlay_excludes()` 只是**布尔**，on-link 判断只用于决定「装不装」，一旦为真就把**整份**排除集合装下去 —— 连**与本地同一网段、本就有 on-link 路由**的地址也被钉到物理默认网关。
- **G2（清理）**：安装与清理都按「**当前**采集到的集合」推导目标条目，对端断开 / 实例停止后集合为空 → 残留不删。

## 2. 现网证据（2026-11-14，ImmortalWrt 25.12.1）

`shdx` 代理 `172.16.0.0/16`，路由表中同时存在：

```
172.18.10.10/32   via 192.168.10.1 dev eth0     # 门闩触发源（off-link，落在 172.16/16 内）
172.18.10.200/32  via 192.168.10.1 dev eth0     # 同上
192.168.10.101/32 via 192.168.10.1 dev eth0     # ← 与 shdx 同属 192.168.10.0/24，不应钉
```

`/32` 比 `/24` 直连路由更具体，且 metric=1 ⇒ 无条件优先，回给同网段对端的包永久绕行网关。
`/etc/init.d/easytier stop` 后 `192.168.10.101/32` **仍在**（G2）。

### 2.2 第二现场（2026-11-14，飞牛 NAS `FNNAS`，**Docker 卸载后**）

第一例是「进程停掉后残留」，本例是**更硬的证据：容器、镜像、二进制全部卸载干净之后，路由仍然在**。

| 项 | 值 |
|---|---|
| 主机 / 内核 | 飞牛 fnOS，`6.18.18.c1186-trim`（`FNNAS` = 192.168.10.105） |
| 网卡 | `ens192` = 192.168.10.105、`ens256` = 192.168.10.106（**同一网段双网卡**） |
| 曾用形态 | Docker 节点：`/vol1/1000/docker/et`（飞牛项目容器名 `fnnas_et`，`host` 网络 + `privileged`） |
| ET 版本 | **未取到**（核对时容器已不存在，`docker exec fnnas_et ET-core -V` 失败）；仅知 compose 目录残留 |

卸载后逐项核对（**全部在主机的 `admin` 用户 + `sudo` 下执行**）：

| 检查 | 结果 | 判读 |
|---|---|---|
| `docker ps -a` / `docker image ls` | 0 容器、0 镜像 | 本体已卸净 |
| `systemctl` / `/etc/init.d` / `/opt` / `/usr/local/bin` | 无 easytier / `ET-*` | 无裸机安装 |
| `ip link` | 无 `et*` / `tun*` | 接口已回收 |
| `ip route show`（main 表） | **8 条** `/32 via 192.168.10.1 dev ens256 proto static metric 1` | ⚠️ G2 残留 |

残留集合（原样记录）：

```
43.154.13.211   # 控制台 et.scmec.vip（config-server 公网）
101.43.96.62    # 公网 peer
124.222.29.94   # 公网 peer
172.25.0.1      # 某 peer 的 underlay
192.168.10.96   # 同网段（当时的运维入口机）
192.168.10.101  # 同网段
192.168.10.201  # 同网段
192.168.10.254  # 同网段
```

**来源排查（三条排除法，结论确定）：**

1. `grep -rnE '<上述公网 IP>' /etc`、`grep -rn` 遍 `/etc/network`、`/etc/netplan`、`/etc/systemd/network`（含 `metric 1`）→ **全部无命中** ⇒ 无持久化配置来源，是纯运行时残留。
2. `ip rule` 的 `10:`（`from 192.168.10.106 lookup 10`）与 `11:`（`from 192.168.10.105 lookup 11`）、以及 `table 10/11` 里的 `192.168.10.0/24 dev ens192|ens256 proto static scope link metric 1` ⇒ **不是 ET 产物**：全仓库只有 `RT_TABLE_MAIN` 引用，ET 从不建 `ip rule` / 自定义表。属飞牛自身的双网卡策略路由，**清理时不得触碰**。
3. 代码侧字节级对账：`virtual_nic.rs:1710` `add_ipv4_host_route(v4, via, Some(1))` → `netlink.rs:578-596`（`.destination(/32)` + `.oif(物理网卡)` + `.priority(1)` + `.table(MAIN)` + `.static_protocol()` + `.gateway(物理默认网关)`）⇒ 与现场 `ip route show` 输出**逐字段一致**。

**⚠️ 关键推论（影响修复优先级）：** 残留里**一半是公网 IP**（含 config-server `43.154.13.211`）。这些地址"本就该被排除"、与 on-link 无关 —— **F1 对它们完全无效，只有 F2 能清掉**。所以 G1 / G2 不是"主次关系"，而是**两个独立缺陷面**：

| 残留条目 | G1 能防住？ | F2 能清掉？ |
|---|---|---|
| 同网段 `192.168.10.96/.101/.201/.254` | ✅（F1 后压根不装） | ✅ |
| 公网 `43.154.13.211` / `101.43.96.62` / `124.222.29.94` / `172.25.0.1` | ❌（off-link，本就该装） | ✅ |

清理步骤与止损口径见 [`../ops/docker-compose-deploy.md`](../ops/docker-compose-deploy.md) §2.7。

## 3. 代码链（已逐行确认）

| 环节 | 位置 | 现状 |
|---|---|---|
| 采集 | `easytier-core/src/gateway/proxy/underlay_exclude.rs:16` `collect_underlay_exclude_ips()` | 收 peer 隧道 `resolved_remote_addr` + STUN 公网 IP + config-server IP；`should_exclude_ip()`（`:69`）只排 overlay VIP / managed IPv6 |
| 门闩 | `easytier-core/src/instance/packet_plane.rs:244` `needs_underlay_excludes()` | 纯谓词（有单测）：`desired` 含 `/0`，或某 underlay IP 被 desired 覆盖且不在 `local_ipv4_networks` 内 |
| 调用 | `easytier/src/instance/virtual_nic.rs:1281-1285`（install）、`:1966-1969`（tick） | `needs_excludes` 为真 → 传**全量** `exclude_desired` 给安装器 |
| 安装 | `easytier/src/instance/virtual_nic.rs:1558` `reconcile_underlay_exclude_routes()`；`:1667` `to_add = desired.difference(installed)` | `add_ipv4_host_route(ip, via=PhysicalDefaultRoute, metric=1)`，无 on-link 过滤 |
| 清理 | `easytier/src/instance/runtime_host/tun_common.rs:211` `cleanup_underlay_exclude_routes()` | `let desired = packet_plane.underlay_exclude_ips().await; if desired.is_empty() { return; }` → 对端已断时什么都不删 |

**判断**：G1 是「门闩粒度 = 集合级、安装粒度 = 条目级」的错配；G2 是「目标集合来源不一致（当前采集 vs 历史已安装）」。

## 4. 修复设计

### F1（G1，必做）：采集阶段过滤 on-link

在 `collect_underlay_exclude_ips()` 里对每个候选 IP 追加 on-link 判断，来源与 `needs_underlay_excludes` 同源（`PeerManagerCore::local_ipv4_networks()`）：

- 命中本机物理网段 ⇒ **不进入集合**（该地址本来就有更具体/等长的直连路由，钉 `/32` 纯属劣化）；
- `local_ipv4_networks()` 为 `None`（未注入）⇒ 保守放行（保持现行为），避免非桌面宿主全员跳过 exclude 而破坏 exit-node 场景；
- 覆盖范围已核：`set_local_ipv4_networks()` 只有 updater 两处调用（`virtual_nic.rs:1851` 启动、`:1961` 每 5 s 刷新），`no_tun` 时 `prepare` 跳过 `start_static_ip`、根本不装 exclude，不构成缺口；枚举失败时 `set_local_ipv4_networks(None)` 保留旧快照（`peer_manager.rs:1911`）；
- **只覆盖 IPv4**：`local_ipv4_networks()` 是 IPv4 网段，IPv6 on-link 不在 F1 范围内（留给 F3）。

理由：F2 之后清理已改为按 `installed` 记账推导（不再按当前采集集合），但 `underlay_exclude_ips()` 这一份快照仍同时供**门闩 `needs_underlay_excludes()`、安装 `desired`、状态上报**使用。在采集侧过滤一次，三处口径天然一致：on-link 条目既不会被装，也不会被记账或干扰门闩判断。若只在 `to_add` 侧过滤，`desired` 与门闩仍含 on-link 条目，安装 / 诊断口径分叉（旧设计的清理按当前采集推导时更会直接残留）。

### F2（G2，必做）：清理按「已安装集合」+ 安装时 via 快照

`cleanup_underlay_exclude_routes()` 改为使用**本实例实际装过的集合**（`exclude_installed`，与 `reconcile` 共用状态），而不是重新采集当前对端：

- 对端已断 / 集合为空时仍能卸载；
- 保持「只删自己装的条目」这一既有安全性约束不变（条目来自 `installed`，本就不会误删用户路由）；
- **via 必须随 installed 一起持久化安装时的 `PhysicalDefaultRoute`**（`ifindex` / `ifname` / `gateway`），**不要在 cleanup 时重新解析**：停止时网络可能已断，`find_ipv4_physical_default()` 返回 `None` → `continue` 跳过删除；Windows `remove_ipv4_host_route()`（`windows.rs:421`）还要 `ifindex + gateway`，用新值删不掉旧条目；netlink 用 `ifname`、macOS 用 `gateway/ifname`。`cleanup_tun_leftovers()`（`tun_common.rs:134`）已全程持有 netns guard，无需另加；
- **状态位置**：`exclude_installed` 现为 updater 任务局部变量（`virtual_nic.rs:1839`），`cleanup` 只拿到 `packet_plane`。照 `proxy_cidr_route_sync` 的 `ArcSwap` 范式存 **IP 集合**没问题，但 `PhysicalDefaultRoute` 定义在 `easytier` crate、`CorePacketPlane` 在 `easytier-core`（core 不能反向依赖 host）⇒ **via 快照需 core 侧镜像结构，或把状态挂到 `GlobalCtx`**（cleanup 已持有 `global_ctx`）；
- **发布粒度（stop 时序）**：`tun_desktop.rs:210-214` 先 `nic.stop()` 再 `cleanup`，而 `NicCtx::shutdown()`（`virtual_nic.rs:1070`）是 `tasks.abort_all()` + `join_next()` ⇒ **不存在 updater 与 cleanup 并行竞争**；真正风险是 **abort 丢状态**：`reconcile` 内全是 `.await`，abort 可能落在「路由已 add、状态未发布」之间。共享状态须在**每次 add/remove 成功后立即发布**，不能在 `reconcile` 末尾一次性发布（否则整批变更丢失）；
- **备选实现（更彻底，评审时二选一）**：改为「**先发布意图、后安装**」——条目先写入共享状态（含 via 快照）再调 `add_*_host_route`，安装失败则回退移除该意图条目。remove 侧本就容忍 `route_remove_already_satisfied`（清理时多删一条实际不存在的条目无害），因此 abort 无论落在「已发布未安装」还是「已安装未发布」都**不丢状态、不漏删除**，连「最后一条」的窗口也关掉；代价是安装失败瞬间共享状态与内核可能短暂不一致（自愈于下一次 reconcile tick）。默认仍按上条「成功后立即发布」实现；
- 退路：若状态随实例析构丢失，可持久化到 `ProxyCidrRouteSyncStatus` 一类状态里，与 `proxy_cidr_route_sync` 同生命周期。

### F3（可选，defer）：安装时二次校验

`add_ipv4_host_route` 前若目标 IP 已存在 on-link 路由（`ip route get` / 本地前缀匹配），跳过并记 debug 日志。**本批 defer**：DHCP 换址已被 5 s 刷新覆盖（除非枚举失败保留旧快照），F3 真正兜的是 IPv6 on-link 与 `local_nets` 长期不可用，价值不足以进首个修复窗口。

### 4.4 升级迁移（随 F1/F2 上线，必做）：存量残留一次性清理

**F1 + F2 都不会自动清掉现网已有的 `/32` 残留**：

- F1 之后 on-link 地址连 `desired` 都进不去，新进程 `exclude_installed` 从空开始，F2 也不会记录它们；
- 只有**再次进入 desired** 的 IP 才有机会因 `route_add_already_satisfied`（EEXIST）被 `installed.insert()` 重新纳管（`virtual_nic.rs:1723`），随后才可能被清掉；
- 因此 §2.2 的 8 条（尤其 4 条同网段）在升级后仍会长期存在。

⇒ 上线包必须附一次性清理：按 [`../ops/docker-compose-deploy.md`](../ops/docker-compose-deploy.md) §2.7 第 3 步逐条 `ip route del`（判定三条件、留档，`ip rule` / `table 10|11` 不动）。**不做**「开机全表扫 `/32` 自动删」——会误删用户路由，违背「只删自己装的」约束。

## 5. 测试

| 层级 | 用例 |
|---|---|
| 纯函数 | `collect` 过滤：`local_nets = Some([192.168.10.0/24])` + 候选 `192.168.10.101` ⇒ 不进入集合；候选 `172.18.10.10` ⇒ 进入 |
| 纯函数 | `local_nets = None` ⇒ 全部进入（保持现行为） |
| 安装/卸载 | `reconcile`：门闩打开、集合含 1 个 on-link + 1 个 off-link ⇒ 只装 off-link 一条 |
| 卸载 | 对端集合为空时 `cleanup` 仍能删掉 `exclude_installed` 中的条目（G2 回归） |
| 卸载（**已落地单测**：`tun_common::tests`） | 全删成功 ⇒ 返回状态 `installed` 为空，且 mock 记录到删除调用（`all_removed_yields_empty_state`） |
| 卸载（**已落地单测**） | 某条删除失败 ⇒ 该条**保留记账**、via 快照保留，成功条目移除（`failed_removal_stays_accounted`；「失败即永久丢账」G2 复发路径回归） |
| 卸载（**已落地单测**） | 无 via 快照 ⇒ 条目保留记账、不调用删除（`missing_via_snapshot_stays_accounted`） |
| 卸载（**已落地单测**） | `NotFound` / already-gone ⇒ 视为已删除、不保留（`already_gone_counts_as_removed`） |
| 卸载 | `desired` 为空、`installed` 非空且物理默认已变 / 解析不到 ⇒ 仍用安装时 via 快照删掉（via 快照回归，**仍待自动化覆盖**） |
| 安装/卸载 | via 切换：旧条目用旧 via 删、新条目用新 via 装（exit-node `/0` 回归） |
| 状态 | `reconcile` 中途 abort ⇒ 已成功的 add/remove 均已发布到共享状态（发布粒度回归；若评审选 §4 备选「先发布意图」，断言改为 abort 后共享状态与内核一致、或对不存在条目的 remove 可容错） |
| 端到端（手测） | 路由器（`172.16/16` 代理）→ 对端同网段：`ip route get <对端物理 IP>` 应命中 `dev eth0`、无 `via`；ET stop 后无 `/32` 残留 |

> E2E 断言不要用 `ip route show | grep 'metric 1'`：exit `/0` 默认路由同为「via 网关 + metric 1」（`ifcfg/mod.rs:257` `default_route_metric() = 1`），会误报。用 Ops 手册 §1 第 ② 项（**已收紧为 `/32|/128 via <物理网关>`**：ET 的 exclude 只装 host route，`/0` 永远不是 exclude）。

## 6. 验收

- 复现现场：ImmortalWrt 路由器开启 `172.16.0.0/16` 代理 + 局域网内有一个 ET 对端（同网段）+ 一个落在 `172.16/16` 内的 underlay 对端 ⇒ 路由表**只应**出现后者，前者不得被钉；
- `stop` / 网络禁用后，Ops 手册 §1 第 ② 项（已收紧为 `/32|/128 via <物理网关>`）命令的输出应为空；
- **存量清理**：`FNNAS` / `shdx` 上线前按 [`docker-compose-deploy.md`](../ops/docker-compose-deploy.md) §2.7 清掉已存在的 `/32`；上线后重启 ET 再 stop，确认不再新增残留；
- 不破坏 exit-node 场景：TUN `/0` 时公网 underlay 目的地仍走物理默认（回归 `docs/current/underlay.md` 的既有断言）；
- 未过上述验收前只更新本文与 Ops 的状态，**禁止写入 `docs/current/`**（[`doc-update-after-task.md`](../ops/doc-update-after-task.md) §1）。

## 7. 不做

- 不动 `needs_underlay_excludes()` 的返回值语义（仍为布尔门闩）——把「是否需要」和「给谁装」混在一个谓词里会让 tick 路径的正交性更差；
- 不引入新配置开关（这是纯粹的「装错东西」缺陷，用户无需开关）。

---

## 8. 风险评估与排期（2026-11-14）

### 8.1 定级：**中高（P1「慢性事故」）** —— 不阻断发布，但会反复制造难定位工单

| 维度 | 评级 | 依据 |
|---|---|---|
| 严重度 S | 中（S2） | 后果是同网段 ET 对端互访绕行网关 / 时通时不通，以及网关变更后的黑洞；**无数据损坏、无越权**（收紧口径见 §8.5.1）；止损只需删路由或重启 |
| 触发概率 P | **中高** | 需①子网代理或 exit-node `/0` ②局域网内有 ET 对端 ③存在"被 proxy CIDR 覆盖且 off-link"的下垫面 IP。**现网两台机器（`shdx` 路由器、`FNNAS` NAS）全部命中** |
| 可发现性 D | 差 | 安装成功即静默，**无日志无指标**；现象是"慢/偶发不通"，且 ICMP 不走被钉路径（`ping` 正常）→ 不能用 ping 证伪 |
| 持续期 | **长（G2 特有）** | 进程停止、**甚至卸载**后仍在（§2.2）→ 故障寿命长于软件本身，且会误导后续排查 |
| 扩散面 | 局部 | 只污染本机路由表 → 到 ET 对端 / 控制台的路径；不传染局域网其它主机 |

> **复核注（详见 §8.5）**：上表按缺口拆开读更准 —— G1 的 P 才是「中高」（需①…③），**G2 的 P 应为「高」**：只要装过任意一条 exclude 且停止时集合为空即残留，不需要同网段对端，exit-node `/0` 更是必装。「无安全影响」建议收紧为「无数据损坏、无越权」：这些 IP 本就是 underlay 端点、运行期本就走物理，只有后来被复用为 overlay 目的地时才构成策略口径破坏；常态后果是绕行网关与网关变更后的黑洞。「局部」在网关节点会放大到下游转发流量。D 可借 F2 的共享状态顺手改善（暴露 installed 集合 / 一行日志）。收紧口径已同步进基表与「为什么不是 P0」行。

**为什么不是 P0：** 不阻断上线、不丢数据、无越权面（被钉 IP 本就是 underlay 端点，运行期本就走物理，见 §8.5.1）；单机 `ip route del` 即恢复。
**为什么不降到 P2：** 无任何可观测信号 + 残留跨卸载存活 ⇒ 每次"网络莫名变慢"都要重新怀疑一遍，排查成本本身即是风险（`openwrt-node-runbook.md` 全篇就是这么来的）。

### 8.2 工期分解（一人、熟悉本模块）

| 工作项 | 触点 | 改动量 | 工时 |
|---|---|---|---|
| **F1** 采集侧 on-link 过滤 | `underlay_exclude.rs:16` / `should_exclude_ip():69`；来源现成：`peer_manager.local_ipv4_networks()`（`virtual_nic.rs:1851`、`:1961` 每 5 s 刷新注入，**headless Linux 同样生效**，NAS / 路由器吃得上） | ~30–40 行 + 单测（抽纯谓词） | 2–3 h |
| F1 回归核对 | 门闩 `packet_plane.rs:244` 语义不动；核 exit-node `/0` 与 `reconcile` 的 `v4_changed` 重装路径 | — | 1–2 h |
| **F2** 清理按已安装集合 | `exclude_installed` 现为 updater 任务**局部变量**（`virtual_nic.rs:1839`），而 `cleanup_underlay_exclude_routes()`（`runtime_host/tun_common.rs:211`）只拿到 `packet_plane` ⇒ 需照 `proxy_cidr_route_sync` 的 `ArcSwap` 范式新增平面状态 + 上报/读取 API + 接线；**via 快照有跨 crate 类型约束**（见 §4 / §8.5） | ~70–90 行 + 单测 | **5–7 h** |
| F2 cleanup 改造 | 用 `installed` 替代 `desired`；**改为持久化安装时 via 快照**，不在停止时重新解析；netns guard 已现成 | ~30–40 行 | **2–3 h** |
| F3（可选，建议 defer） | 装前二次校验 on-link（真正兜的是 IPv6 on-link / `local_nets` 长期不可用，非 DHCP 换址） | ~15 行 | 2–3 h |
| 自测 / E2E | 本文 §5 的 8 条：纯函数 ×2、安装/卸载 ×4、手测（`shdx` 代理 `172.16/16` + 同网段对端 + `172.16/16` 内对端；`stop` 后按 §1 ②（`/32|/128 via <gw>`）过滤应为空，**不要用 `grep 'metric 1'`**）。现网设备齐备但 `shdx` 上 stop 需维护窗口 | — | 3–4 h |
| CI 门禁 | `cargo fmt --all -- --check`、`cargo metadata --locked`、`script\easytier-test.cmd fast`（本机冷缓存 clippy 单次可 20 min+）；新失败按 `ci-failure-log-presubmit` 追加 [`ci-failure-log.md`](../ops/ci-failure-log.md) §5 | — | 2–4 h（多为等待） |
| 文档收尾 | 本文勾选 + `openwrt-node-runbook.md` §4.3 标注"已修（版本 x）"（**待验收后**）+ `docs/README.md` 索引状态行；**未验收不得进 `current/`**；行为改动与 `docs(roadmap)` 拆分提交 | — | 1 h |
| **一次性清理（§4.4，必做）** | 存量 `/32` 不会被 F1/F2 自动清掉，按 [`docker-compose-deploy.md`](../ops/docker-compose-deploy.md) §2.7 第 3 步执行 | — | **+0.5 天** |

**合计：**
- G1+F2 最小闭环：**≈1.5–2 天**（不含 E2E 等待）；
- 含 E2E + CI + 文档 + 一次性清理：**≈2–2.5 天**（顺）/ **3–3.5 天**（含一次返工余量）；
- 叠加 F3：**+0.5 天**（不建议，defer 理由见 §4 / §8.5）。

**置信度：中高。** 不确定性仍在 F2，但**风险点已换**（§8.5）：`exclude_installed` 从任务局部变量提升为平面共享状态，会碰到 `ArcSwap` + netns guard 的既有范式与 `PhysicalDefaultRoute` 的跨 crate 类型约束；`nic.stop()` 是 `abort_all()` + `join_next()`，**不存在 updater 与 cleanup 的并行竞争**，真正的坑是 **reconcile 中途 abort 丢状态** —— 状态须逐条发布。0.5 天缓冲留着，但花在发布粒度与 via 快照上，不是花在加锁上。

### 8.3 落地顺序（2026-11-14 实际同批落地；如需回滚可按 F2 / F1 拆两个 commit）

> 实际执行时 F1+F2 同批落地（改动面交集大：共享状态两侧都要动），下表保留为**拆分粒度**参考；PR 边界可视 review 情况再切。

| PR | 内容 | 理由 |
|---|---|---|
| **PR-1** | F2（清理按已安装集合） | 先止血：直击 §2.2 这类"卸了还在"的现场（含公网条目）；自包含、回归面小 |
| **PR-2** | F1（采集侧 on-link 过滤） | 再修根因：同网段条目压根不进集合 |
| 后续 | F3 / 清理路径改"按物理前缀反查" | 挂 [`routing-tech-debt.md`](./routing-tech-debt.md)，不阻塞 |

### 8.4 现场与实现状态（截至 2026-11-14，未验收）

**代码（已落地，未验收）：**

- **F2**：`CorePacketPlane` 新增 `UnderlayExcludeState`（已安装集合 + 安装时 via 快照，`ArcSwap` 逐条发布）；`reconcile_underlay_exclude_routes()` 每次成功 add/remove / via 切换后发布；`cleanup_underlay_exclude_routes()`（`tun_common.rs`）改用发布状态卸载并复位为空 —— 对端已断 / 集合清空时照样能删，via 不再停止时重新解析；**清理核心抽为 `remove_installed_underlay_excludes()`（状态进 → 剩余状态出）**：删除失败 / 无 via 快照的条目**保留记账**（warn + 回写状态），只有全成功才清空，杜绝「失败即永久丢账」（G2 复发路径）；
- **F1**：`collect_underlay_exclude_ips()` 采集阶段按 `local_ipv4_networks()` 丢弃 on-link IPv4（`None` 未注入 ⇒ 保守放行；IPv6 不在 F1 范围，留给 F3）。
- 自测状态：`cargo check -p easytier-core -p easytier -p easytier-web` ✅（21:18，当时带 `unused import: Ipv4Addr` warning，**已修、待复验**）、`cargo fmt --all -- --check` ✅、`cargo metadata --locked` ✅；单测：core `underlay` 过滤 10 条 ✅，`tun_common::tests` 新增 4 条（全删清账 / 失败保留 / 无 via 保留 / NotFound 容错，见 §5）**待实跑**（复核会话受本机 MSVC 环境 / 沙箱限制：`reg.exe` 被拦截、C 依赖 build script 失败，非代码问题）；恒真占位用例 `report_then_read_roundtrips` 已删除。clippy `-D warnings` 与 E2E（§5 + §6）**未跑完** —— 未过验收前保持本文状态，**不得写入 `docs/current/`**。

**运维现场（待执行）：**

- `FNNAS` 上 ET **本体已卸载**（容器 / 镜像 / 二进制 / 接口全无）；
- **8 条 `/32` 残留路由与 `/vol1/1000/docker/et` 目录尚未删除**（命令见 [`../ops/docker-compose-deploy.md`](../ops/docker-compose-deploy.md) §2.7；新装包上线前也必须先清存量，见 §4.4）；
- **未做**：重启自证、控制台侧孤儿设备清理、`shdx` 维护窗口内的 stop 断言。

---

### 8.5 复核补记（2026-11-14 晚，代码级复核）

逐行复核 `underlay_exclude.rs` / `packet_plane.rs` / `virtual_nic.rs` / `tun_common.rs` / `tun_desktop.rs` / `ifcfg`（netlink、windows、darwin）后，对 §8.1–§8.2 做以下修正；§4 / §5 / §6 已同步落地。

#### 8.5.1 定级细化

| 项 | 修正 |
|---|---|
| 严重度 | S2 成立，但「无安全影响」宜收紧为「无数据损坏、无越权」。被钉的 IP 本就是 underlay 端点、运行期本就走物理，「绕过隧道」只在后来被复用为 overlay 目的地时才构成策略口径破坏；**常态后果是绕行网关 / 时通时不通，以及网关变更后的黑洞** |
| 触发概率 | **拆开**：G1 中高（需子网代理或 `/0` + 局域网对端 + 被 CIDR 覆盖的 off-link IP）；**G2 高**（只要装过任意一条 exclude 且停止时集合为空即残留，不需要同网段对端，exit-node `/0` 必装）。§2.2 的 4 条公网残留即 G2 证据 |
| 可发现性 | D「差」成立：`report_proxy_cidr_route_sync` 只记 proxy CIDR、不记 exclude。F2 的共享状态可顺手暴露成状态 / 一行日志，几乎零成本把 D 抬到「中」 |
| 扩散面 | 「局部」在网关节点会放大：`shdx` 作为 LAN 网关时陈旧 `/32` 影响所有经它转发的客户端；Windows wintun 持久化适配器同理长寿命 |

#### 8.5.2 三处实现纠正（直接影响工时）

1. **via 必须持久化，不能保留「重新解析」**。`cleanup_underlay_exclude_routes()` 现在重新调 `find_ipv4_physical_default()`；停止时网络已断 → `None` → `continue` 跳过删除。Windows 删除还要 `ifindex + gateway`（`windows.rs:421`），用新值删不掉旧条目；netlink 用 `ifname`、macOS 用 `gateway/ifname`。⇒ 安装时 `PhysicalDefaultRoute` 随 `installed` 一起存。netns guard 已在 `tun_common.rs:134` 全程持有，无需另加。
2. **跨 crate 类型约束**。`PhysicalDefaultRoute` 定义在 `easytier` crate，`CorePacketPlane` 在 `easytier-core`，core 不能反向依赖 host。照 `ArcSwap` 范式存 IP 集合没问题，存 via 需要 **core 侧镜像结构**（`ifindex` / `ifname` / `gateway` 纯类型）或**把状态挂到 `GlobalCtx`**（cleanup 已持有 `global_ctx`）。发布点建议放进 `reconcile` 内部（1 处），4 个调用点只加参数。
3. **stop 时序不是「竞争」，是「abort 丢状态」**。`tun_desktop.rs:210-214` 先 `nic.stop()` 再 `cleanup`；`NicCtx::shutdown()`（`virtual_nic.rs:1070`）= `tasks.abort_all()` + `join_next()` ⇒ cleanup 时 updater 已死并 join 完，**无并行竞争**。但 `reconcile` 内全是 `.await`，abort 可能落在「路由已 add、状态未发布」之间；若末尾一次性发布，整批变更全丢。⇒ **每次 add/remove 成功后立即发布**（默认）；更彻底的备选「先发布意图、后安装」可关掉最后一条的丢失窗口，见 §4 F2 备选实现，评审时二选一。§8.2 的 0.5 天缓冲保留，改花在这里。

#### 8.5.3 升级不会清掉存量残留（新增必做项）

F1 后 on-link 地址连 `desired` 都进不去，新进程 `installed` 从空开始；只有再次进入 `desired` 的 IP 才会因 EEXIST 被 `installed.insert()` 重新纳管（`virtual_nic.rs:1723`，`route_add_already_satisfied`）。⇒ §2.2 的 8 条（尤其 4 条同网段）升级后仍长期存在。**上线包必须附一次性清理**（§4.4），+0.5 天；不做开机全表扫描自动删。此前「重启 ET 不自愈」的绝对说法同样修正为「仅对再次进入 desired 的 IP 有机会纳管」。

#### 8.5.4 其它口径

- F1 的 `local_nets=None` 保守放行已核实：`no_tun` 节点压根不装 exclude（`prepare` 跳过 `start_static_ip`），不构成缺口；DHCP 换址由 5 s 刷新覆盖，F3 真正兜的是 IPv6 on-link 与枚举长期失败 —— **defer 结论不变，理由换掉**。
- E2E 断言统一为 §1 ② 的 `/32|/128 via <物理网关>` 过滤（已同步进 §5/§6 与两个 Ops 手册命令）；`grep 'metric 1'` 会把 exit-node `/0` 默认路由（同为 metric 1 + via 网关）误报成残留。
- 提交门禁补齐：`cargo fmt --all -- --check`、`cargo metadata --locked`、新失败追加 [`ci-failure-log.md`](../ops/ci-failure-log.md) §5；**行为改动与 `docs(roadmap)` 拆分提交**；「已修（版本 x）」须待验收后再标，未验收不进 `docs/current/`（[`doc-update-after-task.md`](../ops/doc-update-after-task.md) §1）。
- 工期采纳 §8.2 修订值：**1.5–2 人日实现+自测 / 2–3 天含 E2E + CI + 文档 + 一次性清理**；置信度中高。

**优先级结论不变**：F1 + F2 同批（若切两个 PR，F2 先合也成立 —— 覆盖面更广、直击「卸载不干净」），F3 defer，排下个修复窗口第一梯队。
