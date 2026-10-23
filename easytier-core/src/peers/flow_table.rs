//! Per-tunnel IP flow accounting.
//!
//! Design: [`docs/roadmap/tunnel-ip-flow-observability.md`](../../../../docs/roadmap/tunnel-ip-flow-observability.md).
//!
//! # Why this lives in the peer manager and not in the tunnel
//!
//! The inner IP header is encrypted **before** a packet reaches a tunnel
//! ([`PeerOutboundPacketRouter`] compresses + encrypts in
//! `try_compress_and_encrypt`, the tunnel filter only ever sees ciphertext),
//! so a tunnel-level counter cannot tell *which* destination a packet carries.
//! Accounting therefore happens where the plaintext is still available:
//!
//! * **TX** — the outbound router knows the final destination IP; it is
//!   attached to the packet as a **local-only** field
//!   ([`crate::packet::ZCPacket::flow_meta`]) and booked by
//!   [`crate::peers::conn::peer::Peer::send_msg`] once the concrete tunnel has
//!   been selected (bond hash / `default_conn` / `select_conn`).
//! * **RX** — the peer manager knows the ingress tunnel
//!   ([`crate::peers::PeerPacketIngress`]) and decrypts the payload before
//!   handing it to the host stack, so the source IP is available there.
//!
//! # Byte accounting口径
//!
//! Bytes are counted as **inner IP payload bytes** (the length of the IP
//! packet before compression/encryption), *not* the wire bytes. This is a
//! different口径 than [`PeerConnStats`](crate::proto::core_peer::peer::PeerConnStats),
//! which counts link bytes at the tunnel filter. The UI must label the two
//! columns differently.

use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use dashmap::{DashMap, mapref::entry::Entry};

use crate::config::PeerId;
use crate::peers::conn::peer_conn::PeerConnId;

/// Direction of a recorded flow, relative to this node.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FlowDirection {
    /// Locally originated traffic: the recorded IP is the **destination**.
    Tx,
    /// Traffic received by this node: the recorded IP is the **source**.
    Rx,
}

/// Identity of one recorded flow: one tunnel × one IP × one direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct FlowKey {
    /// The tunnel (PeerConn) that carried the traffic.
    pub conn_id: PeerConnId,
    /// The peer on the other end of that tunnel.
    pub peer_id: PeerId,
    /// Destination IP (Tx) or source IP (Rx).
    pub ip: IpAddr,
    /// Direction relative to this node.
    pub dir: FlowDirection,
}

/// Counters of one recorded flow.
#[derive(Debug, Default)]
pub struct FlowEntry {
    /// Inner IP bytes observed since the flow was first seen.
    pub total_bytes: AtomicU64,
    /// Packets observed since the flow was first seen.
    pub total_packets: AtomicU64,
    /// Inner IP bytes observed inside the *current* rate window.
    pub window_bytes: AtomicU64,
    /// Inner IP bytes observed inside the *previous* (closed) rate window.
    ///
    /// The window is closed by [`FlowTable::rotate_rate_window`] at a fixed
    /// cadence, so the reported rate does not depend on who polls the table.
    pub rate_bytes: AtomicU64,
    /// Unix millis of the first observation.
    pub first_seen_ms: AtomicU64,
    /// Unix millis of the last observation.
    pub last_seen_ms: AtomicU64,
    /// True when at least one *sampled* packet contributed to this entry, i.e.
    /// the byte counters are an estimate.
    pub sampled: AtomicBool,
}

/// Tunables of the flow table.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct FlowTableConfig {
    /// Master switch. When false the table records nothing and costs nothing
    /// on the data path (one atomic load per packet).
    pub enabled: bool,
    /// Keep 1 out of N packets (1 = keep everything).
    pub sample_rate: u32,
    /// Maximum number of distinct IPs kept per tunnel.
    pub max_per_conn: usize,
    /// Maximum number of entries kept in the whole table.
    pub max_total: usize,
    /// Entries without traffic for this long are dropped.
    pub ttl: Duration,
    /// Prefix length used when a view masks IPv4 addresses.
    pub mask_prefix_len_v4: u8,
    /// Prefix length used when a view masks IPv6 addresses.
    pub mask_prefix_len_v6: u8,
    /// Whether this node may answer EXACT IP requests (decision D13).
    ///
    /// The control plane must *also* ask for EXACT explicitly; when either
    /// side says no, the node answers with masked (prefix-aggregated) data.
    pub allow_exact_ip: bool,
}

impl Default for FlowTableConfig {
    fn default() -> Self {
        Self {
            // Default OFF: low-end devices (MIPS routers, ...) must be able to
            // run with zero flow-accounting overhead.
            enabled: false,
            sample_rate: 1,
            max_per_conn: 256,
            max_total: 8192,
            ttl: Duration::from_secs(600),
            mask_prefix_len_v4: 24,
            mask_prefix_len_v6: 64,
            allow_exact_ip: false,
        }
    }
}

/// How a caller wants to see the recorded flows.
#[derive(Clone, Copy, Debug)]
pub enum FlowView {
    /// Exact IPs.
    Exact,
    /// Prefix-aggregated IPs (the control-plane default, see decision D8).
    Masked {
        v4_prefix_len: u8,
        v6_prefix_len: u8,
    },
}

/// One row of a [`FlowTable::snapshot`].
#[derive(Clone, Debug)]
pub struct FlowSnapshot {
    pub conn_id: PeerConnId,
    pub peer_id: PeerId,
    /// Exact IP, or the aggregated prefix in a masked view.
    pub ip: IpAddr,
    /// 0 for an exact IP, otherwise the prefix length used for aggregation.
    pub prefix_len: u8,
    pub dir: FlowDirection,
    /// Inner IP bytes (scaled up when the flow was sampled).
    pub bytes: u64,
    /// Packets (scaled up when the flow was sampled).
    pub packets: u64,
    /// Bytes per second measured over the last closed rate window.
    pub rate_bytes_per_sec: u64,
    /// Number of distinct IPs merged into this row (masked views only).
    pub flow_count: u32,
    /// True when the counters are an estimate (sampling was active).
    pub sampled: bool,
    /// Unix millis of the first observation.
    pub first_seen_ms: u64,
    /// Unix millis of the last observation.
    pub last_seen_ms: u64,
}

/// Coarse aggregate of the whole table, used by the periodic `CORE::FLOW` log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowSummary {
    /// Number of live entries (distinct conn/peer/ip/direction keys).
    pub entries: usize,
    /// Number of tunnels with at least one entry.
    pub conns: usize,
    /// Sum of inner IP bytes sent by this node.
    pub tx_bytes: u64,
    /// Sum of inner IP bytes received by this node.
    pub rx_bytes: u64,
    /// Sum of the last closed rate windows.
    pub rate_bytes_per_sec: u64,
}

/// In-memory table of per-tunnel IP flows.
pub struct FlowTable {
    flows: DashMap<FlowKey, Arc<FlowEntry>>,
    /// Number of live entries per tunnel, so the per-tunnel cap can be
    /// enforced in O(1) instead of scanning the whole table per packet.
    per_conn_counts: DashMap<PeerConnId, usize>,
    /// Master switch (hot path: one relaxed atomic load).
    enabled: AtomicBool,
    sample_rate: AtomicU32,
    max_per_conn: AtomicUsize,
    max_total: AtomicUsize,
    ttl_ms: AtomicU64,
    mask_prefix_len_v4: AtomicU8,
    mask_prefix_len_v6: AtomicU8,
    /// Whether EXACT IP views may be served (decision D13).
    allow_exact_ip: AtomicBool,
    /// Rolling counter used to pick sampled packets.
    sample_cursor: AtomicU64,
}

impl FlowTable {
    pub fn new(cfg: FlowTableConfig) -> Self {
        let table = Self {
            flows: DashMap::new(),
            per_conn_counts: DashMap::new(),
            enabled: AtomicBool::new(false),
            sample_rate: AtomicU32::new(1),
            max_per_conn: AtomicUsize::new(0),
            max_total: AtomicUsize::new(0),
            ttl_ms: AtomicU64::new(0),
            mask_prefix_len_v4: AtomicU8::new(0),
            mask_prefix_len_v6: AtomicU8::new(0),
            allow_exact_ip: AtomicBool::new(false),
            sample_cursor: AtomicU64::new(0),
        };
        table.configure(cfg);
        table
    }

    /// Apply a configuration. **Construction-time only.**
    ///
    /// The hot path only reads plain atomics, so no lock is involved.
    ///
    /// Deliberately private (F-3): the amplification factor applied on read is
    /// the *current* `sample_rate`, while `FlowEntry::sampled` is a sticky
    /// per-entry flag. Retuning `sample_rate` on a live table would multiply
    /// counters that were accumulated at a different rate — 1 GB recorded at
    /// rate 1 reads back as 8 GB after switching to rate 8. Anyone needing new
    /// settings must build a new table, which is what a network-instance
    /// restart does; changing `[flow_stats]` therefore requires restarting the
    /// instance.
    fn configure(&self, cfg: FlowTableConfig) {
        self.enabled.store(cfg.enabled, Ordering::Relaxed);
        self.sample_rate
            .store(cfg.sample_rate.max(1), Ordering::Relaxed);
        self.max_per_conn.store(cfg.max_per_conn, Ordering::Relaxed);
        self.max_total.store(cfg.max_total, Ordering::Relaxed);
        self.ttl_ms
            .store(cfg.ttl.as_millis() as u64, Ordering::Relaxed);
        self.mask_prefix_len_v4
            .store(cfg.mask_prefix_len_v4, Ordering::Relaxed);
        self.mask_prefix_len_v6
            .store(cfg.mask_prefix_len_v6, Ordering::Relaxed);
        self.allow_exact_ip
            .store(cfg.allow_exact_ip, Ordering::Relaxed);
        // Counters are only meaningful together with the rate they were
        // recorded at, so a reconfiguration drops them. A no-op today (the sole
        // caller is `new`), but it keeps the invariant if `configure` is ever
        // made public again.
        self.flows.clear();
        self.per_conn_counts.clear();
        self.sample_cursor.store(0, Ordering::Relaxed);
    }

    /// Snapshot of the current configuration.
    pub fn config(&self) -> FlowTableConfig {
        FlowTableConfig {
            enabled: self.is_enabled(),
            sample_rate: self.sample_rate.load(Ordering::Relaxed),
            max_per_conn: self.max_per_conn.load(Ordering::Relaxed),
            max_total: self.max_total.load(Ordering::Relaxed),
            ttl: Duration::from_millis(self.ttl_ms.load(Ordering::Relaxed)),
            mask_prefix_len_v4: self.mask_prefix_len_v4.load(Ordering::Relaxed),
            mask_prefix_len_v6: self.mask_prefix_len_v6.load(Ordering::Relaxed),
            allow_exact_ip: self.allow_exact_ip.load(Ordering::Relaxed),
        }
    }

    /// Whether accounting is active. Hot path: one relaxed atomic load.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Number of live entries (all directions, all tunnels).
    pub fn len(&self) -> usize {
        self.flows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.flows.is_empty()
    }

    /// Book one packet.
    ///
    /// `bytes` is the **inner IP** length. Returns `true` when the packet was
    /// actually booked (i.e. not dropped by sampling).
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        conn_id: PeerConnId,
        peer_id: PeerId,
        ip: IpAddr,
        dir: FlowDirection,
        bytes: u64,
        packets: u64,
        now_ms: u64,
    ) -> bool {
        if !self.is_enabled() {
            return false;
        }

        let sample_rate = self.sample_rate.load(Ordering::Relaxed);
        let sampled = if sample_rate > 1 {
            let cursor = self.sample_cursor.fetch_add(1, Ordering::Relaxed);
            if !cursor.is_multiple_of(u64::from(sample_rate)) {
                return false;
            }
            true
        } else {
            false
        };

        let key = FlowKey {
            conn_id,
            peer_id,
            ip,
            dir,
        };

        let mut inserted = false;
        let entry = match self.flows.entry(key) {
            Entry::Occupied(e) => e.get().clone(),
            Entry::Vacant(e) => {
                inserted = true;
                let arc = Arc::new(FlowEntry {
                    first_seen_ms: AtomicU64::new(now_ms),
                    ..Default::default()
                });
                e.insert(arc.clone());
                arc
            }
        };

        if sampled {
            entry.sampled.store(true, Ordering::Relaxed);
        }
        entry.total_bytes.fetch_add(bytes, Ordering::Relaxed);
        entry.total_packets.fetch_add(packets, Ordering::Relaxed);
        entry.window_bytes.fetch_add(bytes, Ordering::Relaxed);
        entry.last_seen_ms.store(now_ms, Ordering::Relaxed);

        if inserted {
            self.enforce_limits(conn_id, &key);
        }
        true
    }

    /// Book a received packet on the tunnel it arrived on.
    ///
    /// `ingress` is `Some((conn_id, peer_id))` for packets that came in on a
    /// peer connection and `None` for locally injected packets. The source IP
    /// of the inner packet is extracted from the (decrypted) payload.
    pub fn record_rx(
        &self,
        ingress: Option<(PeerConnId, PeerId)>,
        payload: &[u8],
        now_ms: u64,
    ) -> bool {
        let Some((conn_id, peer_id)) = ingress else {
            return false;
        };
        let Some((src_ip, _dst_ip)) = parse_inner_ips(payload) else {
            return false;
        };
        self.record(
            conn_id,
            peer_id,
            src_ip,
            FlowDirection::Rx,
            payload.len() as u64,
            1,
            now_ms,
        )
    }

    /// Aggregate counters for the periodic `CORE::FLOW` log line (decision D2).
    ///
    /// Deliberately allocation-light: a single pass over the table, no
    /// per-prefix merging. Returns all-zero when accounting is disabled.
    pub fn summary(&self) -> FlowSummary {
        if !self.is_enabled() {
            return FlowSummary::default();
        }
        let sample_rate = u64::from(self.sample_rate.load(Ordering::Relaxed).max(1));
        let mut summary = FlowSummary::default();
        let mut conns = HashSet::new();
        for entry in self.flows.iter() {
            let key = entry.key();
            summary.entries += 1;
            conns.insert(key.conn_id);
            // 采样开启时按 1/N 放大，与 snapshot 的估算口径保持一致（§4.5）。
            let scale = if entry.sampled.load(Ordering::Relaxed) {
                sample_rate
            } else {
                1
            };
            let bytes = entry.total_bytes.load(Ordering::Relaxed) * scale;
            match key.dir {
                FlowDirection::Tx => summary.tx_bytes += bytes,
                FlowDirection::Rx => summary.rx_bytes += bytes,
            }
            summary.rate_bytes_per_sec += entry.rate_bytes.load(Ordering::Relaxed) * scale;
        }
        summary.conns = conns.len();
        summary
    }

    /// Close the current rate window and start a new one.
    ///
    /// The reported rate is therefore always "bytes seen during the last
    /// closed window", independent of the polling cadence of any consumer.
    pub fn rotate_rate_window(&self) {
        for entry in self.flows.iter() {
            let closed = entry.window_bytes.swap(0, Ordering::Relaxed);
            entry.rate_bytes.store(closed, Ordering::Relaxed);
        }
    }

    /// Drop expired entries and enforce the global cap.
    ///
    /// ## `per_conn_counts` 的竞态（§17.6 R2）
    ///
    /// `per_conn_counts` 只是**上限判定的加速索引**，不是权威计数：权威值永远是
    /// `flows` 里实际的键数。热路径 `enforce_limits` 会对它做 `+= 1`，而这里要按
    /// `flows` 重算。旧实现用「先 `clear()` 再逐个累加」，中间那段窗口里热路径的
    /// `+= 1` 会写在**已清空的表**上，随后被重新累加覆盖 ⇒ 计数永久偏小，上限越来
    /// 越松（且没有任何地方会纠正）。
    ///
    /// 现做法：先在本地算出完整的 `next` 映射，再**逐键覆盖 + 删除多余键**，整个过程
    /// 不做 `clear()`。即便期间有热路径插入新流，也只是让计数短暂偏小一个，下一轮
    /// GC（30s）就会收敛；而**超出**上限的偏差可以靠 `enforce_limits` 的兜底扫描
    /// 纠正。功能上界仍是「近似但会收敛」，不会单调漂移到无界。
    ///
    /// 想彻底消除近似，只有把 `flows` 与计数放进同一把锁——那会让每包付锁代价，
    /// 与 D5「关闭时零开销、开启时低开销」冲突，故不做。
    pub fn gc(&self, now_ms: u64) {
        let ttl_ms = self.ttl_ms.load(Ordering::Relaxed);
        self.flows.retain(|_, entry| {
            now_ms.saturating_sub(entry.last_seen_ms.load(Ordering::Relaxed)) <= ttl_ms
        });

        // Rebuild the per-tunnel index without ever leaving it empty.
        let mut next: HashMap<PeerConnId, usize> = HashMap::new();
        for entry in self.flows.iter() {
            *next.entry(entry.key().conn_id).or_insert(0) += 1;
        }
        // Drop tunnels that no longer have entries.
        self.per_conn_counts
            .retain(|conn_id, _| next.contains_key(conn_id));
        // Overwrite the survivors with the freshly counted values.
        for (conn_id, count) in next {
            self.per_conn_counts.insert(conn_id, count);
        }

        // Keep the map from keeping the memory of a burst of dead entries.
        let max_total = self.max_total.load(Ordering::Relaxed);
        if self.flows.capacity() > max_total.saturating_mul(2) {
            self.flows.shrink_to_fit();
        }
    }

    /// Enforce the per-tunnel and global caps.
    ///
    /// The per-tunnel check is O(1) thanks to `per_conn_counts`; only when a
    /// cap is actually exceeded do we scan the table to evict the oldest
    /// entries of that tunnel.
    ///
    /// `just_inserted` is the key that triggered this call. It is **never**
    /// evicted here (§17.6 R3): the entry was only just written, so its
    /// `last_seen_ms` is the same as (or newer than) any other candidate, and
    /// picking it would delete the flow we just started accounting for — the
    /// next packet re-creates it, and a burst of new destinations then thrashes
    /// the table instead of converging to the cap.
    ///
    /// Over-limit eviction removes **all** the excess, not a single entry, so a
    /// burst drains in one pass instead of one entry per insertion.
    fn enforce_limits(&self, conn_id: PeerConnId, just_inserted: &FlowKey) {
        let max_per_conn = self.max_per_conn.load(Ordering::Relaxed);
        let over_per_conn = {
            let mut count = self.per_conn_counts.entry(conn_id).or_insert(0);
            *count += 1;
            *count > max_per_conn
        };

        if over_per_conn {
            // Over the per-tunnel cap: evict the least recently seen entries of
            // this tunnel, oldest first, skipping the one just inserted.
            let mut candidates: Vec<(FlowKey, u64)> = self
                .flows
                .iter()
                .filter(|e| e.key().conn_id == conn_id && e.key() != just_inserted)
                .map(|e| (*e.key(), e.last_seen_ms.load(Ordering::Relaxed)))
                .collect();
            candidates.sort_by_key(|(_, ts)| *ts);
            // How many to drop: enough to bring the tunnel back to the cap.
            let excess = candidates
                .len()
                .saturating_sub(max_per_conn.saturating_sub(1));
            for (key, _) in candidates.into_iter().take(excess) {
                self.remove_entry(&key);
            }
        }

        // Global cap. Same rule: never drop the entry we just inserted.
        let max_total = self.max_total.load(Ordering::Relaxed);
        let excess = self.flows.len().saturating_sub(max_total);
        if excess > 0 {
            let mut candidates: Vec<(FlowKey, u64)> = self
                .flows
                .iter()
                .filter(|e| e.key() != just_inserted)
                .map(|e| (*e.key(), e.last_seen_ms.load(Ordering::Relaxed)))
                .collect();
            candidates.sort_by_key(|(_, ts)| *ts);
            for (key, _) in candidates.into_iter().take(excess) {
                self.remove_entry(&key);
            }
        }
    }

    /// Remove one entry and keep the per-tunnel counter in sync.
    fn remove_entry(&self, key: &FlowKey) {
        if self.flows.remove(key).is_some()
            && let Some(mut count) = self.per_conn_counts.get_mut(&key.conn_id)
        {
            *count = count.value().saturating_sub(1);
        }
    }

    /// Build a view of the recorded flows.
    ///
    /// * masked views merge the entries that share a prefix and report how many
    ///   distinct IPs were merged (`flow_count`), so a consumer can still tell
    ///   "one host" from "fourteen hosts in this /24";
    /// * the result is sorted by bytes (descending) and truncated to `top_n`.
    pub fn snapshot(&self, view: FlowView, top_n: usize) -> Vec<FlowSnapshot> {
        self.build_snapshot(view, top_n, None)
    }

    /// Build a view of the flows of a single tunnel.
    pub fn snapshot_for_conn(
        &self,
        conn_id: PeerConnId,
        view: FlowView,
        top_n: usize,
    ) -> Vec<FlowSnapshot> {
        self.build_snapshot(view, top_n, Some(conn_id))
    }

    /// 单次遍历把所有条目按 conn_id 分桶，每条隧道取 top_n。
    ///
    /// 对比 `snapshot_for_conn` 逐隧道扫描（O(隧道数 × 条目数)），
    /// 这里只遍历一次表（O(条目数)），适合一次性给所有隧道附 flows。
    pub fn snapshot_all_conns(
        &self,
        view: FlowView,
        top_n: usize,
    ) -> HashMap<PeerConnId, Vec<FlowSnapshot>> {
        if !self.is_enabled() {
            return HashMap::new();
        }
        let sample_rate = u64::from(self.sample_rate.load(Ordering::Relaxed).max(1));

        // 先按 (conn_id, peer_id, dir, ip, prefix_len) 聚合，再按 conn_id 分桶。
        let mut merged: HashMap<(PeerConnId, PeerId, FlowDirection, IpAddr, u8), FlowSnapshot> =
            HashMap::new();

        for entry in self.flows.iter() {
            let key = entry.key();
            let (ip, prefix_len) = match view {
                FlowView::Exact => (key.ip, 0),
                FlowView::Masked {
                    v4_prefix_len,
                    v6_prefix_len,
                } => mask_ip(key.ip, v4_prefix_len, v6_prefix_len),
            };
            let scale = if entry.sampled.load(Ordering::Relaxed) {
                sample_rate
            } else {
                1
            };
            let slot = merged
                .entry((key.conn_id, key.peer_id, key.dir, ip, prefix_len))
                .or_insert_with(|| FlowSnapshot {
                    conn_id: key.conn_id,
                    peer_id: key.peer_id,
                    ip,
                    prefix_len,
                    dir: key.dir,
                    bytes: 0,
                    packets: 0,
                    rate_bytes_per_sec: 0,
                    flow_count: 0,
                    sampled: false,
                    first_seen_ms: u64::MAX,
                    last_seen_ms: 0,
                });
            slot.bytes += entry.total_bytes.load(Ordering::Relaxed) * scale;
            slot.packets += entry.total_packets.load(Ordering::Relaxed) * scale;
            slot.rate_bytes_per_sec += entry.rate_bytes.load(Ordering::Relaxed) * scale;
            slot.flow_count += 1;
            slot.sampled |= entry.sampled.load(Ordering::Relaxed);
            slot.first_seen_ms = slot
                .first_seen_ms
                .min(entry.first_seen_ms.load(Ordering::Relaxed));
            slot.last_seen_ms = slot
                .last_seen_ms
                .max(entry.last_seen_ms.load(Ordering::Relaxed));
        }

        // 按 conn_id 分桶，每桶按 bytes 降序截断到 top_n。
        let mut buckets: HashMap<PeerConnId, Vec<FlowSnapshot>> = HashMap::new();
        for snapshot in merged.into_values() {
            buckets.entry(snapshot.conn_id).or_default().push(snapshot);
        }
        for flows in buckets.values_mut() {
            flows.sort_by_key(|s| std::cmp::Reverse(s.bytes));
            flows.truncate(top_n);
        }
        buckets
    }

    fn build_snapshot(
        &self,
        view: FlowView,
        top_n: usize,
        conn_filter: Option<PeerConnId>,
    ) -> Vec<FlowSnapshot> {
        if !self.is_enabled() {
            return Vec::new();
        }
        let sample_rate = u64::from(self.sample_rate.load(Ordering::Relaxed).max(1));

        let mut merged: HashMap<(PeerConnId, PeerId, FlowDirection, IpAddr, u8), FlowSnapshot> =
            HashMap::new();

        for entry in self.flows.iter() {
            let key = entry.key();
            if let Some(conn_id) = conn_filter
                && key.conn_id != conn_id
            {
                continue;
            }
            let (ip, prefix_len) = match view {
                FlowView::Exact => (key.ip, 0),
                FlowView::Masked {
                    v4_prefix_len,
                    v6_prefix_len,
                } => mask_ip(key.ip, v4_prefix_len, v6_prefix_len),
            };
            let scale = if entry.sampled.load(Ordering::Relaxed) {
                sample_rate
            } else {
                1
            };
            let slot = merged
                .entry((key.conn_id, key.peer_id, key.dir, ip, prefix_len))
                .or_insert_with(|| FlowSnapshot {
                    conn_id: key.conn_id,
                    peer_id: key.peer_id,
                    ip,
                    prefix_len,
                    dir: key.dir,
                    bytes: 0,
                    packets: 0,
                    rate_bytes_per_sec: 0,
                    flow_count: 0,
                    sampled: false,
                    first_seen_ms: u64::MAX,
                    last_seen_ms: 0,
                });
            slot.bytes += entry.total_bytes.load(Ordering::Relaxed) * scale;
            slot.packets += entry.total_packets.load(Ordering::Relaxed) * scale;
            slot.rate_bytes_per_sec += entry.rate_bytes.load(Ordering::Relaxed) * scale;
            slot.flow_count += 1;
            slot.sampled |= entry.sampled.load(Ordering::Relaxed);
            slot.first_seen_ms = slot
                .first_seen_ms
                .min(entry.first_seen_ms.load(Ordering::Relaxed));
            slot.last_seen_ms = slot
                .last_seen_ms
                .max(entry.last_seen_ms.load(Ordering::Relaxed));
        }

        let mut out: Vec<FlowSnapshot> = merged.into_values().collect();
        out.sort_by_key(|s| std::cmp::Reverse(s.bytes));
        out.truncate(top_n);
        out
    }
}

/// Current wall clock in unix milliseconds (0 if the clock is unavailable).
pub fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Truncate an address to a prefix, returning the masked address and the
/// prefix length that was applied.
///
/// Prefix lengths are clamped to `1..=32` (IPv4) / `1..=128` (IPv6). A value of
/// 0 used to mean "no aggregation" and returned the exact address, which let a
/// form field bypass decision D8 (exact IPs never leave the node). Exact IPs
/// are only available through [`FlowView::Exact`] with the node-side
/// `allow_exact_ip` opt-in. The clamp also keeps the shifts below in range:
/// `value << 32` (or `<< 128`) would overflow and panic in debug builds.
pub fn mask_ip(ip: IpAddr, v4_prefix_len: u8, v6_prefix_len: u8) -> (IpAddr, u8) {
    match ip {
        IpAddr::V4(v4) => {
            let plen = v4_prefix_len.clamp(1, 32);
            let bits = u32::from(32 - plen);
            let masked = u32::from(v4) & (!0u32 << bits);
            (IpAddr::V4(Ipv4Addr::from(masked)), plen)
        }
        IpAddr::V6(v6) => {
            let plen = v6_prefix_len.clamp(1, 128);
            let bits = u32::from(128 - plen);
            let masked = u128::from(v6) & (!0u128 << bits);
            (IpAddr::V6(Ipv6Addr::from(masked)), plen)
        }
    }
}

/// Extract the inner (source, destination) addresses of a plaintext IP packet.
///
/// Only the base header is inspected: for IPv4 the addresses always live at
/// fixed offsets (12..16 / 16..20) and for IPv6 at 8..24 / 24..40, so options
/// and extension headers do not affect the result.
pub fn parse_inner_ips(payload: &[u8]) -> Option<(IpAddr, IpAddr)> {
    if payload.is_empty() {
        return None;
    }
    match payload[0] >> 4 {
        4 => {
            if payload.len() < 20 {
                return None;
            }
            let mut src = [0u8; 4];
            let mut dst = [0u8; 4];
            src.copy_from_slice(&payload[12..16]);
            dst.copy_from_slice(&payload[16..20]);
            Some((IpAddr::from(src), IpAddr::from(dst)))
        }
        6 => {
            if payload.len() < 40 {
                return None;
            }
            let mut src = [0u8; 16];
            let mut dst = [0u8; 16];
            src.copy_from_slice(&payload[8..24]);
            dst.copy_from_slice(&payload[24..40]);
            Some((IpAddr::from(src), IpAddr::from(dst)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn conn(n: u8) -> PeerConnId {
        uuid::Uuid::from_bytes([n; 16])
    }

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))
    }

    fn table() -> FlowTable {
        FlowTable::new(FlowTableConfig {
            enabled: true,
            ..Default::default()
        })
    }

    #[test]
    fn disabled_table_records_nothing() {
        let t = FlowTable::new(FlowTableConfig::default());
        assert!(!t.is_enabled());
        assert!(!t.record(conn(1), 7, ip(1), FlowDirection::Tx, 100, 1, 1000));
        assert!(t.is_empty());
        assert!(t.snapshot(FlowView::Exact, 10).is_empty());
    }

    /// §17.6 R7: measure the hot-path cost instead of quoting the §6 estimate.
    ///
    /// `#[ignore]` because it is a benchmark, not an assertion — wall-clock
    /// numbers on a shared CI box are noise. Run it explicitly:
    ///
    /// ```text
    /// cargo test -p easytier-core --lib flow_record_cost -- --ignored --nocapture
    /// ```
    ///
    /// It prints ns/record for the disabled switch and for the enabled table at
    /// a realistic flow cardinality, so the README / roadmap can quote a
    /// measured number with its口径 instead of a theoretical one.
    #[test]
    #[ignore = "benchmark: run with --ignored --nocapture to print numbers"]
    fn flow_record_cost_is_measured() {
        use std::hint::black_box;
        use std::time::Instant;

        const ITERS: u64 = 2_000_000;

        // 1. Master switch off: the only work per packet is one atomic load.
        let off = FlowTable::new(FlowTableConfig::default());
        assert!(!off.is_enabled());
        let start = Instant::now();
        for i in 0..ITERS {
            black_box(off.record(
                conn(1),
                7,
                ip((i % 200) as u8),
                FlowDirection::Tx,
                1500,
                1,
                i,
            ));
        }
        let off_ns = start.elapsed().as_nanos() as f64 / ITERS as f64;

        // 2. Enabled, 200 live destinations on one tunnel (Phase 0 saw ~14
        //    globally, so this is deliberately pessimistic).
        let on = FlowTable::new(FlowTableConfig {
            enabled: true,
            max_per_conn: 256,
            max_total: 8192,
            ..Default::default()
        });
        for i in 0..200u64 {
            on.record(conn(1), 7, ip(i as u8), FlowDirection::Tx, 1, 1, i);
        }
        let start = Instant::now();
        for i in 0..ITERS {
            black_box(on.record(
                conn(1),
                7,
                ip((i % 200) as u8),
                FlowDirection::Tx,
                1500,
                1,
                i,
            ));
        }
        let on_ns = start.elapsed().as_nanos() as f64 / ITERS as f64;

        // 3. Enabled + 1/8 sampling (the high-packet-rate fallback).
        let sampled = FlowTable::new(FlowTableConfig {
            enabled: true,
            sample_rate: 8,
            max_per_conn: 256,
            max_total: 8192,
            ..Default::default()
        });
        for i in 0..200u64 {
            sampled.record(conn(1), 7, ip(i as u8), FlowDirection::Tx, 1, 1, i);
        }
        let start = Instant::now();
        for i in 0..ITERS {
            black_box(sampled.record(
                conn(1),
                7,
                ip((i % 200) as u8),
                FlowDirection::Tx,
                1500,
                1,
                i,
            ));
        }
        let sampled_ns = start.elapsed().as_nanos() as f64 / ITERS as f64;

        println!("=== flow accounting hot-path cost (ns per packet) ===");
        println!("switch off          : {off_ns:8.2}");
        println!("enabled, 200 flows  : {on_ns:8.2}");
        println!("enabled, 1/8 sample : {sampled_ns:8.2}");
        // 1 Gbps @ 1500B ≈ 83.3 kpps; 10 Gbps ≈ 833 kpps.
        for pps in [83_333.0_f64, 833_333.0] {
            let share = on_ns * pps / 1e9 * 100.0;
            println!("  enabled @ {pps:>9.0} pps -> {share:5.2}% of one core");
        }

        // Guard rails so a catastrophic regression fails the benchmark run.
        assert_eq!(on.len(), 200, "200 distinct destinations must be kept");
        assert!(
            off_ns < 20.0,
            "disabled switch must be a single atomic load, got {off_ns:.2} ns"
        );
    }

    #[test]
    fn configure_drops_counters_recorded_at_the_old_sample_rate() {
        let t = table();
        assert!(t.record(conn(1), 7, ip(1), FlowDirection::Tx, 100, 1, 1000));
        assert_eq!(t.len(), 1);

        // F-3: the read path scales totals by the *current* sample_rate, so
        // totals recorded at rate 1 must not survive a switch to rate 8 —
        // otherwise 1 GB reads back as 8 GB.
        t.configure(FlowTableConfig {
            enabled: true,
            sample_rate: 8,
            ..Default::default()
        });

        assert!(t.is_empty(), "counters must not survive a reconfiguration");
        assert_eq!(t.config().sample_rate, 8);
    }

    #[test]
    fn summary_reports_totals_and_is_zero_when_disabled() {
        let disabled = FlowTable::new(FlowTableConfig::default());
        assert_eq!(disabled.summary(), FlowSummary::default());

        let t = table();
        t.record(conn(1), 7, ip(1), FlowDirection::Tx, 100, 1, 1000);
        t.record(conn(1), 7, ip(2), FlowDirection::Rx, 50, 1, 1000);
        t.rotate_rate_window();

        let summary = t.summary();
        assert_eq!(summary.entries, 2);
        assert_eq!(summary.conns, 1);
        assert_eq!(summary.tx_bytes, 100);
        assert_eq!(summary.rx_bytes, 50);
        assert_eq!(summary.rate_bytes_per_sec, 150);
    }

    #[test]
    fn tx_and_rx_are_attributed_to_their_tunnel() {
        let t = table();
        t.record(conn(1), 7, ip(1), FlowDirection::Tx, 100, 1, 1000);
        t.record(conn(2), 8, ip(2), FlowDirection::Rx, 200, 1, 1000);
        let snaps = t.snapshot(FlowView::Exact, 10);
        assert_eq!(snaps.len(), 2);
        let tx = snaps.iter().find(|s| s.dir == FlowDirection::Tx).unwrap();
        assert_eq!(tx.conn_id, conn(1));
        assert_eq!(tx.peer_id, 7);
        assert_eq!(tx.bytes, 100);
        let rx = snaps.iter().find(|s| s.dir == FlowDirection::Rx).unwrap();
        assert_eq!(rx.conn_id, conn(2));
        assert_eq!(rx.bytes, 200);
    }

    #[test]
    fn counters_accumulate_per_ip() {
        let t = table();
        for _ in 0..10 {
            t.record(conn(1), 7, ip(1), FlowDirection::Tx, 50, 1, 1000);
        }
        let snaps = t.snapshot(FlowView::Exact, 10);
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].bytes, 500);
        assert_eq!(snaps[0].packets, 10);
    }

    #[test]
    fn sampling_keeps_a_subset_and_marks_estimate() {
        let t = FlowTable::new(FlowTableConfig {
            enabled: true,
            sample_rate: 8,
            ..Default::default()
        });
        for i in 0..80u8 {
            t.record(conn(1), 7, ip(i), FlowDirection::Tx, 100, 1, 1000);
        }
        // 1 of every 8 packets is kept -> 10 entries out of 80.
        let snaps = t.snapshot(FlowView::Exact, 100);
        assert_eq!(snaps.len(), 10);
        assert!(snaps.iter().all(|s| s.sampled));
        // The estimate is scaled back up to the observed volume.
        assert_eq!(snaps.iter().map(|s| s.bytes).sum::<u64>(), 80 * 100);
    }

    #[test]
    fn per_conn_cap_is_enforced() {
        let t = FlowTable::new(FlowTableConfig {
            enabled: true,
            max_per_conn: 4,
            ..Default::default()
        });
        for i in 0..20u8 {
            t.record(conn(1), 7, ip(i), FlowDirection::Tx, 100, 1, 1000);
        }
        let snaps = t.snapshot(FlowView::Exact, 100);
        assert!(
            snaps.len() <= 4,
            "expected <= 4 entries, got {}",
            snaps.len()
        );
    }

    #[test]
    fn global_cap_is_enforced() {
        let t = FlowTable::new(FlowTableConfig {
            enabled: true,
            max_per_conn: 100,
            max_total: 10,
            ..Default::default()
        });
        for c in 1..=4u8 {
            for i in 0..20u8 {
                t.record(
                    conn(c),
                    u32::from(c),
                    ip(i),
                    FlowDirection::Tx,
                    100,
                    1,
                    1000,
                );
            }
        }
        let snaps = t.snapshot(FlowView::Exact, 1000);
        assert!(
            snaps.len() <= 10,
            "expected <= 10 entries, got {}",
            snaps.len()
        );
    }

    #[test]
    fn ttl_drops_expired_entries() {
        let t = table();
        t.record(conn(1), 7, ip(1), FlowDirection::Tx, 100, 1, 1_000);
        assert_eq!(t.len(), 1);
        // ttl is 600s -> 600_000 ms; 700s later the entry is expired.
        t.gc(1_000 + 700_000);
        assert_eq!(t.len(), 0);
    }

    /// §17.6 R2: `gc` must not leave `per_conn_counts` smaller than the real
    /// number of entries. The old implementation cleared the map and re-summed;
    /// a concurrent `record` in that window was lost forever, so the cap drifted
    /// looser on every GC. Rebuilding into a local map and overwriting in place
    /// keeps the index at least as accurate as the table it describes.
    #[test]
    fn gc_rebuild_keeps_per_conn_counts_in_sync() {
        let t = FlowTable::new(FlowTableConfig {
            enabled: true,
            max_per_conn: 1000,
            max_total: 1000,
            ..Default::default()
        });
        for i in 0..10u8 {
            t.record(conn(1), 7, ip(i), FlowDirection::Tx, 100, 1, 1_000);
        }
        // Wipe the index to simulate the worst case the race could produce.
        t.per_conn_counts.clear();
        t.gc(1_000);
        assert_eq!(
            t.per_conn_counts.get(&conn(1)).map(|c| *c),
            Some(10),
            "gc must restore the exact per-tunnel count from the table"
        );

        // Entries of a dead tunnel are dropped from the index too.
        for i in 0..3u8 {
            t.record(conn(2), 8, ip(i), FlowDirection::Tx, 100, 1, 1_000);
        }
        t.gc(1_000 + 700_000);
        assert_eq!(t.len(), 0);
        assert!(
            t.per_conn_counts.is_empty(),
            "the index must not keep tunnels whose entries all expired"
        );
    }

    /// §17.6 R3: the entry that triggered the cap check is the newest one and
    /// must survive. Evicting it would re-create it on the next packet and make
    /// a burst of new destinations thrash the table.
    #[test]
    fn per_conn_cap_never_evicts_the_entry_just_inserted() {
        let t = FlowTable::new(FlowTableConfig {
            enabled: true,
            max_per_conn: 3,
            max_total: 1000,
            ..Default::default()
        });
        // Fill to the cap, then insert a burst of new destinations.
        for i in 0..20u8 {
            let target = ip(i);
            assert!(
                t.record(conn(1), 7, target, FlowDirection::Tx, 100, 1, 1_000),
                "record must report success"
            );
            // The freshest destination is always still present right after.
            assert!(
                t.flows.contains_key(&FlowKey {
                    conn_id: conn(1),
                    peer_id: 7,
                    ip: target,
                    dir: FlowDirection::Tx,
                }),
                "the just-inserted flow {target} was evicted by its own cap check"
            );
        }
        assert_eq!(t.len(), 3, "the tunnel must converge to max_per_conn");
    }

    /// R3 also covers the global cap: one over-limit insertion drops every
    /// excess entry at once, not one per insertion.
    #[test]
    fn global_cap_evicts_all_excess_in_one_pass() {
        let t = FlowTable::new(FlowTableConfig {
            enabled: true,
            max_per_conn: 1000,
            max_total: 5,
            ..Default::default()
        });
        for i in 0..50u8 {
            t.record(conn(1), 7, ip(i), FlowDirection::Tx, 100, 1, 1_000);
            assert!(
                t.len() <= 5,
                "table grew past max_total after inserting flow {i}: {}",
                t.len()
            );
        }
    }

    #[test]
    fn masked_view_merges_prefixes_and_counts_flows() {
        let t = table();
        t.record(conn(1), 7, ip(1), FlowDirection::Tx, 100, 1, 1000);
        t.record(conn(1), 7, ip(2), FlowDirection::Tx, 200, 1, 1000);
        t.record(
            conn(1),
            7,
            IpAddr::V4(Ipv4Addr::new(10, 0, 1, 1)),
            FlowDirection::Tx,
            400,
            1,
            1000,
        );

        let masked = t.snapshot(
            FlowView::Masked {
                v4_prefix_len: 24,
                v6_prefix_len: 64,
            },
            10,
        );
        assert_eq!(masked.len(), 2);
        let bucket = masked
            .iter()
            .find(|s| s.prefix_len == 24 && s.ip == IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)))
            .unwrap();
        assert_eq!(bucket.flow_count, 2);
        assert_eq!(bucket.bytes, 300);

        let exact = t.snapshot(FlowView::Exact, 10);
        assert_eq!(exact.len(), 3);
        assert!(exact.iter().all(|s| s.prefix_len == 0));
    }

    #[test]
    fn rate_window_is_independent_of_polling_cadence() {
        let t = table();
        t.record(conn(1), 7, ip(1), FlowDirection::Tx, 1000, 1, 1000);
        t.rotate_rate_window();
        // The window that just closed is reported as the rate.
        let snaps = t.snapshot(FlowView::Exact, 10);
        assert_eq!(snaps[0].rate_bytes_per_sec, 1000);
        // A new window starts empty.
        t.rotate_rate_window();
        let snaps = t.snapshot(FlowView::Exact, 10);
        assert_eq!(snaps[0].rate_bytes_per_sec, 0);
    }

    #[test]
    fn snapshot_is_truncated_and_sorted_by_bytes() {
        let t = table();
        t.record(conn(1), 7, ip(1), FlowDirection::Tx, 10, 1, 1000);
        t.record(conn(1), 7, ip(2), FlowDirection::Tx, 1000, 1, 1000);
        t.record(conn(1), 7, ip(3), FlowDirection::Tx, 100, 1, 1000);
        let snaps = t.snapshot(FlowView::Exact, 2);
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].bytes, 1000);
        assert_eq!(snaps[1].bytes, 100);
    }

    #[test]
    fn parse_inner_ips_handles_v4_and_v6() {
        let mut v4 = vec![0u8; 20];
        v4[0] = 0x45;
        v4[12..16].copy_from_slice(&[10, 0, 0, 1]);
        v4[16..20].copy_from_slice(&[10, 0, 0, 2]);
        let (src, dst) = parse_inner_ips(&v4).unwrap();
        assert_eq!(src, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(dst, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));

        let mut v6 = vec![0u8; 40];
        v6[0] = 0x60;
        v6[8..24].copy_from_slice(&[0xfd, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
        v6[24..40].copy_from_slice(&[0xfd, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 16]);
        let (src, dst) = parse_inner_ips(&v6).unwrap();
        assert!(src.is_ipv6());
        assert!(dst.is_ipv6());

        assert!(parse_inner_ips(&[]).is_none());
        assert!(parse_inner_ips(&[0x45, 0, 0]).is_none());
    }

    #[test]
    fn rx_records_the_source_ip_of_the_inner_packet() {
        let t = table();
        let mut v4 = vec![0u8; 20];
        v4[0] = 0x45;
        v4[12..16].copy_from_slice(&[10, 0, 0, 7]);
        v4[16..20].copy_from_slice(&[10, 0, 0, 1]);

        // Locally injected packets carry no tunnel -> not counted.
        assert!(!t.record_rx(None, &v4, 1000));
        assert!(t.is_empty());

        assert!(t.record_rx(Some((conn(3), 9)), &v4, 1000));
        let snaps = t.snapshot(FlowView::Exact, 10);
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].dir, FlowDirection::Rx);
        assert_eq!(snaps[0].ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)));
        assert_eq!(snaps[0].conn_id, conn(3));
        assert_eq!(snaps[0].bytes, 20);
    }

    #[test]
    fn rx_ignores_non_ip_payloads() {
        let t = table();
        assert!(!t.record_rx(Some((conn(3), 9)), b"not-an-ip-packet", 1000));
        assert!(t.is_empty());
    }

    #[test]
    fn mask_ip_truncates_to_prefix() {
        let (ip, plen) = mask_ip(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)), 24, 64);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(10, 1, 2, 0)));
        assert_eq!(plen, 24);
        let (ip, plen) = mask_ip(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)), 16, 64);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(10, 1, 0, 0)));
        assert_eq!(plen, 16);
    }

    #[test]
    fn mask_ip_clamps_zero_prefix_len_to_one() {
        // 0 used to bypass D8 by returning the exact address. It now clamps to
        // /1, so a form field can no longer leak exact IPs.
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3));
        assert_eq!(mask_ip(v4, 0, 64), (IpAddr::V4(Ipv4Addr::UNSPECIFIED), 1));

        let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
        assert_eq!(mask_ip(v6, 24, 0), (IpAddr::V6(Ipv6Addr::UNSPECIFIED), 1));

        // Full-width prefixes clamp instead of overflowing.
        let (ip, plen) = mask_ip(v4, 32, 128);
        assert_eq!((ip, plen), (v4, 32));
        assert_eq!(mask_ip(v6, 24, 128), (v6, 128));
    }

    #[test]
    fn masked_view_with_zero_prefix_len_clamps_and_aggregates() {
        let t = table();
        t.record(conn(1), 7, ip(1), FlowDirection::Tx, 100, 1, 1000);
        t.record(conn(1), 7, ip(2), FlowDirection::Tx, 200, 1, 1000);

        let rows = t.snapshot(
            FlowView::Masked {
                v4_prefix_len: 0,
                v6_prefix_len: 0,
            },
            10,
        );

        // 0 clamps to /1: 10.0.0.1 and 10.0.0.2 land in the same 0.0.0.0/1 bucket.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].prefix_len, 1);
        assert_eq!(rows[0].flow_count, 2);
        assert_eq!(rows[0].bytes, 300);
    }
}
