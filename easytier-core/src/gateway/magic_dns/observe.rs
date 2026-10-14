//! P3: observe MagicDNS answers so domain policy routes learn their IPs.
//!
//! `MagicDnsQueryResolver` is the only place in the codebase that sees what an
//! application actually resolved, and it already honours the four-level
//! resolution order (hosts → route zone → split → upstream). We therefore
//! subscribe to the **returned answer** rather than querying the hosts table.
//!
//! The rule set stays authoritative from local config (`[[policy_routes]]`);
//! only the IP set is learned here.

use std::{
    net::Ipv4Addr,
    sync::{Arc, Weak},
};

use async_trait::async_trait;

use crate::peers::{peer_manager::PeerManagerCore, route::local_route_table::name_is_in_zone};

use super::packet::{MagicDnsQuery, MagicDnsQueryResolver};

/// Wraps a resolver and feeds observed answers into the unified routing table.
pub struct ObservingDnsResolver {
    inner: Arc<dyn MagicDnsQueryResolver>,
    peer_manager: Weak<PeerManagerCore>,
}

impl ObservingDnsResolver {
    pub fn new(inner: Arc<dyn MagicDnsQueryResolver>, peer_manager: Weak<PeerManagerCore>) -> Self {
        Self {
            inner,
            peer_manager,
        }
    }
}

#[async_trait]
impl MagicDnsQueryResolver for ObservingDnsResolver {
    async fn resolve(&self, query: MagicDnsQuery) -> Option<Vec<u8>> {
        let response = self.inner.resolve(query.clone()).await;
        // No domain policy routes → observation is a pure no-op; skip parsing.
        let Some(peer_manager) = self.peer_manager.upgrade() else {
            return response;
        };
        if !peer_manager.local_route_table().has_domain_rules() {
            return response;
        }
        if let Some(answer) = &response
            && let Some(qname) = parse_dns_question_name(&query.payload)
        {
            // C4: MagicDNS route hostnames (`<host>.<tld_dns_zone>`) resolve to
            // mesh virtual IPs. Those answers belong to the mesh, never to a
            // domain policy route, so they must not be learned here — otherwise
            // a `domains = ["*.et.net"]` rule would hijack mesh traffic.
            let zone = peer_manager.dns_route_identity().2;
            if !name_is_in_zone(&qname, &zone) {
                let ips = parse_dns_a_records(answer);
                if !ips.is_empty() {
                    // Synchronous update: L3 is ready before the caller sees the
                    // answer. The L2 OS route still lands within the 1 s
                    // reconcile tick, so the very first connection keeps a ≤1 s
                    // window (§9.8.4 — this design does **not** promise zero
                    // leakage).
                    peer_manager.observe_dns_answer(&qname, &ips);
                }
            }
        }
        response
    }
}

/// Read a (possibly compressed) DNS name starting at `start`.
///
/// Returns the lower-case name and the offset just past the name in the
/// original record — i.e. the cursor before any compression jump.
fn read_name(msg: &[u8], start: usize) -> Option<(String, usize)> {
    let mut labels: Vec<&[u8]> = Vec::new();
    let mut cursor = start;
    let mut resume: Option<usize> = None;

    // Bound the walk so a pointer loop cannot spin forever.
    for _ in 0..128 {
        let len = *msg.get(cursor)? as usize;
        if len == 0 {
            return Some((join_labels(&labels), resume.unwrap_or(cursor + 1)));
        }
        if len & 0xc0 == 0xc0 {
            let low = *msg.get(cursor + 1)? as usize;
            let pointer = ((len & 0x3f) << 8) | low;
            if resume.is_none() {
                resume = Some(cursor + 2);
            }
            cursor = pointer;
            continue;
        }
        // A label is at most 63 bytes; reject reserved length bits.
        if len > 63 {
            return None;
        }
        let end = cursor.checked_add(1 + len)?;
        labels.push(msg.get(cursor + 1..end)?);
        cursor = end;
    }
    None
}

fn join_labels(labels: &[&[u8]]) -> String {
    let mut out = String::new();
    for (i, label) in labels.iter().enumerate() {
        if i > 0 {
            out.push('.');
        }
        out.push_str(&String::from_utf8_lossy(label).to_ascii_lowercase());
    }
    out
}

fn read_u16(msg: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*msg.get(at)?, *msg.get(at + 1)?]))
}

/// QNAME of the first question, normalized to lower case without a trailing
/// dot. `None` when the message is malformed or carries no question.
pub fn parse_dns_question_name(msg: &[u8]) -> Option<String> {
    if read_u16(msg, 4)? == 0 {
        return None;
    }
    let (name, _) = read_name(msg, 12)?;
    if name.is_empty() { None } else { Some(name) }
}

/// IPv4 addresses from every A record in the answer section.
pub fn parse_dns_a_records(msg: &[u8]) -> Vec<Ipv4Addr> {
    let Some(qdcount) = read_u16(msg, 4) else {
        return Vec::new();
    };
    let Some(ancount) = read_u16(msg, 6) else {
        return Vec::new();
    };

    let mut pos = 12usize;
    for _ in 0..qdcount {
        let Some((_, next)) = read_name(msg, pos) else {
            return Vec::new();
        };
        pos = next.saturating_add(4); // QTYPE + QCLASS
    }

    let mut out = Vec::new();
    for _ in 0..ancount {
        let Some((_, next)) = read_name(msg, pos) else {
            break;
        };
        let Some(rr_type) = read_u16(msg, next) else {
            break;
        };
        let Some(rdlength) = read_u16(msg, next + 8) else {
            break;
        };
        let rdata = next + 10;
        if rr_type == 1
            && rdlength == 4
            && let Some(bytes) = msg.get(rdata..rdata + 4)
        {
            out.push(Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]));
        }
        pos = rdata.saturating_add(rdlength as usize);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_query(qname: &str) -> Vec<u8> {
        let mut msg = vec![0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        for label in qname.split('.').filter(|label| !label.is_empty()) {
            msg.push(label.len() as u8);
            msg.extend_from_slice(label.as_bytes());
        }
        msg.push(0);
        msg.extend_from_slice(&[0x00, 0x01]); // QTYPE = A
        msg.extend_from_slice(&[0x00, 0x01]); // QCLASS = IN
        msg
    }

    fn build_a_response(qname: &str, ips: &[Ipv4Addr]) -> Vec<u8> {
        let mut msg = build_query(qname);
        msg[2] = 0x81; // QR = response
        msg[3] = 0x80;
        msg[6] = (ips.len() >> 8) as u8;
        msg[7] = ips.len() as u8;
        for ip in ips {
            msg.extend_from_slice(&[0xc0, 0x0c]); // NAME → offset 12 (compression)
            msg.extend_from_slice(&[0x00, 0x01]); // A
            msg.extend_from_slice(&[0x00, 0x01]); // IN
            msg.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]); // TTL 60
            msg.extend_from_slice(&[0x00, 0x04]); // RDLENGTH
            msg.extend_from_slice(&ip.octets());
        }
        msg
    }

    #[test]
    fn parses_question_name() {
        let query = build_query("Api.Example.COM.");
        assert_eq!(
            parse_dns_question_name(&query).as_deref(),
            Some("api.example.com")
        );
    }

    #[test]
    fn parses_a_records_through_compression() {
        let ips: Vec<Ipv4Addr> = vec![
            "203.0.113.7".parse().unwrap(),
            "203.0.113.8".parse().unwrap(),
        ];
        let response = build_a_response("api.example.com", &ips);
        assert_eq!(parse_dns_a_records(&response), ips);
    }

    #[test]
    fn ignores_non_a_answers() {
        let mut response = build_a_response("api.example.com", &[]);
        response[6] = 0;
        response[7] = 1;
        response.extend_from_slice(&[0xc0, 0x0c]); // NAME
        response.extend_from_slice(&[0x00, 0x05]); // CNAME
        response.extend_from_slice(&[0x00, 0x01]); // IN
        response.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]);
        response.extend_from_slice(&[0x00, 0x02]); // RDLENGTH
        response.extend_from_slice(&[0xc0, 0x0c]); // rdata: name pointer
        assert!(parse_dns_a_records(&response).is_empty());
    }

    #[test]
    fn malformed_messages_do_not_panic() {
        assert!(parse_dns_question_name(&[]).is_none());
        assert!(parse_dns_question_name(&[0u8; 12]).is_none());
        assert!(parse_dns_a_records(&[0xff; 4]).is_empty());
        // Self-referential compression pointer must terminate, not loop.
        assert!(
            parse_dns_question_name(&[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xc0, 0x0c]).is_none()
        );
    }
}
