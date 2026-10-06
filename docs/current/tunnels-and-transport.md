# 隧道与传输（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-06
- 范围：Peer 隧道 scheme、中继/打洞与「看起来像什么」
- 相关：[`peer-connections.md`](./peer-connections.md)（多连接与发送路径）
- 规划中的抗识别 / 伪装见：[`../roadmap/traffic-camouflage.md`](../roadmap/traffic-camouflage.md)
- 索引：[`../README.md`](../README.md)

本文只描述 **代码今天提供的传输形态**，不承诺抗 DPI 或「像正常上网」。

---

## 1. 已支持的隧道 scheme（概念）

实现分散在 `easytier`（原生 tunnel / feature）与 `easytier-core` 的 connectivity 协议表。常见 scheme：

| Scheme | 底层大致形态 | 备注 |
|--------|----------------|------|
| `tcp` | 普通 TCP | 特征接近自定义长连接 |
| `udp` | UDP + EasyTier mux | 打洞/直连常用；易被 UDP 策略影响 |
| `ws` / `wss` | WebSocket（`wss` 带 TLS） | 常用示例端口含 **443** 与自定义端口（如 **8443**）；**443 不一定可用**。内层仍是 EasyTier 协议载荷，不是完整网站伪装 |
| `quic` | QUIC/UDP | 有 TLS 外观，指纹仍可能可识别 |
| `wg` | WireGuard 风格 UDP | 独立协议指纹 |
| `faketcp` | 以 TCP 报文形态承载（feature） | 偏「UDP 改头/抗干扰」类能力，**不是 HTTPS 网站伪装** |

具体能否编译/启用取决于 Cargo feature（如 `websocket`、`quic`、`faketcp`、`wireguard`）与宿主平台。

---

## 2. 连接建立方式（与伪装无关）

| 方式 | 今天做什么 |
|------|------------|
| 直连 / 手动 peer | 按配置 URL 建隧道 |
| UDP/TCP 打洞 | 尽量建立 P2P；失败则依赖其它路径 |
| 共享节点 / 中继 | 经第三方节点转发（foreign / relay 相关逻辑） |
| 配置服务器 | 节点连 `easytier-web` 等拿配置（可走 ws/wss） |

**打洞成功 ≠ 流量不可识别。** 运营商限制 P2P 时，即使连通，仍可能限速、阻断或干扰特定协议。

---

## 3. 与「混淆 / 伪装」的差距（已知）

今天 **没有** 下列能力（见 Roadmap）：

- Trojan / REALITY 级「对外就是某个网站的 TLS」  
- 浏览器 TLS 指纹（JA3/ALPN 等）仿造  
- 通用 pluggable transport（obfs4、meek 等）框架  
- 把 PeerConn 数据面强制封装进「只像浏览网页」的策略档

相对接近的现成用法（**端点完全自定义**；443 只是推荐之一）：

- 经可达中继，使用 **`wss://域名:443/et`**（合法证书）——当网络放行 443 时，端口与 TLS 层更像 Web；  
- 若 **443 被阻断或仅放行特定端口**，改用自定义端口/scheme，例如：  
  - `wss://relay.example.com:8443/et`  
  - `tcp://relay.example.com:5000`  
  - `tcp://10.0.0.2:8443`（内网中继）  
- 内层仍是 EasyTier，深度包检测或主动探测仍可能区分。

有序 peer URL 列表与档位回落见 [`peer-connections.md`](./peer-connections.md) §2.2。

---

## 4. 验收对照（现状）

| 场景 | 期望（今天） |
|------|----------------|
| 配置 `wss://host:443/et` | 可建立 WebSocket+TLS 隧道（feature 开启且网络放行 443 时） |
| 配置 `wss://host:8443/et` 或 `tcp://host:5000` | 同样可建隧道；**不要求**对方开放 443 |
| 环境阻断 443、放行自定义端口 | 只要 peer URL 指向可达端口即可通 |
| 仅靠 `udp` 打洞 | 可能成功；不保证抗限速/阻断 |
| 期望「完全像访问普通网站」 | **未实现** |
| `faketcp` | 改变传输封装形态；不等于 HTTPS 伪装 |
