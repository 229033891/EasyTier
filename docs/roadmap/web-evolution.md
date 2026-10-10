# EasyTier Web 演进路线

## Status

- Status: **Roadmap**
- 最近审阅：2026-10-03
- 状态说明：Active（作为 `easytier-web` 后续开发与升级的指导文档）
- 适用范围：`easytier-web`（配置服务器 + REST API + 浏览器 UI）
- 命名约定：产品正式名统一为 **`easytier-web`**（不使用 controller 等别名）；见 [`../current/product-map.md`](../current/product-map.md) 第 1 节
- 短期约束：**不修改** `easytier` / `easytier-core`，仅演进 `easytier-web`
- 兼容目标：与当前已发布的 `easytier` / `easytier-core` **运行时百分百兼容**
- 部署目标：支持 **Windows** 与 **Linux** 独立部署、独立升级
- 索引：[`../README.md`](../README.md)

相关文档：

- [`../current/product-map.md`](../current/product-map.md)（全产品/组件地图）
- [`../ops/web-build-deploy.md`](../ops/web-build-deploy.md)（打包与 Win/Linux 部署）
- [`../ops/web-upgrade.md`](../ops/web-upgrade.md)（现网 web 部署升级与保库）
- [`../current/web-managed-config.md`](../current/web-managed-config.md)（managed config 增量同步）
- [`../current/architecture.md`](../current/architecture.md)（core 架构边界，本路线短期不改 core）

---

## 1. 产品定位

EasyTier 产品家族内有两个可独立交付的组件：

| 组件 | 职责 | 交付形态 |
|------|------|----------|
| `easytier` / `easytier-core` | VPN 节点：组网、转发、隧道、实例生命周期 | `easytier-core` / `easytier-cli` 等 |
| `easytier-web` | 配置服务器 + Web 控制台：向节点下发配置；向浏览器提供 REST API / UI | `easytier-web`、`easytier-web-embed` |

二者关系：

```text
浏览器 / 静态前端  --(HTTP REST)---->  easytier-web :11211 (默认)
easytier-core 节点 --(udp/tcp/ws)--->  easytier-web :22020 (默认配置服务器)
```

说明：

1. **运行/交付层面**：它们是两个可单独安装、单独升级的程序。
2. **工程层面**：同属一个 monorepo、共享协议与配置类型；web 在编译期依赖 `easytier` / `easytier-core`。
3. 准确表述：**同一产品家族中的两个可独立交付组件**，不是互不相关的两个项目。

本路线只规划 `easytier-web` 的演进；节点侧短期冻结。

---

## 2. 演进原则

1. **兼容优先**：任何 web 变更不得破坏与现有节点的配置协议、心跳、Web RPC、配置格式语义。
2. **只改 web**：短期不改 `easytier` / `easytier-core` / `easytier-proto` 的对外行为；若必须动共享类型，需先修订本路线并评估节点兼容性。
3. **可独立升级**：运维可只替换 web 二进制 / 镜像 / 前端静态资源，无需同步升级全部节点。
4. **双平台交付**：每次可发布版本至少提供 Windows 与 Linux 可运行产物（沿用现有 CI 矩阵）。
5. **同版本心智**：web 与 core 仍共享 `EASYTIER_VERSION` 语义；web 独自发补丁时，文档与发布说明须标明「兼容的节点版本范围」。
6. **冻结接口显式化**：把「节点可见接口」当作稳定 API；控制台内部实现可自由重构。

---

## 3. 冻结接口（短期不可破坏）

以下接口视为与节点的兼容契约。**允许增强（向后兼容）**，**禁止破坏性变更**，除非同步升级节点并修订本路线。

### 3.1 配置服务器（节点侧）

- 默认监听与协议：`config_server_port`（默认 `22020`）、`config_server_protocol`（默认 `udp,tcp` 同端口双听；亦可 `ws` 等，注意 tcp 与 ws 不能共用同一端口）
- 心跳：`HeartbeatRequest` / `HeartbeatResponse` 字段语义与节奏约定
- Web / 管理相关 RPC：`WebClientService` 等现有调用语义
- 隧道与安全相关行为：现有 listener / tunnel / web_security 约定
- 运行态 reconcile / managed config 下发语义（含 Full；以及已落地的 Patch 接收端行为）

### 3.2 配置数据

- 下发给节点的网络配置结构与 TOML/结构化语义（`NetworkConfig` 等）
- user-owned / web-owned 配置 ownership 规则
- managed config revision、Full Exact Set、PATCH CAS 等已文档化语义  
  （详见 `easytier-web-managed-config-sync-plan.md`）

### 3.3 版本与能力广告

- 节点依赖的能力探测、错误码、拒绝原因等已有行为
- 不得在未升级节点的前提下引入「节点必须识别否则失败」的强制新字段

### 3.4 明确不在冻结范围内（可改）

- 浏览器 REST API 的路径组织、鉴权体验、分页与筛选（对前端可做版本化迁移）
- Web UI / `frontend` / `frontend-lib` 交互与视觉
- SQLite schema（通过 migration 演进；注意升级与回滚策略）
- 用户/组/权限、OIDC、验证码、会话存储
- 日志、运维参数、GeoIP、部署开关（`--no-web`、`--api-host`、embed 等）
- 进程内实现重构（只要对外节点协议不变）

---

## 4. 阶段规划

### 阶段 A：独立交付与稳定基线（当前）

目标：确认 web 可作为独立产品部署升级，并固定兼容边界。

工作项：

1. 以本文件为后续开发约束来源。
2. 发布/构建沿用现有 CI：产出 `easytier-web` 与 `easytier-web-embed`。
3. 支持 Windows（msvc）与 Linux（musl 等）部署。
4. 推荐默认交付：`easytier-web-embed`（单文件含前端）；需要拆分时用 `easytier-web` + 静态前端。

验收：

- 仅升级 web，已对接的旧版节点仍能心跳、拉配置、受控运行。
- Win / Linux 均可完成「安装 → 启动 → 登录控制台 → 管理已有节点」闭环。

### 阶段 B：控制台能力演进（短期主战场）

目标：在冻结节点协议的前提下，持续改进配置服务器运维面与 Web 体验。

优先方向（按需排期，不强制一次做完）：

1. **控制台 UX**：网络/设备/用户管理、状态可见性、错误可读性。
2. **REST / 内部 API**：稳定性、权限模型、审计、限流；对 breaking 变更做显式版本或兼容期。
3. **配置发布体验**：在已支持的 Full/Patch 接收能力之上，完善 Console 侧发布、校验、冲突提示（不改节点协议）。
4. **多租户与安全**：账号体系、OIDC、会话、CSRF/CORS、密钥与 webhook 安全加固。
5. **可运维性**：配置项文档化、健康检查、备份/迁移 SQLite、日志与指标。
6. **部署形态**：Windows 服务/安装包说明、Linux systemd/Docker 说明、前后端分离部署指南。

约束：

- 不引入必须改 core 才能工作的新协议。
- 数据库变更必须带 migration；说明是否可逆。

### 阶段 C：发布与升级机制打磨（与 B 并行）

目标：让「只升 web」成为常规运维动作。

工作项：

1. 发布说明模板：兼容的 `easytier` / `easytier-core` 版本范围、迁移步骤、回滚步骤。
2. 升级检查清单：协议未变确认、DB migration、前端/API 兼容、端口与 `--api-host`。
3. 可选：web 自身的健康/版本 API，便于运维确认「当前控制台版本」与「声明兼容的节点版本」。

### 阶段 D：可选的中长期（需重新决策后启动）

仅在明确需要时进入；**不是短期默认路径**。

可能方向：

1. 收窄 web 对 `easytier` 的 feature 依赖，降低二进制体积与无关 VPN 栈耦合。
2. 将稳定协议/配置类型更多收敛到 `easytier-proto` 或独立契约层，便于 web 与 core 同步演进。
3. 若必须变更节点可见协议：改为「web + core 同版本发布」，并更新本路线的兼容策略。

进入阶段 D 前必须书面确认：兼容策略、发版节奏、是否仍保证「只升 web」。

---

## 5. 部署与升级约定

### 5.1 推荐部署

| 模式 | 产物 | 适用 |
|------|------|------|
| 一体部署（默认推荐） | `easytier-web-embed` | 单机快速部署，Win/Linux 直接运行 |
| 分离部署 | `easytier-web` + `frontend/dist` | 前端 CDN/nginx，API 单独升级 |
| 容器 | 现有 Docker 发布链路（若使用） | Linux 服务器 / K8s |

常用端口（默认，可配置）：

- 配置服务器：`22020`
- API：`11211`
- 可选独立 web 静态端口（embed 且拆端口时）

### 5.2 独立升级步骤（原则）

详细操作见：[`../ops/web-upgrade.md`](../ops/web-upgrade.md)。

原则摘要：

1. 备份 SQLite（默认 `et.db`，含可能存在的 `-wal`/`-shm`）及运行配置/环境变量。
2. 确认待发布 web 版本声明的节点兼容范围覆盖现网节点。
3. **只替换二进制/前端，不删除、不更换 db 路径**；启动后自动 migration。
4. 启动后验证：控制台可登录；抽样节点心跳与配置下发正常。
5. 异常则回滚二进制 + 必要时恢复 DB 备份。
6. 生产环境建议 `--db` 使用绝对路径，避免换工作目录导致连到空库。

### 5.3 平台

- **Windows**：使用 CI 产出的 `.exe`（含 `easytier-web` / `easytier-web-embed`）。
- **Linux**：使用 musl 等静态/准静态产物，便于多发行版部署。

具体 target 矩阵以 `.github/workflows/linux.yml` 为准，本文件不重复维护完整列表。

---

## 6. 开发工作流（后续按此执行）

每次改动 `easytier-web` 前自检：

1. **影响面**：是否触碰第 3 节冻结接口？触碰则停止，先改路线或证明向后兼容。
2. **兼容验证**：至少覆盖「旧节点 + 新 web」：心跳、拉/推配置、控制台关键管理路径。
3. **数据**：新增 migration；写明升级与回滚注意点。
4. **前端**：若 API 变更，同步 `frontend` / `frontend-lib`；分离部署时说明 `api_meta` / `--api-host`。
5. **发布**：Win + Linux 产物；发布说明写清兼容节点版本与升级步骤。

建议的变更分类标签（可用于 PR / commit）：

- `web/ui`：纯前端
- `web/api`：REST/控制台 API（节点不可见）
- `web/ops`：部署、日志、配置项、打包
- `web/db`：SQLite / migration
- `web/compat`：任何可能影响节点兼容的改动（需额外审查）

---

## 7. 非目标（短期）

1. 修改 `easytier` / `easytier-core` 行为或强制节点升级。
2. 将 web 拆出为与 monorepo 无关、且零依赖 core 源码的完全独立仓库（成本高，非短期必要）。
3. 承诺任意跨大版本的「乱序版本组合」长期兼容（默认策略是：web 兼容既定节点版本范围）。
4. 以 web 为入口引入新的节点必选协议能力。

---

## 8. 决策记录

| 日期 | 决策 | 说明 |
|------|------|------|
| 2026-09-29 | 双组件独立交付 | core 与 `easytier-web` 可分开部署升级 |
| 2026-09-29 | 短期只演进 `easytier-web` | 冻结 core；保持对现有节点百分百兼容 |
| 2026-09-29 | Win/Linux 一等公民 | 独立升级路径必须覆盖两平台 |
| 2026-09-29 | 协议兼容靠冻结接口 | 同步演进是长期模型；短期用「不改节点可见接口」保证兼容 |
| 2026-09-29 | 正式名统一为 `easytier-web` | 不使用 controller 等别名；三分法为 core / gui / easytier-web |

---

## 9. 修订方式

1. 本文件是 `easytier-web` 演进的源文件；实施计划类文档（如 managed config sync）补充细节，不得与本文冲突。
2. 若要修改冻结接口、开始改 core、或放弃「只升 web」，必须先更新本文「Status / 阶段 / 决策记录」再开发。
3. 阶段 B/C 的具体需求可另开 issue 或子计划文档，但需回链到本文对应阶段。
)
