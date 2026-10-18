# Windows 安装器重启提示：根因审查与修复方案

## Status

- Status: **Current**（审查结论 + 修复已实施，待实机回归）
- 最近审阅：2026-10-18（实施 A+B：`ET_TryDeleteBak` 消除重启标志 / 强制 `perMachine` + 存量迁移；makensis 编译验证通过）
- 范围：`easytier-gui` 的 NSIS 安装器（`easytier-gui/src-tauri/windows/`）在 Windows 上的重启提示触发链路
- 索引：[`../README.md`](../README.md)
- 相关：[`desktop-gui-and-config-server.md`](./desktop-gui-and-config-server.md)（`ET-Gui` 进程模型）· [`windows-wintun-interface-missing.md`](./windows-wintun-interface-missing.md)（同为 Windows 平台面）
- 安装打包流程：[`../ops/windows-build-pack.md`](../ops/windows-build-pack.md)

本文回答一个问题：**为什么在 Windows Server 上装 ET 不提示重启，在 Windows 11 上装就提示重启**，并给出经核实的修复方案。

> **阅读提示**：本文有两次结论修正（§2.3、§5.1）。历史讨论中「currentUser 安装导致无法停服务」的说法**已被本机实测推翻**，请以本文 §3 为准，不要引用旧结论。

---

## 1. 现象

同一份 `ET_2.7.7_x64-setup.exe`：

| 机器 | 安装后 |
|------|--------|
| Windows Server 2022 | 正常结束，**不提示重启** |
| Windows 11 | 提示**需要重启** |

容易误判为「Windows 版本差异」。**不是。**

---

## 2. 先把机制钉死：重启提示只有一个来源

### 2.1 安装器里没有任何一行主动弹「请重启」

全仓检索 `REBOOT|restart required|RebootRequired` 的结果：`installer.nsi` 与 `installer-hooks.nsh` 中**没有**任何重启提示文案；`English.nsh` 的 28 条 `LangString` 里也没有。`lib.rs:983` 的 `need_restart` 是 **RPC server 重建**，与系统重启无关。

### 2.2 唯一来源：NSIS `/REBOOTOK` → 置位重启标志

NSIS 官方语义（[Delete 参考](https://nsis.sourceforge.io/Reference/Delete)）：

> If `/REBOOTOK` is specified and the file cannot be deleted then the file is deleted when the system reboots -- **if the file will be deleted on a reboot, the reboot flag will be set.**

安装结束时 NSIS 见到该标志 → 弹重启收尾页。底层即 `MoveFileEx(..., MOVEFILE_DELAY_UNTIL_REBOOT)` 写 `PendingFileRenameOperations`（[微软文档](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw)）。

### 2.3 ET 里的 6 处 `/REBOOTOK`（逐行核对）

| 位置 | 目标 | 是否可能失败 |
|------|------|--------------|
| [installer-hooks.nsh#L141](../../easytier-gui/src-tauri/windows/installer-hooks.nsh#L141) | `ET.exe.bak` 等主二进制备份 | **可能**（被映射） |
| [installer-hooks.nsh#L192-L214](../../easytier-gui/src-tauri/windows/installer-hooks.nsh#L192-L214) | `Packet.dll.bak` / `wintun.dll.bak` / `WinDivert*.sys.bak` | **可能**（被映射） |
| [installer-hooks.nsh#L327](../../easytier-gui/src-tauri/windows/installer-hooks.nsh#L327) | `easytier-gui.exe.bak` 兼容 shim | 可能 |
| [installer.nsi#L819](../../easytier-gui/src-tauri/windows/installer.nsi#L819) | `resources_ancestors` 目录（仅卸载） | 一般不触发 |

**关键**：`Packet.dll` 是 GUI 主程序的**静态导入**（`pnet_datalink` 的 `#[link(name = "Packet")]`），进程活着就必然锁着；`wintun.dll` 在有 TUN 时被 `LoadLibrary`；`WinDivert*.sys` 由内核驱动映射。

→ **只要安装那一刻还有 ET 进程存活，`.bak` 必删不掉 → 必置重启标志 → 必提示重启。**

### 2.4 结论：差异来自「安装时是否还有进程存活」，不是系统版本

`installer-hooks.nsh#L35-L38` 的既有注释已给出同向判断，本次实测证实：

| | Windows Server（本机实测） | Windows 11 |
|---|---|---|
| 安装前是否在跑 | 干净机器 | 托盘常驻 + `ET-Gui` 服务运行 |
| `.bak` 能否立即删 | 能 → 不置标志 | 不能 → 置标志 |
| 结果 | 无提示 | **提示重启** |

再加一层放大器：ET 关闭窗口是**隐藏到托盘**而非退出，所以桌面机「重装/升级」几乎必中，服务器「首次全新安装」几乎不中。

---

## 3. 实测证据（本机 Windows Server 2022 / 10.0.20348）

### 3.1 安装形态

```
HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\ET  →  ET 2.7.7
InstallLocation : C:\Users\Administrator\AppData\Local\ET
```

即 **currentUser 安装**，`Tauri` 生成物确认 `!define INSTALLMODE "currentUser"` → `RequestExecutionLevel user`。

### 3.2 服务与进程（关键）

```
ET-Gui 服务: State=Running  StartMode=Auto
PathName  : C:\Users\Administrator\AppData\Local\ET\ET.exe ... --daemon
           --config-server udp://et.scmec.vip:2202 --secure-mode
```

| PID | Session | Owner | Parent | Elevated |
|-----|---------|-------|--------|----------|
| 6360 | 0 | `NT AUTHORITY\SYSTEM` | `services.exe` | **True** |
| 17072 | 2 | `DEV\Administrator` | `explorer.exe` | **True** |

```powershell
# 令牌提权检测（TokenElevation），复现用
[Tok]::IsElevated(17072)  # → True
```

### 3.3 两个决定性事实

1. **服务 binPath 是运行时推导的**：`service.rs:57` 用 `program: crate::get_exe_path()`，而 `lib.rs:1487-1496` 的 `get_exe_path()` 就是 `std::env::current_exe()`。→ **服务路径跟随可执行文件本身，不写死安装目录。**
2. **GUI 进程本身是提权的**：`lib.rs:1528` 在 `run_gui()` 入口调用 `check_sudo()`；未提权则 `elevate::Command` 自我重启（`lib.rs:1499-1513`）。这就是为什么托盘进程 PID 17072 父进程是 `explorer.exe` 却 `Elevated=True`。

### 3.4 ⚠️ 对早期结论的修正

早期判断「currentUser 安装 → 普通权限 → 无法 `sc stop` SYSTEM 服务 → 必然失败」，**不成立**。

- 微软默认服务 DACL 确实不含 `SERVICE_STOP`（[文档](https://learn.microsoft.com/en-us/windows/win32/services/service-security-and-access-rights)）；
- **但** ET 的主进程实际运行在**提权令牌**下（§3.3 第 2 点），拥有 `SERVICE_ALL_ACCESS` 级别能力。

→ 所以「提权不足」**不是**重启提示的根因。真正根因是 §2.4 的**时序问题**：安装器执行 `Delete /REBOOTOK` 时，进程尚未完全释放文件映射。

---

## 4. 当前安装器的提权链路（逐段核对）

`NSIS_HOOK_PREINSTALL` → [`installer-hooks.nsh#L290-L298`](../../easytier-gui/src-tauri/windows/installer-hooks.nsh#L290-L298)：

1. `ET_ElevateServiceStopForInstall`（L259-L288）：探测服务是否 `RUNNING`；是则 `ExecShellWait "runas"` 提权 `sc stop`。**静默安装会弹 UAC。**
2. `ET_EnsureGuiStopped`（L152-L160）：`sc stop` → `taskkill` → 轮询 → PowerShell 清扫 → `Rename` 主 exe。
3. `ET_UnlockThirdPartyBinaries`（L162-L218）：`Rename` 为 `.bak` → `Delete /REBOOTOK`。

**问题点**：第 3 步的 `Delete /REBOOTOK` 与第 2 步的进程退出之间存在竞态。`Rename` 成功后 `.bak` 仍可能被残留映射持有数秒（`Stop-Process` 到内核释放映像有延迟），此时 `Delete` 失败即置重启标志。

---

## 5. 修复方案（分三阶段，禁止合并发布）

### 5.1 阶段 A：止血 —— 消除「为清理垃圾而重启」（建议先做）

**目标**：即使 `.bak` 一时删不掉，也**不置**重启标志。

**改法**（`installer-hooks.nsh`）：

1. `Rename` 成功后，用**有限次重试**删除 `.bak`（例如 5 次 × 400ms），覆盖映像释放延迟；
2. 重试仍失败则**放弃本次删除**，改用 `MoveFileEx` 直写 `PendingFileRenameOperations`（不经 NSIS 标志位），或干脆**留残留**，由**下次安装的 PREINSTALL 前置清理**；
3. **移除** 所有「仅用于清理 `.bak`」的 `Delete /REBOOTOK`。

**收益**：消除绝大多数 Win11 重启提示，**不触碰提权模型**，回归面小。
**风险**：低。副作用仅为极端情况下目录内短暂残留 `.bak`。
**工作量**：改 0.5h，验证 1 天（需 Win11 真机 + 服务运行态覆盖升级）。

> **验收标准**：Win11 上「托盘常驻 + `ET-Gui` Running」状态下覆盖安装，**安装结束不出现重启提示**，且 `%LOCALAPPDATA%\ET` 下无 `.bak`。

### 5.2 阶段 B：根治 —— 收敛到机器级安装（独立排期）

**目标**：让安装器与解卸载器天然处于可管理服务的上下文，从源头不产生 `.bak`。

| 项 | 现状 | 目标 |
|---|---|---|
| 安装形态 | `currentUser` → `%LOCALAPPDATA%\ET` | `perMachine` → `Program Files\ET` |
| 安装上下文 | `RequestExecutionLevel user` | `RequestExecutionLevel admin` |
| 注册表 | `HKCU\...\Uninstall\ET` | `HKLM\...\Uninstall\ET` |

**有利条件**（已验证，§3.3）：服务 binPath 由 `get_exe_path()` 动态推导，**迁移后服务路径自动正确**，无需硬编码改写。`installer-hooks.nsh` 已有的 `ET_MigrateGuiServiceBinPath`（L300-L307）可继续兜底老版本。

**代价与风险**：

- **存量迁移**：`HKCU` → `HKLM`、路径变更，老用户无法被新安装器直接覆盖升级，需要**迁移引导**（检测旧安装 → 提权卸载 → 重装）。
- **Tauri 2 无公开 `installMode`**：只能改已 fork 的 `installer.nsi` 模板（本项目已 fork，可行）。
- **弹 UAC**：企业静默部署需确认「管理员上下文运行」的策略前提。

**工作量**：1–2 周（含迁移脚本 + 三机种回归）。

### 5.3 阶段 C：企业合规（持续）

- 退出码遵循标准语义：需重启时返回 **3010**，而非弹收尾页；
- 安装日志落盘（`DetailPrint` 目前不写文件）；
- 代码签名（对外分发前提）。

---

## 6. 其他产品的做法（对照）

| 产品 | 安装形态 | 服务归属 | 文件占用处理 |
|---|---|---|---|
| WireGuard for Windows | MSI / `wireguard.exe` 服务常驻 | SYSTEM 服务 | 服务先停，安装器已是 admin |
| Tailscale | 纯 MSI，默认 `C:\Program Files\Tailscale`；策略集中 `HKLM\SOFTWARE\Policies\Tailscale`（[文档](https://tailscale.com/docs/install/windows/msi)） | 系统服务 | admin 上下文直接替换 |
| OpenVPN GUI / 主流 VPN | MSI，机器级 | 服务 | 安装器提权 |
| **ET（现状）** | **NSIS，`%LOCALAPPDATA%`** | **SYSTEM 服务** | **rename → `.bak` → `/REBOOTOK`** |

行业共性：**机器级安装 + 安装器本身具备服务控制权限**，因此不存在「用户进程去停系统服务」的竞态，也就不会产生重启提示。ET 是其中唯一采用「用户级安装目录」的形态。

> 说明：以上对照基于各产品公开安装文档，未逐字节核对其安装器实现；仅用于说明形态差异，不作实现依据。

---

## 7. 待实施清单

| ID | 内容 | 阶段 | 状态 |
|----|------|------|------|
| R-1 | `.bak` 删除改为有限重试，失败不置重启标志 | A | **已实施**（`ET_TryDeleteBak`） |
| R-2 | 移除仅用于清理 `.bak` 的 `Delete /REBOOTOK` | A | **已实施**（9 处全部改为 `ET_TryDeleteBak`） |
| R-3 | PREINSTALL 增加上次残留的前置清理 | A | **已实施**（`ET_PurgeStaleBackups`） |
| R-4 | 安装日志落盘 | A | 待做（`DetailPrint` 仍不落盘） |
| R-5 | `installer.nsi` 切换 `perMachine` + 存量迁移 | B | **已实施**（见 §11） |
| R-6 | 退出码 3010 语义 | C | 待做 |

### 实施说明（2026-10-18）

**关键实现点**：`ET_TryDeleteBak` 用**有界重试**（6 次 × 400ms）覆盖「进程退出到内核释放映像」的窗口；仍失败则**放弃并记录日志**，交由下次安装的 `ET_PurgeStaleBackups` 清理。全程**不触碰** NSIS 重启标志——这是消除提示的核心。

残留的 `RMDir /REBOOTOK`（`installer.nsi` 卸载段）**有意保留**：卸载时目录残留确实需要重启清理，属合理语义，且不发生在安装路径上。

---

## 8. 验证矩阵（每个阶段都必须覆盖）

| # | 场景 | 关注点 |
|---|------|--------|
| 1 | 全新安装（无旧版） | 不提示重启 |
| 2 | 覆盖升级（托盘常驻 + `ET-Gui` Running） | **主用例**，不提示重启、无 `.bak` |
| 3 | 覆盖升级（仅托盘，服务停止） | 同上 |
| 4 | 静默 `/P` / `/S` | **不弹 UAC 卡死**，退出码正确 |
| 5 | 卸载（服务运行中） | `ET-Gui` 被删除，目录清空 |
| 6 | 从 2.7.7 升级到新版本 | 存量迁移正确 |

机种：Windows 11（主）+ Windows Server 2022 + 一个干净 Win10。

---

## 9. 复现命令

```powershell
# 1) 确认安装形态
Get-ItemProperty HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ET |
  Select-Object DisplayVersion, InstallLocation

# 2) 确认服务与进程
Get-CimInstance Win32_Service -Filter "Name='ET-Gui'" |
  Select-Object State, StartMode, PathName
Get-CimInstance Win32_Process -Filter "Name='ET.exe'" |
  Select-Object ProcessId, ParentProcessId, ExecutablePath

# 3) 确认重启标志来源（安装后立即查）
(Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager' `
  -Name PendingFileRenameOperations -ErrorAction SilentlyContinue
).PendingFileRenameOperations |
  Where-Object { $_ -match 'wintun|Packet|WinDivert|ET\.exe' }

# 4) 确认残留
Get-ChildItem "$env:LOCALAPPDATA\ET" -Filter *.bak -Force
```

**反向验证（最快确认链路）**：在 Win11 上先手动停 `ET-Gui` 服务并退出托盘，再装同一安装包——若不再提示重启，即 100% 证实 §2.4。

---

## 10. 已知缺口

- 本机**无 `makensis` / `cargo-tauri`**，本审查**未实际构建**安装包；`.bak` 残留与 `Delete` 失败为**机制推断 + 官方语义**，尚未在本机复现（本机安装时进程在跑但未观察到待重启项，与「Rename 成功且删除成功」一致）。
- Windows 11 侧的重启提示为**用户报告现象**，未取得该机器的 `PendingFileRenameOperations` 快照；§5.1 验收时须补齐取证。
- 未核对安装包是否已代码签名。

---

## 11. 已实施改动（2026-10-18）

### 11.1 文件

| 文件 | 改动 |
|------|------|
| [`installer-hooks.nsh`](../../easytier-gui/src-tauri/windows/installer-hooks.nsh) | 新增 `ET_TryDeleteBak` / `ET_PurgeStaleBackups` / `ET_MigratePerUserInstall`；9 处 `Delete /REBOOTOK` 改为安全删除；`ET_ElevateServiceStopForInstall` 注释更新 |
| [`installer.nsi`](../../easytier-gui/src-tauri/windows/installer.nsi) | 强制 `INSTALLMODE "perMachine"`（保留 `LEGACYINSTALLMODE`）；新增 `Var LegacyUserDir`；`RestorePreviousInstallLocation` 支持识别旧 per-user 安装 |

### 11.2 A 阶段：消除「为清理垃圾而重启」

```
ET_TryDeleteBak <path>
  ├─ 最多 6 次 × 400ms 尝试 Delete
  ├─ 成功 → 结束
  └─ 仍失败 → DetailPrint 警告，留给下次安装；**绝不置重启标志**
```

调用点（9 处）：`ET_UnlockOneInstallExe`(1) · `ET_UnlockThirdPartyBinaries`(8) · `POSTINSTALL` shim 清理(1)。
`ET_PurgeStaleBackups` 在 PREINSTALL **最早期**执行，清理上一轮遗留。

### 11.3 B 阶段：perMachine + 存量迁移

**强制 machine-wide**：`installer.nsi` 在定义处覆盖 `{{install_mode}}`，因为 Tauri 2 无公开 `installMode` 选项。效果：

```nsis
!define LEGACYINSTALLMODE "{{install_mode}}"   ; 保留原值用于迁移判断
!define INSTALLMODE "perMachine"               ; 强制覆盖
→ RequestExecutionLevel admin
→ $INSTDIR = $PROGRAMFILES64\ET
```

**存量迁移**（`ET_MigratePerUserInstall`，仅在检测到旧 per-user 安装时执行）：

1. `RestorePreviousInstallLocation` 探测 HKCU 下的旧路径 → 存入 `$LegacyUserDir`
2. 停 + 删旧 `ET-Gui` / `easytier-gui` 服务注册
3. 清扫旧目录存活进程
4. 删 HKCU 卸载注册项（避免「程序和功能」出现两条）
5. `RMDir /r` 旧目录（**不使用** `/REBOOTOK`）
6. **保留** `%APPDATA%` / `%LOCALAPPDATA%` 用户配置（bundle id 不变，迁移后配置无缝继承）

### 11.4 验证方式（本机实测）

用 Tauri 自带的 `makensis`（`%LOCALAPPDATA%\tauri\NSIS`）+ 真实模板 + 真实 hooks 编译：

| 验证项 | 方法 | 结果 |
|--------|------|------|
| NSIS 语法 / 宏展开 | `makensis /V2` 全量编译 | **退出码 0** |
| `perMachine` 生效 | 在 `currentUser` 分支埋必失败宏 | **未触发** ⇒ 分支未激活 |
| 迁移宏被插入 | 在宏内埋非法指令 | **报错链 `NSIS_HOOK_PREINSTALL → ET_MigratePerUserInstall`** ⇒ 确已插入 |
| `/REBOOTOK` 清除 | 全文件检索 | 仅剩注释 + 卸载段 `RMDir`（有意保留） |

> 说明：上述为**编译期验证**，证明改动可编译、分支正确、宏已插入。**运行时行为**（真实安装/升级/卸载）仍需按 §8 矩阵在 Win11 + Server 实机回归。

### 11.5 尚未覆盖

- **R-4 安装日志**：`DetailPrint` 仍不落盘，企业排障不便。
- **实机回归**：本次未执行 §8 的 6 条场景。
- **CI**：未改 `windows.yml`（产物路径仍为 `bundle/nsis/*.exe`，NSIS 形态未变，无需改动）。
