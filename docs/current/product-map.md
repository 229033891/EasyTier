# EasyTier 产品与组件地图

## Status

- Status: **Current**
- 最近审阅：2026-10-03
- 状态说明：帮助理解 monorepo 内各产品/组件职责与关系
- 范围：仓库内主要 crate、前端包与贡献组件
- 索引：[`../README.md`](../README.md)
- 配套文档：
  - [`architecture.md`](./architecture.md)（core 架构细节）
  - [`../roadmap/web-evolution.md`](../roadmap/web-evolution.md)（Web 演进路线）
  - [`../ops/web-upgrade.md`](../ops/web-upgrade.md)（Web 升级保库）

---

## 1. 术语约定（统一命名）

产品与文档中**统一使用下列正式名称**，避免混用非正式别名：

| 正式名称 | 指什么 | 不要再用 |
|----------|--------|----------|
| **节点 / `easytier-core`（二进制）** | 真正组网的进程 | 含糊的 “easytier code” 等（易与 crate 名混淆时写全） |
| **`easytier-gui`** | 本机图形客户端 | 仅说 “客户端” 时尽量写全名 |
| **`easytier-web`** | 配置服务器 + REST API + 浏览器 UI | **controller**、控制面产品名、其它自造英文名 |

说明：

1. **`easytier-web` 是正式产品名**（含配置服务器与 Web UI；embed 产物为 `easytier-web-embed`）。
2. 口语里说「网页控制台 / 配置后台」可以，书面与代码、Issue、PR、发布说明一律写 **`easytier-web`**。
3. 下文若出现「控制台」，仅指 `easytier-web` 提供的 UI/管理体验，**不是**另一个叫 controller 的产品。

### 1.1 用户视角三分法（推荐心智模型）

| 部分 | 正式名称 | 作用 | 是否必须 |
|------|----------|------|----------|
| 组网节点 | `easytier-core`（二进制） | P2P VPN 数据面/节点 | 是（组网就需要） |
| 本机 GUI | `easytier-gui` | 本机启停与状态 | 否 |
| 集中管理 | **`easytier-web`** | 下发配置 + 浏览器管理 | 否（集中配置时需要） |

---

## 2. 一句话总览

EasyTier 做一件事：**把多台设备组成一张虚拟局域网（P2P VPN）**。

仓库里既有**用户可安装的产品**，也有**库与平台适配层**。先按「谁给谁服务」理解：

```text
                    ┌──────────────────────────┐
                    │  管理 / 配置入口            │
                    │  easytier-web / gui / cli  │
                    └────────────┬───────────────┘
                                 │ 配置 / 控制
                                 ▼
┌──────────────┐     ┌─────────────────────┐     ┌──────────────┐
│ easytier-web │◄───►│ 节点进程 easytier-core│◄───►│  其他节点 P2P  │
│（配置+UI+API）│     │ （由 easytier crate  │     │              │
└──────────────┘     │  编出的可执行文件）    │     └──────────────┘
                     └──────────▲────────────┘
                                │
                 可嵌入：GUI / JS / Go / 移动端 / FFI …
```

---

## 3. `easytier` 与 `easytier-core` 的区别

这是最容易混淆的一对名字。

| | `easytier-core` **crate（库）** | `easytier` **crate（原生宿主）** |
|--|-------------------------------|--------------------------------|
| **是什么** | 可移植的组网内核库 | 原生 composition root（组合根） |
| **职责** | 配置、协议状态、路由、peer、连通编排、包处理、实例生命周期 | OS 资源、TUN、原生协议引擎、进程集成、CLI、本机呈现 |
| **与 OS** | 尽量不直接访问 OS 网络；通过 Host 能力 / Adapter 请求能力 | 真正创建 socket、TUN，对接操作系统 |
| **用户交付** | 一般不单独安装；被其它产物链接/嵌入 | 编出可执行文件：`easytier-core`、`easytier-cli` 等 |

依赖方向：

```text
easytier-proto     （Protobuf / RPC 类型与契约）
       ↑
easytier-core      （可移植组网内核 · 库）
       ↑
easytier           （Windows / Linux 等原生宿主 · 库+二进制入口）
       ↓
发布物进程名：
  easytier-core    ← 日常说的「节点」
  easytier-cli     ← 命令行运维工具
```

### 命名陷阱（务必分清）

| 名字 | 实际含义 |
|------|----------|
| 目录 / crate：`easytier-core` | **库**，核心逻辑 |
| 目录 / crate：`easytier` | **原生宿主**，打出二进制 |
| 发布的可执行文件：`easytier-core` / `easytier-core.exe` | 由 **`easytier` crate** 编译出来的**节点进程**，不是「只含 core 库、没有宿主」的意思 |

**记法：**

- 理解「组网怎么算、怎么路由」→ 看 **`easytier-core` crate**
- 理解「在 Win/Linux 上怎么跑起来、CLI 从哪来」→ 看 **`easytier` crate**
- 用户机器上跑的节点进程 → 通常叫 **`easytier-core` 二进制**

架构权威说明见：[`architecture.md`](./architecture.md)。

---

## 4. 用户最常接触的产品

| 产品 | 位置 / 产物 | 作用 |
|------|-------------|------|
| **节点本体** | `easytier` crate → 二进制 `easytier-core` | 真正组网：打洞、加密、路由、TUN、中继等 |
| **命令行工具** | 同 crate → 二进制 `easytier-cli` | 查看状态、管理 peers/路由等 |
| **桌面 / 移动 GUI** | `easytier-gui`（Tauri + Vue） | 本机图形界面：启停节点、看状态；可对接 `easytier-web` |
| **`easytier-web`** | `easytier-web` + `frontend` | **独立服务**：配置服务器 + REST API + 浏览器 UI |

关系要点：

1. **只组网**：跑节点（`easytier-core` 二进制或 GUI 内嵌）即可，不必部署 `easytier-web`。
2. **集中配置 / 网页管理**：再部署 `easytier-web`；节点连接其配置端口拉配置。
3. **`easytier-gui` ≠ `easytier-web`**：GUI 管本机体验；`easytier-web` 是远程配置中心 + 多用户管理，可单独部署升级。

---

## 5. 工程分层（库，通常不单独安装）

| 组件 | 作用 |
|------|------|
| `easytier-proto` | Protobuf / RPC 类型、描述符与 feature 切片 |
| `easytier-core` | 可移植控制面 / 数据面核心逻辑 |
| `easytier` | 原生宿主：把 core 接到 OS，并提供 CLI/二进制入口 |

```text
easytier-proto  ←  easytier-core  ←  easytier
                         ↑
         easytier-web / GUI / JS / Go / 移动端 / FFI 等调用或嵌入
```

---

## 6. `easytier-web` 内部结构

| 部分 | 作用 |
|------|------|
| `easytier-web`（Rust） | 配置服务器 + REST API + SQLite |
| `easytier-web/frontend` | 浏览器 UI |
| `easytier-web/frontend-lib` | 被 **`easytier-web` 前端** 与 **`easytier-gui`** 共用的前端库 |
| `easytier-web/config-generator` | 配置生成相关前端（可走 wasm） |

交付形态：

- `easytier-web`：API + 配置服务器（可不嵌前端）
- `easytier-web-embed`：同上并嵌入前端静态资源（单文件部署常用）

数据与升级：见 [`../ops/web-upgrade.md`](../ops/web-upgrade.md)（**换程序不换库**）。

---

## 7. 其它运行时与平台组件

| 组件 | 作用 |
|------|------|
| `easytier-js` | 浏览器 / Cloudflare 等 JS 运行时中跑 EasyTier（WASM 等） |
| `easytier-go` | Go 侧嵌入节点能力 |
| `easytier-contrib/easytier-ffi` | C ABI，供其它语言/应用嵌入 |
| `easytier-contrib/easytier-android-jni` | Android JNI 适配 |
| `easytier-contrib/easytier-ios` | iOS 适配 |
| `easytier-contrib/easytier-ohrs` | 鸿蒙 / OpenHarmony 相关 |
| `easytier-contrib/easytier-magisk` | Android Magisk 模块形态 |
| `easytier-contrib/easytier-mini` | 精简原生节点（能力子集） |
| `easytier-contrib/easytier-uptime` | 运维 / 可用性相关贡献组件 |
| `tauri-plugin-vpnservice` | GUI（尤其移动）使用的 VPN Service 插件 |

共性：**同一套组网能力，换皮到不同平台或语言。**

---

## 8. 按场景看逻辑关系

### 场景 A：两台机器直接组网

两边各跑节点（`easytier-core` 二进制或 GUI），相同网络名/密钥，可选公共共享节点做引导。**不需要 `easytier-web`。**

### 场景 B：集中配置与网页管理

1. 部署 `easytier-web`（建议 embed 一体包）
2. 节点指向该配置服务器
3. 浏览器（或 GUI 连 API）改配置 → `easytier-web` 持久化并下发给节点

### 场景 C：普通用户装桌面客户端

安装 `easytier-gui` → 本机启停节点；UI 复用 `frontend-lib`；需要系统 VPN 能力时用 `tauri-plugin-vpnservice`。

### 场景 D：App / 其它语言集成

不直接依赖 CLI，而通过 FFI / JNI / JS / Go 等把核心能力嵌入自有程序。

---

## 9. 与当前 `easytier-web` 演进策略的对应

| 关注点 | 在本地图中的位置 |
|--------|------------------|
| 短期只改、可单独升级 | **`easytier-web`**（配置服务器 + UI + API） |
| 短期冻结、保持兼容 | `easytier` / `easytier-core`（节点侧行为与协议） |
| 二者运行时关系 | `easytier-web` 管配置；节点跑网络；经配置协议对接，进程分离 |

详见：[`../roadmap/web-evolution.md`](../roadmap/web-evolution.md)。

---

## 10. 记忆口诀

- **Core 库** = 干活的组网内核（可移植）
- **easytier crate** = 把内核接到真实 OS，并打出节点/CLI 二进制
- **easytier-core 二进制** = 用户机器上跑的节点进程
- **`easytier-web`** = 发配置 + REST API + 网页 UI（可单独升级；**不要叫 controller**）
- **`easytier-gui`** = 本机图形壳
- **JS / Go / FFI / 移动** = 把同一套节点能力嵌到别的环境
- **proto** = 全家共用的协议与 RPC 类型

---

## 11. 修订方式

1. 新增用户可交付产物或调整职责边界时，更新本文第 4–7 节。
2. `easytier` / `easytier-core` 分层原则以 [`architecture.md`](./architecture.md) 为准；本文只做产品向说明。
3. `easytier-web` 单独演进与升级流程不在此展开，回链到 `easytier-web-*` 专用文档。
4. 产品正式名变更时，先改第 1 节术语约定，再全文替换。
