# MagicDNS 与 DNS 策略（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-16（续修 R1–R4：分流按行 skip、主上游全坏回落系统 DNS、热重载硬失败可重试、retain zone 不装 Split、跨 client 才 warn；前端 **C1 编辑器灰化**（§3 控制台段）与 **C4 观测内联提示**；此前：route 后到 reapply、上游逐条 skip、hosts/Split 账本、本机优先、域名预检）；2026-10-13（**域名路由（`[[policy_routes]]`）已落地**，其 DNS 侧观测与泄漏面见 [`traffic-steering.md`](./traffic-steering.md) §5.1；`domain-proxy` 旧草案已被取代）；2026-10-09（静态 hosts 支持 `*.suffix` 通配；**修正**通配匹配语义为 RFC 4592 多级；**补齐守卫**：单标签 zone 拒绝、route zone 冲突对精确条目也生效，见 §3）
- 范围：进程内 MagicDNS、`DnsConfig`（hosts / Split DNS / 上游）、OS 接线与控制台覆盖信号
- 手工接线（OpenWrt / 非 systemd）：[`magic-dns-manual-wiring.md`](./magic-dns-manual-wiring.md)
- 剩余缺口 / 决策留档：[`../roadmap/dns-policy.md`](../roadmap/dns-policy.md)
- **缺陷与冲突审查（2026-10-16，3 BUG / 4 冲突 / 9 优化）**：[`dns-policy-audit.md`](./dns-policy-audit.md)
- 相关：桌面 underlay DNS [`traffic-steering.md`](./traffic-steering.md) §3；**域名路由（`[[policy_routes]]` domains，已落地）** 见 [`traffic-steering.md`](./traffic-steering.md) §5（旧草案 [`../roadmap/domain-proxy.md`](../roadmap/domain-proxy.md) 已被统一路由模型取代）
- 索引：[`../README.md`](../README.md)

本文只描述 **代码今天做什么**。产品帮助文案应以本文为准。

---

## 1. 是什么

MagicDNS = 节点进程内权威 DNS +（可选）把本机查询指到 fake IP，用于：

1. **路由 hostname → A**（mesh 内节点名，TTL 约 1s）
2. **静态 hosts**（`DnsConfig.hosts`，默认 TTL 300s）
3. **Split DNS**（后缀 → 指定上游）
4. **默认可配上游**（未配则回落系统 DNS）

开关：`enable_magic_dns` / `--accept-dns` → `flags.accept_dns`。OFF = 不发布、不解析、不改本机 DNS（发布与解析首期仍耦合）。

**Fake IP（硬编码）**：`10.255.255.254`（`MAGIC_DNS_FAKE_IP`）。RFC1918 私网地址；避开 Tailscale `100.100.100.100`；**不要**改成公网段（如 `6.6.6.6`），也不要再随意切换（全网 OS DNS/`/32` 重写）。

---

## 2. 本地解析优先级（固定）

1. 静态 hosts（`DnsConfig.hosts` / MagicDNS `static_hosts` 通道）  
2. MagicDNS 权威 zone（路由 hostname）  
3. Split forwarder（`DnsConfig.forwarders`，Catalog 最长后缀匹配）  
4. 默认上游（`DnsConfig.upstream_dns`；空则系统 DNS）

冲突：hosts 与路由 hostname 同名时 **hosts 赢**。空上游 / 无匹配 split → **沿用系统 DNS**（禁止静默 NXDOMAIN）。

---

## 3. 配置模型

`NetworkConfig.dns_config`（proto field **74**）→ `ConfigLoader` / TOML `[dns_config]` / CLI：

| 字段 | 作用 |
|------|------|
| `hosts` | `DnsHostEntry{name, ips[]}` 静态清单；`name` 支持精确主机，或 `*.suffix` 通配（**语法**只允许最左单标签 `*`，且父域 ≥2 标签；装成 Catalog 父 zone + `*.suffix.` A） |
| `forwarders` | `DnsForwarder{domains[], servers[]}` Split DNS（后缀匹配转发，不是 `*` 通配语法） |
| `upstream_dns` | 根 zone 上游列表；空 = 系统 DNS |

CLI 示例：`--dns-host 'app.internal.=10.1.2.3'`、`--dns-host '*.corp.example=10.1.2.3'` / `--dns-forward` / `--dns-upstream`。控制台：`frontend-lib` 的 `DnsHostsEditor` / `DnsForwardersEditor` / upstream 列表。

**静态 hosts 通配规则**（2026-10-09）：

- **语法**：只允许最左单标签 `*.parent`；`a.*.b` / `*foo` / `*.` / 空一律拒绝。**该条目拥有的 Catalog zone（精确名自身，或通配的父域）必须 ≥2 个标签**：`*.com` / `com` 这类单标签 zone 被拒绝（`MIN_STATIC_HOST_ZONE_LABELS = 2`）——否则会装成 `com.` 权威 zone，劫持该节点**全部** `.com` 解析。
- **匹配（RFC 4592，多级）**：`*.corp.example` 除了命中 `x.corp.example`，**也命中** `a.b.corp.example` —— hickory 的通配查找会逐级剥标签后重试（`hickory-server` 的 `inner_lookup_wildcard`）。想「只覆盖一级」必须改实现，改文档表述没用；父域越短影响面越大（这也是上面要求 ≥2 标签的原因）。
- 同一父 zone 可同时放精确条目与通配（`corp.example` 的 apex A + `*.corp.example` 的 A）；apex 本身**不**被通配覆盖，需要显式配 `corp.example`。
- **守卫**（`server_instance.rs::static_host_skip_reason()`，apply 路径与状态 RPC 共用同一判定，故「状态页报了但实际没装」不会发生）：
  - 单标签 zone（`*.com` / `com`）→ 拒绝（上一条，语法/策略层）。
  - 静态条目的 zone 与任一 **route zone 同名** → 跳过，**精确与通配都查**。`update()` 在重建 route 权威后会再跑 `reapply_static_hosts()`，因此 **route 后到** 也会把已装冲突条目卸掉并 warn，而不是静默留在 Catalog。
  - 静态条目的 zone 等于 MagicDNS 路由 TLD（默认 `et.net.`）→ 跳过。该 zone 在 `new()` 里早于任何路由装好，可能不在 route zone 快照里，故单独判一次。
  - **通配**的父 zone 已被 Split forwarder 占用 → 跳过（Split 赢）。**精确**条目与同名 Split 冲突时 **hosts 赢**（warn，并从 `applied_split_zones` 去掉该 zone）。精确子域 hosts（如 `app.et.net.`）仍允许覆盖同名路由 hostname（B2）。
  - mesh 通道与本机 `dns_config.hosts` 同 `(zone, rr)` 冲突时 **本机优先**（warn）。
- **三条入口的校验分层**（有意为之，判定共用 `HostZoneTarget::reject_reason()` / `DnsConfig::invalid_hosts()`，不会漂）：`--dns-host` **硬失败**（报错并终止启动）；TOML `[dns_config].hosts` 在**加载时** warn 一次（带文件名）并在运行时再 warn 跳过；控制台 / 托管下发的条目只运行时 warn 跳过 —— 远端 **hosts** 配置永远不能把节点弄起不来。`upstream_dns` / `forwarders.servers` 非法条目 **逐条 warn + skip**；某一分流的 servers **全部**无效 → 跳过该分流行（其它分流照常装）；启动期 `upstream_dns` **全部**无效 → **回落系统 DNS**（不拖垮 `Server::new`）；**热重载**时全坏则 **保留上一份根上游** + warn（避免静默绕过企业解析器）。热重载 `retain_zones` 按本轮**精确** hosts 的 zone + route/TLD 推导（通配不保留，Split 赢；勿用上一轮 `applied_static_zones`）。任一步硬失败不推进配置快照并重试（同一失败配置只打一次 error）。

**控制台编辑器常显**（2026-10-08 起）：进入配置页即由 `syncNormalizedNetwork` 补一个空 `dns_config`，DNS 编辑器始终可见，不再需要先点「+配置策略」——即「有 `dns_config`」不代表用户配过策略，别把它当成「已配置」信号。高级设置的「功能开关」分组标题也一并去掉（纯视觉）。

**关闭 MagicDNS / 无 TUN 时编辑器灰化**（2026-10-16 起，C1）：`enable_magic_dns=false` 或无 TUN 时，hosts / forwarders / upstream 三块编辑器整体灰化（`fieldset[disabled]` + `pointer-events: none`；标题与帮助 tooltip 不受影响）并显示「开启后才接线生效」的内联提示 —— 配置**仍可保存在网络里**，只是不接线。`policy_routes` 编辑器另按 `expectedDnsCoverage()`（`enable_magic_dns` / `no_tun`）在含 `domains` 规则时内联提示「域名规则学不到 IP」（C4）；网络表单不知道设备 OS/版本，故已开 MagicDNS 时该调用恒为 `manual_required`，提示会常显。

上游格式：**仅 IP 字面量**（如 `1.1.1.1`、`udp://8.8.8.8:53`）；DoT/DoH 与 hostname 上游未做。

---

## 4. 分发路径（R1）

| 路径 | 覆盖 | 说明 |
|------|------|------|
| Managed config | 纳管设备 | `DnsConfig.hosts` 随 web-owned Full/Patch 下发；OFF 实例仍可先存着，启用即生效 |
| MagicDNS 静态通道 | 已开 MagicDNS 的 mesh 节点 | `UpdateDnsRecord` + `client="static-hosts"` → `static-hosts:<tunnel>`；断连清理；TTL 长于路由 1s；与本机 `dns_config.hosts` 同 RR 冲突时**本机优先** |

`< 2.7.4` 客户端：prost 静默丢未知字段；控制台标 `version_too_old` 并跳过 dns patch。旧节点抢到 `:49813` server 时静态 hosts 可能不可见（控制台 warning，未改选举协议）。

---

## 5. 上游查询策略

`ForwardAuthority` 使用：

- `ServerOrderingStrategy::UserProvidedOrder`
- `num_concurrent_reqs = 1`（**禁止**多上游竞速）
- `timeout = 2s`，`attempts = 2`

依据：异构上游下 NXDOMAIN 终局、GeoDNS 抖动、企业过滤合规。转发套接字经 `magic_dns_forward_connector()` 绑物理默认网卡（与 `RuntimeDnsResolver` / TUN exclude 同族），避免上游指 VPN 内地址时进 TUN 成环。与 fake_ip 同址的上游会被排除。

---

## 6. 热重载

`MagicDnsServerInstance` 轮询 `get_dns_config()` → `reload_dns_policy`（hosts / split / 根 forwarder Catalog upsert，**不**重建 UDP 监听）。`InstanceConfigPatch.dns_config` 从 managed `hot_patch_base` 剥离，避免仅改 DNS 触发整实例 overwrite。客户端 heartbeat 检测 hosts 变更并走静态通道。

---

## 7. OS 接线矩阵

| 平台 | 自动接线 | 说明 |
|------|----------|------|
| Windows | ✅ | 接口 NameServer + SearchList |
| macOS（非 NE） | ✅ | `DarwinConfigurator`（scutil） |
| Linux + systemd-resolved | ✅ | drop-in `easytier-magic-dns.conf`：`DNS=10.255.255.254` + `Domains=~et.net` |
| Linux / OpenWrt 非 systemd | 手工 | 见 [`magic-dns-manual-wiring.md`](./magic-dns-manual-wiring.md) |
| Android Tauri / JNI | ✅ | VpnService DNS = fake_ip + `/32`（关则 JNI 可回落运营商 DNS） |
| iOS / macOS-NE | ❌ | 首期明确不支持 |
| `no_tun` | ❌ | 不启动 MagicDNS Runner |

控制台「DNS 覆盖」枚举（B6）：`off` / `covered` / `manual_required` / `unsupported` / `version_too_old`。二期心跳 `magic_dns_os_wired` 可覆盖 OS 推断。仅 **`covered`**（或 `os_wired=true`）算「推了就能用」。

---

## 8. 代码锚点

| 主题 | 路径 |
|------|------|
| MagicDNS server / Catalog | `easytier/src/instance/dns_server/` |
| 记录同步 / static_hosts | `easytier-core/src/gateway/magic_dns/` |
| 转发绑物理网卡 | `magic_dns_forward_connector` / `RuntimeDnsIoProvider` |
| Linux drop-in | `LinuxResolvedConfigurator` |
| 覆盖推断 UI | `easytier-web/frontend-lib` → `dnsCoverage.ts` |
| Fake IP 常量 | `MAGIC_DNS_FAKE_IP` = `10.255.255.254` |

---

## 9. 已知局限（发版可注明）

- 上游仅 IP 字面量；无 DoT/DoH  
- `kill -9` 后 systemd drop-in 可能残留（正常 `close()` / 卸载应删带 EasyTier 头的文件）  
- 离线归档无 `os` 时徽章偏 `manual_required`  
- `StaticDnsHost` vs `DnsHostEntry` 双建模长期漂移风险  
- 发布/解析仍单开关；headless「只被解析」未拆  
- ~~域名驱动子网代理未接~~ → **已由统一路由模型落地**：`[[policy_routes]]` 的 `domains` 规则订阅本 resolver 的**实际应答**学到 IP（`ObservingDnsResolver`）。见 [`traffic-steering.md`](./traffic-steering.md) §5
- **DNS 泄漏面（产品必须明示）**：上游查询强制绑**物理网卡**，即 DNS 查询**本身不走**指定出口；域名路由只保证「连接」走指定出口。另：`tld_dns_zone`（默认 `et.net.`）内的名字**不参与**域名路由观测（避免劫持 mesh 流量）
- 静态 hosts 通配按 **RFC 4592 多级**匹配（`*.corp.example` 也吃 `a.b.corp.example`）—— 有意保留的语义，不是缺陷，见 §3  
- **行为变化（发版需注明）**：`*.com` / `com` 这类单标签 zone、以及 zone 与 route zone 同名的静态条目，现在一律被拒（旧版本会静默装上，并在路由变化时来回抖）；`--dns-host` 遇到非法名字从「静默跳过」改为**报错退出**（TOML / 托管下发仍是 warn + 运行时跳过）
