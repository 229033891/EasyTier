use std::{
    net::Ipv4Addr,
    time::{Duration, Instant},
};

use dashmap::DashMap;
pub use smoltcp::wire::IpProtocol;
use smoltcp::wire::Ipv4Packet;

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
struct IpReassemblerKey {
    source: Ipv4Addr,
    destination: Ipv4Addr,
    id: u16,
}

#[derive(Debug, Clone)]
struct IpFragment {
    /// Byte offset of this fragment within the reassembled datagram.
    ///
    /// Widened to `u32` on purpose (CR-fix): the header field is a 13-bit value
    /// and the fragment's own length is up to 65535, so `u16` arithmetic could
    /// wrap and make a *later* fragment appear to overlap, or make a fragmented
    /// datagram look complete at the wrong length.
    offset: u32,
    data: Vec<u8>,
}

/// Largest IPv4 payload that can legitimately be reassembled.
///
/// IPv4's total-length field is 16 bits, so a reassembled datagram can never
/// exceed the header. Anything beyond that is a malformed or hostile input and
/// is dropped rather than buffered.
const MAX_IPV4_PAYLOAD_LEN: u32 = u16::MAX as u32 - smoltcp::wire::IPV4_HEADER_LEN as u32;

#[derive(Debug)]
struct IpPacket {
    /// Expected reassembled length in bytes, `None` until the last fragment
    /// arrives. Widened to `u32` for the same reason as [`IpFragment::offset`].
    total_length: Option<u32>,
    fragments: Vec<IpFragment>,
}

impl IpPacket {
    fn new() -> Self {
        Self {
            total_length: None,
            fragments: Vec::new(),
        }
    }

    fn add_fragment(&mut self, fragment: IpFragment) {
        for existing in &self.fragments {
            let existing_end = existing.offset + existing.data.len() as u32;
            let fragment_end = fragment.offset + fragment.data.len() as u32;
            if existing.offset <= fragment.offset && fragment.offset < existing_end {
                tracing::trace!(
                    existing_offset = existing.offset,
                    fragment_offset = fragment.offset,
                    existing_len = existing.data.len(),
                    fragment_len = fragment.data.len(),
                    "fragment overlap"
                );
                return;
            }
            if fragment.offset <= existing.offset && existing.offset < fragment_end {
                tracing::trace!(
                    existing_offset = existing.offset,
                    fragment_offset = fragment.offset,
                    existing_len = existing.data.len(),
                    fragment_len = fragment.data.len(),
                    "fragment overlap"
                );
                return;
            }
        }
        self.fragments.push(fragment);
    }

    fn set_total_length(&mut self, total_length: u32) {
        self.total_length = Some(total_length);
    }

    fn assemble(&mut self) -> Option<Vec<u8>> {
        let total_length = self.total_length?;
        self.fragments.sort_by_key(|fragment| fragment.offset);

        let mut offset = 0u32;
        let mut ret = Vec::with_capacity(total_length as usize);
        for fragment in &self.fragments {
            if fragment.offset != offset {
                return None;
            }
            ret.extend_from_slice(&fragment.data);
            offset += fragment.data.len() as u32;
            // Stop before the accumulator could ever be trusted to compare
            // against a wrapped value; `u32` makes this unreachable in practice,
            // but the ceiling is also the reassembly limit.
            if offset > MAX_IPV4_PAYLOAD_LEN {
                tracing::warn!(offset, "reassembly exceeded the IPv4 payload ceiling");
                return None;
            }
        }

        (offset == total_length).then_some(ret)
    }
}

impl<T: AsRef<[u8]> + ?Sized> From<&Ipv4Packet<&T>> for IpFragment {
    fn from(packet: &Ipv4Packet<&T>) -> Self {
        Self {
            offset: packet.frag_offset() as u32,
            data: packet.payload().to_vec(),
        }
    }
}

#[derive(Debug)]
struct IpReassemblerValue {
    packet: IpPacket,
    timestamp: Instant,
}

#[derive(Debug)]
pub struct IpReassembler {
    packets: DashMap<IpReassemblerKey, IpReassemblerValue>,
    timeout: Duration,
}

impl IpReassembler {
    pub fn new(timeout: Duration) -> Self {
        Self {
            packets: DashMap::new(),
            timeout,
        }
    }

    pub fn is_packet_fragmented<T: AsRef<[u8]>>(packet: &Ipv4Packet<T>) -> bool {
        packet.frag_offset() != 0 || packet.more_frags()
    }

    pub fn is_last_fragment<T: AsRef<[u8]>>(packet: &Ipv4Packet<T>) -> bool {
        !packet.more_frags()
    }

    pub fn add_fragment<T: AsRef<[u8]> + ?Sized>(
        &self,
        packet: &Ipv4Packet<&T>,
    ) -> Option<Vec<u8>> {
        let total_length = packet.total_len() - packet.header_len() as u16;
        if total_length != packet.payload().len() as u16 {
            tracing::trace!(
                ?total_length,
                payload_len = ?packet.payload().len(),
                "unexpected total length",
            );
            return None;
        }

        let key = IpReassemblerKey {
            source: packet.src_addr(),
            destination: packet.dst_addr(),
            id: packet.ident(),
        };
        let fragment: IpFragment = packet.into();

        tracing::trace!(?key, offset = fragment.offset, total_length, "add fragment");

        // A fragment that starts beyond what IPv4 can express is malformed by
        // construction: the reassembled datagram could never be that long. smoltcp
        // does not check `frag_offset + payload_len <= 65535` for us, and the old
        // `u16` arithmetic could wrap here, letting a hostile set of fragments
        // satisfy `offset == total_length` at a nonsense length and hand the
        // caller an oversized pseudo-datagram.
        let fragment_end = fragment.offset + fragment.data.len() as u32;
        if fragment_end > MAX_IPV4_PAYLOAD_LEN {
            tracing::warn!(
                ?key,
                offset = fragment.offset,
                fragment_end,
                MAX_IPV4_PAYLOAD_LEN,
                "dropping fragment that cannot fit in an IPv4 datagram",
            );
            return None;
        }

        let mut entry = self.packets.entry(key.clone()).or_insert_with(|| {
            let packet = IpPacket::new();
            let timestamp = Instant::now();
            IpReassemblerValue { packet, timestamp }
        });
        let value_mut = entry.value_mut();

        if Self::is_last_fragment(packet) {
            value_mut.packet.set_total_length(fragment_end);
        }

        value_mut.packet.add_fragment(fragment);
        if let Some(data) = value_mut.packet.assemble() {
            drop(entry);
            self.packets.remove(&key);
            Some(data)
        } else {
            value_mut.timestamp = Instant::now();
            None
        }
    }

    pub fn remove_expired_packets(&self) {
        let timeout = self.timeout;
        self.packets
            .retain(|_, value| value.timestamp.elapsed() <= timeout);
        self.packets.shrink_to_fit();
    }
}

pub struct ComposeIpv4PacketArgs<'a> {
    pub buf: &'a mut [u8],
    pub src_v4: &'a Ipv4Addr,
    pub dst_v4: &'a Ipv4Addr,
    pub next_protocol: IpProtocol,
    pub payload_len: usize,
    pub payload_mtu: usize,
    pub ip_id: u16,
}

pub fn compose_ipv4_packet<F>(args: ComposeIpv4PacketArgs, mut cb: F) -> anyhow::Result<()>
where
    F: FnMut(&[u8]) -> anyhow::Result<()>,
{
    let total_pieces = args.payload_len.div_ceil(args.payload_mtu);
    let mut buf_offset = 0;
    let mut fragment_offset = 0;
    let mut cur_piece = 0;
    while fragment_offset < args.payload_len {
        let next_fragment_offset =
            std::cmp::min(fragment_offset + args.payload_mtu, args.payload_len);
        let fragment_len = next_fragment_offset - fragment_offset;
        let packet_len = fragment_len + smoltcp::wire::IPV4_HEADER_LEN;
        let mut ipv4_packet =
            Ipv4Packet::new_unchecked(&mut args.buf[buf_offset..buf_offset + packet_len]);
        ipv4_packet.set_version(4);
        ipv4_packet.set_header_len(smoltcp::wire::IPV4_HEADER_LEN as u8);
        ipv4_packet.set_total_len(packet_len as u16);
        ipv4_packet.set_ident(args.ip_id);
        ipv4_packet.clear_flags();
        if total_pieces > 1 {
            ipv4_packet.set_more_frags(cur_piece != total_pieces - 1);
            ipv4_packet.set_frag_offset(fragment_offset as u16);
        } else {
            ipv4_packet.set_dont_frag(true);
            ipv4_packet.set_frag_offset(0);
        }
        ipv4_packet.set_dscp(0);
        ipv4_packet.set_ecn(0);
        ipv4_packet.set_hop_limit(32);
        ipv4_packet.set_src_addr(*args.src_v4);
        ipv4_packet.set_dst_addr(*args.dst_v4);
        ipv4_packet.set_next_header(args.next_protocol);
        ipv4_packet.fill_checksum();

        tracing::trace!(?ipv4_packet, "proxy ipv4 packet composed");

        cb(ipv4_packet.as_ref())?;

        buf_offset += next_fragment_offset - fragment_offset;
        fragment_offset = next_fragment_offset;
        cur_piece += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_ipv4_packet_fragments_arbitrary_payload() {
        let payload_len = 1932;
        let payload_mtu = 1256;
        let mut buffer = vec![0xff; smoltcp::wire::IPV4_HEADER_LEN + payload_len];
        let expected_payload = buffer[smoltcp::wire::IPV4_HEADER_LEN..].to_vec();
        let mut fragments = Vec::new();

        compose_ipv4_packet(
            ComposeIpv4PacketArgs {
                buf: &mut buffer,
                src_v4: &Ipv4Addr::new(10, 1, 2, 3),
                dst_v4: &Ipv4Addr::new(10, 1, 2, 4),
                next_protocol: IpProtocol::Udp,
                payload_len,
                payload_mtu,
                ip_id: 42,
            },
            |fragment| {
                fragments.push(fragment.to_vec());
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(fragments.len(), 2);
        let first = Ipv4Packet::new_checked(fragments[0].as_slice()).unwrap();
        let second = Ipv4Packet::new_checked(fragments[1].as_slice()).unwrap();
        assert!(first.more_frags());
        assert_eq!(first.frag_offset(), 0);
        assert!(!second.more_frags());
        assert_eq!(second.frag_offset(), payload_mtu as u16);

        let reassembled = first
            .payload()
            .iter()
            .chain(second.payload())
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(reassembled, expected_payload);
    }

    #[test]
    fn reassembler() {
        let raw_packets = [
            vec![
                0x45, 0x00, 0x00, 0x1c, 0x1c, 0x46, 0x20, 0x01, 0x40, 0x06, 0xb1, 0xe6, 0xc0, 0xa8,
                0x00, 0x01, 0xc0, 0xa8, 0x00, 0x02, 0x04, 0x05, 0x06, 0x07, 0x04, 0x05, 0x06, 0x07,
            ],
            vec![
                0x45, 0x00, 0x00, 0x1c, 0x1c, 0x46, 0x00, 0x02, 0x40, 0x06, 0xb1, 0xe6, 0xc0, 0xa8,
                0x00, 0x01, 0xc0, 0xa8, 0x00, 0x02, 0x08, 0x09, 0x0a, 0x0b, 0x04, 0x05, 0x06, 0x07,
            ],
            vec![
                0x45, 0x00, 0x00, 0x1c, 0x1c, 0x46, 0x20, 0x00, 0x40, 0x06, 0xb1, 0xe6, 0xc0, 0xa8,
                0x00, 0x01, 0xc0, 0xa8, 0x00, 0x02, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
            ],
            vec![
                0x45, 0x00, 0x00, 0x1c, 0x1c, 0x47, 0x20, 0x00, 0x40, 0x06, 0xb1, 0xe6, 0xc0, 0xa8,
                0x00, 0x01, 0xc0, 0xa8, 0x00, 0x02, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
            ],
        ];

        let reassembler = IpReassembler::new(Duration::from_secs(1));

        for (idx, raw_packet) in raw_packets.iter().enumerate() {
            let packet = Ipv4Packet::new_checked(raw_packet.as_slice()).unwrap();
            let ret = reassembler.add_fragment(&packet);
            if idx != 2 {
                assert!(ret.is_none());
            } else {
                assert!(ret.is_some());
            }
        }

        reassembler.remove_expired_packets();
        assert_eq!(1, reassembler.packets.len());

        std::thread::sleep(Duration::from_secs(2));
        reassembler.remove_expired_packets();
        assert_eq!(0, reassembler.packets.len());
    }

    /// Build a single-fragment IPv4 packet whose payload is `payload_len` bytes
    /// and whose `frag_offset` field is `frag_offset`.
    fn raw_fragment(ident: u16, frag_offset: u16, payload_len: usize) -> Vec<u8> {
        let packet_len = smoltcp::wire::IPV4_HEADER_LEN + payload_len;
        let mut buf = vec![0u8; packet_len];
        let mut packet = Ipv4Packet::new_unchecked(&mut buf);
        packet.set_version(4);
        packet.set_header_len(smoltcp::wire::IPV4_HEADER_LEN as u8);
        packet.set_total_len(packet_len as u16);
        packet.set_ident(ident);
        packet.set_more_frags(false);
        packet.set_frag_offset(frag_offset);
        packet.set_next_header(IpProtocol::Udp);
        packet.set_src_addr(Ipv4Addr::new(10, 1, 2, 3));
        packet.set_dst_addr(Ipv4Addr::new(10, 1, 2, 4));
        packet.fill_checksum();
        buf
    }

    #[test]
    fn fragment_spanning_past_the_ceiling_is_rejected() {
        // The regression. `frag_offset` is a 13-bit field, so the largest
        // declared offset is `0x1FFF`, but a fragment can carry up to a full
        // IPv4 payload. `offset + payload` can therefore exceed what IPv4 can
        // express, and smoltcp does not check it for us. With the old `u16`
        // arithmetic `total_length + frag_offset` wrapped, so a crafted pair
        // could satisfy `offset == total_length` at a nonsense length and hand
        // the caller an oversized pseudo-datagram.
        let reassembler = IpReassembler::new(Duration::from_secs(60));

        // Largest declared offset, plus a full-sized payload.
        let bad = raw_fragment(9, 0x1FFF, MAX_IPV4_PAYLOAD_LEN as usize);
        let packet = Ipv4Packet::new_checked(bad.as_slice()).unwrap();
        assert!(
            reassembler.add_fragment(&packet).is_none(),
            "a fragment spanning past the IPv4 ceiling must be dropped"
        );
        assert!(
            reassembler.packets.is_empty(),
            "the rejected fragment must not be buffered"
        );
    }

    #[test]
    fn in_range_fragment_is_still_buffered() {
        // Guard against over-tightening: a large but representable fragment must
        // still be accepted (it simply does not complete reassembly on its own).
        let reassembler = IpReassembler::new(Duration::from_secs(60));

        let raw = raw_fragment(13, 0x1FFF, 64);
        let packet = Ipv4Packet::new_checked(raw.as_slice()).unwrap();
        assert!(
            reassembler.add_fragment(&packet).is_none(),
            "an incomplete but valid reassembly returns None"
        );
        assert_eq!(
            reassembler.packets.len(),
            1,
            "a valid fragment must still be buffered"
        );
    }

    #[test]
    fn legitimate_maximum_size_fragment_is_still_accepted() {
        // A fragment that exactly fills the ceiling must still reassemble.
        let reassembler = IpReassembler::new(Duration::from_secs(60));

        let payload_len = MAX_IPV4_PAYLOAD_LEN as usize;
        let raw = raw_fragment(11, 0, payload_len);
        let packet = Ipv4Packet::new_checked(raw.as_slice()).unwrap();
        let reassembled = reassembler
            .add_fragment(&packet)
            .expect("a maximum-size fragment must reassemble");
        assert_eq!(reassembled.len(), payload_len);
    }
}
