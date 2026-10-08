# Admin pin 被协议 preference failover 绕过

## Status

- Status: **In progress**（§4.1 **3+2** 已落地；§4.6 **客户端身份命中**已落地；responder 入站 pin 待开工；待 CI / 集成测确认）
- 日期：2026-10-08
- 触发：fork `229033891/EasyTier` 发版 CI **ET Test #107 / #108** 连续失败（`releases/v2.7.46`、`v2.7.47`）；对照 **#106（v2.7.45）全绿**
- 索引：[`../README.md`](../README.md)
- 相关发版：[`../ops/release-version-bump.md`](../ops/release-version-bump.md)

---

## 1. CI 现象（已核实）

| Run | 分支 | 结论 |
|-----|------|------|
| [ET Test #106](https://github.com/229033891/EasyTier/actions/runs/37727502137) | `releases/v2.7.45` | success |
| [ET Test #107](https://github.com/229033891/EasyTier/actions/runs/37739109739) | `releases/v2.7.46` | failure（同两用例） |
| [ET Test #108](https://github.com/229033891/EasyTier/actions/runs/37753715656) | `releases/v2.7.47` | failure（同两用例） |

失败 job：`Test (easytier)`。摘要：`1330 passed, 1 failed, 1 timed out`（#108；job 日志已用 `gh` 独立拉取核实，断言行与采样行原文见 §2–§3）。

| 用例 | 结果 | 分类 |
|------|------|------|
| `credential_rejects_incorrect_admin_pin` | **FAIL** ~1.4s，`!admin_peers.contains(&cred_peer_id)` | **真问题**（安全语义） |
| `credential_non_reusable_across_two_admins_allows_only_one_peer` | **TIMEOUT** 60s | **竞态 flake** |

版本 bump commit 本身无关；与 **2.7.46 起默认协议顺序改为 `udp,tcp` +「配置 URL 优先、preference 作 failover」** 强相关。

---

## 2. 真问题：错误 admin pin 仍能连上

### 2.1 测试意图

`easytier/src/tests/credential_tests.rs` · `credential_rejects_incorrect_admin_pin`：

- Admin 启用 secure mode，credential 节点 `peers` 配 `tcp://10.1.1.1:11010` + **错误的** `peer_public_key`（admin Noise 公钥 pin）。
- 期望：双方 `connected_peers` / 路由中互不可见。

### 2.2 握手侧行为（代码）

客户端 Noise 握手（`easytier-core/src/peers/conn/peer_conn.rs`）：

1. `pinned_remote_static_pubkey`：用 `tunnel_info.remote_addr` 与 `pinned_peers` 做 **URL 精确匹配**，取 pin。
2. 客户端握手分支（`peer_conn.rs`）：
   - `:897` 同网名且 `role_hint != 1` → 直接拒绝（`role_hint must be 1`）。**这是 role 校验，与 pin 命不命中无关**，不要和下面的 pin 路径混淆。
   - `:939` 判定 `SecureAuthLevel`：`role_hint != 1 && pinned == None` → `EncryptedUnauthenticated`（短路径）；否则进 `verify_remote_auth`（`:942`）。
   - 对 **admin pin 未命中**（`role_hint == 1` 且 `pinned_remote_pubkey == None`，例如 failover 到不同 scheme 后对不上 `pinned_peers` 条目）→ 进 `verify_remote_auth`，发起方无 `network_secret` 时走 `:780` 的 `EncryptedUnauthenticated`，**仍可建连**。
   credential→admin 通常同网名 + admin `role_hint==1`，落到上面 admin pin 未命中那条。
3. 若 pin 命中但公钥不匹配 → `pinned remote static pubkey mismatch`，握手失败（符合预期）。

Admin 作为 responder **不查 pin**；合法 credential 公钥仍会被 admin 接受。因此「错误 pin 仍出现在 admin_peers」= **客户端侧未坚持拒绝**。

### 2.3 Preference failover 如何绕过 pin

Manual reconnect（`easytier-core/src/connectivity/manual/mod.rs` + `preference_candidate_urls`）：

- 候选顺序（2.7.46+）：`[配置 URL, …preference 改写的 failover]`。
- 默认 preference：`udp,tcp`。
- 配置为 `tcp://host:port` 时候选约为：`[tcp://…, udp://…]`。
- Tunnel 的 `remote_addr` 记的是 **实际拨打的 candidate URL**，不是「配置 URI」。

错误 pin 挂在 `tcp://…` 上时：

```text
1. dial tcp://…  → pin 命中 → 公钥不匹配 → 握手失败 ✓
2. failover udp://… → URL ≠ pin 条目 → pin 未命中
   → EncryptedUnauthenticated → 建连成功 ✗
```

**结论：带 `peer_public_key` 的对端，在 pin 认证失败后改用不同 scheme 的 failover，等于绕过 admin pin。**

### 2.4 为何 v2.7.45 绿、46/47 红

2.7.46 引入（同发版窗口）：

- 默认协议顺序改为 `udp,tcp`；
- `preference_candidate_urls`：配置 URL 优先，preference 仅作 failover。

旧默认 `["tcp"]` 下 tcp peer 的候选只有 `[tcp]`，udp 一次都拨不到，pin 恒被 enforcement；新默认下「tcp 因错误 pin 失败 → udp 绕过」稳定复现。

---

## 3. Flake：双 admin 非复用凭据可见性超时

`credential_non_reusable_across_two_admins_allows_only_one_peer` 卡在
`wait_stable_failover_visibility_on_admins`（需连续 **3** 次 `stable`）。

#108 日志在 nextest 60s 杀掉时已是：

```text
failover visibility: … stable=true samples=2
```

差一次采样。独立性**未证伪**：同一改动（udp 优先）同样可能拖慢收敛。先修 pin 再看是否自愈；只放宽 timeout，不把 samples 降到 2（那是削弱测试）。

---

## 4. 建议修复

### 4.1 产品语义（优先）— **已落地 3+2**

原候选：

1. **硬规则**：`peer_public_key` 已配置时禁止 scheme failover（未采用；正确 pin 时仍允许传输层 failover）。
2. **查 pin 用配置 URI**：failover 握手仍按配置 peers URI 查 pin。
3. **认证失败分类**：pin mismatch 类型化，不再进 preference 下一候选。

**已实现（3+2）：**

- `peers::Error::PinnedRemotePubkeyMismatch` + `anyhow_is_pinned_remote_pubkey_mismatch`（禁止字符串匹配）。
- `verify_remote_auth` 公钥不匹配时返回该变体。
- `manual::reconnect` / `reconnect_with_ip_version`：遇 pin mismatch **立即返回**，不拨下一 scheme。
- `apply_resolved_endpoint_info`：始终把 `TunnelInfo.remote_addr` 设为**配置 URI**（dial candidate 留在 `resolved_remote_addr`），正确 pin 时 udp 兜底仍 enforcement。

### 4.2 回归

- 保留 / 加强 `credential_rejects_incorrect_admin_pin`（默认 `udp,tcp` 下必须红→绿）。
- 可选：显式用例「错误 pin + preference 含 udp」断言握手失败且无 `connected_peers`。

### 4.3 Flake（次要）

- 双 admin 超时：略增 timeout；**不要**把 `stable_samples` 降到 2（与 §3 一致，降采样会削弱测试）。勿用重跑掩盖 §2。

### 4.4 发版

不宜仅靠重跑把 #107/#108 当绿。pin 绕过属安全语义缺口，应修后再标 release CI 可信。

### 4.5 直连 / responder 路径（已确认：敞口，非阻塞）

代码层已确认 pin 是 **按 URI 绑定、且 responder 完全不查**：

- Responder 硬编码 `pinned_pubkey = None`：`peer_conn.rs:1177-1184`（`do_noise_handshake_as_server`，注释 `Server doesn't use pinned_pubkey since it's the responder`）。`verify_remote_auth` 在 responder 侧永远跳过重查 pin。
- pin 查找按 `tunnel_info.remote_addr.url` 精确匹配 `pinned_peers`（`context.rs:872-883`）。只有「拨打/接收地址 == 配置的 manual peer URI」时才命中。
- 直连通道真实存在：`connectivity/direct/mod.rs`（`direct/udp.rs`），其 tunnel `remote_addr` 是解析出的真实地址（`resolve_remote_addr` / `resolve_literal_url`），**不会**等于 manual URI → pin 查不到 → `None`。
- **所有客户端侧 tunnel 共用一个漏斗**：manual / 直连 / 打洞 / relay 都经 `peer_manager.rs:2019` `add_client_tunnel_with_origin` → `do_handshake_as_client()` → 同一个 `do_noise_handshake_as_client`。即打洞、relay 的 `remote_addr` 同样不是 manual URI，**今天也一样查不到 pin**；§4.5 #1 的范围实际是「一切非 manual 拨号路径」，不止直连。

因此即便 manual 这条路被 §4.1 修好，下面两类仍绕得过 admin pin：

1. 客户端经**直连 / 打洞 / relay**等非 manual 路径连 admin（路由交换学到的 endpoint）：客户端 `pinned_remote_static_pubkey` 用真实地址查 → 不命中 → `EncryptedUnauthenticated` 建连。
2. admin 作为 **responder** 接收任意入站直连：本身就不查 pin。

影响面：生产多节点网络（路由交换后互知真实地址）才触发；当前 `credential_rejects_incorrect_admin_pin` 是 2 节点 manual 拓扑，直连不会先建起，所以本修复足以让 CI #107/#108 转绿。属安全语义缺口，但**非本次发版阻塞项**。

### 4.6 直连 / responder 的修复方案（客户端身份命中 **已落地**；responder 仍待开工）

根因：pin 绑定在「传输层 URI」而非「对端身份（Noise static pubkey）」。直连真实地址对不上 `pinned_peers` 的 URI → 查不到 pin → 客户端落到 `EncryptedUnauthenticated`。下面方案基于对代码的核对（见 §5 锚点）。

> **已否决**：把全部 `Some(pk)` 收成全局集合，且「集合非空则对端必须是成员，否则拒」。这会把「按 URI 可选 pin」变成「全网 allowlist」——只 pin 了部分 peer 时，未 pin 的 SharedNode / 其它 peer 会被误拒；responder 若复用出站 pin 集合，还会在 `is_pubkey_trusted` 之前挡掉已信任的入站 credential。

#### 4.6.1 思路（保留可选 pin 语义）

`pinned_peers: Vec<(Url, Option<String>)>`（`config/peers.rs:240` / snapshot `context.rs:79`）语义不变：某 dial URI 上的 `peer_public_key` 仍是**该条目**的期望公钥，不是全网唯一允许名单。

客户端握手拿到 `remote_static` 后，step 2 按优先级：

1. **URI 命中 pin**（现有 `pinned_remote_static_pubkey`）：与 `remote_static` 不符 → `PinnedRemotePubkeyMismatch`；相符 → `PeerVerified`。
2. **URI 未命中，但 `remote_static` 等于某个已配置条目的 `Some(pk)`**（用 `BASE64_STANDARD` 与配置同 codec）：视为「直连打到了已知 pin 身份」→ `PeerVerified`。这样真实地址 / 直连不再依赖 URL 精确匹配。
3. **其余**（无 URI pin，且 remote 对不上任何已配置 pin）→ **跳过 step 2**，继续走 step 3 `is_pubkey_trusted` / step 4 `EncryptedUnauthenticated`。**不因「别处配了 pin」而全局拒绝。**

可选派生只读索引（实现细节，非配置变更）：

```rust
// snapshot 内派生，便于 O(1) 查「remote 是否等于某个已 pin 公钥」（已按此实现）
pub pinned_pubkey_index: HashSet<[u8; 32]>, // Some(pk) 解码后的 32 字节，不作 deny-if-absent
```

**索引按解码后的 32 字节存，不按原始字符串存**：`pinned_peers` 的 pk 是配置字符串**原样克隆**进 runtime 的（`instance/config.rs:243`，无校验、无归一化），非规范 base64（URL-safe、缺 padding、带空白）会让字符串比对**静默失配** → 身份命中漏掉 → 落回 `EncryptedUnauthenticated`。实现即 `build_pinned_pubkey_index`（`config/peers.rs`）：解码、非法条目 skip + warn、比对直接用 `remote_static` 的 32 字节，全程不需要 encode（与 §4.6.3 的 codec 注一致）。

§4.1 的 URI 强制与本方案**可并存**：§4.1 保 failover 路径上 URI 命中；本方案补直连 / 真实地址路径上的身份命中。

**单点全覆盖**：身份命中在 `do_noise_handshake_as_client` 一处生效即可——manual / 直连 / 打洞 / relay 全部经 `add_client_tunnel_with_origin` 走同一个握手（见 §4.5），改一处、四种传输全兜住。

#### 4.6.2 改动清单

1. **客户端 pin 解析**（`peer_conn.rs` / `context.rs:872`）：
   - 保留 URI 查表（`get_pinned_remote_static_pubkey_b64` / `pinned_remote_static_pubkey`）。
   - 新增「按 `remote_static` 是否落在任一已配置 pin 值上」的查找（可放 `context`：遍历 `pinned_peers` 的 `Some(pk)`，或维护上面的 `pinned_pubkey_index`）。
   - 在 `instance/config.rs:243` 填充、`attached.rs:429` `clear()` 后同步维护索引；单测 `context.rs:1123` / `:1203` 补默认空索引。

2. **`verify_remote_auth` step 2**（peer_conn.rs:744）保持 `Option<&[u8]>` 形态即可，语义扩展为调用方传入的「生效 pin」：
   - 调用方先算：`uri_pin`；若无，再看 `remote_static` 是否命中 `pinned_pubkey_index`（命中则把该 pk decode 后当作 pin，得到 `PeerVerified`）；都无则传 `None`。
   - step 2 本体仍是：`Some(pinned)` 时比对，不符则 `PinnedRemotePubkeyMismatch`；`None` 则落入 trusted。
   - **不要**改成「集合非空必须成员」。
   - `:939` 短路径保持「无生效 pin」才走 `EncryptedUnauthenticated`（`pinned_remote_pubkey.is_none()`），不要改成 `pinned_pubkeys.is_empty()`。

3. **Responder（§4.5 #2）单独设计，不复用出站 pin 集合**：
   - 今日同网名入站已走 `verify_remote_auth`（`:1179`，`role_hint == 1`）；未信任 credential 多在 step 5 被拒；`:1184` 的 `None` 表示**出站 `peer_public_key` 不自动变成入站锁**。
   - 若产品要「admin 只接受若干 credential 公钥」，需**显式入站策略**（新配置或明确语义），再传入 step 2；禁止把 dial-out `pinned_peers` 直接塞进 `:1184`。
   - 本小节若只先堵客户端直连绕过，responder 可标为 follow-up，避免一次 PR 混两套语义。

#### 4.6.3 向后兼容 / 风险

- `pinned_peers` 配置结构不变 → **schema 零改动**。
- 未配任何 `peer_public_key` → 行为与现状一致。
- 只 pin 部分 peer 时：未 pin 的连接仍可走 trusted / `EncryptedUnauthenticated`（相对「全局集合强制」无回归）。
- 错误 pin + 直连真实地址：URI 可能对不上，但 remote 也**对不上**错误 pin 值 → 仍无「生效 pin」→ 仍可能落到 `EncryptedUnauthenticated`。若产品要求「只要配置里出现过 pin，拨到未知身份也必须拒」，那是**更强策略**，需单独产品确认，不能默认等同于本方案；测试要写清期望。
- codec：编码/比对用现有 `BASE64_STANDARD`（`peer_conn.rs` 已有 `decode_b64_32`），不要假设存在 `encode_b64_32`。

#### 4.6.4 测试

- 扩展 / 变体 `credential_rejects_incorrect_admin_pin`：credential **经直连拨 admin 真实地址**；在「URI 命中错误 pin」路径上仍拒。若走真实地址且 URI 未命中，按 §4.6.3 写明期望（仅身份命中升级 / 或另开「强策略」用例）。
- 回归：同网同时存在「带 `peer_public_key` 的 admin peer」与「不带 pin 的 SharedNode peer」时，SharedNode 仍可建连。
- Responder 用例仅在入站策略方案确定后加；未信任入站被拒可依赖现有 trusted / step 5 行为，不必绑出站 pin。
- 单测：URI 命中不符 → 拒；URI 未命中但 remote 等于某 pin → `PeerVerified`；二者皆无 → 落入后续步骤。

#### 4.6.5 排期 / 落地状态

- **客户端身份命中（堵 §4.5 #1 的「正确 pin + 非 manual 路径」）已落地**：`pinned_pubkey_index`（`HashSet<[u8; 32]>`）+ `remote_matches_configured_pin`；`do_noise_handshake_as_client` 在 URI pin 缺失时用身份命中作为生效 pin。
- **responder 入站 pin**仍待产品定义后另 PR（不复用出站 `pinned_peers`）。
- 与 §4.1 并存；错误 pin + 非 URI 命中路径仍可能 `EncryptedUnauthenticated`（见 §4.6.3 强策略说明）。

---

## 5. 代码锚点

| 区域 | 路径 |
|------|------|
| 失败用例 | `easytier/src/tests/credential_tests.rs`（`credential_rejects_incorrect_admin_pin`、`credential_non_reusable_across_two_admins_allows_only_one_peer`） |
| pin 查找 | `easytier-core/src/peers/context.rs` · `pinned_remote_static_pubkey` / `remote_matches_configured_pin` |
| pin 身份索引 | `easytier-core/src/config/peers.rs` · `build_pinned_pubkey_index` / `PeerRuntimeSnapshot::pinned_pubkey_index` |
| pin 类型化错误 | `easytier-core/src/peers/error.rs` · `PinnedRemotePubkeyMismatch` / `anyhow_is_pinned_remote_pubkey_mismatch` |
| responder 不查 pin | `easytier-core/src/peers/conn/peer_conn.rs` · `do_noise_handshake_as_server` `:1177-1184` |
| 客户端 tunnel 漏斗 | `easytier-core/src/peers/peer_manager.rs` · `add_client_tunnel_with_origin` `:2019`（manual/直连/打洞/relay 共用） |
| 直连通道 | `easytier-core/src/connectivity/direct/mod.rs`（`direct/udp.rs`） |
| 握手鉴权 | `easytier-core/src/peers/conn/peer_conn.rs` · `verify_remote_auth` / `do_noise_handshake_as_client` |
| failover | `easytier-core/src/config/protocol_preference.rs` · `preference_candidate_urls` |
| reconnect | `easytier-core/src/connectivity/manual/mod.rs` · `reconnect` / `reconnect_candidate` |
| pin 落盘到 runtime | `easytier-core/src/instance/config.rs` · `pinned_peers` |
