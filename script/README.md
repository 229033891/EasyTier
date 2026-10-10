# script/ 使用说明

本目录放本地构建、安装与运维脚本。Windows 打包请**前台**双击 `.cmd` 或在终端运行（便于看日志）。

打包时控制台输出会**同时**写入 `artifacts\logs\`（如 `easytier-windows-yyyyMMdd-HHmmss.log`），失败后可直接打开该文件排查。

更细的 Ops 文档见仓库 [`docs/ops/`](../docs/ops/)。

---

## 一、Windows 本地打包（常用）

入口都是「双击 / 前台运行」；底层是同目录的 `.ps1`，公共逻辑在 `build-common.ps1`（勿单独执行）。

### 1. 仅 Web embed

| 脚本 | 作用 | 产物 |
|------|------|------|
| `easytier-web-debug.cmd` | 调试：API + Vite（不打 exe） | 两个新窗口 |
| `easytier-web-fast.cmd` | 日常：`release-fast` + embed | `target\release-fast\easytier-web-embed.exe` |
| `easytier-web-release.cmd` | 正式：`--release` + embed | `target\release\easytier-web-embed.exe` |

等价 PowerShell：

```powershell
.\script\build-easytier-web.ps1 -Dev
.\script\build-easytier-web.ps1 -Profile Fast
.\script\build-easytier-web.ps1 -Profile Release
.\script\build-easytier-web.ps1 -Interactive
```

文档：[`docs/ops/web-build-deploy.md`](../docs/ops/web-build-deploy.md)

### 2. 完整 Windows 包（GUI NSIS + 无头 zip）

对齐 CI `windows.yml` 布局。

| 脚本 | 作用 | 产物（默认 `artifacts\`） |
|------|------|---------------------------|
| `easytier-windows-debug.cmd` | 调试：`pnpm tauri dev` | 无安装包 |
| `easytier-windows-fast.cmd` | 日常：GUI NSIS（`--release`）+ 无头 `release-fast` | NSIS `*.exe` + `ET-windows-*` + `.zip` |
| `easytier-windows-release.cmd` | 正式：两端 `--release` | 同上 |

等价 PowerShell：

```powershell
.\script\build-easytier-windows.ps1 -Dev
.\script\build-easytier-windows.ps1 -Profile Fast
.\script\build-easytier-windows.ps1 -Profile Release
.\script\build-easytier-windows.ps1 -SkipGui          # 只要无头包
.\script\build-easytier-windows.ps1 -SkipHeadless     # 只要 NSIS
.\script\build-easytier-windows.ps1 -Interactive
```

说明：

- **Fast** 时 GUI/NSIS 仍走 cargo `--release`（Tauri 装包目录固定为 `release/`）；`release-fast` 只加速无头 `ET-*`。
- 打 GUI NSIS 前会检查 `%LOCALAPPDATA%\tauri\MicrosoftEdgeWebview2Setup.exe`（可用 Downloads 预置，脚本会自动拷贝）。
- MSVC 未进 PATH 时，先按 [`docs/ops/windows-msvc-local-build.md`](../docs/ops/windows-msvc-local-build.md) 加载 vcvars。
- 依赖备齐说明见 [`docs/ops/windows-build-pack.md`](../docs/ops/windows-build-pack.md)。

### 3. 本地 ET Test（push 前自检）

对齐 CI [`.github/workflows/test.yml`](../.github/workflows/test.yml)。日志同样写入 `artifacts\logs\`。

| 脚本 | 作用 | 覆盖 |
|------|------|------|
| `easytier-test-fast.cmd` | 日常 push 前 | `fmt` + `Cargo.lock` + `clippy -D warnings` + `cargo hack` features |
| `easytier-test-full.cmd` | 更完整 | Fast + WASI（若已 `rustup target add wasm32-wasip1`）+ `nextest`（**不含** three_node） |

等价 PowerShell：

```powershell
.\script\test-easytier.ps1 -Profile Fast -InstallTools
.\script\test-easytier.ps1 -Profile Full -InstallTools
.\script\test-easytier.ps1 -Interactive
```

Linux / WSL（含可选 three_node）：

```bash
./script/test-easytier.sh --install-tools
./script/test-easytier.sh --full
./script/test-easytier.sh --full --three-node
```

说明：

- **Fast** 能拦住本次 `releases/v2.7.5` 上失败的 fmt / clippy / features。
- **three_node**（含 FakeTCP）需要 Linux 的 tun/bridge/`sudo`；Windows 上 Full 会跳过，请用 WSL 或等 CI。
- 缺 `cargo-hack` / `cargo-nextest` 时加 `-InstallTools` / `--install-tools`。
- Windows 与 CI 的差异（有意为之）：**不用 `--features full`**（会编 `openssl-crypto`）；clippy **不含 `--all-targets`/`--tests`**；`--no-default-features` 用 **`cargo check`**（与 CI cargo-hack 一致，避免 Windows 平台模块 dead_code 误报）。完整矩阵仍靠 Linux CI。
- 脚本会自动设 `CARGO_PROFILE_DEV_DEBUG=0`、短 `CARGO_TARGET_DIR`。
- 若 clippy/hack 仍失败：用「x64 本机工具」终端（需 `cl.exe`），见 [`docs/ops/windows-msvc-local-build.md`](../docs/ops/windows-msvc-local-build.md)。

---

## 二、从 Release 安装（下载成品，不是本地编译）

| 脚本 | 平台 | 说明 |
|------|------|------|
| `install.cmd` / `install.ps1` | Windows | 管理员安装；从 GitHub Release 拉 `ET-windows-*` |
| `install.sh` | Linux | 安装 / 备份 / 恢复 / 卸载等（交互） |
| `update.sh` | Linux | 从 Release 升级 |
| `check-install.sh` | Linux | 检查安装状态 |
| `preflight-local-upgrade.sh` | Linux | 本地升级预检（已装目录 vs 新包） |

`et-ops-common.sh` 为 `install.sh` / `update.sh` 共用库，**不要直接执行**。

远程一键安装（从 GitHub 拉最新 `install.sh`；无 TTY 时默认 core 轻量）：

```bash
curl -fsSL https://raw.githubusercontent.com/229033891/EasyTier/main/script/install.sh \
  | sudo bash -s install
```

或 clone 后交互安装（推荐 Server / Client）：

```bash
git clone https://github.com/229033891/EasyTier.git
cd EasyTier
sudo bash script/install.sh
```

远程一键升级（从 GitHub 拉最新 `update.sh` 并执行）：

```bash
curl -fsSL https://raw.githubusercontent.com/229033891/EasyTier/main/script/update.sh \
  -o /tmp/et-update.sh && sudo bash /tmp/et-update.sh --auto
```

文档：[`docs/ops/deploy-install.md`](../docs/ops/deploy-install.md)

---

## 三、其它

| 脚本 | 说明 |
|------|------|
| `docker-up.sh` | Docker Compose 一键启动（node / console / all） |
| `install-protoc.sh` | 安装 protoc（构建用） |
| `install-upx.sh` | 安装 UPX |
| `build-wasi-core.sh` | 构建 WASI core |
| `benchmark-two-node.sh` | 双节点 benchmark |
| `test-cli-multi-instance.sh` | CLI 多实例测试 |
| `tools/` | 本地缓存的 wasm-bindgen / Binaryen 等（可选，避免构建时联网） |

---

## 四、怎么选

```text
只要改 Web 控制台、打 embed exe     →  easytier-web-*.cmd
要 GUI 安装包 + ET-core/cli/web zip →  easytier-windows-*.cmd
push 前对齐 lint / feature 门控       →  easytier-test-fast.cmd
本地尽量对齐 CI 测试                 →  easytier-test-full.cmd / test-easytier.sh
只要现成发布包装到机器上           →  install.cmd / install.sh
```
