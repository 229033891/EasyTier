# 上游 main → dev 移植 TODO

Status: **Roadmap**

- 最近审阅：2026-10-05（对照 `origin/dev` / `origin/main` 复核）
- 背景：`main` 相对 `dev` 多出的提交里，**6 个是上游功能/修复**（#2609、#2622、#2626、#2627、#2632、#2633），其余为 fork 行政提交（README 改指向、sponsor 删除、CI/Docker 同步等，**不移植**）。提交计数会随时间漂移，以 `git log origin/dev..origin/main` 为准。
- 总原则：**不 `merge main → dev`**（`dev` 大幅领先，且 `releases/v2.7.3a` 收尾中）。一律 cherry-pick / 适配移植。
- 上游提交位置：`main` 分支。
- 发版线：P0 默认先合入 `dev`；若 `v2.7.3a` 仍依赖 web 稳定性且未冻结，再决定是否双推 release 分支（不默认双推）。

---

## P0 —— 先做（小 diff，高确定性）

两项都改 `easytier-web/src/client_manager/mod.rs`：**先 #2633，再 #2609**（同 PR 连续提交，或串行合入；勿并行改同一 accept/prune 路径）。

### 1. #2633 并发握手 `fix(web): accept client handshakes concurrently`
- [x] 移植 `easytier-web/src/client_manager/mod.rs` accept 循环：串行 `await accept_or_upgrade` → `JoinSet`（上限 `MAX_PENDING_HANDSHAKES = 4096`，pin 住 in-flight accept future，避免 WebSocket upgrade 丢连接）
- [x] 新增 `easytier-web/src/client_manager/listener_tests.rs`（非 `#[ignore]` 用例；容量用例保持 ignore，fd 上限 16384 另行验证）
- [ ] `cargo test -p easytier-web` + `cargo fmt --check`（本机 MSVC/ring 编译受阻，待 CI 或可用环境验证）
- 说明：dev 现状（`mod.rs` accept 循环约 L190–225）在 accept 循环内串行 await handshake，造成队头阻塞；`JoinSet` import 已有，无新增依赖。

### 2. #2609 退役被取代会话 `fix(web): retire superseded live client sessions`
- [x] `client_manager/mod.rs`：15s 清理闭包改为 `prune_sessions`（`is_running && !is_superseded` 才留，其余 `remove_if(Arc::ptr_eq)` + `stop()`）
- [x] `client_manager/session.rs`：`stop()` 追加 abort 两个后台 task；新增 `is_superseded()`（紧贴 `is_running`）
- [x] `client_manager/storage.rs`：`owns_authorized_session` 旁新增 `is_session_superseded`（读既有 `managed_runtime_states.session_epoch`，零 schema 变更）
- [x] 移植 `session/lifecycle_tests.rs`（244 行纯测试）
- [ ] `cargo test -p easytier-web` + `cargo fmt --check`（本机 MSVC/ring 编译受阻，待 CI 或可用环境验证）
- 说明：修的是资源泄漏（换地址重连后旧 live session + worker 未 stop），不是选路正确性（dev 的 epoch 门已兜住）。P0，紧随 #2633。

---

## P1 —— 复现确认后再做（需适配，不能直接 cherry-pick）

### 3. #2632 TCP 打洞保活 `fix(peers): keep TCP hole-punched connections alive with 1s pings`
- [ ] 前置：先在 dev 上复现症状（idle TCP 打洞连接是否因 `max_backoff_idx=5` → 约 32s backoff 被 NAT 回收致掉线），无复现则挂起
- [ ] 不扩 `PeerConnectionOrigin` 枚举（dev 保持 `Network/Attached`）；保留 `is_hole_punched: bool`
- [ ] **接入点（必读）**：ping 在 `PeerConn::start_pingpong` 创建；hole punch 经 `HolePunchTunnelSink` → `peer_manager` 的 `set_is_hole_punched(!is_directly_connected)`。TCP/UDP 打洞共用同一 bool，**不能**只凭 `is_hole_punched` 开 1s ping（会误伤 UDP）。适配任选其一，并写进实现注释：
  1. 在 `PeerConn` 增加 `ping_max_interval: Option<Duration>`（或 `is_tcp_hole_punched`），由 peer_manager 在 **TCP** 打洞 admission 时写入；或
  2. `start_pingpong` 时结合 `is_hole_punched` + tunnel 类型（TCP）判断
- [ ] 不碰 `HolePunchTunnelSink` 签名；direct / manual / listener / UDP 打洞保持现有 backoff
- [ ] `PeerConnPinger::new` 加可选 `max_interval: Option<Duration>`（默认 `None` = 现有 backoff 不变）；`peer_conn_ping.rs` 加 cap 分支，保留现有 `backoff_idx` / `max_backoff_idx` 字段逻辑
- [ ] 相关单测 + `cargo test -p easytier-core` + fmt
- 说明：直接合必撞（上游 origin 6 变体 vs dev 2 变体，`peer_conn.rs` / `peer_conn_ping.rs` / tests 全受影响）。无复现前挂起。

---

## P2 —— 看实现再定级（先拆零风险部分）

### 4. #2626 QUIC 缓冲池 `perf(quic-proxy): use BufPool and BufMargins in QuicSocket`
- [ ] 先读 main 实现，确认池所有权/锁粒度（`try_send` 并发语义）后再移植 `easytier/src/gateway/quic_proxy.rs`（约 2 处分配点 + `PacketMargins`→`BufMargins` 统一，含 `make_socket_pair` / `forward` / `test_gso` 测试同步）
- [ ] 说明：dev 已有 #2625 的 `BufPool` / `BufMargins`，quic 路径仍 `BytesMut::with_capacity`，属顺水推舟
- [ ] `cargo test -p easytier` 相关用例 + fmt

### 5. #2627 WG 缓冲池 `perf(wireguard): rename WG_MAX_PACKET_SIZE and optimize scratch buffers`
- [ ] 先合重命名部分（`MAX_PACKET` → `WG_MAX_PACKET_SIZE`，零行为风险，可单独先行）
- [ ] 再从 main 引入 `FixedBufPool`（dev 的 `utils/buf.rs` 目前只有 `BufPool`，无此类型）
- [ ] 读核 `TunnResult<'a>` 借用：上游用 `WG_BUF_POOL.acquire()` 的 guard 包住 buffer，再 `session.send(packet).await`；确认 guard 在 await 期间保持存活、boringtun 初始化保证后再动分配点
- [ ] `cargo test -p easytier` 相关用例 + fmt（含新增 `unsafe` 的 safety 注释复核）

---

## P3 —— 发版后单开分支集成

### 6. #2622 中央网络控制台 `feat(web): add central network management console`
- [ ] `dev` 发版收尾后再开独立集成（分支建议 `integrate/central-network`），从 `main` 拉入解冲突
- [ ] 规模参考：约 101 文件、约 1.9 万加行；冲突重灾区 `client_manager/`、`REST`、`frontend-lib`（正是 dev 改得最多的区域）
- [ ] 前置：P0–P2 落袋、`releases/v2.7.3a` 发出

---

## 非目标（明确不做）

- 不 `merge main → dev`（会带入 README 改指向、sponsor 删除等 fork 行政提交 + ~19k 行中央控制台）。
- 不移植 fork 行政提交（README 改指向、sponsor 删除、仅 CI/Docker 对齐类变更；与 `dev` 已对齐的无需再动）。
- P1 在无复现前挂起；P2 在未读实现前不定级。
