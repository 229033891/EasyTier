# AGENTS.md（EasyTier）

Agent 协作指引。本文件 = **必读约束** + **本机编译/测试最短路径**；长规范见 `.cursor/rules/` 与 `docs/`，不要在这里重复。

## 先读（Read first）

1. [`docs/README.md`](docs/README.md) — 文档分层（`current` / `roadmap` / `ops` / `archive`）
2. [`docs/current/system-overview.md`](docs/current/system-overview.md) — 系统如何拼起来
3. 任务相关的 Current，或绑定的 Roadmap（不确定就问用户）

**Brand**：不把上游/fork/源项目身份写进 `docs/current|ops|roadmap`、根 README 或产品文案 i18n。详见 [`docs/README.md`](docs/README.md)。

## 本机编译（Windows / MSVC）

**核心事实**：本机 `cargo` 直接跑会在 `ring` 等含 C 代码的依赖上失败（cl.exe 找不到 INCLUDE/LIB）。MSVC 环境（cl + INCLUDE + LIB）必须通过 `script/build-common.ps1` 的加载器临时导入，**不要**把 `Hostx64\x64` 单独塞进用户 PATH（缺 INCLUDE/LIB，工具链升级后还会失效）。

**注意**：dot-source `script/build-common.ps1` 前必须先设 `$RepoRoot`，否则报错退出：

```powershell
Set-Location <仓库根>
$RepoRoot = (Get-Location).Path
. .\script\build-common.ps1
Initialize-EasytierWindowsCargoEnv | Out-Null
Import-EasytierMsvcEnvironment | Out-Null
```

### 常用入口（按场景选）

| 场景 | 入口 |
|------|------|
| push/提交前 lint 门禁（fmt+lockfile+clippy+features） | `script\easytier-test.cmd fast` |
| 更完整的本地测试 | `script\easytier-test.cmd full`（不含 three_node，那需要 Linux/WSL） |
| Web 控制台 embed exe | `script\easytier-web-fast.cmd`（release-fast） |
| GUI NSIS + 无头 zip 完整包 | `script\easytier-windows-fast.cmd` |
| 单包快速编译检查（agent 用） | 见下「Agent 后台编译检查」 |

脚本内部自动做三件事（与上面手动示例同一顺序）：
1. `Initialize-EasytierWindowsCargoEnv`：`CARGO_PROFILE_DEV_DEBUG=0`（避免 D8050）+ 短 `CARGO_TARGET_DIR`（默认 `.cache\cargo-target`）+ TEMP 重定向；**并在函数末尾自动调用** `Import-EasytierMsvcEnvironment`（`script/build-common.ps1:319`）；
2. `Import-EasytierMsvcEnvironment`：vswhere 定位 `vcvars64.bat` 并把环境导入当前进程（手动示例里显式再调一次是幂等的，可省略）；
3. 缺 perl/cargo-hack 等工具时的兜底路径。

也就是说：`Initialize` → `Import`（自动），不要按相反顺序理解；`Ensure-WindowsBuildPrereqs`（`build-common.ps1:326`）是另一条入口，仅供 thunk-rs 打包链使用。

### 两个实测踩过的坑（2026-11）

1. **`CARGO_TARGET_DIR` 被外部会话污染**：持久 shell 里可能残留别的会话设的 `CARGO_TARGET_DIR`（如 `.cache\aws-lc-probe\target`），导致构建进错目录、增量状态错乱。跑编译前显式复位：
   `$env:CARGO_TARGET_DIR = "$RepoRoot\.cache\cargo-target"`。
2. **lib.exe LNK1104（无法打开 `Temp\2\lnk{GUID}.tmp`）**：外部会话的 `%TEMP%` 目录被并行进程/杀软污染时，aws-lc-sys 等的 lib.exe 会随机失败（同一个包这次死在探针、下次死在归档，表现不稳定）。解法：给 cargo 进程换干净的独立 TEMP（如 `$RepoRoot\.cache\tmp2`，跑前清空），`$env:TEMP` 与 `$env:TMP` 都要设。

### Agent 后台编译检查（推荐模式）

冷缓存 `cargo check -p easytier-web -p easytier-core` 可能超过 20 分钟，**前台跑会被会话超时杀掉**（后台 detached 进程也会随会话结束被杀，轮询发现进程消失要重启）。用后台 + 轮询：

```powershell
# 启动（detached；包名逗号分隔）
$p = Start-Process powershell -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass',
  '-File',"$RepoRoot\.cache\check-local.ps1",
  '-Packages','easytier-proto,easytier-web,easytier-core,easytier-ohrs') -WindowStyle Hidden -PassThru
"PID=$($p.Id)"

# 轮询（进程存活 + 关键行）
Get-Process -Id <PID> -ErrorAction SilentlyContinue
Get-Content "$RepoRoot\.cache\check-local.log" -Tail 40
```

`.cache\check-local.ps1` 是**本地脚手架，不入库**（`.cache/` 已被忽略）；换机/新会话若不存在就照下面要点自建：

1. 设 `$RepoRoot`（仓库根）→ dot-source `script\build-common.ps1`；
2. `Initialize-EasytierWindowsCargoEnv` + `Import-EasytierMsvcEnvironment`；
3. **TEMP/TMP 换到 `.cache\tmp2` 并先清空**（防 LNK1104）；
4. 把 `-Packages` 拼成 `-p <pkg>` 调 `cargo check`，全部输出（含加载日志、`INCLUDE set`）写 `.cache\check-local.log`，末行打 `run exit=`（每次覆盖）。

> 注意：`cargo test` 的 `$LASTEXITCODE` 会被管道/`Out-File` 覆盖，脚本里判失败要用 `$LASTEXITCODE` 紧跟在 cargo 之后，别依赖 PowerShell 的 `2>&1` 输出（stderr 的 "Compiling …" 会伪装成错误行）。

## 提交前必查（`.cursor/rules/ci-failure-log-presubmit.mdc`）

1. `cargo fmt --all -- --check`（失败先 `cargo fmt --all`）
2. `cargo metadata --format-version 1 --locked`
3. `script\easytier-test.cmd fast`（或按 `docs/ops/ci-failure-log.md` 的表格跑对应 clippy）
4. 新失败 → 按该文档 §4 模板追加到 §5

**未完成上述检查不要 commit / push**（用户明确只要文档或跳过自检除外）。

## 任务收尾（文档更新，`.cursor/rules/doc-update-after-task.mdc`）

用户认为任务完成、或要求提交/开 PR 且含功能/行为改动时：

1. 按 [`docs/ops/doc-update-after-task.md`](docs/ops/doc-update-after-task.md) §2 决策表定类型与最少更新集；
2. **Current 严门槛**：未验收行为只更新绑定的 Roadmap/Ops（勾选 + 待验收），**禁止**写进 `docs/current/`；
3. 无绑定文档 → 先提 1–2 个候选请用户确认，不要散改多篇 Current；
4. 纯错别字/链接、或用户明确跳过 → 可免。

## Git / 卫生

- 只在用户要求时提交；**不** force-push `main`/`master`；**不** amend 他人的提交
- **不**把 `.workbuddy/memory/` 与产品代码一起 stage（agent 笔记不要混进功能提交）
- 行为改动（`feat` / `fix`）与 `docs(roadmap)` 尽量拆开提交，便于 release notes 与回滚

## 不要臆造

- 不把 Roadmap / Archive 当作已上线的 Current 行为
- 不以「CI 会抓」为由跳过 fmt / lockfile 自检
- 不说「本地 Windows Full = GitHub ET Test」

## 参考

- MSVC 本机构建细节：[`docs/ops/windows-msvc-local-build.md`](docs/ops/windows-msvc-local-build.md)
- 打包依赖备齐：[`docs/ops/windows-build-pack.md`](docs/ops/windows-build-pack.md)
- 脚本索引：[`script/README.md`](script/README.md)
