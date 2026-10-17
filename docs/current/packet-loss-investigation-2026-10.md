# 生产网「连接中途偶发丢包」调查记录（2026-10-17）

> 本文是一次**现网实测调查**的完整证据记录，不是设计文档。目的是：
> 1. 固化可复现的测量方法与原始数据；
> 2. 明确**哪些假设已被证伪**，避免后续重复排查；
> 3. 记录**已确认的缺陷**（含两个与网络无关的 ET 侧 bug）；
> 4. 列出**仍未定性**的部分与下一步取证方法。
>
> 相关：[`tailscale-gap-and-conflict-audit.md`](./tailscale-gap-and-conflict-audit.md)（代码层审查，§5.1 是症状分链）、[`peer-connections.md`](./peer-connections.md)（保活/拆连时间常数）。

---

## Status

| 项 | 状态 |
|---|---|
| 症状 | 连接进行中偶发丢包 + 偶发 200–500ms 时延尖峰；另有一次 ~71s 完全黑洞 |
| 根因 | **未定性**（NAT 老化已解释黑洞；偶发丢包与尖峰尚未定位） |
| ET 控制面是否为原因 | **已证伪**（见 §4） |
| 本机代理是否为原因 | **部分成立**（外层 2%→0%，但内层尖峰与其无关，见 §5） |
| 确认的 ET 侧缺陷 | 3 项（§6） |
| 现场数据 | `C:\Users\Administrator\AppData\Local\Temp\2\opencode\repro\`（本机，临时） |

---

## 1. 环境

**观测机（DEV，本记录作者所在机器）**

| 项 | 值 |
|---|---|
| ET 虚拟地址 | `et` = `10.10.10.101/24`，MTU 1280 |
| 物理地址 | `Ethernet0` = `192.168.10.101/24`，网关 `192.168.10.1` |
| ET 版本 | `2.7.6`，`C:\Users\Administrator\AppData\Local\ET\ET.exe` |
| ET 配置 | `…\com.kkrainbow.easytier\config.d\cd57f3d0-f7d7-415f-b6f8-0821cd123b1e.toml`，`[source] source = "web"` |
| 关键 flags | `default_protocol = "udp"`、`disable_tcp_hole_punching = true`、`peer_link_replica_fill_max = 1` |
| 并存软件 | Clash Verge（mihomo TUN）、OpenVPN TAP（已断开）、VMware Tools |

**被测对端**：home-pc = `10.10.10.51`，实测 RTT 基线 **12–13ms**。

**PingInfoView 报告**（`report.html`，115KB）：**在另一台机器上生成**，目标 `10.10.10.101`，窗口 `12:57:20 – 13:07:18`，268 行 / 236 成功 / 32 失败。

---

## 2. 复现方法

```powershell
# 内层：经 ET 隧道
ping -n 240 -w 1000 10.10.10.51

# 外层：同一时刻并发，用于分离「隧道内」与「物理出口」
$a = Start-Job { ping -n 150 -w 1000 10.10.10.51 }
$b = Start-Job { ping -n 150 -w 1000 43.154.13.211 }   # et.scmec.vip 公网服务器
Wait-Job $a,$b; Receive-Job $a; Receive-Job $b

# ET 侧同步取证（日志为 UTC，warn 级）
Copy-Item "$env:LOCALAPPDATA\com.kkrainbow.easytier\logs\easytier.log" .\et.log -Force

# 路由归属
Find-NetRoute -RemoteIPAddress 43.154.13.211
Get-NetRoute -InterfaceAlias et -AddressFamily IPv4 | Where-Object Protocol -eq NetMgmt
```

> **方法学要点**：日志时间戳是 **UTC**，`+8` 才是本地时间；换算错会导致所有时间对齐失效。

---

## 3. 原始数据

### 3.1 Run A — Clash TUN **开**，单跑内层，240 包（13:49:28–13:53:28）

```
Sent 240, Received 239, Lost 1 (0.4%)
min 11ms   p50 13ms   p95 14ms   max 413ms
#74  413ms      #113 Request timed out      #143 72ms      #199 234ms
```

**同一时间窗内 ET 日志**（`05:49–05:54Z`）**仅一行**：

```
05:49:31 boringtun_easytier::noise::timers: HANDSHAKE(KEEPALIVE + REKEY_TIMEOUT)
```

无拆连、无换路、无路由变更、无 peer 状态变化。

### 3.2 Run B / Run C — Clash TUN 开 / 关，并发对照，各 150 包

| 条件 | 目标 | 出口 | 丢包 | min / max / avg |
|---|---|---|---|---|
| TUN **开** 13:54:26–13:56:59 | `10.10.10.51` | ET 隧道 | 0/150 | 10 / 24 / 13 ms |
| TUN **开** 13:54:26–13:56:59 | `43.154.13.211` | **Clash TUN** | **3/150 = 2%** | 25 / 98 / 27 ms |
| TUN **关** 13:58:03–14:00:33 | `10.10.10.51` | ET 隧道 | 0/150 | 12 / **524** / 18 ms |
| TUN **关** 13:58:03–14:00:33 | `43.154.13.211` | **Ethernet0** | **0/150** | 25 / 72 / 27 ms |

Run C 内层三个尖峰的精确时刻（其余 147 包全在 12–14ms）：

```
13:59:12  114ms
13:59:19  524ms
13:59:30  194ms
```

**Run C 期间 ET 日志零条**（`05:49:31Z` 之后完全空白）。

---

## 4. 已证伪的假设

### 4.1 「是 ET 在做网络检查 / 路径切换造成的」——**证伪**

Run A 中 ET 全程静止（唯一日志是 wireguard 定时器），内层仍出现 0.4% 丢包与 413/234/72ms 尖峰。
Run C 中 ET 同样零日志，仍出现 524/194/114ms 尖峰。
→ **时延尖峰与 ET 的控制面动作无因果关系。**

### 4.2 「残留 2.1% 丢包是 bond / conn_select 切出口造成的」——**证伪**

Run B/C 的两个窗口内 `peer_link_replica_fill_max = 1`、无 bond 成员变动、ET 无任何选路日志，丢包仍出现（Run A）/ 不出现（Run B/C）——**与 ET 侧行为无相关性**。

### 4.3 「是底层链路的稳定劣化」——**部分证伪**

Run B 内层 150 包 **0% 丢包、max 24ms**；Run A/C 同样目标却出现 400–500ms 尖峰。同一路径在数分钟内从「完全平稳」变成「毛刺频发」，**不符合稳定劣化**，更符合**瞬时排队**。

### 4.4 「是 Clash 的 gvisor TUN 造成的」——**部分证伪**

关掉 TUN 后内层尖峰**没有消失，反而出现更大的 524ms**。
→ TUN 不是内层尖峰的原因（但确实影响外层，见 §5）。

---

## 5. 已确认：两个 VPN 双向互抢路由

这是本次调查最确定的结构性问题，**双向都被劫持**。

### 5.1 方向一：ET 外层流量被 Clash TUN 接管

Clash 运行时配置：`tun.enable: true`、`stack: gvisor`、`auto-route: true`、`dns-hijack: any:53`，**全文无 `route-exclude-address`**。

```
0.0.0.0/0  via 198.18.0.2   (Meta / mihomo TUN)  metric 0     ← 胜出
0.0.0.0/0  via 192.168.10.1 (Ethernet0)         metric 256
```

`Find-NetRoute` 实测：

| 目标 | 出口 | 命中路由 |
|---|---|---|
| `43.154.13.211`（et.scmec.vip） | `Meta` | `0.0.0.0/0` metric 0 |
| `124.222.29.94`（对端公网端点） | `Meta` | `0.0.0.0/0` metric 0 |

**后果**：ET 所有外层流量穿过 mihomo 的 **gvisor 用户态协议栈**。
**可观测影响**：走 TUN 的那条路实测 **2% 丢包**；切回物理网卡后 **0%**（样本仅 3 个包，`p≈0.05`，属趋势而非确证）。

### 5.2 方向二：Clash 代理服务器的流量被 ET 隧道接管

`et` 网卡上有 **23 条 `Protocol=NetMgmt` / metric 9000 / on-link** 路由，由 ET 为**对端发布的代理子网**而安装：

```
103.21.244.0/22   103.22.200.0/22   103.31.4.0/22
104.16.0.0/13     104.24.0.0/14     108.162.192.0/18
131.0.72.0/22     141.101.64.0/18   152.65.228.35/32
152.65.234.23/32  162.158.0.0/15    173.245.48.0/20
188.114.96.0/20   190.93.240.0/20   197.234.240.0/22
198.41.128.0/17   20.27.177.113/32  20.205.243.166/32
172.16.0.0/16     172.18.0.0/16     172.25.0.0/24
172.64.0.0/13     192.168.8.0/22
```

`Find-NetRoute 104.19.32.86`（本机 Clash 正在使用的代理节点）→ **出口 `et`，命中 `104.16.0.0/13`**。

**后果**：Clash 连接自己代理服务器的包被送进 ET 隧道，从对端节点出网；ET 的稳定性与 Clash 的稳定性互相耦合。
**源头**：某个 ET 节点的 `proxy_networks` / `proxy_cidrs` 发布了一整份代理 IP 列表。**这是唯一源头，清掉它比在 Clash 侧加排除更重要。**
**注**：`192.168.8.0/22` 覆盖 192.168.8–11，但本机 LAN `192.168.10.0/24` 更长故未受影响；其余私网段（`172.16/16` 等）会被 ET 隧道吃掉。

### 5.3 附带的 DNS 问题

```
Resolve-DnsName -Server 10.255.255.254 DEV   →  198.18.0.21   （Clash fake-ip，非 10.10.10.101）
```

`dns-hijack: any:53` 使 **ET 虚拟 DNS 在本机完全不可用**，节点名解析全被 Clash 冒名应答。

---

## 6. 已确认的 ET 侧缺陷

### 6.1 同 peer 多条 conn 被成簇拆掉（放大器，非起因）

`easytier.log`（warn 级，本地时间）：

| 时刻 | 事件 |
|---|---|
| 07:53:43 / 07:53:55 | 同 peer 两次拆连，间隔 12s |
| 11:26:55 / 11:30:27 / 11:30:31 | 三次 |
| 11:49:57 / 11:50:02 | 两次 |
| 12:04:15 / 12:04:16 / 12:04:22 | **7 秒内三次** |
| 12:23:39 / 12:23:44 | 两次 |
| **13:43:10 / 18 / 23 / 29** | **peer 2752719068 的 4 条 conn 在 19 秒内全被拆**，`loss_rate_1` 0.10→0.14 递增 |

全天 20 次 ping 触发的拆连，被拆 conn 的 `loss_rate_1` 在 **0.05–0.45**；peer `2365013564` 一天内 **81 次** conn 关闭。

`loss_rate_1` 递增说明**雪崩**：一条断 → 其流量压到剩余 conn → 剩余 conn 劣化 → 再断。
**ET 缺少「同一 peer 保留最后一条可用 conn」的保护**。这是 P1.5「抖动宽限」的具体表现（见审计文档 §8 P1）。

### 6.2 `find_interface_index("et")` 失败导致 socket 创建直接失败（**与网络无关的代码缺陷**）

```
easytier::arch::windows: Failed to find interface index for et      × 202
```

按分钟聚类：09:36 / 10:23 / 11:21 / 11:35 / 12:17 / 12:25 / 12:29 / 12:31，**每波约 32 次**。

失败传播链（**每一环都用 `?` 直接返回错误**）：

```
arch/windows.rs:83   find_interface_index()            → Err(NotFound)
arch/windows.rs:100  set_ip_unicast_if()               → Err          (设 IP_UNICAST_IF 失败)
arch/windows.rs:137  setup_socket_for_win()            → Err          (L157 `?`)
tunnel/common.rs:68  setup_socket2_ext()               → TunnelError  (L79 `?`)
```

**后果**：`find_interface_index` 失败的那一刻，**所有需要把出口网卡钉在 `et` 上的隧道/打洞 socket 全部创建失败**。
`find_interface_index` 用 `network_interface::NetworkInterface::show()` 按名字匹配硬编码的 `dev_name`，在适配器被重建/枚举异常时会失配，且**失配被当作致命错误**。

### 6.3 黑洞期 ET 无法自愈，只能等拆连链（既有结论，此处补证据）

PingInfoView 报告中 `12:57:22–12:58:30` 连续 27 次失败（~71s），DEV 日志在 **`12:57:43`** 出现
`too many consecutive pingpong failures, closing the connection … peer 2752719068` —— 落在黑洞正中，
`12:58:33` 恢复。黑洞起点（12:57:22）早于 ET 察觉（12:57:43）21s，**与「S1b 把发现前的等待从 32s 压到 10s」之后的预期量级一致**（注意：该次仍跑在默认 32s 的旧构建上）。

---

## 7. 未定性的部分

### 7.1 内层 200–500ms 尖峰的落点

形态是**瞬时排队**（3 个尖峰集中在 18 秒内，其余时间完全平稳），候选落点：

- home-pc 的上行链路 / 其 NAT 的缓冲区；
- ET 隧道自身（若该 conn 经中继，则中继节点的排队）；
- 对端主机自身的调度延迟。

**取证方法（必须双侧同时做）**：在 home-pc 上同一时刻跑
`ping -n 300 -w 1000 10.10.10.101`（内层）与 `ping -n 300 -w 1000 223.5.5.5`（它自己的公网出口），
对齐时间戳后判定尖峰是否共现。

### 7.2 PingInfoView 报告里的 0ms

报告 268 行中 **111 行为 0ms**、125 行为 15/16ms、另有 203/203/297ms，并在 `12:59` 前后从 15/16ms **永久切换**为 0ms。

从一台**非 DEV** 的机器 ping `10.10.10.101` 得到 `<1ms` 回复，物理上只可能来自本机协议栈。
用户已确认网内 VIP 分配唯一，因此**该现象尚未解释**。待验证项：
报告生成的那台机器上是否存在本地代理/TUN 拦截，或残留的 `.101` 地址。

---

## 8. 重测清单（新版本 / 新配置下）

> 前置：本轮已把 `DEFAULT_PING_INTERVAL_MAX_SEC` 由 32 改为 **10**（默认，无需配置），
> 并在 GUI（高级设置 → 保活 Ping 最大间隔 / 拆连失败阈值）与 CLI（`--ping-interval-max-sec`）暴露入口。
> **但请注意：该改动不改变拆连触发条件**（触发条件是「连续 10 次 ping 失败」，失败后退避已归 1s），
> 因此它**不会消除 §7.1 的尖峰**，两件事必须分开评估。

| # | 项 | 判据 |
|---|---|---|
| 1 | 确认默认已生效 | `ping -n 20 10.10.10.51` 的稳态 RTT 不变；观察 ET 是否还有 32s 级空窗 |
| 2 | 建立干净基线 | Clash TUN **关闭**，`ping -n 300 -w 1000 10.10.10.51`，记录丢包与 max |
| 3 | 检查成簇拆连 | 观察 `easytier.log` 是否仍出现同一 peer 数秒内多次 `closing the connection` |
| 4 | 检查 §6.2 | `Select-String 'Failed to find interface index' easytier.log` 计数是否仍增长 |
| 5 | 路由洁净度 | `Find-NetRoute 43.154.13.211` 应落在 `Ethernet0`；`et` 上不应有覆盖代理 IP 段的路由 |
| 6 | §7.1 定位 | 按 §7.1 双侧同时抓 |

---

## 9. Changelog

| 日期 | 内容 |
|---|---|
| 2026-10-17 | 初稿。证伪「ET 控制面导致丢包」与「bond 切路导致丢包」两个假设；确认 Clash TUN 与 ET 隧道双向互抢路由；记录 `find_interface_index` 失败导致 socket 创建失败的缺陷；尖峰落点与 0ms 现象待定性。 |