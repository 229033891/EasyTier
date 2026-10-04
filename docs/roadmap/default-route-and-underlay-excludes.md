# 默认路由与 Underlay 排除路由（方案讨论）

## Status

- Status: **Roadmap**（Phase 1 / 1b 桌面+移动 D+ 已落地，见 [`../current/traffic-steering.md`](../current/traffic-steering.md)；本文保留方案对比与剩余缺口）
- 日期：2026-10-04
- 最近审阅：2026-10-04
- 状态说明：桌面期望集合过滤 + 逃生阀 + exclude 统一门控已落地；Android / OHOS / GUI 移动端同步过滤对端 `/0`；桌面 DNS 绑定物理默认网卡。Phase 2（/1+/1 覆盖）仍待按需。
- 目标读者：产品决策 + 路由/出口实现
- 索引：[`../README.md`](../README.md)
- **现状行为**：[`../current/traffic-steering.md`](../current/traffic-steering.md)（已按 D+ 更新）
- **相关规划**：[`traffic-steering-vNext.md`](./traffic-steering-vNext.md)
- 决策原则：**更简单、更好性能、更稳定**；不引入「看着有默认路由、实际不抓流量」的半吊子全隧道。

---

## 1. 问题

围绕本机 TUN 系统路由，讨论三个相关问题：

1. 管理面 / P2P / STUN 相关 IP 的 **underlay 主机排除路由**，是否应始终创建？
2. 对端宣告的 **高 metric `0.0.0.0/0`**，是否有必要装进 OS？
3. 与 WireGuard / OpenVPN 的全隧道做法相比，EasyTier 应采用哪条路径？

---

## 2. 现状（代码今天做什么）

实现要点见 Current：[`traffic-steering.md`](../current/traffic-steering.md) §3。摘要：

| 条件 | TUN `0.0.0.0/0` | Underlay 排除主机路由 |
|------|-----------------|------------------------|
| 本机 `exit_nodes` 可解析且有下一跳（`local_exit_default`），未 `manual_routes` / `no_tun` | **会装**，低 metric（抢物理默认） | **会装**（peer 隧道远端、STUN 公网 IP、config-server 等） |
| 仅有具体 proxy CIDR（无本机出口默认） | 不装 `/0` | **不装**；若曾装过会清空 |
| 对端通告 `0.0.0.0/0`，本机 **未** 开出口 | **可能装**，高 metric（Windows 路由度量约 9000，显示常为 9000+接口度量） | **不装** |

顺序（exit 开启时）：先装排除路由 → 再装 TUN `/0`；卸默认时先卸 TUN `/0` → 再卸排除。

高 metric `/0` 的意图：避免未开出口时抢走物理默认网关。在常见有线/Wi‑Fi 默认 metric（几十）下，高 metric TUN `/0` **几乎不会被选中**，对抓主机流量基本无效。

---

## 3. Underlay 排除：始终创建 vs 门控

### 3.1 排除路由的唯一目的

TUN 上存在**会生效的**默认路由时，避免 peer / STUN / 管理面流量被吸进 TUN 形成环路或断连。

无 TUN 生效默认路由时，这些目的地址本就走物理默认路由，主机排除路由是冗余的。

### 3.2 对比

| 方案 | 简单 | 性能 | 稳定 | 结论 |
|------|------|------|------|------|
| **仅 `local_exit_default` 时创建**（现状） | 好 | 好（无 exit 可跳过收集/安装） | 好 | **保持** |
| **始终创建** | 控制流略简 | 差（无 exit 也常改路由表） | 差（换网时易钉在旧网关） | **否决** |

Always-on 唯一明显收益：exit 刚打开时排除路由可能已在，缩短竞态窗口。现状已用「先 exclude、后 `/0`」保证顺序，不必用 always-on 换性能与换网稳定性。

---

## 4. 高 metric `0.0.0.0/0` 是否必要

### 4.1 选路事实

同前缀比 metric。物理 `/0`（metric 通常远小于 9000）始终优先于高 metric TUN `/0`。因此：

- **不能**实现「未开 exit 时的软全隧道」；
- 只增加路由表噪音，以及 metric 翻转路径（`reconcile_ipv4_default_metric`）的复杂度。

### 4.2 狭窄例外

- 本机完全没有物理默认路由时，高 metric `/0` 可能兜底；
- exit 开关切换时略方便「改 metric」——但当前实现仍是删了再加，收益可忽略。

### 4.3 结论

**对端宣告的 `/0` 不必写入 OS。**  
L3 仍可按现状用对端 `/0` 作选路兜底（见 Current §2）；L2 只在本机 exit 时安装**有效**默认路由。

### 4.4 代码核验修正（2026-10-04）

§4.2 那句「收益可忽略」被低估了：高度量 `/0` **确实会在一种真实场景下生效**，因此本方案不是「无行为变化」。

| 待修正表述 | 核验结果 |
|-----------|---------|
| §4.1「不能实现未开 exit 时的软全隧道」 | 正确，但结论应表述为：物理默认**存在且度量更优**时它赢不过；**本机物理默认缺失、或物理度量劣于高度量（Windows 9000 / Linux 65535 / Darwin 7）时，它就是唯一默认路由并真实接管本机流量** |
| §4.2「狭窄例外」 | 该例外即上表场景，且是**本机发起**流量唯一受影响处；对端 `/0` 的其余消费者全部走内存路由表，不受 OS 路由影响（见 §7.3） |
| §4.2「切换时略方便改 metric」 | 不准确。度量翻转（`reconcile_ipv4_default_metric`）存在的原因正是**对端 `/0` 与本机 exit `/0` 共享同一个 `BTreeSet` 元素**，exit 状态翻转时必须重写同一条路由的度量；删掉对端 `/0` 的安装后该机制才成为死代码 |
| §5 表格「Underlay / 服务端」行 | 补充：EasyTier 只写**路由度量**，**从不设置接口度量**（全仓库无 `InterfaceMetric` / `UseAutomaticMetric` / `netsh`），Windows 显示值为「路由度量 + 系统决定的接口度量」，「高度量」是相对优先级而非硬保证 |
| §6「A. 现状：两套 `/0`」 | 补充：IPv6 侧并不存在两套——对端宣告的 `::/0` **目前不写入任何 OS 路由**（`::/0` 只随本机出口安装），IPv4/IPv6 在 L2 上本就不对称 |

---

## 5. 与 WireGuard / OpenVPN 对比

| | OpenVPN | WireGuard (`wg-quick`) | EasyTier（现状） |
|--|---------|------------------------|------------------|
| 全隧道开关 | `redirect-gateway`（常加 `def1`） | `AllowedIPs` 含 `0.0.0.0/0` | exit → 低 metric `/0`；对端 `/0` → 高 metric `/0` |
| 压过物理默认 | LPM：`0.0.0.0/1` + `128.0.0.0/1`，或替换默认 | 同左，或策略路由 | metric（TUN `/0` ≈ 1） |
| Underlay / 服务端 | 先给 `--remote` 装主机路由再 redirect | Endpoint 必须仍走物理 | exit 时 exclude 集合（多 peer + STUN + 管理面） |
| 非全隧道 | 只加具体 route | `AllowedIPs` 只写需要前缀 | 具体 proxy CIDR；另有「无效」高 metric `/0` |
| 语义 | 开=抓全部，关=不装默认 | 开=抓全部，关=不装默认 | exit=真全隧道；高 metric `/0`≈摆设（**见 §4.4 修正**：在无物理默认路由的宿主上会真实生效） |

要点：

- **真全隧道**：WG/OpenVPN 更干净——要么装会生效的默认（或 `/1`+`/1`），要么不装；不会保留「故意无效」的默认路由。
- **分裂隧道**：WG 的 AllowedIPs 模型与 EasyTier「只装具体 proxy CIDR」一致。
- **产品形态**：EasyTier 是 mesh + 可选 exit + 子网代理，不是「一连接就整网 VPN」。应对齐：**有 exit 才装有效默认 + exclude；无 exit 不装 `/0`。**

`/1`+`/1`（OpenVPN `def1`）更抗物理口 metric 异常，但实现更重（双路由、IPv6 等价、Windows/Android/VpnService 各测）。适合作为**第二阶段加固**，不是第一步。

---

## 6. 方案对比（简单 / 性能 / 稳定）

| 方案 | 简单 | 性能 | 稳定 | 评价 |
|------|------|------|------|------|
| **A. 现状** | 差（两套 `/0`、metric 切换） | 中 | 中 | 高 metric `/0` 增加复杂度却几乎不干活（**见 §4.4**） |
| **B. 始终 exclude** | 中 | 差 | 差 | 否决 |
| **C. exit 立刻改 `/1`+`/1`** | 差 | 略差 | 最好 | 第二阶段可选（抗物理 metric 异常，但双路由+全平台测试成本高；仅现场出现抢不过时再做） |
| **D+. 去掉高 metric `/0`，期望集合过滤+显式逃生（改良版 D）** | **最好** | **最好** | **最好** | **推荐采纳**（D 本体代价见 §7.3（2）（3）；D+ 用期望集合过滤+`allow_peer_default_without_exit`逃生+分平台落地收敛代价，且第 1 步与第 3 项必须同进同退） |

---

## 7. 推荐目标模型（方案 D+：改良版 D）

```
无 exit（!local_exit_default）
  → 只装具体 proxy CIDR
  → 不装任何 0.0.0.0/0（含对端宣告的 /0；期望集合层过滤，见 §7.3（1））
  → 不装 underlay 排除路由（过渡期：若 TUN 上已有/将有 /0 仍装排除做兜底，见 §7.3（4））
  → 例外逃生：`allow_peer_default_without_exit=true` 时才允许旧高 metric 兜底（默认 false）

有可解析 exit（local_exit_default）
  → 先装 underlay 主机排除（peer / STUN / 管理面，含私网/ULA）
  → 再装一条会生效的 TUN 默认路由（低 metric /0，或日后 /1+/1）
  → 出口掉线：先卸 TUN 默认，再卸排除
```

### 7.1 为何同时满足三点

- **更简单**：L2 一句话——只有 `local_exit_default` 才装默认路由；可删除对端 `/0`→OS、以及默认路由 metric 翻转逻辑。
- **更好性能**：无 exit 时零 exclude 收集/安装；少一条无用默认路由与相关系统调用。
- **更稳定**：消除假全隧道；真全隧道仍保持「exclude → 默认」顺序；避免 always-on exclude 换网钉死旧网关。

### 7.2 明确不建议

1. 始终创建 underlay 静态路由。  
2. 保留高 metric `0.0.0.0/0`「占位」。  
3. 未观察 metric 竞态前，全面改为 OpenVPN `def1`（牺牲简单与性能）。

### 7.3 落地前必须确认的五件事（2026-10-04 代码核验）

**（1）过滤点必须做在「期望集合」，不能只做在 `apply_route_changes`。**  
1 秒 reconciler 用 `added = 期望集合 − 已安装集合` 重试。若 `/0` 仍留在期望集合里、只是 `apply_route_changes` 拒绝安装，则它永远进不了已安装集合 → **每 tick 重试一次且永不早退**。  
→ 必须在 `resolve_proxy_cidrs` / `collect_proxy_cidr_state` 层过滤。这样做之后 §8 Phase 1 第 3 项（删 `reconcile_ipv4_default_metric` 与 `specific_route_metric` 的两处显式覆盖）才成立，两者是同一个不变量。

**（2）平台面必须一并决策：桌面端不装 ≠ 全平台不装。**

| 平台 | 实际安装方 | 对端通告 `/0` 的现状 | 若采纳方案 D+ |
|------|-----------|---------------------|-------------|
| Windows / Linux / macOS（非 NE）/ FreeBSD | 本进程 `apply_route_changes` | 高度量安装，通常不生效 | Phase 1 做：期望集合过滤，不再安装 |
| Android（GUI / Web、`easytier-android-jni`） | `VpnService.Builder.addRoute`，由 GUI/Kotlin 汇总**所有 peer 的 `proxy_cidrs`** | **直接成为默认路由**（无度量概念、不经 exit 门控） | Phase 1b 稍后做：见 §7.3（2.1），不阻塞桌面 |
| OHOS | `aggregate_tun_routes` 汇总所有 peer 的 `proxy_cidrs` + `simplify_routes` | 任一对端宣告 `/0` 时**更具体前缀被折叠进 `0.0.0.0/0`** | Phase 1b 稍后做：见 §7.3（2.1），不阻塞桌面 |
| iOS / macOS NE | 宿主 App / NE provider 的 `includedRoutes` | 本仓库不安装、无法约束 | 本仓库内无法强制该不变式，验收表需标注为平台外 |

参照先例：对端宣告的 **IPv6 proxy CIDR 目前在任何平台都不安装**，所以「不装对端 `/0`」在代码中已有同族先例，不是全新范式。

**（2.1）Android / OHOS 实现位置（2026-10-04 已核验，实现稍后）：**

- Android JNI：`easytier-contrib/easytier-android-jni/kotlin/.../EasyTierManager.kt:151` 全量收集 `routes.*.proxy_cidrs` → `EasyTierVpnService.t.kt:67 builder.addRoute`，无 exit 门控、无 metric；示例 `README.md:245 addRoute("0.0.0.0",0)` 即全隧道。
- GUI 移动端：`easytier-gui/src/composables/mobile_vpn.ts:293 getRoutesForVpn` 全量 `proxy_cidrs + node_config.routes + MagicDNS 100.100.100.101/32`，同样无 exit 过滤。
- OHOS：`easytier-contrib/easytier-ohrs/crates/easytier-ohos-core/src/routing.rs:56 aggregate_tun_routes`（`virtual_cidr + manual + 全 peer proxy`）→ `simplify_routes:23` 折叠，调用方 `easytier-contrib/easytier-ohrs/src/kernel_bridge/socket_server.rs:499`。
- 保护模型不同：移动端无 host exclude 路由，靠 `need_protect → VpnService.protect / VpnConnection.protect`（见 `docs/current/socket-protection.md`），fail-closed，只保序不保实时。
- 稍后实现清单：① 聚合函数加 `local_exit_default` 入参，无 exit 则过滤对端 `/0` 后再 `simplify`；② 保留 `manual_routes` 与 MagicDNS `/32`；③ 补单测「任一对端 `/0` + 无 exit → 不出 `/0`」「有 exit → 出 `/0`」；④ GUI 文案按平台区分提示。

**（3）测试面：现状零覆盖，D 落地前需先补。**

- 没有任何测试断言写进 OS 的 `/0` 或其度量；`apply_route_changes` / `reconcile_ipv4_default_metric` / `reconcile_underlay_exclude_routes` / `find_ipv4_physical_default` 均无测试。
- 现有 `peer_ospf_route_tests` 中 `default_proxy_cidr_is_excluded_from_specific_lookup`、`default_proxy_remains_available_for_service_paths` 只钉住**内存**语义，**必须保留**（证明 L3/服务路径仍能解析对端 `/0`）。
- 需要**新增**：① 「无 exit + 对端 `/0` → 期望集合不含 `/0`」；② 「无物理默认路由」场景的显式验收，否则该行为失效时无人察觉。

**（4）换网 / 私网排除 / 门控（2026-10-04 过渡实现已落地，D+ 收尾待做）：**  
- `reconcile_underlay_exclude_routes` 每轮重新发现物理默认；网关/ifindex 变化时先卸旧 via 上的主机路由再换下一跳。  
- `is_excludable_*` 允许 RFC1918 / ULA 作为 underlay 排除目标（仍过滤环回/链路本地/组播及 overlay VIP）。  
- 过渡门控为「本机 exit **或** TUN 上已有/将有 `/0`」时安装（`will_have_tun_default`），防止迁移期对端 `/0` 残留时无排除保护；期望集合过滤上线、确认无残留后收窄为纯 `local_exit_default` 门控。  
- 管理面：`WebClient` 启动时 eager 解析 config-server，缩小装 `/0` 前的 DNS 竞态（见下条剩余缺口）。

**（5）桌面 DNS protect（2026-10-04 已落地 Phase-1）：**  
Android 可用 `VpnService.protect`；桌面端改为：TUN 就绪时登记 ifname，`RuntimeDnsResolver` 的 hickory UDP/TCP 套接字绑定到**物理默认网卡**（`SO_BINDTODEVICE` / `IP_BOUND_IF` / `IP_UNICAST_IF`），config-server underlay 解析走同一 `DnsResolver` 钩子。系统 `lookup_host` 仅在无 TUN / 无物理默认时使用。完整「平台 socket protect API」仍非必需；换网后依赖短 TTL 刷新物理 ifname。

### 7.4 D+ 相对 D 的增量（本次修订）

- L3 不动：对端 `/0` 仍作内存选路兜底，只删 L2 安装，不伤子网代理 fallback。
- 无物理默认兜底不再静默删除：默认关闭，`allow_peer_default_without_exit`（默认 `false`）显式 opt-in，并在 RPC/状态页展示「期望 vs 已安装 vs 最后错误」。
- 分平台落地：桌面先行；Android / OHOS / iOS-NE 按 §7.3（2）另行决策，不阻塞桌面。
- C（`/1`+`/1`）明确为 Phase 2：仅当现场出现物理 metric 异常导致低 metric TUN `/0` 抢不过时再做。

---

## 8. 落地阶段

### Phase 1（D+ 桌面端）— **已落地**

1. **L2**：在期望集合层（`resolve_proxy_cidrs`）过滤对端 `0.0.0.0/0`，**不再**写入 OS（除非逃生阀）。  
2. **仅** 期望集合含 TUN `/0` 时安装 underlay exclude（统一门控 `will_have_tun_default`）。  
3. 非 exit `/0` 的高度量路径仅服务逃生阀；无逃生阀时 metric 翻转为 no-op。有逃生阀时 `reconcile_ipv4_default_metric` **仍是活代码**（按旧/新 metric 精确删除后再装，避免 `already_satisfied` 误收敛）。  
4. **L3**：保持对端 `/0` 作选路兜底。  
5. 逃生+可观测：`allow_peer_default_without_exit`（默认 `false`）+ `proxy_cidr_route_sync` 状态摘要。  
6. 已更新 [`../current/traffic-steering.md`](../current/traffic-steering.md)。

### Phase 1b（D+ 移动端）— **已落地**

- Android JNI + GUI `getRoutesForVpn`：无 exit 过滤对端 `/0`，有 exit 显式加 `/0`，保留 `manual_routes` 与 MagicDNS。
- OHOS `aggregate_tun_routes`：同上；单测覆盖折叠进 `/0`。

### Phase 2（按需）

若现场出现「物理默认 metric 异常导致低 metric TUN `/0` 抢不过」：

- exit 默认路由改为 `0.0.0.0/1` + `128.0.0.0/1`（及 IPv6 等价覆盖），不依赖 metric；
- 仍保持 exclude 仅 exit 门控。

### 验收（Phase 1）

| 场景 | 期望 |
|------|------|
| 可解析 `exit_nodes`，未 `manual_routes` / `no_tun` | TUN 有生效默认路由；有 underlay 排除；公网经出口 |
| 无 exit，对端通告 `10.0.0.0/24` | 仅有该 CIDR；**无** TUN `/0`；**无** exclude |
| 无 exit，对端通告 `0.0.0.0/0` | OS **无** TUN 默认路由；L3 仍可按兜底选路（若业务需要） |
| 清空 / 出口不可达 | 卸默认与排除；具体 CIDR 保留 |
| 开 exit 瞬间 | 排除已装齐（或同轮先于 `/0`），P2P/管理面不断 |

> 补充（2026-10-04 核验）：上表第 3 行改为「桌面不装」还需补一条**反向验收**——「本机无物理默认路由 + 对端通告 `/0`」时今天会真实接管本机流量，方案 D+ 用 `allow_peer_default_without_exit` 显式承接该行为（默认关闭，需确认可接受，或提示用户改用 `manual_routes`）。另：移动端（Android / OHOS）与 iOS / macOS NE 的路由不经过本套逻辑，验收表需按 §7.3（2）的平台表分别标注。

---

## 9. 一句话

最好的「简单 + 性能 + 稳定」不是做成完整 OpenVPN，而是对齐 WireGuard 的 AllowedIPs 思维：

> **需要全隧道才装会生效的默认路由；不需要就不装；排除路由只服务这条真默认路由。**
