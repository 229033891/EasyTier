# P2P 直连稳定性审查（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-27（**§3.3 MTU 上限收紧：`MAX_TUNNEL_MTU` 由 `u16::MAX` 改为 8168 = `MAX_UDP_SESSION_DATAGRAM_SIZE` 8192 − UDP 隧道头 8 − peer-manager 头 16，合法范围变为 `[1300, 8168]`**；2026-10-17：S3 摘要表数值对齐出厂 1300→TUN 1280（正文早已更新、摘要表遗漏）；§3.3 同步 CR-13 —— MTU 改为共享校验 `validate_tunnel_mtu`，越界由「静默产生坏 MTU」改为拒绝启动；新增 **S1b「UDP 打洞 conn 缺 NAT 保活上限」**，2026-10-17 已修为 10s 上限）
- 范围：两节点**已直连**后，**长连接交互式流量（RDP / SSH / 终端）**的体感稳定性
- 关联：[`p2p-direct-throughput-audit.md`](./p2p-direct-throughput-audit.md)（同一条通路的吞吐瓶颈）、[`peer-connections.md`](./peer-connections.md)（选路 / bond）、[`tunnels-and-transport.md`](./tunnels-and-transport.md)、[`tailscale-gap-and-conflict-audit.md`](./tailscale-gap-and-conflict-audit.md)（Bug/冲突 vs 设计分栏）
- 上游缺口跟踪：[`../roadmap/connection-stability-todo.md`](../roadmap/connection-stability-todo.md)
- 索引：[`../README.md`](../README.md)

> 本文区分**已核实**（读代码 / 常量）与**推断**（标注「推断」）。未做现网抓包与压测。

---

## 1. 现场症状与结论

**症状**：同样网络条件下，RDP 经 EasyTier 比经 Tailscale 更容易卡顿。

**结论**：不是单点缺陷，而是**五个机制叠加**，且都是「丢一点包 → 体感放大」的类型：

| # | 机制 | 类型 | 影响 |
|---|------|------|------|
| S1 | ping 连续失败 5 次即**拆连接**，且无短时抖动宽限 | 已核实 | 闪断后恢复慢 → RDP 冻结数秒 |
| S1b | UDP 打洞 conn **无 NAT 保活上限**（退避到 32s > NAT 老化 ~30s） | **已修** 2026-10-17 → 10s | 空闲后黑洞无人察觉，随后拆连+重打洞 |
| S2 | `default_protocol` 含 `tcp`，UDP 不可用时**回落 TCP** → TCP-over-TCP | 已核实 | 轻微丢包即吞吐崩塌 |
| S3 | MTU **静态** 1280（线缆 ~1352B），无 PMTU 发现、无 MSS 钳制 | 已核实；越界校验 2026-10-17 已补（CR-13） | 隧道套隧道时仍可能超路径 MTU → 丢包 |
| S4 | bond / 质量选路会**重映射流**到另一条 conn | 已核实 | 换路瞬间乱序 → 卡顿 |
| S5 | 发送队列满时**静默丢包**（深度仅 128） | 已核实 | 突发即丢，内层 TCP 误判为拥塞 |

其中 **S5 与吞吐文档的 B2 是同一处代码**：[`p2p-direct-throughput-audit.md`](./p2p-direct-throughput-audit.md) §3。

---

## 2. 出厂默认（v2.7.6，已逐项核实）

来源：`easytier-core/src/config/toml.rs:106-173` `gen_default_flags()`。

| 参数 | 默认值 | 位置 | 稳定性含义 |
|------|--------|------|-----------|
| `mtu` | **1300** | `toml.rs` `gen_default_flags` | TUN 实际取 `1300 - 20 = 1280`（对齐 Tailscale；见 §3.3） |
| `enable_encryption` | **true** | `toml.rs:116` | 默认加密；MTU 因此被扣 20 |
| `encryption_algorithm` | `EncryptionAlgorithm::default()` | `toml.rs:147` | — |
| `data_compress_algo` | **None** | `toml.rs:133` | ✅ 默认不压缩，无额外延迟 |
| `default_protocol` | **`"udp,tcp"`** | `toml.rs:114` | UDP 优先（对），但保留了 TCP 回落（S2） |
| `ping_fail_close_count` | **5** | `toml.rs:159` | 连续 5 次 ping 失败即断连（S1） |
| `ping_interval_max_sec` | **10**（2026-10 起；此前 32） | `toml.rs:132` | 自适应 ping 上限；**丢包时退避重置为 1s**。10 是 NAT 安全值：32s 曾长于 ~30s 的 carrier-NAT UDP 映射，直连隧道空闲即黑洞 |
| `peer_link_bond_count` | **2** | `toml.rs:171` | 出厂即多路径（S4） |
| `enable_kcp_proxy` | **false** | `toml.rs:135` | 抗丢包代理默认关 |
| `enable_quic_proxy` | **false** | `toml.rs:141` | 同上 |
| `bind_device` | **true** | `toml.rs:134` | 绑物理网卡（利于绕过本机代理隧道） |
| `lazy_p2p` | false | `toml.rs:128` | 不减少打洞 |
| `multi_thread` | true（`multi_thread_count=2`） | `toml.rs:132,146` | — |

**全代码库无 FEC**（前向纠错），`grep` 全仓无相关实现 —— 与 WireGuard 类方案一致，非差异点。

---

## 3. 机制详述

### 3.1 S1 — 闪断后拆连接代价高

`peers/conn/peer_conn_ping.rs`：

- 单次 ping 超时 **2s**（`:226`）。
- 连续失败达 `fail_close_count`（默认 **5**，`:327`）即 **break 拆连接**。
- 丢包时退避被重置：`loss_counter > 0` → `backoff_idx = 0` → **每 1s 一次 ping**（`:100-101`）。
- 最坏路径：5 次 × (2s 超时 + 1s 间隔) ≈ **15s 内拆连接**。

拆连接后需**重新拨号 + 重新 Noise 握手 + 可能重新打洞**，恢复以秒计。RDP 在这段时间表现为**冻结**。

> **与 WireGuard 类方案的差别**：WireGuard 会话在短暂丢包期间**保持**，链路恢复后直接续传，不需要重建握手。EasyTier 的 PeerConn 是**拆掉重建**语义 —— 这是 RDP 体感差于 Tailscale 的最可能主因之一。
> **推断成分**：未实测拆连接耗时；但「拆连接 + 重握手」的结构性代价可由代码确证。

出厂**无短时抖动宽限** —— 上游清单 [`connection-stability-todo.md`](../roadmap/connection-stability-todo.md) **P1.5** 正是此需求（「短暂丢包不立即拆 conn，避免比 OpenVPN 更神经质」），**状态：未实现**。

### 3.2 S2 — TCP 回落导致 TCP-over-TCP

`config/protocol_preference.rs:6-13` 明确记录了设计权衡：

> `udp` 放在最前，因为默认启用的传输里只有它**不叠加自己的可靠性**；`tcp`/`ws`/`wss`/`quic`/`faketcp` 都会重传，内层 TCP 流（**RDP**、SSH、HTTP）就此变成 TCP-over-TCP，**轻微丢包即卡死**。

UDP 直连正常时无此问题。但当 UDP 打洞失败 / UDP 被限速 / 连接被判定不可用而**回落 `tcp`** 时，RDP 走 TCP-over-TCP → 经典重传放大 → 卡顿。

**这是「同网络下时好时坏」的典型来源**：是否踩中取决于本次实际使用了哪个 scheme。

### 3.3 S3 — MTU 静态且偏大

`easytier/src/instance/virtual_nic.rs`（**2026-10-17 CR-13 已改**，下面是改后代码）：

```rust
// 单一共享校验：与 api_input 走同一条规则，避免两处分叉
let mtu_in_config = match easytier_core::config::validate_tunnel_mtu(flags.mtu) {
    Ok(configured) => configured,
    Err(err) => {
        tracing::error!(?err, configured_mtu = flags.mtu,
            "refusing to start tun interface with an unusable MTU");
        return Err(anyhow::anyhow!("invalid tunnel MTU {}: {err}", flags.mtu).into());
    }
};
// 校验保证 mtu >= MIN_TUNNEL_MTU（= 1280 + 加密开销），故不会下溢；
// saturating_sub 只是防未来常量改动悄悄回绕的兜底
let mtu_in_config = if flags.enable_encryption {
    mtu_in_config.saturating_sub(easytier_core::config::ENCRYPTION_MTU_OVERHEAD)  // -20
} else { mtu_in_config };
```

⇒ 默认加密下 **TUN MTU = 1280**（与 Tailscale `tstun` `defaultMTU` 一致）。

**⚠️ 行为变更（CR-13 + 上限收紧，需写进 release note）**：合法范围 **`[1300, 8168]`**（`MIN_TUNNEL_MTU = 1280 + 20`；`MAX_TUNNEL_MTU = 8192 - 8 - 16 = 8168` —— `MAX_UDP_SESSION_DATAGRAM_SIZE`（`socket/udp/virtual_socket.rs:34`）减去 UDP 隧道头 8 与 peer-manager 头 16；加密的 20 由接口侧扣除，所以配置值本身就是 ZCPacket 上限）。两条边界各自会挡下什么：

- **下限（CR-13）**：改前 `mtu` 越界（如 `0` / `-1`）会**静默产生一个坏 MTU**，Linux 上 `SIOCSIFMTU` 只抛裸 `EINVAL`；改后**拒绝启动**并给出可读错误。**部署中若有 `mtu` 设成 `0` / `-1` / 极小值的配置，升级后实例将起不来**，需先改配置。
- **上限（2026-10-27 收紧）**：`u16::MAX` 从来就载不动 —— UDP 会话路径超过 8192 的数据报在 `socket/udp/session.rs:442` 直接拒收，TCP 帧化路径的 body 上限只有 2000（`tunnel/framed.rs:23` `TCP_MTU_BYTES`）。改前 `mtu = 9000`（jumbo）属于「能启动但**静默不通**」，改后**拒绝启动**。**部署中若有 `mtu > 8168` 的配置，升级后实例将起不来**，需先改配置。注意该上限只保证 UDP 会话路径能承载；TCP 隧道上实际可用上限更低（约 1976），见下。
- `api_input.rs` 侧同步改为 `u32::try_from` + 共享校验，`ConfigError::InvalidMtu` 载荷由 `u32` 改为 `i64`（忠实反映调用方实际发来的值，GUI 传 `-1` 不再被报成 `4294967295`）。

线缆包大小（`packet/mod.rs:450`）：

```
UDP Tunnel: TUN MTU + 24 (Easy) + 20 (Encrypted) + 8 (UDP) + 20 (IP) = TUN MTU + 72
```

⇒ 默认外层 UDP 包 ≈ **1280 + 72 = 1352 字节**（旧默认 1360+72=1432）。

**风险**：路径 MTU 极低（隧道套隧道、部分 4G/企业网）时仍可能分片或丢包。代码库**无 PMTU 发现、无 MSS 钳制**—— MTU 仍纯静态；出厂已取保守值，极端路径仍需手动再调低。

> **历史**：2026-10-16 前出厂 `flags.mtu=1380` → TUN 1360；对照 Tailscale 1280 偏大约 80B，已改为 1300。
> **推断成分**：未抓包确认本环境路径 MTU；需按 §5 方法实测。

### 3.4 S4 — 路径重映射引入乱序

`peers/conn/conn_bond.rs` + `conn_select.rs`：

- 出厂 `bond_count=2`：`send_msg` **按内层五元组哈希**选成员（`bond_member_index`，`conn_bond.rs:332`）。
- 缺陷窗口：当 bond 发送集**重建**（成员掉线、质量档变化）时，`pick_bond_set` 结果变化 → 同一 RDP 流的哈希目标**变到另一条 conn**（`conn_bond.rs:232-329`）。
- `select_conn` 亦会在满足迟滞条件后切 `default_conn`（默认 ~15s 才切，`conn_select.rs:211-220`）。

换路瞬间，两条独立 UDP 路径的**时延差**会造成乱序；RDP 侧视为丢包 → 卡顿。
代码已用**确定性 tie-break**（`conn_id`，`conn_bond.rs:217-222`）抑制同分抖动，但**无法消除成员变更导致的重映射**。

### 3.5 S5 — 队列满静默丢包

见 [`p2p-direct-throughput-audit.md`](./p2p-direct-throughput-audit.md) §3 B2：环缓冲 128（可用 124），`force_send` 满即 `BufferFull` → **丢包而非背压**。
对交互式流：突发（RDP 画面刷新 / 粘贴大文本）瞬间打满队列 → 丢包 → 卡顿。

---

## 4. 与 Tailscale 的差异对照

| 维度 | EasyTier（默认） | Tailscale | 差异是否可核实 |
|------|------------------|-----------|----------------|
| **会话断连语义** | ping 失败 5 次 **拆 PeerConn**，重握手 | WireGuard 会话保持，短暂丢包后**续传** | ✅ EasyTier 侧已核实；对端为公开行为 |
| **MTU** | TUN **1280**（`flags.mtu=1300`，线缆 ~1352；2026-10-16 前为 1360） | **1280** | ✅ 出厂已对齐 |
| **多路径** | 出厂 `bond_count=2`，按流哈希，**成员可变** | 单 peer 单会话（不看多条 conn 抢流） | ✅ |
| **丢包抗性代理** | KCP/QUIC proxy **默认关** | 无对应物（靠 WG 自身） | ✅ |
| **传输回落** | UDP → **TCP**（TCP-over-TCP 风险） | 直连失败走 DERP（自有帧格式，非裸 TCP 隧道） | ⚠️ 部分推断 |
| **本机代理隧道干扰** | `bind_device=true`（应绑物理网卡） | 常见规则中 Tailscale 多被代理**放行** | ⚠️ **推断**，需按 §5 核实 |

> **本机环境特记**：该机器默认路由经 `Meta Tunnel`（198.18.0.1/30，网关 198.18.0.2），且 `127.0.0.1:7897` 有代理监听 —— 属典型「本机透明代理接管出站」形态。若代理规则**放行 Tailscale 但未放行 EasyTier**，则两者走的根本不是同一条 underlay，体感差异与 EasyTier 代码无关。**这是必须首先排除的混杂因素**（§5 步骤 1）。

---

## 5. 可立即执行的缓解（配置层，无需改代码）

按「性价比」排序：

| 优先级 | 项 | 现值 | 建议 | 目的 |
|--------|-----|------|------|------|
| **0** | 排除混杂因素 | — | 确认 EasyTier 的 UDP 是否被本机代理接管（见下方步骤 1） | 否则后续调参结论无效 |
| **1** | `mtu` | **1300**（出厂已对齐） | 仍不通可试 **1280**（→ TUN 1260）或更低 | 极端路径再压 MTU |
| **2** | `ping_fail_close_count` | 5 | **10** | 容忍闪断，避免 S1 拆连接 |
| **3** | `default_protocol` | `"udp,tcp"` | **`"udp"`**（确认 UDP 可用时）；或 `"udp,quic"` | 消除 S2 TCP-over-TCP |
| **4** | `peer_link_bond_count` | 2 | **1**（对照测试） | 消除 S4 重映射；单流路径更稳 |
| 5 | `enable_kcp_proxy` / `enable_quic_proxy` | false | 若确为高丢包链路，**单端试开** | 抗丢包（上游 P2.5 未产品化，需手工） |
| — | `data_compress_algo` | None | **保持 None** | 压缩增加延迟，勿开 |

> ⚠️ **MTU 注意**：TUN MTU = `flags.mtu - 20`（加密时，`virtual_nic.rs:737`）。出厂已是 **1300→TUN 1280**；直接填 1280 会得到 TUN 1260。这是最容易填错的一项。
> ⚠️ MTU 属「单端可试」参数（`connection-stability-todo.md` §2.6），但建议**两端一致**以免排障混淆。老网络若 TOML/库里仍写着 1380，不会随升级自动改写。

**步骤 1（排除代理混杂）**：
1. 用 `easytier-cli peer` 查看实际建立的 PeerConn **协议**（是 `udp` 还是回落成了 `tcp`）、**RTT / loss / jitter**。
2. 若协议是 `tcp` → 直接命中 S2，优先修协议回落，而非调 MTU。
3. 抓包/路由确认 EasyTier 的 UDP 出站是否走了 `Meta` 隧道而非物理网卡 `Ethernet0`（`bind_device=true` 本意是绑物理网卡）。

---

## 6. 需代码改动（上游缺口，不在本仓可配范围）

| 项 | 说明 | 跟踪 |
|----|------|------|
| **短时抖动宽限** | 短暂丢包不立即拆 conn | P1.5（**未实现**） |
| **队列满背压** | 满时等待而非静默丢弃 | [`p2p-direct-throughput-audit.md`](./p2p-direct-throughput-audit.md) B2 |
| **MTU 自适应 / PMTU** | 静态 MTU 改自适应，或至少 MSS 钳制 | 本文 S3；无上游条目 |
| **KCP/QUIC 策略产品化** | 丢包场景自动启用（非隐性默认） | P2.5（**未实现**） |
| **失败必有回落路径** | 可配置保底档 | S1（`connection-stability-todo.md` §1） |

---

## 7. 局限

1. **未做现网抓包 / 压测**：S1 的恢复耗时、S3 的真实路径 MTU、S5 的丢包率均**未实测**。
2. **与 Tailscale 的对照**基于公开行为认知，未在本机做 A/B 对照实验；「代理放行差异」明确标为**推断**。
3. 只覆盖 **UDP 直连** 通路；QUIC / WG / faketcp 直连路径未核查。
4. 未覆盖 TUN 读取侧、`peer_manager` 上游 pipeline、以及网关 / 子网代理场景。
5. 结论基于 **v2.7.6**；上游行为可能变化。

---

## 8. 结论（一句话）

EasyTier 的 P2P 直连**能跑满带宽**（现场实测下载约 70 MB/s），其 RDP 体感弱于 Tailscale 的原因**不在吞吐，而在「丢一点包就放大成卡顿」的五个机制** —— 其中 **ping 失败 5 次即拆连接**、**TCP 回落**仍可用配置缓解；**MTU 出厂已对齐 Tailscale 1280**（2026-10-16），老配置若仍写 1380 需手工改或重建默认。
