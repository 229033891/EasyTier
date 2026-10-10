# EasyTier Windows 一键打包（GUI + 无头包）

## Status

- Status: **Ops**
- 最近审阅：2026-10-10
- 适用范围：本机打出与 CI `windows.yml` 同布局的 **GUI NSIS 安装包** + **无头 `ET-windows-*` 目录/zip**
- 索引：[`../README.md`](../README.md)
- 配套：
  - [`web-build-deploy.md`](./web-build-deploy.md)（仅 Web embed 一键脚本）
  - [`windows-msvc-local-build.md`](./windows-msvc-local-build.md)（MSVC / vcvars）

---

## 0. 和 Web 脚本的关系

同一套入口风格，共用 `script/build-common.ps1`：

| 场景 | 入口 |
|------|------|
| **仅 Web embed** | `script\easytier-web-*.cmd` |
| **完整 Windows 包**（GUI NSIS + 无头 zip） | `script\easytier-windows-*.cmd` |

完整 Windows 包会**内含** `ET-web-embed.exe`（与 CI headless 一致），不必再单独跑 web-release，除非你只想要单个 web 二进制。

---

## 1. 一键入口（推荐）

仓库提供三个固定入口（**前台运行**；控制台输出同时写入 `artifacts\logs\easytier-windows-*.log`）：

```bat
script\easytier-windows-debug.cmd      REM 调试：pnpm tauri dev（不打安装包）
script\easytier-windows-fast.cmd       REM 日常：GUI NSIS(--release) + 无头(release-fast)
script\easytier-windows-release.cmd    REM 正式：两端都 --release（更接近 CI）
```

或直接调 PowerShell：

```powershell
.\script\build-easytier-windows.ps1 -Dev
.\script\build-easytier-windows.ps1 -Profile Fast
.\script\build-easytier-windows.ps1 -Profile Release
.\script\build-easytier-windows.ps1 -SkipGui                 # 只要无头包（Fast 时最快）
.\script\build-easytier-windows.ps1 -SkipHeadless            # 只要 NSIS
.\script\build-easytier-windows.ps1 -Interactive
```

**建议：**

1. 先备齐本机依赖（见下节 + [`web-build-deploy.md` §2.0.1](./web-build-deploy.md#201-构建前先备齐本地依赖)）。
2. **打 GUI/NSIS 前**预置 WebView2 引导包（脚本会在耗时编译前检查，见 §4）。
3. 用前台窗口跑，便于看卡在 GUI / web frontend / cargo 哪一步。
4. 日常只要无头包：`easytier-windows-fast.cmd` 等价于加 `-SkipGui` 更快；或直接 `.\script\build-easytier-windows.ps1 -Profile Fast -SkipGui`。
5. **Fast** 时 GUI/NSIS 仍走 cargo `--release`（Tauri 装包目录固定为 `release/`）；`release-fast` 只加速无头 `ET-*`。
6. MSVC 未进 PATH 时，先按 [`windows-msvc-local-build.md`](./windows-msvc-local-build.md) 加载 vcvars 再跑。

---

## 2. 产物布局（默认 `artifacts\`）

与 CI artifact 命名对齐：

| 产物 | 说明 |
|------|------|
| `artifacts\*.exe` | GUI **NSIS** 安装包（Tauri bundle） |
| `artifacts\ET-windows-x86_64\` | 无头：`ET-core.exe` / `ET-cli.exe` / `ET-web-embed.exe` + `wintun`/`Packet`/`WinDivert` |
| `artifacts\ET-windows-x86_64.zip` | 上述目录的 zip（可用 `-NoZip` 跳过） |

架构目录名随主机：`x86_64` / `arm64` / `i686`（来自 `easytier/third_party/<arch>`）。

---

## 3. 脚本在做什么（对照 CI）

大致对齐 `.github/workflows/windows.yml`：

1. 从 `easytier/third_party/<arch>/` 拷贝 `*.dll` / `*.sys` → `easytier-gui/src-tauri/`（Tauri resources）
2. `pnpm` 先构建 `easytier-gui^...`（依赖，含 frontend-lib 一次），再 `pnpm run build:app`；然后 `pnpm tauri build --bundles nsis`（跳过二次 `beforeBuildCommand`）
3. 构建 `easytier-web` 前端（含 wasm），再 `cargo build -p easytier-web -p easytier --features easytier-web/embed`
4. 收集到 `artifacts/`

实现：`script/build-easytier-windows.ps1`；公共逻辑：`script/build-common.ps1`。

---

## 4. 构建前依赖（摘要）

| 依赖 | 用途 |
|------|------|
| Node.js + pnpm | GUI / Web 前端 |
| Rust（`rust-toolchain.toml`）+ MSVC | GUI、core、web |
| 7-Zip（`7z`） | thunk-rs 解包 |
| wasm-pack + wasm-bindgen + wasm-opt | 仅无头包里的 web embed 前端需要 |
| `easytier/third_party/<arch>/` | 运行时 DLL/驱动（仓库已带） |
| **WebView2 bootstrapper**（打 GUI NSIS 时） | `%LOCALAPPDATA%\tauri\MicrosoftEdgeWebview2Setup.exe`（约 1.8MB） |

脚本在开编前会检查 WebView2 引导包；缺失时尝试从 `Downloads` / `Desktop` / `script\tools\` 自动拷贝，仍没有则**立刻失败**并打印下载地址（避免编完才卡在微软 CDN）。

下载：https://go.microsoft.com/fwlink/p/?LinkId=2124703  

Web 侧工具落盘路径与 [`web-build-deploy.md` §2.0.1](./web-build-deploy.md#201-构建前先备齐本地依赖) 相同。
