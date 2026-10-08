# Windows 本机 MSVC 环境（Rust / C 依赖）

## Status

- Status: **Ops**
- 最近审阅：2026-10-08
- 适用范围：本机用 `x86_64-pc-windows-msvc` 编 `easytier` / `easytier-core` 及依赖 `ring`、aws-lc 等需 `cl.exe` 的 crate
- 索引：[`../README.md`](../README.md)
- 相关：[`web-build-deploy.md`](./web-build-deploy.md)

---

## 现象

普通 PowerShell / Cursor 终端里：

- `where cl` 找不到；
- `cargo test` / `cargo build` 编 `ring` 时报 `cl.exe` 失败（Access denied / exit code 2 等），**不等于本机没装 MSVC**。

常见原因：**已装 Build Tools，但当前 shell 未加载 vcvars**（`INCLUDE` / `LIB` / PATH 未注入）。

---

## 本机核实（参考环境）

以下以一台已装 **VS Build Tools 2022** 的机器为例（路径可能随版本略变）：

| 项 | 值 |
|----|-----|
| 产品 | Visual Studio Build Tools 2022（约 17.14.x） |
| 安装根 | `C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools` |
| MSVC | `VC\Tools\MSVC\14.44.x`，`cl` ≈ 19.44.x |
| Windows SDK | `C:\Program Files (x86)\Windows Kits\10\`（如 `10.0.26100.0`） |
| Rust | `rustup` 默认 / 仓库 `rust-toolchain.toml` → `x86_64-pc-windows-msvc` |

自检：

```powershell
# 安装是否存在
Test-Path "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
& "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" `
  -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
  -property displayName,installationPath,installationVersion

# cl 是否在磁盘上（未必在 PATH）
Get-ChildItem "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC" `
  -Recurse -Filter cl.exe -ErrorAction SilentlyContinue |
  Select-Object -First 4 FullName
```

---

## 正确用法：先 vcvars，再 cargo

在**同一** cmd / 已继承环境的会话里：

```bat
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
cd /d D:\EasyTier
cargo test -p easytier-core --lib peers::error::tests
```

PowerShell 可：

```powershell
cmd /c 'call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" && cd /d D:\EasyTier && cargo test -p easytier-core --lib peers::error::tests'
```

或打开：

`...\BuildTools\Common7\Tools\Launch-VsDevShell.ps1` / `LaunchDevCmd.bat`

成功标志：`where cl` 指向 `...\MSVC\...\bin\Hostx64\x64\cl.exe`，且 `cl` 能打印编译器横幅。

### `ring` 仍失败：MSVC `D8050`（环境块过大）

已进 vcvars、手工 `cl -c` 某个 `.c` 也成功，但 `cargo` 编 `ring` 报：

`cl: 致命错误 D8050 : 无法执行 …\c1.dll : 未能解析命令行参数记录`

常见原因：**进程环境块过大**（vcvars 注入大量 `WindowsSdk*` / `Framework*` 变量，再叠加 Cursor / 长 `PATH`、长 `CARGO_TARGET_DIR`）。手工 `cl` 环境小能编过；`cargo` 的 build-script 子进程环境更大 → D8050。

处理（任选）：

1. 缩短 `PATH` / `INCLUDE` / `LIB`，只保留 MSVC + UCRT + `cargo`/`rustc` + `system32`；
2. 使用短 `CARGO_TARGET_DIR`（如 `C:\et`），避免 `D:\…\target\debug\build\ring-…\out\…` 把命令行顶满；
3. 清掉不必需的 vcvars 派生变量（保留 `VCINSTALLDIR`、`VSCMD_ARG_TGT_ARCH`，`ring` build.rs 会读）；
4. 确保 `PROTOC` 指向本机 `protoc.exe`（否则后续 `prost-wkt-types` 会另报错）。

---

## Agent / 文档约定

- 文档里写「本机 MSVC/ring 编译受阻」时，优先排查 **是否未进 vcvars**，再谈 **D8050 / 环境过大**，最后才谈缺组件。
- **不要**在未加载开发者环境的普通终端里判定「机器没有 MSVC」。
- 缺组件时：用 VS Installer 给 Build Tools 勾选 **「使用 C++ 的桌面开发」** / **MSVC v143** + **Windows 10/11 SDK**。

---

## 与发版 CI 的关系

发版流水线（如 fork 上 **ET Test**）自带完整 MSVC；本机未 vcvars 失败**不能**用来否定 CI 或跳过应在本机复现的回归。pin / credential 等用例仍以 CI 或已 vcvars 的本机结果为准。
