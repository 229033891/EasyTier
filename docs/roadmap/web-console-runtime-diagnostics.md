# Web 控制台：系统诊断 + 运行日志

## Status

- Status: **Roadmap**（阶段 1 MVP 已实现，待本机/CI 编译与联调验收）
- 最近审阅：2026-10-10
- 归属：[`web-evolution.md`](./web-evolution.md) 阶段 B「可运维性」
- 范围：仅 `easytier-web`（REST + 前端）；**不改** `easytier` / `easytier-core` 节点协议
- 索引：[`../README.md`](../README.md)

---

## 1. 背景与目标

运维排查控制台问题时（例如预定 `udp,tcp` 双听，实际进程只开了 UDP），目前只能 SSH 看 `journalctl` / unit / `ss`，浏览器侧缺少自助诊断入口。

目标：在 Web 控制台侧栏增加 **两个独立菜单**（仅管理员）：

| 菜单 | 用途 |
|------|------|
| **系统诊断** | 对照**预定设置**，检查控制台是否按预期在跑；给出通过/异常项与摘要，并可展开运行参数证据 |
| **运行日志** | 在浏览器内查看控制台进程近期日志（有界缓冲，可刷新/自动滚动） |

二者分开；权限与「用户管理 / 接入密钥」同级（管理员）。

「预定设置」来源优先级（实现时写死文档化）：

1. **进程启动参数**（`Cli` / 环境变量解析结果）——权威「本进程意图」
2. 可选二期：与安装侧 `install-options.env` 对比（需可读路径约定；一期可不做）

---

## 2. 现状缺口

| 能力 | 现状 |
|------|------|
| 侧栏扩展 | `frontend/src/components/MainPage.vue` 的 `navItems` + `frontend/src/main.ts` 路由；已有 `isAdmin` 门控 |
| 控制台自身 CLI/运行参数 | `Cli`（`easytier-web/src/main.rs`）启动时解析，**无**对外 REST |
| 实际监听是否生效 | 启动时 `add_config_server_listeners`（`easytier-web/src/main.rs`）；**无**「意图 vs 实际」对照 API |
| 控制台进程日志 | stdout / 可选 `--file-log-dir` / systemd journal；**无**内存缓冲、**无** `/api/.../logs` |
| 设备侧日志级别 | `proxy-rpc` → 节点 `LoggerRpcService`（与本需求无关） |
| 设备网络配置 | 已有 `GET .../networks/config/{inst}` 等（**不是**本页「系统诊断」） |

---

## 3. 产品范围

### 3.1 系统诊断（菜单名：`系统诊断`）

**定位**：不是单纯「配置 dump」，而是回答：

> 控制台是否已按预定设置正常运行？

页面结构建议：

1. **总览**：`healthy` / `degraded` / `unhealthy` + 一句话摘要（如「配置下发：预定 udp,tcp，实际仅 udp」）。
2. **检查项列表**：每项含 `id`、状态（当前实现产出 `pass` / `fail`；`warn` / `skip` 预留）、期望、实际、说明（稳定 `detail.code`）。标题与说明由前端按 `id` / `code` 本地化。
3. **运行参数（证据）**：可折叠 JSON/文本；「一键复制」便于贴工单。
4. **刷新**按钮；进入页自动跑一次诊断。

#### 一期检查项（建议）

| id | 检查内容 | 期望来源 | 实际来源 | 失败示例 |
|----|----------|----------|----------|----------|
| `api_up` | API 可服务（本接口能返回即通过） | 进程存活 | 自检 | （几乎总是 pass） |
| `config_server_protocol` | 配置下发协议 | `Cli.config_server_protocol` | 已成功注册的 listener scheme 集合 | 预定 `udp,tcp`，实际只有 `udp` |
| `config_server_port` | 配置下发端口 | `Cli.config_server_port` | 同上绑定端口 | 端口不一致（少见） |
| `db_ok` | SQLite 可打开/轻量查询 | `Cli.db` | 对 db 做 `SELECT 1` 或现有健康探针 | 文件损坏/权限 |
| `heartbeat_sane` | 心跳参数合法 | CLI 解析结果 | 按同一规则重算（如 `min < timeout`） | min ≥ timeout 等 |
| `webhook` | 仅报告是否已配置 | CLI 布尔 | 同左 | 不因未配置而 fail |
| `oidc` | 仅报告是否已配置 | CLI 布尔 | 同左 | 不因未配置而 fail |

说明：

- `config_server_protocol` 是核心价值（对应现网「只开了 UDP」类故障）。
- 实际 scheme 集合应从 `ClientManager` / listener 注册结果导出，**不要**只回显 CLI 字符串（否则「意图=实际」永远假绿）。
- 不做宿主机防火墙 / UFW / 公网连通性探测（一期；避免误报与权限问题）。
- 不做「客户端能否连上」主动拨测（可选二期）。

#### 交互文案约束

- 页内写明：诊断对象是**本控制台进程**，不是节点 NetworkConfig。
- 总览用中文状态色：通过 / 警告 / 异常。

### 3.2 运行日志（菜单名：`运行日志`）

展示控制台进程近期 tracing/log 行：

- 默认返回最近 N 行（建议默认 `tail=500`，上限不超过 ring buffer 容量 1000；见 §4.2）
- 支持手动刷新；可选简易自动刷新（如 2–5s 轮询，一期可不做 SSE）
- 级别过滤（可选二期）
- 清空缓冲（可选；仅清进程内缓冲）

**不在一期范围：**

- 直接调用 `journalctl` / 读任意宿主机路径
- 跨进程聚合多实例日志
- 完整日志检索/下载超大历史文件

### 3.3 非目标

- 不改节点协议、不依赖节点升级
- 不把设备 NetworkConfig / TOML 生成塞进「系统诊断」页
- 不向普通用户开放
- 不在帮助文案中提前描述未落地行为（产品 i18n 随实现同一 PR 更新）
- 一期不读 `install-options.env`、不探测防火墙、不做外网拨测

---

## 4. API 草案（控制台可见，节点不可见）

均需 **登录 + 管理员**。`admin` 前缀沿用现有 `/api/v1/admin/config-tokens`；
但现有管理员路由**并不统一**（如 `/api/v1/users` 就没带前缀），实现时二选一即可，定了要同步改回本文件。

### 4.1 系统诊断

```http
GET /api/v1/admin/system-diagnostics
```

（若需保留原始快照给脚本，可用同一响应内的 `snapshot` 字段，不必再拆第二个公开菜单。）

响应示例：

```json
{
  "overall": "degraded",
  "summary": { "code": "degraded", "params": ["config_server_protocol"] },
  "generated_at": "2026-10-10T10:00:00Z",
  "checks": [
    {
      "id": "config_server_protocol",
      "status": "fail",
      "expected": "udp,tcp",
      "actual": "udp",
      "detail": { "code": "listener_mismatch" }
    },
    {
      "id": "db_ok",
      "status": "pass",
      "expected": "readable",
      "actual": "ok"
    }
  ],
  "snapshot": {
    "version": "2.x.x",
    "api_listen": "0.0.0.0:11211",
    "config_server_port": 22020,
    "config_server_protocol": "udp,tcp",
    "config_server_listening": ["udp"],
    "heartbeat_min_response_ms": 3500,
    "heartbeat_timeout_ms": 15000,
    "db_path": "et.db",
    "console_log_level": "info",
    "file_log_dir": null,
    "webhook_configured": false,
    "oidc_configured": false
  }
}
```

> **文案本地化**：后端只回**稳定 id / code + 数据**，不返回中英文标题。
> 检查项标题取 `web.system_diagnostics.checks.<id>`，说明取 `web.system_diagnostics.details.<detail.code>`，
> 总览摘要取 `web.system_diagnostics.overall_summary.<summary.code>`。这样与既有
> `convert_rpc_error` → 前端 `formatApiErrorDetail` 的错误码模式一致，英文界面不会出现中文。

> **端口 / 路径对照（勿混）**：`api_server_port` 默认 **11211**（REST API 监听，`api_listen` 取它）；
> `config_server_port` 默认 **22020**（节点连过来下发配置）；`db` 默认 **`et.db`**（相对进程 CWD）。
> 三者字段名相近，`snapshot` 里必须分开呈现。

实现要点：

1. **意图字段有现成来源**：`easytier-web/src/main.rs` 启动时那条 `tracing::info!(… "easytier-web starting")`
   已经打印了 `version` / `api_address` / `api_port` / `config_protocol` / `config_port` / `heartbeat_*` / `webhook_enabled`——
   诊断的 `snapshot` **复用同一批字段**即可，不必另起一套 CLI 解析。
2. 启动时保留脱敏 `snapshot` 意图字段；listener 注册成功后更新 `config_server_listening`（或等价集合）。
3. 诊断接口每次请求时：拼 checks（期望 vs 实际），计算 `overall`。
4. `overall` 规则建议：任一项 `fail` → 至少 `degraded`；关键项（如 db）`fail` → `unhealthy`；仅 warn → `degraded`；全 pass → `healthy`。
   实现里总览只返回 `overall` + `summary.code`/`params`，正文由前端渲染。

### 4.2 运行日志

```http
GET /api/v1/admin/logs?tail=500
```

响应示例：

```json
{
  "capacity": 1000,
  "lines": [
    {
      "ts": "2026-10-10T10:00:01.234Z",
      "level": "INFO",
      "target": "easytier_web",
      "message": "easytier-web starting"
    }
  ]
}
```

实现要点：

1. 进程内 **有界 ring buffer**（固定容量，覆写最旧）。
2. 挂在现有日志初始化路径上（`easytier-web/src/main.rs` 启动时调用的
   `log::init_with_default_console_targets` + `log::enable_memory_buffer`，实现在 `easytier/src/common/log/mod.rs`），**不替代** stdout/文件。
3. 一期轮询即可；二期可加 SSE。

---

## 5. 前端改动清单

| 项 | 位置 |
|----|------|
| 侧栏两项 | `frontend/src/components/MainPage.vue` 的 `navItems`（`isAdmin` 分支）：`systemDiagnostics`、`runtimeLogs` |
| 路由 | `frontend/src/main.ts` |
| 页面 | `frontend/src/components/SystemDiagnostics.vue`、`RuntimeLogs.vue`（与 `DeviceList.vue` 等现有命名一致） |
| API 客户端 | `frontend/src/modules/api.ts` |
| i18n | 菜单名（建议 `web.main.system_diagnostics` / `web.main.runtime_logs`，沿用现有 `web.main.*` 命名）+ 检查项标题/说明 |
| 非管理员 | redirect `dashboard` |

图标建议：系统诊断 `pi pi-heart` / `pi pi-check-circle`；运行日志 `pi pi-list`。

---

## 6. 安全与隐私

1. **仅管理员**；普通用户 403。
2. **禁止**在 snapshot/诊断中返回：密码、session secret、webhook secret、OIDC client_secret、接入密钥、内部 auth token。
3. 日志可能含 URL/token 片段：UI 提示勿外传；二期可脱敏。
4. 无新 DB migration（一期）。

---

## 7. 分期与验收

### 阶段 1（MVP）

- [x] Listener 实际 scheme 可查询 + `GET /api/v1/admin/system-diagnostics`
- [x] Ring buffer + `GET /api/v1/admin/logs?tail=`
- [x] 侧栏「系统诊断」「运行日志」+ 管理员门控 + i18n
- [x] 诊断页：总览、检查项、刷新、复制 snapshot
- [x] 日志页：列表、刷新

验收：

1. 管理员可见两菜单；非管理员不可见且直链被拒。
2. 人为造成「CLI=`udp,tcp` 但实际只注册 udp」时，诊断页 `config_server_protocol` 为 fail，总览非 healthy。
3. 双听正常时该项 pass，总览 healthy（在其它项也 pass 时）。
4. 运行日志页刷新可见近期 info 日志。
5. 无 secret 明文；仅升 web；节点行为不变。

### 阶段 2（可选）

- [ ] 与 `install-options.env` 对比（路径约定 + 权限）
- [ ] 日志级别过滤 / SSE / 脱敏
- [ ] 本机端口 `ss` 级探测（慎用；权限与容器场景）
- [ ] 可选 config-server 自环拨测（udp/tcp 各一次）

---

## 8. 风险与对策

| 风险 | 对策 |
|------|------|
| 只回显 CLI 导致假绿 | 必须以**实际 listener** 为 actual |
| 日志含敏感信息 | admin-only + 警告；二期脱敏 |
| Ring buffer 内存 | 固定容量 |
| 与 journal / 防火墙不一致 | UI 标明范围：进程内意图与监听，不含宿主防火墙 |
| 与旧称混淆 | 菜单与文档统一为「系统诊断」（曾用名：实时配置 / 控制台诊断） |

---

## 9. 变更分类（PR 标签）

- `web/ui`：侧栏、两页面、i18n
- `web/api`：admin system-diagnostics / logs
- `web/ops`：排障入口说明（实现后可补 `ops/` 一句）

不涉及：`web/db`、`web/compat`。

---

## 10. 决策记录

| 日期 | 决策 |
|------|------|
| 2026-10-10 | 侧栏两个菜单：原「实时配置」方案 + 运行日志 |
| 2026-10-10 | 仅管理员；一期 ring buffer，不读 journalctl |
| 2026-10-10 | 「实时配置」改为对照预定设置的健康检查，不只 dump 配置 |
| 2026-10-10 | 菜单定名 **系统诊断**（曾用「控制台诊断」）；API `system-diagnostics` |
| 2026-10-10 | 诊断 actual 必须来自实际 listener/运行态，禁止仅回显 CLI |
| 2026-10-10 | 短期只改 `easytier-web`，保持节点兼容 |

---

## 11. 修订方式

1. 实现前以本文为准；检查项 id 可在 PR 微调，但不得改 core。
2. 落地后更新 Status；产品 i18n 只描述已实现行为。
3. 与 [`web-evolution.md`](./web-evolution.md) 冲突时，先更新演进文档决策表再改本文。
