use easytier_proto::api::{
    instance::{FlowView, PeerInfo, list_peer_route_pair},
    manage::{MyNodeInfo, NetworkInstanceRunningInfo},
};

use crate::{
    config::toml::ConfigLoader as _,
    instance::{CoreInstance, CoreInstanceHost, CoreInstanceState},
};

use super::super::instance_rpc::effective_policy_route_to_proto;

/// Builds the process-level running snapshot directly from one core Instance.
///
/// `flow_view` selects the IP detail of the per-tunnel flow lists; `None` means
/// "masked" (the default for the control plane). See
/// `docs/roadmap/tunnel-ip-flow-observability.md`.
#[allow(deprecated)]
pub async fn network_instance_running_info<H>(
    instance: &CoreInstance<H>,
    flow_view: Option<FlowView>,
) -> anyhow::Result<NetworkInstanceRunningInfo>
where
    H: CoreInstanceHost,
{
    let running = !matches!(
        instance.state(),
        CoreInstanceState::Created | CoreInstanceState::Stopped
    );
    if !instance.is_ready() {
        return Ok(NetworkInstanceRunningInfo {
            running,
            error_msg: instance.latest_error(),
            ..Default::default()
        });
    }

    let resolved_flow_view = crate::management::flow_view::resolve_flow_view_with_prefix(
        flow_view,
        instance.flow_stats_allow_exact_ip(),
        instance
            .flow_table()
            .config()
            .mask_prefix_len_v4,
        instance.flow_table().config().mask_prefix_len_v6,
    );
    let flow_table = instance.flow_table().clone();

    let peers = instance
        .peer_snapshots()
        .await
        .into_iter()
        .map(|snapshot| {
            let mut peer_info = PeerInfo {
                peer_id: snapshot.peer_id,
                default_conn_id: snapshot.default_conn_id.map(Into::into),
                directly_connected_conns: snapshot
                    .directly_connected_conns
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                conns: snapshot.conns.into_iter().map(Into::into).collect(),
            };
            crate::management::flow_view::fill_conn_flows(
                &mut peer_info.conns,
                &flow_table,
                resolved_flow_view,
            );
            peer_info
        })
        .collect::<Vec<_>>();
    let node = instance.node_snapshot().await;
    let routes = instance
        .route_snapshots()
        .await
        .into_iter()
        .map(Into::into)
        .collect::<Vec<_>>();
    let peer_route_pairs = list_peer_route_pair(peers.clone(), routes.clone());
    // Compute before `peers` moves into the snapshot struct. The disabled case
    // is forwarded to `snapshot_reachability` (not short-circuited to an empty
    // vec) so the API says "unknown + reason=disabled" per roadmap §5 instead of
    // the frontend silently hiding the badge as if the core were too old.
    let peer_vip_reachability = crate::instance::vip_probe::snapshot_reachability(
        instance.vip_probe_state.as_ref(),
        peers.iter().map(|p| p.peer_id),
        instance.vip_probe_disabled(),
    );
    let policy_routes_effective = instance
        .effective_policy_routes()
        .await
        .into_iter()
        .map(effective_policy_route_to_proto)
        .collect();
    let dev_name = instance
        .toml_config()
        .map(|config| config.get_flags().dev_name)
        .unwrap_or_default();

    Ok(NetworkInstanceRunningInfo {
        dev_name,
        my_node_info: Some(MyNodeInfo {
            virtual_ipv4: node.ipv4_addr.map(Into::into),
            virtual_ipv6: node.ipv6_addr.map(Into::into),
            hostname: node.hostname,
            version: node.version,
            ips: Some(node.ip_list),
            stun_info: Some(node.stun_info),
            listeners: node.listeners.into_iter().map(Into::into).collect(),
            // Client private keys are returned only by the explicit portal RPC.
            vpn_portal_cfg: None,
            peer_id: node.peer_id,
        }),
        events: instance.management_events(),
        routes,
        peers,
        peer_route_pairs,
        running,
        error_msg: instance.latest_error(),
        foreign_network_summary: Some(instance.foreign_network_route_summary().await),
        proxy_cidr_route_sync: Some(
            instance
                .packet_plane()
                .proxy_cidr_route_sync_status()
                .summary(),
        ),
        // Mirrored for host-side route aggregators (Android VpnService / OHOS),
        // which build their own route sets and cannot read the core config.
        // Sent unconditionally: the host decides between the `optional` presence
        // bit and the value, and proto3 cannot express "explicitly false".
        allow_peer_default_without_exit: Some(
            instance
                .toml_config()
                .map(|config| config.get_flags().allow_peer_default_without_exit)
                .unwrap_or(false),
        ),
        exit_nodes: instance
            .toml_config()
            .map(|config| {
                config
                    .get_exit_nodes()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        // Core-computed L2 desired set; host installers apply it verbatim
        // (manual_routes / policy CIDRs included). Computed on demand from
        // live snapshots, so managed pushes are reflected on the next poll.
        desired_proxy_cidrs: instance.packet_plane().proxy_cidr_desired().await,
        // VIP dataplane reachability per connected peer (GUI "connected vs
        // reachable"). Display-only; independent of control-plane keepalive.
        peer_vip_reachability,
        // Runtime policy-route view after live exit_nodes merge (L6).
        policy_routes_effective,
    })
}
