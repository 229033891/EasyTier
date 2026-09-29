# EasyTier Web 部署与升级指南

## Status

- 状态：Active
- 适用范围：已部署的 `easytier-web` / `easytier-web-embed` 原地升级
- 命名约定：产品正式名统一为 **`easytier-web`**（不使用 controller 等别名）
- 核心目标：**保留 SQLite 业务数据**，升级后账号、网络配置、设备与会话相关数据仍可用
- 配套文档：
  - `docs/easytier-web-evolution-roadmap.md`（Web 演进路线）
  - `docs/easytier-web-build-and-deploy.md`（打包与 Win/Linux 部署）
  - `docs/easytier-product-map.md`（全产品地图）

---

## 1. 数据存在哪里

| 内容 | 默认位置 | 如何指定 |
|------|----------|----------|
| 业务数据库（用户、组、网络配置、managed revision、会话等） | 工作目录下的 `et.db` | `--db` 或环境变量 `ET_WEB_DB` |
| GeoIP（可选，非业务主数据） | 内嵌或外部 mmdb | `--geoip-db` / `ET_GEOIP_DB` |
| 日志文件（若开启） | `--file-log-dir` 指定目录 | `ET_WEB_FILE_LOG_DIR` |

要点：

1. **真正需要保全的是 SQLite 文件**（以及你自定义的绝对路径副本）。
2. 默认 `et.db` 是**相对路径**，相对的是**进程启动时的当前工作目录**。换目录启动等于换库，看起来像「数据丢了」。
3. 生产环境强烈建议使用**绝对路径**，例如：
   - Windows：`--db D:\easytier-web\data\et.db`
   - Linux：`--db /var/lib/easytier-web/et.db`
4. 新版本启动时会自动执行 schema migration（`Migrator::up`），**只要仍指向同一库文件**，数据会在原库上升级，而不是重建空库。

相关代码行为（概念上）：

```text
启动 → 打开 --db 指向的 SQLite
     → 若文件不存在则新建
     → 自动执行未应用的 migration
     → 继续提供 API / 配置服务器
```

---

## 2. 升级前检查

1. 确认新版本兼容现网节点版本（见发布说明 / 演进路线冻结接口）。
2. 记下当前运行参数：
   - `--db` / `ET_WEB_DB`
   - 端口：`--config-server-port`、`--api-server-port` 等
   - `--api-host`、OIDC、`--no-web` 等
   - 实际工作目录（若 db 为相对路径）
3. 确认磁盘上 db 文件路径与大小；有条件可先只读打开验证能查到用户表。
4. 安排短维护窗口（替换二进制期间配置服务器会短暂不可用；节点通常会重连）。

---

## 3. 标准升级步骤（Windows / Linux 通用）

### 3.1 备份（必须）

在停止服务前或刚停止后立刻备份：

```text
# 至少备份这些
et.db                  # 主库（按你的实际路径）
et.db-wal              # 若存在（SQLite WAL）
et.db-shm              # 若存在
# 以及你的启动脚本 / systemd unit / 计划任务 / 环境变量文件
```

建议同时做一份带时间戳的副本，例如：

- Windows：`et.db.bak-20260929`
- Linux：`et.db.bak-20260929`

不要只剪切走原库；保留原路径文件供新进程继续使用。

### 3.2 停止旧进程

- Windows：结束 `easytier-web.exe` / `easytier-web-embed.exe`，或停掉对应服务。
- Linux：`systemctl stop easytier-web`（若已做成服务），或结束对应进程。

确认没有进程仍占用 `et.db`（避免备份不完整或替换失败）。

### 3.3 替换程序，不碰数据库

1. 用新版本二进制替换旧文件：
   - 一体部署：替换 `easytier-web-embed`（Windows 为 `.exe`）
   - 分离部署：替换 `easytier-web`，如有前端静态资源再更新 `frontend/dist`（或 nginx 根目录）
2. **不要删除、移动、清空 `et.db`**
3. **不要改 `--db` 到一个新的空路径**（除非你有意迁移，见第 5 节）
4. 启动参数尽量与升级前一致（尤其是 db 路径与端口）

### 3.4 启动新版本

用原工作目录、原参数启动。例如：

```bash
# Linux 示例（绝对路径）
./easytier-web-embed --db /var/lib/easytier-web/et.db
```

```powershell
# Windows 示例（绝对路径）
.\easytier-web-embed.exe --db D:\easytier-web\data\et.db
```

启动后程序会：

1. 打开已有 `et.db`
2. 自动应用尚未执行的 migration
3. 用升级后的 schema 继续读写原有业务数据

### 3.5 升级后验证

1. 控制台能打开并登录（原账号）。
2. 原有网络配置 / 设备列表仍在。
3. 抽样节点：心跳正常，配置仍可下发或保持受管状态。
4. 查看日志无 migration 失败、无反复「creating a new one」却指向了空库。

若日志出现创建Database not found, creating a new one`，说明**连到了新的空路径**，不是原库；立即停服，检查工作目录与 `--db`，切回正确路径（不要用这个新空库覆盖备份）。

---

## 4. 回滚

当新版本无法启动、migration 失败、或业务异常时：

1. 停止新进程。
2. 恢复旧二进制。
3. 数据库处理：
   - **若新版本尚未成功写库或未做破坏性 migration**：通常可直接用当前 `et.db` 回跑旧版本（仍建议先再备份一次当前文件）。
   - **若新版本已应用仅新版本能懂的 schema**：将 `et.db`（及 wal/shm）恢复为升级前备份，再启动旧二进制。
4. 验证登录与节点连接。

原则：**二进制与 schema 要匹配**；不能长期用「很旧的程序 + 已被新 migration 改过的库」除非发布说明明确支持向下兼容。

---

## 5. 迁移数据目录（可选）

若要从相对路径改为绝对路径，或换机器：

1. 停服。
2. 完整复制 `et.db`（及 `et.db-wal` / `et.db-shm` 若存在）到新位置。
3. 用新 `--db` 绝对路径启动，**不要**同时在新旧路径各跑一个实例。
4. 确认控制台数据完整后，再归档旧文件（先留备份一段时间）。

跨机器时，一并带走启动参数（端口、OIDC、`api-host` 等）。节点侧配置的「配置服务器地址」若 IP/域名变了，需要同步改节点指向。

---

## 6. 开发侧约定（保证「升级不丢数」）

后续改 `easytier-web` 时必须遵守：

1. **schema 变更只能通过 migration**（`easytier-web/src/migrator/`），禁止手改生产库作为发布步骤。
2. migration 必须在「有旧数据的库」上可重复验证：空库升级、旧版本库升级两条路径都要过。
3. 优先**向后兼容、可保留数据**的变更（加列、加表、可空默认值）；避免无备份就不可逆的 drop/毁掉列。
4. 发布说明写清：
   - 是否有 DB migration
   - 是否可回滚到上一 web 版本
   - 回滚是否必须恢复 db 备份
5. 不要把「删除 `et.db` 再启动」写进任何升级文档或脚本。

---

## 7. 快速检查清单

升级前：

- [ ] 已备份 `et.db`（及 wal/shm）
- [ ] 已记录 `--db` 实际路径与启动参数
- [ ] 新版本兼容现网节点

升级中：

- [ ] 已停旧进程
- [ ] 只替换二进制/前端，未删除数据库
- [ ] 仍用同一 `--db`（建议绝对路径）启动

升级后：

- [ ] 能用原账号登录
- [ ] 原配置/设备仍在
- [ ] 节点心跳与配置下发正常
- [ ] 日志无「误建新库」

失败时：

- [ ] 停服 → 换回旧二进制 → 必要时恢复 db 备份 → 验证

---

## 8. 管理员忘记密码

公开自行注册已关闭，用户只能由管理员创建。若唯一的 `admin` 忘记密码，在**停服**后对**同一库文件**执行（明文与登录页相同）：

```text
easytier-web --db <et.db 绝对路径> --reset-password-user admin --new-password <新密码>
```

命令成功后立即退出、不启动服务。随后用新密码登录。若还有其他管理员账号，也可在「用户管理」里为 `admin` 重置密码。

---

## 9. 一句话结论

**升级 = 换程序，不换库。**  
保留并继续使用同一个 SQLite 文件（绝对路径），新版本启动时自动 migration；升级前备份，失败则二进制与库备份一起回滚。
