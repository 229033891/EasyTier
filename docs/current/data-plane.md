# Data plane（现状）

## Status

- Status: **Current**
- Last reviewed: 2026-10-03
- Index: [`../README.md`](../README.md)
- 历史实现计划（Archive）：[`../archive/data-plane-runtime-plan.md`](../archive/data-plane-runtime-plan.md)

## 已落地

| 能力 | 位置 / 说明 |
|------|-------------|
| `DataPlaneRuntime` | `easytier-core`：路由策略、smoltcp 流、资源策略 |
| SOCKS5 / 端口转发 | Gateway Adapter，叠在 DataPlaneRuntime 之上 |
| Go 标准 `net` | `easytier-go` 经 ABI 暴露 Overlay/smoltcp 路径 |
| FFI completion queue | `easytier-contrib/easytier-ffi` 实例级完成流（非旧 per-op 轮询） |
| 架构边界 | 见 [`architecture.md`](./architecture.md) |

## 已知缺口 / 排除项

- **不**把 `kcp-sys` 移植到 `wasm32-wasip1`；WASI 组合不装 KCP
- KCP 不通过 FFI v2 / Go data-plane session 暴露
- 以仓库代码与测试为准；详表与阶段记录见 Archive 长文中的 Definition of done

## 相关文档

- [`architecture.md`](./architecture.md) — crate / 依赖方向
- [`socket-protection.md`](./socket-protection.md) — Host VPN-bypass 契约
- [`../archive/data-plane-runtime-plan.md`](../archive/data-plane-runtime-plan.md) — 原规划全文（动机、阶段、验收清单）
