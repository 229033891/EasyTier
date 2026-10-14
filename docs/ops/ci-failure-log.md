# CI / ET Test 失败记录（本地清单）

## Status

- Status: **Ops**（追加式日志；提交前必查）
- 最近审阅：2026-10-14
- 索引：[`../README.md`](../README.md)
- 本地复现：[`../../script/easytier-test-fast.cmd`](../../script/easytier-test-fast.cmd) / [`../../script/test-easytier.ps1`](../../script/test-easytier.ps1)
- MSVC 环境：[`windows-msvc-local-build.md`](./windows-msvc-local-build.md)
- ET Test 前置 job：`.github/workflows/test.yml` → `Check fmt & lockfile` / `Check clippy`

---

## 1. 提交前自检（强制）

推送 / 开 PR / 触发 ET Test **之前**，按顺序做：

1. **读本文 §3「仍有效的检查项」**，对照本次改动是否会踩同一类坑。
2. **凡改了 Rust（含 `easytier-gui/src-tauri`）先过 fmt + lockfile**（与 CI `check-fmt` 同命令，不依赖 vcvars）：

```bash
cargo fmt --all -- --check
cargo metadata --format-version 1 --locked > /dev/null
```

本地若 `--check` 失败：先 `cargo fmt --all` 再重跑 `--check`，把格式 diff 一并提交。

3. **按改动面跑本地命令**（在已加载 vcvars + `script/build-common.ps1` 环境变量的终端里）：

| 改动面 | 最低命令 |
|--------|----------|
| 仅 docs | 跳过 cargo（仍读本文，避免误改代码未查） |
| 任意 `.rs` | 上表 `cargo fmt --all -- --check` + lockfile（必做） |
| `easytier-core` / `easytier` | `cargo clippy -p easytier-core -p easytier --bins --tests -- -D warnings` |
| `easytier-web` | `cargo clippy -p easytier-web --bins --tests -- -D warnings` |
| `easytier-web` + 多 crate | `.\script\easytier-test-fast.cmd`（或 `test-easytier.ps1 -Profile Fast`） |
| OpenWrt / Linux / Windows 打包相关 | 至少 `cargo check -p easytier-web`（embed 会编 web） |
| `frontend-lib` / locales `*.yaml` | 确保纯量值里含 ASCII `: ` 时加引号；本地可 `pnpm --filter easytier-frontend-lib build` |

4. **环境**：`CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_TARGET_DIR=.cache\cargo-target`、vcvars；勿裸跑导致 ring/`-Z7` 误判。
5. 新失败入库：填 §4 模板，追加到 §5；若已修复，在对应条目标 **已修复** 并移到 §6 或保留并勾选。

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

- [ ] **`cargo fmt --all -- --check`**：ET Test 最先跑的 job；手改/合并后的换行、长调用链、`use` 排序都会挂。提交前必过；失败则 `cargo fmt --all`。
- [ ] **`Cargo.lock` 与元数据一致**：`cargo metadata --format-version 1 --locked`；改过 `Cargo.toml` 依赖后要更新并提交 lock。
- [ ] **locales YAML 纯量冒号**：`en.yaml` 等纯量值里出现 ASCII `: `（如 `Web source): the …`）必须加双引号；中文全角 `：` 一般没事。否则 `easytier-frontend-lib` / Windows GUI 构建会挂在 `vite:transform-yaml`。
- [ ] **clippy `clone_on_copy`**：`Uuid` 等 `Copy` 类型不要 `.clone()`，直接传值。
- [ ] **trait 必须 in scope**：调用 `list_network_configs` 等 `Storage` / `PersistentConfig` 方法时，文件顶部要有 `use …::Storage as _`（测试子模块要自己再 import，`as _` 不会经 `super::*` 继承）。
- [ ] **clippy `collapsible_if`**：嵌套 `if let` 能合并则合并，或抽 helper（如 `send_runtime_notify`）。
- [ ] **clippy `too_many_arguments`**：函数 >7 参需打包参数或 `#[allow(clippy::too_many_arguments)]` 并写清原因。
- [ ] **编 `easytier-web` 的路径**：OpenWrt `build-bins`、Linux musl、Windows headless embed 都会编 web；只改 core 绿不够。
- [ ] **本地 clippy 对齐 CI**：`-- -D warnings`，与 ET Test Check clippy 一致。
- [ ] **Criterion bench group 名不要含 `/`**：Criterion 落盘会把 `/` 换成 `_`；`script/check_routing_bench.py` 按落盘路径读 `estimates.json`，group 用 `policy_route_empty` / `policy_route_{scale}`。
- [ ] **routing 规模门禁比值用 1.50**：fixture 全是同前缀 `/24`，1K→10K 二分本身约 1.25–1.35×；CI 阈值勿再收紧到 1.20（会误杀）。线性扫描约 10×，1.50 仍挡得住。
- [ ] **桌面专用 Tauri 权限勿进共用 capability**：`autostart:*` 等仅 desktop 依赖提供的权限，必须写在带 `platforms: [macOS, windows, linux]` 的 capability；写进 Android 也会加载的 `migrated.json` 会在 ET Android `tauri build` 报 `Permission … not found`。

---

## 4. 条目模板

```markdown
### YYYY-MM-DD — <短标题>

| 项 | 内容 |
|----|------|
| 仓库/分支 | |
| Run | |
| 失败 job | |
| 根因一类 | fmt / lockfile / clippy / compile / flaky-test / env / frontend-yaml |
| 状态 | 未修复 / 已修复 |

**摘要**：

**防再犯**：

**修复说明**：（可选）
```

---

## 5. 失败记录（新 → 旧）

### 2026-10-14 — Hot-path gates：10K/1K 比值 1.25× 超 1.20

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/38553112317 |
| 失败 job | Hot-path gates → Enforce hot-path thresholds |
| 根因一类 | env（CI 阈值过紧） |
| 状态 | 已修复（`SCALE_DEGRADATION_LIMIT` 1.20 → 1.50） |

**摘要**：路径修复后 gate 真正跑通；`policy_route_hit/miss` 10K/1K 为 **1.25× / 1.27×**，踩 1.20。fixture 全 `/24` 时同桶二分随 N 增 `log2(10)`，约 1.25–1.35× 属预期，非线性扫描回归（约 10×）。

**防再犯**：见 §3「routing 规模门禁比值用 1.50」。

---

### 2026-10-14 — Hot-path gates：`policy_route` criterion 路径找不到

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/38547749833 |
| 失败 job | Hot-path gates (alloc + bench) → Enforce hot-path thresholds |
| 根因一类 | env（gate 路径） |
| 状态 | 已修复（bench group 去 `/` + `check_routing_bench.py` 读 `policy_route_*`） |

**摘要**：`cargo bench` 已跑出 `policy_route/empty/miss` 等结果，但 Criterion 把 group 名里的 `/` 写成 `_`（落盘为 `target/criterion/policy_route_empty/miss/...`），门禁仍找 `policy_route/empty/...`，报 missing `estimates.json`。

**防再犯**：见 §3「Criterion bench group 名不要含 `/`」；改 gate 时对照 `target/criterion` 实际目录。

---

### 2026-10-14 — Windows GUI：`traffic_routing_summary` 未加引号冒号

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev`（本地打包） |
| Run | `artifacts/logs/easytier-windows-20261014-164934.log` |
| 失败 job | 本地 `build-easytier-windows.ps1` → `pnpm … easytier-frontend-lib build` |
| 根因一类 | frontend-yaml |
| 状态 | 已修复（`en.yaml` `traffic_routing_summary` 已双引号包裹） |

**摘要**：`traffic_routing_summary` 纯量含 `steering: policy` / `table: custom`，js-yaml 报 `bad indentation of a mapping entry`（约 226:85），vite `transform-yaml` 失败。

**防再犯**：与 §3「locales YAML 纯量冒号」相同；改 `en.yaml` 后至少 `node -e "require('js-yaml').load(...)"` 或 `pnpm --filter easytier-frontend-lib build`。

---

### 2026-10-14 — Check fmt & lockfile：`cargo fmt --check` 未过

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | ET Test → job **Check fmt & lockfile**（约 2 分钟前失败 / exit 1） |
| 失败 job | Check fmt & lockfile（`cargo fmt --all -- --check`） |
| 根因一类 | fmt |
| 状态 | 已修复（本地已 `cargo fmt --all`，`--check` 通过；待提交推送） |

**摘要**：多处手写/合并后的排版与 rustfmt 不一致，CI 仅做 `--check` 不自动改。涉及文件（本轮 diff）：

| 文件 | 典型差异 |
|------|----------|
| `easytier-core/src/config/api_input.rs` | 长 `assert_eq!(…peers…)` 拆行 |
| `easytier-core/src/gateway/proxy/cidr_monitor.rs` | 布尔赋值并成一行 |
| `easytier-core/src/management/full/mod.rs` | `register_instance_management_rpc(…)` 并成一行 |
| `easytier-core/src/management/full/remote_client.rs` | 函数签名 / `let Some` 链格式 |
| `easytier-core/src/peers/peer_manager.rs` | `validate_and_build_policy_rules`、`.await` 链换行 |
| `easytier-core/src/wasi/web_client.rs` | `gen_config_validated_for_run` 调用换行 |
| `easytier-gui/src-tauri/src/manager.rs` | `use` 排序；`GUIConfig::new_with_runtime_fp` / `emit` / `note_observed_runtime_fp` 参数换行 |

本轮日志未见 `Cargo.lock`/`--locked` 失败；lockfile 步骤仍须在改依赖后本地验证。

**防再犯**：

1. 任意 `.rs` 改动提交前：`cargo fmt --all -- --check`（失败则 `cargo fmt --all`）。
2. 改 `Cargo.toml` 后：`cargo metadata --format-version 1 --locked`。
3. 不要依赖 IDE「看起来整齐」——以仓库 `rustfmt` / toolchain `1.95` 为准（与 CI `setup-rust-toolchain` 一致）。

**修复说明**：本地已执行 `cargo fmt --all`，随后 `--check` 通过。

---

### 2026-10-14 — Windows GUI / frontend-lib：`en.yaml` 未加引号冒号

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev`（本地打包） |
| Run | `artifacts/logs/easytier-windows-20261014-114636.log` |
| 失败 job | 本地 `build-easytier-windows.ps1` → `pnpm … easytier-frontend-lib build` |
| 根因一类 | frontend-yaml |
| 状态 | 已修复（`en.yaml` 相关 tip 已加双引号；见后续 commit） |

**摘要**：`config_source_console_tip` / `config_source_local_tip` 纯量含 `): the` / `config: it`，js-yaml 报 `bad indentation of a mapping entry`（约 1019:62），vite `transform-yaml` 失败。

**防再犯**：英文 tip 含 ASCII `: ` 一律加引号；改 locales 后跑 `pnpm --filter easytier-frontend-lib build`。

---

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
| 2026-10-14 | §1 强制增加与 CI 同命令的 `cargo fmt --check` + lockfile；§3 增加 locales YAML 冒号规则 |
