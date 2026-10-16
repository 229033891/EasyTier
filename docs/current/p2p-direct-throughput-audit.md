# P2P 直连吞吐瓶颈审查（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-13（首版：逐行核实两节点 P2P 直连数据通路；定位 3 处吞吐瓶颈 / 排除 3 处误判）
- 范围：同一对 peer **已建立直连**后，**单条 PeerConn** 的出站数据通路吞吐上限
- 关联：[`peer-connections.md`](./peer-connections.md)（多 PeerConn 与 bond 发送路径）、[`tunnels-and-transport.md`](./tunnels-and-transport.md)（隧道 scheme）
- 同一通路的**稳定性 / 卡顿**分析：[`p2p-direct-stability-audit.md`](./p2p-direct-stability-audit.md)（本文 B2 即该文 S5）
- 索引：[`../README.md`](../README.md)

> 本文只描述 **代码今天做什么**，以及**从代码结构可确证的瓶颈**。
> **未做现网压测**：以下结论为静态核查（逐行读代码 + 常量核实），量化收益需另做 benchmark 验证。

---

## 1. 结论摘要

**P2P 直连的「选路 / 打洞 / 协议协商」不是瓶颈**；瓶颈集中在**直连建立之后**的单包串行发送路径上。

| # | 瓶颈 | 位置 | 性质 | 严重度 |
|---|------|------|------|--------|
| **B1** | 出队循环**逐包 `await` 发送**，无批量提交 | `socket/udp/session.rs:704-765` | 吞吐天花板 | **高** |
| **B2** | 发送队列深度仅 **128**（实际可用 124），满时**静默丢弃** | `socket/ring.rs:12,163`；`socket/udp/mod.rs:12` | 突发丢包 | **高** |
| **B3** | 每包 **≥2 次全量内存拷贝**，非真零拷贝 | `packet/mod.rs:581`；`tunnel/udp.rs:27-29` | CPU / 内存带宽 | 中 |

已排除（§4）：连接质量选路、路由查找、bonding 分流。

> 出厂默认 `peer_link_bond_count=2` 时，**多并行流**可按流哈希分摊到 2 条 conn；**单条大象流**（单个 RDP / iperf 单线程）仍受本节 B1/B2 限制——与 [`peer-connections.md`](./peer-connections.md) §3 的表述一致。

---

## 2. 直连出站数据通路（已逐行核实）

```
TUN 读包
  │
  └─ peers/peer_manager.rs  PeerOutboundPacketRouter::send_msg_by_ip
        │  ACL + pipelines → 路由查表（DashMap 精确命中，非热点）
        │
        └─ peers/conn/Peer::send_msg            // 选 conn（bond 按流哈希，否则 default_conn）
              │
              └─ peer_conn.rs:1459  PeerConn::send_msg
                    └─ self.sink.send(msg).await          // ← tunnel sink
                          │
                          └─ tunnel/udp.rs:113  UdpTunnelSink::start_send
                                │  packet.convert_type(UDP)
                                │  codec.validate_payload()
                                └─ session_send_tx.force_send(outbound)   // ← B2：满则 Err(Full)
                                      │
                                      └─ socket/ring.rs  RingSocketSender<T>
                                            AsyncHeapRb 容量 128（预留 4 → 可用 124）
                                            │
                                            └─ socket/udp/session.rs:704  forward_udp_session_to_socket
                                                  loop {
                                                    outgoing.next().await      // ← 取 1 个
                                                    codec.encode_tunnel_packet // ← 加密 1 个
                                                    socket.send_to(..).await   // ← B1：发 1 个，串行
                                                  }
                                                        │
                                                        └─ 物理网卡
```

**关键观察**：从 `start_send` 到最终 `send_to`，中间隔了一个**单生产者/单消费者的环缓冲**，由**独立 task 逐包串行搬运**。发送侧（`start_send`）只做入队，本身很快；真正的吞吐由搬运 task 决定。

---

## 3. 瓶颈详述

### B1（高）出队循环逐包 `await`，无批量提交

`socket/udp/session.rs:704-765`：

```rust
loop {
    let Some(outbound) = outgoing.next().await else { break };   // 取 1 个
    let outbound = match outbound { /* ... */ };
    let (datagram, completion) = match outbound {
        UdpSessionOutbound::Datagram { .. } => { /* codec.encode */ }
        UdpSessionOutbound::TunnelPacket(packet) => {
            let datagram = codec.encode_tunnel_packet(packet)?;  // 加密 1 个
            (datagram, None)
        }
    };
    match socket.send_to(&datagram, peer_addr).await { /* 发 1 个 */ }
}
```

问题：

1. **每个包一次独立 syscall**：无 `sendmmsg` / `sendmsg` 批处理，无 UDP GSO。
2. **取包 → 加密 → 发送严格串行**：发送等待期间不预取、不预加密下一个包，CPU 与网卡无法重叠。
3. **队列被闲置**：环缓冲里已有 124 个包待发时，循环仍一次只取一个——缓冲深度没有转化成批量提交能力。

在 UDP 直连下，该循环的吞吐上界 ≈ `1 / (单包加密 + syscall 往返)`，**与链路带宽无关**。这正是「直连正常但跑大流量掉速」的结构性原因。

**对照**：TCP 隧道路径有批量聚合——`tunnel/framed.rs:243` 的 `max_buffer_count = 64` + `chunks_vectored`（`framed.rs:199`）做 vectored write。**UDP 直连路径没有对应机制**。

### B2（高）队列深度 128，满时静默丢弃

| 常量 | 值 | 位置 |
|------|-----|------|
| `RING_SOCKET_CAPACITY` | 128 | `socket/ring.rs:12` |
| `RING_SOCKET_RESERVED_CAPACITY` | 4 | `socket/ring.rs:13` |
| `UDP_SESSION_QUEUE_CAPACITY` | 128 | `socket/udp/mod.rs:12` |

`ring.rs:163` 的可用深度判据：

```rust
if base.occupied_len() >= base.capacity().get() - RING_SOCKET_RESERVED_CAPACITY {
    return Err(RingSocketSendError::Full(item));
}
```

→ **实际可用 124 个包**。按默认加密下 TUN MTU **1360**（`virtual_nic.rs:735-742`，见 [`p2p-direct-stability-audit.md`](./p2p-direct-stability-audit.md) §3.3）估算，满载约 `124 × 1360` ≈ **168 KB**（内层）。

两个后果：

1. **高 BDP 链路填不满管道**：高延迟 + 高带宽链路的 BDP 常远超 186 KB，队列成为硬限。
2. **满时丢包而非背压**：`UdpTunnelSink::start_send`（`tunnel/udp.rs:124`）调用 `force_send`，`Full` 直接映射为 `TunnelError::BufferFull`（`tunnel/mod.rs:53`）。**直连路径上这是丢包，不是等待**。表现为「连接正常但突发即掉速」，且内层 TCP 会把丢包误判为拥塞而退让。

> 注：`socket/udp/session.rs:789-790` 的 `UdpSessionEnqueuePolicy` 区分 `Lossy`（`try_send`）/ `Reliable`（`force_send`），但**隧道数据面走的是 tunnel sink 侧的 `force_send`**，不经过该入队策略的等待语义。

### B3（中）每包多次全量拷贝

- `packet/mod.rs:581` `new_with_payload`：`reserve(total_len)` + `mut_payload().copy_from_slice(payload)` → **1 次拷贝**，且配合 `unsafe set_len`（`:586`）手工管理长度。
- `tunnel/udp.rs:24-36` `zcpacket_from_udp_session_payload`：`buf.resize(HEADER + len, 0)`（清零）+ `copy_from_slice` → **再 1 次拷贝**。

命名上为 `ZCPacket`（zero-copy），但直连路径**至少 2 次全量内存拷贝**。属于 CPU 与内存带宽开销，不是吞吐硬顶，但在小包高 PPS 场景会显著放大。

---

## 4. 已排除的非瓶颈（逐项核实）

| 项 | 结论 | 依据 |
|----|------|------|
| **连接质量选路** | 不在包路径上 | `peers/conn/conn_select.rs` 的评分 / 熔断 / 迟滞仅在周期性重选时求值，不参与每包发送 |
| **路由查找** | 每包零堆分配 | `LocalRouteTable::lookup` 有 CI 零分配门禁：`easytier-core/tests/hot_path_alloc.rs`，空表 / 1K / 10K 规则下均断言 0 次分配 |
| **bonding 分流** | 不引入额外排队 | `conn_bond.rs` 仅按 sticky flow key 选成员（`bond_member_index`），发送仍走所选 PeerConn 的同一条 sink；单流不跨 conn |

---

## 5. 影响面

| 场景 | 是否受影响 |
|------|-----------|
| 单条大象流（单线程 iperf / 单个 RDP / 大文件） | **是**（B1 + B2，最明显） |
| 多并行小流 | 部分（B1 的每包开销；bond_count=2 时可分摊到 2 条 conn） |
| 高延迟高带宽链路（跨地域直连） | **是**（B2 队列深度不足） |
| 低延迟局域网直连 | 影响较小（BDP 低，队列够用；仍受 B1 每包 syscall 限制） |
| 空闲 / 控制面流量 | 否 |

---

## 6. 改进方向（未实施，仅记录）

按「改动量小 → 收益直接」排序：

| 优先级 | 改动 | 目标瓶颈 | 预期方向 |
|--------|------|----------|----------|
| **高** | 出队循环批量取包 + `sendmmsg` / GSO 批量提交 | B1 | 直连单流吞吐显著提升 |
| **高** | 队列满时改为背压等待（或可配置策略），而非静默丢弃 | B2 | 消除突发丢包与内层 TCP 误判 |
| 中 | `UDP_SESSION_QUEUE_CAPACITY` 提高或按 BDP 自适应 | B2 | 高延迟链路吞吐 |
| 中 | ring → session 之间引入 buffer 池，消除 B3 拷贝 | B3 | 降低 CPU 占用 |

> 与 [`../roadmap/connection-stability-todo.md`](../roadmap/connection-stability-todo.md) 的「稳定性」方向相关但**不同**：本文是吞吐 / 丢包结构问题，非连接建立或保活问题。

---

## 7. 核查方法与局限

- **方法**：静态逐行阅读 `easytier-core` 数据通路（`peers/conn/` → `tunnel/udp.rs` → `socket/udp/` → `socket/ring.rs`），核实全部相关常量与错误映射分支。
- **版本**：`easytier` v2.7.6。
- **局限**：
  1. **未做现网 / 本地压测**，B1 的量化吞吐上界、B2 的实际丢包率均**未实测**。
  2. 未覆盖非 UDP 直连路径（TCP / QUIC / WG 直连）：TCP 有 vectored write 聚合（`framed.rs`），QUIC/WG 走各自实现，**未在本文核查**。
  3. 未覆盖 TUN 读取侧与 peer_manager 上游 pipeline 的开销。
- **后续**：如需把本文升级为可依赖的性能结论，建议补一组本地直连 benchmark（单流 iperf3，对比 bond_count=1/2，并采集 `BufferFull` 计数）。
