# TUN 数据面吞吐瓶颈（单流 ~50 MB/s）— 待决策

- Status: **Roadmap**
- 最近审阅：2026-11-10
- 索引：[`../README.md`](../README.md)
- 关联：[`../current/p2p-direct-throughput-audit.md`](../current/p2p-direct-throughput-audit.md)（出站通路静态审查）、[`../current/egress-fair-queue.md`](../current/egress-fair-queue.md)（出口分桶）、[`../ops/et-kernel-forward-check.md`](../ops/et-kernel-forward-check.md)（巡检手册）

> 本文只记录**实测事实 + 未开工的改造候选**。在改造落地并通过验收前，**不得**据本文改写 `docs/current/`。

---

## 0. 结论先行

单流吞吐在 Windows 与 Linux 两端都被 **TUN 用户态路径**固定在同一量级，成本约 **58.7 CPU秒/GB**：

- 与**内核转发**无关（实测内核 forward 正常工作，见 §2.4）
- 与**链路**无关（千兆只用到 40%，且全双工收发不互占）
- 与**核数**无关（10 逻辑核整机占用仅 49%，瓶颈是单线程流水线）

能改的只有 `virtual_nic.rs` 的 TUN 读写路径。**改造尚未开工**，待决策。

---

## 1. 口径（引用数字必须带这一段）

| 项 | 值 |
|---|---|
| 客户端 | DEV `10.10.10.101`，Windows，10 逻辑核（Xeon E5-2680 v4 @2.4GHz），`et` MTU 1380 |
| 对端 | `10.10.10.201`，Windows，peer `cost=p2p` / `tunnel_proto=wg` |
| ET 版本 | `2.7.11-9e69e8ea`（两端一致） |
| 负载 | `robocopy` **单流** SMB 拉取 5.28 GB 镜像（`O365ProPlusRetail.img`），非 HTTP、非 iperf3 |
| 采样脚本 | `target/tmp/copytest/monitor4.ps1`：逐秒读 `et` 适配器字节计数 + ET 进程 CPU + `10.10.10.201:445` TCP 握手探针 + `10.10.10.25` ICMP |
| 判定「走没走隧道」 | 比对 `et` 适配器增量与文件大小（>50% 才算 tunnel-bound） |

**前置条件**：Windows 客户端必须关闭 SMB Multichannel，否则流量会**绕过隧道**走物理网卡（见 §2.1）。

---

## 2. 实测数据

### 2.1 SMB Multichannel 会静默绕过隧道（重要）

客户端连接 `\\10.10.10.201` 时，SMB3 Multichannel 会探测服务器的其它地址，额外建立 `192.168.10.101 → 192.168.10.201` 通道：

| 配置 | 吞吐 | `et` 适配器流量 | ET CPU |
|---|---|---|---|
| Multichannel 开（系统默认） | 642 MB / 5.83 s = **104.97 MB/s** | **0 MB** | +0.03 cpu-s |
| Multichannel 关（强制） | 5.28 GB / 107.9 s = **50.11 MB/s** | 5,570 MB | 319.2 cpu-s |

后果：**任何基于 SMB 的吞吐测试在默认配置下测的都是物理局域网**，与隧道无关。运维含义见 [`../ops/et-kernel-forward-check.md`](../ops/et-kernel-forward-check.md) §5。

### 2.2 走隧道（单流 5.28 GB）

| 指标 | 值 |
|---|---|
| 平均吞吐 | **50.11 MB/s**（400 Mbps，千兆的 40%） |
| 逐秒速度 p05 / p50 / p95 | 48.64 / 53.66 / 56.93 MB/s |
| 零进度采样（停顿） | **0 / 105** |
| ET CPU | 319.2 cpu-s / 107.9 s = **2.96 核** |
| 单位成本 | **58.7 CPU秒/GB**（≈17 MB/s/核） |
| 封装开销 | `eth0` rx 6,136 MB vs `et` rx 5,570 MB ≈ **+10%** |

结论：**大文件传输本身不卡**（0 停顿、曲线平稳），问题是吞吐上限，不是抖动。

### 2.3 线程画像（同一窗口）

| 观察 | 值 |
|---|---|
| 最高线程 | **108.9%**（已跑满一个核） |
| 其余 8 个线程 | 25–43% |
| 整机 CPU | 49%（10 核） |
| 反推每包成本 | 50.11 MB/s @ MTU 1380 ≈ **38,000 包/秒**；收+发 76k 操作/秒 ÷ 3 核 → **≈13 µs/操作** |

`13 µs/操作` 远高于一次 `read`/`write` syscall 的 2–5 µs，说明大头在异步机制（锁竞争 + waker 反复唤醒 + 每包 `BytesMut` 拆分/推进），**不是 syscall 数量**。

### 2.4 延迟代价（TCP 握手探针，µs 级）

| 场景 | `201:445` p50 / p99 | `shdx` ICMP p50 / p99 |
|---|---|---|
| 空闲 | 1.02 / 2.88 ms | 0 / 2 ms |
| 走隧道满载 50 MB/s | **26.06 / 37.19 ms** | **23 / 32 ms** |
| 走物理满载 105 MB/s | 14.15 / 16.66 ms | 9 / 15 ms |

物理直连把千兆打满只涨到 14/9 ms，隧道只用 46% 链路却涨到 26/23 ms → **ET 自身排队贡献 12–15 ms**，与 [`../current/high-concurrency-tunnel-stall-2026-11.md`](../current/high-concurrency-tunnel-stall-2026-11.md) 的「饱和排队」是同一机制。

### 2.5 内核转发与设备侧排除项

在 Ubuntu 节点 `hk` 上实测（详见 ops 手册）：`ip_forward=1`、`rp_filter=2`、5 秒内 `IpForwDatagrams +172`（且 `InReceives 671 = Forw 172 + InDelivers 499` 自洽）、`et` 设备 errors/dropped/missed 全 0、`fq` 队列 0 drop、`softnet` 0 drop。

**内核转发与链路层可以从怀疑列表划掉。**

---

## 3. 根因（代码锚点）

| # | 位置 | 事实 | 后果 |
|---|---|---|---|
| R1 | `easytier/src/instance/virtual_nic.rs:671`、`:772` | `let (a, b) = BiLock::new(dev)`，`TunStream`（读）与 `TunAsyncWrite`（写）共用一把锁 | 收发在 TUN 上互斥，流水线单点 |
| R2 | 同文件 `TunStream::poll_next` | 读缓冲硬上限 2500 字节 | 一次 `read` 只取一个包，无法批量 |
| R3 | `tun` 依赖（包 `tun-easytier`，本地 git 依赖）`platform/linux/device.rs` | `if queues_num != 1 { return Err(InvalidQueuesNumber) }`；`configuration.rs` 标注 `queues` 已废弃、恒为 1 | 单队列，多线程无法并行进出 TUN |
| R4 | 同 crate 全量检索 | `VNET_HDR` / `TUNSETOFFLOAD` / `offload` **出现 0 次** | 无法开 virtio 头 → GSO/TSO 用不上 |
| R5 | 运行时证据 | `ethtool -k et` 显示 `tx-tcp-segmentation: off [requested on]` | 申请过 TSO，被内核因无 virtio 头拒绝 |

推论：**只把读缓冲从 2500 调大没有收益**——PI off 且无 vnet_hdr 时，TUN 一次 `read` 只返回一个包。

---

## 4. 改造候选（均未开工）

| 方案 | 改动落点 | 编码 | 自测 | 风险 | 预期收益 |
|---|---|---|---|---|---|
| **A. 拆 `BiLock`** | `virtual_nic.rs:671/772`；Linux 不能走 crate 的 `raw_fd`（linux 后端直接 `Err(NotImplemented)`），需 `dev.as_raw_fd()` + `libc::dup()` + 自建 `tokio::io::unix::AsyncFd` 承担写半边 | 0.5 天 | 0.5–1 天 | **低—中** | **中**（去全局锁 + 减少 waker churn，按 §2.3 推断可能高于 B） |
| **B. vnet_hdr + offload** | 需改 `tun` 依赖（`IFF_VNET_HDR` + `TUNSETOFFLOAD`）+ ET 侧适配 `virtio_net_hdr`、改写 `payload_offset` | 2 天 | 2 天 | **中—高** | **低—中**（只降 syscall 次数） |
| **C. 多队列 TUN** | 同上 crate + `Tunnel` 抽象（`virtual_nic.rs:2236/2274` 的 `split()` 调用点） | 4 天 | 3 天+ | **高** | 大但边际递减，建议排在 B 之后 |

### 风险明细

**A**
- `dup` 出来的 fd 需自行管理生命周期，原 `Device` 的 `close_fd_on_drop` 只管原始 fd，漏关即句柄泄漏。
- `virtual_nic.rs` 的 **A10 注释**已记录：实例重建时必须保证 AsyncFd 的 epoll DEL 先于 fd 号复用完成；多一个 fd 就多一处该时序。
- Windows 无 `dup`/`AsyncFd`，只能 cfg 分叉保留 `BiLock` → 两平台行为不一致，测试矩阵翻倍。
- macOS `macos-ne` 与 `has_packet_info=true` 会改 `payload_offset += 4`，必须同批验证，否则静默错位。

**B**
- `virtio_net_hdr` 构造错误的表现为**静默丢包/内容损坏**，不报错。
- 动到「一包一加密」假设：GSO 超级包要么整体加密（对端需重组）要么用户态切分，两者都要改语义。
- Android / iOS / macOS / FreeBSD 共用同一 crate 接口，回归面最广；改的是 pinned git 依赖，需 bump rev 并重跑打包。

**C**：需改 `Tunnel` trait 的 `split()` 语义，面太大，不建议单独做。

---

## 5. 决策前置实验（30 分钟，先做这个）

把两端 `et` MTU 从 1380 调到 1280 重跑同一条 SMB 复制（关闭 Multichannel）：

- 包数 +7.8%，吞吐若同步下降 7~8% → 成本由**包率**驱动，A 与 B 都值得投
- 吞吐基本不变 → 成本在字节侧（加密/拷贝），**两个方案都别急着做**，先做 profile

Windows 侧无 `perf`，这是不写代码就能拿到的唯一判别信号。

---

## 6. 已证伪的假设（勿重复）

| 假设 | 实测结论 |
|---|---|
| 「换 Linux 内核转发就会更快」 | 内核转发本来就正常（`IpForwDatagrams +172`/5s，0 丢包）；瓶颈在用户态 TUN 路径，换系统不会自动变好 |
| 「加核能吃满」 | 单线程已跑满一个核，其余线程 25–43%，整机仅 49% —— 加核无用 |
| 「千兆半线速是天花板」 | 千兆全双工收发不互占；50 MB/s 仅线速 40%，软件还有空间 |
| 「把读缓冲调大就能批量收发」 | PI off 时一次 `read` 只返回一个包，缓冲再大也无用（见 §3 推论） |
| 「SMB 复制测的就是隧道吞吐」 | 默认开 Multichannel 时 0 字节经 `et`，测的是物理局域网（见 §2.1） |

---

## 7. 未决

1. 是否投方案 A（1–1.5 人日）；投则先跑 §5 前置实验。
2. 方案 B 的收益预期已从「最大」下修为「低—中」，是否仍排在 A 之后。
3. Windows Wintun 能否也拆锁，取决于其 session 并发 send/receive 是否安全 —— 未验证。
4. Linux 端缺少多核对照机（`hk` 仅 1 vCPU、`shdx` 为 OpenWrt），跨平台对比目前**无有效样本**。
