# 上游 main → dev 移植 TODO

Status: **Roadmap**（P0–P2 代码已落袋，待 CI / `cargo test`；P3 待发版后独立集成）

- 最近审阅：2026-10-08（发版线由 `v2.7.3d` 更正为当前发版线，现 `releases/v2.7.45`；对照 `origin/dev`）
- 背景：`main` 相对 `dev` 多出的提交里，**6 个是上游功能/修复**（#2609、#2622、#2626、#2627、#2632、#2633），其余为 fork 行政提交（README 改指向、sponsor 删除、CI/Docker 同步等，**不移植**）。提交计数会随时间漂移，以 `git log origin/dev..origin/main` 为准。
- 总原则：**不 `merge main → dev`**（`dev` 大幅领先）。一律 cherry-pick / 适配移植。
- 上游提交位置：`main` 分支。
- 发版线：P0–P2 已合入 `dev`；验证通过后跟当前 `releases/v2.7.*` 发版线（2026-10-08 为 `releases/v2.7.45`），不默认双推历史分支。
- 索引：[`../README.md`](../README.md)

---

## P0 —— 先做（小 diff，高确定性）

两项都改 `easytier-web/src/client_manager/mod.rs`：**先 #2633，再 #2609**（同 PR 连续提交，或串行合入；勿并行改同一 accept/prune 路径）。

### 1. #2633 并发握手 `fix(web): accept client handshakes concurrently`
- [x] 移植 `easytier-web/src/client_manager/mod.rs` accept 循环：串行 `await accept_or_upgrade` → `JoinSet`（上限 `MAX_PENDING_HANDSHAKES = 4096`，pin 住 in-flight accept future，避免 WebSocket upgrade 丢连接）
- [x] 新增 `easytier-web/src/client_manager/listener_tests.rs`（非 `#[ignore]` 用例；容量用例保持 ignore，fd 上限 16384 另行验证）
- [ ] `cargo test -p easytier-web` + `cargo fmt --check`（本机 MSVC/ring 编译受阻，待 CI 或可用环境验证）
- 说明：dev 已用 `JoinSet` 并发 accept；无新增依赖。

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
- [x] 前置：用户要求继续移植；按 Option 1 适配（不依赖现场复现）。若线上未见 idle TCP 打洞掉线，风险仍低（仅缩短 TCP hole-punch 的 ping 上限）
- [x] 不扩 `PeerConnectionOrigin` 枚举（dev 保持 `Network/Attached`）；保留 `is_hole_punched: bool`
- [x] 在 `PeerConn` 增加 `ping_max_interval: Option<Duration>`；`peer_manager` admission 在 `!is_directly_connected` 且 **非 UDP** tunnel 时写入 `Some(1s)`（覆盖 tcp / faketcp host label；不碰 `HolePunchTunnelSink`）
- [x] `PeerConnPinger` / `PingIntervalController` 增加 `max_interval` cap；默认 32s 保留现有 backoff
- [x] 单测：ping interval controller + tunnel_type 分类 + PeerConn policy
- [ ] `cargo test -p easytier-core` + fmt（本机 MSVC/ring 编译受阻，待 CI 验证）
- 说明：直接合必撞（上游 origin 6 变体 vs dev 2 变体）。仅 hole-punch 会传 `is_directly_connected=false`；用「非 UDP」判定避免 FakeTCP host label 漏匹配。

---

## P2 —— 看实现再定级（先拆零风险部分）

### 4. #2626 QUIC 缓冲池 `perf(quic-proxy): use BufPool and BufMargins in QuicSocket`
- [x] `PacketMargins` → `type PacketMargins = BufMargins`；`try_send` 用 `BufPool::write`；`margins.len()` → `margins.size()`（含 `test_gso`）
- [x] 说明：dev 已有 #2625 的 `BufPool` / `BufMargins`，与上游 #2626 对齐
- [ ] `cargo test -p easytier` 相关用例 + fmt（本机 MSVC/ring 编译受阻，待 CI 验证）

### 5. #2627 WG 缓冲池 `perf(wireguard): rename WG_MAX_PACKET_SIZE and optimize scratch buffers`
- [x] `MAX_PACKET` → `WG_MAX_PACKET_SIZE`（含测试/握手 recv 缓冲）
- [x] `utils/buf.rs` 新增 `FixedBufPool` / `FixedBufGuard` + `test_fixed_buf_pool`
- [x] encapsulate / decapsulate / handshake / routine 路径改用 `WG_BUF_POOL.acquire()`；guard 在 `session.send(...).await` 期间保持存活
- [ ] `cargo test -p easytier` 相关用例 + fmt（本机 MSVC/ring 编译受阻，待 CI 验证）

---

## P3 —— 发版后单开分支集成

### 6. #2622 中央网络控制台 `feat(web): add central network management console`
- [ ] 当前发版线（`releases/v2.7.44` 等）收尾后再开独立集成（分支建议 `integrate/central-network`），从 `main` 拉入解冲突
- [ ] 规模参考：约 101 文件、约 1.9 万加行；冲突重灾区 `client_manager/`、`REST`、`frontend-lib`（正是 dev 改得最多的区域）
- [ ] 前置：P0–P2 落袋并完成 CI/`cargo test` 验证

---

## 非目标（明确不做）

- 不 `merge main → dev`（会带入 README 改指向、sponsor 删除等 fork 行政提交 + ~19k 行中央控制台）。
- 不移植 fork 行政提交（README 改指向、sponsor 删除、仅 CI/Docker 对齐类变更；与 `dev` 已对齐的无需再动）。
- P0–P2 代码已落袋，待 CI/`cargo test` 验证；P3 发版后再开独立分支。
