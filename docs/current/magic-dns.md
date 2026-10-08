# MagicDNS 与 DNS 策略（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-06
- 范围：进程内 MagicDNS、`DnsConfig`（hosts / Split DNS / 上游）、OS 接线与控制台覆盖信号
- 手工接线（OpenWrt / 非 systemd）：[`magic-dns-manual-wiring.md`](./magic-dns-manual-wiring.md)
- 剩余缺口 / 决策留档：[`../roadmap/dns-policy.md`](../roadmap/dns-policy.md)
- 相关：桌面 underlay DNS [`traffic-steering.md`](./traffic-steering.md) §3；域名子网代理（未实现）[`../roadmap/domain-proxy.md`](../roadmap/domain-proxy.md)
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
| `hosts` | `DnsHostEntry{name, ips[]}` 静态清单 |
| `forwarders` | `DnsForwarder{domains[], servers[]}` Split DNS |
| `upstream_dns` | 根 zone 上游列表；空 = 系统 DNS |

CLI 示例：`--dns-host` / `--dns-forward` / `--dns-upstream`。控制台：`frontend-lib` 的 `DnsHostsEditor` / `DnsForwardersEditor` / upstream 列表。

上游格式：**仅 IP 字面量**（如 `1.1.1.1`、`udp://8.8.8.8:53`）；DoT/DoH 与 hostname 上游未做。

---

## 4. 分发路径（R1）

| 路径 | 覆盖 | 说明 |
|------|------|------|
| Managed config | 纳管设备 | `DnsConfig.hosts` 随 web-owned Full/Patch 下发；OFF 实例仍可先存着，启用即生效 |
| MagicDNS 静态通道 | 已开 MagicDNS 的 mesh 节点 | `UpdateDnsRecord` + `client="static-hosts"` → `static-hosts:<tunnel>`；断连清理；TTL 长于路由 1s |

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
- 域名驱动子网代理未接（见 domain-proxy Roadmap）
