# Windows 下 overlay 偶发 200–500ms 延迟尖峰：根因审计（2026-11-03）

## Status

- Status: **Current**
- 日期：2026-11-03
- 最近审阅：2026-11-03（第三轮收尾：复核根因表述；P1-2 补强 / P2-1 后台刷新 / P2-2 上游预算已落地；**P3 现网复测仍待用户机验收** —— 详见 §6）
- 范围：**Windows 平台**的 `find_physical_default` 阻塞调用链，以及跨平台的
  MagicDNS filter 数据面耦合。现象观测机为 Windows 11（10 逻辑核），本机并存 3 个隧道适配器
  （另两个第三方 VPN 软件 + ET 自身的 wintun 网卡）
- 版本基线：`2.7.10-b7247d30`（同版本组：DEV 本机 / KWANG / 201）
- 关联：[`underlay.md`](./underlay.md)（物理链路与 DNS 绑定）、[`magic-dns.md`](./magic-dns.md)（MagicDNS 现状 SoT）、
  [`dns-policy-audit.md`](./dns-policy-audit.md)（域名策略审查）、[`data-plane.md`](./data-plane.md)、
  [`packet-loss-investigation-2026-10.md`](./packet-loss-investigation-2026-10.md)（上一轮丢包调查，**其尖峰结论已被本文推翻，见 §4.4**）
- 索引：[`../README.md`](../README.md)

> 行号均为撰写时锚点，可能漂移；以**函数名 / 日志文案**为准。
> 本文是**证据记录**，不是设计文档 —— 目的是固化测量方法、明确已被证伪的假设（避免重复排查）、
> 定位到可修改的代码锚点。

---

## 0. 一句话结论

**不是物理网络问题，也不是 Windows ARP 问题，而是 ET 内部一个「同步阻塞的 Win32 调用」占死 tokio worker。**

- **主因（成立）**：`WindowsIfConfiger::find_physical_default` 是同步函数，内部调用重量级的 Win32
  网卡枚举（`NetworkInterface::show()`，实测尾延迟 **913.6ms**），却从 `async fn` 里**直接调用、
  没有 `spawn_blocking`** → 同步阻塞**占死 tokio worker 线程**。且 `interface_name_from_index`
  位于 **for 循环体内**，每条默认路由都重做一次全量网卡枚举。
- **传导通道（需收窄）**：该查询服务于 DNS 物理网卡绑定（MagicDNS 转发 / STUN·host 解析等）。
  MagicDNS filter **挂在**统一上行 pipeline 上，但**仅**对 `fake_ip:53` 包 await 完整 DNS；
  普通 overlay ICMP **不会**走 DNS 解析。尖刺传导的主机制是 **同 worker 被 sync 调用占死**，
  DNS/STUN 只是常见触发源之一（10 分钟采样里 6 次毛刺仅 #488 与 DNS/STUN 强相关）。
- **量级吻合**：实测阻塞尾延迟 857–914ms，观测到的尖刺为 212 / 247 / 418 / 490ms，
  属**概率性长尾**，与"偶发毛刺"形态一致。
- **评审（2026-11-03 第三轮）**：主因正确；首版「每个上行包 await DNS」表述过重。
  P0 止血企业级合格；P1-1 仍为数据面 timeout（真异步回注未做，记为后续演进）。

---

## 1. 症状与基线

| 项 | 值 |
|---|---|
| 现象 | `ping 10.10.10.16`（KWANG，ET 虚拟地址）间隔一段时间出现毛刺，**零丢包** |
| 正常态 | p50 2–3ms |
| 毛刺幅度 | 212 / 247 / 418 / 490ms（单次） |
| 用户预期 | "理论延迟应该 <1ms" |
| 对照目标 | 同窗口物理网关 `192.168.10.1`：p50 0.59ms、max 5.17ms、>50ms **0 次** |

**关键观察**：毛刺在 **两条独立隧道**（KWANG 走 udp、201 走 wg）上**同时出现**（#488 时
418ms 与 417ms，seq 相差 1），但两条隧道的 underlay 物理端点不同 →
说明阻塞点在**两条隧道共用的下层**。

---

## 2. 测量方法学

本节是本文最有复用价值的部分 —— 三个工具坑都踩过。

### 2.1 四路并发 ping 对照

同时打四条路径，区分「overlay 专属」与「物理链路级」：

| 流 | 目标 | 作用 |
|---|---|---|
| OVL16 | `10.10.10.16` | overlay（udp 隧道） |
| OVL201 | `10.10.10.201` | overlay（wg 隧道，另一条独立隧道） |
| GW | `192.168.10.1` | 物理网关，检验物理链路 / 网卡 / 系统级阻塞 |
| DNSUP | `94.140.14.14` | DNS 上游基线 |

`ping.exe -w 1000 -n 6000`（Windows 自带，无 sleep 抖动；注意 `Thread.Sleep` 定时抖动会被
计入 RTT，上一轮调查已因此误判，见 `packet-loss-investigation-2026-10.md` §12.4）。

### 2.2 pktmon 抓包 + wire RTT

**这是本次定案的关键手段** —— 绕开 ping.exe 的进程启动开销，直接用网卡时间戳算往返：

```
pktmon filter remove
pktmon filter add -t UDP              # 第二轮补上 UDP（第一轮只抓 ARP+ICMP，是关键盲区）
pktmon start --capture --pkt-size 0 --file-name udp.etl
pktmon stop
pktmon etl2txt udp.etl --out udp_udp.txt
```

**wire RTT 定义**：同一 ICMP seq 的 `Tx(请求)` → `Rx(回复)` 间隔。这比 ping.exe 的 RTT 干净，
因为不包含 ping 进程的调度开销。

> **坑 1 — pktmon 输出是 UTF-16LE**。必须 `io.open(..., encoding='utf-16', errors='replace')`；
> 用 utf-8 读会全乱码 → 表现为「ARP 一条都没有」的**假阴性**（本轮踩过两次）。
>
> **坑 2 — 时间戳是 9 位小数（100ns）**。Python `strptime` 只接受 6 位 → 必须
> `frac = (frac + '000000')[:6]`。
>
> **坑 3 — 头行与载荷行分属两行**，必须配对解析。头行含 `方向/类型/组件/筛选器`，
> 载荷行才是协议内容。
>
> **组件号**：`1` = et(wintun) 网卡（裸三层帧，前缀 `ip: A > B: ICMP echo request, id N, seq M`）；
> `12` = 物理 Ethernet0（Ethernet 帧，前缀 `AA-BB-... > CC-DD-..., ethertype IPv4 ... : A > B: ICMP ...`）。
> 两种前缀格式不同，需要两条正则。
>
> **载荷格式**：物理网卡上的 UDP 是 `192.168.10.101.11011 > 192.168.10.201.60527: UDP, length 72`
> —— **点分四段 + 点分端口**，不是 `ip: A > B: UDP port N`（那是 et 网卡的裸三层格式）。

### 2.3 ET 内部日志按 ICMP seq 精确对齐

日志里 `easytier::instance::virtual_nic: convert zc packet to tun packet ret=b"..."` 携带
**完整的 IP 包**（bytes 字面量）。手工解析 IP+ICMP 头可取出 **ICMP seq**，从而把
ET 的注入时刻与 ping.exe 的回复**逐条一一对应**，得到毛刺的真实绝对时刻。

> **坑 4 — ICMP seq 偏移是 `ihl+6`**。`ihl+4` 是 **id** 不是 seq；写错会把所有包挤进同一个字典键。
>
> **坑 5 — bytes 字面量不能用 `eval()` 解析**。日志里含未转义的 `"` → 必须手写
> `parse_bytes_literal()` 逐字符处理 `\xNN \n \r \t \0 \\ \" \'`。
>
> **坑 6 — reply 的对端 IP 在 `src` 字段而非 `dst`**。按 `(dst, seq)` 配对 reply 会全部落空。

### 2.4 ET 运行时基线测量

在 ET 自身所在进程外，用等价调用测同步 Win32 调用的成本分布：

| 调用 | p50 | **max** |
|---|---|---|
| 网卡枚举（`NetworkInterface::show()` 等价） | 27.9ms | **913.6ms** |
| `GetIpForwardTable2` | 14.2ms | **856.8ms** |
| 一次 `find_physical_default`（组合） | 40.1ms | — |

**尾延迟 857–914ms 是决定性数字** —— 与观测尖刺同量级，且解释了为什么是"偶发"。

---

## 3. 拓扑：overlay 的 underlay 都在本地局域网

由 `ET-cli -p 127.0.0.1:15999 peer` + UDP 抓包反推真实端点：

| 虚地址 | 节点 | 隧道 | **物理端点** |
|---|---|---|---|
| 10.10.10.16 | KWANG | udp | **192.168.10.96**（本局域网） |
| 10.10.10.201 | 201 | wg | **192.168.10.201**（本局域网） |
| 10.10.10.27 | Reno15 | relay(2) | 172.30.204.173（公网中继） |
| — | hk | udp,tcp | 101.43.96.62 / 124.222.29.94（公网） |

→ **KWANG 与 201 都在 `192.168.10.0/24`，overlay 往返完全不经过 WAN 出口。**
这点是后面所有归因的基础：任何"WAN 拥塞 / bufferbloat"类解释在这条链路上**不适用**。

**ET 自身发包节拍**（150s UDP 抓包实测，各流 Tx 间隔）：

| 流 | p50 | p90 | p99 | max |
|---|---|---|---|---|
| →192.168.10.96:51981 (KWANG) | 93 | 335 | 336 | 337 |
| →192.168.10.201:60527 (201) | 126 | 266 | 334 | 336 |
| →101.43.96.62:11010 | 162 | 335 | 337 | 343 |
| →124.222.29.94:11010 | 165 | 334 | 341 | 343 |
| →43.154.13.211:11010 | 135 | 318 | 343 | 361 |

全部卡在 **335ms** 附近 = `FAST_PROBE_INTERVAL = 333ms`。**ET 全部流量发包并集的最大空隙
= 191ms，>200ms 的空隙 0 次** → 335ms 间隙是**正常探活节拍**，不是停顿。

---

## 4. 已被证伪的假设

这一节的作用是**防止后续重复排查**。每条都有反证数据。

### 4.1 Windows ARP / 邻居解析（第二轮的假设）—— ❌ 证伪

pktmon 显示 ICMP echo request 在 `11:53:32.038` 就已由 Windows 交给 et 网卡（组件 1 Tx），
`11:53:32.292` 收到 reply → **wire 往返本身就是 253ms**。Windows 侧的邻居解析根本不是延迟所在。

补充：ARP 事件共 46 条，**全部在物理网卡（组件 12）**，et 接口（组件 1）**零 ARP**。

### 4.2 物理链路 / 网卡 / 系统级阻塞 —— ❌ 证伪

同窗口对照：物理网关 wire RTT p50=0.59ms、p90=0.67ms、p99=1.19ms、**max=5.17ms**、>50ms **0 次**；
150s 窗口 GW p50=1ms、max=1ms。**延迟 100% 在 ET 内部。**

### 4.3 WAN 出口拥塞（bufferbloat）—— ❌ 不适用

见 §3：overlay underlay 全在本局域网，不经 WAN 出口。
（上一轮的"物理对照"只 ping 了网关，看不到 WAN 拥塞 —— 但本轮证明 overlay 根本不走 WAN。）

### 4.4 ET 数据面整体停摆（第二轮结论）—— ❌ 证伪

150s 基线：ET 发包并集最大空隙 **191ms**，>200ms **0 次**。毛刺窗口内日志显示
上行发送 / 下行接收 / TUN 注入**仍在正常产生** —— 注入节奏被**拉长**而非停止。

→ 不是"停摆"，是**延迟交付**。这与 §5 的"数据面 await 阻塞调用"模型一致：任务没死，
但卡在 await 上。

### 4.5 ET 进程被冻结 / runtime 全局饿死 —— ❌ 证伪

控制面在毛刺窗口内**仍在输出**：`web_client` config-server heartbeat、`hickory` 自身日志、
`boringtun` noise 计时器。

### 4.6 WireGuard 握手 / 重协商 —— ❌ 证伪

`SESSION_EXPIRED(REJECT_AFTER_TIME)` 每 60s 一次、`Received handshake_initiation` 每 60s 一次，
与毛刺时刻不重合。

### 4.7 ET 数据面丢包 / 背压 —— ❌ 证伪

6 个计数器（`nic_channel_drops` / `tunnel_buffer_full` / `udp_ring_full` / `pending_flush_err` /
`no_dst_drops` / `relay_pending_full`）**全程为 0**。

### 4.8 WG_BUF_POOL 耗尽阻塞 —— ❌ 证伪

`easytier/src/utils/buf.rs` 的 `FixedBufPool::acquire()` 用
`queue.pop().unwrap_or_else(|| vec![0u8; SIZE])` —— **池空时直接分配，永不阻塞**。

### 4.9 UDP 隧道重传 —— ❌ 证伪

`UdpSessionPacketKind` 只有 Syn / Sack / Data，`Sack` 仅用于建连阶段的 NAT 地址交换
→ **无重传**，丢包变丢包，不会变延迟。

### 4.10 「DNS RTT ≈ wire RTT 说明同源阻塞」（第二轮结论）—— ❌ 证伪

第二轮观察：三次毛刺的 wire RTT（253 / 290 / 493ms）与 DNS 上游 RTT（262 / 295 / 492ms）
几乎相等，据此推断"DNS 与数据面共用同一条被阻塞的路径"。

反证：

- 150s 抓包窗口内 overlay **零毛刺**（p99=3ms, max=3ms），但 DNS 上游**仍发生超时** →
  两者可独立出现。
- 10 分钟采样的 **6 个毛刺里只有 1 个**与 STUN/DNS 强相关（#488，前 5s 内 25 条 STUN 事件 +
  6 条 DNS 查询），其余 4 个**零相关**（0 条 STUN、0–1 条 DNS）。
- `94.140.14.14` 的**基线 RTT 就是 227–238ms**（海外 anycast 的常态），另有 12/95 次超时 →
  第二轮把它当"探针"是错的，这是巧合。

> **注意**：这条证伪**不否定** §5 的根因 —— 根因不是"DNS 慢"，而是"DNS 解析路径上挂着一个
> 同步阻塞调用，且这条路径被接进了数据面"。DNS 只是**唯一会稳定触发**该阻塞的入口
> （5s 缓存 TTL + 每次 socket 创建都查），所以毛刺与 DNS 活动**统计相关但非同因**。

---

## 5. 根因（代码级）

### 5.1 主因：物理默认路由查询是同步阻塞调用，跑在 async 上下文

调用链（每一环都已回读源码确认）：

```
RuntimeDnsIoProvider::bind_udp               easytier/src/common/dns.rs:404
  └─ physical_dns_bind_device().await        easytier/src/common/dns.rs:94
       └─ (5s 缓存未命中) lookup_physical_dns_ifname()      dns.rs:119
            └─ ifcfg.find_ipv4_physical_default().await     ifcfg/windows.rs:382
                 └─ Self::find_physical_default(...)          ifcfg/windows.rs:482
                      ├─ GetIpForwardTable2()                 [同步 Win32]
                      └─ for row in rows:
                           interface_name_from_index()       windows.rs:516  ← 循环体内！
                             └─ network_interface::NetworkInterface::show()
                                [枚举全部网卡，同步、重量级]  windows.rs:537
```

**三个叠加缺陷**：

| # | 缺陷 | 位置 | 后果 |
|---|---|---|---|
| D1 | `interface_name_from_index` 在 **for 循环体内**，每条默认路由都调一次全量网卡枚举 | `ifcfg/windows.rs:516` | N 倍放大（Windows 常见 1 条，但多默认路由/多网卡环境会成倍恶化） |
| D2 | `find_physical_default` 是**同步函数**，从 `async fn` 里直接调用，**没有 `spawn_blocking`**；`ifcfg/` 整个模块零 `spawn_blocking` | `ifcfg/windows.rs:482` | **同步 Win32 调用直接占死 tokio worker 线程** |
| D3 | `PHYSICAL_DNS_IF_CACHE_TTL = 5s` | `common/dns.rs:54` | 最多每 5 秒触发一次 → 概率性长尾 |

**触发条件已闭合**：`register_dns_tun_exclude` 在 TUN 就绪时被调用
（`easytier/src/common/global_ctx.rs:259`）→ `prefer_physical_bound_dns()` 为 true
→ **每次 DNS 上游 socket 创建都会走这条阻塞链**。

**Linux 同源缺陷**：`ifcfg/netlink.rs:681` 的 `interface_name_from_index` 同样在 for 循环体内，
`find_physical_default`（`netlink.rs:648`）同样是同步函数从 async 调用 → **P0 修复必须同时覆盖
Windows 与 Linux 两处**。

### 5.2 传导通道：物理绑定阻塞 worker；MagicDNS/STUN 为常见触发源

`easytier-core/src/peers/peer_manager.rs:3939` `run_nic_packet_process_pipeline()`：

```rust
let pipelines = self.nic_packet_process_pipeline.load_full();
for pipeline in pipelines.iter().rev() {
    if let Some(filter) = pipeline.filter_if_active() {
        let _ = filter.try_process_packet_from_nic(data).await;   // ← 数据面 await DNS
    }
}
```

它在 `peer_manager.rs:4299` `send_msg_by_ip()` 里被调用 —— **所有上行数据包的统一入口**。

`MagicDnsPacketFilter::try_process_packet_from_nic`
（`easytier-core/src/gateway/magic_dns/packet.rs`）**仅当**包目标是 MagicDNS 假 IP
（`10.255.255.254`）且 UDP/53 时才 await 完整 DNS 解析；其它包（含 overlay ICMP）快速返回。
触发物理绑定阻塞的典型链：

```
MagicDnsServerInstanceData::resolve            dns_server/server_instance.rs
  └─ resolve_query_inner()
       └─ snapshot_catalog_for + handle_request   ← 读锁已缩短（P1-2）
            └─ hickory ForwardAuthority
                 └─ magic_dns_forward_connector()          common/dns.rs
                      └─ RuntimeDnsIoProvider::bind_udp → §5.1 的阻塞链
```

**架构问题（精确表述）**：物理默认路由的 **sync Win32 枚举**若在 tokio worker 上跑，会卡住
同 worker 上的所有任务（含 overlay）。MagicDNS 假 IP 查询 / STUN host 解析是常见触发源；
并非「pipeline 里每个包都 await DNS」。

> 实施状态（2026-11-03 第三轮）：`resolve_query_inner` 走 `snapshot_catalog_for`（含 zone-key
> 补强）+ 3s 超时；物理绑定查询已 `spawn_blocking` + stale-while-revalidate；
> `CatalogRequestHandler` 因生产未配置 `listen_udp` 不承载流量，暂不处理，依据见 §6。

### 5.3 三段式分解验证

用 ET 日志的 ICMP seq 精确配对，把往返切成三段：

| 段 | p50 | p90 | p99 | max |
|---|---|---|---|---|
| ET 注入 TUN → Windows 收到回复 | 0.52 | 0.92 | 1.02 | **2.44ms** |
| ET 收到 peer 包 → 注入 TUN | — | — | 23 | **290ms** ← 尖刺落点 |
| 上行发出 → 下行收到（隧道往返） | 0 | — | 1 | 11ms |

→ 尖刺落在**「ET 收到 peer 包 → 注入 TUN」**这一段，即 ET 内部处理路径；
隧道往返与 TUN 注入后的交付都正常。这与 §5.2 的模型精确吻合。

---

## 6. 修复方案（含实施进度；第三轮已收尾代码项）

> **判断修正（第二轮）**：首版 §6 的 P2-2 写作「一次 STUN 解析会**并发**打 6 个上游」，与
> `ordered_failover_opts()` 的 `num_concurrent_reqs=1` 自相矛盾，且把
> `RuntimeDnsResolver`（STUN/host 解析路径）与 MagicDNS 转发链混为一谈。
> 经通读 hickory-resolver / hickory-server 0.25.2 源码后修正，见下文 P2-2。

### 实施进度总览（2026-11-03 第三轮收尾）

| 项 | 状态 | 落地位置 |
|---|---|---|
| P0 | ✅ 已实施（P0-1/2/3 合并） | `ifcfg/{windows,netlink,darwin}.rs`：`find_physical_default` 全平台改 async + `spawn_blocking`；Windows 侧 `InterfaceSnapshot` 一次枚举建双索引（iface_name ↔ index），循环内 O(1) 查 |
| P1-1 | ✅ 已实施（方案微调） | `easytier-core/src/gateway/magic_dns/packet.rs`：`DATA_PLANE_RESOLVE_TIMEOUT = 3s`，filter 内 `tokio::time::timeout` 包裹 resolver，超时 WARN + 丢包。**与原方案差异**：未做「spawn 独立任务 + 结果写缓存」，而是直接给数据面解析设硬上限 —— 抖动最坏占住 lane 3s 而非无限期；真异步回注留作后续演进 |
| P1-2 | ✅ 已实施 | `dns_server/server_instance.rs`：`MAGIC_DNS_RESOLVE_TIMEOUT = 3s`；`resolve_query_inner` 改为 `snapshot_catalog_for`（读锁内只做 Catalog 查找 + clone，解析在锁外）+ 3s 超时；上游列表排除 fake_ip |
| P1-2 补强 | ✅ 已实施 | `server.rs`：`catalog_zone_key` 从 query 名向上反推 Catalog 注册 key；`snapshot_catalog_for` 不再用 `authority.origin()`（ForwardAuthority origin 恒为 `.`） |
| P2-1 | ✅ 已实施 | `common/dns.rs`：TTL 5s 新鲜命中直接返回；过期则 **stale-while-revalidate**（立即返回 stale + `AtomicBool` 单飞后台刷新）；冷启动仍 await；查找失败继续 stale 兜底 |
| P2-2 | ✅ 已实施 | `build_system_forward_config` 改用 `ordered_failover_opts()`；`resolver_config()` 经 `apply_bounded_resolver_budget`（并发 1 / 2s / attempts 2）；`RuntimeDnsResolver` hickory 路径外层 `HICKORY_LOOKUP_TIMEOUT = 5s` |
| P3 复测 | ❌ **未开始（待用户机）** | 按 §7 跑 10 分钟双目标 ping + pktmon；Current **不**宣称尖刺已消失 |

### P0 — 消除同步阻塞（✅ 已实施）

**P0-1** `ifcfg/windows.rs` 与 `ifcfg/netlink.rs` 的 `find_physical_default` 调用改由
`tokio::task::spawn_blocking` 承载（同步实现体不变，只把"在哪个线程上跑"改掉）。

**P0-2** `interface_name_from_index` 的 `NetworkInterface::show()` **提到循环外**，
一次枚举建 `HashMap<u32, String>` 查表，循环内 O(1) 查。

**P0-3** 更彻底：Windows 侧改用 `GetAdaptersAddresses` 一次性建表，
避免 `network_interface` crate 的反复全量枚举（`network_interface` 在 Windows 上会
枚举所有适配器，含 3 个隧道适配器）。

> 实际落地：P0-1/2/3 合并为一次改动。三平台统一 async 签名 + `spawn_blocking` +
> 内部 `anyhow::Error` 边界转换；Windows 用 `InterfaceSnapshot` 一次枚举建
> 「index → 名字」与「名字 → index」双索引；netlink 侧 `interface_name_table()`。

> 验收：修复后 §2.4 的基线测量应显示 worker 线程不再被占死；§7 的复测中 >50ms 尖刺应显著下降。

### P1 — 解耦数据面与 DNS 解析（✅ P1-1 / P1-2 已实施）

**P1-1** MagicDNS filter **不应在数据面 await DNS**。改为：命中假 IP 时立即放行/丢弃该
DNS 包并 `spawn` 到独立任务解析，结果写回缓存；后续同域名查询走缓存。
→ DNS 抖动不再传导到 overlay 数据面。
> 实际落地为「timeout 3s 上限 + 超时丢包」（见进度表），spawn+缓存方案留作后续演进。

**P1-2** `resolve_query_inner`（`dns_server/server_instance.rs:734`）里的 `read_catalog().await`
在整个 DNS 解析期间持有 `tokio::RwLock` **读锁** → 根转发器热重载
（`reload_root_forwarder` 需要写锁）会被一个慢 DNS 堵住。建议 clone 出所需 zone 后立刻放锁。
> 实际落地：新增 `Server::snapshot_catalog_for(&self, name) -> Option<Catalog>`，
> 读锁内 `catalog.find(name)` + clone authorities 后放锁，返回独立 `Catalog` 快照；
> `resolve_query_inner` 在快照上 `handle_request()`，整体再包 3s 超时。

**P1-2 补强（✅ 已实施，第三轮）**：
原问题：`snapshot_catalog_for` 用 `authorities[0].origin()` 作 upsert key，而
`ForwardAuthority` origin **恒为 root**，split forwarder 注册名却是 zone
（如 `hostname.example`）。单 authority 快照 + 同 query 时 `find` 仍能走到 `.`，
故多为潜伏正确性风险，不是当时线上必炸路径。

> 实际落地：`catalog_zone_key` 从 query 名沿 `base_name()` 向上，用父级 `find`
> 结果指针是否变化反推 Catalog 注册 key；快照按该 key upsert。单测覆盖
> split zone key ≠ `origin()`。

**附带结论：`CatalogRequestHandler`（`server.rs`）是第二条持读锁路径**，但只服务于
`ServerFuture` 的独立 UDP 监听，生产 `listen_udp = None` 未启用
→ **不承载 MagicDNS 实际流量，暂不处理**，在此记录依据。

### P2 — 缓存与上游治理（✅ 第三轮已实施）

**P2-1（✅）** TTL 5s + stale-while-revalidate：新鲜命中直接返回；过期立即返回 stale，
并用 `PHYSICAL_DNS_IF_REFRESHING` 单飞后台刷新；冷启动仍 await；查找失败继续 stale 兜底。

**P2-2（✅，判断曾修正）**—— 治理对象是**总耗时预算 / 尝试次数**，不是「并发数」：

hickory 0.25.2：`num_concurrent_reqs = 1` ⇒ 串行 failover；`attempts` 是每上游重试次数。
首版「一次 STUN 并发打 6 个上游」有误——那是 `resolver_config()` 路径的默认 opts 风险。

修复前风险表（历史）：

| 路径 | 修复前 | 修复后 |
|---|---|---|
| MagicDNS 显式 `upstream_dns` | 已是 `ordered_failover_opts()` | 不变 |
| MagicDNS 系统 DNS 转发 | system `ResolverOpts`（2 并发 × 5s） | **`ordered_failover_opts()`** |
| STUN / host `resolver_config()` | system opts | **`apply_bounded_resolver_budget`** + 外层 **5s** `HICKORY_LOOKUP_TIMEOUT` |

### P3 — 复测验收（❌ 未开始，待用户机）

按 §7 跑 10 分钟双目标 ping + pktmon UDP 抓包，确认：
`>50ms` 尖刺归零、ET 发包并集最大空隙回到 <200ms、
`find_physical_default` 的 worker 占用时间下降一个数量级。
**在复测完成前，Current 不把「尖刺已消失」写成可依赖现状。**

---

## 7. 验收方法（可复用）

```powershell
# 1) 提级日志
& "C:\Program Files\ET\ET-cli.exe" -p 127.0.0.1:15999 logger set debug
# ... 复现 ...
& "C:\Program Files\ET\ET-cli.exe" -p 127.0.0.1:15999 logger set info   # 改完记得复原

# 2) 抓包（务必带 UDP —— 只抓 ARP+ICMP 会漏掉关键链路）
pktmon filter remove
pktmon filter add -t UDP
pktmon start --capture --pkt-size 0 --file-name udp.etl
# ... 复现 ...
pktmon stop
pktmon etl2txt udp.etl --out udp_udp.txt

# 3) 四路对照 ping（Windows 自带，注意别用 Thread.Sleep 计时）
ping.exe -w 1000 -n 6000 10.10.10.16    > long_OVL16.txt
ping.exe -w 1000 -n 6000 10.10.10.201   > long_OVL201.txt
ping.exe -w 1000 -n 6000 192.168.10.1   > long_GW.txt

# 4) 收尾
pktmon filter remove
```

判读口径：

| 指标 | 修复前 | 期望 |
|---|---|---|
| overlay p99 / max | 132ms / 418ms | <10ms / <50ms |
| overlay >50ms 次数 | 6 次 / 10min | 0 |
| 物理网关 max | 1–5ms | 不变（对照组） |
| ET 发包并集最大空隙 | 191ms | 不变（这是正常探活节拍） |
| `find_physical_default` 占用 | 40ms p50 / 900ms max | 移出 worker 线程 |

---

## 8. 工具坑记录（复用时直接抄）

| # | 坑 | 正确做法 |
|---|---|---|
| 1 | pktmon `etl2txt` 输出是 **UTF-16LE** | `io.open(..., encoding='utf-16', errors='replace')`；用 utf-8 会全乱码 → 表现为"零事件"的**假阴性** |
| 2 | pktmon 时间戳 **9 位小数（100ns）** | `frac = (frac + '000000')[:6]` 后再 `strptime` |
| 3 | pktmon **头行/载荷行分离** | 头行含 `方向/类型/组件/筛选器`，需与载荷行配对 |
| 4 | 物理网卡 UDP 载荷是 **点分四段 + 点分端口** | `192.168.10.101.11011 > 192.168.10.201.60527: UDP, length 72` |
| 5 | et 网卡 vs 物理网卡前缀不同 | 裸三层 `ip: A > B: ...` vs Ethernet `AA-BB-... > CC-DD-..., ethertype IPv4 ... :` |
| 6 | ICMP **seq 偏移是 `ihl+6`** | `ihl+4` 是 id 不是 seq；写错会把所有包挤进同一字典键 |
| 7 | 日志里的 bytes 字面量**不能用 `eval()`** | 日志含未转义的 `"` → 手写 `parse_bytes_literal()` |
| 8 | ICMP **reply 的对端在 `src` 字段** | 不是 `dst`；按 `(dst, seq)` 配 reply 会全部落空 |
| 9 | PowerShell 执行策略拦截 `& script.ps1` | `powershell.exe -NoProfile -ExecutionPolicy Bypass -File script.ps1` |
| 10 | PS 5.1 解析 .ps1 时含非 ASCII 注释会 ParserError | .ps1 注释纯 ASCII |
| 11 | Git Bash heredoc 里 `\d` 会被转义成 `/d` | 用 Write 工具写 .py 文件，不用 heredoc |

---

## 9. 运维注意

- ET GUI 进程以 `--file-log-level warn` 启动；临时提级用
  `ET-cli -p 127.0.0.1:15999 logger set debug`，**改完记得 `logger set info` 复原**。
- `pktmon filter remove` 清过滤器；抓包产物（`*.etl` / `etl2txt` 文本，单个可达 ~10MB）可删。
- 曾尝试用 `New-NetNeighbor` 加静态邻居，因接口不支持失败 —— **未留下任何静态邻居配置，无需回滚**。
