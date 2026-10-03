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
    peers::context::{PeerContext, PeerEvent},
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
    let processor = NicPacketProcessor { nic_channel };
    let mut packet = ZCPacket::new_with_payload(b"next");
    packet.fill_peer_manager_hdr(1, 2, PacketType::Data as u8);

    let result = tokio::time::timeout(
        Duration::from_millis(100),
        processor.try_process_packet_from_peer(packet),
    )
    .await;

    assert!(result.is_ok(), "a full host queue blocked the peer router");
    assert!(result.unwrap().is_none());
    assert_eq!(nic_receiver.try_recv().unwrap().payload(), b"queued");
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

#[test]
fn disable_relay_data_classifies_data_plane_packets_only() {
    for packet_type in [
        PacketType::Data,
        PacketType::KcpSrc,
        PacketType::KcpDst,
        PacketType::QuicSrc,
        PacketType::QuicDst,
        PacketType::DataWithKcpSrcModified,
        PacketType::DataWithQuicSrcModified,
        PacketType::ForeignNetworkPacket,
    ] {
        assert!(is_relay_data_packet(packet_type as u8));
    }

    for packet_type in [
        PacketType::RpcReq,
        PacketType::RpcResp,
        PacketType::Ping,
        PacketType::Pong,
        PacketType::HandShake,
        PacketType::NoiseHandshakeMsg1,
        PacketType::NoiseHandshakeMsg2,
        PacketType::NoiseHandshakeMsg3,
        PacketType::RelayHandshake,
        PacketType::RelayHandshakeAck,
    ] {
        assert!(!is_relay_data_packet(packet_type as u8));
    }
}

#[test]
fn disable_relay_data_inspects_foreign_network_inner_packet_type() {
    let network_name = "net1".to_string();

    let mut rpc_packet = ZCPacket::new_with_payload(b"rpc");
    rpc_packet.fill_peer_manager_hdr(1, 2, PacketType::RpcReq as u8);
    let mut foreign_rpc_packet = ZCPacket::new_for_foreign_network(&network_name, 2, &rpc_packet);
    foreign_rpc_packet.fill_peer_manager_hdr(10, 20, PacketType::ForeignNetworkPacket as u8);

    assert_eq!(
        foreign_rpc_packet.foreign_network_inner_packet_type(),
        Some(PacketType::RpcReq as u8)
    );
    assert!(!is_relay_data_zc_packet(&foreign_rpc_packet));

    let mut data_packet = ZCPacket::new_with_payload(b"data");
    data_packet.fill_peer_manager_hdr(1, 2, PacketType::Data as u8);
    let mut foreign_data_packet = ZCPacket::new_for_foreign_network(&network_name, 2, &data_packet);
    foreign_data_packet.fill_peer_manager_hdr(10, 20, PacketType::ForeignNetworkPacket as u8);

    assert_eq!(
        foreign_data_packet.foreign_network_inner_packet_type(),
        Some(PacketType::Data as u8)
    );
    assert!(is_relay_data_zc_packet(&foreign_data_packet));
}

fn data_packet(from_peer_id: PeerId, to_peer_id: PeerId) -> ZCPacket {
    let mut packet = ZCPacket::new_with_payload(b"data");
    packet.fill_peer_manager_hdr(from_peer_id, to_peer_id, PacketType::Data as u8);
    packet
}

#[test]
fn forged_attached_source_header_does_not_bypass_relay_disable() {
    let packet = data_packet(77, 3);
    let network_ingress = PeerPacketIngress::Peer {
        peer_id: 2,
        conn_id: PeerConnId::new_v4(),
        origin: PeerConnectionOrigin::Network,
    };

    assert!(should_drop_relay_data(
        true,
        &packet,
        1,
        network_ingress,
        false,
    ));
}

#[test]
fn attached_ingress_and_destination_bypass_relay_disable() {
    let packet = data_packet(2, 3);
    let attached_ingress = PeerPacketIngress::Peer {
        peer_id: 2,
        conn_id: PeerConnId::new_v4(),
        origin: PeerConnectionOrigin::Attached,
    };
    let network_ingress = PeerPacketIngress::Peer {
        peer_id: 2,
        conn_id: PeerConnId::new_v4(),
        origin: PeerConnectionOrigin::Network,
    };

    assert!(!should_drop_relay_data(
        true,
        &packet,
        1,
        attached_ingress,
        false,
    ));
    assert!(!should_drop_relay_data(
        true,
        &packet,
        1,
        network_ingress,
        true,
    ));
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
