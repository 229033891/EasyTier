# EasyTier：基于 GitHub 产物的安装与升级方案

## Status

- Status: **Roadmap / Partially landed**（`ET Release` 对 `releases/v*` 自动 Publish **已落地**；客户端内更新、统一安装脚本默认源等仍属后续）
- 最近审阅：2026-10-09
- 现状对照：发版流水线见 `.github/workflows/release.yml`；安装脚本用法见 [`../ops/deploy-install.md`](../ops/deploy-install.md)。
- 目标：后续**以 GitHub Actions / Release / GHCR 为唯一发版来源**，统一安装与升级路径
- 索引：[`../README.md`](../README.md)
- 配套：
  - [`../ops/deploy-install.md`](../ops/deploy-install.md)（Linux 交互脚本用法）
  - [`../ops/web-upgrade.md`](../ops/web-upgrade.md)（控制台保库升级）
  - `docker-compose.yml`（Docker 部署示例）
  - `script/install.sh` / `script/install.ps1`
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
2. **对外安装**：走 **Release 附件**（`script/install.sh` / `install.ps1` 默认读 `229033891/EasyTier` 的 latest/tag）。
3. **Docker**：镜像源为 `ghcr.io/229033891/et:<tag>`（`docker-compose.yml` 默认直连 GHCR）。DaoCloud 公共加速对该镜像不在白名单，不可用。勿再使用历史第三方同名镜像（ENTRYPOINT / 二进制名不同）。

---

## 2. 推荐发版流水线

```text
dev  ──(手动)──►  ET Linux / Windows / Android
       │                      │
       │                      └─► ET Docker（填 Linux run_id，打 tag 如 dev）
       │
releases/vX.Y.Z ──(push)──► ET Linux + Windows + Android（并行 Artifact）
       │
       └─► **ET Release 自动**（三端同 commit 均 success）
             └─► 直接 **Publish** Release（tag = 分支名去掉 releases/，如 v2.7.1）
             └─► 若同 SHA 上 OpenWrt 也成功，一并附上
```

**安装脚本只认已 Publish 的 Release**（有版本号、可回滚、国内镜像可缓存）。Artifact 仅供开发自测与打 Docker。

### 2.1 已落地：`releases/v*` 自动 Publish

Workflow：`.github/workflows/release.yml`（`ET Release`）

**自动路径**（`workflow_run` 定义须在默认分支 `main`）：

1. 推送 / 更新 `releases/vX.Y.Z` → 自动跑 ET Linux / Windows / Android  
2. 任一端成功结束 → 唤醒 `ET Release`  
3. 该 commit 上**三端都已 success** → 打包并 **Publish**（`make_latest`）  
4. 尚有端未完成 → 退出等待下一次唤醒  
5. 同 tag 已发布 → 跳过（防重复）

**手动路径**仍可用：Actions → ET Release；`publish=false` 时只建 Draft。

```bash
gh workflow run "ET Release" --ref main \
  -f source_branch=releases/v2.7.1 \
  -f publish=true
```

---

## 3. 安装与升级（运维细节）

Linux / Windows 二进制安装、systemd 模式、`install.sh` / `update.sh` 交互流程：**以 Ops 为准** → [`../ops/deploy-install.md`](../ops/deploy-install.md)。

Docker：`docker-compose.yml` + [`../ops/docker-compose-deploy.md`](../ops/docker-compose-deploy.md)。升级以换镜像 tag + `pull` / `up` 为主，**挂载卷保库**。

本文 §1–§2 侧重 **产物矩阵与发版流水线**；§4 起为客户端内更新与后续增强。

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
| **P0** | `releases/v*` 三端成功 → **自动 Publish** Release | **已落地** | 中 | — |
| **P1** | Linux `update` 体验打磨（版本提示/changelog）；Web **仅提示**有新版+链到 Release | 后续 | 低～中 | 2～4 人日 |
| **P2** | Windows GUI Tauri Updater（签名、停服务、可回滚） | 后续 | 中高 | 1～2 人周 |
| **P3** | Android 应用内更新 | 后续 | 高 | 1～3 人周+ |
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
  -f image_tag=dev \
  -f mark_latest=false \
  -f mark_unstable=true
```

正式：`releases/*` 上 Linux 成功后，`ET Docker` 可由 `workflow_run` 自动推送；包体对外仍建议走 **Publish 后的 Release**。

---

## 7. 决策摘要

| 问题 | 答案 |
|------|------|
| 以后包从哪来？ | **GitHub Actions →（正式）Release / GHCR** |
| Artifact 会自动变 Release 吗？ | **`releases/v*` 上三端同 commit 成功后会自动 Publish**；其它分支仍可手动跑 `ET Release` |
| 生产装什么？ | Linux：**安装脚本 + Release**；容器：**compose + GHCR**；桌面：**GUI NSIS** |
| 升级怎么做（今天）？ | 换包/换镜像，**保 db 与 config**；脚本用 `update`，Docker 用 `pull + up` |
| 客户端内一键更新？ | **后续**（见 §5）；近中期不做 GUI/Android 静默更新 |

---

## 8. 其它可选增强（未实现）

1. 安装脚本增加 `install --source docker`（拉镜像写 systemd + compose）。  
2. compose 用 `.env` 统一 `IMAGE_TAG` / `CONFIG_SERVER`，避免硬编码 NAS 主机名。  

---

## 9. 修订记录

| 日期 | 说明 |
|------|------|
| 2026-10-03 | P0：`ET Release` Draft 聚合 |
| 2026-10-03 | `releases/v*`：三端 success → 自动 Publish；`workflow_run` 须在 `main` |
