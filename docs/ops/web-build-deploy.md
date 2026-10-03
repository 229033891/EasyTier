# EasyTier Web 打包与部署

## Status

- Status: **Ops**
- 最近审阅：2026-10-03
- 适用范围：修改 `easytier-web` 后如何打出可在 **Windows / Linux** 部署的产物
- 命名：产品正式名为 **`easytier-web`**
- 索引：[`../README.md`](../README.md)
- 配套：
  - [`web-upgrade.md`](./web-upgrade.md)（现网升级与保库）
  - [`../roadmap/web-evolution.md`](../roadmap/web-evolution.md)（演进路线）

---

## 0. ⚠️ 重要：不要默认直接生成 exe

**默认不要执行 `cargo build` / `cargo build --release` 去生成 `easytier-web.exe`。** 全量编译 + 链接耗时很长（见 [2.2.1](#221-为何很慢如何加快本地迭代)），在未确认需求前直接开编会浪费大量时间。

正确做法：

1. **先问用户**：是否需要现在就生成 exe / 可部署产物？
2. **问清楚目标**：平台（Windows / Linux）、用途（本地试跑 / 正式发版）、是否要 embed 一体包。
3. **得到用户明确同意后**，再按 [第 2 节](#2-推荐流程本地改完--可部署) 构建；日常迭代优先用 `release-fast`，而不是直接 `--release`。

> 只有当用户明确要求「打包 / 生成 exe / 出可部署产物」时才编译。仅改代码、排查问题、看类型或构建配置时，不要顺手触发全量编译。

---

## 1. 你要交付什么

推荐默认交付：**一体包** `easytier-web-embed`

| 产物 | 内容 | 适用 |
|------|------|------|
| **`easytier-web-embed`**（推荐） | 配置服务器 + REST API + 前端 UI（已嵌入） | Win/Linux 单文件部署 |
| `easytier-web` | 仅 API + 配置服务器，不含前端 | 前后端分离；前端另用 nginx/静态托管 |

业务数据**不在**二进制里，在 SQLite（默认 `et.db`）。打包只换程序，升级时保留原库。见升级指南。

---

## 2. 推荐流程（本地改完 → 可部署）

```text
0. 先把构建依赖备到本地（避免构建中途卡在下载）
1. 构建前端  →  easytier-web/frontend/dist/
2. 编译 Rust（--features embed）→  得到 embed 二进制
3. 拷贝到目标机（Win 或 Linux）
4. 用绝对路径 --db 指向原库（或新库）启动
```

### 2.0 Windows 一键脚本（推荐本地打包）

仓库提供三个固定入口（**前台运行**，日志打在控制台）：

```bat
script\easytier-web-debug.cmd      REM 调试：cargo run API + Vite（不打 exe）
script\easytier-web-fast.cmd       REM 日常打包：release-fast + embed
script\easytier-web-release.cmd    REM 正式包：--release + embed
```

或直接调 PowerShell：

```powershell
.\script\build-easytier-web.ps1 -Dev
.\script\build-easytier-web.ps1 -Profile Fast
.\script\build-easytier-web.ps1 -Profile Release
.\script\build-easytier-web.ps1 -Interactive   # 仍可用交互菜单
```

**建议约定：**

1. **先备齐本地依赖，再开始编**（见 [2.0.1](#201-构建前先备齐本地依赖)）。不要指望构建过程中边编边从 GitHub 拉取工具链——网络不稳时会卡很久或直接失败。
2. **用前台窗口跑**（双击 cmd / 在已打开的终端里跑），不要后台静默执行，便于对照阶段输出判断卡在哪一步。
3. **依赖已齐 + Fast（release-fast）时，正常大约 5 分钟可完成**（含前端 + embed）。首次全量或 Release（全量 LTO）会更久。
4. 前端 `dist` 已是最新时，可用 `.\script\build-easytier-web.ps1 -SkipFrontend -Profile Fast` 只重编 Rust。

产物（`Fast`）：`target\release-fast\easytier-web-embed.exe`  
产物（`Release`）：`target\release\easytier-web-embed.exe`

#### 2.0.1 构建前先备齐本地依赖

脚本会在编译前检查并优先复用本地工具；**请先一次性下好放到约定路径**，之后构建不再依赖外网下载：

| 依赖 | 用途 | 本机放置 / 要求 |
|------|------|-----------------|
| **Node.js + pnpm** | 前端 | 已安装且在 PATH |
| **Rust**（`rust-toolchain.toml`） | 编 `easytier-web` | `rustup` 已装好 |
| **wasm-pack** | `config-generator` WASM | `cargo install wasm-pack` 或 PATH 可用 |
| **wasm-bindgen CLI** | wasm-pack 胶水 | 版本需与 `Cargo.lock` 一致（当前多为 `0.2.128`）；放到 `%USERPROFILE%\.cargo\bin\` 或 `script\tools\wasm-bindgen-<ver>\` |
| **Binaryen / wasm-opt** | WASM 优化 | `wasm-pack` 固定 **Binaryen `version_117`**。下载 [binaryen-version_117-x86_64-windows.tar.gz](https://github.com/WebAssembly/binaryen/releases/download/version_117/binaryen-version_117-x86_64-windows.tar.gz)，放到 `script\tools\`（脚本会自动解压），或保证存在 `script\tools\binaryen-version_117\bin\wasm-opt.exe` |
| **7-Zip（`7z`）** | Windows 上 `thunk-rs` 解包 VC-LTL / YY-Thunks | 安装后保证 `7z.exe` 在 PATH，或装在 `C:\Program Files\7-Zip\`（脚本会自动加入 PATH） |
| **VC-LTL / YY-Thunks / protoc（可选缓存）** | 避免 thunk / protobuf 构建时再联网 | 可通过环境变量指向本地缓存：`VC_LTL`、`YY_THUNKS`、`PROTOC`；脚本也会尝试 `%USERPROFILE%\.cache\...` 与仓库 `.deps-cache\` |

> 构建中途若出现「Installing wasm-bindgen / downloading binaryen / 7z not found」一类错误，应停下来把对应依赖补到上表路径，**不要反复空转重试下载**。

### 2.1 构建前端（仓库根目录）

需要：Node.js、pnpm。

```bash
# 在 EasyTier 仓库根目录
pnpm -r install
pnpm -r --workspace-concurrency=1 --filter "./easytier-web/*" build
```

成功后应存在：`easytier-web/frontend/dist/`（CI 还会把 `config-generator` 拷进该目录；本地若做过完整 filter build，通常已包含）。

### 2.2 编译 `easytier-web-embed`

> ⚠️ 动手前先确认：见 [第 0 节](#0-️-重要不要默认直接生成-exe)。**未经用户同意，不要直接跑编译生成 exe。**

需要：Rust（见仓库 `rust-toolchain.toml`）。

**在本机对应平台原生编译**（最省事）：

```bash
# Windows（PowerShell / 在仓库根目录）— 日常迭代推荐 release-fast
cargo build --profile release-fast --package easytier-web --features embed
# 产物：target\release-fast\easytier-web.exe（脚本会另存为 easytier-web-embed.exe）

# 正式发版再用全量 release（更慢）
cargo build --release --package easytier-web --features embed
# 产物：target\release\easytier-web.exe

# Linux
cargo build --profile release-fast --package easytier-web --features embed
# 或 --release；产物在对应 profile 目录下
```

说明：

1. **必须先有 `frontend/dist`**，否则 embed 可能缺页面或构建不符合预期。
2. **Windows 包在 Windows 上编，Linux 包在 Linux 上编**（或走 CI）。本机交叉编译可行但工具链复杂，短期不推荐。
3. 若只要 API、前端另外部署：去掉 `--features embed`，得到纯 `easytier-web`。

### 2.2.1 为何很慢、如何加快（本地迭代）

仓库默认 `[profile.release]` 开了 **全量 LTO**（`lto = true`）且 `codegen-units = 1`，链接阶段往往要数分钟，这是「每次打包很久」的主因，不是前端 pnpm 慢。

| 场景 | 建议命令 | 说明 |
|------|----------|------|
| **日常改 UI / 试功能** | `cargo build --profile release-fast -p easytier-web --features embed` | thin LTO，链接快很多；产物在 `target/release-fast/`；依赖齐时整包约 **5 分钟** |
| **正式发版 / 生产包** | `cargo build --release -p easytier-web --features embed` | 体积与优化最好，慢可接受 |
| **只改了 frontend 页面、lib 未动** | 在 `easytier-web/frontend` 执行 `pnpm exec vite build`，再编 Rust | 跳过 `frontend-lib` 全量重建 |
| **frontend-lib 也改了** | `pnpm build`（frontend 目录）再编 Rust | 需要完整前端构建 |

首次 `release-fast` 仍会编依赖；之后只改 `easytier-web` / 前端时，通常只需重编 web 包并快速链接。

### 2.3 发到目标机的最小文件集

一体部署至少需要：

```text
easytier-web-embed(.exe)     # 程序
# 以及你已有的：
et.db                        # 升级时保留，不要覆盖成空库
# 可选：启动脚本、systemd unit、环境变量文件
```

不需要把整个源码仓库拷到生产机。

---

## 3. 启动示例

### Windows

```powershell
.\easytier-web-embed.exe --db D:\easytier-web\data\et.db
```

默认大致：

- 配置服务器：`22020`
- API /（embed 时）UI：`11211`

浏览器访问：`http://<服务器IP>:11211`（以实际参数为准）。

### Linux

```bash
chmod +x ./easytier-web-embed
./easytier-web-embed --db /var/lib/easytier-web/et.db
```

可用 systemd 做成服务；关键是 **WorkingDirectory / `--db` 绝对路径** 固定，避免连错库。

---

## 4. 用 CI 打 Win + Linux 双平台包（推荐发正式包时）

仓库已有 `.github/workflows/linux.yml`：

1. 先 build 前端 artifact  
2. 再在 matrix 上编译，其中包含：
   - `x86_64-pc-windows-msvc` → Windows x64
   - `x86_64-unknown-linux-musl` 等 → Linux  
3. 对 web：`cargo build --release --target $TARGET --package=easytier-web --features=embed`，再改名为 `easytier-web-embed`

操作方式：

- push 到相关分支触发，或  
- GitHub Actions 里 **workflow_dispatch** 手动跑 **EasyTier Core**

从 Actions artifact / Release 下载对应平台的 `easytier-web-embed`，拷到服务器即可。

---

## 5. 现网升级时怎么用打包结果

1. 备份目标机上的 `et.db`（及 wal/shm）  
2. 停旧进程  
3. **只替换** `easytier-web-embed` 二进制  
4. **同一 `--db` 路径**启动  
5. 验证登录与节点  

细节：[`web-upgrade.md`](./web-upgrade.md)。

---

## 6. 常见问题

| 问题 | 处理 |
|------|------|
| 打开网页 404 / 空白 | 是否用了 embed？前端是否先 build 再编 Rust？ |
| 升级后像空库 | `--db` 或工作目录变了；改回绝对路径 |
| 只有 Windows 开发机，要 Linux 包 | 用 CI 打 Linux musl，或在 Linux 机器/容器里编 |
| 要不要安装 Rust 到生产机 | **不要**；生产机只跑编译好的二进制 |
| 卡在 Installing wasm-bindgen / 下载 binaryen | 先按 [2.0.1](#201-构建前先备齐本地依赖) 放到本地，再前台重跑脚本 |
| `7z is needed to unpack libraries` | 安装 7-Zip，或确认 `C:\Program Files\7-Zip\7z.exe` 存在后用 `script\easytier-web-fast.cmd`（会自动加 PATH） |
| 构建要多久 | 依赖齐 + `release-fast`：**大约 5 分钟**；全量 `--release` 或首次冷编译更久 |

---

## 7. 一句话

**改 `easytier-web` 后：先备齐本地依赖，再用 `script\easytier-web-fast.cmd` 前台构建（日常）；正式包用 `script\easytier-web-release.cmd`。或手动先 pnpm 编前端，再 `cargo build --profile release-fast -p easytier-web --features embed`。依赖齐时约 5 分钟完成。把 embed 二进制拷到目标机，配合原有 `et.db` 启动即可。双平台正式包优先走现有 CI。**

**但注意：不要一上来就编 exe —— 先问用户是否需要生成产物、要哪个平台，得到确认后再动手，避免浪费大量编译时间（见第 0 节）。**
