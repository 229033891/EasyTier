# 配置页 / 运行页职责拆分

## Status

- Status: **Roadmap**（主体已落地；仅剩可选「保存并运行」次要入口）
- 最近审阅：2026-10-09
- **现状 SoT**：[`../current/desktop-gui-and-config-server.md`](../current/desktop-gui-and-config-server.md) §6
- 前置（已落地）：配置编辑统一操作栏（2026-10-08；原文已归档，相关人工回归项见 [`../current/system-overview.md`](../current/system-overview.md) §7）
- 相关代码：`easytier-web/frontend-lib/src/components/RemoteManagement.vue`；Web `DeviceManagement.vue`（`?mode=status|config`）；GUI `easytier-gui` combined
- 索引：[`../README.md`](../README.md)

---

## 1. 已落地（2026-10-08）

| 项 | 状态 |
|----|------|
| GUI combined 运行中：配置页无「运行网络」；运行页启停 +「节点配置」 | **已做** |
| GUI 停止态：默认运行空态（启动 + 节点配置） | **已做** |
| Web：`mode=config` 不挂启停主按钮；`mode=status` 保持启停 | **已做** |
| 配置页脏：取消编辑 + 保存；干净：返回运行页 | **已做** |

---

## 2. 剩余（可选）

- [ ] 配置页次要「**保存并运行 / 应用并重启**」入口（运行中须确认断流）
- [ ] 双端验收截屏归档（运行中 / 停止 / 配置脏 / 配置干净）

---

## 3. 非目标

- 不强制 GUI 拆成两个 URL
- 不做跨端「当前页签」同步
- 产品帮助文案只描述 Current §7 行为
