# CI / ET Test 失败记录（本地清单）

## Status

- Status: **Ops**（追加式日志；提交前必查）
- 最近审阅：2026-10-13
- 索引：[`../README.md`](../README.md)
- 本地复现：[`../../script/easytier-test-fast.cmd`](../../script/easytier-test-fast.cmd) / [`../../script/test-easytier.ps1`](../../script/test-easytier.ps1)
- MSVC 环境：[`windows-msvc-local-build.md`](./windows-msvc-local-build.md)

---

## 1. 提交前自检（强制）

推送 / 开 PR / 触发 ET Test **之前**，按顺序做：

1. **读本文 §3「仍有效的检查项」**，对照本次改动是否会踩同一类坑。
2. **按改动面跑本地命令**（在已加载 vcvars + `script/build-common.ps1` 环境变量的终端里）：

| 改动面 | 最低命令 |
|--------|----------|
| 仅 docs | 跳过 cargo（仍读本文，避免误改代码未查） |
| `easytier-core` / `easytier` | `cargo clippy -p easytier-core -p easytier --bins --tests -- -D warnings` |
| `easytier-web` | `cargo clippy -p easytier-web --bins --tests -- -D warnings` |
| `easytier-web` + 多 crate | `.\script\easytier-test-fast.cmd`（或 `test-easytier.ps1 -Profile Fast`） |
| OpenWrt / Linux / Windows 打包相关 | 至少 `cargo check -p easytier-web`（embed 会编 web） |

3. **环境**：`CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_TARGET_DIR=.cache\cargo-target`、vcvars；勿裸跑导致 ring/`-Z7` 误判。
4. 新失败入库：填 §4 模板，追加到 §5；若已修复，在对应条目标 **已修复** 并移到 §6 或保留并勾选。

Agent / 人工均适用：**未完成 §1 不要提交**。

---

## 2. 如何追加一条记录

复制 §4 模板 → 贴到 §5 顶部（新的在上）→ 填字段 → 更新 §3 检查项（若可写成可重复的防再犯规则）。

字段说明：

| 字段 | 内容 |
|------|------|
| 日期 | `YYYY-MM-DD` |
| 仓库/分支 | 如 `229033891/EasyTier` `dev` |
| Run | Actions URL 或 run id |
| 失败 job | 如 Check clippy / Build / Test (…) |
| 根因一类 | clippy / compile / flaky-test / env |
| 摘要 | 一两句 |
| 防再犯 | 可执行的检查或代码约定 |
| 状态 | 未修复 / 已修复（commit 或说明） |

---

## 3. 仍有效的检查项（提交前扫一眼）

改代码时优先自查：

- [ ] **clippy `clone_on_copy`**：`Uuid` 等 `Copy` 类型不要 `.clone()`，直接传值。
- [ ] **trait 必须 in scope**：调用 `list_network_configs` 等 `Storage` / `PersistentConfig` 方法时，文件顶部要有 `use …::Storage as _`（测试子模块要自己再 import，`as _` 不会经 `super::*` 继承）。
- [ ] **clippy `collapsible_if`**：嵌套 `if let` 能合并则合并，或抽 helper（如 `send_runtime_notify`）。
- [ ] **clippy `too_many_arguments`**：函数 >7 参需打包参数或 `#[allow(clippy::too_many_arguments)]` 并写清原因。
- [ ] **编 `easytier-web` 的路径**：OpenWrt `build-bins`、Linux musl、Windows headless embed 都会编 web；只改 core 绿不够。
- [ ] **本地 clippy 对齐 CI**：`-- -D warnings`，与 ET Test Check clippy 一致。

---

## 4. 条目模板

```markdown
### YYYY-MM-DD — <短标题>

| 项 | 内容 |
|----|------|
| 仓库/分支 | |
| Run | |
| 失败 job | |
| 根因一类 | clippy / compile / flaky-test / env |
| 状态 | 未修复 / 已修复 |

**摘要**：

**防再犯**：

**修复说明**：（可选）
```

---

## 5. 失败记录（新 → 旧）

### 2026-10-12 — Check clippy：`collapsible_if` + `too_many_arguments`

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/38288982393 |
| 失败 job | Check clippy（测试本身绿） |
| 根因一类 | clippy |
| 状态 | 已修复（本地；待推送验证） |

**摘要**：S7 `notifier.send` 写成嵌套 `if let` 触发 `collapsible_if`；S4 给 `Session::new` 增加第 8 参触发 `too_many_arguments`；`runtime_revision` Patch 合并同为嵌套 `if let`。

**防再犯**：嵌套 `if let` 合并或抽 helper；构造函数超 7 参立刻 `allow` 或打包。本地：`cargo clippy -p easytier-web --bins --tests -- -D warnings`。

**修复说明**：`send_runtime_notify`；`Session::new` 上 `#[allow(clippy::too_many_arguments)]`；Patch 分支 `if let … && let …`。

---

### 2026-10-12 — OpenWrt / Linux / Windows：`Storage` trait 未导入

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | OpenWrt `38248929650`；Linux `38248829470`；Windows `38248879394` |
| 失败 job | build-bins / musl build / headless Windows（编 `easytier-web`） |
| 根因一类 | compile |
| 状态 | 已修复 |

**摘要**：`session.rs` heartbeat 填 W 调用 `db().list_network_configs`，未 `use easytier_core::management::remote_client::Storage as _`。

**防再犯**：凡调用 `Storage` trait 方法，同文件（及 `#[cfg(test)]` 子模块）必须导入；改 web 后至少 `cargo check -p easytier-web`。

---

### 2026-10-12 — Check clippy：`clone_on_copy`（Uuid）

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/38244668294 |
| 失败 job | Check clippy（测试绿） |
| 根因一类 | clippy |
| 状态 | 已修复 |

**摘要**：`remote_client.rs` 对 `Uuid`（Copy）使用 `id.clone()`。

**防再犯**：对 Copy 类型直接 `push(id)`；本地 clippy `-D warnings`。

---

## 6. 已归档说明

条目默认留在 §5 并标「已修复」，便于检索。仅当条目过时且被更好规则替代时，再挪到本节并删 §3 对应勾选项。

---

## 7. 决策记录

| 日期 | 决策 |
|------|------|
| 2026-10-13 | 建立本文：ET Test / 打包失败追加记录；提交前强制对照 §1 + §3 |
| 2026-10-13 | Agent 与人工同一套清单；与 Cursor rule「提交前查 CI 失败日志」对齐 |
