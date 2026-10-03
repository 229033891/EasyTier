# 节点间连接（现状）

## Status

- Status: **Current**
- 最近审阅：2026-10-03
- 范围：同一对 peer 之间的 `PeerConn` / 默认发送路径
- 规划中的多链路带宽聚合见：[`../roadmap/multi-link-bonding.md`](../roadmap/multi-link-bonding.md)
- 隧道 scheme 与伪装差距见：[`tunnels-and-transport.md`](./tunnels-and-transport.md)
- 索引：[`../README.md`](../README.md)

本文只描述 **代码今天做什么**。

---

## 1. 术语

| 术语 | 含义 |
|------|------|
| **Peer** | 对端节点（按 `peer_id`） |
| **PeerConn** | 一条具体底层隧道（TCP / UDP / QUIC / WG / ring 等），绑定一条 `Tunnel` |
| **default_conn** | 该 Peer 上缓存的「当前用于发送」的那条 PeerConn |

同一对 peer 之间 **可以同时存在多条 PeerConn**（多 listener、多协议、打洞 + 直连、重连残留等）。  
安全侧可跨多条 PeerConn 复用 Peer 级会话（见 `easytier/docs/peer_conn_secure_mode_v3.md`）。

---

## 2. 发送路径：多连接，单活动出口

实现：`easytier-core` → `peers::conn::Peer::select_conn` / `send_msg`。

1. 若已有缓存的 `default_conn` 且仍可用 → **所有 `send_msg` 走这一条**。  
2. 否则在存活连接中按延迟挑选一条（打洞连接在 ping 未确认前不会抢流量），写入 `default_conn`。  
3. **不会**按包或按流把流量分摊到多条 PeerConn 上。

因此：

- CLI / 状态里可能看到 `peer_conn_count > 1`。  
- **有效吞吐仍受当前默认那条隧道限制**。  
- 多连接今天的用途是 **路径冗余、选优、故障切换**，不是带宽聚合。

---

## 3. 与运营商「单连接限速」的关系

部分网络对单条 TCP/UDP 流有带宽上限。  
在现状模型下，即使两端之间有多条 PeerConn，数据面仍只使用 `default_conn`，**无法靠多连接叠加带宽**。

若需要该能力，见 Roadmap：[`../roadmap/multi-link-bonding.md`](../roadmap/multi-link-bonding.md)。

---

## 4. 验收对照（现状）

| 场景 | 期望（今天） |
|------|----------------|
| 两节点仅一条存活隧道 | 全部流量走该隧道 |
| 两节点多条存活隧道 | 状态可列出多条；发送仍只走 `default_conn`（通常为延迟更优者） |
| 默认隧道断开 | 重新 `select_conn`，切到另一条存活连接（若有） |
| 希望 N 条并行加带宽 | **未实现** |
