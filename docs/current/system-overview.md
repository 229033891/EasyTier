# EasyTier 系统总览（Agent 入口）

## Status

- Status: **Current**
- 最近审阅：2026-10-06
- 读者：实现复查 / 其他 agent（先读本文，再下钻专题）
- 索引：[`../README.md`](../README.md)
- 命名与场景细节：[`product-map.md`](./product-map.md)；crate 边界：[`architecture.md`](./architecture.md)

本文是**系统怎么串起来**的地图：一张总图 + 阅读顺序 + 代码锚点。不复述产品命名长表（product-map）或 host/core 边界细则（architecture）。

---

## 1. 三件可交付物

```text
┌──────────────────┐   配置协议 / API    ┌────────────────────────────┐
│  easytier-web    │◄──────────────────►│  节点进程                    │
│  Console +       │                    │  easytier-core 二进制        │
│  config-server   │                    │  （easytier crate 编出）     │
└──────────────────┘                    └─────────────▲────────────────┘
                                                      │ 本机启停 / RPC
                                        ┌─────────────┴────────────────┐
                                        │  easytier-gui                 │
                                        │  Normal：同进程嵌节点          │
                                        │  Service：前台 ET + 后台 ET-Gui│
                                        └──────────────────────────────┘
```

| 正式名 | 干什么 | 深入文档 |
|--------|--------|----------|
| 节点 / `easytier-core` 二进制 | P2P VPN 数据面 | [`architecture.md`](./architecture.md)、[`traffic-steering.md`](./traffic-steering.md) |
| `easytier-gui` | 本机壳；可选 `ET-Gui` 服务 | [`desktop-gui-and-config-server.md`](./desktop-gui-and-config-server.md) |
| `easytier-web` | 集中配置 + 浏览器 UI | [`web-managed-config.md`](./web-managed-config.md) |
| 命名陷阱（core 库 vs 二进制） | — | [`product-map.md`](./product-map.md) |

---

## 2. 依赖与所有权（一句话）

```text
easytier-proto  ←  easytier-core（可移植策略）  ←  easytier（OS / TUN / 服务）
```

- **策略与选路**在 core；**真 socket / TUN / 路由表 / DNS bind**在 host（`easytier`）。
- 进程内可共享：Ring Tunnel、protected TCP ports、**config-server report registry**（每进程一份）。
- 实例状态不进进程全局对象。细则：[`architecture.md`](./architecture.md) § Process-scoped state。

---

## 3. 配置权威流（两条方向，勿混）

| 方向 | 含义 | Current 文档 |
|------|------|--------------|
| Console → 节点 | Full PUT / PATCH CAS，Session 收敛 | [`web-managed-config.md`](./web-managed-config.md) |
| 节点 / GUI → Console | web-owned 本地保存后 `ReportManagedNetworkConfig` | [`desktop-gui-and-config-server.md`](./desktop-gui-and-config-server.md) §2 |

桌面 **Service 模式**下，config-server `WebClient` 在 **`ET-Gui` 服务进程**；GUI 前台只 RPC。不要只查 GUI 进程内 `WEB_CLIENT`。

---

## 4. 流量路径（审路由时）

| 层 | 问题 | 文档 |
|----|------|------|
| L2 | OS 会不会把包送进 TUN | [`traffic-steering.md`](./traffic-steering.md) §3（D+：对端 `/0` 默认不装；本机 `exit_nodes` 可装） |
| L3 | 进 TUN 后交给哪个 peer | 同文档 §2 |
| DNS / underlay | 装 `/0` 后如何避开黑洞 | 同文档 §3；规划缺口见 Roadmap |

---

## 5. 推荐阅读顺序（按任务）

| 任务 | 顺序 |
|------|------|
| 改 core 边界 / feature | `architecture.md` → Validation |
| 改桌面服务模式 / 保存同步 | `desktop-gui-and-config-server.md` → archive 复查 checklist |
| 改 Console managed config | `web-managed-config.md` → `roadmap/web-evolution.md`（未实现侧） |
| 改出口 / 默认路由 | `traffic-steering.md` → `roadmap/domain-proxy.md` / vNext（未实现侧） |
| 改 MagicDNS / DnsConfig | `magic-dns.md` → `magic-dns-manual-wiring.md` → `roadmap/dns-policy.md`（缺口） |
| 搞清产品叫什么 | `product-map.md`（勿与本文总图重复扩写） |
| 历史验证记录 | `../archive/`（**不**当 Current SoT） |

---

## 6. 与代码对照的最短锚点

| 主题 | 路径 |
|------|------|
| Report registry | `easytier-core/src/management/full/config_server_client.rs` |
| GUI sync | `easytier-gui/src-tauri/src/lib.rs` → `sync_web_owned_network_config` |
| 服务名 | `easytier-gui/src-tauri/src/service.rs` → `"ET-Gui"` + `--daemon` |
| L3 选路 | `PeerOutboundPacketRouter::get_msg_dst_peer_ipv4` |
| L2 / underlay | `easytier/src/instance/virtual_nic.rs` + `resolve_proxy_cidrs` |
| 桌面 underlay DNS | `easytier/src/common/dns.rs` → `RuntimeDnsResolver` |
| MagicDNS | `easytier/src/instance/dns_server/`；总览 [`magic-dns.md`](./magic-dns.md) |

---

## 7. 维护

行为变更时：先改对应 Current 专题，再改本文链接/一句话；Roadmap 文不得冒充已交付。
