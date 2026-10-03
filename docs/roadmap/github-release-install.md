# EasyTier：基于 GitHub 产物的安装与升级方案

## Status

- Status: **Roadmap**（发版聚合已部分落地；客户端内更新仍属后续）
- 最近审阅：2026-10-03
- 目标：后续**以 GitHub Actions / Release / GHCR 为唯一发版来源**，统一安装与升级路径
- 索引：[`../README.md`](../README.md)
- 配套：
  - [`../ops/deploy-install.md`](../ops/deploy-install.md)（Linux 交互脚本用法）
  - [`../ops/web-upgrade.md`](../ops/web-upgrade.md)（控制台保库升级）
  - `docker-compose.yml`（Docker 部署示例）
  - `script/easytier-install.sh` / `script/install.ps1`
  - 讨论材料：[`discussion-proposal-2026-10.md`](./discussion-proposal-2026-10.md)、[`market-comparison-2026-10.md`](./market-comparison-2026-10.md)

---

## 1. 产物矩阵（GitHub 生成）

| 渠道 | Workflow | 产物 | 用途 |
|------|----------|------|------|
| Linux 无头包 | `ET Linux` | Artifact `ET-linux-x86_64`：`ET-core` / `ET-cli` / `ET-web-embed` | VPS / NAS 二进制部署 |
| Windows 无头包 | `ET Windows` | Artifact `ET-windows-x86_64`：三个 exe + `wintun`/`Packet`/`WinDivert` | Windows 服务节点 / CLI |
| Windows GUI | `ET Windows` | Artifact `ET-gui-windows-x86_64`：NSIS 安装包 | 桌面客户端 |
| Android | `ET Android` | APK | 手机端 |
| Docker 镜像 | `ET Docker` | `ghcr.io/<owner>/et:<tag>`（可选 Docker Hub） | 容器部署 |
| 正式 Release | `ET Release` | GitHub Release 附件 zip | **对外安装脚本默认下载源** |

约定：

1. **日常验证**：`workflow_dispatch` 打 Artifact；Docker 用对应 `ET Linux` 的 `run_id` 手动触发。
2. **对外安装**：走 **Release 附件**（`script/easytier-install.sh` / `install.ps1` 默认读 `229033891/EasyTier` 的 latest/tag）。
3. **Docker**：优先 `ghcr.io/229033891/et:<tag>`；勿再依赖旧镜像 `easytier/easytier`（ENTRYPOINT / 二进制名不同）。

---

## 2. 推荐发版流水线

```text
dev / dev-en  ──(手动)──►  ET Linux / Windows / Android
       │                      │
       │                      └─► ET Docker（填 Linux run_id，打 tag 如 dev-en）
       │
releases/vX.Y.Z ──(push)──► ET Linux + Windows + Android（并行 Artifact）
       │
       └─► 手动跑 ET Release（可自动发现上述成功 run）
             └─► 创建 **Draft** Release + 附件
             └─► 人工检查 SHA/附件后 Publish（安装脚本才看得到 latest）
```

**安装脚本只认已 Publish 的 Release**（有版本号、可回滚、国内镜像可缓存）。Artifact 仅供开发自测与打 Docker。

### 2.1 已落地：低风险 Draft 聚合（P0）

Workflow：`.github/workflows/release.yml`（`ET Release`）

用法（三端在同一 `releases/…` 分支都构建成功后）：

1. Actions → **ET Release** → Run workflow  
2. 填写：
   - `version`：如 `v2.7.0`（将作为 tag / Release 名）
   - `source_branch`：如 `releases/v2.7.0`（用于查找最新成功构建；可留空则用当前触发 ref）
   - `core_run_id` / `gui_run_id` / `mobile_run_id`：默认 `0` = **自动发现**该分支上对应 workflow 最近一次 success；需要钉死某次构建时再填具体 id  
3. 成功后仓库出现 **Draft** Release，正文含各 run 链接与 `headSha`  
4. **人工 Publish**（本 workflow **不会** `make_latest`，避免误发）

仍可用显式 `run_id` 覆盖，兼容旧操作习惯。

---

## 3. 安装方案（按场景）

### 3.1 Linux 控制台 / 节点（二进制，推荐生产）

入口：`script/easytier-install.sh`（文档：[`../ops/deploy-install.md`](../ops/deploy-install.md)）

| 模式 | 安装 | 升级 |
|------|------|------|
| Server（控制台） | `install --mode server` → systemd `ET-web` + 可选本机节点 | `update`：下 Release 包 → 替换 `/opt/easytier/ET-*` → 重启服务；**保留 `et.db`** |
| Client（节点） | `install --mode client` → systemd `ET-core@…` + `--config-server` | 同上 `update` |

一键示例：

```bash
# 控制台
sudo bash script/easytier-install.sh install --mode server --auto \
  --public-host et.example.com

# 节点
sudo bash script/easytier-install.sh install --mode client --auto \
  --server-host 'udp://et.example.com:22020/admin'

# 升级（已安装机器）
sudo bash script/easytier-install.sh update
```

升级原则：**先停服务 → 换二进制 → 启服务 → healthcheck**；数据库与 config 目录不覆盖。

### 3.2 Windows

- 无头：Release 中 `ET-windows-*.zip`  
- 桌面：NSIS（`ET-gui-*`）；安装钩子会停 `ET-Gui` / 旧 `easytier-gui` 服务后再覆盖  

### 3.3 Docker

见仓库根目录 `docker-compose.yml`；升级以换镜像 tag + `pull` / `up` 为主，**挂载卷保库**。

---

## 4. 客户端内「检查更新」能力矩阵（现状）

| 终端 | 现状 | 备注 |
|------|------|------|
| Linux 脚本 | **有** `update` 读 Release | 半自动，需 SSH/运维执行 |
| Windows GUI | **无** Tauri Updater（`createUpdaterArtifacts: false`） | 需重装 NSIS |
| Android | **无** 应用内 GitHub 更新 | APK 手动或商店 |
| Web UI | **无** 连 Release 的检测 | 仅有 i18n 残留文案可能 |
| Docker | 镜像拉取 | 不走 Release zip |

---

## 5. 分阶段：已做 / 后续（工作量与风险）

| 优先级 | 项 | 状态 | 风险 | 粗估 |
|--------|----|------|------|------|
| **P0** | `ET Release` 自动发现三端成功 run → **Draft** Release | **已落地** | 低 | — |
| **P1** | Linux `update` 体验打磨（版本提示/changelog）；Web **仅提示**有新版+链到 Release | 后续 | 低～中 | 2～4 人日 |
| **P2** | Windows GUI Tauri Updater（签名、停服务、可回滚） | 后续 | 中高 | 1～2 人周 |
| **P3** | Android 应用内更新 | 后续 | 高 | 1～3 人周+ |
| — | 构建完成后**全自动 Publish + latest**（无人确认） | **不做**（默认） | 高 | — |
| — | Core/Web 进程内静默自替换二进制 | **不做**（默认） | 高 | — |

### 5.1 后续实现时注意

1. **签名与供应链**：GUI/Android 更新必须校验；无私钥与 HTTPS 哈希则勿开自动下载安装。  
2. **版本语义**：gui / web / core 是否同 tag；混升时的协议兼容说明。  
3. **国内 GitHub 访问**：客户端检测需复用安装脚本的镜像策略。  
4. **运行中替换**：复用 NSIS/`sc stop ET-Gui` 经验；Web 更新优先指引到脚本而非进程自杀替换。

---

## 6. Docker 镜像如何打出（运维）

手动（开发分支验证）：

```bash
# 先有一次成功的 ET Linux（Artifact: ET-linux-x86_64）
gh workflow run "ET Docker" --ref <branch> \
  -f run_id=<Linux_run_id> \
  -f image_tag=dev-en \
  -f mark_latest=false \
  -f mark_unstable=true
```

正式：`releases/*` 上 Linux 成功后，`ET Docker` 可由 `workflow_run` 自动推送；包体对外仍建议走 **Publish 后的 Release**。

---

## 7. 决策摘要

| 问题 | 答案 |
|------|------|
| 以后包从哪来？ | **GitHub Actions →（正式）Release / GHCR** |
| Artifact 会自动变 Release 吗？ | **不会自动 Publish**；`ET Release` 可自动发现并生成 **Draft**，人工发布 |
| 生产装什么？ | Linux：**安装脚本 + Release**；容器：**compose + GHCR**；桌面：**GUI NSIS** |
| 升级怎么做（今天）？ | 换包/换镜像，**保 db 与 config**；脚本用 `update`，Docker 用 `pull + up` |
| 客户端内一键更新？ | **后续**（见 §5）；近中期不做 GUI/Android 静默更新 |

---

## 8. 其它可选增强（未实现）

1. 安装脚本增加 `install --source docker`（拉镜像写 systemd + compose）。  
2. compose 用 `.env` 统一 `IMAGE_TAG` / `CONFIG_SERVER`，避免硬编码 NAS 主机名。  
3. Release checklist bot：三端 success 后评论「可跑 ET Release」提醒（仍不自动 Publish）。

---

## 9. 修订记录

| 日期 | 说明 |
|------|------|
| 2026-10-03 | P0：`ET Release` 支持按分支自动发现 run → Draft；客户端 Updater 列入 P1–P3 后续 |
