# CI / ET Test 失败记录（本地清单）

## Status

- Status: **Ops**（追加式日志；提交前必查）
- 最近审阅：2026-10-18
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
| `frontend-lib` / locales `*.yaml` / `Status.vue` tip | 确保纯量值里含 ASCII `: ` 时加引号；多行 tip 用 `<br>` + `escape: false`（勿改成 `true`）；本地至少 `pnpm --filter easytier-frontend-lib exec vue-tsc -b` |

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

- [ ] **常量 `assert!` 用 `const { assert!(…) }`**：对 `const`/`static` 的恒真断言会触发 clippy `assertions_on_constants`（`-D warnings`）。例：`FORWARD_EGRESS_QUEUE_CAP > 32` → `const { assert!(…) }`（见 `forward_egress` 测例）。
- [ ] **`cargo fmt --all -- --check`**：ET Test 最先跑的 job；手改/合并后的换行、长调用链、`use` 排序都会挂。提交前必过；失败则 `cargo fmt --all`。
- [ ] **`Cargo.lock` 与元数据一致**：`cargo metadata --format-version 1 --locked`；改过 `Cargo.toml` 依赖后要更新并提交 lock。
- [ ] **locales YAML 纯量冒号**：`en.yaml` 等纯量值里出现 ASCII `: `（如 `Web source): the …`）必须加双引号；中文全角 `：` 一般没事。否则 `easytier-frontend-lib` / Windows GUI 构建会挂在 `vite:transform-yaml`。
- [ ] **PrimeVue tip 多行**：返回类型若写 `{ escape: false }`，实现必须是 `escape: false` + `lines.join('<br>')`（见 `rowLatencyTip`）。写成 `escape: true` 会 `vue-tsc` TS2322，挂掉 ET Windows / Android 的 `prepare-build`。
- [ ] **clippy `clone_on_copy`**：`Uuid` 等 `Copy` 类型不要 `.clone()`，直接传值。
- [ ] **trait 必须 in scope**：调用 `list_network_configs` 等 `Storage` / `PersistentConfig` 方法时，文件顶部要有 `use …::Storage as _`（测试子模块要自己再 import，`as _` 不会经 `super::*` 继承）。
- [ ] **clippy `collapsible_if`**：嵌套 `if let` 能合并则合并，或抽 helper（如 `send_runtime_notify`）。
- [ ] **clippy `too_many_arguments`**：函数 >7 参需打包参数或 `#[allow(clippy::too_many_arguments)]` 并写清原因。
- [ ] **`Vec<f64>` 不要 `push(0)`**：整数字面量是 `i32`，ET Windows GUI embed 编 `easytier-web` 会 E0308。写 `0.0`。
- [ ] **测试 helper 返回类型跟 `try_get` 一致**：`conn_id` 等字符串列写成 `i64` 会在 `clippy --tests` 报 E0277（元组类型对不上 / `i64` 与 `&str` 比较）。改 SQL 列类型后同步改 helper 与断言。
- [ ] **SQL 时间窗必须带显式上界**：查询采样/历史表时，除下界 `sampled_at >= ?` 外还必须有上界 `sampled_at <= ?`。节点时钟超前时写入的行「来自未来」，没有上界它们会落进**每一次**查询的窗口，把曲线整体抬高（见 `docs/current/tunnel-ip-flow-fixes.md` F-2）；删上界时必须同步删掉对应的「未来行被忽略」回归测试。
- [ ] **编 `easytier-web` 的路径**：OpenWrt `build-bins`、Linux/Windows `build-web-embed` 都会编 web；只改 core / 无头绿不够。GUI 打包不编 web，但改 frontend-lib 仍影响 GUI。
- [ ] **本地 clippy 对齐 CI**：`-- -D warnings`，与 ET Test Check clippy 一致。
- [ ] **Criterion bench group 名不要含 `/`**：Criterion 落盘会把 `/` 换成 `_`；`script/check_routing_bench.py` 按落盘路径读 `estimates.json`，group 用 `policy_route_empty` / `policy_route_{scale}`。
- [ ] **routing 规模门禁比值用 1.50**：fixture 全是同前缀 `/24`，1K→10K 二分本身约 1.25–1.35×；CI 阈值勿再收紧到 1.20（会误杀）。线性扫描约 10×，1.50 仍挡得住。
- [ ] **桌面专用 Tauri 权限勿进共用 capability**：`autostart:*` 等仅 desktop 依赖提供的权限，必须写在带 `platforms: [macOS, windows, linux]` 的 capability；写进 Android 也会加载的 `migrated.json` 会在 ET Android `tauri build` 报 `Permission … not found`。
- [ ] **Linux/macOS 编 `easytier-gui` 前须有 `binaries/ET-cli-<host>`**：`tauri.*.conf.json` 的 `externalBin` 会在 build-script 校验路径；ET Test `clippy --all` 也走这条。CI 由 `prepare-build`（`gui: true`）放 stub；正式打包再 stage 真 CLI。本地：`mkdir -p easytier-gui/src-tauri/binaries && touch …/ET-cli-$(rustc -vV | awk '/^host:/{print $2}')`。
- [ ] **proto 注释勿让续行以 `+`/`-`/`*` 开头**：prost 会把字段注释编成 `///`；Markdown 把行首 `+` 当成列表，后续未缩进行触发 clippy `doc_lazy_continuation`（`-D warnings` 直接挂）。改写成逗号并列，或续行缩进两空格。
- [ ] **`#[cfg(test)]` helper 若无断言引用也算 dead_code**：CI 编 lib test 时 `cfg(test)` 为真，未调用的 `pub(super)`/`fn` 仍触发 `-D dead-code`。helper 写了就要在同文件测试里用，否则删掉。
- [ ] **`mod tests` 调父模块私有函数用 `super::`**：unix-only DB 权限 helper 等，裸名在子模块不可见，Linux CI 会 E0425。
- [ ] **数据面 `send_msg` 勿纯 `try_send`**：无有界等待时 TCP 段/UDP 分片突发会整波 `BufferFull`，dataplane 大包单测与现网突发都会挂；keepalive 的 `try_send`（LocalCongestion）可保留。
- [ ] **attached 直连测试清 `is_hole_punched`**：`PeerConn::new*` 默认 hole-punched=true；`has_direct_attached_conn` 要求 `!is_hole_punched && is_attached`，测试需与 `peer_manager` 准入一致地 `set_is_hole_punched(false)`。
- [ ] **Status 前端单测对齐 Panel/i18n**：节点详情是 `node_info_details` Panel + `node_info_group_udp_nat_type`，不要再找 `show_node_details` / `UDP NAT Type:` 硬编码。
- [ ] **删 UI 控件后清 unused import**：`vue-tsc -b` 开 `noUnusedLocals`（TS6133）。去掉按钮/组件后同步删 `primevue` 等 import（如 `Button`）；改 `Status.vue` 后至少 `pnpm --filter easytier-frontend-lib exec vue-tsc -b`。
- [ ] **改 dataplane/WASI guest 后刷新嵌入 wasm**：ET Go 跑的是 `easytier-go/internal/artifact/easytier_core.wasm`，不是仓库现编 Rust。Close→FIN（`DataPlaneTcpStream::Drop`）等行为若只改源码不 `go generate`，Go 测会报 `FIN not sent`。artifact/`proto` provenance 的 Commit 必须一致。
- [ ] **Android release 须开 v1+v2 签名**：仅有 APK Sig Block（v2）而无 `META-INF/*.RSA`（v1）时，不少国产安装器报「解析失败 / 没有签名文件」。`signingConfigs.release` 显式 `enableV1Signing = true` / `enableV2Signing = true`；发版前可 unzip 检查是否含 `META-INF/*.RSA`。
- [ ] **OpenWrt LuCI `.htm` 必须 LF**：LuCI 把模板正文塞进 `write("…")` 时只转义 `\n`，不转义 `\r`。Windows 上打出的 CRLF apk 会让状态页报 `unfinished string near '"'`。打包前 `sed -i 's/\r$//'`；`luci-app-easytier` 有 `Build/Prepare` 钩子与 `.gitattributes` `eol=lf`。
- [ ] **数据面 next_hop 要对齐 `latency_first`**：默认 `latency_first=true` 时发包走 LeastCost（`next_hop_peer_id_latency_first`），不是 LeastHop（`next_hop_peer_id`）。拓扑类测试若只等 LeastHop，新建边在 peer-center 仍为默认 500 时 LeastCost 会继续走已测速的 admin 路径，`assert_ping_forwarded_by` 会误报 forbidden forwarder。`wait_route_next_hop` 必须按实例 flag 选表；纯拓扑用例可 `disable_latency_first`。生效值要跟 `PeerPacketPolicy::from_flags` 一致：`flags.latency_first && !flags.p2p_only`（`p2p_only` 会强制关掉时延优先）。

- [ ] **改 workflow / composite action 时：网络类步骤必须有硬超时**：`apt-get update` 撞上半开连接会无限等（ET Linux #169 白烧 4h41m）。要点：`Acquire::http::Timeout` 默认 120s 且是**空闲超时**，连接 trickle 就永不触发，必须压到 20s 左右；job 级一律写 `timeout-minutes`；**composite action 的 step 不支持 `timeout-minutes`**，必须用 `sudo timeout --kill-after=30 300 apt-get …` 命令级兜底（`--kill-after` 防 SIGTERM 被忽略）。详见 §5 的 2026-11-10 条目。

- [ ] **往 `container:` 路径上放 composite action 前，先搜 `hashFiles` 和 `git `**：容器 job 里 `hashFiles()` 在 post 阶段按宿主机路径解析工作区会直接失败，且失败发生在清理阶段——构建成功、artifact 已上传，job 照样变红，极易误判成编译问题；容器内 git 也可能因 `dubious ownership` 返回 128（`safe.directory` 只写进临时 HOME）。两者都在普通 runner 上正常，只在容器里爆。详见 §5 的 2026-11-10 条目。

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

### 2026-11-10 — ET Linux：apt 镜像不可达把 build-gui / build-web-embed 挂死 4h41m

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/42762937112（`ET Linux #169`，workflow_dispatch，最终 cancelled） |
| 失败 job | build-gui、build-web-embed |
| 根因一类 | env / apt 镜像不可达（**不是编译慢**） |
| 状态 | 已修复 |

**摘要**：整条运行记 4h42m42s，其中约 4h41m 是纯等待，没有任何编译发生。

- `build-gui` 的 `Prepare build environment` 步骤 `duration_ms=16943373`（4h42m23s）；`build-web-embed` 同步骤 4h38m。两者都停在 `Install dependencies` 的 `apt-get update`。
- 日志特征：`Ign:2/3/4/5 http://azure.archive.ubuntu.com/ubuntu …` 连续重试 → 回退 `https://archive.ubuntu.com` 后最后一行是 `Get:5 … noble-security InRelease [126 kB]`，之后**零输出**直到 cancel。
- 对照：同一次运行的 `build`（无头 musl）跑**同一个步骤**只用 `duration_ms=10564`（10.5s），编译 9m28s、全 job 10m32s。可见代码与配置并未变慢，是 runner 到 Azure apt 镜像的半开连接 + apt 没有总超时。

**根因（为什么它会「卡死」而不是报错）**：

1. `apt-mirrors.txt` 是**优先级列表，Azure 排第一**。Azure 镜像不可达时 apt 逐条 `Ign:`，然后 failover 到公网 `archive.ubuntu.com`——而公网 archive 并没有为该 fleet 做 provision，传输经常停在某个索引文件中途（本仓库与多个上游项目都停在同一个 `noble-security InRelease [126 kB]`）。
2. **决定性因素**：`Acquire::http::Timeout` 默认为 120s，且它是**空闲超时（idle timeout）**。连接只要还开着、偶尔滴几个字节，就永远不触发——apt 会一直等下去。所以「半开连接 + trickle」的组合下，apt 既不报错也不退出。
3. 附带因素：runner 有 AAAA 记录但 IPv6 路由不可用，`connect()` 会先卡 IPv6 再回落 IPv4，进一步放大等待。
4. 缺兜底：`apt-get update` 是裸命令，composite action 的 step 又不支持 `timeout-minutes`，于是没有任何东西能终止它。

**防再犯（分两层：根治 + 兜底）**：

*根治——让正常路径不碰 apt 索引*：卡死只发生在 `apt-get update`（要拉 20+ 个索引文件，每个都是一次可能 stall 的连接），而 `apt-get install` 只下载 2–3 个 `.deb`。runner 镜像自带 `/var/lib/apt/lists`，所以先直接 `install`，成功就零索引请求；只有包名解析不到（本地索引不足）时才回退到有界 `update`。实测 runner 预装清单里 `gcc/g++/make` 已有，缺的只有 `mold` / `musl-tools` / webkit 系列。

*兜底——即使必须联网也快速失败*：
- 把 `Acquire::ForceIPv4` / `Retries=3` / `http(s)::Timeout=20` 写进 `/etc/apt/apt.conf.d/99-et-ci-failfast`，而不是只作命令行参数——这样第三方 action 内部自己跑的 apt 也自动受保护。
- `apt-mirrors.txt` + `sources.list` + `sources.list.d/*` 一并换回官方 HTTPS 源（含 `ports.ubuntu.com`）。
- `timeout --kill-after=30`：`--kill-after` 兜住 SIGTERM 被忽略的情况（实测忽略 TERM 的进程 1s 后被 SIGKILL、退出码 137）。composite action 的 step 不支持 `timeout-minutes`，只能用命令级。
- `linux.yml` / `windows.yml` 全部 job 加 `timeout-minutes`。

*第二层根治——预构建 CI 镜像（已落地）*：`.github/docker/ci/Dockerfile` 把 `build-essential` / `mold` / `musl-tools` / webkit 全家桶 / Rust 1.95（含 musl、wasm target）/ protoc 35.1 全部烘进 `ghcr.io/<owner>/easytier-ci`，构建 job 用 `container:` 直接起镜像，**构建期一次 apt 都不执行**。镜像构建期间同样带 apt 四项防护，损坏的是镜像构建而不是业务 CI。

- 构建/推送：`.github/workflows/ci-image.yml`（Dockerfile 变更、每周一 03:00 UTC、手动触发）。
- 启用方式：仓库变量 `ET_CI_IMAGE=ghcr.io/<owner>/easytier-ci:latest`。**未设变量时表达式解析成 `{"image":null}`，等价于直接在 runner 上跑**——即一键回退通道，删掉变量即可。
- 镜像内写 `/etc/easytier-ci-image` 作为哨兵，`prepare-build` 命中后跳过全部 apt 步骤并校验关键工具；工具缺失会明确报错而不是等到链接阶段。
- 预期：环境准备从 3–5min 降到 10–20s，且彻底免疫 apt 索引僵死。

*容器化实测数据*（2026-11-10，同一 commit 对比）：

| 运行 | 方式 | 总时长 |
|---|---|---|
| #169 42762937112 | 修复前 | 4h42m（手动 cancel） |
| 42802980322 | 修复后 / 普通 runner | 24m03s |
| 42812330906 | 修复后 / CI 镜像 | **16m35s** |

`Prepare build environment` 变化：`build` 10.5s → 75s（新增 install 探测）→ **4s**（镜像）；`build-gui` 4h41m → 5m44s → 4m34s（余下是 pnpm 前端构建的真实工作量）。容器初始化约 31–40s。

*容器 job 独有的两个坑*（都只在 `container:` 模式下出现，普通 runner 上一切正常）：

1. **`hashFiles()` 在 composite action 的 post 阶段必失败**：
   ```
   The template is not valid. ...prepare-pnpm/action.yml (Line: 73, Col: 14):
   hashFiles('**/pnpm-lock.yaml') failed. Fail to hash files under directory
   '/home/runner/work/<repo>/<repo>'
   ```
   post 阶段 runner 会重新求值每个 step 的 inputs，此时 glob 按**宿主机**路径解析。后果极具迷惑性：所有构建步骤成功、artifact 也上传了，job 却因为一个清理步骤变红。解法是把哈希算进 `$GITHUB_ENV`，让 post 阶段只读 env（`env.STORE_PATH` 在 post 阶段可用，说明这条路可行）。
2. **容器内任何 git 命令都可能返回 128**：
   ```
   fatal: detected dubious ownership in repository at '/__w/<repo>/<repo>'
   ```
   runner 只把 `safe.directory` 写进临时 HOME（`/__w/_temp/<uuid>`），后续步骤用 `/github/home`，看不到该配置。所以「算指纹」这类工作**不能用 `git ls-files`**，要用 `find` + `sha256sum` 纯文件哈希。

结论：往容器化路径上放 composite action 时，先搜一遍 `hashFiles` 和 `git ` 调用。

**排查方法**：先看**步骤级** `started_at → completed_at`，不要只看 job 总时长——`Prepare build environment` 里串了 apt / protoc / rust / pnpm 四件事，极易被误判成「编译慢」。同一次运行内做对照最有效（#169 里 `build` 跑同一步骤只用 10.5s）。

**修复说明**：`.github/actions/prepare-build/action.yml`、`.github/workflows/linux.yml`、`.github/workflows/windows.yml`。

**同批一并处理**：
- `build-web-embed` 合并原 `build_web`（前端 dist 就地构建），去掉 `needs` 串行等待与 `ET-web-dashboard` artifact 往返（`release.yml` 对该 artifact 只有 `rm -rf` / `continue`，不存在也安全）。
- `releases/**` 的 `codegen-units` 由 `1` 调到 `4`（fat LTO 保留）：LTO 合并阶段本就单线程，CGU 只影响 codegen 并行度。
- `windows.yml` 全部 job 补 `timeout-minutes`（build_web 30 / build-gui 150 / build-headless 120 / build-web-embed 120 / result 10）。

**关于 sccache 的更正**：`build` job 报 `Rust cache_hits=0` / `misses=462`，一度被当作「sccache 失效」。但本 fork 上 ET Linux 此前**只跑过 #169 这一次**，首次冷启动必然 0 命中，不能据此下结论。`cache_writes=780` 说明缓存已写入，需第二次运行看命中率再决定是否改配置。`cache_write_errors=20` 与 GHA cache 的 10GB 上限是后续要盯的点。

### 2026-10-28 — ET Test clippy：`binaries/ET-cli-<linux-host>` 缺失

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/40657056705 |
| 失败 job | Check clippy |
| 根因一类 | env / tauri externalBin |
| 状态 | 已修复 |

**摘要**：桌面 `tauri.*.conf.json` 增加 `externalBin: binaries/ET-cli` 后，Linux 上 `cargo clippy --all` 编 `easytier-gui` 时 build-script 要求 `binaries/ET-cli-x86_64-unknown-linux-gnu` 存在。

**防再犯**：`prepare-build` 在 `gui: true` 时 stage stub（见 §3）；打包路径继续 stage 真 CLI。

**修复说明**：`.github/actions/prepare-build/action.yml` 增加 stub 步骤。

### 2026-10-27 — Android APK：仅 v2 签名导致安装「没有签名文件」

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `releases/v2.7.9` |
| Run | Release `v2.7.9` / ET Android `40506438992` |
| 失败 job | 用户安装（非 CI 红） |
| 根因一类 | android-signing |
| 状态 | 已修复（Release APK 补 jarsigner v1；gradle 强制 v1+v2） |

**摘要**：产物含 `APK Sig Block 42` 但无 `META-INF/*.RSA`。部分 OEM 安装器只认 JAR v1，报解析失败/没有签名文件。

**防再犯**：见 §3「Android release 须开 v1+v2 签名」。

### 2026-10-27 — ET Go：嵌入 wasm 过期导致 Close 不发 FIN

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `releases/v2.7.9` / `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/40493996791 |
| 失败 job | Test Go modules |
| 根因一类 | stale-artifact / wasm |
| 状态 | 已修复（按 HEAD 重建 `easytier_core.wasm` + provenance；本地 FIN 三测已绿） |

**摘要**：Go `Close` 走嵌入 guest 的 `close_resource`。源码已有 `DataPlaneTcpStream::Drop`→`stream.close()` 发 FIN，但 artifact provenance 仍指向本仓库不存在的 `599e4eac…`，guest 不发 FIN，对端 Read 超时（`want io.EOF`）。

**防再犯**：见 §3「改 dataplane/WASI guest 后刷新嵌入 wasm」。

### 2026-10-27 — 全线打包：Status.vue 未用 `Button` 导致 vue-tsc TS6133

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `releases/v2.7.9`（`ed14e5d2` 同步） |
| Run | https://github.com/229033891/EasyTier/actions/runs/40489145962 等（Windows/Linux/Android/Test/OpenWrt） |
| 失败 job | Windows `build-gui`；Linux `build_web`；Android `build-mobile`；Test `Build test`；OpenWrt `build-bins` |
| 根因一类 | frontend / vue-tsc |
| 状态 | 已修复（去掉 `Button` import；`vue-tsc -b` 已绿） |

**摘要**：`ed14e5d2` 去掉节点详情复制按钮，但 `Status.vue` 仍 `import { …, Button, … } from 'primevue'`。`vue-tsc -b` 报 `TS6133: 'Button' is declared but its value is never read`，所有编 frontend-lib 的打包/测试 job 在 prepare-build 即 exit 2。

**防再犯**：见 §3「删 UI 控件后清 unused import」。

### 2026-10-27 — ET Linux / ET Test：前端 Status 单测 + clippy/DB 测试 + try_send 突发

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/40476840157 、https://github.com/229033891/EasyTier/actions/runs/40476849720 |
| 失败 job | Linux `build_web`；Test `Check clippy` / `Test (easytier)` |
| 根因一类 | frontend-test / clippy / compile / regression |
| 状态 | 已修复（本地未推送） |

**摘要**：
1. `status-rendering.spec.ts` 仍点旧按钮 `show_node_details` / 断言 `UDP NAT Type:`，UI 已改为一级 Panel `node_info_details` + i18n key。
2. `db/mod.rs` unix 单测在子模块里裸调私有 `sqlite_database_path` / `restrict_database_sidecar_permissions`（需 `super::`）。
3. clippy `match_like_matches_macro`（`should_write_device_archive`）；`build_udp_followup_fragment` dead_code。
4. `PeerConn::send_msg` 纯 `try_send` 使 dataplane 大包/TCP 突发直接 `BufferFull` 超时；改为 100ms 有界 `send`。
5. `peer_channel_uses_admission_origin_*` 未 `set_is_hole_punched(false)`，`has_direct_attached_conn` 恒假。

**防再犯**：
- 前端改 Panel/i18n 文案后同步 `status-rendering.spec.ts`。
- `mod tests` 访问父模块私有 fn 一律 `super::`。
- 数据面 egress 勿纯 `try_send`：至少有界等待，否则 TCP/分片突发必挂。
- attached 直连测试要清 `is_hole_punched`（与 `peer_manager` 准入一致）。
- `cfg(test)` helper 未用即删（`-D dead-code`）。

### 2026-10-25 — ET Test：`cargo fmt --check`（api_input flow_stats 测试）

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/40194422628 |
| 失败 job | Check fmt & lockfile（连带 Run linters & check / test） |
| 根因一类 | fmt |
| 状态 | 已修复（`cargo fmt --all`；`api_input.rs` 三处换行） |

**摘要**：`flow_stats` 相关测试里 `NetworkConfig::new_from_config(...).unwrap()` 换行与 rustfmt 不一致。本轮 clippy / Build test / 各 Test\* / Windows / Android 均绿；只有 fmt 门禁红。

**防再犯**：任意 `.rs` 提交前 `cargo fmt --all -- --check`。

---

### 2026-10-25 — ET Test：`prefer_peer_relay` 双 admin 探针打到 forbidden admin

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/40148655510 |
| 失败 job | Test (easytier) |
| 根因一类 | flaky-test |
| 状态 | 已修复（测试侧） |

**摘要**：`prefer_peer_relay_uses_forwarded_coverage_across_two_admins` 在 `wait_route_next_hop`（LeastHop 已是 relay）后 `assert_ping_forwarded_by` 失败：`TrafficPacketsForwarded` 在 forbidden admin 上 +3（三次 ping）。默认 `latency_first=true`，数据走 LeastCost；新建 `source→relay` 边在 peer-center 仍为默认代价 500 时，LeastCost 继续走已测速的 `source→admin→relay→target`，admin 计入转发。

**防再犯**：见 §3「数据面 next_hop 要对齐 `latency_first`」。

**修复说明**（`easytier/src/tests/credential_tests.rs`）：

- `wait_route_next_hop` 按实例生效的 `latency_first`（`flags.latency_first && !flags.p2p_only`，与 `PeerPacketPolicy::from_flags` 一致）等待真正的数据面 next hop；
- `prefer_peer_relay_*` 拓扑用例对所有节点 `disable_latency_first`，避免测速时序干扰投影断言。

---

### 2026-10-24 — ET Test：P1.5 最后一条路径 hold 拖死 failover（credential 用例连挂）

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | 首个红 ET Test `40072947771`（HEAD `87225ba7`）；同因 `40074168968`（`c45ed1cc`）、`40076151092`（`1fe10220`）；最后绿 `40046135326`（`36bba9e4`） |
| 失败 job | ET Test（Linux，`--features wireguard`） |
| 根因一类 | 逻辑回归（`4aa7bb50` P1.5） |
| 状态 | 已修复 |

**摘要**：`4aa7bb50` 的 P1.5 gate 在达 `ping_fail_close_count` 且「无已验证替代」时跳过关闭，再用 **额外 miss 数**（`+5`）兜底强关。两处失真：① miss 间隔由 `PingIntervalController` 退避决定，5 个 miss 的墙钟时间不可控（实测≈+10s）；② 「从未 round-trip 成功过」的新建 conn 也享受 hold，而这正是 credential 直连重试的场景。

结果：dead conn 一直占着 `has_live_conns()`，`has_directly_connected_conn` 为真 → direct connector 不重拨 → `wait_direct_peer(30s)` 报 `direct peers=[]`；`credential_non_reusable_*` 在 60.08s 撞 60s 预算。

**防再犯**：连续失败阈值只表达「失败次数」，任何「再等一会儿」的宽限必须用**墙钟**（`Duration`）表达并写上界；新增 hold 规则时要回答「这条 conn 凭什么是值得保护的路径」，并配一条「gate 始终 deny 也必须能关」的单测。

**修复说明**（`easytier-core/src/peers/conn/peer_conn_ping.rs`）：

- 传输已关（`TunnelError::Shutdown` 或 `close_notify.is_closed()`）→ 立刻 break，不再空数 miss；
- 从未 round-trip 成功的 conn 不适用 hold（`had_roundtrip`），到阈值即关，恢复 `4aa7bb50` 之前的时序；
- 真正的最后路径宽限改为墙钟 `LAST_PATH_HOLD_MAX = 5s`（自首次越过阈值起算），到期强关并打 `last path hold expired`；
- 宽限期的 `ping fail close skipped` 降为 debug（stability plan §3.4），关闭日志串保持不变；
- 新增单测 `never_round_tripped_conn_closes_even_when_gate_denies`（gate 一直 deny 也必须能关）。

---

### 2026-10-23 — ET Test：`flow_history` 测试 `conn_id` 元组类型 E0277

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/39974545421 |
| 失败 job | Check clippy（`easytier-web` `--tests`） |
| 根因一类 | compile |
| 状态 | 已修复（`run_series` 返回 `(String, String, …)`） |

**摘要**：`run_series` 用 `try_get::<String>("conn_id")`，返回类型却写成 `Vec<(i64, String, …)>`，断言又用 `r.0 == "42"`，clippy 编测试时 E0277。

**防再犯**：见 §3「测试 helper 返回类型跟 `try_get` 一致」。

---

### 2026-10-23 — ET Windows GUI：`totals.push(0)` E0308

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/39913358517 （同因：39904818294） |
| 失败 job | `build-gui`（x86_64-pc-windows-msvc）编 `easytier-web` |
| 根因一类 | compile |
| 状态 | 已修复（`totals.push(0.0)`） |

**摘要**：`Vec<f64>` 上 `push(0)`，rustc E0308 `expected f64, found integer`。TS2322 已过，pnpm 绿之后仍卡在 GUI embed 的 web crate。

**防再犯**：见 §3「`Vec<f64>` 不要 `push(0)`」；改 `easytier-web` 后 Windows GUI 也会编它。

---

### 2026-10-23 — ET Windows/Android：`rowFlowTip` `escape: true` 触发 TS2322

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | Windows https://github.com/229033891/EasyTier/actions/runs/39903751945 ；Android https://github.com/229033891/EasyTier/actions/runs/39903622775 |
| 失败 job | `build-gui` / `build-mobile` → Prepare build environment → `pnpm --filter easytier-gui... build` |
| 根因一类 | frontend / vue-tsc |
| 状态 | 已修复（`Status.vue` 改回 `<br>` + `escape: false`；`vue-tsc -b` 已绿） |

**摘要**：`81c01fcd` 把 `rowFlowTip` 从 `escape: false` 改成 `escape: true`，但函数返回类型字面量仍是 `{ escape: false }`，`vue-tsc -b` 报 `TS2322: Type 'true' is not assignable to type 'false'`。Windows/Android GUI 的 `prepare-build` 都在 `easytier-frontend-lib` 处 exit 2。且 `\n` + `escape:true` 也不符合同文件 `rowLatencyTip` 的多行 tip 约定（PrimeVue 会折叠换行）。

**防再犯**：见 §3「PrimeVue tip 多行」；改 `frontend-lib` 后至少 `pnpm --filter easytier-frontend-lib exec vue-tsc -b`。

**修复说明**：`rowFlowTip` 与 `rowLatencyTip` 对齐：`lines.join('<br>')` + `escape: false`。

---

### 2026-10-18 — OpenWrt LuCI：状态页 `unfinished string`（CRLF）

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/luci-app-easytier` + EasyTier OpenWrt 包 |
| Run | 现网装 `ET-openwrt-x86_64-apk.zip` 后 LuCI 状态页 Runtime error |
| 失败 job | 运行时（非 CI 编译红） |
| 根因一类 | env / packaging |
| 状态 | 已修复（路由器 `sed` 去 `\r`；仓库加 `.gitattributes` + Prepare 钩子 + workflow `sed`） |

**摘要**：装上的 `easytier_status.htm` 为 CRLF（`33386` 字节 / MD5 `5c009e7f…`），仓库 LF 版为 `32226` / `af0fed62…`。LuCI `template_parser` 对 TEXT chunk 只转义 `\`/`"`/`\n`，裸 `\r` 落在 `write("…")` 里导致 `unfinished string near '"'`（报在源文件约第 15 行）。

**防再犯**：`*.htm`/`*.lua` 固定 `eol=lf`；OpenWrt `Prepare feed` 与 `Build/Prepare/luci-app-easytier` 打包前去 CR；装包后若再撞可 `sed -i 's/\r$//' /usr/lib/lua/luci/view/easytier/*.htm`。

---

### 2026-10-17 — lib test：`has_active_probe` dead_code

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | ET Test（lib test / `-D warnings`） |
| 失败 job | Check / Test（`easytier-core` lib test） |
| 根因一类 | clippy / compile（`-D dead-code`） |
| 状态 | 已修复（删除未使用的 `PeerConnLiveness::has_active_probe`） |

**摘要**：`peer_conn_liveness.rs` 中 `#[cfg(test)] pub(super) fn has_active_probe` 从未被测试或其它代码调用；CI 以 `-D warnings` 编译 lib test 时直接挂。

**防再犯**：见 §3「`#[cfg(test)]` helper 若无断言引用也算 dead_code」。新增仅测用 helper 时，要么在同文件测试里立刻用上，要么不要提交。

**修复说明**：删除该方法；`active_probe()` 私有 API 仍被 `before_send` 使用。

### 2026-10-16 — Check clippy：`doc_lazy_continuation`（proto 生成注释）

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | ET Test → Check clippy |
| 失败 job | Check clippy |
| 根因一类 | clippy |
| 状态 | 已修复（`api_manage.proto` `desired_proxy_cidrs` 注释去掉行首 `+`） |

**摘要**：`desired_proxy_cidrs` 多行注释第二行以 `+ manual_routes…` 开头；prost 生成 `/// + …` 后被当成 Markdown 列表，后续续行触发 `clippy::doc_lazy_continuation`（5 处），`easytier-proto` 编译失败。

**防再犯**：见 §3「proto 注释勿让续行以 `+`/`-`/`*` 开头」。

**修复说明**：改为逗号并列写法（`peer routes, policy local_routes, …`），本地 `cargo clippy -p easytier-proto -- -D warnings` 通过。

### 2026-10-15 — ET Android：`autostart:default` permission not found

| 项 | 内容 |
|----|------|
| 仓库/分支 | `229033891/EasyTier` `dev` |
| Run | https://github.com/229033891/EasyTier/actions/runs/38652316606 |
| 失败 job | build-mobile (aarch64-linux-android) → Build |
| 根因一类 | compile（Tauri ACL / 平台权限） |
| 状态 | 已修复（`desktop-autostart.json` + 从 `migrated.json` 移出） |

**摘要**：GUI 登录自启把 `autostart:*` 写进共用 `capabilities/migrated.json`；Android 目标不编 `tauri-plugin-autostart`，build 脚本校验 capability 时报 `Permission autostart:default not found`。

**防再犯**：见 §3「桌面专用 Tauri 权限勿进共用 capability」。

**修复说明**：`autostart` 权限迁到 `capabilities/desktop-autostart.json`（`platforms: macOS/windows/linux`）。

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
