# EasyTier Windows 一键打包（GUI + 无头包）

## Status

- Status: **Ops**
- 最近审阅：2026-10-28（CI：`build-gui` ∥ `build-headless`；写清交付三分法）
- 适用范围：本机打出与 CI `windows.yml` 同布局的 **GUI NSIS 安装包**（含 `ET-cli`）+ **无头 `ET-windows-*` 目录/zip**
- 索引：[`../README.md`](../README.md)
- 产品语义：[`../current/product-map.md`](../current/product-map.md) §1.2
- 配套：
  - [`web-build-deploy.md`](./web-build-deploy.md)（仅 Web embed 一键脚本）
  - [`windows-msvc-local-build.md`](./windows-msvc-local-build.md)（MSVC / vcvars）

---

## 0. Windows 交付三分法（先读）

| # | 产品 | 你该拿什么 | 备注 |
|---|------|------------|------|
| 1 | **Web 控制台** | `script\easytier-web-*.cmd` → `easytier-web-embed`；或无头目录里的 `ET-web-embed.exe` | 独立产品；集中配置时才需要 |
| 2 | **桌面 GUI + CORE + CLI** | NSIS（`artifacts\*.exe` / CI `ET-gui-windows-*`） | `ET.exe` **内嵌** core；同目录 `ET-cli.exe` |
| 3 | **无桌面节点 CORE + CLI** | 无头包中的 `ET-core.exe` + `ET-cli.exe` + 驱动 | **不需要** GUI；**不必然**需要 Web |

当前 CI/本地「完整 Windows 包」仍产出 **两份 artifact**（GUI NSIS + 无头 zip）。无头 zip **附带** `ET-web-embed.exe` 是部署便利，**不是**把产品 1 并进产品 3 的语义定义。只要节点可忽略 embed；只要控制台优先用 Web 单独脚本。

---

## 0.1 和 Web 脚本的关系

> ⛔ **不要手搓打包命令。** 本地打安装包**必须**用本目录列出的 `script\*.cmd` / `script\build-*.ps1` 入口。
> 直接拼 `cargo build --release` / `pnpm tauri build` 会缺脚本内置的 `CARGO_PROFILE_DEV_DEBUG=0`、
> 短 `CARGO_TARGET_DIR`、可写 `TEMP`、`perl` 补齐、WebView2 引导包预检等前置，通常以
> MSVC `D8050`、esbuild「拒绝访问」或编完才卡在微软 CDN 收场。
> 目录名是**单数 `script\`**（仓库根没有 `scripts/`）。总览见 [`../README.md`](../README.md)「⛔ 打包」。

同一套入口风格，共用 `script/build-common.ps1`：

| 场景 | 入口 |
|------|------|
| **仅 Web embed**（产品 1） | `script\easytier-web-*.cmd` |
| **完整 Windows 包**（GUI NSIS + 无头 zip；无头内仍附带 embed） | `script\easytier-windows-*.cmd` |

完整 Windows 包的无头半边会**附带** `ET-web-embed.exe`（与 CI headless 一致），不必再单独跑 web-release，除非你只想要单个 web 二进制或要跳过 GUI/无头整包。

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

| 产物 | 对应产品 | 说明 |
|------|----------|------|
| `artifacts\*.exe` | **2 桌面** | GUI **NSIS**（`ET.exe` 内嵌 core + **`ET-cli.exe`** sidecar） |
| `artifacts\ET-windows-x86_64\` | **3 无头节点**（+ 附带 1） | `ET-core.exe` / `ET-cli.exe` + 驱动；**另附** `ET-web-embed.exe`（便利，可忽略） |
| `artifacts\ET-windows-x86_64.zip` | 同上 | 上述目录的 zip（可用 `-NoZip` 跳过） |
| （Web 单独脚本产物） | **1 Web** | 见 [`web-build-deploy.md`](./web-build-deploy.md)；不强制走本脚本 |

架构目录名随主机：`x86_64` / `arm64` / `i686`（来自 `easytier/third_party/<arch>`）。

桌面 GUI（Win / macOS / Linux）通过各平台 conf 的 Tauri `externalBin`（`tauri.windows.conf.json` / `tauri.macos.conf.json` / `tauri.linux.conf.json` → `binaries/ET-cli`）附带 CLI；**不要**写进共享 `tauri.conf.json`，以免 Android 构建也去找 sidecar。打包装前须先 `Stage-GuiEtCliSidecar` / CI「Stage ET-cli sidecar」。Windows NSIS 安装后一般为 `C:\Program Files\ET\ET-cli.exe`，安装器会把 **`$INSTDIR` 写入系统 PATH**（幂等；卸载时移除；**需新开终端**才生效）。连 GUI 实例常用 `-p 127.0.0.1:15999`。当前 `windows.yml` GUI 矩阵以 Windows 为主；workflow 已预留 darwin/linux 收集与 stage，恢复矩阵行即可同样带 CLI。

---

## 3. 脚本在做什么（对照 CI）

本地脚本仍可一次打出 GUI + 无头（串行）。CI（`.github/workflows/windows.yml`）为缩短墙钟已拆成并行 job：

| CI job | 产物 | 说明 |
|--------|------|------|
| `build_web`（ubuntu） | `ET-web-dashboard-windows` | web 前端 dist，供 embed |
| `build-gui`（windows） | `ET-gui-windows-*` | NSIS；与 headless **并行** |
| `build-headless`（windows） | `ET-windows-*` | 下载 web dist 后编 core/cli/embed；与 GUI **并行** |

本地大致步骤：

1. 从 `easytier/third_party/<arch>/` 拷贝 `*.dll` / `*.sys` → `easytier-gui/src-tauri/`（Tauri resources）
2. `pnpm` 先构建 `easytier-gui^...`（依赖，含 frontend-lib 一次），再 `pnpm run build:app`
3. `cargo build -p easytier --bin easytier-cli --release` → stage 到 `src-tauri/binaries/`；再 `pnpm tauri build --bundles nsis`（跳过二次 `beforeBuildCommand`）
4. 构建 `easytier-web` 前端（含 wasm），再 `cargo build -p easytier-web -p easytier --features easytier-web/embed`
5. 收集到 `artifacts/`

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
