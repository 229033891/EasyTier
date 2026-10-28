# 中继转发：每目的 peer 有界出口队列

## Status

- Status: **Roadmap**（代码已落地，待现网验收）
- 最近审阅：2026-10-28
- 绑定实现：`easytier-core/src/peers/forward_egress.rs`、`PeerPacketRouter::handle_packet`
- 相关：[`connection-stability-todo.md`](./connection-stability-todo.md)（P-AUDIT.6 明确不做环满背压；本项**不**推翻该结论）

---

## 1. 问题

`PeerPacketRouter` 是单条串行循环。中转包在 router 内直接 `await PeerConn::send_msg`（出口满时最多等 100ms）时，**所有** peer 的入向处理一起卡住。

不对称拓扑（`A→R` 快、`R→B` 慢）下表现为：带宽上不去、端到端 TCP RTO 毛刺、无关目的 C 也被拖累。

中继本身几乎不做加解密（见 Current 审计与代码门控）；瓶颈在**共享 router HOL + 两段链路无按目的隔离的出口队列**。

---

## 2. 方案（已实现）

每 `dst_peer_id` 一条有界 mpsc（容量 [`FORWARD_EGRESS_QUEUE_CAP`](../../easytier-core/src/peers/forward_egress.rs) = 64）：

1. Router 只做 `try_enqueue`（非阻塞）；满则 `BufferFull` + `data_plane_tunnel_buffer_full`，**不** await。
2. 每目的独立 drain 任务调用既有 `send_msg_internal`（仍可 100ms 有界等待）；lane 空闲 60s 回收，避免实例重建后 pin 住 `PeerMap`。
3. 慢目的 B 不阻塞目的 C。

**不是** P-AUDIT.6 的「UDP 环改阻塞背压」——环满语义不变；只是把共享 router 从出口等待中拆开。

顺带：`RelayPeerMap::send_msg` / `send_msg_to_next_hop` 增加 `next_hop_hint`，`send_msg_internal` 复用已解析的网关，去掉每包重复的 `get_gateway_peer_id`。

---

## 3. 验收（待现网）

- [ ] 三节点 mesh，强制中转：`cost>1`；复制大文件时中继上 `data_plane_tunnel_buffer_full` 行为可解释（满 lane / 满 PeerConn）。
- [ ] 故意压慢 `R→B`：发往 C 的流量不应出现与 B 同量级的 HOL 卡顿。
- [ ] 突发型抖动明显减轻（lane 吸收约 64 包突发）；持续超载时的 RTO 台阶仍存在——那是端到端拥塞控制对丢包的正常反应，本项不消除（lane + 100ms 有界等待只是把丢弃从 router 挪到 drain）。
- [ ] `cargo test -p easytier-core --lib peers::forward_egress` / `peers::relay_peer_map` 绿。

---

## 4. 非目标

- 不做全局入口→出口真背压链（P-AUDIT.6）。
- 不做同网络中转 per-peer BPS 配额（另项）。
- 不把 SessionKey 每包分配优化绑进本项。

---

## Changelog

| 日期 | 说明 |
|------|------|
| 2026-10-28 | 立项并落地 per-dst forward egress + next_hop_hint；待现网勾验收 |
| 2026-10-28 | 修 lane 空闲回收；本机路径透传 next_hop_hint；补 registry 单测；验收口径区分突发 vs 持续超载 |
