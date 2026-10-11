# Docker Compose 部署（节点 / 控制台）

## Status

- Status: **Ops**
- 最近审阅：2026-10-09
- 索引：[`../README.md`](../README.md)
- 配套：仓库根目录 [`docker-compose.yml`](../../docker-compose.yml)

单镜像（`ET-core` / `ET-cli` / `ET-web-embed`），**两个独立容器**：

| 服务 | 进程 | 典型场景 |
|------|------|----------|
| `node` | `ET-core` | NAS / VPS 组网节点（**compose 默认启动**） |
| `console` | `ET-web-embed` | 自托管 Web 控制台 + 配置下发 |

镜像默认：`ghcr.io/229033891/et:<Release tag>`。  
DaoCloud（`m.daocloud.io`）对该镜像**不在白名单**，拉镜像会报 `not in the allowlist`，勿再当作默认加速源。

总方案：[`../roadmap/github-release-install.md`](../roadmap/github-release-install.md)。Linux 二进制安装见 [`deploy-install.md`](./deploy-install.md)。

---

## 1. 准备（首次）

只需准备部署目录并放入 compose（**挂载目录可由脚本自动创建**）：

```bash
export DEPLOY_DIR=/vol1/1000/docker/easytier
mkdir -p "$DEPLOY_DIR"
cd "$DEPLOY_DIR"
curl -fsSL -o docker-compose.yml \
  https://github.com/229033891/EasyTier/raw/main/docker-compose.yml
```

**节点**启动前请确认：

1. `docker-compose.yml` 里 `--config-server` / `ET_CONFIG_SERVER` 指向你的控制台（任选其一：`udp://et.example.com:22020/admin` 或 `tcp://et.example.com:22020/admin`；控制台默认 `--config-server-protocol=udp,tcp`）
2. 节点 config 挂载路径与 compose 一致（默认 `/vol1/1000/docker/easytier/config`，按 NAS UID 修改）；`script/docker-up.sh` 会在启动前 `mkdir -p`
3. 节点需要 **host 网络、特权、`/dev/net/tun`**（compose 已配置）

**控制台**首次启动后浏览器访问：`http://<主机IP>:8080`；配置下发端口 **UDP 22020 与 TCP 22020** 均须可达（客户端 URL 任选一种协议）。

---

## 2. SSH 一键命令

推荐用 [`script/docker-up.sh`](../../script/docker-up.sh)：**自动解析 volumes 挂载并 `mkdir -p`**，无 Docker 权限时自动加 `sudo`，再 `compose up`。

旧版 Docker 若不支持 `docker compose`，脚本会自动尝试 `docker-compose`。

**飞牛提示：** 普通用户 `admin` 常无 `/var/run/docker.sock` 权限；手写命令请加 `sudo`，或把用户加入 `docker` 组后重新登录。管道 `curl | bash` 遇慢网会长时间无输出，可先下载再执行。

### 2.0 脚本一键启动（推荐）

```bash
# 推荐：先下载再执行（可见进度，避免管道假死感）
curl -fL --connect-timeout 15 --max-time 60 -o /tmp/docker-up.sh \
  https://github.com/229033891/EasyTier/raw/main/script/docker-up.sh
bash /tmp/docker-up.sh node --dir /vol1/1000/docker/easytier --pull

# 只启动控制台
bash /tmp/docker-up.sh console --dir /vol1/1000/docker/easytier --pull

# 同时启动控制台 + 节点
bash /tmp/docker-up.sh all --dir /vol1/1000/docker/easytier --pull
```

本地已有仓库时：

```bash
bash script/docker-up.sh node --dir /vol1/1000/docker/easytier --pull
```

### 2.1 只启动节点（手写 compose 命令）

```bash
cd /vol1/1000/docker/easytier
sudo docker compose pull
sudo docker compose up -d
```

等价于只拉起 `et-node`；`console` 带 `profiles: ["console"]`，默认不会启动。

### 2.2 只启动控制台

```bash
cd /vol1/1000/docker/easytier
sudo docker compose --profile console pull console
sudo docker compose --profile console up -d console
```

若当前已在跑节点，可先停节点再起控制台，或见 §2.3 同时运行。

### 2.3 同时启动控制台 + 节点（同一台机器）

```bash
cd /vol1/1000/docker/easytier
sudo docker compose --profile console pull
sudo docker compose --profile console up -d
```

会起两个容器：`et-console`、`et-node`。

**本机自建控制台时**，把节点里的 `--config-server` 改为指向本机，例如：

```text
udp://127.0.0.1:22020/admin
# 或 tcp://127.0.0.1:22020/admin（控制台默认同时监听 UDP+TCP）
```

（`host` 网络下本机回环即可；跨容器不要用容器名，节点不在 bridge 网络里。）

### 2.4 查看状态 / 日志

```bash
cd /vol1/1000/docker/easytier
sudo docker compose ps
sudo docker compose logs -f node
sudo docker compose --profile console logs -f console
```

控制台进程内 warn+ 滚动文件默认写到数据卷 `et-console-data/logs/`（compose 传入 `--file-log-dir=/app/data/logs`）。也可在控制台「运行日志」页切换「文件」来源查看；关闭文件日志用 `--file-log-level off`。

### 2.5 停止

```bash
# 只停节点
cd /vol1/1000/docker/easytier && sudo docker compose stop node

# 只停控制台
cd /vol1/1000/docker/easytier && sudo docker compose --profile console stop console

# 停掉全部（含 console profile）
cd /vol1/1000/docker/easytier && sudo docker compose --profile console down
```

### 2.6 升级（保挂载卷）

改 `docker-compose.yml` 里的镜像 tag 后：

```bash
cd /vol1/1000/docker/easytier
sudo docker compose --profile console pull
sudo docker compose --profile console up -d
```

仅节点时去掉 `--profile console` 即可。`config/` 与 `et-console-data/` 不会被覆盖。

---

## 3. 飞牛 NAS UI 简要说明

1. 文件管理：把 `docker-compose.yml` 放到 `/vol1/<uid>/docker/easytier/`（`config` 可由 `docker-up.sh` 自动创建）  
2. Docker → **项目 / Compose** → 从该路径创建项目  
3. **只跑节点**：直接「启动」→ 等价 `docker compose up -d`  
4. **还要控制台**：若 UI 支持 profile，勾选 `console`；否则 SSH 执行 §2.3  
5. 节点连**远程**控制台时，无需在 NAS 上起 `console`

---

## 4. 常见问题（飞牛实装）

| 现象 | 原因 | 处理 |
|------|------|------|
| `permission denied ... docker.sock` | 用户不在 `docker` 组 | 命令加 `sudo`，或 `sudo usermod -aG docker $USER` 后重新登录 |
| `not in the allowlist` / DaoCloud 拒绝 | `m.daocloud.io` 未收录本镜像 | 使用 `ghcr.io/229033891/et:<tag>`（compose 默认） |
| `curl \| bash` 长时间无输出 | `-s` 静默 + 网络慢 | 去掉 `-s`，或先下到 `/tmp` 再 `bash` |
| 误建 `udp`、`"22020` 等目录 | 旧版 `docker-up.sh` 把 ports 当成 volumes | 删掉误建目录；用已修复脚本（仅解析 `volumes:`） |

---

## 5. 与安装脚本的区别

| 方式 | 适用 |
|------|------|
| `script/install.sh` | Linux 裸机 / VPS：systemd + Release zip |
| `docker-compose.yml` + `script/docker-up.sh` | NAS、已有 Docker 的环境；节点需 TUN / 特权 |

二者不要在同一台机器上对同一角色重复安装（例如既装 systemd 节点又跑 `et-node` 容器）。
