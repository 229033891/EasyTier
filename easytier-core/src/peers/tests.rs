use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::foundation::time::{Duration, timeout};

use crate::{
    foundation::token_bucket::{ArcByteLimiter, ByteLimiter},
    packet::{PacketType, ZCPacket},
    peers::{
        PeerConnectionOrigin, PeerPacketIngress,
        conn::{
            peer_conn::{PeerConn, PeerConnId},
            peer_map::PeerMap,
            peer_session::PeerSessionStore,
        },
        context::{NetworkIdentity, PeerContext},
        create_packet_recv_chan,
        error::Error,
        recv_packet_envelope_from_chan,
        test_support::NoopPeerContext,
    },
    tunnel::ring::create_ring_tunnel_pair,
};

/// Build a `PeerMap` for tests (flow accounting disabled).
fn new_test_peer_map(
    packet_send: crate::peers::PacketRecvChan,
    context: crate::peers::context::ArcPeerContext,
    my_peer_id: crate::config::PeerId,
) -> PeerMap {
    PeerMap::new(
        packet_send,
        context,
        my_peer_id,
        Arc::new(crate::peers::flow_table::FlowTable::new(
            crate::peers::flow_table::FlowTableConfig::default(),
        )),
    )
}
struct RejectingRecvLimiter;

impl ByteLimiter for RejectingRecvLimiter {
    fn try_consume(&self, _bytes: u64) -> bool {
        false
    }
}

struct LimitedPeerContext {
    limiter: ArcByteLimiter,
}

impl PeerContext for LimitedPeerContext {
    fn network_identity(&self) -> NetworkIdentity {
        NetworkIdentity::default()
    }

    fn recv_limiter(
        &self,
        _network_name: &str,
        _is_foreign_network: bool,
    ) -> Option<ArcByteLimiter> {
        Some(self.limiter.clone())
    }
}

impl PeerConn {
    #[tracing::instrument]
    async fn do_handshake_as_server(&mut self) -> Result<(), Error> {
        self.do_handshake_as_server_ext(|_, _| Ok(())).await
    }
}

#[tokio::test]
async fn peer_conn_handshake_over_memory_tunnel() {
    let peer_session_store = Arc::new(PeerSessionStore::new());
    let (client_tunnel, server_tunnel) = create_ring_tunnel_pair();
    let client_ctx = Arc::new(NoopPeerContext::default());
    let server_ctx = Arc::new(NoopPeerContext::default());

    let mut client = PeerConn::new(1, client_ctx, client_tunnel, peer_session_store.clone());
    let mut server = PeerConn::new(2, server_ctx, server_tunnel, peer_session_store);

    let (client_ret, server_ret) = tokio::join!(
        client.do_handshake_as_client(),
        server.do_handshake_as_server()
    );

    client_ret.unwrap();
    server_ret.unwrap();
    assert_eq!(client.get_peer_id(), 2);
    assert_eq!(server.get_peer_id(), 1);
    assert_eq!(client.get_conn_info().features, ["liveness-echo-v1"]);
    assert_eq!(server.get_conn_info().features, ["liveness-echo-v1"]);
}

#[tokio::test]
async fn peer_recv_limit_drops_excess_data_without_blocking_ping() {
    let peer_session_store = Arc::new(PeerSessionStore::new());
    let (client_tunnel, server_tunnel) = create_ring_tunnel_pair();
    let client_ctx = Arc::new(NoopPeerContext::default());
    let server_ctx = Arc::new(LimitedPeerContext {
        limiter: Arc::new(RejectingRecvLimiter),
    });

    let mut client = PeerConn::new(1, client_ctx, client_tunnel, peer_session_store.clone());
    let mut server = PeerConn::new(2, server_ctx, server_tunnel, peer_session_store);

    let (client_ret, server_ret) = tokio::join!(
        client.do_handshake_as_client(),
        server.do_handshake_as_server()
    );
    client_ret.unwrap();
    server_ret.unwrap();

    let (client_tx, _client_rx) = create_packet_recv_chan();
    let (server_tx, mut server_rx) = create_packet_recv_chan();
    client.start_recv_loop(client_tx).await;
    server.start_recv_loop(server_tx).await;

    let client_rx_packets = client.get_stats().rx_packets;

    let mut data = ZCPacket::new_with_payload(b"over limit");
    data.fill_peer_manager_hdr(1, 2, PacketType::Data as u8);
    client.send_msg(data).await.unwrap();

    let mut ping = ZCPacket::new_with_payload(&[]);
    ping.fill_peer_manager_hdr(1, 2, PacketType::Ping as u8);
    client.send_msg(ping).await.unwrap();

    timeout(Duration::from_secs(1), async {
        while client.get_stats().rx_packets == client_rx_packets {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("receive limiting must not block the following pong");

    assert!(
        timeout(Duration::from_millis(50), server_rx.recv())
            .await
            .is_err(),
        "over-limit data packet should be dropped"
    );
}

#[tokio::test]
async fn peer_conn_noise_handshake_advertises_liveness_echo() {
    fn context(peer_key: u8) -> Arc<NoopPeerContext> {
        let private = StaticSecret::from([peer_key; 32]);
        let public = PublicKey::from(&private);
        Arc::new(
            NoopPeerContext::new(NetworkIdentity {
                network_name: "net".to_owned(),
                network_secret: Some("secret".to_owned()),
                network_secret_digest: None,
            })
            .with_secure_mode(crate::proto::common::SecureModeConfig {
                enabled: true,
                local_private_key: Some(BASE64_STANDARD.encode(private.as_bytes())),
                local_public_key: Some(BASE64_STANDARD.encode(public.as_bytes())),
            }),
        )
    }

    let (client_tunnel, server_tunnel) = create_ring_tunnel_pair();
    let mut client = PeerConn::new(
        1,
        context(1),
        client_tunnel,
        Arc::new(PeerSessionStore::new()),
    );
    let mut server = PeerConn::new(
        2,
        context(2),
        server_tunnel,
        Arc::new(PeerSessionStore::new()),
    );

    let (client_ret, server_ret) = tokio::join!(
        client.do_handshake_as_client(),
        server.do_handshake_as_server()
    );

    client_ret.unwrap();
    server_ret.unwrap();
    assert_eq!(client.get_conn_info().features, ["liveness-echo-v1"]);
    assert_eq!(server.get_conn_info().features, ["liveness-echo-v1"]);
}

#[tokio::test]
async fn peer_conn_handshake_matches_plaintext_secret_identity() {
    let peer_session_store = Arc::new(PeerSessionStore::new());
    let (client_tunnel, server_tunnel) = create_ring_tunnel_pair();
    let client_ctx = Arc::new(NoopPeerContext::new(NetworkIdentity {
        network_name: "net".to_string(),
        network_secret: Some("secret".to_string()),
        network_secret_digest: None,
    }));
    let server_ctx = Arc::new(NoopPeerContext::new(NetworkIdentity {
        network_name: "net".to_string(),
        network_secret: Some("secret".to_string()),
        network_secret_digest: None,
    }));

    let mut client = PeerConn::new(1, client_ctx, client_tunnel, peer_session_store.clone());
    let mut server = PeerConn::new(2, server_ctx, server_tunnel, peer_session_store);

    let (client_ret, server_ret) = tokio::join!(
        client.do_handshake_as_client(),
        server.do_handshake_as_server()
    );

    client_ret.unwrap();
    server_ret.unwrap();
    assert!(client.matches_local_network_secret());
    assert!(server.matches_local_network_secret());
}

#[tokio::test]
async fn local_packet_channel_injection_has_no_attached_privilege() {
    let (sender, mut receiver) = create_packet_recv_chan();
    let mut packet = ZCPacket::new_with_payload(b"local");
    packet.fill_peer_manager_hdr(77, 3, PacketType::Data as u8);

    sender.send(packet).await.unwrap();

    let envelope = recv_packet_envelope_from_chan(&mut receiver).await.unwrap();
    let (_, ingress) = envelope.into_parts();
    assert_eq!(ingress, PeerPacketIngress::Local);
    assert!(!ingress.is_attached());
}

#[tokio::test]
async fn peer_map_forwards_packet_over_memory_tunnel() {
    let peer_session_store = Arc::new(PeerSessionStore::new());
    let (client_tunnel, server_tunnel) = create_ring_tunnel_pair();
    let client_ctx = Arc::new(NoopPeerContext::default());
    let server_ctx = Arc::new(NoopPeerContext::default());

    let mut client_conn = PeerConn::new(
        1,
        client_ctx.clone(),
        client_tunnel,
        peer_session_store.clone(),
    );
    let mut server_conn = PeerConn::new(2, server_ctx.clone(), server_tunnel, peer_session_store);

    let (client_ret, server_ret) = tokio::join!(
        client_conn.do_handshake_as_client(),
        server_conn.do_handshake_as_server()
    );
    client_ret.unwrap();
    server_ret.unwrap();

    let (client_tx, _client_rx) = create_packet_recv_chan();
    let (server_tx, mut server_rx) = create_packet_recv_chan();
    let client_map = new_test_peer_map(client_tx, client_ctx, 1);
    let server_map = new_test_peer_map(server_tx, server_ctx, 2);

    client_map.add_new_peer_conn(client_conn).await.unwrap();
    server_map.add_new_peer_conn(server_conn).await.unwrap();

    let mut packet = ZCPacket::new_with_payload(b"hello");
    packet.fill_peer_manager_hdr(1, 2, PacketType::Data as u8);
    client_map.send_msg_directly(packet, 2).await.unwrap();

    let received = timeout(Duration::from_secs(1), server_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.payload(), b"hello");
}

/// A packet sent through the real peer path (bond / default_conn / select_conn)
/// must be booked on the tunnel that actually carries it.
#[tokio::test]
async fn tx_flow_is_attributed_to_the_selected_tunnel() {
    use crate::peers::flow_table::{FlowDirection, FlowTable, FlowTableConfig, FlowView};

    let peer_session_store = Arc::new(PeerSessionStore::new());
    let (client_tunnel, server_tunnel) = create_ring_tunnel_pair();
    let client_ctx = Arc::new(NoopPeerContext::default());
    let server_ctx = Arc::new(NoopPeerContext::default());

    let mut client_conn = PeerConn::new(
        1,
        client_ctx.clone(),
        client_tunnel,
        peer_session_store.clone(),
    );
    let mut server_conn = PeerConn::new(2, server_ctx.clone(), server_tunnel, peer_session_store);

    let (client_ret, server_ret) = tokio::join!(
        client_conn.do_handshake_as_client(),
        server_conn.do_handshake_as_server()
    );
    client_ret.unwrap();
    server_ret.unwrap();

    let flow_table = Arc::new(FlowTable::new(FlowTableConfig {
        enabled: true,
        ..Default::default()
    }));

    let (client_tx, _client_rx) = create_packet_recv_chan();
    let (server_tx, _server_rx) = create_packet_recv_chan();
    let client_map = PeerMap::new(client_tx, client_ctx, 1, flow_table.clone());
    let server_map = PeerMap::new(
        server_tx,
        server_ctx,
        2,
        Arc::new(FlowTable::new(FlowTableConfig::default())),
    );

    let client_conn_id = client_conn.get_conn_id();
    client_map.add_new_peer_conn(client_conn).await.unwrap();
    server_map.add_new_peer_conn(server_conn).await.unwrap();
    let dst_ip: std::net::IpAddr = "10.10.10.9".parse().unwrap();
    let mut packet = ZCPacket::new_with_payload(b"hello");
    packet.fill_peer_manager_hdr(1, 2, PacketType::Data as u8);
    packet.set_flow_meta(crate::packet::FlowMeta {
        dst_ip,
        inner_len: 5,
    });

    let peer = client_map.get_peer_by_id(2).expect("peer with the conn");
    peer.send_msg(packet).await.unwrap();

    let snaps = flow_table.snapshot(FlowView::Exact, 10);
    assert_eq!(snaps.len(), 1, "expected exactly one recorded flow");
    assert_eq!(snaps[0].dir, FlowDirection::Tx);
    assert_eq!(snaps[0].ip, dst_ip);
    assert_eq!(snaps[0].bytes, 5);
    assert_eq!(snaps[0].peer_id, 2);
    // The flow must be booked on the tunnel that carried it.
    assert_eq!(
        snaps[0].conn_id, client_conn_id,
        "flow booked on the wrong tunnel"
    );
}

/// A packet received over a real encrypted tunnel must be booked as an RX flow
/// on the tunnel it arrived on, with the *inner* source IP.
///
/// This is the RX counterpart of `tx_flow_is_attributed_to_the_selected_tunnel`:
/// it covers the wiring that the pure `FlowTable` unit tests cannot — the
/// ingress (`peer_id` + `conn_id`) survives into the receive envelope, and the
/// payload is plaintext (decrypted) by the time the accounting runs.
#[tokio::test]
async fn rx_flow_is_attributed_to_the_receiving_tunnel() {
    use crate::peers::flow_table::{FlowDirection, FlowTable, FlowTableConfig, FlowView};

    let peer_session_store = Arc::new(PeerSessionStore::new());
    let (client_tunnel, server_tunnel) = create_ring_tunnel_pair();
    let client_ctx = Arc::new(NoopPeerContext::default());
    let server_ctx = Arc::new(NoopPeerContext::default());

    let mut client_conn = PeerConn::new(
        1,
        client_ctx.clone(),
        client_tunnel,
        peer_session_store.clone(),
    );
    let mut server_conn = PeerConn::new(2, server_ctx.clone(), server_tunnel, peer_session_store);

    let (client_ret, server_ret) = tokio::join!(
        client_conn.do_handshake_as_client(),
        server_conn.do_handshake_as_server()
    );
    client_ret.unwrap();
    server_ret.unwrap();

    let server_conn_id = server_conn.get_conn_id();
    let server_flow_table = Arc::new(FlowTable::new(FlowTableConfig {
        enabled: true,
        ..Default::default()
    }));

    let (client_tx, _client_rx) = create_packet_recv_chan();
    let (server_tx, mut server_rx) = create_packet_recv_chan();
    let client_map = PeerMap::new(
        client_tx,
        client_ctx,
        1,
        Arc::new(FlowTable::new(FlowTableConfig::default())),
    );
    let server_map = PeerMap::new(server_tx, server_ctx, 2, server_flow_table.clone());
    client_map.add_new_peer_conn(client_conn).await.unwrap();
    server_map.add_new_peer_conn(server_conn).await.unwrap();

    // A real IPv4 packet: EasyTier carries the inner IP packet verbatim, which
    // is why the RX side can recover the source address after decryption.
    let src: std::net::IpAddr = "10.10.10.9".parse().unwrap();
    let mut inner = vec![0u8; 28];
    inner[0] = 0x45;
    inner[2..4].copy_from_slice(&28u16.to_be_bytes());
    inner[12..16].copy_from_slice(&[10, 10, 10, 9]);
    inner[16..20].copy_from_slice(&[10, 10, 10, 2]);
    inner[20..28].copy_from_slice(b"payload!");

    let mut packet = ZCPacket::new_with_payload(&inner);
    packet.fill_peer_manager_hdr(1, 2, PacketType::Data as u8);
    client_map.send_msg_directly(packet, 2).await.unwrap();

    let envelope = timeout(
        Duration::from_secs(1),
        recv_packet_envelope_from_chan(&mut server_rx),
    )
    .await
    .unwrap()
    .unwrap();
    let (received, ingress) = envelope.into_parts();
    assert_eq!(
        received.payload(),
        inner.as_slice(),
        "inner packet must survive the tunnel byte-for-byte"
    );
    // This is exactly what `PeerManager::record_rx_flow` consumes: the
    // plaintext payload plus the ingress carried by the envelope.
    let PeerPacketIngress::Peer {
        peer_id, conn_id, ..
    } = ingress
    else {
        panic!("expected a peer ingress, got {ingress:?}");
    };
    assert_eq!(peer_id, 1);
    assert_eq!(
        conn_id, server_conn_id,
        "RX accounting must be attributed to the tunnel that carried the packet"
    );

    assert!(server_flow_table.record_rx(
        Some((conn_id, peer_id)),
        received.payload(),
        crate::peers::flow_table::now_unix_millis(),
    ));

    let snaps = server_flow_table.snapshot(FlowView::Exact, 10);
    assert_eq!(snaps.len(), 1, "expected exactly one recorded flow");
    assert_eq!(snaps[0].dir, FlowDirection::Rx);
    assert_eq!(snaps[0].ip, src, "RX flow must record the inner source IP");
    assert_eq!(snaps[0].bytes, inner.len() as u64);
    assert_eq!(snaps[0].packets, 1);
    assert_eq!(snaps[0].peer_id, 1);
    assert_eq!(
        snaps[0].conn_id, server_conn_id,
        "flow booked on the wrong tunnel"
    );
    // MASKED view collapses it to the configured prefix but keeps the count.
    let masked = server_flow_table.snapshot(
        FlowView::Masked {
            v4_prefix_len: 24,
            v6_prefix_len: 64,
        },
        10,
    );
    assert_eq!(masked.len(), 1);
    assert_eq!(masked[0].ip.to_string(), "10.10.10.0");
    assert_eq!(masked[0].flow_count, 1);
}

#[tokio::test]
async fn peer_channel_uses_admission_origin_instead_of_packet_header() {
    let peer_session_store = Arc::new(PeerSessionStore::new());
    let (client_tunnel, server_tunnel) = create_ring_tunnel_pair();
    let client_ctx = Arc::new(NoopPeerContext::default());
    let server_ctx = Arc::new(NoopPeerContext::default());

    let mut client_conn = PeerConn::new(
        1,
        client_ctx.clone(),
        client_tunnel,
        peer_session_store.clone(),
    );
    let mut server_conn = PeerConn::new_with_peer_id_hint_and_origin(
        2,
        server_ctx.clone(),
        server_tunnel,
        None,
        peer_session_store,
        PeerConnectionOrigin::Attached,
    );
    let (client_ret, server_ret) = tokio::join!(
        client_conn.do_handshake_as_client(),
        server_conn.do_handshake_as_server()
    );
    client_ret.unwrap();
    server_ret.unwrap();
    let server_conn_id = server_conn.get_conn_id();

    let (client_tx, _client_rx) = create_packet_recv_chan();
    let (server_tx, mut server_rx) = create_packet_recv_chan();
    let client_map = new_test_peer_map(client_tx, client_ctx, 1);
    let server_map = new_test_peer_map(server_tx, server_ctx, 2);
    client_map.add_new_peer_conn(client_conn).await.unwrap();
    server_map.add_new_peer_conn(server_conn).await.unwrap();
    assert!(server_map.has_direct_attached_peer(1));

    let mut packet = ZCPacket::new_with_payload(b"forged source");
    packet.fill_peer_manager_hdr(77, 3, PacketType::Data as u8);
    client_map.send_msg_directly(packet, 2).await.unwrap();

    let envelope = timeout(
        Duration::from_secs(1),
        recv_packet_envelope_from_chan(&mut server_rx),
    )
    .await
    .unwrap()
    .unwrap();
    let (received, ingress) = envelope.into_parts();
    assert_eq!(
        received.peer_manager_header().unwrap().from_peer_id.get(),
        77
    );
    assert_eq!(
        ingress,
        PeerPacketIngress::Peer {
            peer_id: 1,
            conn_id: server_conn_id,
            origin: PeerConnectionOrigin::Attached,
        }
    );
}

/// P-AUDIT.4: a peer entry outlives its last PeerConn, so `has_peer`
/// (contains_key) cannot be used as an availability check. The forwarding gate
/// must reject the dead entry so the packet can fall through to a relay leg
/// instead of failing inside `Peer::send_msg`.
#[tokio::test]
async fn has_peer_with_live_conn_tracks_conn_liveness_not_map_presence() {
    let peer_session_store = Arc::new(PeerSessionStore::new());
    let (client_tunnel, server_tunnel) = create_ring_tunnel_pair();
    let client_ctx = Arc::new(NoopPeerContext::default());
    let server_ctx = Arc::new(NoopPeerContext::default());

    let mut client_conn = PeerConn::new(
        1,
        client_ctx.clone(),
        client_tunnel,
        peer_session_store.clone(),
    );
    let mut server_conn = PeerConn::new(2, server_ctx.clone(), server_tunnel, peer_session_store);
    let (client_ret, server_ret) = tokio::join!(
        client_conn.do_handshake_as_client(),
        server_conn.do_handshake_as_server(),
    );
    client_ret.unwrap();
    server_ret.unwrap();

    let (client_tx, _client_rx) = create_packet_recv_chan();
    let client_map = new_test_peer_map(client_tx, client_ctx, 1);
    client_map.add_new_peer_conn(client_conn).await.unwrap();

    // Self is always reachable (self-send path), even with an empty peer map.
    assert!(client_map.has_peer_with_live_conn(1));
    // Unknown peer: both predicates agree.
    assert!(!client_map.has_peer(999));
    assert!(!client_map.has_peer_with_live_conn(999));

    // Live conn: both true.
    assert!(client_map.has_peer(2));
    assert!(client_map.has_peer_with_live_conn(2));

    let conn_id = client_map.get_peer_default_conn_id(2).await.unwrap();
    client_map.close_peer_conn(2, &conn_id).await.unwrap();
    timeout(Duration::from_secs(1), async {
        while client_map.has_peer_with_live_conn(2) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    // The whole point: the map entry lingers, so `has_peer` still says yes
    // while the availability check correctly says no.
    assert!(client_map.has_peer(2));
    assert!(!client_map.has_peer_with_live_conn(2));
}

#[tokio::test]
async fn peer_map_reselects_cached_connection_after_close() {
    let peer_session_store = Arc::new(PeerSessionStore::new());
    let (client_tunnel_a, server_tunnel_a) = create_ring_tunnel_pair();
    let (client_tunnel_b, server_tunnel_b) = create_ring_tunnel_pair();
    let client_ctx = Arc::new(NoopPeerContext::default());
    let server_ctx = Arc::new(NoopPeerContext::default());

    let mut client_conn_a = PeerConn::new(
        1,
        client_ctx.clone(),
        client_tunnel_a,
        peer_session_store.clone(),
    );
    let mut server_conn_a = PeerConn::new(
        2,
        server_ctx.clone(),
        server_tunnel_a,
        peer_session_store.clone(),
    );
    let mut client_conn_b = PeerConn::new(
        1,
        client_ctx.clone(),
        client_tunnel_b,
        peer_session_store.clone(),
    );
    let mut server_conn_b =
        PeerConn::new(2, server_ctx.clone(), server_tunnel_b, peer_session_store);

    let (client_a_ret, server_a_ret, client_b_ret, server_b_ret) = tokio::join!(
        client_conn_a.do_handshake_as_client(),
        server_conn_a.do_handshake_as_server(),
        client_conn_b.do_handshake_as_client(),
        server_conn_b.do_handshake_as_server(),
    );
    client_a_ret.unwrap();
    server_a_ret.unwrap();
    client_b_ret.unwrap();
    server_b_ret.unwrap();

    let (client_tx, _client_rx) = create_packet_recv_chan();
    let (server_tx, mut server_rx) = create_packet_recv_chan();
    let client_map = new_test_peer_map(client_tx, client_ctx, 1);
    let server_map = new_test_peer_map(server_tx, server_ctx, 2);

    client_map.add_new_peer_conn(client_conn_a).await.unwrap();
    client_map.add_new_peer_conn(client_conn_b).await.unwrap();
    server_map.add_new_peer_conn(server_conn_a).await.unwrap();
    server_map.add_new_peer_conn(server_conn_b).await.unwrap();

    let mut first_packet = ZCPacket::new_with_payload(b"first");
    first_packet.fill_peer_manager_hdr(1, 2, PacketType::Data as u8);
    client_map.send_msg_directly(first_packet, 2).await.unwrap();
    let first_received = timeout(Duration::from_secs(1), server_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_received.payload(), b"first");

    let first_conn_id = client_map.get_peer_default_conn_id(2).await.unwrap();
    assert_ne!(first_conn_id, PeerConnId::default());
    client_map.close_peer_conn(2, &first_conn_id).await.unwrap();
    timeout(Duration::from_secs(1), async {
        while client_map.get_peer_default_conn_id(2).await == Some(first_conn_id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let mut second_packet = ZCPacket::new_with_payload(b"second");
    second_packet.fill_peer_manager_hdr(1, 2, PacketType::Data as u8);
    client_map
        .send_msg_directly(second_packet, 2)
        .await
        .unwrap();
    let second_received = timeout(Duration::from_secs(1), server_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_received.payload(), b"second");
    assert_ne!(
        client_map.get_peer_default_conn_id(2).await,
        Some(first_conn_id)
    );
}
