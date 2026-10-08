# MagicDNS 手工接线（Linux 非 systemd / OpenWrt）

## Status

- Status: **Current**
- 最近审阅：2026-10-08（fake IP 由 `10.10.10.10` 改为 `10.255.255.254`，本文示例已同步）
- 适用范围：开启 `enable_magic_dns` / `--accept-dns` 后，进程内 MagicDNS 已在跑，但 OS 未自动把查询指到 fake IP 的平台
- 行为总览：[`magic-dns.md`](./magic-dns.md)；剩余缺口：[`../roadmap/dns-policy.md`](../roadmap/dns-policy.md)；systemd 自动接线见 `LinuxResolvedConfigurator`

## 背景

MagicDNS 服务地址（fake IP）固定为 **`10.255.255.254`**（`MAGIC_DNS_FAKE_IP`，**当前最终值**——每次切换等于全网 OS DNS/`/32` 路由重写）。

选址理由：

1. RFC1918 私网 `10.0.0.0/8`，不可公网路由（否决过 `6.6.6.6` / `3.3.3.3` 等公网段）。
2. **避开** Tailscale MagicDNS 的 `100.100.100.100`（同机冲突）；亦不再使用历史 CGNAT `100.100.100.53`。
3. 由历史地址 `10.10.10.10` 迁移到 `10.255.255.254`（10/8 末尾，几乎不与常见业务网段/网关冲突）；迁移是一次性拍板，不是中间态。

Windows / macOS（非 NE）/ Android（Tauri 与 JNI）会自动接线；**Linux 在检测到 systemd-resolved 时**也会写 drop-in。

下列环境需要手工把「仅 mesh 域名」指到 fake IP，其余域名仍走原有上游：

- OpenWrt / BusyBox dnsmasq
- 无 systemd-resolved 的传统 Linux（`/etc/resolv.conf` 静态或 NetworkManager 自管）
- 容器 / 精简发行版

验收口径（控制台 B6）：仅 **`covered`** 或手工接线成功后上报的 `magic_dns_os_wired=true` 算「推了就能用」。未接线的 Linux 显示 **需手动**。

---

## 1. OpenWrt / dnsmasq

假设 TLD 为默认 `et.net`（可用 `tld-dns-zone` / 配置 `tld_dns_zone` 修改）。

在 dnsmasq 配置中增加（或 UCI `list server`）：

```text
server=/et.net/10.255.255.254
```

含义：后缀 `et.net` 的查询发给 MagicDNS；其它域名仍用 dnsmasq 原有上游。

应用：

```sh
/etc/init.d/dnsmasq restart
# 或
uci commit dhcp && /etc/init.d/dnsmasq reload
```

自检：

```sh
nslookup some-hostname.et.net 10.255.255.254
# 或
dig @10.255.255.254 some-hostname.et.net A +short
```

应返回 mesh 内虚拟 IP。若超时，先确认节点已启用 MagicDNS，且本机有到 `10.255.255.254/32` 的路由（通常由 EasyTier TUN 自动下发）。

---

## 2. systemd-resolved（对照：程序自动行为）

自动 drop-in 路径：

`/etc/systemd/resolved.conf.d/easytier-magic-dns.conf`

```ini
# Added by easytier
[Resolve]
DNS=10.255.255.254
Domains=~et.net
```

手工等价时写入后：

```sh
systemctl reload systemd-resolved
resolvectl flush-caches
resolvectl status   # 应看到 DNS Domain: ~et.net
```

停止 MagicDNS / 关闭 `accept_dns` 后，EasyTier 会删除带 `# Added by easytier` 头的该文件。

---

## 3. 无 resolved 的传统 resolv.conf

不推荐把 **全部** DNS 改成 fake IP（会劫持公网解析）。优先用 dnsmasq / unbound 做后缀分流。

若仅作临时调试，可在另一台已接线的节点上查询，或：

```sh
dig @10.255.255.254 hostname.et.net
```

---

## 4. 与控制台徽章的关系

| 状态 | 含义 |
|------|------|
| 自动 | OS 已（或预期会）自动接线 |
| 需手动 | Linux/OpenWrt 等需按本文接线；或心跳 `magic_dns_os_wired=false` |
| 不支持 | iOS / macOS-NE / `no_tun` 等 |
| 客户端过旧 | `< 2.7.4`（`DNS_POLICY_MIN_VERSION`），能查 mesh 名但不能应用 `DnsConfig.hosts` |

---

## 5. 常见问题

- **只有 hosts / upstream 配了但仍解析失败**：R3 上游与 hosts 是服务端行为；本机查询若不指向 `10.255.255.254`，永远走不到 MagicDNS。
- **改了 `tld_dns_zone`**：手工配置里的后缀必须与之一致（例如 `corp.internal` → `server=/corp.internal/10.255.255.254`）。
- **与 Tailscale 同机**：MagicDNS 用 **`10.255.255.254`**（不是 `.100`），避免与 Tailscale `100.100.100.100` 冲突。
- **上游只能填 IP 字面量**（首期）：`1.1.1.1`、`udp://8.8.8.8:53`、`[fd00::1]:53` 可以；填 hostname 会报 invalid address。DoT/DoH / 域名上游另立项。
- **kill -9 / 异常退出**：systemd drop-in 可能残留，mesh 域名会黑洞到下次正常启动（`close()` 会删文件）。卸载脚本应删除 `/etc/systemd/resolved.conf.d/easytier-magic-dns.conf`（仅当文件带 EasyTier 头）。
- **Windows 停止清理**：正常禁用网络 / 退出时会自动删除本实例装过的代理 CIDR 路由（含 `::/0`）与 TUN 网卡上写入的 `NameServer`（仅回滚自己写过的值）。kill 进程等非正常退出仍可能残留，下次启用→禁用一次即可清掉。
- **混编旧核心当选 MagicDNS server（:49813）**：旧版会静默丢掉 `static_hosts` 等未知字段，静态 hosts 可能暂时不可见。控制台对 `< 2.7.4` 会标「客户端过旧」并提示；升级或保证新版本节点抢到 server 后再验 hosts。
