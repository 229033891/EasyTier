# 服务模式 Web 配置同步 & 配置页下拉框修复复查

- Status: **Archive**（历史复查记录；日常以 Current 为准）
- 日期：2026-10-04
- 分支：`releases/dev1005`
- 目标读者：实现复查 / QA / 发布前确认
- **Current SoT**（进程模型与回写路径）：[`../current/desktop-gui-and-config-server.md`](../current/desktop-gui-and-config-server.md)
- Agent 系统入口：[`../current/system-overview.md`](../current/system-overview.md)

---

## 1. 问题 A：服务模式保存 web-owned 配置误报「client 未运行」

### 1.1 现象

GUI 服务模式顶部状态显示「已连接」，但保存 web-owned 配置时提示类似：

> web-owned config saved locally but config-server client is not running

### 1.2 根因

| 组件 | 所在进程 | 行为 |
|------|----------|------|
| Config-server `WebClient` | ET-Gui **服务进程** | 真正持有会话；`GetConfigServerStatus` 由此汇报 connected |
| GUI 进程 `WEB_CLIENT` | GUI 进程 | 切到 service 模式时被清空 |
| 旧的 `save_network_config` 同步 | GUI 进程 | 只查本地 `WEB_CLIENT` → 恒为「未运行」 |

状态查询已走 RPC，同步却仍走本进程句柄，造成「已连接却无法同步」。

### 1.3 正式修复（非临时方案）

统一权威路径：**进程级 `ConfigServerReportClient` registry**。

1. `WebClient` 启动时 `install_config_server_report_client`；Drop 时按 `Arc::ptr_eq` 清理（重叠生命周期安全）。
2. Proto 增加 `WebClientService.ReportManagedNetworkConfig`。
3. 服务/远程：GUI → RPC → 服务进程读同一 registry → 上报 Console。
4. 本机 normal：GUI 直接 `report_via_process_client`（与 RPC 服务端同一入口）。
5. 稳定 wire `error_code`：`not_enabled | not_connected | not_authorized | revision_conflict | ownership_conflict | invalid`；GUI 文案集中映射。
6. `GetConfigServerStatus` 与 sync 共用 registry 的 connected 判断。

### 1.4 关键文件

- `easytier-proto/proto/api_manage.proto`
- `easytier-core/src/management/full/config_server_client.rs`（新建）
- `easytier-core/src/management/full/web_client.rs`
- `easytier-core/src/management/full/process_rpc.rs`
- `easytier-gui/src-tauri/src/lib.rs`
- `easytier-contrib/easytier-ohrs/src/nearby_management.rs`（stub：`not_enabled`）

### 1.5 Bug 复查结论

| 项 | 结论 |
|----|------|
| 本机 registry 与 RPC 双路径 | 先 `report_via_process_client`；仅 `NotEnabled` 回落 RPC，避免 `is_some()` TOCTOU |
| 服务模式 registry 为空 | 预期；必须走 RPC |
| Drop / 替换 WebClient | `ptr_eq` 清理，旧实例 Drop 不会抹掉新实例 |
| Revision conflict | 本地已持久化编辑保留；revision 缓存在 **owner 进程**刷新；可重试或从 Console 重载 |
| OHOS nearby | 明确 stub，不假装可同步 |
| 新旧版本混部 | 旧服务无新 RPC → GUI 得方法缺失错误（需成对升级 GUI+服务） |
| 单测 | `management::full::config_server_client` 3/3 通过（含串行锁防全局 registry 竞态） |

残余风险（可接受）：

- 本机全量 GUI 链接受本机 MSVC/aws-lc 环境影响，未在本机完整 `cargo check -p easytier-gui`；core 相关单测已通过。

已消除：

- `mark_enabled` / install 顺序：现为先 install report client，再 `mark_enabled`；Drop 先清 status 再清 registry。
- `full/mod.rs` 多余 `pub use`（`ConfigServerReportClient` / `clear_*` / `install_*` / `managed_report_ok` / error_code 别名）已收紧，避免 clippy `-D warnings`。

### 1.6 建议手测

1. **Normal + config-server**：改 web-owned 配置 → 保存成功；断线后应提示 disconnected。
2. **Service + config-server**：状态 Connected → 保存 web-owned → 应成功同步（不再报 client 未运行）。
3. **Service 未配置 config-server**：保存 web-owned → `not_enabled` 文案。
4. **升级配对**：新旧 GUI/服务混用时确认错误可理解。

---

## 2. 问题 B：端口转发 TCP/UDP 与配置页下拉样式

### 2.1 现象

端口转发协议使用 `SelectButton`，视觉上像蓝色 `tcp|udp` 标签，不像可点选控件；初始节点协议为可输入 `AutoComplete`，与加密/压缩 `Select` 不统一。

### 2.2 修复

| 位置 | 原控件 | 新控件 |
|------|--------|--------|
| 端口转发协议（列表 + 编辑对话框） | `SelectButton` | `Select`（`et-proto-select`，嵌在绑定地址 `InputGroup` 左侧） |
| 初始节点 / 监听 / mapped listener（`UrlInput`） | `AutoComplete` | `Select`（固定协议表；未知/旧协议仍临时入选项以保证 round-trip） |
| 加密算法 / 数据压缩 | `Select` | 保留，加 `et-select` 统一高度/chevron |
| ACL chain type | `Select` | 加 `et-select` |
| ACL action / 默认动作、图表时间范围等 | `SelectButton` | **保留**（二选一/少选项切换更合适） |
| 子网代理 / 路由等 | `AutoComplete` multiple | **保留**（需自由输入 CIDR） |

样式：`style.css` 增加紧凑协议下拉宽度、InputGroup 圆角衔接、chevron 悬停主色。

### 2.3 测试

- `easytier-web/frontend-lib/tests/config-ui.spec.ts`：协议断言改为 `#port_forward_proto_0`；`SelectStub` 支持 string options。

---

## 3. 复查清单（发布前）

- [ ] 服务模式保存 web-owned 配置可同步
- [ ] Normal 模式同步仍正常
- [ ] 端口转发切换 tcp/udp 后配置序列化正确
- [ ] 初始节点切换协议时默认端口按协议表更新（tcp↔udp 等同默认端口保持；切到不同默认端口时更新）
- [ ] 导入含未知协议的 URL 仍可选中并保存
- [ ] 加密/压缩/ACL chain type 下拉外观一致、浮层不透明
- [ ] GUI 与 ET-Gui 服务成对安装/升级

---

## 4. 不在本次范围

- NSIS installer-hooks 加固（同分支其它改动，另审）
- Console 侧 managed config 协议变更
- 推送远程（需明确确认后再 push）
