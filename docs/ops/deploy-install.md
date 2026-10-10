# EasyTier 部署脚本

## Status

- Status: **Ops**
- 最近审阅：2026-10-09（修复 update 在 set -e 下因 `[[ ]] &&` 静默退出）
- 索引：[`../README.md`](../README.md)

一键安装 / 更新 / 备份 / 恢复自托管控制台与节点。**默认全程交互式**，无需记忆命令行参数。

脚本位置：

- `script/install.sh` — 安装入口（独立，不调用 `update.sh`）
- `script/update.sh` — 升级入口（独立，不调用 `install.sh`）
- `script/et-ops-common.sh` — 二者共用的函数库（被 source，不是互相调用）

配套静态检查：`script/check-install.sh`。

总方案（GitHub 产物 / Docker / 升级清单）：[`../roadmap/github-release-install.md`](../roadmap/github-release-install.md)。

**Docker Compose**（NAS / 容器；[`docker-up.sh`](../../script/docker-up.sh) 自动建目录 + 启动）：[`docker-compose-deploy.md`](./docker-compose-deploy.md)。

## 远程一键安装（VPS）

```bash
# 推荐：clone 后本地执行（最稳，且不依赖远端 main 是否已含脚本）
git clone https://github.com/229033891/EasyTier.git
cd EasyTier
sudo bash script/install.sh

# 或管道执行（从本仓库最新 Release 安装；无 TTY 时默认 core 轻量模式）
curl -fsSL https://github.com/229033891/EasyTier/raw/main/script/install.sh | sudo bash -s install
```

## 主菜单

```bash
sudo bash script/install.sh
```

可选：安装、更新、备份、恢复、健康检查、状态、卸载。安装过程中会交互询问：

- 部署模式（server 控制台 / client 节点 / core 轻量）
- **控制台域名**（server/client 必填，无默认值）
- 端口、备份、防火墙
- **NAT / 出口转发**（可选，默认关闭）
- **下载源**（自动 / 直连 GitHub / 国内镜像）

脚本固定从 [229033891/EasyTier Releases](https://github.com/229033891/EasyTier/releases)
拉取最新包（包名 `ET-linux-<arch>-<tag>.zip`）。

## NAT / 出口节点转发（可选）

本机作为其他节点上网出口时开启。默认**关闭**；交互安装会询问，或：

```bash
sudo bash script/install.sh install --mode core --auto --enable-nat
# 指定 WAN 网卡（默认自动取默认路由出口）
sudo bash script/install.sh install --mode core --auto --enable-nat --nat-wan eth0
```

开启后脚本会：

1. 持久化 `net.ipv4.ip_forward=1`（`/etc/sysctl.d/99-easytier-forward.conf`）
2. 安装幂等脚本 `ET-nat.sh` + `ET-nat.service`（MASQUERADE / FORWARD，comment=`ET-NAT`）
3. 为 ET-core 打开 `--enable-exit-node` 与 `--proxy-forward-by-system`（core 模式同时写入配置）
4. 若 UFW/firewalld 已启用，尽量放宽转发 / masquerade（不替代云安全组）

卸载时会尝试移除 NAT 规则与 `ET-nat.service`。健康检查会在已开启时校验 `ip_forward` 与 MASQUERADE。

## 默认端口（Server）

| 端口 | 协议 | 用途 |
|------|------|------|
| 22020 | TCP | Web 控制台 / API（本机，或经 Nginx 反代）；默认也监听配置下发 TCP |
| 22020 | UDP | 配置下发（Client 直连；默认与 TCP 同时开启） |
| 443 | TCP | Web 控制台（Nginx HTTPS 反代时对外） |
| 11010 | UDP/TCP | 节点 P2P 组网 |
| 11011 | UDP/TCP | WireGuard / WebSocket（本机节点或 core 默认 listeners） |
| 11012 | TCP | WebSocket Secure（本机节点或 core 默认 listeners） |

防火墙放行会按 `CONFIG_PROTOCOL`（支持 `udp,tcp` 等多协议）分别开放配置下发端口；本机节点 / core 时额外放行 11011、11012。

服务端默认 `--config-server-protocol udp,tcp`（双协议监听）。Client 连接时**任选其一**写入 URL，例如 `udp://host:22020/<token>` 或 `tcp://host:22020/<token>`（按网络环境选择，客户端不会自动切换）。

安装时可选择 **Nginx HTTPS 443 反代**（脚本仅配置 EasyTier `--api-host` 与防火墙，
**不自动生成 Nginx 配置**）。须手动部署 Nginx，例如：

```nginx
server {
    listen 443 ssl http2;
    server_name et.example.com;   # 改成你的域名

    # ssl_certificate     /path/to/fullchain.pem;
    # ssl_certificate_key /path/to/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:22020;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

宝塔可将上述内容放到 `/www/server/panel/vhost/nginx/<域名>.conf`。
配置下发端口 **UDP 22020** 仍须直连，不能只靠 HTTPS 反代。

```bash
# 自动化示例（Nginx 仍须手动部署）
sudo bash script/install.sh install --mode server --auto \
  --public-host et.example.com \
  --nginx-https-proxy yes
```

Client 节点使用 **接入 Token**（默认 `admin`，可在 Web「接入 Token」页管理），不是登录用户名：

```bash
sudo bash script/install.sh install --mode client --auto \
  --server-host et.example.com \
  --config-token admin
```

也可直接粘贴完整 URL：

```bash
sudo bash script/install.sh install --mode client --auto \
  --server-host 'udp://et.example.com:22020/admin'
```

## 常用操作

```bash
# 升级 —— 远程一键见下文「远程一键升级」；本地仓库：
sudo bash script/update.sh
# 或
sudo bash script/install.sh update

sudo bash script/install.sh backup
sudo bash script/install.sh restore
sudo bash script/install.sh healthcheck
sudo bash script/install.sh status
sudo bash script/install.sh uninstall
```

`update` / `restore` / `uninstall` 在终端下会二次确认；`restore` 有多份备份时可从列表选择。

## 升级（update.sh）

`script/update.sh` 与 `install.sh` **互不调用**；二者各自 `source et-ops-common.sh`，
固定从 [Releases](https://github.com/229033891/EasyTier/releases) 拉最新包。
（`install.sh update` 仅调用库内函数，不会 exec `update.sh`。）

### 远程一键升级（推荐，无需先 clone）

从 GitHub `main` 拉取最新 `update.sh`（缺失时会再拉 `et-ops-common.sh`），再升级本机 `/opt/easytier` 二进制：

```bash
# 非交互（已装机器常用）
curl -fsSL https://raw.githubusercontent.com/229033891/EasyTier/main/script/update.sh \
  -o /tmp/et-update.sh && sudo bash /tmp/et-update.sh --auto

# 交互确认 / 选下载源
curl -fsSL https://raw.githubusercontent.com/229033891/EasyTier/main/script/update.sh \
  -o /tmp/et-update.sh && sudo bash /tmp/et-update.sh
```

国内拉 raw 较慢时，可用镜像拉取脚本（二进制下载仍可在脚本内选「自动」）：

```bash
curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/229033891/EasyTier/main/script/update.sh \
  -o /tmp/et-update.sh && \
  sudo ET_COMMON_URL="https://ghfast.top/https://raw.githubusercontent.com/229033891/EasyTier/main/script/et-ops-common.sh" \
    bash /tmp/et-update.sh --auto
```

说明：

- 写入 `/tmp` 再执行，避免在 `~/EasyTier` 目录下误用**旧版**本地 `et-ops-common.sh`。
- 管道 `curl ... | sudo bash` 也可以；脚本在无真实路径时同样会从 GitHub 拉 common。
- 强制始终用远端 common：`ET_FORCE_REMOTE_COMMON=1`。

### 本地仓库升级

```bash
cd ~/EasyTier && git pull && sudo bash script/update.sh --auto
```

会做：

1. 检测已安装角色（server / client / core）
2. 备份 `et.db`（若存在）
3. 停止服务 → 下载 `ET-linux-<arch>-<tag>.zip` → 替换二进制
4. 按原角色重写 systemd（与 install 一致，含 NAT 的 `--enable-exit-node` 等）
5. 若曾开启 NAT：刷新 `ET-nat.service`（从 `install-options.env` / 运行态恢复）
6. 健康检查

可选参数（与 install 对齐）：

```bash
sudo bash script/update.sh --auto
sudo bash script/update.sh --enable-nat          # 强制保留/刷新 NAT
sudo bash script/update.sh --configure-firewall  # 刷新本机防火墙放行
sudo bash script/update.sh --public-host et.example.com
```

## 下载源（国内）

安装或更新时会询问下载源，一般选 **1) 自动** 即可：

1. **自动（推荐）** — 先直连 GitHub，失败再试 ghfast / ghproxy
2. **仅直连 GitHub** — 适合海外 VPS
3. **ghfast.top 镜像**
4. **ghproxy 镜像**
5. **自定义镜像前缀**

也可通过环境变量预设（跳过下载源询问）：

```bash
export GH_PROXY="https://ghfast.top/"
sudo -E bash script/install.sh install
```

## 自动化（CI / 脚本）

须显式指定域名，无默认值：

```bash
sudo bash script/install.sh install --mode server --auto --public-host console.example.com
sudo bash script/install.sh install --mode client --auto --server-host console.example.com
sudo bash script/install.sh restore --file /path/to/backup.tar.gz --auto
```

## 静态检查

```bash
bash script/check-install.sh
```
