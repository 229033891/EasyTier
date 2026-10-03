# EasyTier：基于 GitHub 产物的安装与升级方案

## Status

- Status: **Roadmap**（方案文档；与现有脚本并存）
- 最近审阅：2026-10-03
- 目标：后续**以 GitHub Actions / Release / GHCR 为唯一发版来源**，统一安装与升级路径
- 索引：[`../README.md`](../README.md)
- 配套：
  - [`../ops/deploy-install.md`](../ops/deploy-install.md)（Linux 交互脚本用法）
  - [`../ops/web-upgrade.md`](../ops/web-upgrade.md)（控制台保库升级）
  - `docker-compose.yml`（Docker 部署示例）
  - `script/easytier-install.sh` / `script/install.ps1`

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
releases/vX.Y.Z ──(push)──► ET Linux + Windows + …
       │
       └─► ET Docker（自动 workflow_run）
       └─► ET Release（汇总 Artifact → GitHub Release）
```

**安装脚本只认 Release**（有版本号、可回滚、国内镜像可缓存）。Artifact 仅供开发自测与打 Docker。

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

### 3.2 Windows 无头 / GUI

| 路径 | 安装 | 升级 |
|------|------|------|
| 无头 | `script/install.ps1` 从 Release 拉 `ET-windows-*` → PATH；`ET-cli service install` | 再跑 `install.ps1` 覆盖；或停服务后解压覆盖 |
| GUI | 安装 `ET-gui-*.exe`（NSIS） | 再装新包；钩子会结束托盘进程并删除 `ET-Gui` 服务 |

### 3.3 Docker（NAS / 单机容器）

入口：仓库根目录 `docker-compose.yml`

| 角色 | 命令要点 |
|------|----------|
| Console | `entrypoint` 覆盖为 `ET-web-embed`；`--api-server-port=8080`；挂载 `./et-console-data` |
| Node | `network_mode: host` + `/dev/net/tun`；`--daemon --config-dir /config --config-server … [--secure-mode]` |

安装：

```bash
# 1. 登录 GHCR（私有包时）
echo $GHCR_TOKEN | docker login ghcr.io -u 229033891 --password-stdin

# 2. 改 image tag / 控制台地址后
docker compose up -d console
docker compose --profile node up -d node
```

升级：

```bash
docker compose pull          # 或 docker pull ghcr.io/229033891/et:<新tag>
docker compose up -d         # 重建容器；console 数据卷保留
```

从旧 `easytier/easytier:latest` 迁移：

1. 镜像改为 `ghcr.io/229033891/et:<tag>`
2. `-d` → `--daemon`
3. 挂载了配置目录则加 `--config-dir /config`
4. 控制台二进制名必须是 `ET-web-embed`（大小写敏感）

### 3.4 Android

从 `ET Android` Artifact / Release 安装 APK；升级 = 安装新 APK（配置一般在应用数据内保留）。

---

## 4. 升级检查清单（通用）

1. **确认来源**：Release tag 或明确的镜像 tag（勿混用未知 `latest`）。
2. **备份**：Server 先 `easytier-install.sh backup` 或拷贝 `et.db` / Docker volume。
3. **停写**：升级窗口内避免改网络配置。
4. **替换**：只换二进制/镜像；保留 db、config、machine-id。
5. **验证**：
   - Server：`healthcheck` 或打开 Web + 配置端口 UDP/TCP 22020
   - Client：能连上 config-server；`ET-cli peer` / 控制台设备在线
6. **回滚**：保留上一版二进制或镜像 tag；db 用升级前备份恢复。

---

## 5. 运维命令速查

| 动作 | Linux 脚本 | Docker |
|------|------------|--------|
| 安装 | `easytier-install.sh install` | `compose up -d` |
| 升级 | `easytier-install.sh update` | `compose pull && up -d` |
| 备份 | `easytier-install.sh backup` | 备份 volume 目录 |
| 健康检查 | `easytier-install.sh healthcheck` | `docker logs` + 打开 :8080 |
| 卸载 | `easytier-install.sh uninstall` | `compose down`（加 `-v` 才会删库） |

Windows GUI：设置里切换模式 / 卸载服务；系统卸载走 NSIS（新包会清 `ET-Gui`）。

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

正式：`releases/*` 上 Linux 成功后，`ET Docker` 由 `workflow_run` 自动推送；再跑 `ET Release` 挂 zip。

---

## 7. 决策摘要

| 问题 | 答案 |
|------|------|
| 以后包从哪来？ | **GitHub Actions →（正式）Release / GHCR** |
| 生产装什么？ | Linux：**安装脚本 + Release**；容器：**compose + GHCR**；桌面：**GUI NSIS** |
| 升级怎么做？ | 换包/换镜像，**保 db 与 config**；脚本用 `update`，Docker 用 `pull + up` |
| 旧 Docker 配置？ | 按 `docker-compose.yml` 头部「迁移注意」改镜像名与参数 |

---

## 8. 后续可选增强（未实现）

1. 安装脚本增加 `install --source docker`（拉镜像写 systemd + compose）。
2. Release 流程文档化「打 tag → 等四端 CI → 一键 Release」checklist。
3. compose 用 `.env` 统一 `IMAGE_TAG` / `CONFIG_SERVER`，避免硬编码 NAS 主机名。
