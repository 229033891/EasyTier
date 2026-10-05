# DNS 策略：hosts 清单 / Split DNS / 上游转发

## Status

- Status: **Roadmap**（语义已拍板；**R1–R3 + 静态通道 + C2 + 热重载 + B6 首期/二期 + Linux drop-in + JNI Android DNS** 已落地；OpenWrt/非 systemd 手工接线见 [`../current/magic-dns-manual-wiring.md`](../current/magic-dns-manual-wiring.md)；iOS/macOS-NE 明确不支持）
- 最近审阅：2026-10-05
- 适用范围：`easytier` 内嵌 MagicDNS（`easytier/src/instance/dns_server/`）+ `NetworkConfig` + Web 控制台 managed 下发
- 相关现状：MagicDNS 权威 zone 已有（路由 hostname → A 记录）；根 zone 转发器写死系统 DNS（`server.rs`）
- 相关路线：[`domain-proxy.md`](./domain-proxy.md)（W2：静态域名→IP + MagicDNS 下发；本文是其 DNS 解析侧落地）、[`traffic-steering.md`](../current/traffic-steering.md)（桌面 DNS 现状与缺口）
- 已拍板：§6 查询策略、§11 默认值/冲突/覆盖信号/fake IP；实现按 §10 顺序推进
- R1 进度：`NetworkConfig.dns_config`（field 74）→ `ConfigLoader` / `api_input` → MagicDNS `apply_static_hosts`（每主机独立 zone，默认 TTL 300s）；**MagicDNS 静态通道**：`UpdateDnsRecord` 增加 `client` / `static_hosts`，客户端以 `client="static-hosts"` 推送本地 `DnsConfig.hosts`（存为 `static-hosts:<tunnel>`，断连清理）；控制台 B6 首期能力推断见 `frontend-lib` `dnsCoverage.ts`
- R3 进度：`DnsConfig.upstream_dns` 非空时替换根 zone `ForwardAuthority`；强制 `UserProvidedOrder` + `num_concurrent_reqs=1` + 2s/2 attempts；空则回落系统 DNS（B1）。转发套接字经 `magic_dns_forward_connector()` 绑物理默认网卡（复用 `RuntimeDnsIoProvider` / TUN exclude，§7）。
- R2 进度：`DnsConfig.forwarders` → `RunConfig.split_forwarders`；`server.rs` 为每个 suffix 安装独立 `ForwardAuthority`（Catalog 最长匹配，优先于根 zone、次于 hosts / 路由 zone）。
- B6 / C2 进度：`frontend-lib` `DnsHostsEditor` + `DnsForwardersEditor` + upstream 列表 + `DnsCoverageBadge`；CLI `--dns-upstream` / `--dns-host` / `--dns-forward`；TOML `[dns_config]` round-trip 测试。
- 热重载：`MagicDnsServerInstance` 轮询 `get_dns_config()` → `reload_dns_policy`（hosts / split / root forwarder Catalog upsert，不重建 UDP）；`InstanceConfigPatch.dns_config` + managed `hot_patch_base` 剥离，避免仅改 DNS 触发整实例 overwrite；客户端 heartbeat 检测 hosts 变更并走静态通道。
- 平台：Linux `LinuxResolvedConfigurator` 写 `/etc/systemd/resolved.conf.d/easytier-magic-dns.conf`（`DNS=100.100.100.53` + `Domains=~et.net`；无 systemd-resolved 则软跳过）；JNI Android `enable_magic_dns` → VpnService DNS=`100.100.100.53` + `/32` 路由（对齐 Tauri）；手工接线文档 [`../current/magic-dns-manual-wiring.md`](../current/magic-dns-manual-wiring.md)。
- B6 二期：心跳 `optional bool magic_dns_os_wired = 14`；桌面 `set_dns` / Linux drop-in / Android `EasyTierJNI.setMagicDnsOsWired` 写入进程标志；控制台有值时覆盖 OS 推断；`manual_required` 徽章链到手工接线文档。
- MagicDNS fake IP：**最终定为 `100.100.100.53`**（CGNAT、避开 Tailscale `.100`、`.53` 好记；曾用 `.101`，勿再切）。**不要**改为 `6.6.6.6`（公网 DoD，见 §11）。

---

## 1. 需求（3 项）

1. **R1 hosts 清单**：控制台维护一份“域名 → IP”静态清单，推送给纳管设备，或供给 MagicDNS 使用；清单未命中时才走正常解析。
2. **R2 Split DNS**：不同域名（后缀）走不同的上游 DNS 解析。
3. **R3 MagicDNS 上游可配**：hosts 与 MagicDNS 权威 zone 都未命中时，按配置的上游转发，而不是只能用系统 DNS。

约束：

- **C1**：`< 2.7.4`（首发 `DnsConfig` 前的已发版）老客户端必须不断连、不报错（行为退化可接受，协议层必须兼容）。控制台对旧版本跳过 dns patch，避免纳管永不收敛。
- **C2**：不走 Web 控制台的设备（纯 TOML/CLI/GUI 手工配置）必须能配出同样能力。

## 2. 现状与可复用资产

| 能力 | 位置 | 结论 |
|---|---|---|
| 权威 zone（路由 hostname → A，TTL 1s，SOA 兜底） | `dns_server/server_instance.rs:update_dns_records` | 可直接挂第二、第三个 zone（hosts 用长 TTL） |
| 根 zone 转发器（系统 DNS，排除自身 fake_ip 防环） | `dns_server/server.rs:86-111`（hickory `ForwardAuthority`） | R3 = 把写死的系统 DNS 换成可配列表 |
| P2P 记录同步（`UpdateDnsRecord` / `MagicDnsRecordStore` 按 client×zone 替换） | `easytier-core/src/gateway/magic_dns/records.rs` | R1 的 mesh 分发通道现成 |
| 控制台全量配置下发（web-owned `NetworkConfig` + managed revision fence） | `easytier-web/src/db` `managed_config_revisions` / `apply_managed_config_update` | R1 的纳管分发通道现成 |
| 桌面 underlay DNS 绑物理网卡 | `traffic-steering.md §桌面DNS`（`RuntimeDnsResolver`） | R3 防环有先例可抄 |

## 3. 数据模型（proto，只加不改）

```proto
message DnsHostEntry { string name = 1; repeated string ips = 2; }
message DnsForwarder { repeated string domains = 1; repeated string servers = 2; }
message DnsConfig {
  repeated DnsHostEntry hosts = 1;
  repeated DnsForwarder forwarders = 2;
  repeated string upstream_dns = 3;   // 为空 = 沿用今天的系统 DNS 行为
}
// NetworkConfig 追加（新编号，老客户端解析时忽略）：
//   optional DnsConfig dns_config = 74;
```

## 4. 本地解析优先级（每台设备，固定顺序）

1. 静态 hosts（R1）→ 2. MagicDNS 权威 zone（路由 hostname）→ 3. Split forwarder，按域名后缀匹配（R2）→ 4. 默认上游（R3；未配则回落系统 DNS）。

冲突与回落（已定）：

- hosts 与路由 hostname 同名时 **hosts 赢**（静态高于动态）。
- `upstream_dns` 为空、或 split 无匹配后缀且默认上游也未配：均 **沿用系统 DNS**（行为不变原则；禁止静默 NXDOMAIN）。

实现落点全在 `dns_server` 内：hosts 做常驻内存权威 zone；R2 给每个 `domains` 建 `ForwardAuthority` 挂对应 suffix（Catalog 天然多 zone，沿用 `upsert`）；R3 替换 `server.rs:87-103` 的 `read_system_conf()`。

## 5. R1 分发：两条路都要做

- **managed config**：hosts 进 `DnsConfig.hosts` 随 web-owned 配置下发。版本 fence 现成，离线设备上线即得；仅覆盖纳管设备。
- **MagicDNS 记录通道**：静态条目以特殊 client（如 `client="static-hosts"`）经 `UpdateDnsRecord` 进 `MagicDnsRecordStore`。覆盖 mesh 内一切开 MagicDNS 的节点（含未纳管设备）。注意静态条目 TTL 必须可配长，**不能**沿用路由记录的 1s，否则更新风暴。

## 6. 上游查询策略：顺序，不竞速（已定案）

默认上游配 2~3 个时，采用**用户顺序 = 优先级的串行 failover**，禁止多上游并行竞速取最快。依据：

1. **正确性**：hickory 里权威 NXDOMAIN 是终局回答，不会再问别的上游。若上游是 `[内网DNS(含私有zone), 公共DNS]` 这种异构组合，竞速时公共 DNS 先抢答 NXDOMAIN，查询直接失败——顺序查询天然免疫。
2. **答案一致性**：GeoDNS/ECS 下不同上游合法答案本来就不同；竞速导致答案逐查询抖动，违反 `domain-proxy.md` 的“A/B 解析同源”前提，子网代理打不中。
3. **合规**：企业 DNS 做过滤审计，顺序=优先级=策略；竞速等于绕过过滤。
4. **现状佐证**：锁定的 `hickory-resolver 0.25.2` 默认就是 `num_concurrent_reqs=2` + SRTT 排序的先胜竞速，且默认 `QueryStatistics` 会悄悄重排用户顺序——实现时必须显式覆盖，否则“配了优先级”变成“没配”。

落地参数（`ForwardAuthority` 经 `ResolverOpts`）：

```rust
opts.server_ordering_strategy = ServerOrderingStrategy::UserProvidedOrder;
opts.num_concurrent_reqs = 1;
opts.timeout = Duration::from_secs(2);  // 靠短超时 failover，不靠并行
opts.attempts = 2;
```

SRTT 自适应只做 opt-in。Hedged query（首个超 Xms 未回才发第二个）是唯一可接受的尾延迟手段，hickory 0.25 无内置，真需要再手写。

## 7. 必测防环项（上线门禁）

上游配成 VPN 内地址是常见用法（上游就是 mesh 里另一台的 MagicDNS）。转发查询**必须走物理网卡**，不得进 TUN 被自家 packet filter 截获，否则配了上游等于断网。桌面：MagicDNS 根 zone `ForwardAuthority` 使用 `magic_dns_forward_connector()`（与 `RuntimeDnsResolver` 同一套 `RuntimeDnsIoProvider` / TUN exclude）；移动端走平台 `protect`。现有 `excluded_forward_nameservers`（防回自身 fake_ip）要扩展到自定义上游集合（已过滤与 fake_ip 同址的上游）。

## 8. 兼容性（C1）与非控制台配置（C2）

- **C1**：全 `optional`/repeated 增量字段，老客户端解析忽略、行为退化为今天，不断连。控制台用已上报的 `easytier_version` 对 `< 2.7.4` 灰显“不支持 DNS 策略”并跳过 dns patch，不静默。发版前用 2.7.3 二进制实测：收带未知字段的 managed 配置不得拒收（protobuf 未知字段默认透传，确认应用层无 `reject_unknown` 即可）。
- **C2**：同一份 `DnsConfig` 必须在 TOML（`easytier-core/src/config/toml.rs`）、CLI flags、GUI `Config.vue` 三处同时落地，否则控制台与手工设备能力分裂。这是除核心解析外的工作量大头，单独排期。

## 9. 平台覆盖与按实例开关

### 9.1 最后一公里矩阵：解析能力全平台有，接线只接了一半

核心层（包拦截 + 进程内权威 DNS + `127.0.0.1:49813` 记录同步）跨平台， gated 于 `enable_magic_dns`。
但“OS 真把查询发给 `100.100.100.53`”这一步分平台：

| 平台 | 是否自动 | 说明 |
|---|---|---|
| Windows | ✅ 自动 | 接口 `NameServer` + `SearchList`（非 NRPT）按 zone 指向 fake_ip |
| macOS（非 NE） | ✅ 自动 | `DarwinConfigurator`（scutil） |
| Linux（含 OpenWrt） | 首期补齐（↓§11 已定） | 默认 systemd drop-in；OpenWrt/非 systemd 走文档手动 |
| Android（Tauri GUI） | ✅ 自动 | VpnService DNS = fake_ip |
| Android（原生 JNI） | 首期补齐（↓§11 已定） | 按 `enableMagicDns` 条件传 fake_ip，与 Tauri 路径对齐 |
| iOS / macOS-NE | ❌ 未接 | NE 路径不走这套，首期明确不支持 |
| `no_tun` 节点 | ❌ 不跑 | `tun_desktop.rs` 直接返回，DNS 全不启动 |

推论：**R1/R2 是“服务端有数据”，但 Linux 手工节点、JNI 安卓节点压根不向 MagicDNS 发问——推了也白推，且静默失败**。
对策：控制台按「DNS 覆盖」信号验收（定义见 §11 B6）；**禁止**把「配置已下发 + 心跳存活」当成覆盖成功。
**R3 是纯服务端行为**，只要 server 在跑就生效，不受客户端 OS 限制——三件事里唯一全平台生效的。

配套补齐（进排期，不算核心解析工作量）：Linux 给 systemd-resolved drop-in 安装脚本或文档化手动步骤；
JNI 安卓把 DNS 改成与 Tauri 路径一致传 fake_ip。

### 9.2 按实例独立开关（现状延续）

`enable_magic_dns`（每 `NetworkConfig` 一个）→ `flags.accept_dns`（`api_input.rs:607`）；
`MagicDnsRuntime::start` 只在 `accept_dns` 时起 Runner，而 publish + resolve + 系统 DNS 全在 Runner 里。
所以 OFF = 三不（不发布自家记录、不解析 mesh 名、不改本机 DNS），mesh 混编天然允许。

- `DnsConfig` 挂在 `NetworkConfig` 下即自动继承按实例粒度，无需另设计。
- 控制台按设备显示开关状态；managed 照样向 OFF 设备推送 `DnsConfig.hosts`（存着、启用即生效），但 MagicDNS 通道分发的条目 OFF 节点收不到——两条路的行为差要在 UI 写清。
- 发布与解析目前耦合（一个开关）。headless 节点可能只想“被解析到”而不想本机 DNS 被接管：
  **已定**首期保持耦合（B5）；拆分记 follow-up。

### 9.3 老客户端的不对称兼容（与混编缺口）

**解析方向（mesh hostname → A）**：老版本当选 `:49813` MagicDNS server 仍可服务路由记录；
查询侧不要求全员新版本。

**注入方向（`DnsConfig.hosts` / `static_hosts`）**：需要新版本。旧核心 prost **静默丢弃**
未知字段；若旧节点抢到 server，静态 hosts **静默丢失**（选举无版本偏好）。控制台策略（零协议改动）：

- `< 2.7.4` 标 `version_too_old`（能查不能配），**不要**写成「完全不支持」。
- 网络里存在过旧节点时挂 warning：静态 hosts 在混编下可能不可见，直到新版本节点担任 server 或全员升级。

## 10. 落地顺序与验收

1. `DnsConfig.hosts` + managed 下发 + 可配 TTL（R1 最小闭环 = W2 Phase 1 静态部分）。
2. 默认上游可配（R3，`server.rs` 级改动）。
3. Split forwarder（R2）。
4. MagicDNS 静态通道 + C2 三端配置补齐。
5. 平台补齐：Linux 生效故事（脚本或文档）+ JNI 安卓 DNS 对齐；控制台“DNS 覆盖”健康信号。

验收：① hosts 命中不走上游（抓包）；② 上游 failover（拔第一上游，≤ timeout+ε 恢复）；③ 异构上游 `NXDOMAIN` 不污染内网名；④ 上游指 VPN 内地址不断网；⑤ 2.7.3 老客户端收新配置不断连且行为不变；⑥ Fake-IP/系统级劫持**不在本次范围**（Phase 3）；⑦ hosts 下发后按 §11 B6 期望覆盖枚举验收（仅 `covered` 算成功；`manual_required`/`unsupported`/`version_too_old` 明确标出）；⑧ OFF 设备：managed hosts 已存储、启用即生效，MagicDNS 通道条目不可见。

## 11. 已拍板（2026-10-05）

- [x] **B1** `upstream_dns` 为空：沿用系统 DNS（行为不变）；禁止「空=不转发/NXDOMAIN」。
- [x] **B2** hosts 与 MagicDNS 路由 hostname 冲突：**hosts 赢**（静态高于动态；§4 已写死）。
- [x] **B3** split 无匹配后缀且默认上游也未配：沿用系统 DNS（同 B1）。
- [x] **B4** DoT/DoH：首期只做明文 UDP/TCP（如 `udp://1.1.1.1:53`，沿用 `domain-proxy.md` 示例格式）；DoT/DoH 另立项。
- [x] **B5** 发布/解析耦合：首期保持一个 `enable_magic_dns`（与现状一致）；headless「只被解析」记 follow-up。
- [x] Linux 生效故事：systemd-resolved drop-in 为默认（`DNS=100.100.100.53` + `Domains=~et.net`，仅 mesh 域名走 MagicDNS，其余不动；卸载删文件即还原），OpenWrt/非 systemd 走文档手动（dnsmasq `server=/et.net/100.100.100.53` 等）。覆盖判定：systemd 系以 `resolvectl status` 见到 `~et.net` 路由为准，非 systemd 以文档自查为准。
- [x] JNI 安卓 DNS：首期对齐，按 `enableMagicDns` 条件传 fake_ip（开则 `100.100.100.53`，关则保持 `223.5.5.5`/`114.114.114.114`），intent 加开关字段；验收与 Tauri 路径同口径。改前先确认 `.t.kt` 是否编译本体。
- [x] **B6**「DNS 覆盖」健康信号（分两期；**不用** `report_time`+配置比对冒充健康）：
  - **首期（跟 R1）**：控制台用已有心跳字段做**期望覆盖**，不新增上报。输入：`enable_magic_dns` + `device_os` + `easytier_version`。枚举：`off` / `covered`（Win、macOS 非 NE、GUI Android 自动接线）/ `manual_required`（Linux/OpenWrt）/ `unsupported`（iOS/macOS-NE、`no_tun`）/ `version_too_old`（`< 2.7.4`，能查不能配）。hosts 验收：仅 `covered` 算「推了就能用」；`manual_required` 标黄并链文档；其余不得静默当成功。
  - **二期（Linux drop-in / JNI 对齐落地时）**：在 `HeartbeatRequest`（或实例状态）增加 `magic_dns_os_wired`（`optional bool` 或小枚举）；`system_config` / VpnService DNS / drop-in **成功**后置 true，`accept_dns` 但接线失败/跳过则 false。沿用心跳 `support_*` 增量字段模式，**不单开 RPC**。真查询探测 fake_ip 可再后置。
- [x] MagicDNS 服务地址：**最终定为 `100.100.100.53`**（`MAGIC_DNS_FAKE_IP`）。理由：① CGNAT 不可公网路由；② 避开 Tailscale `100.100.100.100`；③ `.53` 好记（DNS 端口联想）。曾用过 `.101`，已一次性迁到 `.53`——**不要**再切到 `.54` 等（每次切换=全网 OS DNS/`/32` 重写）。仍硬编码；**可配化另立项**。
- [x] **否决 `6.6.6.6` / `3.3.3.3` 等公网段**：`6.0.0.0/8`（DoD）、`3.0.0.0/8`（公网）全球可路由。fake IP 必须落在 CGNAT（`100.64.0.0/10`）或文档专用前缀（`192.0.2.0/24` 等）。
- [x] **混编选主缺口（§9.3）**：旧节点抢到 `:49813` → 静态 hosts 静默丢失。首期用控制台版本 warning，不改选举协议。
- [ ] **已知局限（发版文档注明即可）**：上游仅 IP 字面量；`kill -9` 后 drop-in 可能残留（卸载删文件）；离线归档无 `os` → 徽章偏 `manual_required`；`manual_required_docs_url` 指 `blob/main` 在分支合入前可能短暂 404；`StaticDnsHost` vs `DnsHostEntry` 双建模长期漂移风险。