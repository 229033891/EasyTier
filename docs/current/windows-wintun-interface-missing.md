# Windows：WinTun 网卡「找不到」导致的 ERROR 海啸与 socket 创建失败

## Status

- Status: **Current**
- 日期：2026-10-18
- 最近审阅：2026-10-18（首版：现象解码 / 传播链 / 根本原因；同日修复 P1 / P2a / P2b / P3a / P3b，P4 未定性，已登记 roadmap **P-WIN.1**）
- 范围：**Windows 平台**，`find_interface_index(<dev_name>)` 失配时的行为。不覆盖 Linux / macOS / FreeBSD 的等价路径
- 相关：[`traffic-steering.md`](./traffic-steering.md)（L2 路由安装与 `dev_name`）、[`underlay.md`](./underlay.md)（物理链路排除路由）、[`packet-loss-investigation-2026-10.md`](./packet-loss-investigation-2026-10.md) §6.2（首次现网取证）
- 修复清单：[`../roadmap/code-review-fixes-2026-10.md`](../roadmap/code-review-fixes-2026-10.md)（原记「2026-10-17 现网发现，尚未修」，2026-10-18 已同步为「已修」）
- 索引：[`../README.md`](../README.md)

> 行号均为撰写时锚点，可能漂移；以函数名 / 日志文案为准。

---

## 1. 一句话结论

`find_interface_index(<dev_name>)` 在 WinTun 适配器**暂时不可枚举**时返回 `Err(NotFound)`，而调用方把它当成**致命错误**：

1. 用 `?` 逐层上抛，**打死所有绑定网卡的 tunnel socket 创建**（真实功能损失）；
2. 对同一个事实打**两份 ERROR**（`arch::windows` 手写 + `ifcfg::windows` 的 `instrument(err)` 自动），一次 burst 刷 ~32 行；
3. 路由安装路径只是 WARN + 周期重试，**能自愈**。

**它不是「枚举 API 失败」，是「适配器此刻真的不在」。** 根因见 §4。

---

## 2. 现象解码

现网 WARN/ERROR 视图（2026-10-18，`dev_name = et`，实例启动/停止 churn 期间）：

| 日志 | 出处 | 含义 |
|------|------|------|
| `easytier::arch::windows: Failed to find interface index for et` | `arch/windows.rs` `find_interface_index` | `NetworkInterface::show()` 结果里没有 `name == "et"` 的项 |
| `easytier::instance::virtual_nic: add ipv4 route failed; will retry cidr=… ifname="et" err=NotFound` | `virtual_nic.rs` `apply_route_changes` | `get_interface_index()` → `None` → `Err(Error::NotFound)`；**会重试** |
| `easytier::common::ifcfg::windows: error=Not found` | `ifcfg/windows.rs` 的 `#[tracing::instrument(err, ret)]` | **不是手写日志**；是 `instrument(err)` 自动事件，`Error::NotFound` 的 `Display` 就是 `Not found` |
| `easytier::instance::virtual_nic: configured WinTun adapter not found; recreating with the same name dev_name=et` | `virtual_nic.rs` `create_dev` | `interface_exists()` 返回 `Some(false)` ⇒ **枚举成功**，适配器**确实不在** |
| `add ipv4 route failed … err=AnyhowError(Failed to add route: 找不到元素。 (code: 1168))` | `ifcfg/windows.rs` `add_ipv4_route` | 另一子情形：接口找到了，`luid.add_routes_ipv4` 报 `ERROR_NOT_FOUND` |
| `instance failed to start error=core instance start cancelled` | `instance/lifecycle.rs` `start` 的 `select!` | `cancel` 置位（`CoreInstance::drop` / `stop`），与路由失败**无因果** |

关键区分：`Some(false)` 证明**枚举是成功的**，只是名字没匹配上。所以**不能**把这条当成「枚举 API 抽风」来处理。

---

## 3. 传播链（每环都用 `?`）

```
find_interface_index(iface)              arch/windows.rs         → Err(NotFound) + tracing::error!
  └─ set_ip_unicast_if()                 arch/windows.rs         → Err            （`?`）
       └─ setup_socket_for_win()         arch/windows.rs         → Err            （`?`）
            └─ setup_socket2_ext()       tunnel/common.rs        → TunnelError    （`?`）
                 └─ tunnel::common::bind<B>()   tunnel/common.rs → TunnelError
```

`tunnel::common::bind<B>()` 是**所有 tunnel 类型共用的 socket 创建入口**（builder 形式，`dev: BindDev` 决定绑哪张网卡）。因此 `find_interface_index` 失败的那一刻，**任何需要绑定网卡的 tunnel / 打洞 socket 都建不出来**。

对比路由路径 —— 它只是 WARN，不进 `installed` 集合，下一 tick 重试：

```
apply_route_changes → ifcfg.add_ipv4_route → get_interface_index → None → Err(NotFound)
   → Err(err) => tracing::warn!("add ipv4 route failed; will retry")     virtual_nic.rs
```

---

## 4. 根本原因

### 4.1 为什么适配器会「不在」

`dev_name` 是**用户固定配置**（`et`；自动生成名形如 `et_12_ab3d`，见 `easytier-gui/src-tauri/src/manager.rs` 的 `is_automatic_windows_dev_name`）。每次实例 stop 拆掉它，下次 start 用**同一个名字**重建。启动/停止 churn 期间就存在「名字存在但适配器不在」的窗口。

触发本次现网 burst 的是 **start/stop churn**：日志中同一 `instance_id` 连续两次 `core instance start cancelled`，中间夹着 `not every direct route session acknowledged the shutdown withdrawal`（stop 路径）。`cancel` 的置位点是 `CoreInstance::drop`（`instance/mod.rs`）与 `lifecycle.rs` 的失败/停止分支。

### 4.2 为什么「暂时不在」会被放大成致命错误

**修复前：`wait_interface_show` 只在建网卡那一次生效，之后再也没有任何就绪门控。**

```
create_dev()                        virtual_nic.rs
  └─ wait_interface_show(ifname)    ← 全仓唯一一处「等网卡出现」
                                       （修复前：无限循环 + 100ms 轮询；P3a 已加 30s 超时）
run()                               virtual_nic.rs
  ├─ assign_ipv4_to_tun_device()
  └─ run_proxy_cidrs_route_updater()   ← 立刻装路由
        └─ sync_proxy_cidrs_with_underlay_excludes() → apply_route_changes()
                                       （修复前无门控；P3b 已在批次顶部用 `interface_present` 整批拦截）
```

`wait_interface_show` 全仓只有 **1 处**调用（`virtual_nic.rs` 的 `create_dev`）。`interface_exists` 修复前只有 1 处（`virtual_nic.rs` 的 `create_tun`，由 `create_dev` 调用），P3b 新增 1 处，作为 `ifcfg/windows.rs` 里 `interface_present` 的静默探针。因此适配器一旦在**建好之后**消失（stop、重建、外部删除），后台任务仍按名字使用它：

- **1s 路由 updater**（`tokio::time::interval(1s)`）→ 期望集合里的 32 条路由逐个 `NotFound`（现网记录「每波约 32 次」）。**Windows 已由 P3b 整批跳过**（每 tick 1 行 WARN）；`interface_present` 在 trait 上默认返回 `true`，其它平台仍保留「逐条失败 + 逐条 WARN」。
- **socket 绑定路径** → 与 NIC 生命周期无关的组件（隧道建立、打洞）可能活得更久，修复前直接建不出来（P1 已降级为「本次不绑网卡 + WARN」）。

### 4.3 语义缺陷：两种情况被合并

```rust
pub fn find_interface_index(iface_name: &str) -> io::Result<u32> {
    let ifaces = NetworkInterface::show().map_err(...)?;   // ① 枚举失败 → 错误
    if let Some(iface) = ifaces.iter().find(|i| i.name == iface_name) {
        return Ok(iface.index);
    }
    tracing::error!("Failed to find interface index for {}", iface_name);  // ② 名字没找到
    Err(io::Error::new(io::ErrorKind::NotFound, iface_name.to_string()))
}
```

② 是**可恢复的瞬态**，却同时被当成「ERROR 级事件」和「致命错误」。正确语义：

- ① 保留为错误（调用方无法区分时不应继续）；
- ② 对 **socket 绑定**降级为「本次不绑网卡 + WARN」，对 **路由安装**保持 WARN + 重试。

### 4.4 为什么 2026-10-07 的降噪没盖住它

`easytier/src/common/log/mod.rs` 的 `effective_log_level` 匹配条件极窄：

```rust
if level == Level::Error
    && target.starts_with("wintun")                          // 必须 native WinTun DLL
    && message.contains("Failed to find matching adapter")   // 必须这条文案
{ Level::Debug } else { level }
```

它只吃 **rust-tun 先 open 后 create** 时 native DLL 的噪音。本文两条日志的 target 是 `easytier::arch::windows` / `easytier::common::ifcfg::windows`，**都不匹配**，照旧打 ERROR。

---

## 5. 影响

| 严重度 | 影响 |
|--------|------|
| 🔴 功能 | 适配器不可枚举的窗口内，**所有绑定网卡的 tunnel / 打洞 socket 创建失败**（`?` 上抛），不是「退化为不绑网卡」 |
| 🟡 可观测性 | 一次 burst ~32 行 ERROR，两份重复事实，淹没同期的 INFO（如 `Interface found`） |
| 🟢 自愈 | 路由安装失败只是 WARN + 重试；`installed` 仅在成功时更新，不会假记账 |
| ⚪ 无因果 | `core instance start cancelled` 由 start/stop churn 触发，**不是**路由失败导致 |

---

## 6. 修复

### 已修（2026-10-18）

| ID | 内容 | 位置 |
|----|------|------|
| **P1** | `set_ip_unicast_if` 找不到网卡时**降级为「本次不绑网卡」+ WARN**，不再 `?` 打死 socket 创建 | `arch/windows.rs` `set_ip_unicast_if` |
| **P2a** | `find_interface_index` 失配改 **WARN**（不再是 ERROR），错误值不变以保持调用方语义 | `arch/windows.rs` `find_interface_index` |
| **P2b** | `ifcfg/windows.rs` 里 6 个按名字解析适配器的 helper，`#[instrument(err, ret)]` → **`err(level = WARN)`**，消除同一事实的第二份 ERROR | `common/ifcfg/windows.rs` |
| **P3a** | `wait_interface_show` 加 **30s 超时**，避免适配器始终不出现时 `create_dev` 无限挂起（形态即「卡在 Starting 然后 start cancelled」） | `ifcfg/windows.rs` |
| **P3b** | 路由 reconcile **整批就绪门控**：新增 trait 方法 `interface_present`（默认 `true`，不影响其他平台；Windows 用**静默**的 `interface_exists` 覆盖），在 `sync_proxy_cidrs_with_underlay_excludes` 顶部拦截 —— 接口不在则整批跳过（1 行 WARN + 上报 `last_error`），不再逐个失败 32 次 | `ifcfg/mod.rs`、`ifcfg/windows.rs`、`virtual_nic.rs` |

> **P2 的实现选择**：没有去扩 `common/log/mod.rs` 的 `effective_log_level`。那个过滤器只能看 `record.args()`（`message` 字段），而 `#[instrument(err)]` 产生的是**字段事件**（`error = …`，无 `message`），过滤器**看不到**，无法按内容匹配。改用 `err(level = WARN)` 直接在生产端定级，干净且不需要 log-filter 特判。`effective_log_level` 原有的 native-WinTun 窄匹配保持不变。

### 未修（记录在案）

| ID | 内容 | 原因 |
|----|------|------|
| **P4** | start/stop churn 的来源（同一 instance 连续两次 `start cancelled`） | **触发器**，未定性：GUI 重复 start？stop 未完成即 start？需单独取证。已登记为 [`../roadmap/connection-stability-todo.md`](../roadmap/connection-stability-todo.md) 的 **P-WIN.1** |

> **P3b 的门控为什么只对 Windows 生效**：`interface_present` 在 trait 上默认返回 `true`，只有 `WindowsIfConfiger` 覆盖。Linux/macOS/FreeBSD 的接口查询可靠，保留原有「逐条失败 + 逐条 WARN」语义，避免用一个平台的问题去掩盖其他平台的真实错误。若将来其他平台出现同类 burst，再按需覆盖。

---

## 7. 复现与验证

- **复现**：Windows 上配置固定 `dev_name`（如 `et`），让实例 start → stop → start 快速循环，观察 `Failed to find interface index` 是否成簇出现。
- **验证 P1**：人为删除/重命名 WinTun 适配器后再发起连接，应看到 WARN 且**隧道仍能建立**（不再因绑定失败而建不出来）；同时确认降级后 socket 的**实际出口网卡**符合预期（未设 `IP_UNICAST_IF` 时由 OS 选路），并留意 WARN 的 `present` 字段区分「适配器不在」与「枚举失败」。
- **验证 P2**：上述日志不应再以 ERROR 出现（`arch::windows` 一行 WARN；`ifcfg::windows` 的 `error=Not found` 降为 WARN）。
- **验证 P3a**：构造适配器始终不出现的场景，`create_dev` 应在 30s 后返回明确错误，而不是无限等待。
- **验证 P3b**：churn 期间观察日志 —— 每 tick 应只有 **1 行**「interface is not enumerable; skipping this route reconcile batch」，而不是 ~32 行 `add ipv4 route failed`；接口恢复后 `installed` 应在一个 tick 内收敛（`proxy_cidr_route_sync` 的 `last_error` 变空）。

**静态验证（2026-10-18）**

| 检查 | 结果 |
|------|------|
| `cargo check -p easytier --lib`（Windows 宿主，`#[cfg(target_os = "windows")]` 路径真实参与编译） | ✅ 通过 |
| `cargo clippy -p easytier --lib` | ✅ 无新增告警 |
| `cargo test -p easytier --lib` | ⚠️ 未能运行：构建脚本需联网下载 VC-LTL / YY-Thunks，本环境不可用（与代码无关） |

> 绕过联网的配方：把 `VC_LTL` / `YY_THUNKS` 指向已缓存的解包目录（如 `target/debug/build/easytier-*/out/VC-LTL-5.2.2`、`.../YY-Thunks-1.1.7`），thunk-rs 会直接复用、不再 `curl` 下载。

P1–P3b 均无独立单测；改动为控制流 + 日志定级，由上述编译/静态检查覆盖。**真机复现与验证仍待做。**

---

## 8. Changelog

| 日期 | 说明 |
|------|------|
| 2026-10-18 | 首版：解码现网 WARN/ERROR 视图；确认「枚举成功但名字失配」而非枚举 API 失败；定位传播链与两层根因；落地 P1 / P2a / P2b / P3a（`cargo check` + `clippy` 通过，真机待验），记录 P3b / P4 |
| 2026-10-18 | 补 **P3b**：`interface_present` 整批就绪门控（trait 默认 `true`，仅 Windows 覆盖，用静默的 `interface_exists`）；P4 登记为 roadmap **P-WIN.1** |
| 2026-10-18 | **复核修正**：① `wait_interface_show` 的轮询改用静默探针（原来每 100ms 一条 WARN，30s 上限下最多 ~300 行，与 P2b 降噪目标相抵）；② `set_ip_unicast_if` 降级 WARN 增加 `present` 字段，区分「适配器不在」与「枚举失败」；③ §4.2 / §7 / Status 口径随 P3a·P3b 校正（原文仍写「无任何就绪门控」「P1–P3a」） |
