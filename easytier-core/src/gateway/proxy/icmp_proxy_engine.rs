use std::{
    net::Ipv4Addr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use dashmap::DashMap;
use quanta::Instant;
use smoltcp::wire::{IPV4_HEADER_LEN, Icmpv4Message, Icmpv4Packet, Ipv4Packet};

use crate::packet::{PacketType, ZCPacket};

use super::{
    cidr_table::ProxyCidrTable,
    ip_reassembler::{ComposeIpv4PacketArgs, IpProtocol, IpReassembler, compose_ipv4_packet},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IcmpProxyContext {
    pub virtual_ipv4: Option<Ipv4Addr>,
    pub enable_exit_node: bool,
    pub no_tun: bool,
}

#[derive(Debug)]
pub enum IcmpProxyAction {
    Pass,
    SendToSocket {
        destination: Ipv4Addr,
        packet: Vec<u8>,
    },
    SendToPeer(Vec<ZCPacket>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct IcmpNatKey {
    real_destination: Ipv4Addr,
    identifier: u16,
    sequence: u16,
}

#[derive(Debug)]
struct IcmpNatEntry {
    source_peer_id: u32,
    local_peer_id: u32,
    source_ip: Ipv4Addr,
    mapped_destination: Ipv4Addr,
    started_at: Instant,
}

const ICMP_ECHO_HEADER_LEN: usize = 8;

#[derive(Debug)]
pub struct IcmpProxyEngine {
    cidr_table: Arc<ProxyCidrTable>,
    nat_table: DashMap<IcmpNatKey, IcmpNatEntry>,
    reassembler: IpReassembler,
    /// Cross-peer NAT key collisions (B3). `warn!` at most once per 60s;
    /// intervening hits stay at `debug!` to avoid log DoS under dual-peer echo.
    nat_collision_count: AtomicU64,
    nat_collision_last_warn_ms: AtomicU64,
}

impl IcmpProxyEngine {
    pub fn new(cidr_table: Arc<ProxyCidrTable>, fragment_timeout: Duration) -> Self {
        Self {
            cidr_table,
            nat_table: DashMap::new(),
            reassembler: IpReassembler::new(fragment_timeout),
            nat_collision_count: AtomicU64::new(0),
            nat_collision_last_warn_ms: AtomicU64::new(0),
        }
    }

    fn should_warn_nat_collision(&self) -> bool {
        use std::time::{SystemTime, UNIX_EPOCH};
        const INTERVAL_MS: u64 = 60_000;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let prev = self.nat_collision_last_warn_ms.load(Ordering::Relaxed);
        if now.saturating_sub(prev) >= INTERVAL_MS {
            self.nat_collision_last_warn_ms
                .store(now, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn handle_peer_packet(
        &self,
        packet: &ZCPacket,
        context: IcmpProxyContext,
    ) -> IcmpProxyAction {
        if self.cidr_table.is_empty() && !context.enable_exit_node && !context.no_tun {
            return IcmpProxyAction::Pass;
        }
        let Some(virtual_ipv4) = context.virtual_ipv4 else {
            return IcmpProxyAction::Pass;
        };
        let Some(header) = packet.peer_manager_header() else {
            return IcmpProxyAction::Pass;
        };
        if header.packet_type != PacketType::Data as u8 || header.is_no_proxy() {
            return IcmpProxyAction::Pass;
        }
        let Ok(ipv4) = Ipv4Packet::new_checked(packet.payload()) else {
            return IcmpProxyAction::Pass;
        };
        if ipv4.version() != 4
            || usize::from(ipv4.header_len()) < IPV4_HEADER_LEN
            || ipv4.next_header() != IpProtocol::Icmp
        {
            return IcmpProxyAction::Pass;
        }

        let mapped_destination = ipv4.dst_addr();
        let real_destination = self.cidr_table.lookup_v4(mapped_destination);
        let is_local_no_tun = context.no_tun && mapped_destination == virtual_ipv4;
        // Subnet-proxy hits (cidr lookup) do not need enable_exit_node; exit-flagged
        // internet traffic does. Aligns with traffic-steering.md / B4.
        if real_destination.is_none() {
            if is_local_no_tun {
                // fall through to local_reply below
            } else if !header.is_exit_node() || !context.enable_exit_node {
                return IcmpProxyAction::Pass;
            }
        }

        let reassembled;
        let request_bytes = if IpReassembler::is_packet_fragmented(&ipv4) {
            reassembled = self.reassembler.add_fragment(&ipv4);
            let Some(reassembled) = reassembled.as_ref() else {
                return IcmpProxyAction::Pass;
            };
            reassembled.as_slice()
        } else {
            ipv4.payload()
        };
        if request_bytes.len() < ICMP_ECHO_HEADER_LEN {
            return IcmpProxyAction::Pass;
        }
        let request = Icmpv4Packet::new_unchecked(request_bytes);
        if request.msg_type() != Icmpv4Message::EchoRequest {
            return IcmpProxyAction::Pass;
        }

        if is_local_no_tun {
            return self.local_reply(
                mapped_destination,
                ipv4.src_addr(),
                header.to_peer_id.get(),
                header.from_peer_id.get(),
                &request,
            );
        }

        let real_destination = real_destination.unwrap_or(mapped_destination);
        let key = IcmpNatKey {
            real_destination,
            identifier: request.echo_ident(),
            sequence: request.echo_seq_no(),
        };
        let source_peer_id = header.from_peer_id.get();
        let local_peer_id = header.to_peer_id.get();
        let source_ip = ipv4.src_addr();
        // B3 observability: key has no source; refuse silent overwrite by another peer.
        // Same-source refresh is allowed. Full demux still needs ident rewrite / src NAT.
        match self.nat_table.entry(key) {
            dashmap::mapref::entry::Entry::Occupied(mut occupied) => {
                if occupied.get().source_peer_id != source_peer_id {
                    let n = self.nat_collision_count.fetch_add(1, Ordering::Relaxed) + 1;
                    if self.should_warn_nat_collision() {
                        tracing::warn!(
                            ?key,
                            existing_peer = occupied.get().source_peer_id,
                            new_peer = source_peer_id,
                            collision_count = n,
                            "icmp nat key collision; keeping first entry"
                        );
                    } else {
                        tracing::debug!(
                            ?key,
                            existing_peer = occupied.get().source_peer_id,
                            new_peer = source_peer_id,
                            collision_count = n,
                            "icmp nat key collision; keeping first entry"
                        );
                    }
                    return IcmpProxyAction::Pass;
                }
                occupied.get_mut().local_peer_id = local_peer_id;
                occupied.get_mut().source_ip = source_ip;
                occupied.get_mut().mapped_destination = mapped_destination;
                occupied.get_mut().started_at = Instant::now();
            }
            dashmap::mapref::entry::Entry::Vacant(vacant) => {
                vacant.insert(IcmpNatEntry {
                    source_peer_id,
                    local_peer_id,
                    source_ip,
                    mapped_destination,
                    started_at: Instant::now(),
                });
            }
        }

        IcmpProxyAction::SendToSocket {
            destination: real_destination,
            packet: request.as_ref().to_vec(),
        }
    }

    pub fn handle_socket_response(&self, peer_ip: Ipv4Addr, packet: &mut [u8]) -> Vec<ZCPacket> {
        let Ok(ipv4) = Ipv4Packet::new_checked(&*packet) else {
            return Vec::new();
        };
        if usize::from(ipv4.header_len()) < IPV4_HEADER_LEN
            || ipv4.payload().len() < ICMP_ECHO_HEADER_LEN
        {
            return Vec::new();
        }
        let reply = Icmpv4Packet::new_unchecked(ipv4.payload());
        if reply.msg_type() != Icmpv4Message::EchoReply {
            return Vec::new();
        }
        let key = IcmpNatKey {
            real_destination: peer_ip,
            identifier: reply.echo_ident(),
            sequence: reply.echo_seq_no(),
        };
        let Some((_, entry)) = self.nat_table.remove(&key) else {
            return Vec::new();
        };
        let Some(payload_len) = packet.len().checked_sub(ipv4.header_len() as usize) else {
            return Vec::new();
        };
        let ip_id = ipv4.ident();
        let mut responses = Vec::new();
        let _ = compose_ipv4_packet(
            ComposeIpv4PacketArgs {
                buf: packet,
                src_v4: &entry.mapped_destination,
                dst_v4: &entry.source_ip,
                next_protocol: IpProtocol::Icmp,
                payload_len,
                payload_mtu: 1200,
                ip_id,
            },
            |buf| {
                let mut packet = ZCPacket::new_with_payload(buf);
                packet.fill_peer_manager_hdr(
                    entry.local_peer_id,
                    entry.source_peer_id,
                    PacketType::Data as u8,
                );
                packet
                    .mut_peer_manager_header()
                    .expect("peer manager header")
                    .set_no_proxy(true);
                responses.push(packet);
                Ok(())
            },
        );
        responses
    }

    pub fn remove_expired_entries(&self, max_age: Duration) {
        self.nat_table
            .retain(|_, entry| entry.started_at.elapsed() < max_age);
        self.nat_table.shrink_to_fit();
    }

    pub fn remove_expired_fragments(&self) {
        self.reassembler.remove_expired_packets();
    }

    fn local_reply(
        &self,
        source: Ipv4Addr,
        destination: Ipv4Addr,
        source_peer_id: u32,
        destination_peer_id: u32,
        request: &Icmpv4Packet<&[u8]>,
    ) -> IcmpProxyAction {
        let mut buffer = vec![0_u8; request.as_ref().len() + 20];
        let mut reply = Icmpv4Packet::new_unchecked(&mut buffer[20..]);
        reply.set_msg_type(Icmpv4Message::EchoReply);
        reply.set_msg_code(0);
        reply.set_echo_ident(request.echo_ident());
        reply.set_echo_seq_no(request.echo_seq_no());
        reply.data_mut().copy_from_slice(request.data());
        reply.fill_checksum();

        let payload_len = buffer.len() - 20;
        let mut responses = Vec::new();
        let _ = compose_ipv4_packet(
            ComposeIpv4PacketArgs {
                buf: &mut buffer,
                src_v4: &source,
                dst_v4: &destination,
                next_protocol: IpProtocol::Icmp,
                payload_len,
                payload_mtu: 1200,
                ip_id: rand::random(),
            },
            |buf| {
                let mut packet = ZCPacket::new_with_payload(buf);
                packet.fill_peer_manager_hdr(
                    source_peer_id,
                    destination_peer_id,
                    PacketType::Data as u8,
                );
                responses.push(packet);
                Ok(())
            },
        );
        IcmpProxyAction::SendToPeer(responses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::proxy::cidr_table::{ProxyCidrRule, ProxyCidrSnapshot};

    fn echo_request_with_payload(
        source: Ipv4Addr,
        destination: Ipv4Addr,
        payload: &[u8],
    ) -> ZCPacket {
        let mut bytes = vec![0_u8; 20 + 8 + payload.len()];
        {
            let mut request = Icmpv4Packet::new_unchecked(&mut bytes[20..]);
            request.set_msg_type(Icmpv4Message::EchoRequest);
            request.set_echo_ident(7);
            request.set_echo_seq_no(11);
            request.data_mut().copy_from_slice(payload);
            request.fill_checksum();
        }
        {
            let packet_len = bytes.len() as u16;
            let mut ipv4 = Ipv4Packet::new_unchecked(&mut bytes);
            ipv4.set_version(4);
            ipv4.set_header_len(20);
            ipv4.set_total_len(packet_len);
            ipv4.set_hop_limit(64);
            ipv4.set_next_header(IpProtocol::Icmp);
            ipv4.set_src_addr(source);
            ipv4.set_dst_addr(destination);
            ipv4.fill_checksum();
        }
        let mut packet = ZCPacket::new_with_payload(&bytes);
        packet.fill_peer_manager_hdr(101, 202, PacketType::Data as u8);
        packet
    }

    fn echo_request(source: Ipv4Addr, destination: Ipv4Addr) -> ZCPacket {
        echo_request_with_payload(source, destination, b"ping")
    }

    fn engine(rule: Option<ProxyCidrRule>) -> IcmpProxyEngine {
        let table = ProxyCidrTable::from_snapshot(ProxyCidrSnapshot {
            rules: rule.into_iter().collect(),
        });
        IcmpProxyEngine::new(Arc::new(table), Duration::from_secs(10))
    }

    #[test]
    fn inactive_proxy_passes_echo_request() {
        let engine = engine(None);
        let packet = echo_request("10.0.0.2".parse().unwrap(), "192.0.2.2".parse().unwrap());

        assert!(matches!(
            engine.handle_peer_packet(
                &packet,
                IcmpProxyContext {
                    virtual_ipv4: Some("10.0.0.1".parse().unwrap()),
                    ..Default::default()
                }
            ),
            IcmpProxyAction::Pass
        ));
    }

    #[test]
    fn exit_flagged_internet_requires_enable_exit_node() {
        let engine = engine(Some(ProxyCidrRule {
            cidr: "10.20.0.0/24".parse().unwrap(),
            mapped_cidr: None,
        }));
        let mut packet = echo_request("10.0.0.2".parse().unwrap(), "8.8.8.8".parse().unwrap());
        packet
            .mut_peer_manager_header()
            .unwrap()
            .set_exit_node(true);

        assert!(matches!(
            engine.handle_peer_packet(
                &packet,
                IcmpProxyContext {
                    virtual_ipv4: Some("10.0.0.1".parse().unwrap()),
                    enable_exit_node: false,
                    ..Default::default()
                }
            ),
            IcmpProxyAction::Pass
        ));

        assert!(matches!(
            engine.handle_peer_packet(
                &packet,
                IcmpProxyContext {
                    virtual_ipv4: Some("10.0.0.1".parse().unwrap()),
                    enable_exit_node: true,
                    ..Default::default()
                }
            ),
            IcmpProxyAction::SendToSocket { .. }
        ));
    }

    #[test]
    fn icmp_nat_rejects_cross_peer_key_collision() {
        let engine = engine(Some(ProxyCidrRule {
            cidr: "127.0.0.0/24".parse().unwrap(),
            mapped_cidr: Some("10.10.10.0/24".parse().unwrap()),
        }));
        let first = echo_request("10.0.0.2".parse().unwrap(), "10.10.10.42".parse().unwrap());
        assert!(matches!(
            engine.handle_peer_packet(
                &first,
                IcmpProxyContext {
                    virtual_ipv4: Some("10.0.0.1".parse().unwrap()),
                    ..Default::default()
                }
            ),
            IcmpProxyAction::SendToSocket { .. }
        ));

        let mut second = echo_request("10.0.0.3".parse().unwrap(), "10.10.10.42".parse().unwrap());
        second
            .mut_peer_manager_header()
            .unwrap()
            .from_peer_id
            .set(303);
        assert!(matches!(
            engine.handle_peer_packet(
                &second,
                IcmpProxyContext {
                    virtual_ipv4: Some("10.0.0.1".parse().unwrap()),
                    ..Default::default()
                }
            ),
            IcmpProxyAction::Pass
        ));
        let entry = engine
            .nat_table
            .get(&IcmpNatKey {
                real_destination: "127.0.0.42".parse().unwrap(),
                identifier: 7,
                sequence: 11,
            })
            .unwrap();
        assert_eq!(entry.source_peer_id, 101);
        assert_eq!(entry.source_ip, "10.0.0.2".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn no_tun_local_request_returns_echo_reply_to_origin_peer() {
        let engine = engine(None);
        let packet = echo_request("10.0.0.2".parse().unwrap(), "10.0.0.1".parse().unwrap());

        let IcmpProxyAction::SendToPeer(replies) = engine.handle_peer_packet(
            &packet,
            IcmpProxyContext {
                virtual_ipv4: Some("10.0.0.1".parse().unwrap()),
                no_tun: true,
                ..Default::default()
            },
        ) else {
            panic!("expected local reply");
        };
        let [reply] = replies.as_slice() else {
            panic!("expected one local reply");
        };
        let header = reply.peer_manager_header().unwrap();
        assert_eq!(header.from_peer_id.get(), 202);
        assert_eq!(header.to_peer_id.get(), 101);
        let ipv4 = Ipv4Packet::new_checked(reply.payload()).unwrap();
        assert_eq!(ipv4.src_addr(), "10.0.0.1".parse::<Ipv4Addr>().unwrap());
        assert_eq!(ipv4.dst_addr(), "10.0.0.2".parse::<Ipv4Addr>().unwrap());
        let reply = Icmpv4Packet::new_checked(ipv4.payload()).unwrap();
        assert_eq!(reply.echo_ident(), 7);
        assert_eq!(reply.echo_seq_no(), 11);
        assert_eq!(reply.data(), b"ping");
    }

    #[test]
    fn peer_packet_rejects_ipv4_header_shorter_than_minimum() {
        let engine = engine(None);
        let mut packet = echo_request("10.0.0.2".parse().unwrap(), "8.0.0.1".parse().unwrap());
        Ipv4Packet::new_unchecked(packet.mut_payload()).set_header_len(16);

        assert!(matches!(
            engine.handle_peer_packet(
                &packet,
                IcmpProxyContext {
                    virtual_ipv4: Some("8.0.0.1".parse().unwrap()),
                    no_tun: true,
                    ..Default::default()
                }
            ),
            IcmpProxyAction::Pass
        ));
    }

    #[test]
    fn mapped_request_and_socket_reply_round_trip() {
        let engine = engine(Some(ProxyCidrRule {
            cidr: "127.0.0.0/24".parse().unwrap(),
            mapped_cidr: Some("10.10.10.0/24".parse().unwrap()),
        }));
        let packet = echo_request("10.0.0.2".parse().unwrap(), "10.10.10.42".parse().unwrap());

        let IcmpProxyAction::SendToSocket {
            destination,
            packet: request,
        } = engine.handle_peer_packet(
            &packet,
            IcmpProxyContext {
                virtual_ipv4: Some("10.0.0.1".parse().unwrap()),
                ..Default::default()
            },
        )
        else {
            panic!("expected socket request");
        };
        assert_eq!(destination, "127.0.0.42".parse::<Ipv4Addr>().unwrap());
        let request = Icmpv4Packet::new_checked(&request).unwrap();
        assert_eq!(request.data(), b"ping");

        let mut response = echo_request(destination, "10.0.0.1".parse().unwrap())
            .payload()
            .to_vec();
        {
            let mut ipv4 = Ipv4Packet::new_unchecked(&mut response);
            let mut reply = Icmpv4Packet::new_unchecked(ipv4.payload_mut());
            reply.set_msg_type(Icmpv4Message::EchoReply);
            reply.fill_checksum();
            ipv4.set_src_addr(destination);
            ipv4.fill_checksum();
        }
        let replies = engine.handle_socket_response(destination, &mut response);
        let [reply] = replies.as_slice() else {
            panic!("expected one socket reply");
        };
        let header = reply.peer_manager_header().unwrap();
        assert_eq!(header.from_peer_id.get(), 202);
        assert_eq!(header.to_peer_id.get(), 101);
        assert!(header.is_no_proxy());
        let ipv4 = Ipv4Packet::new_checked(reply.payload()).unwrap();
        assert_eq!(ipv4.src_addr(), "10.10.10.42".parse::<Ipv4Addr>().unwrap());
        assert_eq!(ipv4.dst_addr(), "10.0.0.2".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn socket_response_rejects_ipv4_header_shorter_than_minimum() {
        let engine = engine(Some(ProxyCidrRule {
            cidr: "127.0.0.0/24".parse().unwrap(),
            mapped_cidr: Some("10.10.10.0/24".parse().unwrap()),
        }));
        let destination = "127.0.0.42".parse().unwrap();
        let request = echo_request("10.0.0.2".parse().unwrap(), "10.10.10.42".parse().unwrap());
        assert!(matches!(
            engine.handle_peer_packet(
                &request,
                IcmpProxyContext {
                    virtual_ipv4: Some("10.0.0.1".parse().unwrap()),
                    ..Default::default()
                },
            ),
            IcmpProxyAction::SendToSocket { .. }
        ));

        let key = IcmpNatKey {
            real_destination: destination,
            identifier: 7,
            sequence: 11,
        };
        let mut response = echo_request(destination, "10.0.0.1".parse().unwrap())
            .payload()
            .to_vec();
        {
            let mut ipv4 = Ipv4Packet::new_unchecked(&mut response);
            ipv4.set_header_len(16);
            let mut reply = Icmpv4Packet::new_unchecked(ipv4.payload_mut());
            reply.set_msg_type(Icmpv4Message::EchoReply);
            reply.set_msg_code(0);
            reply.set_echo_ident(7);
            reply.set_echo_seq_no(11);
        }

        assert!(
            engine
                .handle_socket_response(destination, &mut response)
                .is_empty()
        );
        assert!(engine.nat_table.contains_key(&key));
    }

    #[test]
    fn large_local_reply_preserves_all_ipv4_fragments() {
        let engine = engine(None);
        let packet = echo_request_with_payload(
            "10.0.0.2".parse().unwrap(),
            "10.0.0.1".parse().unwrap(),
            &[0; 2400],
        );

        let IcmpProxyAction::SendToPeer(replies) = engine.handle_peer_packet(
            &packet,
            IcmpProxyContext {
                virtual_ipv4: Some("10.0.0.1".parse().unwrap()),
                no_tun: true,
                ..Default::default()
            },
        ) else {
            panic!("expected local replies");
        };

        assert_eq!(replies.len(), 3);
        assert!(replies.iter().all(|packet| {
            let header = packet.peer_manager_header().unwrap();
            header.from_peer_id.get() == 202 && header.to_peer_id.get() == 101
        }));
    }

    #[test]
    fn large_socket_reply_preserves_all_ipv4_fragments() {
        let engine = engine(Some(ProxyCidrRule {
            cidr: "127.0.0.0/24".parse().unwrap(),
            mapped_cidr: Some("10.10.10.0/24".parse().unwrap()),
        }));
        let destination = "127.0.0.42".parse().unwrap();
        let packet = echo_request_with_payload(
            "10.0.0.2".parse().unwrap(),
            "10.10.10.42".parse().unwrap(),
            &[0; 2400],
        );
        assert!(matches!(
            engine.handle_peer_packet(
                &packet,
                IcmpProxyContext {
                    virtual_ipv4: Some("10.0.0.1".parse().unwrap()),
                    ..Default::default()
                },
            ),
            IcmpProxyAction::SendToSocket { .. }
        ));
        assert!(engine.nat_table.contains_key(&IcmpNatKey {
            real_destination: destination,
            identifier: 7,
            sequence: 11,
        }));

        let mut response =
            echo_request_with_payload(destination, "10.0.0.1".parse().unwrap(), &[0; 2400])
                .payload()
                .to_vec();
        {
            let mut ipv4 = Ipv4Packet::new_unchecked(&mut response);
            let mut reply = Icmpv4Packet::new_unchecked(ipv4.payload_mut());
            reply.set_msg_type(Icmpv4Message::EchoReply);
            reply.fill_checksum();
            ipv4.set_src_addr(destination);
            // Raw sockets may return a buffer with bytes beyond the IPv4 total
            // length. The native implementation composes from the received
            // buffer length, so keep that case covered without changing the
            // existing in-place composer in this refactor.
            ipv4.set_total_len(1220);
            ipv4.fill_checksum();
        }
        let ipv4 = Ipv4Packet::new_checked(&response).unwrap();
        let echo_reply = Icmpv4Packet::new_checked(ipv4.payload()).unwrap();
        assert_eq!(echo_reply.msg_type(), Icmpv4Message::EchoReply);
        assert_eq!(echo_reply.echo_ident(), 7);
        assert_eq!(echo_reply.echo_seq_no(), 11);

        let replies = engine.handle_socket_response(destination, &mut response);
        assert_eq!(replies.len(), 3);
        assert!(replies.iter().all(|packet| {
            let header = packet.peer_manager_header().unwrap();
            header.from_peer_id.get() == 202
                && header.to_peer_id.get() == 101
                && header.is_no_proxy()
        }));
    }
}
