# Docker Compose 部署（节点 / 控制台）

## Status

- Status: **Ops**
- 最近审阅：2026-11-14（新增 §2.7 卸载与残留路由清理；飞牛现网路径：`/vol1/1000/docker/et`，容器 `fnnas_et`）
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
# 飞牛现网常用目录见 §3；新建也可沿用同路径
export DEPLOY_DIR=/vol1/1000/docker/et
mkdir -p "$DEPLOY_DIR"
cd "$DEPLOY_DIR"
curl -fsSL -o docker-compose.yml \
  https://github.com/229033891/EasyTier/raw/main/docker-compose.yml
```

**节点**启动前请确认：

1. `docker-compose.yml` 里 `--config-server` / `ET_CONFIG_SERVER` 指向你的控制台（任选其一：`udp://et.example.com:22020/admin` 或 `tcp://et.example.com:22020/admin`；控制台默认 `--config-server-protocol=udp,tcp`）
2. 节点 config 挂载路径与 compose 一致（飞牛现网多为 `/vol1/1000/docker/et/config`，按 NAS UID / 项目名修改）；`script/docker-up.sh` 会在启动前 `mkdir -p`
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
# GitHub raw 慢/HTTP2 中断时，改用：
#   https://ghfast.top/https://raw.githubusercontent.com/229033891/EasyTier/main/script/docker-up.sh
curl -fL --http1.1 --connect-timeout 15 --max-time 120 -o /tmp/docker-up.sh \
  https://github.com/229033891/EasyTier/raw/main/script/docker-up.sh
bash /tmp/docker-up.sh node --dir /vol1/1000/docker/et --pull

# 只启动控制台
bash /tmp/docker-up.sh console --dir /vol1/1000/docker/et --pull

# 同时启动控制台 + 节点
bash /tmp/docker-up.sh all --dir /vol1/1000/docker/et --pull
```

本地已有仓库时：

```bash
bash script/docker-up.sh node --dir /vol1/1000/docker/et --pull
```

### 2.1 只启动节点（手写 compose 命令）

```bash
cd /vol1/1000/docker/et
sudo docker compose pull
sudo docker compose up -d
```

等价于只拉起节点服务；飞牛 UI 建的项目容器名常为 **`fnnas_et`**（不必是示例里的 `et-node`）。`console` 带 `profiles: ["console"]`，默认不会启动。

### 2.2 只启动控制台

```bash
cd /vol1/1000/docker/et
sudo docker compose --profile console pull console
sudo docker compose --profile console up -d console
```

若当前已在跑节点，可先停节点再起控制台，或见 §2.3 同时运行。

### 2.3 同时启动控制台 + 节点（同一台机器）

```bash
cd /vol1/1000/docker/et
sudo docker compose --profile console pull
sudo docker compose --profile console up -d
```

会起节点 + 控制台两个服务（容器名以 compose / 飞牛 UI 为准）。

**本机自建控制台时**，把节点里的 `--config-server` 改为指向本机，例如：

```text
udp://127.0.0.1:22020/admin
# 或 tcp://127.0.0.1:22020/admin（控制台默认同时监听 UDP+TCP）
```

（`host` 网络下本机回环即可；跨容器不要用容器名，节点不在 bridge 网络里。）

### 2.4 查看状态 / 日志

```bash
cd /vol1/1000/docker/et
sudo docker compose ps
sudo docker compose logs -f node
sudo docker compose --profile console logs -f console
# 飞牛现网容器名：
sudo docker exec fnnas_et ET-core -V
```

控制台进程内 warn+ 滚动文件默认写到数据卷 `et-console-data/logs/`（compose 传入 `--file-log-dir=/app/data/logs`）。也可在控制台「运行日志」页切换「文件」来源查看；关闭文件日志用 `--file-log-level off`。

### 2.5 停止

```bash
# 只停节点
cd /vol1/1000/docker/et && sudo docker compose stop node

# 只停控制台
cd /vol1/1000/docker/et && sudo docker compose --profile console stop console

# 停掉全部（含 console profile）
cd /vol1/1000/docker/et && sudo docker compose --profile console down
```

### 2.6 升级（保挂载卷）

改 `docker-compose.yml` 里的镜像 tag 后（飞牛现网示例）：

```bash
cd /vol1/1000/docker/et
# 现网曾用南大 GHCR 镜像：ghcr.nju.edu.cn/229033891/et:vX.Y.Z
grep -n image docker-compose.yml
sudo sed -i 's|229033891/et:[^ "]*|229033891/et:v2.7.12|g' docker-compose.yml
sudo docker compose pull
sudo docker compose up -d
sudo docker exec fnnas_et ET-core -V
```

若还跑了 `console` profile，加上 `--profile console`。`config/` 与控制台数据卷不会被覆盖。南大镜像尚未同步新 tag 时，可临时改 `ghcr.io/229033891/et:v2.7.12` 再 pull。

### 2.7 卸载（含残留路由清理）

> **Docker 卸载 ≠ 卸干净。** `host` 网络 + `privileged` 的节点会往**宿主机**主路由表写 `/32` 排除路由，实例停止时清理路径可能漏删（缺陷与现场见 [`../roadmap/underlay-exclude-onlink-filter.md`](../roadmap/underlay-exclude-onlink-filter.md) §2.2）。卸完**必须**跑第 3 步。

**1) 备份配置（含节点身份，删了就换 peer id）**

```bash
cd /vol1/1000/docker/et
sudo tar czf /vol1/1000/docker/et-config-backup-$(date +%F).tgz -C /vol1/1000/docker/et config
sudo ls -lh /vol1/1000/docker/et-config-backup-*.tgz        # 必须非空；0 字节说明没打进去
sudo tar tzf /vol1/1000/docker/et-config-backup-*.tgz | head  # 应列出 config/ 与 *.toml
```

**2) 停容器 + 删项目（不动镜像以外的宿主机资源）**

```bash
cd /vol1/1000/docker/et && sudo docker compose --profile console down
sudo docker ps -a                                   # 应无 fnnas_et / et-node
sudo docker image ls | grep '229033891/et'           # 要彻底清：sudo docker image rm <ID>
```

`down` 回 `WARN[...] No resource found to remove for project "et"` **属正常** —— 说明容器早已不存在，不用处理。

**3) 清残留路由（关键，`down` 之后马上做）**

```bash
# ① 先看有没有：形如 x.x.x.x/32 via <物理网关> dev <物理网卡> proto static metric 1
#    （只查 host route：/0 默认路由同样是「via 网关 + metric 1」，不加 /32|/128 过滤会误报）
ip route show | grep -E '/(32|128) via '

# ② 留档，便于回滚
ip route show | grep -E '/(32|128) via ' | sudo tee /vol1/1000/docker/et-routes-before-cleanup.txt

# ③ 删除（示例：网关 192.168.10.1 / 网卡 ens256，按 ① 的实测结果替换）
for h in 192.168.10.96 192.168.10.101 192.168.10.201 192.168.10.254 ; do
  sudo ip route del $h/32 via 192.168.10.1 dev ens256 metric 1
done
# 公网条目同样是 ET 残留（含 config-server），一并清：
for h in 43.154.13.211 101.43.96.62 124.222.29.94 172.25.0.1 ; do
  sudo ip route del $h/32 via 192.168.10.1 dev ens256 metric 1
done

# ④ 校验：同网段应回到直连（不再 via 网关）
ip route get 192.168.10.101
ip route show | grep -E '/(32|128) via '    # ET 残留应为 0；/0 默认路由等非 host route 不算
```

**判定为 ET 残留的依据（三条同时成立才动手）：**

| 判据 | 说明 |
|---|---|
| 形如 `/32 via <物理默认网关> dev <物理网卡> proto static metric 1` | 与 `add_ipv4_host_route(ip, via, Some(1))` 的 netlink 参数逐字段一致（`easytier/src/common/ifcfg/netlink.rs:578-596`） |
| `/etc`、`/etc/network`、`/etc/netplan`、`/etc/systemd/network` 里 `grep` 不到这些 IP | 无持久化配置来源 ⇒ 纯运行时残留 |
| 集合里含 ET 控制台 / peer 的公网 IP | ET 特有特征，NAS 网络配置不会给公网 IP 单独钉 `/32` |

**⚠️ 不要动的：** `ip rule` 的 `from <IP> lookup <10/11>` 与 `table 10/11` 里的 `192.168.10.0/24 dev ens192|ens256 proto static scope link` —— ET 只写 `RT_TABLE_MAIN`，**从不建 `ip rule` / 自定义表**，那些是飞牛自身的（同网段）双网卡策略路由。

**4) 重启自证（可选但推荐）**

残留无持久化来源 ⇒ 重启后应全部消失。重启再查 3-① 若仍有 `/32 via <网关>`，说明来源不是 ET（那时别删，回查 NAS 网络配置）。

**5) 删部署目录 + 控制台收尾**

```bash
sudo rm -rf /vol1/1000/docker/et            # 备份 tgz 已在 /vol1/1000/docker/ 保留
```

- 到控制台设备列表删掉这台机的**孤儿设备记录**（旧 peer id 随容器消失，否则长期离线挂着）；
- 备份里的 `*.toml` 含网络名 / 密钥类内容，别放共享目录；确认不再重装后可按留存期删除。

---

## 3. 飞牛 NAS（FNNAS 现网实装）

### 3.1 现网路径（2026-10 核对）

| 项 | 值 |
|----|-----|
| 主机 | `FNNAS` |
| Compose 目录 | **`/vol1/1000/docker/et`** |
| Compose 文件 | `/vol1/1000/docker/et/docker-compose.yml` |
| 容器名 | **`fnnas_et`**（飞牛 UI 生成，不是仓库示例的 `et-node`） |
| 镜像（示例） | `ghcr.nju.edu.cn/229033891/et:v2.7.12`（南大镜像；官方为 `ghcr.io/229033891/et:…`） |
| 角色 | 仅节点（连远程控制台）；未在本机启 `console` |

**不要**再找 `/vol1/1000/docker/easytier`：该路径可能从未创建，或已在回收站（`.@#local/trash/...`）。

SSH 快速定位（换机时）：

```bash
sudo docker ps -a --format 'table {{.Names}}\t{{.Image}}\t{{.Status}}'
sudo find /vol1 -maxdepth 5 -path '*/docker/*/docker-compose.yml' 2>/dev/null
```

### 3.2 UI 说明

1. 文件管理：compose 放在 `/vol1/<uid>/docker/<项目名>/`；**本机现网项目名是 `et`**（不是 `easytier`）  
2. Docker → **项目 / Compose** → 从该路径管理；容器名以 UI / `docker ps` 为准（现网 **`fnnas_et`**）  
3. **只跑节点**：直接「启动」→ 等价 `docker compose up -d`  
4. **还要控制台**：若 UI 支持 profile，勾选 `console`；否则 SSH 执行 §2.3  
5. 节点连**远程**控制台时，无需在 NAS 上起 `console`  
6. 核对版本：`sudo docker exec fnnas_et ET-core -V`（或 `ET-core --version`）
7. **卸载**（含必须做的残留路由清理）：见 §2.7 —— 只 `docker compose down` 会留下 `/32 via <网关> metric 1`

---

## 4. 常见问题（飞牛实装）

| 现象 | 原因 | 处理 |
|------|------|------|
| `cd .../easytier: No such file` | 现网目录是 **`/vol1/1000/docker/et`** | 用 §3.1 路径；或 `find` / `docker ps` 定位 |
| `No such container: et-node` | 飞牛容器名是 **`fnnas_et`** | `sudo docker ps -a` 看实际名 |
| `permission denied ... docker.sock` | 用户不在 `docker` 组 | 命令加 `sudo`，或 `sudo usermod -aG docker $USER` 后重新登录 |
| `not in the allowlist` / DaoCloud 拒绝 | `m.daocloud.io` 未收录本镜像 | 用 `ghcr.io/...` 或现网南大前缀 `ghcr.nju.edu.cn/...` |
| `curl: (18) HTTP/2 stream ...` / 脚本未下完 | GitHub raw 不稳定 | `--http1.1`，或 `ghfast.top` 镜像；或跳过脚本直接 §2.6 |
| `curl \| bash` 长时间无输出 | `-s` 静默 + 网络慢 | 去掉 `-s`，或先下到 `/tmp` 再 `bash` |
| 误建 `udp`、`"22020` 等目录 | 旧版 `docker-up.sh` 把 ports 当成 volumes | 删掉误建目录；用已修复脚本（仅解析 `volumes:`） |
| 卸载时 `down` 报 `No resource found to remove for project "et"` | 容器/网络已不存在 | 正常，无需处理；继续走 §2.7 第 3 步清残留路由 |
| **卸载后 `ip route` 仍有 `/32 via <网关> proto static metric 1`** | ET 排除路由清理路径漏删（`host` 网络写到宿主机主表） | 按 §2.7 第 3 步判定并删除；不动 `ip rule` / `table 10\|11` |
| 卸载后重装，控制台多出一台离线设备 | `config/` 被删 ⇒ 换了节点身份 | 旧记录在控制台手动删除；要保身份就别删 `config/` |

---

## 5. 与安装脚本的区别

| 方式 | 适用 |
|------|------|
| `script/install.sh` | Linux 裸机 / VPS：systemd + Release zip |
| `docker-compose.yml` + `script/docker-up.sh` | NAS、已有 Docker 的环境；节点需 TUN / 特权 |

二者不要在同一台机器上对同一角色重复安装（例如既装 systemd 节点又跑 `et-node` 容器）。
