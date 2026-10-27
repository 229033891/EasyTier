//! Unit tests for PeerManagerCore.

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use dashmap::DashMap;
use quanta::Instant;
use x25519_dalek::{PublicKey, StaticSecret};

use super::*;
use crate::{
    config::runtime::CoreRuntimeConfig,
    config::{CoreConfig, IpPrefix, NetworkIdentity, NodeConfig, ProxyNetworkConfig},
    host::packet::{HostPacketSender, host_packet_channel},
    peers::{
        PeerPacketFilter, PeerPacketFilterResult,
        context::{PeerContext, PeerEvent},
    },
    proto::common::{PeerFeatureFlag, StunInfo},
};

impl PeerManagerCore {
    pub(crate) fn new_portable_for_test(
        config: PortablePeerManagerConfig,
        nic_channel: HostPacketSender,
    ) -> anyhow::Result<Self> {
        let runtime_config = CoreRuntimeConfigStore::new(
            CoreRuntimeConfig::default(),
            Arc::new(config.snapshot.clone()),
        );
        let public_ipv6_runtime =
            CorePublicIpv6Runtime::new(runtime_config.clone(), Arc::new(()), Arc::new(()));
        let stun_info_source = Arc::new(RuntimeConfigStunInfoSource(runtime_config.clone()));
        Self::new(
            config,
            Vec::new(),
            runtime_config,
            stun_info_source,
            nic_channel,
            public_ipv6_runtime,
            Arc::new(()),
            None,
            Arc::new(()),
        )
    }
}

struct RuntimeConfigStunInfoSource(CoreRuntimeConfigStore);

impl PeerStunInfoSource for RuntimeConfigStunInfoSource {
    fn stun_info(&self) -> StunInfo {
        self.0.snapshot().peer.runtime.stun_info.clone()
    }
}

struct SameNetworkContext {
    contains_every_address: bool,
}

impl PeerContext for SameNetworkContext {
    fn network_identity(&self) -> NetworkIdentity {
        NetworkIdentity {
            network_name: "test".to_string(),
            network_secret: None,
            network_secret_digest: None,
        }
    }

    fn is_ip_in_same_network(&self, ip: &IpAddr) -> bool {
        self.contains_every_address
            || matches!(ip, IpAddr::V4(ip) if ip.octets()[0..2] == [10, 144])
    }
}

#[derive(Default)]
struct CountingPeerEventSink(AtomicUsize);

impl crate::events::CoreEventSink for CountingPeerEventSink {
    fn emit(&self, _event: crate::events::CoreEvent) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

struct DropCountingNicFilter(Arc<AtomicUsize>);

impl Drop for DropCountingNicFilter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[async_trait::async_trait]
impl super::super::NicPacketFilter for DropCountingNicFilter {
    async fn try_process_packet_from_nic(&self, _data: &mut ZCPacket) -> bool {
        true
    }
}

#[tokio::test]
async fn full_host_packet_queue_does_not_block_peer_packet_processing() {
    let (nic_channel, mut nic_receiver) = mpsc::channel(1);
    nic_channel
        .send(HostPacket::copy_from_payload(b"queued"))
        .await
        .unwrap();
    let stats = StatsManager::default();
    let nic_channel_drops =
        stats.get_counter(MetricName::DataPlaneNicChannelDrops, LabelSet::new());
    let processor = NicPacketProcessor {
        nic_channel,
        nic_channel_drops: nic_channel_drops.clone(),
    };
    let mut packet = ZCPacket::new_with_payload(b"next");
    packet.fill_peer_manager_hdr(1, 2, PacketType::Data as u8);

    let result = tokio::time::timeout(
        Duration::from_millis(100),
        processor.try_process_packet_from_peer(packet),
    )
    .await;

    assert!(result.is_ok(), "a full host queue blocked the peer router");
    assert!(matches!(
        result.unwrap(),
        PeerPacketFilterResult::Consumed
    ));
    assert_eq!(nic_receiver.try_recv().unwrap().payload(), b"queued");
    assert_eq!(nic_channel_drops.get(), 1);
}

#[test]
fn managed_nic_pipeline_removal_preserves_in_flight_snapshot() {
    let drops = Arc::new(AtomicUsize::new(0));
    let pipeline = Arc::new(ArcSwap::from_pointee(Vec::new()));
    let (entry, registration) =
        managed_nic_pipeline_entry(Box::new(DropCountingNicFilter(drops.clone())), &pipeline);
    append_nic_pipeline(&pipeline, entry);
    let reader = pipeline.load_full();

    registration.close();

    assert!(pipeline.load().is_empty());
    assert_eq!(drops.load(Ordering::Relaxed), 0);

    drop(reader);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn managed_pipeline_guard_releases_filter_without_a_runtime() {
    let drops = Arc::new(AtomicUsize::new(0));
    let pipeline = Arc::new(ArcSwap::from_pointee(Vec::new()));
    let (entry, registration) =
        managed_nic_pipeline_entry(Box::new(DropCountingNicFilter(drops.clone())), &pipeline);
    append_nic_pipeline(&pipeline, entry);

    drop(registration);

    assert!(pipeline.load().is_empty());
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

fn portable_runtime_config(network_name: &str) -> PeerRuntimeConfig {
    PeerRuntimeConfig {
        core: CoreConfig {
            node: NodeConfig {
                peer_id: None,
                network_name: network_name.to_owned(),
                ..Default::default()
            },
            ..Default::default()
        },
        network_identity: NetworkIdentity {
            network_name: network_name.to_owned(),
            network_secret: Some("secret".to_owned()),
            network_secret_digest: None,
        },
        stun_info: StunInfo::default(),
        feature_flags: PeerFeatureFlag::default(),
        secure_mode: None,
        host_routing: HostRoutingPolicy::default(),
    }
}

fn credential_secure_mode() -> crate::proto::common::SecureModeConfig {
    let private = StaticSecret::from([7; 32]);
    let public = PublicKey::from(&private);
    crate::proto::common::SecureModeConfig {
        enabled: true,
        local_private_key: Some(BASE64_STANDARD.encode(private.as_bytes())),
        local_public_key: Some(BASE64_STANDARD.encode(public.as_bytes())),
    }
}

fn build_portable_for_test(runtime: PeerRuntimeConfig) -> anyhow::Result<PeerManagerCore> {
    build_portable_config_for_test(PortablePeerManagerConfig::new(runtime))
}

fn build_portable_config_for_test(
    config: PortablePeerManagerConfig,
) -> anyhow::Result<PeerManagerCore> {
    let (packet_tx, _packet_rx) = host_packet_channel();
    PeerManagerCore::new_portable_for_test(config, packet_tx)
}

#[tokio::test]
async fn portable_peer_manager_builds_and_stops_from_normalized_config() {
    let runtime = portable_runtime_config("portable-net");
    let core = build_portable_for_test(runtime).unwrap();

    assert_eq!(core.context.network_name(), "portable-net");
    assert_ne!(core.context.instance_id(), uuid::Uuid::nil());
    assert_eq!(core.data_compress_algo, CompressorAlgo::None);
    assert!(core.list_foreign_network_infos(false).await.is_empty());

    core.run().await.unwrap();
    let route = core.route_algo_inst.ospf_route().unwrap();
    assert!(route.task_count() > 0);
    assert!(!core.stats_manager().cleanup_task_is_stopped());
    assert!(!core.acl_filter.cleanup_task_is_stopped());
    core.clear_resources().await;
    assert_eq!(route.task_count(), 0);
    assert!(core.stats_manager().cleanup_task_is_stopped());
    assert!(core.acl_filter.cleanup_task_is_stopped());
    assert!(core.foreign_network_manager.is_stopped_for_test().await);
    assert!(
        !core
            .foreign_network_manager
            .admission_is_open_for_test()
            .await
    );
}
#[tokio::test]
async fn runtime_updates_retain_manager_owned_peer_identity() {
    let core = build_portable_for_test(portable_runtime_config("portable-net")).unwrap();
    let current = core.runtime_config.snapshot();
    let expected_peer_id = core.my_peer_id();
    let expected_instance_id = current.peer.runtime.core.node.instance_id;
    let mut next = current.as_ref().clone();
    let next_peer = Arc::make_mut(&mut next.peer);
    next_peer.runtime.core.node.peer_id = Some(expected_peer_id.wrapping_add(1));
    next_peer.runtime.core.node.instance_id = Some([1; 16]);
    let submitted = next.peer.clone();

    let published = core.update_runtime_config(next).await.unwrap();

    assert_eq!(
        published.peer.runtime.core.node.peer_id,
        Some(expected_peer_id)
    );
    assert_eq!(
        published.peer.runtime.core.node.instance_id,
        expected_instance_id
    );
    assert_eq!(
        submitted.runtime.core.node.peer_id,
        Some(expected_peer_id.wrapping_add(1))
    );
    assert_eq!(submitted.runtime.core.node.instance_id, Some([1; 16]));
    core.clear_resources().await;
}

#[cfg(not(feature = "zstd"))]
#[tokio::test]
async fn portable_peer_manager_rejects_requested_unavailable_zstd() {
    let mut config = PortablePeerManagerConfig::new(portable_runtime_config("portable-net"));
    config.snapshot.flags.data_compress_algo = crate::proto::common::CompressionAlgoPb::Zstd.into();

    let error = build_portable_config_for_test(config).err().unwrap();

    assert_eq!(
        error.to_string(),
        "compression algorithm is unavailable in this build: ZstdDefault"
    );
}

#[cfg(not(any(
    feature = "aes-gcm",
    feature = "openssl-crypto",
    feature = "ring-crypto"
)))]
#[tokio::test]
async fn portable_peer_manager_rejects_requested_unavailable_aes() {
    let mut config = PortablePeerManagerConfig::new(portable_runtime_config("portable-net"));
    config.snapshot.flags.enable_encryption = true;
    config.snapshot.flags.encryption_algorithm = "aes-gcm".to_owned();

    let error = build_portable_config_for_test(config).err().unwrap();

    assert_eq!(
        error.to_string(),
        "encryption algorithm is unavailable in this build: aes-gcm"
    );
}

#[tokio::test]
async fn portable_peer_manager_rejects_unknown_encryption_algorithm() {
    let mut config = PortablePeerManagerConfig::new(portable_runtime_config("portable-net"));
    config.snapshot.flags.enable_encryption = true;
    config.snapshot.flags.encryption_algorithm = "rot13".to_owned();

    let error = build_portable_config_for_test(config).err().unwrap();

    assert_eq!(error.to_string(), "invalid encryption algorithm: rot13");
}

#[tokio::test]
async fn unknown_ipv6_has_no_peer_destination() {
    let core = build_portable_for_test(portable_runtime_config("ipv6-net")).unwrap();
    let unknown = "fd00::2".parse().unwrap();

    let (peers, is_self) = core.get_msg_dst_peer_ipv6(&unknown).await;

    assert!(peers.is_empty());
    assert!(!is_self);
}

/// Minimal IPv4+UDP frame aimed at `dst`.
///
/// `ZCPacket::new_nic_packet()` on its own allocates no header space, so
/// `payload_len()` underflows; going through `new_with_payload` produces the
/// shape the NIC pipeline really hands the router, and keeps
/// `flow_key_from_payload` on its normal IP-5-tuple path.
fn nic_packet_to(dst: Ipv4Addr) -> ZCPacket {
    let mut frame = vec![0u8; 28];
    frame[0] = 0x45; // IPv4, 20-byte header
    frame[9] = 6; // UDP
    frame[12..16].copy_from_slice(&dst.octets());
    frame[16..20].copy_from_slice(&[10, 0, 0, 1]); // source
    frame[20..22].copy_from_slice(&40000u16.to_be_bytes()); // src port
    frame[22..24].copy_from_slice(&53u16.to_be_bytes()); // dst port
    ZCPacket::new_with_payload(&frame)
}

/// Counter-conservation guard for the pending-packet flusher (P-AUDIT.12).
///
/// The flusher retries a parked packet with `allow_buffer = false`. Were that
/// attempt to count a drop before handing the packet back for `put_back`, every
/// retry would inflate `data_plane_no_dst_peer_drops` — and that is precisely
/// the counter the roadmap tells operators to watch for improvement, so packets
/// that were in fact buffered would read as pure loss.
#[tokio::test]
async fn flusher_retry_is_not_counted_as_a_drop() {
    let core = build_portable_for_test(portable_runtime_config("flush-net")).unwrap();
    let router = &core.outbound_packet_router;

    // Nothing in a fresh network advertises this address, so the lookup really
    // does take the "no route" branch rather than passing by luck.
    let unroutable: Ipv4Addr = "10.253.0.1".parse().unwrap();
    assert!(
        router
            .get_msg_dst_peer_ipv4(&unroutable, None)
            .await
            .0
            .is_empty(),
        "precondition: the destination must have no route"
    );

    let drops_before = router.counters.no_dst_peer_drops.get();
    let buffered_before = router.counters.no_dst_peer_buffered.get();

    let mut msg = nic_packet_to(unroutable);
    msg.fill_peer_manager_hdr(router.my_peer_id, 0, PacketType::Data as u8);
    let status = router
        .send_msg_by_ip_after_pipeline(msg, IpAddr::V4(unroutable), true, false)
        .await
        .expect("an unroutable destination is not a send error");

    assert!(
        matches!(status, SendByIpStatus::StillUnroutable(_)),
        "the flusher needs the packet handed back so it can put_back until the \
         original TTL, not an opaque Done that looks like a successful send"
    );
    assert_eq!(
        router.counters.no_dst_peer_drops.get(),
        drops_before,
        "a retry that will be re-parked must not be counted as a drop"
    );
    assert_eq!(
        router.counters.no_dst_peer_buffered.get(),
        buffered_before,
        "a flush attempt must never re-enter the buffering path"
    );

    // Contrasting arm: with buffering allowed the packet is parked instead, so
    // `buffered` moves and `drops` still does not.
    let mut msg = nic_packet_to(unroutable);
    msg.fill_peer_manager_hdr(router.my_peer_id, 0, PacketType::Data as u8);
    let status = router
        .send_msg_by_ip_after_pipeline(msg, IpAddr::V4(unroutable), true, true)
        .await
        .expect("parking is not a send error");

    assert!(
        matches!(status, SendByIpStatus::Done),
        "a parked packet leaves the router's hands"
    );
    assert_eq!(
        router.counters.no_dst_peer_buffered.get(),
        buffered_before + 1
    );
    assert_eq!(router.counters.no_dst_peer_drops.get(), drops_before);
    assert_eq!(
        router.pending_packets.take_all(Instant::now()).ready.len(),
        1,
        "the packet must actually be parked rather than silently dropped"
    );
}

#[tokio::test]
async fn foreign_network_stop_waits_for_inflight_admission() {
    let core = build_portable_for_test(portable_runtime_config("portable-net")).unwrap();
    let manager = core.foreign_network_manager.clone();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let admission_manager = manager.clone();
    let admission_entered = entered.clone();
    let admission_release = release.clone();
    let admission = tokio::spawn(async move {
        admission_manager
            .hold_admission_for_test(admission_entered, admission_release)
            .await
    });
    entered.notified().await;

    let stop_manager = manager.clone();
    let stop = tokio::spawn(async move { stop_manager.stop().await });
    tokio::task::yield_now().await;
    assert!(!stop.is_finished());

    release.notify_waiters();
    admission.await.unwrap().unwrap();
    stop.await.unwrap();
    assert!(manager.is_stopped_for_test().await);
    assert!(!manager.admission_is_open_for_test().await);

    core.clear_resources().await;
}

#[tokio::test]
async fn portable_peer_manager_uses_host_context_adapters() {
    let config = PortablePeerManagerConfig::new(portable_runtime_config("portable-net"));
    let runtime_config = CoreRuntimeConfigStore::new(
        CoreRuntimeConfig::default(),
        Arc::new(config.snapshot.clone()),
    );
    let public_ipv6_runtime =
        CorePublicIpv6Runtime::new(runtime_config.clone(), Arc::new(()), Arc::new(()));
    let events = Arc::new(CountingPeerEventSink::default());
    let (packet_tx, _packet_rx) = host_packet_channel();

    let core = PeerManagerCore::new(
        config,
        Vec::new(),
        runtime_config,
        Arc::new(()),
        packet_tx,
        public_ipv6_runtime,
        events.clone(),
        None,
        Arc::new(()),
    )
    .unwrap();

    core.context.issue_event(PeerEvent::PeerAdded(99));
    assert_eq!(events.0.load(Ordering::Relaxed), 1);
    core.clear_resources().await;
}

#[tokio::test]
async fn portable_peer_assembly_preserves_submitted_acl_groups() {
    let mut config = PortablePeerManagerConfig::new(portable_runtime_config("portable-net"));
    let acl = crate::proto::acl::Acl {
        acl_v1: Some(crate::proto::acl::AclV1 {
            chains: Vec::new(),
            group: Some(crate::proto::acl::GroupInfo {
                declares: vec![crate::proto::acl::GroupIdentity {
                    group_name: "ops".to_owned(),
                    group_secret: "ops-secret".to_owned(),
                }],
                members: vec!["ops".to_owned()],
            }),
        }),
    };
    config.snapshot.set_acl_groups(Some(&acl));
    let (packet_tx, _packet_rx) = host_packet_channel();

    let core = PeerManagerCore::new_portable_for_test(config, packet_tx).unwrap();

    let groups = core.context.peer_groups(86);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].group_name, "ops");
    assert!(groups[0].verify("ops-secret", 86));
    assert_eq!(core.context.acl_group_declarations()[0].group_name, "ops");
    core.clear_resources().await;
}

#[tokio::test]
async fn credential_peer_policy_sync_excludes_group_material() {
    let mut runtime = portable_runtime_config("portable-net");
    runtime.network_identity.network_secret = None;
    runtime.network_identity.network_secret_digest = None;
    runtime.secure_mode = Some(credential_secure_mode());
    let core = Arc::new(build_portable_for_test(runtime).unwrap());

    let acl = crate::proto::acl::Acl {
        acl_v1: Some(crate::proto::acl::AclV1 {
            chains: Vec::new(),
            group: Some(crate::proto::acl::GroupInfo {
                declares: vec![crate::proto::acl::GroupIdentity {
                    group_name: "ops".to_owned(),
                    group_secret: "ops-secret".to_owned(),
                }],
                members: vec!["ops".to_owned()],
            }),
        }),
    };
    let mut services = CoreRuntimeConfig::default();
    services.acl.acl = Some(acl.clone());
    let mut source_peer = core.runtime_config.snapshot().peer.as_ref().clone();
    source_peer.set_acl_groups(Some(&acl));
    let source = CoreRuntimeConfigStore::new(services, Arc::new(source_peer));

    core.follow_network_policy(source.clone(), vec!["ops".to_owned()])
        .await
        .unwrap();

    let applied = core.runtime_config.snapshot();
    assert!(applied.peer.acl_group_declarations.is_empty());
    assert!(applied.peer.peer_group_memberships.is_empty());
    assert!(
        applied
            .services
            .acl
            .acl
            .as_ref()
            .unwrap()
            .acl_v1
            .as_ref()
            .unwrap()
            .group
            .is_none()
    );

    source.update_services(|services| {
        services.acl.tcp_whitelist = vec!["22".to_owned()];
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let applied = core.runtime_config.snapshot();
            if applied.services.acl.tcp_whitelist == ["22"] {
                assert!(
                    applied
                        .services
                        .acl
                        .acl
                        .as_ref()
                        .unwrap()
                        .acl_v1
                        .as_ref()
                        .unwrap()
                        .group
                        .is_none()
                );
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    core.clear_resources().await;
}

#[tokio::test]
async fn node_snapshot_exposes_normalized_runtime_state() {
    let instance_id = uuid::Uuid::from_u128(0x00112233445566778899aabbccddeeff);
    let mut runtime = portable_runtime_config("portable-net");
    runtime.core.node.instance_id = Some(*instance_id.as_bytes());
    runtime.core.node.hostname = Some("portable-node".to_owned());
    runtime.core.routes.ipv4 = Some(IpPrefix::new("10.20.0.91".parse().unwrap(), 16).unwrap());
    runtime.core.routes.proxy_networks = vec![ProxyNetworkConfig {
        real: IpPrefix::new("10.40.0.0".parse().unwrap(), 16).unwrap(),
        mapped: Some(IpPrefix::new("10.50.0.0".parse().unwrap(), 16).unwrap()),
    }];
    runtime.stun_info.public_ip = vec!["192.0.2.91".to_owned()];
    let core = build_portable_for_test(runtime).unwrap();
    let listener = Url::parse("tcp://0.0.0.0:11010").unwrap();

    let snapshot = core.node_snapshot(vec![listener.clone()]).await;

    assert_eq!(snapshot.peer_id, core.my_peer_id());
    assert_eq!(snapshot.instance_id, instance_id);
    assert_eq!(snapshot.hostname, "portable-node");
    assert_eq!(snapshot.ipv4_addr, Some("10.20.0.91/16".parse().unwrap()));
    assert_eq!(snapshot.proxy_networks.len(), 1);
    assert_eq!(snapshot.listeners, vec![listener]);
    assert_eq!(snapshot.stun_info.public_ip, vec!["192.0.2.91"]);
    assert_eq!(snapshot.version, env!("CARGO_PKG_VERSION"));
    assert!(snapshot.public_ipv6_addr.is_none());
    assert!(snapshot.ipv6_public_addr_prefix.is_none());

    let dns_identity = core.dns_route_identity();
    assert_eq!(
        dns_identity,
        (
            "portable-node".to_owned(),
            Some("10.20.0.91/16".parse::<cidr::Ipv4Inet>().unwrap().into()),
            String::new(),
        )
    );
}

#[tokio::test]
async fn portable_peer_manager_auth_uses_managed_credentials() {
    let admin_a = build_portable_for_test(portable_runtime_config("portable-net")).unwrap();
    let admin_b = build_portable_for_test(portable_runtime_config("portable-net")).unwrap();
    let generated = admin_a.credential_manager().generate_credential(
        vec!["guest".to_owned()],
        false,
        Vec::new(),
        Duration::from_secs(3600),
    );
    let private_bytes: [u8; 32] = BASE64_STANDARD
        .decode(generated.secret)
        .unwrap()
        .try_into()
        .unwrap();
    let public_key = PublicKey::from(&StaticSecret::from(private_bytes));

    assert!(
        admin_a
            .context
            .is_pubkey_trusted(public_key.as_bytes(), "portable-net")
    );
    assert!(
        !admin_a
            .context
            .is_pubkey_trusted(public_key.as_bytes(), "other")
    );
    let trusted = admin_a.context.trusted_credential_pubkeys("secret");
    assert_eq!(trusted.len(), 1);

    let propagated_key = trusted[0].credential.as_ref().unwrap().pubkey.clone();
    admin_b.context.update_trusted_keys(
        std::collections::HashMap::from([(
            propagated_key.clone(),
            crate::peers::context::TrustedKeyMetadata {
                source: crate::peers::context::TrustedKeySource::OspfCredential,
                expiry_unix: None,
            },
        )]),
        "portable-net",
    );
    assert!(
        admin_b
            .context
            .is_pubkey_trusted(&propagated_key, "portable-net")
    );
    assert!(admin_b.context.is_pubkey_trusted_with_source(
        &propagated_key,
        "portable-net",
        crate::peers::context::TrustedKeySource::OspfCredential,
    ));
    assert!(!admin_b.context.is_pubkey_trusted_with_source(
        &propagated_key,
        "portable-net",
        crate::peers::context::TrustedKeySource::OspfNode,
    ));

    assert!(
        admin_a
            .credential_manager()
            .revoke_credential(&generated.credential_id)
            .unwrap()
    );
    admin_b
        .context
        .update_trusted_keys(std::collections::HashMap::new(), "portable-net");
    assert!(
        !admin_b
            .context
            .is_pubkey_trusted(&propagated_key, "portable-net")
    );
}

#[tokio::test]
async fn portable_host_policy_controls_local_exit_node_fallback() {
    let external_ipv4 = Ipv4Addr::new(203, 0, 113, 10);
    let default_core = build_portable_for_test(portable_runtime_config("portable-net")).unwrap();
    assert_eq!(
        default_core.get_msg_dst_peer_ipv4(&external_ipv4).await,
        (Vec::new(), false)
    );

    let mut runtime = portable_runtime_config("portable-net");
    runtime.host_routing.local_exit_node_fallback = true;
    let fallback_core = build_portable_for_test(runtime).unwrap();
    assert_eq!(
        fallback_core.get_msg_dst_peer_ipv4(&external_ipv4).await,
        (vec![fallback_core.my_peer_id()], true)
    );
}

#[tokio::test]
async fn portable_peer_manager_rejects_inconsistent_network_names() {
    let mut runtime = portable_runtime_config("identity-net");
    runtime.core.node.network_name = "node-net".to_owned();
    let (packet_tx, _packet_rx) = host_packet_channel();

    let result =
        PeerManagerCore::new_portable_for_test(PortablePeerManagerConfig::new(runtime), packet_tx);

    assert!(result.is_err());
}

#[tokio::test]
async fn portable_peer_manager_rejects_unavailable_config_capabilities() {
    let mut digest_mismatch = portable_runtime_config("portable-net");
    digest_mismatch.network_identity.network_secret_digest = Some([1; 32]);
    assert!(build_portable_for_test(digest_mismatch).is_err());

    let mut secure_without_keys = portable_runtime_config("portable-net");
    secure_without_keys.secure_mode = Some(crate::proto::common::SecureModeConfig {
        enabled: true,
        ..Default::default()
    });
    assert!(build_portable_for_test(secure_without_keys).is_err());

    let mut mismatched_keys = portable_runtime_config("portable-net");
    mismatched_keys.secure_mode = Some(credential_secure_mode());
    mismatched_keys
        .secure_mode
        .as_mut()
        .unwrap()
        .local_public_key = Some(BASE64_STANDARD.encode([9; 32]));
    assert!(build_portable_for_test(mismatched_keys).is_err());

    let mut credential_without_secure_mode = portable_runtime_config("portable-net");
    credential_without_secure_mode
        .network_identity
        .network_secret = None;
    credential_without_secure_mode
        .network_identity
        .network_secret_digest = None;
    assert!(build_portable_for_test(credential_without_secure_mode).is_err());
}

#[tokio::test]
async fn portable_peer_manager_accepts_credential_client_config() {
    let mut runtime = portable_runtime_config("portable-net");
    runtime.network_identity.network_secret = None;
    runtime.network_identity.network_secret_digest = None;
    runtime.secure_mode = Some(credential_secure_mode());

    let core = build_portable_for_test(runtime).unwrap();
    assert!(core.context.feature_flags().is_credential_peer);
    assert!(core.context.network_identity().network_secret.is_none());
    assert!(core.is_secure_mode_enabled);
    core.clear_resources().await;
}

#[tokio::test]
async fn portable_peer_manager_accepts_legacy_unlimited_limits() {
    let runtime = portable_runtime_config("portable-net");
    let mut flags = PortablePeerManagerConfig::new(runtime.clone())
        .snapshot
        .flags;
    flags.instance_recv_bps_limit = u64::MAX;
    flags.foreign_relay_bps_limit = u64::MAX;
    let mut config = PortablePeerManagerConfig::new(runtime.clone());
    config.snapshot = PeerRuntimeSnapshot::new(runtime, flags);

    let core = build_portable_config_for_test(config).unwrap();
    assert!(core.context.recv_limiter("portable-net", false).is_none());
    assert!(core.context.recv_limiter("foreign-net", true).is_none());
    core.clear_resources().await;
}

#[tokio::test]
async fn portable_peer_manager_builds_configured_recv_limiters() {
    let mut runtime = portable_runtime_config("portable-net");
    runtime.core.traffic.instance_recv_bps_limit = Some(1024);
    runtime.core.traffic.foreign_relay_bps_limit = Some(2048);
    let core = build_portable_for_test(runtime).unwrap();

    let instance_a = core.context.recv_limiter("portable-net", false).unwrap();
    let instance_b = core.context.recv_limiter("other-net", false).unwrap();
    assert!(Arc::ptr_eq(&instance_a, &instance_b));

    let foreign_a = core.context.recv_limiter("foreign-a", true).unwrap();
    let foreign_a_again = core.context.recv_limiter("foreign-a", true).unwrap();
    let foreign_b = core.context.recv_limiter("foreign-b", true).unwrap();
    let foreign_named_instance = core.context.recv_limiter("instance", true).unwrap();
    assert!(Arc::ptr_eq(&foreign_a, &foreign_a_again));
    assert!(!Arc::ptr_eq(&foreign_a, &foreign_b));
    assert!(!Arc::ptr_eq(&instance_a, &foreign_a));
    assert!(!Arc::ptr_eq(&instance_a, &foreign_named_instance));

    core.clear_resources().await;
    assert!(core.context.recv_limiter("portable-net", false).is_none());
    assert!(core.context.recv_limiter("foreign-a", true).is_none());
}

#[tokio::test]
async fn portable_peer_manager_rejects_invalid_identity_and_prefixes() {
    let mut digest_only = portable_runtime_config("portable-net");
    digest_only.network_identity.network_secret = None;
    digest_only.network_identity.network_secret_digest = Some([1; 32]);
    assert!(build_portable_for_test(digest_only).is_err());

    let mut wrong_family = portable_runtime_config("portable-net");
    wrong_family.core.routes.ipv4 = Some(IpPrefix {
        address: "2001:db8::1".parse().unwrap(),
        prefix_len: 64,
    });
    assert!(build_portable_for_test(wrong_family).is_err());

    let mut proxy_host_bits = portable_runtime_config("portable-net");
    proxy_host_bits.core.routes.proxy_networks = vec![crate::config::ProxyNetworkConfig {
        real: IpPrefix::new("10.50.0.7".parse().unwrap(), 16).unwrap(),
        mapped: None,
    }];
    assert!(build_portable_for_test(proxy_host_bits).is_err());

    let mut wrong_proxy_family = portable_runtime_config("portable-net");
    wrong_proxy_family.core.routes.proxy_networks = vec![crate::config::ProxyNetworkConfig {
        real: IpPrefix::new("10.50.0.0".parse().unwrap(), 16).unwrap(),
        mapped: Some(IpPrefix::new("2001:db8::".parse().unwrap(), 64).unwrap()),
    }];
    assert!(build_portable_for_test(wrong_proxy_family).is_err());

    let mut advertised = portable_runtime_config("portable-net");
    advertised
        .core
        .routes
        .advertised_routes
        .push(IpPrefix::new("10.60.0.0".parse().unwrap(), 16).unwrap());
    assert!(build_portable_for_test(advertised).is_err());

    let mut foreign = portable_runtime_config("portable-net");
    foreign
        .core
        .routes
        .foreign_networks
        .push(crate::config::ForeignNetworkConfig {
            name: "other-net".to_owned(),
            cidrs: Vec::new(),
        });
    assert!(build_portable_for_test(foreign).is_err());
}

#[test]
fn portable_peer_manager_reports_missing_tokio_runtime() {
    let result = build_portable_for_test(portable_runtime_config("portable-net"));

    let Err(error) = result else {
        panic!("construction outside Tokio must fail");
    };
    assert!(error.to_string().contains("entered Tokio runtime"));
}

#[test]
fn recent_traffic_fanout_policy_only_marks_single_peer() {
    assert!(should_mark_recent_traffic_for_fanout(0));
    assert!(should_mark_recent_traffic_for_fanout(1));
    assert!(!should_mark_recent_traffic_for_fanout(2));
}

// ---------------------------------------------------------------------------
// Self-delivery guard for `not_send_to_tun` / `no_proxy`.
//
// Regression for the P0 where `send_probe_msg_on_conn` set both flags on a
// packet bound for a *remote* peer. On the far side that makes
// `NicPacketProcessor` skip the TUN (`!is_not_send_to_tun()` is false) and
// `IcmpProxyEngine::handle_peer_packet` pass on `is_no_proxy`, so no Echo Reply
// is ever produced and every per-link VIP probe reports "unreachable".
// ---------------------------------------------------------------------------

const MY_ID: PeerId = 1;
const REMOTE_ID: PeerId = 2;

#[test]
fn self_delivery_guard_is_not_applied_to_a_remote_peer() {
    // The P0: destination is another node, so a probe must reach its TUN / ICMP
    // proxy and be answered.
    assert!(!needs_self_delivery_guard(
        false, true, REMOTE_ID, MY_ID, false
    ));
}

#[test]
fn self_delivery_guard_is_applied_to_self_delivery() {
    // Proxy-induced self-delivery to a non-local address: keep the loop guard.
    // Args: (local_exit_node_fallback, not_send_to_self, to, my, is_local_vip)
    assert!(needs_self_delivery_guard(false, true, MY_ID, MY_ID, false));
}

#[test]
fn self_delivery_guard_respects_local_vip_and_exit_fallback() {
    // A local VIP is delivered to the host stack on purpose.
    assert!(!needs_self_delivery_guard(false, true, MY_ID, MY_ID, true));
    // `local_exit_node_fallback` disables the guard wholesale.
    assert!(!needs_self_delivery_guard(true, true, MY_ID, MY_ID, false));
    // A caller that does want self-delivery (`not_send_to_self == false`)
    // never gets the flags.
    assert!(!needs_self_delivery_guard(
        false, false, MY_ID, MY_ID, false
    ));
}

fn resolved_remote_addr_from_url(addr: Option<&str>) -> Option<ProtoUrl> {
    addr.map(|addr| Url::parse(addr).unwrap().into())
}

#[test]
fn resolved_remote_addr_check_rejects_virtual_network_ip() {
    let context: ArcPeerContext = Arc::new(SameNetworkContext {
        contains_every_address: false,
    });
    let resolved_remote_addr = resolved_remote_addr_from_url(Some("tcp://10.144.0.2:1234"));

    let err = check_resolved_remote_addr_not_from_virtual_network(&context, resolved_remote_addr);

    assert!(matches!(err, Err(Error::Other(_))));
}

#[test]
fn resolved_remote_addr_check_allows_external_or_non_ip_sources() {
    let context: ArcPeerContext = Arc::new(SameNetworkContext {
        contains_every_address: false,
    });
    for resolved_remote_addr_url in [
        Some("tcp://192.0.2.10:1234"),
        Some("tcp://example.test:1234"),
        Some("ring://peer"),
        Some("unix:///tmp/easytier.sock"),
        None,
    ] {
        assert!(
            check_resolved_remote_addr_not_from_virtual_network(
                &context,
                resolved_remote_addr_from_url(resolved_remote_addr_url),
            )
            .is_ok()
        );
    }
}

#[test]
fn resolved_remote_addr_check_allows_loopback_inside_virtual_network() {
    let context: ArcPeerContext = Arc::new(SameNetworkContext {
        contains_every_address: true,
    });
    let resolved_remote_addr = resolved_remote_addr_from_url(Some("tcp://127.0.0.1:1234"));

    let ret = check_resolved_remote_addr_not_from_virtual_network(&context, resolved_remote_addr);

    assert!(ret.is_ok());
}

fn route_with_ipv4(
    peer_id: u32,
    ipv4_addr: Option<std::net::Ipv4Addr>,
) -> crate::proto::core_peer::peer::Route {
    crate::proto::core_peer::peer::Route {
        peer_id,
        ipv4_addr: ipv4_addr.map(|addr| cidr::Ipv4Inet::new(addr, 24).unwrap().into()),
        ..Default::default()
    }
}

#[test]
fn ipv4_broadcast_peer_selection_skips_peers_without_ipv4() {
    let routes = vec![
        route_with_ipv4(1, Some(std::net::Ipv4Addr::new(10, 126, 126, 1))),
        route_with_ipv4(2, None),
        route_with_ipv4(3, Some(std::net::Ipv4Addr::new(10, 126, 126, 3))),
        route_with_ipv4(4, None),
    ];

    assert_eq!(
        PeerOutboundPacketRouter::select_ipv4_broadcast_peers(&routes, 3),
        vec![1]
    );
}

#[test]
fn gc_recent_traffic_removes_expired_and_connected_entries() {
    let stale_peer = 1;
    let direct_peer = 2;
    let active_peer = 3;
    let recent_have_traffic = DashMap::new();

    recent_have_traffic.insert(
        stale_peer,
        Instant::now() - RECENT_HAVE_TRAFFIC_TTL - Duration::from_millis(1),
    );
    recent_have_traffic.insert(direct_peer, Instant::now());
    recent_have_traffic.insert(active_peer, Instant::now());

    let future_peer = 4;
    recent_have_traffic.insert(future_peer, Instant::now() + Duration::from_secs(1));

    gc_recent_traffic_entries(&recent_have_traffic, Instant::now(), |peer_id| {
        peer_id == direct_peer
    });

    assert!(!recent_have_traffic.contains_key(&stale_peer));
    assert!(!recent_have_traffic.contains_key(&direct_peer));
    assert!(recent_have_traffic.contains_key(&active_peer));
    assert!(recent_have_traffic.contains_key(&future_peer));
}

#[test]
fn recent_traffic_notifies_only_when_demand_becomes_active() {
    let tracker = RecentTrafficTracker::new(1);
    let peer_id = 2;
    let signal = tracker.p2p_demand_notify();

    let initial_version = signal.version();
    tracker.mark(peer_id, false, true, |_| false);
    assert_eq!(signal.version(), initial_version + 1);

    let first_seen = *tracker.recent_have_traffic.get(&peer_id).unwrap();
    std::thread::sleep(Duration::from_millis(5));
    tracker.mark(peer_id, false, true, |_| false);
    assert_eq!(
        signal.version(),
        initial_version + 1,
        "fresh demand should not wake all p2p workers again"
    );
    let refreshed_seen = *tracker.recent_have_traffic.get(&peer_id).unwrap();
    assert!(refreshed_seen > first_seen);

    if let Some(mut last_seen) = tracker.recent_have_traffic.get_mut(&peer_id) {
        *last_seen = Instant::now() - RECENT_HAVE_TRAFFIC_TTL - Duration::from_millis(1);
    }
    tracker.mark(peer_id, false, true, |_| false);
    assert_eq!(signal.version(), initial_version + 2);
}

#[test]
fn recent_traffic_tolerates_future_timestamps() {
    let tracker = RecentTrafficTracker::new(1);
    let peer_id = 2;
    tracker
        .recent_have_traffic
        .insert(peer_id, Instant::now() + Duration::from_secs(1));

    assert!(tracker.has(peer_id, Instant::now(), |_| false));
    tracker.mark(peer_id, false, true, |_| false);
}

/// Three-tick priority sequence: preferred A drops → failover to B → A recovers
/// and must be selected again. Without the `select_by == Quality` gate on the
/// switch band, costs are all 0 and the band would hold B forever.
#[test]
fn priority_failover_returns_to_preferred_peer() {
    use crate::config::PeerId;
    use crate::peers::conn::conn_select::ConnSelectConfig;
    use crate::peers::route::local_route_table::SelectBy;

    let cfg = ConnSelectConfig::default();
    let cost = |peer: PeerId| -> Option<i32> {
        match peer {
            1 | 2 => Some(0), // both reachable; priority never consults cost
            _ => None,
        }
    };
    let eligible = |_: i32| true;

    // Tick 1: no incumbent, A preferred → adopt A.
    let (chosen, challenger) = select_reconciled_peer(
        SelectBy::Priority,
        None,
        Some(1),
        &cost,
        &eligible,
        None,
        cfg,
    );
    assert_eq!((chosen, challenger), (Some(1), None));

    // Tick 2: A gone, B is best → fail over immediately.
    let cost_a_down = |peer: PeerId| -> Option<i32> {
        match peer {
            2 => Some(0),
            _ => None,
        }
    };
    let (chosen, challenger) = select_reconciled_peer(
        SelectBy::Priority,
        Some(1),
        Some(2),
        &cost_a_down,
        &eligible,
        None,
        cfg,
    );
    assert_eq!((chosen, challenger), (Some(2), None));

    // Tick 3: A recovered (still preferred by list order) → must switch back.
    let (chosen, challenger) = select_reconciled_peer(
        SelectBy::Priority,
        Some(2),
        Some(1),
        &cost,
        &eligible,
        None,
        cfg,
    );
    assert_eq!((chosen, challenger), (Some(1), None));
}

#[test]
fn quality_switch_band_still_holds_incumbent() {
    use crate::config::PeerId;
    use crate::peers::conn::conn_select::ConnSelectConfig;
    use crate::peers::route::local_route_table::SelectBy;

    let cfg = ConnSelectConfig {
        switch_windows: 3,
        ..ConnSelectConfig::default()
    };
    // Incumbent cost 100, challenger 95 — inside the default band.
    let cost = |peer: PeerId| -> Option<i32> {
        match peer {
            1 => Some(100),
            2 => Some(95),
            _ => None,
        }
    };
    let eligible = |_: i32| true;

    let (chosen, challenger) = select_reconciled_peer(
        SelectBy::Quality,
        Some(1),
        Some(2),
        &cost,
        &eligible,
        None,
        cfg,
    );
    assert_eq!((chosen, challenger), (Some(1), None));
}

// ---------------------------------------------------------------------------
// L3: `failover_when = never` pin resolution
//
// `PeerId` is a random u32 reported by the peer itself, so it is neither
// stable across a restart nor unique over time. The pin therefore carries the
// remote static public key, and these tests pin down every branch of the
// decision — including the two that an id-only pin got wrong.
// ---------------------------------------------------------------------------

fn key(tag: u8) -> Vec<u8> {
    vec![tag; 32]
}

/// First pass: pin the first candidate and remember its key.
#[test]
fn never_pins_the_first_candidate_with_its_key() {
    let reachable = vec![(7u32, Some(key(1)))];
    assert_eq!(
        decide_pinned_peer(None, None, &reachable),
        Some(PinnedPeer::new(7, Some(key(1))))
    );
    // No candidate yet ⇒ nothing to pin.
    assert_eq!(decide_pinned_peer(None, None, &[]), None);
}

/// The node is down: the id is gone from the peer map and no candidate
/// resolves. The pin must **survive** — this is the whole point of `never`.
///
/// Before this fix the pin was dropped as soon as `has_peer` went false (about
/// three seconds after the disconnect), so the rule fell back to the physical
/// gateway within seconds.
#[test]
fn never_keeps_the_pin_while_the_node_is_down() {
    let pin = PinnedPeer::new(7, Some(key(1)));
    assert_eq!(
        decide_pinned_peer(Some(&pin), None, &[]),
        Some(pin.clone()),
        "a `never` rule must keep waiting for its peer, not fall through"
    );
}

/// The exit node restarted and came back with a new id but the same key: follow
/// it instead of black-holing the route forever.
#[test]
fn never_follows_the_same_node_under_a_new_id() {
    let pin = PinnedPeer::new(7, Some(key(1)));
    let reachable = vec![(99u32, Some(key(1)))];
    assert_eq!(
        decide_pinned_peer(Some(&pin), None, &reachable),
        Some(PinnedPeer::new(99, Some(key(1)))),
        "the same key under a new id is the same node"
    );
}

/// A different node claimed the old id: never route to a stranger.
#[test]
fn never_drops_the_pin_when_its_id_is_reused_by_another_node() {
    let pin = PinnedPeer::new(7, Some(key(1)));
    // Id 7 is live but now belongs to key 2; nothing else is reachable.
    let decided = decide_pinned_peer(Some(&pin), Some(Some(key(2))), &[]);
    assert_eq!(
        decided, None,
        "routing to whoever took the id would leak traffic to the wrong node"
    );

    // If a real candidate exists, the rule starts over from it.
    let reachable = vec![(42u32, Some(key(3)))];
    assert_eq!(
        decide_pinned_peer(Some(&pin), Some(Some(key(2))), &reachable),
        Some(PinnedPeer::new(42, Some(key(3))))
    );
}

/// Same id, same key: steady state, nothing changes.
#[test]
fn never_keeps_the_pin_when_id_and_key_both_match() {
    let pin = PinnedPeer::new(7, Some(key(1)));
    let reachable = vec![(7u32, Some(key(1)))];
    assert_eq!(
        decide_pinned_peer(Some(&pin), Some(Some(key(1))), &reachable),
        Some(pin)
    );
}

/// Without keys (plaintext / no-noise) a pin whose id is still live stays put.
///
/// This covers the "same id, still there" case only; what happens once the id
/// is *gone* is the separate question answered by
/// [`never_without_keys_follows_a_restarted_candidate`].
#[test]
fn never_without_keys_falls_back_to_id_matching() {
    let pin = PinnedPeer::new(7, None);
    assert_eq!(
        decide_pinned_peer(Some(&pin), Some(None), &[]),
        Some(pin.clone())
    );
    // Down, no key: still pinned.
    assert_eq!(decide_pinned_peer(Some(&pin), None, &[]), Some(pin));
}

/// After the last conn drops, the call site must feed `None` (not `Some(None)`).
///
/// `Peer` clears `peer_public_key` when its last conn goes away, while the map
/// entry survives ~3s. If `resolve_pinned_peer` used `has_peer`, that window
/// became `pinned_id_live = Some(None)` and a keyed pin was dropped as a
/// "stranger". With `has_peer_with_live_conn` the zombie maps to `None`, and
/// case 4 keeps waiting — matching traffic-steering §5 "节点只是掉线 ⇒ 锁定保留".
#[test]
fn never_keeps_a_keyed_pin_across_the_zombie_window_after_last_conn_drops() {
    let pin = PinnedPeer::new(7, Some(key(1)));
    assert_eq!(
        decide_pinned_peer(Some(&pin), None, &[]),
        Some(pin),
        "zombie entry (no live conn) must look like 'id gone', not 'stranger'"
    );
}

/// A keyed pin against a **live** keyless occupant is a mode mismatch
/// (`OccupantUnverifiable`), not a stranger — keep the pin. A true stranger
/// presents a **different** static key (`DifferentNode`).
#[test]
fn never_keeps_a_keyed_pin_when_a_live_plaintext_peer_holds_the_id() {
    let pin = PinnedPeer::new(7, Some(key(1)));
    assert_eq!(
        decide_pinned_peer(Some(&pin), Some(None), &[]),
        Some(pin),
        "keyless occupant of the pinned id is mode change, not proof of a stranger"
    );
}

/// A keyless (plaintext) pin against an occupant that later presents a key is
/// `PinUnverifiable`: there was never a verifiable identity to revoke. Keep
/// the pin; restart follow-along still uses case 3 when the id leaves the map.
#[test]
fn never_keeps_a_keyless_pin_when_the_occupant_later_presents_a_key() {
    let pin = PinnedPeer::new(7, None);
    assert_eq!(
        decide_pinned_peer(Some(&pin), Some(Some(key(1))), &[]),
        Some(pin),
        "plaintext pin must not reset just because the occupant gained a key"
    );
}

/// The classification behind the log wording: "a different node took the id"
/// and "the identity cannot be verified" are **different facts** — and they
/// must drive **different actions** (drop vs keep), not just different warn
/// strings on the same restart path.
#[test]
fn pin_classification_separates_a_stranger_from_an_unverifiable_identity() {
    use crate::peers::route::local_route_table::PinMatch;

    let keyed = PinnedPeer::new(7, Some(key(1)));
    let keyless = PinnedPeer::new(7, None);

    // Steady state: the same node, same id, same key.
    assert_eq!(keyed.classify(7, Some(&key(1))), PinMatch::Same);
    // Plaintext steady state: the id is the only handle there is.
    assert_eq!(keyless.classify(7, None), PinMatch::Same);

    // Both sides keyed, keys differ ⇒ **a different node** holds the id.
    assert_eq!(keyed.classify(7, Some(&key(2))), PinMatch::DifferentNode);

    // The pin is keyed but the occupant is not ⇒ the occupant cannot be
    // verified (plaintext conn, or encryption was switched on the far side).
    assert_eq!(
        keyed.classify(7, None),
        PinMatch::OccupantUnverifiable,
        "a keyed pin against a keyless occupant is not the same fact as a stranger"
    );

    // The pin is keyless but the occupant is keyed ⇒ there was never a
    // verifiable identity to compare.
    assert_eq!(
        keyless.classify(7, Some(&key(1))),
        PinMatch::PinUnverifiable
    );

    // An empty key is "no key" on either side, exactly like the normalised
    // `None` — the B1 defect must not come back through the classifier.
    assert_eq!(keyed.classify(7, Some(&[])), PinMatch::OccupantUnverifiable);
    assert_eq!(keyless.classify(99, Some(&[])), PinMatch::PinUnverifiable);
}

/// `matches` is the boolean view of `classify` and must stay exactly that —
/// the two must never drift apart.
#[test]
fn pin_matches_is_the_boolean_view_of_classify() {
    use crate::peers::route::local_route_table::PinMatch;

    let keyed = PinnedPeer::new(7, Some(key(1)));
    let keyless = PinnedPeer::new(7, None);

    for (pin, peer_id, key) in [
        (&keyed, 7u32, Some(key(1))),
        (&keyed, 7, Some(key(2))),
        (&keyed, 7, None),
        (&keyed, 99, Some(key(1))),
        (&keyless, 7, None),
        (&keyless, 99, None),
        (&keyless, 7, Some(key(1))),
    ] {
        assert_eq!(
            pin.matches(peer_id, key.as_deref()),
            pin.classify(peer_id, key.as_deref()) == PinMatch::Same,
            "matches and classify disagree for pin={pin:?} id={peer_id} key={key:?}"
        );
    }
}

/// B1: two plaintext peers must never compare equal by "empty key == empty
/// key".
///
/// `noise_remote_static_pubkey` is an empty `Vec` — not a missing one — for a
/// plaintext conn, and `Peer::add_conn` stores it verbatim. Before the keys were
/// normalised, a keyless pin carried `Some(vec![])`, every other plaintext peer
/// also presented `Some(vec![])`, and the two compared **equal**: a stranger
/// claiming the pinned id was accepted as the same node.
#[test]
fn an_empty_public_key_is_not_a_shared_identity() {
    // A keyless pin (normalised: `None`) matches only on the id.
    let pin = PinnedPeer::new(7, None);
    assert!(
        pin.matches(7, None),
        "the pinned id itself still matches the keyless pin"
    );
    assert!(
        !pin.matches(99, None),
        "a different id must not match just because both sides are keyless"
    );

    // The verification also holds for the value the wire actually produces:
    // if a caller ever hands us the raw empty `Vec`, it must not read as an
    // identity either. `Some([]) == Some([])` is the trap.
    let raw = PinnedPeer::new(7, Some(Vec::new()));
    assert!(
        !raw.matches(99, Some(&[])),
        "an empty key must not make unrelated plaintext peers equal"
    );

    // One side verified, the other not: never a match (mode disagreement).
    let keyed = PinnedPeer::new(7, Some(key(1)));
    assert!(!keyed.matches(7, None));
    assert!(!pin.matches(7, Some(&key(1))));
}

/// B2: in plaintext mode the pin must still **follow a restart**.
///
/// There is no verifiable identity here, so "the pinned node came back under a
/// new id" and "a stranger took the old id" look identical from the outside.
/// The choice (2026-10-20) is to follow the first reachable candidate: it keeps
/// restart survival, which is an everyday operation, instead of black-holing the
/// route forever on an unverifiable maybe-attack.
///
/// This must survive the empty-key normalisation: it is asserted directly, not
/// as a side effect of `[] == []`.
#[test]
fn never_without_keys_follows_a_restarted_candidate() {
    let pin = PinnedPeer::new(7, None);
    // The pinned id is gone; a candidate is reachable under a new one.
    assert_eq!(
        decide_pinned_peer(Some(&pin), None, &[(99u32, None)]),
        Some(PinnedPeer::new(99, None)),
        "a keyless pin follows the candidate rather than black-holing"
    );
    // Nothing reachable: still wait.
    assert_eq!(decide_pinned_peer(Some(&pin), None, &[]), Some(pin));
}

/// B1/B2 together: with real keys, the stranger is still refused — the strong
/// protection the keyed path always had must not be weakened by the keyless
/// branch above.
#[test]
fn never_with_keys_still_refuses_a_stranger_on_the_pinned_id() {
    let pin = PinnedPeer::new(7, Some(key(1)));
    assert!(
        decide_pinned_peer(Some(&pin), Some(Some(key(2))), &[]).is_none(),
        "a different key on the pinned id must drop the pin, not follow it"
    );
}

/// R4: which rules consult a cost — and therefore which rules may record one.
///
/// The cost memo is shared across rules, so `ResolvedRule::costed_peers` is
/// only meaningful if it records "this rule read a cost", not "a number existed
/// for this peer". This exercises **`rule_reads_cost` itself**, the function the
/// reconcile loop calls; there is no test-only stand-in that could drift from it.
#[test]
fn rule_reads_cost_covers_every_select_by_and_failover_when() {
    use crate::peers::route::local_route_table::{
        AllUnavailable, FailoverWhen, RouteRule, SelectBy,
    };

    // Build the compiled rule directly from its public fields: this is the
    // exact shape the reconcile loop sees, and the test calls `rule_reads_cost`
    // on it — no test-only twin that could drift from the production function.
    let rule = |select_by: SelectBy, failover_when: FailoverWhen| RouteRule {
        id: 0,
        cidr: Some("10.0.0.0/24".parse().unwrap()),
        domains: Vec::new(),
        peers: vec!["10.144.0.9".parse().unwrap()],
        merge_exit_nodes: false,
        select_by,
        failover_when,
        if_all_unavailable: AllUnavailable::Open,
        require_confirmed: false,
        quality_degraded_cost: Some(100),
    };

    // `quality` ranks by cost whatever the failover policy is.
    assert!(rule_reads_cost(&rule(
        SelectBy::Quality,
        FailoverWhen::Unreachable
    )));
    assert!(rule_reads_cost(&rule(
        SelectBy::Quality,
        FailoverWhen::Degraded
    )));
    // `degraded` needs a cost to compare against the ceiling, whatever selects.
    assert!(rule_reads_cost(&rule(
        SelectBy::Priority,
        FailoverWhen::Degraded
    )));
    assert!(rule_reads_cost(&rule(
        SelectBy::Balance,
        FailoverWhen::Degraded
    )));
    // Everything else decides by reachability / list order alone.
    assert!(!rule_reads_cost(&rule(
        SelectBy::Priority,
        FailoverWhen::Unreachable
    )));
    assert!(!rule_reads_cost(&rule(
        SelectBy::Priority,
        FailoverWhen::Never
    )));
    assert!(!rule_reads_cost(&rule(
        SelectBy::Balance,
        FailoverWhen::Unreachable
    )));
}

fn set_packet_destination(packet: &mut [u8], destination: IpAddr) {
    match destination {
        IpAddr::V4(destination) => {
            assert_eq!(packet[0] >> 4, 4);
            packet[16..20].copy_from_slice(&destination.octets());
            packet[10..12].fill(0);
            let mut checksum: u32 = packet[..20]
                .chunks_exact(2)
                .map(|word| u32::from(u16::from_be_bytes([word[0], word[1]])))
                .sum();
            while checksum >> 16 != 0 {
                checksum = (checksum & 0xffff) + (checksum >> 16);
            }
            packet[10..12].copy_from_slice(&(!(checksum as u16)).to_be_bytes());
        }
        IpAddr::V6(destination) => {
            assert_eq!(packet[0] >> 4, 6);
            packet[24..40].copy_from_slice(&destination.octets());
        }
    }
}

struct DestinationRewriteNicFilter(IpAddr);

#[async_trait::async_trait]
impl super::super::NicPacketFilter for DestinationRewriteNicFilter {
    async fn try_process_packet_from_nic(&self, packet: &mut ZCPacket) -> bool {
        // Leave to_peer_id unset: the router must use the rewritten IP header.
        assert_eq!(packet.peer_manager_header().unwrap().to_peer_id.get(), 0);
        set_packet_destination(packet.mut_payload(), self.0);
        true
    }
}

async fn check_nic_filter_destination_routing(addresses: [IpAddr; 4]) {
    let [source, original, rewritten, unrouted] = addresses;
    let mut peers = Vec::new();
    let mut receivers = Vec::new();
    for address in [source, original, rewritten] {
        let mut runtime = portable_runtime_config("nic-filter-routing");
        if address.is_ipv4() {
            runtime.core.routes.ipv4 = Some(IpPrefix::new(address, 24).unwrap());
        } else {
            runtime.core.routes.ipv6 = Some(IpPrefix::new(address, 64).unwrap());
        }
        let (sender, receiver) = host_packet_channel();
        let peer = PeerManagerCore::new_portable_for_test(
            PortablePeerManagerConfig::new(runtime),
            sender,
        )
        .unwrap();
        peer.run().await.unwrap();
        peers.push(peer);
        receivers.push(receiver);
    }
    let sender = &peers[0];
    for peer in &peers[1..] {
        let (client, server) = crate::tunnel::ring::create_ring_tunnel_pair();
        let (client_result, server_result) = tokio::join!(
            sender.add_client_tunnel(client, true),
            peer.add_tunnel_as_server(server, true),
        );
        client_result.unwrap();
        server_result.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if sender.get_msg_dst_peer(&original).await == (vec![peers[1].my_peer_id()], false)
                && sender.get_msg_dst_peer(&rewritten).await
                    == (vec![peers[2].my_peer_id()], false)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both destination routes must be ready before sending");
    assert_eq!(
        sender.get_msg_dst_peer(&unrouted).await,
        (Vec::new(), false)
    );
    sender
        .add_nic_packet_process_pipeline(Box::new(DestinationRewriteNicFilter(rewritten)))
        .await;

    // Exercise both misdelivery to an existing peer and silent loss for an
    // address with no route. Send once per case, without retrying data packets.
    for destination in [original, unrouted] {
        let mut payload = match source {
            IpAddr::V4(source) => {
                let mut payload = vec![0u8; 20];
                payload[0] = 0x45;
                payload[2..4].copy_from_slice(&20u16.to_be_bytes());
                payload[8] = 64;
                payload[9] = 253; // Experimental protocol; no transport header.
                payload[12..16].copy_from_slice(&source.octets());
                payload
            }
            IpAddr::V6(source) => {
                let mut payload = vec![0u8; 40];
                payload[0] = 0x60;
                payload[6] = 59; // No next header.
                payload[7] = 64;
                payload[8..24].copy_from_slice(&source.octets());
                payload
            }
        };
        set_packet_destination(&mut payload, destination);
        sender
            .send_msg_by_ip(ZCPacket::new_with_payload(&payload), destination, false)
            .await
            .unwrap();

        let packet = tokio::time::timeout(Duration::from_secs(5), receivers[2].recv())
            .await
            .expect("rewritten destination peer did not receive the packet")
            .unwrap()
            .into_core_packet();
        set_packet_destination(&mut payload, rewritten);
        assert_eq!(packet.payload(), payload);
        let header = packet.peer_manager_header().unwrap();
        assert_eq!(header.from_peer_id.get(), sender.my_peer_id());
        assert_eq!(header.to_peer_id.get(), peers[2].my_peer_id());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), receivers[1].recv())
                .await
                .is_err(),
            "packet reached the old destination"
        );
    }

    for peer in &peers {
        peer.clear_resources().await;
    }
}

#[tokio::test]
async fn nic_filter_rewritten_ipv4_destination_controls_routing() {
    check_nic_filter_destination_routing([
        "10.82.0.1".parse().unwrap(),
        "10.82.0.2".parse().unwrap(),
        "10.82.0.3".parse().unwrap(),
        "10.82.0.99".parse().unwrap(),
    ])
    .await;
}

#[tokio::test]
async fn nic_filter_rewritten_ipv6_destination_controls_routing() {
    check_nic_filter_destination_routing([
        "fd00::1".parse().unwrap(),
        "fd00::2".parse().unwrap(),
        "fd00::3".parse().unwrap(),
        "fd00::99".parse().unwrap(),
    ])
    .await;
}

#[tokio::test]
async fn packet_batches_resume_at_next_filter_in_order() {
    struct Filter {
        stage: u8,
        seen: Arc<std::sync::Mutex<Vec<(u8, u8)>>>,
    }
    #[async_trait::async_trait]
    impl PeerPacketFilter for Filter {
        async fn try_process_packet_from_peer(
            &self,
            packet: ZCPacket,
        ) -> PeerPacketFilterResult {
            self.seen
                .lock()
                .unwrap()
                .push((self.stage, packet.payload()[0]));
            match self.stage {
                0 => PeerPacketFilterResult::Consumed,
                1 | 2 => {
                    let mut extra = packet.clone();
                    extra.mut_payload()[0] += if self.stage == 1 { 10 } else { 1 };
                    PeerPacketFilterResult::PassBatch(vec![packet, extra])
                }
                _ => PeerPacketFilterResult::Pass(packet),
            }
        }
    }
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let pipelines = (0..4)
        .map(|stage| {
            permanent_peer_pipeline_entry(Box::new(Filter {
                stage,
                seen: seen.clone(),
            }))
        })
        .collect::<Vec<_>>();
    process_peer_packet(&pipelines, ZCPacket::new_with_payload(&[0])).await;
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            (3, 0),
            (2, 0),
            (1, 0),
            (0, 0),
            (0, 10),
            (1, 1),
            (0, 1),
            (0, 11),
        ]
    );
}
