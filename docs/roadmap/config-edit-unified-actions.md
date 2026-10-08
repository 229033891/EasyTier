# 配置编辑统一操作栏（保存 / 放弃更改 / 运行网络）

## Status

- Status: **Done**（2026-10-08 已落地）
- 日期：2026-10-08
- 背景：停止态编辑与运行态编辑共用同一 `Config` 表单，但底部操作、保存可见性、取消语义三处不一致；运行态点“运行网络”无确认直接重建实例断流。
- 相关代码：`easytier-web/frontend-lib/src/components/RemoteManagement.vue`、`src/modules/config-dirty.ts`；测试 `tests/config-dirty.spec.ts`、`tests/remote-management-config.spec.ts`
- 相关 Current：[`../current/desktop-gui-and-config-server.md`](../current/desktop-gui-and-config-server.md) §4（配置页开关约定）
- 索引：[`../README.md`](../README.md)

---

## 落地摘要

| 项 | 行为 |
|----|------|
| 底部栏 | `[放弃更改] [保存] … \| [启动 / 运行网络 / 停用]`；工具栏保存已下沉 |
| 保存 | 配置面板常驻底部；只读 / 无 meta 时 disabled + tooltip |
| 放弃更改 | 脏 → 重拉已保存配置；combined 运行态编辑且干净 → 退出编辑 |
| 脏标记 | `toBackendNetworkConfig` JSON，忽略 `dev_name`；标题栏「未保存」角标 |
| 切网 | Select 单向绑定 + 确认；内部 `setSelectedInstanceId` 不弹窗 |
| 再运行 | 运行中点「运行网络」确认重建断流；停止态一键 |

### 实现时相对原稿的修正

1. **放弃更改**不限 `isEditingNetwork`：停止态脏草稿也显示（`showConfigPanel && dirty`）。
2. **`remoteSave`** 只影响运行中 `run_network`；停止态路径仍总会 `save_config`。

---

## 1. 原状行为（已核实，归档）

| 场景 | 进入编辑 | 取消 | 应用 |
|------|----------|------|------|
| 停止态 | 从服务端重拉（`loadCurrentNetworkConfig`） | 无取消按钮 | `save_config` 落盘 + `update_network_instance_state(id, false)` 拉起；冷启动，无中断 |
| 运行态 | 同上（`editNetwork` 重拉） | `cancelEditNetwork` 只翻标志位，**草稿不清也不重载** | `run_network(cfg, remoteSave)` 作用于运行中实例，实例重建、短暂断流，**无确认** |

共用的坑（已修）：

- 编辑中途切换网络静默丢弃草稿 → Select 拆 `:modelValue` + `@update:modelValue` 异步确认。
- 保存只在顶部工具栏 → 下沉底部常驻。
- 停止态无放弃、运行态再跑无确认 → 已统一。

## 2. 已拍板语义

1. **运行仍=应用草稿**；`remoteSave` 权限分支保留（仅运行中试跑可不落盘）。
2. **保存两边常驻底部栏**，只读 disabled + tooltip；工具栏只留文件编辑/导入/导出。
3. **取消=真还原**（脏时重拉）；文案「放弃更改」。
4. **脏标记**：`toBackendNetworkConfig` JSON，剔 `dev_name`；加载/保存/运行后重拍快照。
5. **运行中再点运行网络要确认**；start / run 分文案。
6. 窄屏保存可退为图标按钮。

## 3. 验收

- [x] `vue-tsc -b` 通过
- [x] `vitest`：脏标记置位/清除、放弃更改回滚、切换网络确认、运行态重启确认（`config-dirty` + `remote-management-config`）
- [ ] 360px 截屏（手工）
