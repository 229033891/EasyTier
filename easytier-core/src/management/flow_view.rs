//! Fill the per-tunnel IP flow details into the management API responses.
//!
//! See `docs/roadmap/tunnel-ip-flow-observability.md` (decisions D8 / D9 / D13).
//!
//! Two rules are enforced here:
//!
//! * **MASKED by default** — a caller must ask for `EXACT` explicitly
//!   (`FlowView::EXACT`), otherwise the node answers with prefix-aggregated
//!   data. The exact IPs of the virtual network are sensitive information.
//! * **Double opt-in for exact IPs** — even an explicit `EXACT` request is
//!   downgraded to `MASKED` unless the node itself is configured with
//!   `flow_stats_allow_exact_ip` (decision D13).

use crate::peers::flow_table::{FlowSnapshot, FlowTable, FlowView};
use crate::proto::api::instance::{FlowView as ProtoFlowView, PeerConnFlow, PeerConnInfo};

/// How many flows are reported per tunnel.
pub const FLOW_TOP_N_PER_CONN: usize = 10;

/// Resolve the effective view for a request.
///
/// `allow_exact_ip` is the node-side switch; `requested` is what the caller
/// asked for. The result is `EXACT` only when both agree.
pub fn resolve_flow_view(requested: Option<ProtoFlowView>, allow_exact_ip: bool) -> FlowView {
    resolve_flow_view_with_prefix(requested, allow_exact_ip, 24, 64)
}

/// Convert the wire representation (prost uses `i32` for enums) into the
/// typed view, ignoring unknown values.
pub fn proto_flow_view(value: Option<i32>) -> Option<ProtoFlowView> {
    value.and_then(|v| ProtoFlowView::try_from(v).ok())
}

/// Same as [`resolve_flow_view`], but with explicit prefix lengths.
pub fn resolve_flow_view_with_prefix(
    requested: Option<ProtoFlowView>,
    allow_exact_ip: bool,
    v4_prefix_len: u8,
    v6_prefix_len: u8,
) -> FlowView {
    match requested {
        Some(ProtoFlowView::Exact) if allow_exact_ip => FlowView::Exact,
        _ => FlowView::Masked {
            v4_prefix_len,
            v6_prefix_len,
        },
    }
}

/// Attach the recorded flows of every tunnel to its `PeerConnInfo`.
///
/// The flows are grouped per tunnel; only the tunnels that actually carry
/// traffic get entries, so an idle tunnel keeps an empty list (and therefore
/// costs nothing on the wire).
pub fn fill_conn_flows(conns: &mut [PeerConnInfo], table: &FlowTable, view: FlowView) {
    if !table.is_enabled() {
        return;
    }
    for conn in conns.iter_mut() {
        let Ok(conn_id) = conn.conn_id.parse::<uuid::Uuid>() else {
            continue;
        };
        let flows = table.snapshot_for_conn(conn_id, view, FLOW_TOP_N_PER_CONN);
        if flows.is_empty() {
            continue;
        }
        conn.flows = flows.iter().map(to_proto_flow).collect();
    }
}

fn to_proto_flow(snapshot: &FlowSnapshot) -> PeerConnFlow {
    PeerConnFlow {
        conn_id: snapshot.conn_id.to_string(),
        peer_id: snapshot.peer_id,
        ip: snapshot.ip.to_string(),
        direction: match snapshot.dir {
            crate::peers::flow_table::FlowDirection::Tx => 0,
            crate::peers::flow_table::FlowDirection::Rx => 1,
        },
        bytes: snapshot.bytes,
        packets: snapshot.packets,
        rate_bytes_per_sec: snapshot.rate_bytes_per_sec,
        last_seen_ms: snapshot.last_seen_ms,
        sampled: snapshot.sampled,
        prefix_len: u32::from(snapshot.prefix_len),
        flow_count: snapshot.flow_count,
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;
    use crate::peers::flow_table::{FlowDirection, FlowTable, FlowTableConfig};
    use crate::proto::api::instance::PeerConnInfo;

    fn conn_id(n: u8) -> uuid::Uuid {
        uuid::Uuid::from_bytes([n; 16])
    }

    fn enabled_table() -> FlowTable {
        FlowTable::new(FlowTableConfig {
            enabled: true,
            ..Default::default()
        })
    }

    #[test]
    fn exact_view_needs_both_sides_to_agree() {
        // No request -> masked.
        assert!(matches!(
            resolve_flow_view(None, true),
            FlowView::Masked { .. }
        ));
        // Explicit EXACT but the node does not allow it -> masked.
        assert!(matches!(
            resolve_flow_view(Some(ProtoFlowView::Exact), false),
            FlowView::Masked { .. }
        ));
        // Explicit EXACT and the node allows it -> exact.
        assert!(matches!(
            resolve_flow_view(Some(ProtoFlowView::Exact), true),
            FlowView::Exact
        ));
        // MASKED request stays masked even when the node allows exact.
        assert!(matches!(
            resolve_flow_view(Some(ProtoFlowView::Masked), true),
            FlowView::Masked { .. }
        ));
    }

    #[test]
    fn fill_conn_flows_attaches_the_flows_of_their_own_tunnel() {
        let table = enabled_table();
        table.record(
            conn_id(1),
            7,
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9)),
            FlowDirection::Tx,
            1200,
            3,
            1000,
        );
        table.record(
            conn_id(2),
            8,
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10)),
            FlowDirection::Rx,
            400,
            1,
            1000,
        );

        let mut conns = vec![
            PeerConnInfo {
                conn_id: conn_id(1).to_string(),
                peer_id: 7,
                ..Default::default()
            },
            PeerConnInfo {
                conn_id: conn_id(2).to_string(),
                peer_id: 8,
                ..Default::default()
            },
            PeerConnInfo {
                conn_id: conn_id(3).to_string(),
                peer_id: 9,
                ..Default::default()
            },
        ];

        fill_conn_flows(
            &mut conns,
            &table,
            FlowView::Exact,
        );

        assert_eq!(conns[0].flows.len(), 1);
        assert_eq!(conns[0].flows[0].bytes, 1200);
        assert_eq!(conns[0].flows[0].direction, 0);
        assert_eq!(conns[0].flows[0].prefix_len, 0);
        assert_eq!(conns[1].flows.len(), 1);
        assert_eq!(conns[1].flows[0].direction, 1);
        // An idle tunnel keeps an empty list.
        assert!(conns[2].flows.is_empty());
    }

    #[test]
    fn masked_view_aggregates_prefixes() {
        let table = enabled_table();
        table.record(
            conn_id(1),
            7,
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9)),
            FlowDirection::Tx,
            100,
            1,
            1000,
        );
        table.record(
            conn_id(1),
            7,
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10)),
            FlowDirection::Tx,
            200,
            1,
            1000,
        );

        let mut conns = vec![PeerConnInfo {
            conn_id: conn_id(1).to_string(),
            peer_id: 7,
            ..Default::default()
        }];

        fill_conn_flows(
            &mut conns,
            &table,
            FlowView::Masked {
                v4_prefix_len: 24,
                v6_prefix_len: 64,
            },
        );

        assert_eq!(conns[0].flows.len(), 1);
        assert_eq!(conns[0].flows[0].prefix_len, 24);
        assert_eq!(conns[0].flows[0].flow_count, 2);
        assert_eq!(conns[0].flows[0].bytes, 300);
    }

    #[test]
    fn disabled_table_produces_no_flows() {
        let table = FlowTable::new(FlowTableConfig::default());
        table.record(
            conn_id(1),
            7,
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9)),
            FlowDirection::Tx,
            100,
            1,
            1000,
        );
        let mut conns = vec![PeerConnInfo {
            conn_id: conn_id(1).to_string(),
            peer_id: 7,
            ..Default::default()
        }];
        fill_conn_flows(&mut conns, &table, FlowView::Exact);
        assert!(conns[0].flows.is_empty());
    }
}
