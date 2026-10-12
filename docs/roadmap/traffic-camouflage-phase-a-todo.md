# Phase A TODO：中继 + wss:443 部署范式

## Status

- Status: **TODO / 暂不实现**（仅任务清单；动手前再确认）
- 创建：2026-10-13
- 父文档：[`traffic-camouflage.md`](./traffic-camouflage.md) §5 Phase A
- 现状隧道：[`../current/tunnels-and-transport.md`](../current/tunnels-and-transport.md)
- 工期估计：约 **3–7 天**（文档偏 3；加状态 UI 偏 5–7）
- **本阶段不做**：原生 PEM 加载、`transport_profile`、uTLS/ALPN、同端口假站、REALITY（属 B/C/D）

---

## 1. 目标与验收

| 目标 | 验收 |
|------|------|
| Ops 可按模板部署「中继 + TLS:443 + 客户端 peer URL」 | 书面步骤可复现；两边缘节点经中继组网 |
| 状态可读：scheme / TLS / 是否经中继 | GUI/控制台（及可选 CLI）不误导「已网站伪装」 |
| 证书叙事诚实 | 写清：合法证书 = **反代终结 TLS**；原生 wss 仍是自签 + skip-verify |

成功标准（产品）：按文档部署后，两节点经中继 `wss:443` 组网；状态能分清 scheme、TLS、是否经中继。

---

## 2. 现状结论（为何 A 值得做）

| 能力 | 代码 | 产品化 |
|------|------|--------|
| `wss://host/et` 默认端口 443 | 有 | 无专用 Ops |
| 显式 `listeners` / 多跳 relay | 有 | 无模板 |
| 原生 wss 证书 | 自签 + 客户端 skip-verify | **不等于**「合法 HTTPS」 |
| 状态 `tunnel_proto` | 有 | 无明确 TLS / 经中继徽章；「中继」文案易混 |

**结论**：A = Ops 模板 + 状态诚实，**不是**改指纹。

---

## 3. 推荐架构（写入 Ops 时以此为准）

```text
公网 :443 TLS (ACME)
        │
        ▼
 Nginx / Caddy（合法证书）
        │  Upgrade → WebSocket
        ▼
本机 EasyTier  listen  ws://127.0.0.1:11011
        │
客户端: -p wss://域名/path   （或 TOML [[peer]]）
```

注意：

- 控制台 Web（若同机）与 peer 隧道 **勿盲目抢 443**；模板须规定反代分流或分端口。
- path（如 `/et`）仅降简单扫描；服务端当前 **不强制** 校验 path——文档勿夸大。
- `mapped_listeners`：公网 URL ≠ bind 时使用。

---

## 4. 任务清单

### A1 — Ops 手册与模板（主交付）

- [ ] 新建 `docs/ops/wss-relay-443.md`（或等价路径）
  - [ ] 中继节点：本机 `ws` 监听 vs 反代 TLS
  - [ ] ACME / Nginx 或 Caddy 示例（Upgrade 头、超时、WebSocket）
  - [ ] 防火墙：443（公网）与本机 11011/11012 说明
  - [ ] 客户端 peer URL 模板（CLI `-p` 与 TOML `[[peer]]`）
  - [ ] 可选 path、`mapped_listeners`、`:443` 特权 / `CAP_NET_BIND_SERVICE`
  - [ ] 明确「原生 wss 自签」与「反代合法证书」两种模式及适用场景
- [ ] 示例配置片段（中继 TOML + 两客户端 TOML）
- [ ] 索引链接：`docs/README.md`、Current tunnels、README / README_CN、必要时 `deploy-install.md`
- [ ] 文案禁令：不写「隐身 / 网站级伪装」；只用 Current 已交付措辞

### A2 — 状态可读（少改核心；UI / 可选 CLI）

- [ ] TLS 展示：可由 scheme 推导（`wss` / `quic` ≈ TLS）；**不**必先改 proto
- [ ] 「经中继」：复用 `cost > 1` / 路由下一跳；与 `avoid_relay_data` 的「中继」标签解耦或改名
- [ ] 短帮助 / tooltip：非网站级伪装（对齐 Current）
- [ ] i18n：`easytier-web/frontend-lib` 中英文
- [ ] （可选）CLI `easytier-cli peer` 输出与上述语义一致

主要触及（实现时）：

- `easytier-web/frontend-lib/src/components/Status.vue`
- `easytier-web/frontend-lib/src/modules/statusDisplay.ts`（或等价）
- locales `cn.yaml` / `en.yaml`
- （可选）`easytier/src/easytier-cli/`

### A3 — 验收清单

- [ ] 文档内嵌冒烟步骤：
  1. 中继按模板起（反代 + ws）
  2. 两客户端 `-p wss://域名/...`
  3. `easytier-cli peer`：到中继为 `wss`；两边缘之间为 `relay` / 多跳
- [ ] （可选）脚本化 smoke；非本阶段必须

---

## 5. 明确不做（防范围膨胀）

| 项 | 归属 |
|----|------|
| 原生 PEM / ACME 进 `websocket.rs` | 超出 A；单独立项 |
| `transport_profile = web_wss` 产品预设 | Phase B |
| uTLS / JA3 / ALPN 仿浏览器 | Phase C |
| 同端口静态站分流、REALITY | Phase C/D |
| 旋转 padding / 时序整形 / 「动态指纹」 | 不在 camouflage 主线优先 |

---

## 6. 风险与缓解

| 风险 | 缓解 |
|------|------|
| 文档写「合法证书」但代码 skip-verify | Ops 强制走「反代模式」叙事；并列「仅连通自签模式」 |
| 443 与控制台 Nginx 冲突 | 模板规定同一反代分流或分端口 |
| path 被当成安全特性 | 标明「可选、非强制」 |
| UI「中继」一词多义 | A2 改名 / 分徽章 |
| i18n 承诺 Roadmap 能力 | 只描述动手当日已有行为 |

---

## 7. 建议排期（动手时）

| 顺序 | 内容 | 约略 |
|------|------|------|
| 1 | A1 文档 + 模板 + 索引 | 2–3 天 |
| 2 | A2 状态 / 文案 | 1–2 天 |
| 3 | A3 验收勾选 | 0.5–1 天 |

依赖：无代码阻塞；A2 可不阻塞 A1 合并。

---

## 8. 决策记录

| 日期 | 决策 |
|------|------|
| 2026-10-13 | 先落本文 TODO，**暂不实现** |
| 2026-10-13 | A 采用「反代合法证书」为主交付路径；原生 PEM 不进 A |
| 2026-10-13 | 不做「动态指纹混淆」包装；产品措辞为「中继 + wss/443 连通保底」 |
