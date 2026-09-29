# EasyTier Web 打包与部署

## Status

- 状态：Active
- 适用范围：修改 `easytier-web` 后如何打出可在 **Windows / Linux** 部署的产物
- 命名：产品正式名为 **`easytier-web`**
- 配套：
  - `docs/easytier-web-upgrade-guide.md`（现网升级与保库）
  - `docs/easytier-web-evolution-roadmap.md`（演进路线）

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
1. 构建前端  →  easytier-web/frontend/dist/
2. 编译 Rust（--features embed）→  得到 embed 二进制
3. 拷贝到目标机（Win 或 Linux）
4. 用绝对路径 --db 指向原库（或新库）启动
```

### 2.1 构建前端（仓库根目录）

需要：Node.js、pnpm。

```bash
# 在 EasyTier 仓库根目录
pnpm -r install
pnpm -r --workspace-concurrency=1 --filter "./easytier-web/*" build
```

成功后应存在：`easytier-web/frontend/dist/`（CI 还会把 `config-generator` 拷进该目录；本地若做过完整 filter build，通常已包含）。

### 2.2 编译 `easytier-web-embed`

需要：Rust（见仓库 `rust-toolchain.toml`）。

**在本机对应平台原生编译**（最省事）：

```bash
# Windows（PowerShell / 在仓库根目录）
cargo build --release --package easytier-web --features embed
# 产物：target\release\easytier-web.exe
# 可改名为 easytier-web-embed.exe 以区分

# Linux
cargo build --release --package easytier-web --features embed
# 产物：target/release/easytier-web
# 可改名为 easytier-web-embed
```

说明：

1. **必须先有 `frontend/dist`**，否则 embed 可能缺页面或构建不符合预期。
2. **Windows 包在 Windows 上编，Linux 包在 Linux 上编**（或走 CI）。本机交叉编译可行但工具链复杂，短期不推荐。
3. 若只要 API、前端另外部署：去掉 `--features embed`，得到纯 `easytier-web`。

### 2.2.1 为何很慢、如何加快（本地迭代）

仓库默认 `[profile.release]` 开了 **全量 LTO**（`lto = true`）且 `codegen-units = 1`，链接阶段往往要数分钟，这是「每次打包很久」的主因，不是前端 pnpm 慢。

| 场景 | 建议命令 | 说明 |
|------|----------|------|
| **日常改 UI / 试功能** | `cargo build --profile release-fast -p easytier-web --features embed` | thin LTO，链接快很多；产物在 `target/release-fast/` |
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

仓库已有 `.github/workflows/core.yml`：

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

细节：`docs/easytier-web-upgrade-guide.md`。

---

## 6. 常见问题

| 问题 | 处理 |
|------|------|
| 打开网页 404 / 空白 | 是否用了 embed？前端是否先 build 再编 Rust？ |
| 升级后像空库 | `--db` 或工作目录变了；改回绝对路径 |
| 只有 Windows 开发机，要 Linux 包 | 用 CI 打 Linux musl，或在 Linux 机器/容器里编 |
| 要不要安装 Rust 到生产机 | **不要**；生产机只跑编译好的二进制 |

---

## 7. 一句话

**改 `easytier-web` 后：先 pnpm 编前端，再 `cargo build --release -p easytier-web --features embed`，把得到的 embed 二进制拷到 Win/Linux，配合原有 `et.db` 启动即可。双平台正式包优先走现有 CI。**
