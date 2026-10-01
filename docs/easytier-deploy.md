# EasyTier 部署脚本

一键安装 / 更新 / 备份 / 恢复自托管控制台与节点。**默认全程交互式**，无需记忆命令行参数。

脚本位置：`script/easytier-install.sh`（配套静态检查：`script/check-easytier-install.sh`）。

## 远程一键安装（VPS）

```bash
# 推荐：clone 后本地执行（最稳，且不依赖远端 main 是否已含脚本）
git clone https://github.com/229033891/EasyTier.git
cd EasyTier
sudo bash script/easytier-install.sh

# 或管道执行（需仓库 main 已包含该脚本；LF 换行）
curl -fsSL https://cdn.jsdelivr.net/gh/229033891/EasyTier@main/script/easytier-install.sh | sudo bash
```

## 主菜单

```bash
sudo bash script/easytier-install.sh
```

可选：安装、更新、备份、恢复、健康检查、状态、卸载。安装过程中会交互询问：

- 部署模式（server 控制台 / client 节点）
- **控制台域名**（必填，无默认值）
- 端口、备份、防火墙
- **下载源**（自动 / 直连 GitHub / 国内镜像）

脚本默认从本仓库 Release 拉包（`GITHUB_REPO` 默认为 `229033891/EasyTier`，
包名格式 `ET-linux-<arch>-<tag>.zip`）；如需上游包可
`export GITHUB_REPO=EasyTier/EasyTier`（注意上游包名不同，仅 x86_64 且需自行确认）。

## 默认端口（Server）

| 端口 | 协议 | 用途 |
|------|------|------|
| 22020 | TCP | Web 控制台 / API（本机，或经 Nginx 反代）；默认也监听配置下发 TCP |
| 22020 | UDP | 配置下发（Client 直连；默认与 TCP 同时开启） |
| 443 | TCP | Web 控制台（Nginx HTTPS 反代时对外） |
| 11010 | UDP/TCP | 节点 P2P 组网 |

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
sudo bash script/easytier-install.sh install --mode server --auto \
  --public-host et.example.com \
  --nginx-https-proxy yes
```

Client 节点使用 **接入 Token**（默认 `admin`，可在 Web「接入 Token」页管理），不是登录用户名：

```bash
sudo bash script/easytier-install.sh install --mode client --auto \
  --server-host et.example.com \
  --config-token admin
```

也可直接粘贴完整 URL：

```bash
sudo bash script/easytier-install.sh install --mode client --auto \
  --server-host 'udp://et.example.com:22020/admin'
```

## 常用操作

```bash
sudo bash script/easytier-install.sh update
sudo bash script/easytier-install.sh backup
sudo bash script/easytier-install.sh restore
sudo bash script/easytier-install.sh healthcheck
sudo bash script/easytier-install.sh status
sudo bash script/easytier-install.sh uninstall
```

`update` / `restore` / `uninstall` 在终端下会二次确认；`restore` 有多份备份时可从列表选择。

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
sudo -E bash script/easytier-install.sh install
```

## 自动化（CI / 脚本）

须显式指定域名，无默认值：

```bash
sudo bash script/easytier-install.sh install --mode server --auto --public-host console.example.com
sudo bash script/easytier-install.sh install --mode client --auto --server-host console.example.com
sudo bash script/easytier-install.sh restore --file /path/to/backup.tar.gz --auto
```

## 静态检查

```bash
bash script/check-easytier-install.sh
```
