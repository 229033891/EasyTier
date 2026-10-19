//! VIP dataplane reachability prober (GUI "connected vs reachable").
//!
//! Injects ICMP Echo Requests into the core data plane and matches Echo
//! Replies through a peer-packet pipeline filter. Results feed
//! `NetworkInstanceRunningInfo.peer_vip_reachability`, which the frontend
//! renders as the `[M/N]` score and the per-row VIP Tag.
//!
//! **Granularity: per link, not per peer.** Every live PeerConn of a peer is
//! probed on *that conn* (`PeerManagerCore::send_probe_msg_on_conn`), so a dead
//! primary shows as unreachable while its standby still answers — the two are
//! indistinguishable if conn selection is allowed to pick the path. The peer as
//! a whole counts reachable when **any** of its links is reachable.
//!
//! Scope note (roadmap §3.1 / §5): the prober deliberately does **not** go
//! through the host TUN or the OS route table. That keeps it free of raw-socket
//! privileges, stops it from polluting the user's own ICMP statistics, and keeps
//! it working on Android/`no_tun`. The consequence is explicit and tested: a
//! broken *host-side* TUN route is invisible here; what this reports is the
//! **mesh forwarding path** to the peer's VIP.
//!
//! Contract (docs/roadmap/gui-connected-vs-reachable.md §3):
//! - 20s cadence, at most `MAX_CONCURRENT_PROBES` links in flight, round-robin
//!   across the whole link set so every link is covered on a bounded schedule,
//! - one timeout ⇒ UNREACHABLE, one success ⇒ REACHABLE,
//! - display-only: no reroute, no conn teardown, no `ping_fail_close_count`,
//!   no P2P control-plane heuristics (`recent_traffic`) and no per-peer user
//!   traffic accounting,
//! - replies are matched by a dedicated echo identifier band, a magic payload,
//!   **and** the peer the identifier was sent to, so neither user pings nor a
//!   spoofing peer can move the score.
//!
//! **This prober is not a keepalive, and that is a decision, not an oversight
//! (roadmap §2.4.1).** Raising the cadence to "keep links warm" was considered
//! and rejected:
//! - NAT mappings are already maintained by the control plane:
//!   `DEFAULT_PING_INTERVAL_MAX_SEC` is 10s and exists precisely to stay inside
//!   the ~30s carrier-NAT UDP lifetime. This prober's 20s is *slower*, so it
//!   adds no mapping-liveness value.
//! - Path selection never reads probe results. `pick_default_conn` scores
//!   `ConnMetrics` derived from control-plane pongs (`latency_us` / `loss_rate` /
//!   `jitter_us`), so a faster probe would not move a single switch decision.
//! - The cost would be real: probes share the conn and its send ring with user
//!   traffic (congestion), their losses can be counted against the path, and
//!   idle-time probe traffic can perturb lazy-p2p.
//!
//! Anything that lets these results influence routing, teardown or traffic
//! accounting changes what this feature *is* and needs its own design review.
//!
//! ICMP/IPv4 headers are composed by hand (no `smoltcp` dependency) so the
//! prober compiles under the default feature set.

use std::{
    collections::HashMap,
    net::Ipv4Addr,
    sync::{
        Arc,
        atomic::{AtomicU16, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::{
    config::PeerId,
    foundation::stats::MetricName,
    packet::{PacketType, ZCPacket},
    peers::{conn::peer_conn::PeerConnId, peer_manager::PeerManagerCore},
    proto::api::instance::{PeerVipReachability, PeerVipState},
};

/// Echo identifier band owned by the prober. High bytes avoid colliding with
/// user pings on either node (kernel ICMP rewrites ident anyway, and replies
/// come back through the mesh pipeline, not the kernel, so this is a
/// belt-and-suspenders second match alongside the magic payload).
const PROBE_IDENT_BASE: u16 = 0xF000;
/// Size of the reserved ident band.
const PROBE_IDENT_BAND: u16 = 0x0FFF;
/// Magic payload prefix; a reply must carry it back to be claimed by us.
const PROBE_MAGIC: &[u8; 4] = b"ETVP";
/// ICMP echo header length (type + code + checksum + ident + seq).
const ICMP_ECHO_HEADER_LEN: usize = 8;
/// IPv4 header length (no options).
const IPV4_HEADER_LEN: usize = 20;
/// One probe round; per the roadmap contract (§3.2).
///
/// **Do not lower this to "keep links warm".** The prober is observability, not
/// a keepalive (see the module docs and roadmap §2.4.1): NAT liveness is already
/// covered faster by the 10s control-plane ping, and path selection does not
/// read these results at all — so a faster cadence buys no stability and only
/// competes with user traffic for the same conn.
pub const PROBE_INTERVAL: Duration = Duration::from_secs(20);
/// Per-probe reply window (roadmap: 2–3s suggested).
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// Max links in flight in one round. The link set is walked round-robin, so this
/// bounds concurrency only — never coverage (see the cursor below).
const MAX_CONCURRENT_PROBES: usize = 3;

/// Probe outcome reasons carried in `PeerVipReachability.error`.
pub mod error {
    /// No reply inside `PROBE_TIMEOUT`.
    pub const TIMEOUT: &str = "timeout";
    /// The send path rejected the packet (no route / tunnel refused / conn gone).
    pub const SEND_FAILED: &str = "send_failed";
    /// This node has no virtual IPv4 of its own, so there is no probe source.
    pub const NO_VIP: &str = "no_vip";
    /// Disabled by configuration.
    pub const DISABLED: &str = "disabled";
    /// TUN-less transport: ICMP semantics differ, so we refuse to guess.
    pub const NO_TUN: &str = "no_tun";
}

/// Identifies one probed link: a peer's PeerConn.
///
/// `conn_id` is `None` only for the peer-level fallback entry (peer connected via
/// a path we cannot name a conn for, e.g. relay-only), which is probed over the
/// routed path instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LinkKey {
    peer_id: PeerId,
    conn_id: Option<PeerConnId>,
}

impl LinkKey {
    fn conn_id_str(&self) -> String {
        self.conn_id.map(|id| id.to_string()).unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
struct ProbeResult {
    state: PeerVipState,
    rtt_ms: Option<u32>,
    checked_at_unix_ms: u64,
    error: String,
}

impl ProbeResult {
    fn new(state: PeerVipState, rtt_ms: Option<u32>, error: &str) -> Self {
        Self {
            state,
            rtt_ms,
            checked_at_unix_ms: unix_now_ms(),
            error: error.to_owned(),
        }
    }
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

fn icmp_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    for chunk in data.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]]) as u32
        } else {
            (chunk[0] as u32) << 8
        };
        sum += word;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// In-flight probe bookkeeping, keyed by echo identifier.
///
/// The target is stored **with** the send time so a reply can be checked against
/// the link the identifier was actually sent to. Without that check any member
/// could answer with our magic payload and award itself "reachable".
type InFlight = HashMap<u16, InFlightProbe>;

#[derive(Debug, Clone)]
struct InFlightProbe {
    key: LinkKey,
    seq: u16,
    sent_at: std::time::Instant,
}

#[derive(Default)]
struct ProbeStateInner {
    /// link → last probe result.
    results: HashMap<LinkKey, ProbeResult>,
}

/// Shared state between the prober task and the reply filter.
#[derive(Default)]
pub struct VipProbeState {
    inner: Mutex<ProbeStateInner>,
    next_ident: AtomicU16,
    /// Round-robin cursor into the sorted link list; guarantees coverage of
    /// every link even when the link count exceeds the per-round cap.
    cursor: AtomicUsize,
    sent: Arc<Mutex<InFlight>>,
}

impl VipProbeState {
    fn claim_ident(&self) -> u16 {
        // Wrap within the reserved band.
        let rel = self.next_ident.fetch_add(1, Ordering::Relaxed) % PROBE_IDENT_BAND;
        PROBE_IDENT_BASE.wrapping_add(rel)
    }

    fn record(&self, key: LinkKey, result: ProbeResult) {
        self.inner.lock().results.insert(key, result);
    }

    fn record_probing(&self, key: LinkKey) {
        self.record(key, ProbeResult::new(PeerVipState::Probing, None, ""));
    }

    /// Marks every still-in-flight probe as unreachable and drains the table.
    /// Called once per round after the reply window closes.
    fn settle_round(&self, sent: &mut InFlight) {
        let mut inner = self.inner.lock();
        for (_, probe) in sent.drain() {
            inner.results.insert(
                probe.key,
                ProbeResult::new(PeerVipState::Unreachable, None, error::TIMEOUT),
            );
        }
    }

    /// Drops results whose link is no longer part of the instance.
    fn gc(&self, live: &[LinkKey]) {
        const HARD_CAP: usize = 4096;
        let mut inner = self.inner.lock();
        if inner.results.len() > HARD_CAP || live.is_empty() {
            inner.results.clear();
            return;
        }
        let live: std::collections::HashSet<&LinkKey> = live.iter().collect();
        inner.results.retain(|key, _| live.contains(key));
    }
}

/// Builds one ICMPv4 Echo Request addressed `my_vip → peer_vip`, wrapped in a
/// ZCPacket ready for the probe send paths.
fn build_echo_request(my_vip: Ipv4Addr, peer_vip: Ipv4Addr, ident: u16, seq: u16) -> ZCPacket {
    // ICMP: type(1) code(1) cksum(2) ident(2) seq(2) + magic(4) + seq-echo(2)
    let icmp_len = ICMP_ECHO_HEADER_LEN + PROBE_MAGIC.len() + 2;
    let mut icmp = vec![0u8; icmp_len];
    icmp[0] = 8; // Echo Request
    icmp[4..6].copy_from_slice(&ident.to_be_bytes());
    icmp[6..8].copy_from_slice(&seq.to_be_bytes());
    icmp[8..12].copy_from_slice(PROBE_MAGIC);
    icmp[12..14].copy_from_slice(&seq.to_be_bytes());
    let cksum = icmp_checksum(&icmp);
    icmp[2..4].copy_from_slice(&cksum.to_be_bytes());

    let total_len = IPV4_HEADER_LEN + icmp_len;
    let mut ip_buf = vec![0u8; total_len];
    ip_buf[0] = 0x45; // v4, IHL=5
    ip_buf[1] = 0; // DSCP/ECN
    ip_buf[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
    ip_buf[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // DF
    ip_buf[8] = 64; // hop limit
    ip_buf[9] = 1; // ICMP
    ip_buf[12..16].copy_from_slice(&my_vip.octets());
    ip_buf[16..20].copy_from_slice(&peer_vip.octets());
    let ip_cksum = icmp_checksum(&ip_buf[..IPV4_HEADER_LEN]);
    ip_buf[10..12].copy_from_slice(&ip_cksum.to_be_bytes());
    ip_buf[IPV4_HEADER_LEN..].copy_from_slice(&icmp);

    let mut packet = ZCPacket::new_with_payload(&ip_buf);
    packet.fill_peer_manager_hdr(0, 0, PacketType::Data as u8);
    packet
}

/// `batch_len = reached + timed_out + send_failed`. Send failures are removed
/// from the in-flight table before the reply window closes, so they must not be
/// counted as "reached" via `batch_len - timed_out`.
fn count_reached(batch_len: usize, timed_out: usize, send_failed: usize) -> usize {
    batch_len
        .saturating_sub(timed_out)
        .saturating_sub(send_failed)
}

/// True when this peer-packet is an echo reply answered by **the peer we probed**
/// for a still-pending identifier. Runs inside the peer packet pipeline; must be
/// cheap and match narrowly.
fn try_match_probe_reply(sent: &Mutex<InFlight>, packet: &ZCPacket) -> Option<(LinkKey, u32)> {
    let hdr = packet.peer_manager_header()?;
    if hdr.packet_type != PacketType::Data as u8 {
        return None;
    }
    let payload = packet.payload();
    if payload.len() < IPV4_HEADER_LEN + ICMP_ECHO_HEADER_LEN + PROBE_MAGIC.len() + 2 {
        return None;
    }
    // IPv4 header: version/IHL, protocol at fixed offset [9].
    if payload[0] >> 4 != 4 || payload[9] != 1 {
        return None;
    }
    // Honour IHL: a reply with IP options still carries ICMP after the full
    // header. Assuming 20 bytes would miss a valid echo and report timeout.
    let ihl = ((payload[0] & 0x0F) as usize).saturating_mul(4);
    if ihl < IPV4_HEADER_LEN || payload.len() < ihl + ICMP_ECHO_HEADER_LEN + PROBE_MAGIC.len() + 2 {
        return None;
    }
    let icmp = &payload[ihl..];
    if icmp[0] != 0 {
        return None; // not an Echo Reply
    }
    let ident = u16::from_be_bytes([icmp[4], icmp[5]]);
    if !(PROBE_IDENT_BASE..PROBE_IDENT_BASE + PROBE_IDENT_BAND).contains(&ident) {
        return None;
    }
    let data = &icmp[ICMP_ECHO_HEADER_LEN..];
    if data.len() < PROBE_MAGIC.len() + 2 || &data[..PROBE_MAGIC.len()] != PROBE_MAGIC {
        return None;
    }

    let from_peer: PeerId = hdr.from_peer_id.get();
    let echoed_seq = u16::from_be_bytes([data[PROBE_MAGIC.len()], data[PROBE_MAGIC.len() + 1]]);
    let mut sent = sent.lock();
    let probe = sent.get(&ident)?;
    // Attribution: the reply must come from the peer this identifier was sent
    // to, and must echo our sequence. Both are unforgeable by a third party.
    if probe.key.peer_id != from_peer || probe.seq != echoed_seq {
        return None;
    }
    let probe = sent.remove(&ident)?;
    let rtt_ms = probe.sent_at.elapsed().as_millis().min(u32::MAX as u128) as u32;
    // Sub-millisecond round trips stay 0 rather than being rounded up to 1ms.
    Some((probe.key, rtt_ms))
}

/// Enumerates every link to probe: one per live PeerConn, in a deterministic
/// order so the round-robin cursor walks a stable sequence.
///
/// A peer whose live conns cannot be enumerated (relay-only / not directly
/// connected) still gets one `conn_id: None` entry, probed over the routed path,
/// so it is never silently absent from the score.
async fn collect_links(peer_manager: &PeerManagerCore) -> Vec<(LinkKey, Ipv4Addr)> {
    let routes = peer_manager.list_route_snapshots().await;
    let my_peer_id = peer_manager.my_peer_id();
    let mut links: Vec<(LinkKey, Ipv4Addr)> = Vec::new();
    for route in routes.iter() {
        if route.peer_id == my_peer_id {
            continue;
        }
        let Some(addr) = route.ipv4_addr.and_then(|inet| inet.address) else {
            continue;
        };
        let peer_vip = Ipv4Addr::from(addr.addr);
        let mut conn_ids = peer_manager.peer_live_conn_ids(route.peer_id);
        conn_ids.sort_unstable();
        if conn_ids.is_empty() {
            links.push((
                LinkKey {
                    peer_id: route.peer_id,
                    conn_id: None,
                },
                peer_vip,
            ));
        } else {
            for conn_id in conn_ids {
                links.push((
                    LinkKey {
                        peer_id: route.peer_id,
                        conn_id: Some(conn_id),
                    },
                    peer_vip,
                ));
            }
        }
    }
    links.sort_by(|a, b| {
        a.0.peer_id
            .cmp(&b.0.peer_id)
            .then_with(|| a.0.conn_id.cmp(&b.0.conn_id))
    });
    links
}

/// One prober task per running instance. Stops when `cancel` fires.
///
/// The reply filter is a **managed** peer-pipeline entry: the registration
/// guard is held for the life of this task and unregisters on return / abort /
/// cancel. That way a restart cannot stack a second filter on the same
/// `PeerManagerCore` (permanent entries cannot be removed without
/// `clear_resources`).
pub async fn run_prober(
    peer_manager: Arc<PeerManagerCore>,
    state: Arc<VipProbeState>,
    cancel: CancellationToken,
) {
    // Filter registration is unconditional: even when the local VIP is missing
    // or probing is disabled now, a later DHCP lease / config flip must not
    // require a restart to start matching replies.
    let filter = ProbeReplyFilter {
        state: state.clone(),
        sent: state.sent.clone(),
    };
    // Kept until this future drops (normal exit, cancel, or task.abort()).
    let _pipeline_guard = peer_manager
        .add_managed_packet_process_pipeline(Box::new(filter))
        .await;

    let stats = peer_manager.stats_manager();
    let simple = || crate::foundation::stats::LabelSet::new();
    let sent_counter = stats.get_counter(MetricName::VipReachabilityProbesSent, simple());
    let reached_counter = stats.get_counter(MetricName::VipReachabilityProbesReached, simple());
    let timeout_counter = stats.get_counter(MetricName::VipReachabilityProbesTimedOut, simple());
    let send_failed_counter =
        stats.get_counter(MetricName::VipReachabilityProbesSendFailed, simple());

    loop {
        // Re-read the local VIP every round: DHCP may hand it out after start,
        // and a lease change must not leave us probing with a stale source.
        let my_vip = peer_manager
            .node_snapshot(Vec::new())
            .await
            .ipv4_addr
            .map(|inet| inet.address());
        let disabled = peer_manager.vip_probe_disabled();
        let no_tun = peer_manager.vip_probe_no_tun();
        let links = collect_links(peer_manager.as_ref()).await;

        match (disabled, my_vip, no_tun) {
            (true, _, _) => {
                for (key, _) in links.iter() {
                    state.record(
                        key.clone(),
                        ProbeResult::new(PeerVipState::Unknown, None, error::DISABLED),
                    );
                }
            }
            (false, None, _) => {
                for (key, _) in links.iter() {
                    state.record(
                        key.clone(),
                        ProbeResult::new(PeerVipState::Unknown, None, error::NO_VIP),
                    );
                }
            }
            (false, Some(_), true) => {
                for (key, _) in links.iter() {
                    state.record(
                        key.clone(),
                        ProbeResult::new(PeerVipState::Unknown, None, error::NO_TUN),
                    );
                }
            }
            (false, Some(my_vip), false) => {
                // Round-robin slice over *links*: take up to MAX_CONCURRENT_PROBES
                // starting at the cursor, then advance it. Every link is covered
                // within ceil(L / MAX) rounds — the cap bounds concurrency, never
                // coverage.
                let total = links.len();
                let take = total.min(MAX_CONCURRENT_PROBES);
                let start = if total == 0 {
                    0
                } else {
                    state.cursor.fetch_add(take, Ordering::Relaxed) % total
                };
                let batch: Vec<(LinkKey, Ipv4Addr)> = (0..take)
                    .map(|offset| links[(start + offset) % total].clone())
                    .collect();

                let seq_base = (unix_now_ms() & 0xFFFF) as u16;
                let round: Vec<(u16, LinkKey, Ipv4Addr, u16)> = {
                    let mut sent = state.sent.lock();
                    let mut idents = Vec::with_capacity(batch.len());
                    for (idx, (key, peer_vip)) in batch.iter().enumerate() {
                        state.record_probing(key.clone());
                        let ident = state.claim_ident();
                        let seq = seq_base.wrapping_add(idx as u16);
                        sent.insert(
                            ident,
                            InFlightProbe {
                                key: key.clone(),
                                seq,
                                sent_at: std::time::Instant::now(),
                            },
                        );
                        idents.push((ident, key.clone(), *peer_vip, seq));
                    }
                    idents
                };

                let mut send_failed = 0usize;
                for (ident, key, peer_vip, seq) in round {
                    let packet = build_echo_request(my_vip, peer_vip, ident, seq);
                    let dst = std::net::IpAddr::V4(peer_vip);
                    sent_counter.inc();
                    let result = match key.conn_id {
                        // Per-link probe: force the egress conn.
                        Some(conn_id) => {
                            peer_manager
                                .send_probe_msg_on_conn(packet, &conn_id, dst, key.peer_id)
                                .await
                        }
                        // Peer-level fallback: routed path (relay-only peers).
                        None => peer_manager.send_probe_msg_by_ip(packet, dst, true).await,
                    };
                    if let Err(err) = result {
                        tracing::debug!(
                            ?err,
                            peer_id = key.peer_id,
                            conn = %key.conn_id_str(),
                            "vip probe send failed"
                        );
                        send_failed += 1;
                        send_failed_counter.inc();
                        state.sent.lock().remove(&ident);
                        state.record(
                            key,
                            ProbeResult::new(PeerVipState::Unreachable, None, error::SEND_FAILED),
                        );
                    }
                }

                // Close the reply window, then settle whatever is still in flight.
                let mut window = tokio::time::interval(PROBE_TIMEOUT);
                window.tick().await; // consume the immediate first tick
                tokio::select! {
                    _ = window.tick() => {}
                    _ = cancel.cancelled() => return,
                }
                let mut sent = state.sent.lock();
                let timed_out = sent.len();
                // Send failures already left `sent`; do not count them as reached.
                let reached = count_reached(batch.len(), timed_out, send_failed);
                state.settle_round(&mut sent);
                drop(sent);
                for _ in 0..reached {
                    reached_counter.inc();
                }
                for _ in 0..timed_out {
                    timeout_counter.inc();
                }
            }
        }

        let live: Vec<LinkKey> = links.into_iter().map(|(key, _)| key).collect();
        state.gc(&live);

        let rest = PROBE_INTERVAL.saturating_sub(PROBE_TIMEOUT);
        tokio::select! {
            _ = crate::foundation::time::sleep(rest) => {}
            _ = cancel.cancelled() => return,
        }
    }
}

struct ProbeReplyFilter {
    state: Arc<VipProbeState>,
    sent: Arc<Mutex<InFlight>>,
}

#[async_trait::async_trait]
impl crate::peers::PeerPacketFilter for ProbeReplyFilter {
    async fn try_process_packet_from_peer(&self, packet: ZCPacket) -> Option<ZCPacket> {
        if let Some((key, rtt_ms)) = try_match_probe_reply(&self.sent, &packet) {
            self.state.record(
                key,
                ProbeResult::new(PeerVipState::Reachable, Some(rtt_ms), ""),
            );
            // Consumed: this reply belongs to the prober, not the TUN.
            return None;
        }
        Some(packet)
    }
}

/// Snapshot for `NetworkInstanceRunningInfo.peer_vip_reachability`.
///
/// One entry per probed link. `disabled` forces every peer to "unknown +
/// reason" — never "unreachable", which would read as "the network is broken".
pub fn snapshot_reachability(
    state: &VipProbeState,
    connected_peer_ids: impl IntoIterator<Item = PeerId>,
    disabled: bool,
) -> Vec<PeerVipReachability> {
    let inner = state.inner.lock();
    let mut out = Vec::new();
    for peer_id in connected_peer_ids {
        if disabled {
            out.push(PeerVipReachability {
                peer_id,
                conn_id: String::new(),
                state: PeerVipState::Unknown as i32,
                rtt_ms: None,
                checked_at_unix_ms: unix_now_ms(),
                error: error::DISABLED.to_owned(),
            });
            continue;
        }
        for (key, result) in inner.results.iter() {
            if key.peer_id != peer_id {
                continue;
            }
            out.push(PeerVipReachability {
                peer_id,
                conn_id: key.conn_id_str(),
                state: result.state as i32,
                rtt_ms: result.rtt_ms,
                checked_at_unix_ms: result.checked_at_unix_ms,
                error: result.error.clone(),
            });
        }
        // A connected peer with no result yet contributes nothing here — the
        // frontend maps absence to "unknown". The probe loop records an explicit
        // `UNKNOWN + reason` row for every non-probing mode (disabled / no VIP /
        // no TUN), so absence only means "no round has run yet".
    }
    out.sort_by(|a, b| {
        a.peer_id
            .cmp(&b.peer_id)
            .then_with(|| a.conn_id.cmp(&b.conn_id))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo_request(my_vip: &str, peer_vip: &str, ident: u16, seq: u16) -> ZCPacket {
        build_echo_request(
            my_vip.parse().unwrap(),
            peer_vip.parse().unwrap(),
            ident,
            seq,
        )
    }

    /// Turns a request into the reply the remote's data plane would send back:
    /// ICMP type flipped to Echo Reply, addresses swapped, checksum fixed.
    fn echo_reply_of(request: &ZCPacket, from: &str, to: &str) -> ZCPacket {
        let mut bytes = request.payload().to_vec();
        bytes[12..16].copy_from_slice(&from.parse::<Ipv4Addr>().unwrap().octets());
        bytes[16..20].copy_from_slice(&to.parse::<Ipv4Addr>().unwrap().octets());
        let off = IPV4_HEADER_LEN;
        bytes[off] = 0; // Echo Reply
        let cksum = icmp_checksum(&bytes[off..off + ICMP_ECHO_HEADER_LEN + 6]);
        bytes[off + 2..off + 4].copy_from_slice(&cksum.to_be_bytes());
        ZCPacket::new_with_payload(&bytes)
    }

    fn link(peer_id: PeerId, conn: u128) -> LinkKey {
        LinkKey {
            peer_id,
            conn_id: Some(uuid::Uuid::from_u128(conn)),
        }
    }

    #[test]
    fn count_reached_excludes_send_failures() {
        // Regression: `batch - timed_out` alone treated send_failed as reached.
        assert_eq!(count_reached(3, 1, 1), 1);
        assert_eq!(count_reached(3, 0, 3), 0);
        assert_eq!(count_reached(3, 3, 0), 0);
        assert_eq!(count_reached(3, 0, 0), 3);
    }

    #[test]
    fn echo_request_wire_format_is_valid_icmp() {
        let packet = echo_request("10.144.144.1", "10.144.144.2", PROBE_IDENT_BASE, 7);
        let payload = packet.payload();
        assert_eq!(payload.len(), IPV4_HEADER_LEN + ICMP_ECHO_HEADER_LEN + 6);

        assert_eq!(payload[0] >> 4, 4);
        assert_eq!(payload[9], 1); // ICMP
        let total_len = u16::from_be_bytes([payload[2], payload[3]]) as usize;
        assert_eq!(total_len, payload.len());

        let icmp = &payload[IPV4_HEADER_LEN..];
        assert_eq!(icmp[0], 8);
        assert_eq!(u16::from_be_bytes([icmp[4], icmp[5]]), PROBE_IDENT_BASE);
        assert_eq!(u16::from_be_bytes([icmp[6], icmp[7]]), 7);
        assert_eq!(icmp_checksum(icmp), 0, "checksum must verify to zero");
    }

    #[test]
    fn reply_with_ip_options_is_still_claimed() {
        // A peer that echoes with IHL>5 must not look like a timeout.
        let state = VipProbeState::default();
        let ident = state.claim_ident();
        let key = link(202, 1);
        let seq = 3;
        state.sent.lock().insert(
            ident,
            InFlightProbe {
                key: key.clone(),
                seq,
                sent_at: std::time::Instant::now(),
            },
        );
        let request = echo_request("10.144.144.2", "10.144.144.1", ident, seq);
        let mut reply = echo_reply_of(&request, "10.144.144.1", "10.144.144.2");
        // Insert 4 bytes of IP options (IHL 6) between the base header and ICMP.
        let mut bytes = reply.payload().to_vec();
        bytes.splice(IPV4_HEADER_LEN..IPV4_HEADER_LEN, [0u8; 4]);
        bytes[0] = 0x46; // v4, IHL=6
        let total = bytes.len() as u16;
        bytes[2..4].copy_from_slice(&total.to_be_bytes());
        bytes[10..12].copy_from_slice(&[0, 0]);
        let ip_cksum = icmp_checksum(&bytes[..24]);
        bytes[10..12].copy_from_slice(&ip_cksum.to_be_bytes());
        reply = ZCPacket::new_with_payload(&bytes);
        reply.fill_peer_manager_hdr(202, 101, PacketType::Data as u8);

        let (got, _rtt) = try_match_probe_reply(&state.sent, &reply).expect("must match");
        assert_eq!(got, key);
    }

    #[test]
    fn reply_is_claimed_only_from_the_probed_peer() {
        let state = VipProbeState::default();
        let ident = state.claim_ident();
        let key = link(202, 1);
        let seq = 3;
        state.sent.lock().insert(
            ident,
            InFlightProbe {
                key: key.clone(),
                seq,
                sent_at: std::time::Instant::now(),
            },
        );
        let request = echo_request("10.144.144.2", "10.144.144.1", ident, seq);

        // A request never matches.
        assert!(try_match_probe_reply(&state.sent, &request).is_none());

        // A reply from a peer we did NOT probe must not be claimed: this is the
        // spoofing guard (any member can echo our magic payload).
        let mut impostor = echo_reply_of(&request, "10.144.144.1", "10.144.144.9");
        impostor.fill_peer_manager_hdr(999, 101, PacketType::Data as u8);
        assert!(try_match_probe_reply(&state.sent, &impostor).is_none());

        // The real peer's reply is claimed, and only once.
        let mut reply = echo_reply_of(&request, "10.144.144.1", "10.144.144.2");
        reply.fill_peer_manager_hdr(202, 101, PacketType::Data as u8);
        let (got, _rtt) = try_match_probe_reply(&state.sent, &reply).expect("must match");
        assert_eq!(got, key);
        assert!(try_match_probe_reply(&state.sent, &reply).is_none());
    }

    #[test]
    fn reply_with_foreign_sequence_is_not_claimed() {
        let state = VipProbeState::default();
        let ident = state.claim_ident();
        state.sent.lock().insert(
            ident,
            InFlightProbe {
                key: link(202, 1),
                seq: 3,
                sent_at: std::time::Instant::now(),
            },
        );
        let request = echo_request("10.144.144.2", "10.144.144.1", ident, 99);
        let mut reply = echo_reply_of(&request, "10.144.144.1", "10.144.144.2");
        reply.fill_peer_manager_hdr(202, 101, PacketType::Data as u8);
        assert!(try_match_probe_reply(&state.sent, &reply).is_none());
    }

    #[test]
    fn unclaimed_ident_is_never_claimed() {
        let state = VipProbeState::default();
        let request = echo_request("10.144.144.2", "10.144.144.1", PROBE_IDENT_BASE + 5, 1);
        let mut reply = echo_reply_of(&request, "10.144.144.1", "10.144.144.2");
        reply.fill_peer_manager_hdr(202, 101, PacketType::Data as u8);
        assert!(try_match_probe_reply(&state.sent, &reply).is_none());
    }

    #[test]
    fn settle_round_marks_in_flight_probes_unreachable() {
        let state = VipProbeState::default();
        let ident = state.claim_ident();
        let key = link(7, 1);
        state.record_probing(key.clone());
        state.sent.lock().insert(
            ident,
            InFlightProbe {
                key: key.clone(),
                seq: 1,
                sent_at: std::time::Instant::now(),
            },
        );

        let mut sent = state.sent.lock();
        state.settle_round(&mut sent);
        drop(sent);

        let snap = snapshot_reachability(&state, [7], false);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].state, PeerVipState::Unreachable as i32);
        assert_eq!(snap[0].error, error::TIMEOUT);
        assert!(state.sent.lock().is_empty());
    }

    #[test]
    fn snapshot_emits_one_entry_per_link() {
        let state = VipProbeState::default();
        // Same peer, two links with different outcomes.
        state.record(
            link(1, 10),
            ProbeResult::new(PeerVipState::Reachable, Some(4), ""),
        );
        state.record(
            link(1, 11),
            ProbeResult::new(PeerVipState::Unreachable, None, error::TIMEOUT),
        );

        let snap = snapshot_reachability(&state, [1], false);
        assert_eq!(snap.len(), 2, "one entry per PeerConn");
        assert_eq!(snap[0].conn_id, uuid::Uuid::from_u128(10).to_string());
        assert_eq!(snap[0].state, PeerVipState::Reachable as i32);
        assert_eq!(snap[1].conn_id, uuid::Uuid::from_u128(11).to_string());
        assert_eq!(snap[1].state, PeerVipState::Unreachable as i32);
    }

    #[test]
    fn peer_level_fallback_entry_has_empty_conn_id() {
        let state = VipProbeState::default();
        state.record(
            LinkKey {
                peer_id: 5,
                conn_id: None,
            },
            ProbeResult::new(PeerVipState::Reachable, Some(9), ""),
        );
        let snap = snapshot_reachability(&state, [5], false);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].conn_id, "");
    }

    #[test]
    fn snapshot_reports_unknown_with_reason_when_disabled() {
        let state = VipProbeState::default();
        state.record(
            link(1, 10),
            ProbeResult::new(PeerVipState::Reachable, Some(5), ""),
        );
        let snap = snapshot_reachability(&state, [1], true);
        // Disabled wins over a stale result: never claim reachability we are no
        // longer measuring.
        assert_eq!(snap[0].state, PeerVipState::Unknown as i32);
        assert_eq!(snap[0].error, error::DISABLED);
    }

    #[test]
    fn gc_drops_results_for_departed_links() {
        let state = VipProbeState::default();
        state.record(
            link(1, 10),
            ProbeResult::new(PeerVipState::Reachable, Some(5), ""),
        );
        state.record(
            link(2, 20),
            ProbeResult::new(PeerVipState::Reachable, Some(5), ""),
        );
        state.gc(&[link(2, 20)]);
        assert!(snapshot_reachability(&state, [1], false).is_empty());
        assert_eq!(snapshot_reachability(&state, [2], false).len(), 1);
    }
}
