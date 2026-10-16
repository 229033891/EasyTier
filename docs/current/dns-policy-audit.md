# 域名策略（DnsConfig）审查：逻辑核对 · BUG · 冲突（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-16（P0/P1 + 残余 R1–R4 已落地；新增残余 R5（未修，可接受）；C1 前端已修、C4 前端部分落地；O1–O9 仍开放）
- 范围：`DnsConfig` 配置模型、MagicDNS 进程内权威 + 上游转发、静态 hosts 分发（托管 + mesh 通道）、与 `enable_magic_dns` / `policy_routes.domains` / 前端编辑器的交互
- 现状 SoT：[`magic-dns.md`](./magic-dns.md)（本文只列**从代码结构可确证的缺陷与冲突**，不复述正常工作流）
- 剩余规划项：[`../roadmap/dns-policy.md`](../roadmap/dns-policy.md)
- 域名路由侧：[`traffic-steering.md`](./traffic-steering.md) §5
- 索引：[`../README.md`](../README.md)

> 本文为**静态核查**（逐行读代码 + 定位调用点），**未做现网复现**。
> 已落地项以单测为验收；标注「疑似」的条目为逻辑推断。

---

## 1. 结论摘要

**功能链路是完整可用的**（配置 → Catalog 权威 → 分发 → 热重载 → OS 接线）。2026-10-16 已修校验分层与 route/static 时序缺口中的 P0/P1；体验类冲突 C1（编辑器零反馈）前端已修、C4 前端已加联动提示（设备侧观测仍开放）；§5 优化项仍开放。

### 1.1 缺陷（BUG）

| # | 缺陷 | 状态 | 修复要点 |
|---|------|------|----------|
| **BUG-1** | route zone 到达时静默覆盖已装静态 hosts | **已修（待联调）** | `update()` 末尾 `reapply_static_hosts()`；`update_dns_record` 先 routes 后 hosts |
| **BUG-2** | 一条非法 `upstream_dns` / `forwarders.servers` 拖垮 MagicDNS | **已修（待联调）** | 逐条 skip；分流按行 skip；主上游全坏回落系统 DNS；步骤硬失败不推进 last |
| **BUG-3** | 精确 hosts 吞掉同名 Split 且 `applied_split_zones` 漂移 | **已修（待联调）** | warn + `forget_split_zone`；保留 hosts 赢 |

### 1.2 冲突（与其它功能 / 开关）

| # | 冲突 | 状态 |
|---|------|------|
| **C1** | `enable_magic_dns` 关闭时编辑器仍可编辑保存，零反馈 | **已修**（前端） |
| **C2** | `policy_routes.domains` 预检误拒通配 + Split | **已修（待联调）**（预检仅保留 TLD） |
| **C3** | mesh hosts 覆盖本机 `dns_config.hosts` | **已修（待联调）**（local 最后合并） |
| **C4** | 域名路由观测依赖 OS 接线；与编辑器无联动 | **前端部分落地** |

> C1/C4 均为**前端**修复：C1 在 `Config.vue` 关掉 MagicDNS（或 `no_tun`）时灰化 hosts / forwarders / upstream 三块编辑器并内联提示；C4 复用 `dnsCoverage.ts::expectedDnsCoverage()`，在 `PolicyRoutesEditor` 按 `enable_magic_dns` / `no_tun` 内联提示「域名规则学不到 IP」。设备侧观测联动（B6 phase 2 `magic_dns_os_wired`）未做。

### 1.3 优化项

见 §5（仍开放）。

---

## 2. 现状逻辑核对（结论：与文档一致）

### 2.1 代码地图

| 层 | 文件 | 关键符号 |
|----|------|----------|
| 配置模型 | `easytier-core/src/config/dns.rs` | `DnsHostEntry`、`HostZoneTarget`、`classify_host_name`、`DnsConfig` |
| 运行时服务 | `easytier/src/instance/dns_server/server_instance.rs` | `update`、`reload_dns_policy`、`reapply_static_hosts`、`static_host_skip_reason`、`update_dns_record` |
| Catalog / 转发 | `.../dns_server/server.rs` | `parse_upstream_nameserver`、`build_forward_config_from_servers`、`forget_split_zone`、`reload_*` |
| 域名路由预检 | `easytier-core/src/peers/route/local_route_table.rs` | `validate_domain_rules_against_dns`（仅 TLD） |
| 前端 | `easytier-web/frontend-lib/src/...` | `dnsCoverage.ts`、`dnsHostValidate.ts`（O2 镜像，已接线）、`Config.vue` DNS 编辑器、`policy/PolicyRoutesEditor.vue` |

### 2.2 生效条件（**关键前提**）

```
tun 路径存在（no_tun=false）
  └─ flags.accept_dns = enable_magic_dns
        └─ MagicDnsRuntime::start(...) → DnsRunner → MagicDnsServerInstance::new
```

**结论**：`enable_magic_dns=false` 或 `no_tun=true` → **`dns_config` 全部字段无任何效果**。这是 C1 的根因。

### 2.3 本地解析优先级

1. 静态 hosts（本机优先于 mesh 通道）
2. MagicDNS route zone
3. Split forwarder
4. 根 forwarder（`upstream_dns`，空 → 系统 DNS）

### 2.4 静态 hosts 装法与守卫

每条 hosts 装成 Catalog 权威 zone；装前过 `static_host_skip_reason()`。`update()` 之后重跑 `reapply_static_hosts()`，与状态 RPC 共用守卫。

---

## 3. 已修缺陷（摘要）

### BUG-1（已修）route zone 后到 → reapply

`update()` 重建 route 权威后调用 `reapply_static_hosts()`；同请求内先写 routes 再写 static hosts。状态与 Catalog 在 reapply 后与守卫一致（冲突条目 skip，不会长期「报了却没装」）。

### BUG-2（已修）非法上游逐条 skip + R1/R2

`build_forward_config_from_servers` 对解析失败条目 warn+continue。分流循环按行 skip。启动期 `upstream_dns` 全坏 → 回落系统 DNS；热重载全坏 → **保留上一份**根上游。`reload_dns_policy` 任一步硬失败 → `Err` 不推进 `last`；同一失败配置只打一次 error。

### BUG-3（已修）精确 hosts 赢 Split + 账本对齐

精确条目覆盖同名 Split 时 `warn!`，并 `forget_split_zone`。热重载 `retain_zones` 按本轮**精确** hosts（+ route/TLD）推导——通配不保留，与「通配 → Split 赢」及启动期一致。

### 残余 R4（已修）冲突 warn 仅跨 client

同 `(zone, rr)` 覆盖时仅当写入方 client 不同才 warn。

### 残余 R5（未修，可接受）mesh 通道移除 hosts 后 Split 暂回落根转发器

精确 hosts 覆盖同名 Split（BUG-3 `forget_split_zone`）后，若这些 hosts 由 **mesh 静态通道**移除（client 断连 / 上游下发空 hosts），走的是 `set_static_hosts_for_client → reapply_static_hosts` 直删路径，**不经** `reload_split_forwarders`：stale-prune 会把该 zone 的（此时已是 hosts 的）权威删掉，而 Split 账本为空 → 不会立即重装 Split，该 zone 落回**根转发器**，直到下一次 `dns_config` 变更触发 `reload_split_forwarders` 重装（R2 重试只针对 reload 硬失败，不覆盖本场景）。

- **同轮 reload 路径不受影响**：`test_reload_hosts_to_split_takes_over_zone` 钉住「同一轮 hosts 清空 + Split 已配 → Split 直接接管」。
- 为什么可接受：无承诺自动恢复（见上文 BUG-3 修复语义＝「hosts 赢 + 账本对齐」，不含移除后的重建）；窗口内行为是「回物理 DNS」而非黑名单/失败解析。
- 缓解方向（如要做）：最小修法是在 `reapply_static_hosts` stale-prune 时对被移除 zone 惰性重装同名 Split（需要读到原始 `DnsConfig`，目前只有 reload 侧持有完整 splits 列表）。

---

## 4. 冲突现状（C1 前端已修 / C4 前端部分落地）

### C1 前端已修：关闭 MagicDNS（或 `no_tun`）时灰化编辑器

`Config.vue`：`dnsPolicyEditorsDisabled = !enable_magic_dns || no_tun` → hosts / forwarders / upstream 三块编辑器包 `fieldset[disabled]` + `pointer-events: none`（整块 `opacity: 0.55`），并在「启用魔法 DNS」开关下方内联提示 `dns.magic_dns_off_hint`。配置**仍可保存进网络**，只是不接线；标题/帮助 tooltip 不受影响。

### C4 前端部分落地：policy 编辑器内联观测提示

`PolicyRoutesEditor.vue` 新增 `dnsCoverage` prop；`Config.vue` 用 `expectedDnsCoverage({ enable_magic_dns, no_tun, os_type: null })` 复算，含 `domains` 规则且非 `covered` 时内联提示（`policy_routes_dns_observe_{off,unsupported,version,manual}`）。网络表单不知道设备 OS/版本 → 已开 MagicDNS 时恒为 `manual_required`，提示常显。**设备侧**观测联动（B6 phase 2 `magic_dns_os_wired`）仍开放。

---

## 5. 优化项（仍开放）

| # | 项 | 说明 |
|---|----|------|
| O1 | 观测仅解析 A 记录 | AAAA-only / 仅 CNAME → 学不到 |
| O2 | 控制台无「被跳过」回显 | **已修**（前端）：`dnsHostValidate.ts` 镜像已接进 hosts / forwarders / upstream 三编辑器，逐行内联 warn；见 §7 |
| O3 | `tld_dns_zone` 无 flag 校验 | 通配值使 TLD 守卫失效或启动失败 |
| O4 | `update()` 全量重建 | 可改差量 |
| O5 | 1s 轮询深比较整个 `DnsConfig` | 建议 watch / revision |
| O6 | stale client key 残留 | 异常断连未清 |
| O7 | `StaticDnsHost` vs `DnsHostEntry` 双建模 | 长期漂移风险 |
| O8 | 版本门静默跳过 | `<2.7.4` 无设备侧 warning |
| O9 | TTL 语义分散 | CLI / TOML / 前端三处规则 |

---

## 6. 剩余优先级

| 优先级 | 项 |
|--------|----|
| **P2** | C4（设备侧观测联动） |
| **P3** | O1、O3–O9 |
| 不排期 | R5（可接受，见 §3；含候选修法） |

---

## 7. 复核用锚点

| 验证目标 | 测试 |
|----------|------|
| 非法上游混排 | `server.rs`：`build_forward_config_skips_invalid_entries_keeps_valid` |
| route 后到 | `tests.rs`：`test_late_route_zone_skips_prior_static_hosts` |
| 精确撞 Split | `tests.rs`：`test_exact_hosts_override_split_zone_bookkeeping` |
| 本机 vs 远程 | `tests.rs`：`test_local_static_hosts_win_over_remote_client` |
| 域名预检允许通配+Split | `local_route_table.rs`：`domain_precheck_allows_wildcard_over_split_zone` |
| hosts→Split 同名接管（R3 回归） | `tests.rs`：`test_reload_hosts_to_split_takes_over_zone` |
| 通配 vs Split（Split 赢） | `tests.rs`：`test_reload_wildcard_hosts_split_still_wins` |
| 分流全坏按行 skip | `server.rs`：`try_new_skips_all_invalid_split_keeps_sibling` |
| 前端：关闭 MagicDNS 灰化 | `Config.vue`：`dnsPolicyEditorsDisabled`（fieldset + 内联提示） |
| 前端：观测内联提示 | `PolicyRoutesEditor.vue`：`showDnsObserveHint` + `policy_routes_dns_observe_*` |
| 前端：skip 判定镜像（O2 已接线） | 三编辑器：`hostNameSkipReason` / `invalidHostIps` / `upstreamInvalid` / `invalidServers` / `invalidDomains` / `forwarderRowSkipped`；用例 `tests/dns-skip-hints.spec.ts`（11） |
