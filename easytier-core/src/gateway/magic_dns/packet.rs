use std::{
    future::Future,
    net::Ipv4Addr,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use smoltcp::wire::{
    IPV4_HEADER_LEN, Icmpv4Message, Icmpv4Packet, IpAddress, IpProtocol, Ipv4Packet,
    UDP_HEADER_LEN, UdpPacket,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    config::PeerId,
    packet::ZCPacket,
    peers::{BoxNicPacketFilter, NicFilterOutcome, NicPacketFilter, peer_manager::PeerManagerCore},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MagicDnsQuery {
    pub source: std::net::SocketAddr,
    pub payload: Vec<u8>,
}

#[async_trait]
pub trait MagicDnsQueryResolver: Send + Sync + 'static {
    async fn resolve(&self, query: MagicDnsQuery) -> Option<Vec<u8>>;
}

/// Owns one Magic DNS resolver installed in the core NIC pipeline.
///
/// `close` cancels absorb workers (so queued/in-flight queries are not injected
/// after teardown), then removes the NIC pipeline entry. Workers may still hold
/// the resolver `Arc` briefly while unwinding an in-flight `select!`.
pub struct MagicDnsResolverRegistration {
    peer_manager: Weak<PeerManagerCore>,
    pipeline: crate::peers::peer_manager::PipelineRegistrationGuard,
    absorb_cancel: CancellationToken,
    runtime: tokio::runtime::Handle,
}

impl MagicDnsResolverRegistration {
    pub(crate) fn new(
        peer_manager: Weak<PeerManagerCore>,
        pipeline: crate::peers::peer_manager::PipelineRegistrationGuard,
        absorb_cancel: CancellationToken,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            peer_manager,
            pipeline,
            absorb_cancel,
            runtime,
        }
    }

    pub async fn close(&self) {
        self.absorb_cancel.cancel();
        self.pipeline.close();
        if let Some(peer_manager) = self.peer_manager.upgrade() {
            peer_manager
                .remove_managed_nic_packet_process_pipeline(&self.pipeline)
                .await;
        }
    }
}

impl Drop for MagicDnsResolverRegistration {
    fn drop(&mut self) {
        self.absorb_cancel.cancel();
        self.pipeline.close();
        let Some(peer_manager) = self.peer_manager.upgrade() else {
            return;
        };
        let pipeline = self.pipeline.clone();
        self.runtime.spawn(async move {
            peer_manager
                .remove_managed_nic_packet_process_pipeline(&pipeline)
                .await;
        });
    }
}

/// Bound for one MagicDNS resolution (inline fallback or async worker).
///
/// Async mode absorbs the UDP query off the NIC pipeline first, so this budget
/// no longer stalls overlay traffic sharing the same worker — it only bounds
/// how long a single query waits before the client must retry.
const RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Pending UDP/53 queries waiting for off-pipeline resolve + inject.
const ABSORB_QUEUE_CAP: usize = 256;

/// Concurrent DNS resolve workers per MagicDNS NIC filter.
const ABSORB_WORKERS: usize = 2;

/// Escape hatch: force the pre-absorb inline await path
/// (`EASYTIER_MAGIC_DNS_DATAPLANE_INLINE=1`).
fn dataplane_inline_enabled() -> bool {
    static INLINE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *INLINE.get_or_init(|| {
        std::env::var("EASYTIER_MAGIC_DNS_DATAPLANE_INLINE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

#[derive(Default)]
struct MagicDnsDataplaneMetrics {
    absorb: AtomicU64,
    queue_overflow: AtomicU64,
    resolve_ok: AtomicU64,
    resolve_timeout: AtomicU64,
    resolve_none: AtomicU64,
    inject_ok: AtomicU64,
    inject_fail: AtomicU64,
}

#[cfg(test)]
impl MagicDnsDataplaneMetrics {
    fn snapshot(&self) -> MagicDnsDataplaneMetricSnapshot {
        MagicDnsDataplaneMetricSnapshot {
            absorb: self.absorb.load(Ordering::Relaxed),
            queue_overflow: self.queue_overflow.load(Ordering::Relaxed),
            resolve_ok: self.resolve_ok.load(Ordering::Relaxed),
            resolve_timeout: self.resolve_timeout.load(Ordering::Relaxed),
            resolve_none: self.resolve_none.load(Ordering::Relaxed),
            inject_ok: self.inject_ok.load(Ordering::Relaxed),
            inject_fail: self.inject_fail.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct MagicDnsDataplaneMetricSnapshot {
    absorb: u64,
    queue_overflow: u64,
    resolve_ok: u64,
    resolve_timeout: u64,
    resolve_none: u64,
    inject_ok: u64,
    inject_fail: u64,
}

struct AbsorbedDnsQuery {
    packet: ZCPacket,
    query: MagicDnsQuery,
    source_ip: Ipv4Addr,
    destination_ip: Ipv4Addr,
    source_port: u16,
    destination_port: u16,
    ip_header_length: usize,
}

struct MagicDnsPacketFilter {
    fake_ip: Ipv4Addr,
    my_peer_id: PeerId,
    resolver: Arc<dyn MagicDnsQueryResolver>,
    queue_tx: mpsc::Sender<AbsorbedDnsQuery>,
    metrics: Arc<MagicDnsDataplaneMetrics>,
}

/// Builds the NIC filter and returns a cancel token that registration must
/// cancel on close so queued / in-flight absorb work does not inject afterward.
pub(crate) fn magic_dns_packet_filter(
    fake_ip: Ipv4Addr,
    my_peer_id: PeerId,
    resolver: Arc<dyn MagicDnsQueryResolver>,
    peer_manager: Weak<PeerManagerCore>,
) -> (BoxNicPacketFilter, CancellationToken) {
    let (queue_tx, queue_rx) = mpsc::channel(ABSORB_QUEUE_CAP);
    let metrics = Arc::new(MagicDnsDataplaneMetrics::default());
    let absorb_cancel = CancellationToken::new();
    spawn_absorb_workers(
        queue_rx,
        resolver.clone(),
        peer_manager,
        my_peer_id,
        metrics.clone(),
        absorb_cancel.clone(),
    );
    (
        Box::new(MagicDnsPacketFilter {
            fake_ip,
            my_peer_id,
            resolver,
            queue_tx,
            metrics,
        }),
        absorb_cancel,
    )
}

fn spawn_absorb_workers(
    queue_rx: mpsc::Receiver<AbsorbedDnsQuery>,
    resolver: Arc<dyn MagicDnsQueryResolver>,
    peer_manager: Weak<PeerManagerCore>,
    my_peer_id: PeerId,
    metrics: Arc<MagicDnsDataplaneMetrics>,
    cancel: CancellationToken,
) {
    let queue_rx = Arc::new(tokio::sync::Mutex::new(queue_rx));
    for worker_id in 0..ABSORB_WORKERS {
        let queue_rx = queue_rx.clone();
        let resolver = resolver.clone();
        let peer_manager = peer_manager.clone();
        let metrics = metrics.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                if cancel.is_cancelled() {
                    tracing::debug!(worker_id, "magic dns absorb worker cancelled");
                    break;
                }
                let next = {
                    let mut guard = queue_rx.lock().await;
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => None,
                        next = guard.recv() => next,
                    }
                };
                let Some(job) = next else {
                    tracing::debug!(worker_id, "magic dns absorb worker exiting");
                    break;
                };
                handle_absorbed_query(
                    job,
                    resolver.as_ref(),
                    &peer_manager,
                    my_peer_id,
                    &metrics,
                    &cancel,
                )
                .await;
            }
            // Dropping the last receiver discards any remaining queued jobs.
        });
    }
}

async fn handle_absorbed_query(
    mut job: AbsorbedDnsQuery,
    resolver: &dyn MagicDnsQueryResolver,
    peer_manager: &Weak<PeerManagerCore>,
    my_peer_id: PeerId,
    metrics: &MagicDnsDataplaneMetrics,
    cancel: &CancellationToken,
) {
    if cancel.is_cancelled() {
        return;
    }

    let timed = tokio::select! {
        biased;
        _ = cancel.cancelled() => return,
        result = tokio::time::timeout(RESOLVE_TIMEOUT, resolver.resolve(job.query)) => result,
    };

    let response = match timed {
        Ok(Some(response)) => {
            metrics.resolve_ok.fetch_add(1, Ordering::Relaxed);
            response
        }
        Ok(None) => {
            metrics.resolve_none.fetch_add(1, Ordering::Relaxed);
            return;
        }
        Err(_) => {
            metrics.resolve_timeout.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                timeout = ?RESOLVE_TIMEOUT,
                "magic dns resolve timed out off data plane; dropping query"
            );
            return;
        }
    };

    if cancel.is_cancelled() {
        return;
    }

    if !apply_udp_response(
        &mut job.packet,
        job.source_ip,
        job.destination_ip,
        job.source_port,
        job.destination_port,
        job.ip_header_length,
        &response,
    ) {
        metrics.inject_fail.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if !finalize_reply_headers(
        &mut job.packet,
        job.source_ip,
        job.destination_ip,
        my_peer_id,
    ) {
        metrics.inject_fail.fetch_add(1, Ordering::Relaxed);
        return;
    }

    if cancel.is_cancelled() {
        return;
    }

    let Some(pm) = peer_manager.upgrade() else {
        metrics.inject_fail.fetch_add(1, Ordering::Relaxed);
        return;
    };
    match pm.inject_self_delivered_nic_packet(job.packet).await {
        Ok(()) => {
            metrics.inject_ok.fetch_add(1, Ordering::Relaxed);
        }
        Err(err) => {
            metrics.inject_fail.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(?err, "magic dns inject after resolve failed");
        }
    }
}

#[async_trait]
impl NicPacketFilter for MagicDnsPacketFilter {
    async fn try_process_packet_from_nic(&self, packet: &mut ZCPacket) -> NicFilterOutcome {
        match classify_magic_dns_packet(packet, self.fake_ip) {
            MagicDnsClassify::Pass => NicFilterOutcome::Pass,
            MagicDnsClassify::IcmpEcho {
                source_ip,
                destination_ip,
                ip_header_length,
            } => {
                let Ok(mut icmp_packet) =
                    Icmpv4Packet::new_checked(&mut packet.mut_payload()[ip_header_length..])
                else {
                    return NicFilterOutcome::Pass;
                };
                if icmp_packet.msg_type() != Icmpv4Message::EchoRequest {
                    return NicFilterOutcome::Pass;
                }
                icmp_packet.set_msg_type(Icmpv4Message::EchoReply);
                icmp_packet.fill_checksum();
                if !finalize_reply_headers(packet, source_ip, destination_ip, self.my_peer_id) {
                    return NicFilterOutcome::Pass;
                }
                NicFilterOutcome::Rewrite
            }
            MagicDnsClassify::UdpDns {
                source_ip,
                destination_ip,
                source_port,
                destination_port,
                ip_header_length,
                query,
            } => {
                if dataplane_inline_enabled() {
                    return inline_resolve_udp(
                        packet,
                        self.resolver.as_ref(),
                        self.my_peer_id,
                        source_ip,
                        destination_ip,
                        source_port,
                        destination_port,
                        ip_header_length,
                        query,
                        &self.metrics,
                    )
                    .await;
                }

                let absorbed = AbsorbedDnsQuery {
                    packet: packet.clone(),
                    query,
                    source_ip,
                    destination_ip,
                    source_port,
                    destination_port,
                    ip_header_length,
                };
                match self.queue_tx.try_send(absorbed) {
                    Ok(()) => {
                        self.metrics.absorb.fetch_add(1, Ordering::Relaxed);
                        NicFilterOutcome::Absorb
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        self.metrics.queue_overflow.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            cap = ABSORB_QUEUE_CAP,
                            "magic dns absorb queue full; dropping query"
                        );
                        // Still Absorb so the raw query is not forwarded as data.
                        NicFilterOutcome::Absorb
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        tracing::warn!("magic dns absorb queue closed; dropping query");
                        NicFilterOutcome::Absorb
                    }
                }
            }
        }
    }

    fn id(&self) -> String {
        "magic_dns_server".to_owned()
    }
}

#[allow(clippy::too_many_arguments)]
async fn inline_resolve_udp(
    packet: &mut ZCPacket,
    resolver: &dyn MagicDnsQueryResolver,
    my_peer_id: PeerId,
    source_ip: Ipv4Addr,
    destination_ip: Ipv4Addr,
    source_port: u16,
    destination_port: u16,
    ip_header_length: usize,
    query: MagicDnsQuery,
    metrics: &MagicDnsDataplaneMetrics,
) -> NicFilterOutcome {
    let response = match tokio::time::timeout(RESOLVE_TIMEOUT, resolver.resolve(query)).await {
        Ok(Some(response)) => {
            metrics.resolve_ok.fetch_add(1, Ordering::Relaxed);
            response
        }
        Ok(None) => {
            metrics.resolve_none.fetch_add(1, Ordering::Relaxed);
            return NicFilterOutcome::Pass;
        }
        Err(_) => {
            metrics.resolve_timeout.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                timeout = ?RESOLVE_TIMEOUT,
                "magic dns resolve timed out on the data plane; dropping query"
            );
            return NicFilterOutcome::Pass;
        }
    };
    if !apply_udp_response(
        packet,
        source_ip,
        destination_ip,
        source_port,
        destination_port,
        ip_header_length,
        &response,
    ) {
        return NicFilterOutcome::Pass;
    }
    if !finalize_reply_headers(packet, source_ip, destination_ip, my_peer_id) {
        return NicFilterOutcome::Pass;
    }
    NicFilterOutcome::Rewrite
}

enum MagicDnsClassify {
    Pass,
    IcmpEcho {
        source_ip: Ipv4Addr,
        destination_ip: Ipv4Addr,
        ip_header_length: usize,
    },
    UdpDns {
        source_ip: Ipv4Addr,
        destination_ip: Ipv4Addr,
        source_port: u16,
        destination_port: u16,
        ip_header_length: usize,
        query: MagicDnsQuery,
    },
}

fn classify_magic_dns_packet(packet: &ZCPacket, fake_ip: Ipv4Addr) -> MagicDnsClassify {
    if packet.peer_manager_header().is_none() {
        return MagicDnsClassify::Pass;
    }
    if packet.payload().len() < IPV4_HEADER_LEN {
        return MagicDnsClassify::Pass;
    }
    let ip_packet = Ipv4Packet::new_unchecked(packet.payload());
    if ip_packet.version() != 4 || ip_packet.dst_addr() != fake_ip {
        return MagicDnsClassify::Pass;
    }

    let ip_header_length = ip_packet.header_len() as usize;
    let ip_total_length = ip_packet.total_len() as usize;
    if ip_header_length < IPV4_HEADER_LEN
        || ip_header_length > ip_total_length
        || ip_total_length != packet.payload().len()
        || ip_packet.frag_offset() != 0
        || ip_packet.more_frags()
    {
        return MagicDnsClassify::Pass;
    }

    let protocol = ip_packet.next_header();
    let source_ip = ip_packet.src_addr();
    let destination_ip = ip_packet.dst_addr();

    match protocol {
        IpProtocol::Udp => {
            let ip_payload = &packet.payload()[ip_header_length..ip_total_length];
            if ip_payload.len() < UDP_HEADER_LEN {
                return MagicDnsClassify::Pass;
            }
            let udp_packet = UdpPacket::new_unchecked(ip_payload);
            let udp_length = udp_packet.len() as usize;
            if udp_length != ip_payload.len() || udp_length < UDP_HEADER_LEN {
                return MagicDnsClassify::Pass;
            }
            if udp_packet.dst_port() != 53 {
                return MagicDnsClassify::Pass;
            }
            let source_port = udp_packet.src_port();
            let destination_port = udp_packet.dst_port();
            MagicDnsClassify::UdpDns {
                source_ip,
                destination_ip,
                source_port,
                destination_port,
                ip_header_length,
                query: MagicDnsQuery {
                    source: std::net::SocketAddr::from((source_ip, source_port)),
                    payload: udp_packet.payload().to_vec(),
                },
            }
        }
        IpProtocol::Icmp => MagicDnsClassify::IcmpEcho {
            source_ip,
            destination_ip,
            ip_header_length,
        },
        _ => MagicDnsClassify::Pass,
    }
}

fn finalize_reply_headers(
    packet: &mut ZCPacket,
    source_ip: Ipv4Addr,
    destination_ip: Ipv4Addr,
    my_peer_id: PeerId,
) -> bool {
    let mut ip_packet = Ipv4Packet::new_unchecked(packet.mut_payload());
    ip_packet.set_src_addr(destination_ip);
    ip_packet.set_dst_addr(source_ip);
    ip_packet.fill_checksum();
    let payload_length = packet.payload().len() as u32;
    let Some(header) = packet.mut_peer_manager_header() else {
        return false;
    };
    header.to_peer_id = my_peer_id.into();
    header.len.set(payload_length);
    true
}

/// Test / inline helper: await resolve on the calling task and rewrite in place.
///
/// Production NIC path uses absorb + workers instead (unless the inline env
/// escape hatch is set).
pub async fn process_magic_dns_packet<F, Fut>(
    packet: &mut ZCPacket,
    fake_ip: Ipv4Addr,
    my_peer_id: PeerId,
    resolve: F,
) -> bool
where
    F: FnOnce(MagicDnsQuery) -> Fut,
    Fut: Future<Output = Option<Vec<u8>>>,
{
    match classify_magic_dns_packet(packet, fake_ip) {
        MagicDnsClassify::Pass => false,
        MagicDnsClassify::IcmpEcho {
            source_ip,
            destination_ip,
            ip_header_length,
        } => {
            let Ok(mut icmp_packet) =
                Icmpv4Packet::new_checked(&mut packet.mut_payload()[ip_header_length..])
            else {
                return false;
            };
            if icmp_packet.msg_type() != Icmpv4Message::EchoRequest {
                return false;
            }
            icmp_packet.set_msg_type(Icmpv4Message::EchoReply);
            icmp_packet.fill_checksum();
            finalize_reply_headers(packet, source_ip, destination_ip, my_peer_id)
        }
        MagicDnsClassify::UdpDns {
            source_ip,
            destination_ip,
            source_port,
            destination_port,
            ip_header_length,
            query,
        } => {
            let Some(response) = resolve(query).await else {
                return false;
            };
            if !apply_udp_response(
                packet,
                source_ip,
                destination_ip,
                source_port,
                destination_port,
                ip_header_length,
                &response,
            ) {
                return false;
            }
            finalize_reply_headers(packet, source_ip, destination_ip, my_peer_id)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_udp_response(
    packet: &mut ZCPacket,
    source_ip: Ipv4Addr,
    destination_ip: Ipv4Addr,
    source_port: u16,
    destination_port: u16,
    ip_header_length: usize,
    response: &[u8],
) -> bool {
    let Some(udp_length) = UDP_HEADER_LEN.checked_add(response.len()) else {
        return false;
    };
    let Some(ip_length) = ip_header_length.checked_add(udp_length) else {
        return false;
    };
    if ip_length > u16::MAX as usize {
        return false;
    }
    let Some(header_length) = packet.buf_len().checked_sub(packet.payload().len()) else {
        return false;
    };
    let Some(inner_length) = header_length.checked_add(ip_length) else {
        return false;
    };

    if packet.mut_inner().capacity() < inner_length {
        packet
            .mut_inner()
            .truncate(header_length + ip_header_length + UDP_HEADER_LEN);
    }
    packet.mut_inner().resize(inner_length, 0);

    let mut ip_packet = Ipv4Packet::new_unchecked(packet.mut_payload());
    ip_packet.set_total_len(ip_length as u16);
    let mut udp_packet = UdpPacket::new_unchecked(ip_packet.payload_mut());
    udp_packet.set_len(udp_length as u16);
    udp_packet.set_src_port(destination_port);
    udp_packet.set_dst_port(source_port);
    udp_packet.payload_mut().copy_from_slice(response);
    udp_packet.fill_checksum(
        &IpAddress::Ipv4(destination_ip),
        &IpAddress::Ipv4(source_ip),
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn udp_query(payload: &[u8], destination_port: u16) -> ZCPacket {
        let mut bytes = vec![0; 20 + 8 + payload.len()];
        {
            let mut ip = Ipv4Packet::new_unchecked(&mut bytes);
            ip.set_version(4);
            ip.set_header_len(20);
            ip.set_total_len((20 + 8 + payload.len()) as u16);
            ip.set_next_header(IpProtocol::Udp);
            ip.set_src_addr("10.0.0.2".parse().unwrap());
            ip.set_dst_addr("10.255.255.254".parse().unwrap());
            let mut udp = UdpPacket::new_unchecked(ip.payload_mut());
            udp.set_src_port(53000);
            udp.set_dst_port(destination_port);
            udp.set_len((8 + payload.len()) as u16);
            udp.payload_mut().copy_from_slice(payload);
        }
        ZCPacket::new_with_payload(&bytes)
    }

    fn icmp_echo_request() -> ZCPacket {
        let mut bytes = vec![0; 20 + 8];
        {
            let mut ip = Ipv4Packet::new_unchecked(&mut bytes);
            ip.set_version(4);
            ip.set_header_len(20);
            ip.set_total_len(28);
            ip.set_next_header(IpProtocol::Icmp);
            ip.set_src_addr("10.0.0.2".parse().unwrap());
            ip.set_dst_addr("10.255.255.254".parse().unwrap());
            let mut icmp = Icmpv4Packet::new_unchecked(ip.payload_mut());
            icmp.set_msg_type(Icmpv4Message::EchoRequest);
        }
        ZCPacket::new_with_payload(&bytes)
    }

    struct ImmediateResolver;

    #[async_trait]
    impl MagicDnsQueryResolver for ImmediateResolver {
        async fn resolve(&self, query: MagicDnsQuery) -> Option<Vec<u8>> {
            Some(format!("echo:{}", String::from_utf8_lossy(&query.payload)).into_bytes())
        }
    }

    struct BlockingResolver {
        entered: Arc<AtomicBool>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl MagicDnsQueryResolver for BlockingResolver {
        async fn resolve(&self, _query: MagicDnsQuery) -> Option<Vec<u8>> {
            self.entered.store(true, Ordering::SeqCst);
            self.release.notified().await;
            Some(b"slow".to_vec())
        }
    }

    #[tokio::test]
    async fn packet_engine_rewrites_dns_query_response() {
        let mut packet = udp_query(b"query", 53);
        let handled = process_magic_dns_packet(
            &mut packet,
            "10.255.255.254".parse().unwrap(),
            42,
            |query| async move {
                assert_eq!(query.source, "10.0.0.2:53000".parse().unwrap());
                assert_eq!(query.payload, b"query");
                Some(b"response".to_vec())
            },
        )
        .await;

        assert!(handled);
        let ip = Ipv4Packet::new_checked(packet.payload()).unwrap();
        assert_eq!(ip.src_addr(), "10.255.255.254".parse::<Ipv4Addr>().unwrap());
        assert_eq!(ip.dst_addr(), "10.0.0.2".parse::<Ipv4Addr>().unwrap());
        let udp = UdpPacket::new_checked(ip.payload()).unwrap();
        assert_eq!(udp.src_port(), 53);
        assert_eq!(udp.dst_port(), 53000);
        assert_eq!(udp.payload(), b"response");
        assert_eq!(packet.get_dst_peer_id(), Some(42));
        assert_eq!(
            packet.peer_manager_header().unwrap().len.get() as usize,
            packet.payload().len()
        );
    }

    #[tokio::test]
    async fn packet_engine_rejects_invalid_ipv4_header_without_mutation() {
        let mut packet = udp_query(b"query", 53);
        Ipv4Packet::new_unchecked(packet.mut_payload()).set_header_len(60);
        let original = packet.payload().to_vec();

        assert!(
            !process_magic_dns_packet(
                &mut packet,
                "10.255.255.254".parse().unwrap(),
                42,
                |_| async { panic!("invalid IPv4 header must not invoke DNS") },
            )
            .await
        );
        assert_eq!(packet.payload(), original);
    }

    #[tokio::test]
    async fn packet_engine_rejects_short_zc_packet_without_panicking() {
        let mut packet =
            ZCPacket::new_from_buf(Default::default(), crate::packet::ZCPacketType::NIC);

        assert!(
            !process_magic_dns_packet(
                &mut packet,
                "10.255.255.254".parse().unwrap(),
                42,
                |_| async { panic!("short packet must not invoke DNS") },
            )
            .await
        );
    }

    #[tokio::test]
    async fn packet_engine_rejects_inconsistent_udp_length_without_mutation() {
        let mut packet = udp_query(b"query", 53);
        let mut ip = Ipv4Packet::new_unchecked(packet.mut_payload());
        UdpPacket::new_unchecked(ip.payload_mut()).set_len(8);
        let original = packet.payload().to_vec();

        assert!(
            !process_magic_dns_packet(
                &mut packet,
                "10.255.255.254".parse().unwrap(),
                42,
                |_| async { panic!("invalid UDP length must not invoke DNS") },
            )
            .await
        );
        assert_eq!(packet.payload(), original);
    }

    #[tokio::test]
    async fn packet_engine_rejects_fragmented_packets_without_mutation() {
        let mut packet = udp_query(b"query", 53);
        Ipv4Packet::new_unchecked(packet.mut_payload()).set_more_frags(true);
        let original = packet.payload().to_vec();

        assert!(
            !process_magic_dns_packet(
                &mut packet,
                "10.255.255.254".parse().unwrap(),
                42,
                |_| async { panic!("fragmented packet must not invoke DNS") },
            )
            .await
        );
        assert_eq!(packet.payload(), original);
    }

    #[tokio::test]
    async fn packet_engine_rejects_oversized_response_without_mutation() {
        let mut packet = udp_query(b"query", 53);
        let original = packet.payload().to_vec();
        let original_length = packet.buf_len();

        assert!(
            !process_magic_dns_packet(
                &mut packet,
                "10.255.255.254".parse().unwrap(),
                42,
                |_| async { Some(vec![0; u16::MAX as usize]) },
            )
            .await
        );
        assert_eq!(packet.buf_len(), original_length);
        assert_eq!(packet.payload(), original);
    }

    #[tokio::test]
    async fn packet_engine_replies_to_icmp_without_calling_dns() {
        let mut packet = icmp_echo_request();
        let handled = process_magic_dns_packet(
            &mut packet,
            "10.255.255.254".parse().unwrap(),
            7,
            |_| async { panic!("ICMP must not invoke DNS") },
        )
        .await;

        assert!(handled);
        let ip = Ipv4Packet::new_checked(packet.payload()).unwrap();
        assert_eq!(ip.src_addr(), "10.255.255.254".parse::<Ipv4Addr>().unwrap());
        let icmp = Icmpv4Packet::new_checked(ip.payload()).unwrap();
        assert_eq!(icmp.msg_type(), Icmpv4Message::EchoReply);
        assert_eq!(packet.get_dst_peer_id(), Some(7));
    }

    #[tokio::test]
    async fn packet_engine_ignores_non_dns_udp() {
        let mut packet = udp_query(b"query", 5353);
        assert!(
            !process_magic_dns_packet(
                &mut packet,
                "10.255.255.254".parse().unwrap(),
                42,
                |_| async { Some(Vec::new()) },
            )
            .await
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nic_filter_absorbs_udp53_without_awaiting_resolver() {
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(tokio::sync::Notify::new());
        let resolver: Arc<dyn MagicDnsQueryResolver> = Arc::new(BlockingResolver {
            entered: entered.clone(),
            release: release.clone(),
        });
        let (queue_tx, mut queue_rx) = mpsc::channel(ABSORB_QUEUE_CAP);
        let metrics = Arc::new(MagicDnsDataplaneMetrics::default());
        let filter = MagicDnsPacketFilter {
            fake_ip: "10.255.255.254".parse().unwrap(),
            my_peer_id: 1,
            resolver,
            queue_tx,
            metrics: metrics.clone(),
        };

        let mut packet = udp_query(b"query", 53);
        let outcome = filter.try_process_packet_from_nic(&mut packet).await;
        assert_eq!(outcome, NicFilterOutcome::Absorb);
        assert!(
            !entered.load(Ordering::SeqCst),
            "resolver must not run on the NIC filter task"
        );
        assert_eq!(metrics.snapshot().absorb, 1);

        let job = tokio::time::timeout(std::time::Duration::from_millis(200), queue_rx.recv())
            .await
            .expect("queue recv timed out")
            .expect("absorbed job");
        assert_eq!(job.query.payload, b"query");
        release.notify_one();
    }

    #[tokio::test]
    async fn nic_filter_rewrites_icmp_synchronously() {
        let (queue_tx, _queue_rx) = mpsc::channel(ABSORB_QUEUE_CAP);
        let filter = MagicDnsPacketFilter {
            fake_ip: "10.255.255.254".parse().unwrap(),
            my_peer_id: 9,
            resolver: Arc::new(ImmediateResolver),
            queue_tx,
            metrics: Arc::new(MagicDnsDataplaneMetrics::default()),
        };
        let mut packet = icmp_echo_request();
        let outcome = filter.try_process_packet_from_nic(&mut packet).await;
        assert_eq!(outcome, NicFilterOutcome::Rewrite);
        assert_eq!(packet.get_dst_peer_id(), Some(9));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn absorb_cancel_aborts_in_flight_resolve_without_inject() {
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(tokio::sync::Notify::new());
        let resolver: Arc<dyn MagicDnsQueryResolver> = Arc::new(BlockingResolver {
            entered: entered.clone(),
            release: release.clone(),
        });
        let metrics = Arc::new(MagicDnsDataplaneMetrics::default());
        let cancel = CancellationToken::new();
        let (tx, rx) = mpsc::channel(ABSORB_QUEUE_CAP);
        spawn_absorb_workers(
            rx,
            resolver,
            Weak::new(),
            1,
            metrics.clone(),
            cancel.clone(),
        );

        tx.try_send(AbsorbedDnsQuery {
            packet: udp_query(b"query", 53),
            query: MagicDnsQuery {
                source: "10.0.0.2:53000".parse().unwrap(),
                payload: b"query".to_vec(),
            },
            source_ip: "10.0.0.2".parse().unwrap(),
            destination_ip: "10.255.255.254".parse().unwrap(),
            source_port: 53000,
            destination_port: 53,
            ip_header_length: 20,
        })
        .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("resolver should start before cancel");

        cancel.cancel();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let snap = metrics.snapshot();
        assert_eq!(snap.resolve_ok, 0, "cancel must abort before resolve completes");
        assert_eq!(snap.inject_ok, 0);
        assert_eq!(snap.inject_fail, 0);
        release.notify_one();
    }
}
