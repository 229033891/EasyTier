use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr as _;
use std::sync::Arc;
use std::time::Duration;

use cidr::Ipv4Inet;
use easytier_core::{
    gateway::magic_dns::MagicDnsRoute,
    host::packet::{HostPacket, HostPacketChannelSink, HostPacketReceiver},
    process_runtime::CoreProcessRuntime,
};
use hickory_client::client::{Client, ClientHandle as _};
use hickory_proto::rr;
use hickory_proto::runtime::TokioRuntimeProvider;
use hickory_proto::udp::UdpClientStream;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::common::global_ctx::{ArcGlobalCtx, tests::get_mock_global_ctx};
use crate::instance::{
    composition::{NativeCoreInstance, runtime_core_host_adapters},
    config::test_core_instance_config,
};

use crate::instance::dns_server::runner::DnsRunner;
use crate::instance::dns_server::server_instance::MagicDnsServerInstance;
use crate::instance::dns_server::{DEFAULT_ET_DNS_ZONE, MAGIC_DNS_FAKE_IP};
use crate::instance::virtual_nic::NicCtx;
use crate::proto::api::instance::Route;
use crate::proto::magic_dns::{MagicDnsServerRpc as _, UpdateDnsRecordRequest, dns_record};
use crate::proto::rpc_types::controller::{BaseController, Controller as _};

pub async fn prepare_env(
    dns_name: &str,
    tun_ip: Ipv4Inet,
) -> (ArcGlobalCtx, Arc<NativeCoreInstance>, NicCtx) {
    prepare_env_with_tld_dns_zone(dns_name, tun_ip, None).await
}

async fn build_test_core(ctx: ArcGlobalCtx) -> (Arc<NativeCoreInstance>, HostPacketReceiver) {
    let (packet_sink, packet_receiver) = tokio::sync::mpsc::channel::<HostPacket>(128);
    let adapters = runtime_core_host_adapters(
        ctx.clone(),
        CoreProcessRuntime::new(),
        Arc::new(HostPacketChannelSink::new(packet_sink)),
    );
    let core_instance = NativeCoreInstance::new(test_core_instance_config(&ctx), adapters).unwrap();
    core_instance.start().await.unwrap();
    (core_instance, HostPacketReceiver::new(packet_receiver))
}

pub async fn prepare_env_with_tld_dns_zone(
    dns_name: &str,
    tun_ip: Ipv4Inet,
    tld_dns_zone: Option<&str>,
) -> (ArcGlobalCtx, Arc<NativeCoreInstance>, NicCtx) {
    let ctx = get_mock_global_ctx();
    ctx.set_hostname(dns_name.to_owned());
    ctx.set_ipv4(Some(tun_ip));

    if tld_dns_zone.is_some() {
        let mut flags = ctx.config.get_flags();
        flags.accept_dns = true; // Enable DNS
        if let Some(zone) = tld_dns_zone {
            flags.tld_dns_zone = zone.to_string();
        }
        ctx.set_flags(flags);
    }

    let (core_instance, host_packet_rx) = build_test_core(ctx.clone()).await;
    let host_packet_rx = Arc::new(tokio::sync::Mutex::new(host_packet_rx));
    let mut virtual_nic = NicCtx::new(
        ctx.clone(),
        core_instance.packet_plane(),
        host_packet_rx,
        Arc::new(Notify::new()),
    );
    virtual_nic.run(Some(tun_ip), None).await.unwrap();

    (ctx, core_instance, virtual_nic)
}

pub async fn check_dns_record(fake_ip: &Ipv4Addr, domain: &str, expected_ip: &str) {
    let stream = UdpClientStream::builder(
        SocketAddr::new((*fake_ip).into(), 53),
        TokioRuntimeProvider::default(),
    )
    .build();
    let (mut client, background) = Client::connect(stream).await.unwrap();
    let background_task = tokio::spawn(background);
    let response = client
        .query(
            rr::Name::from_str(domain).unwrap(),
            rr::DNSClass::IN,
            rr::RecordType::A,
        )
        .await
        .unwrap_or_else(|e| panic!("DNS query failed unexpectedly for domain '{domain}': {e}"));
    background_task.abort();
    let _ = background_task.await;

    println!("Response: {:?}", response);

    assert_eq!(response.answers().len(), 1, "{:?}", response.answers());
    let resp = response.answers().first().unwrap();
    assert_eq!(
        resp.clone().into_parts().rdata.into_a().unwrap().0,
        expected_ip.parse::<Ipv4Addr>().unwrap()
    );
}

#[tokio::test]
async fn test_magic_dns_server_instance() {
    let tun_ip = Ipv4Inet::from_str("10.144.144.10/24").unwrap();
    let (global_ctx, core_instance, virtual_nic) = prepare_env("test1", tun_ip).await;
    let tun_name = virtual_nic.ifname().await.unwrap();
    let fake_ip = Ipv4Addr::from_str(MAGIC_DNS_FAKE_IP).unwrap();
    let dns_server_inst = MagicDnsServerInstance::new(
        core_instance.packet_plane(),
        global_ctx,
        Some(tun_name),
        tun_ip,
        fake_ip,
    )
    .await
    .unwrap();

    let routes = [
        MagicDnsRoute {
            hostname: "test1".to_string(),
            ipv4_addr: Some("8.8.8.8".parse().unwrap()),
        },
        MagicDnsRoute {
            hostname: "中文".to_string(),
            ipv4_addr: Some("8.8.8.8".parse().unwrap()),
        },
        MagicDnsRoute {
            hostname: ".invalid".to_string(),
            ipv4_addr: Some("8.8.8.8".parse().unwrap()),
        },
    ];
    dns_server_inst
        .data
        .update_dns_records(routes.iter(), DEFAULT_ET_DNS_ZONE)
        .await
        .unwrap();

    check_dns_record(&fake_ip, "test1.et.net", "8.8.8.8").await;
    check_dns_record(&fake_ip, "中文.et.net", "8.8.8.8").await;
}

#[tokio::test]
async fn test_magic_dns_runner() {
    // Test first runner with default DNS settings
    {
        let tun_ip = Ipv4Inet::from_str("10.144.144.10/24").unwrap();
        let (global_ctx, core_instance, virtual_nic) = prepare_env("test1", tun_ip).await;
        let tun_name = virtual_nic.ifname().await.unwrap();
        let fake_ip = Ipv4Addr::from_str(MAGIC_DNS_FAKE_IP).unwrap();
        let mut dns_runner = DnsRunner::new(
            core_instance.packet_plane(),
            global_ctx,
            Some(tun_name),
            tun_ip,
            fake_ip,
        );

        let cancel_token = CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();
        let t = tokio::spawn(async move {
            dns_runner.run(cancel_token_clone).await;
        });
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Test default settings: query should resolve test1.et.net to tunnel IP via default fake IP
        check_dns_record(&fake_ip, "test1.et.net", "10.144.144.10").await;

        cancel_token.cancel();
        t.await.unwrap();

        // Wait a bit for cleanup
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // Test second runner with different TLD zone
    {
        let tun_ip = Ipv4Inet::from_str("10.144.144.20/24").unwrap();
        // NOTE: Using same fake IP to avoid system DNS configuration conflicts
        let custom_tld_zone = "custom.local."; // Different TLD zone is safer
        let (global_ctx, core_instance, virtual_nic) =
            prepare_env_with_tld_dns_zone("test2", tun_ip, Some(custom_tld_zone)).await;
        let tun_name = virtual_nic.ifname().await.unwrap();
        let fake_ip = Ipv4Addr::from_str(MAGIC_DNS_FAKE_IP).unwrap();
        let mut dns_runner = DnsRunner::new(
            core_instance.packet_plane(),
            global_ctx,
            Some(tun_name),
            tun_ip,
            fake_ip,
        );

        let cancel_token = CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();
        let t = tokio::spawn(async move {
            dns_runner.run(cancel_token_clone).await;
        });
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Test with same fake IP but different TLD zone
        check_dns_record(&fake_ip, "test2.custom.local", "10.144.144.20").await;

        cancel_token.cancel();
        t.await.unwrap();
    }
}

#[tokio::test]
async fn test_magic_dns_update_replaces_records_for_same_client() {
    let tun_ip = Ipv4Inet::from_str("100.100.100.0/24").unwrap();
    let ctx = get_mock_global_ctx();
    ctx.set_hostname("test1".to_string());
    ctx.set_ipv4(Some(tun_ip));

    let (core_instance, _packet_receiver) = build_test_core(ctx.clone()).await;

    let fake_ip = Ipv4Addr::from_str(MAGIC_DNS_FAKE_IP).unwrap();
    let dns_server_inst =
        MagicDnsServerInstance::new(core_instance.packet_plane(), ctx, None, tun_ip, fake_ip)
            .await
            .unwrap();

    let mut ctrl = BaseController::default();
    ctrl.set_tunnel_info(Some(crate::proto::common::TunnelInfo {
        tunnel_type: "tcp".to_string(),
        local_addr: None,
        remote_addr: Some(crate::proto::common::Url {
            url: "tcp://127.0.0.1:54321".to_string(),
        }),
        resolved_remote_addr: None,
    }));

    dns_server_inst
        .data
        .update_dns_record(
            ctrl.clone(),
            UpdateDnsRecordRequest {
                zone: DEFAULT_ET_DNS_ZONE.to_string(),
                routes: vec![Route {
                    hostname: "test1".to_string(),
                    ipv4_addr: Some(Ipv4Inet::from_str("8.8.8.8/32").unwrap().into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    dns_server_inst
        .data
        .update_dns_record(
            ctrl,
            UpdateDnsRecordRequest {
                zone: DEFAULT_ET_DNS_ZONE.to_string(),
                routes: vec![Route {
                    hostname: "test1".to_string(),
                    ipv4_addr: Some(Ipv4Inet::from_str("1.1.1.1/32").unwrap().into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let dns_records = dns_server_inst
        .data
        .get_dns_record(
            BaseController::default(),
            crate::proto::common::Void::default(),
        )
        .await
        .unwrap();
    let zone_records = dns_records.records.get(DEFAULT_ET_DNS_ZONE).unwrap();
    let a_records = zone_records
        .records
        .iter()
        .filter_map(|record| match record.record.as_ref() {
            Some(crate::proto::magic_dns::dns_record::Record::A(a))
                if a.name == "test1.et.net." =>
            {
                Some(a)
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(a_records.len(), 1, "{a_records:?}");
    let resolved_ip = Ipv4Addr::from(a_records[0].value.unwrap_or_default());
    assert_eq!(resolved_ip, Ipv4Addr::new(1, 1, 1, 1));

    let mut ctrl = BaseController::default();
    ctrl.set_tunnel_info(Some(crate::proto::common::TunnelInfo {
        tunnel_type: "tcp".to_string(),
        local_addr: None,
        remote_addr: Some(crate::proto::common::Url {
            url: "tcp://127.0.0.1:54321".to_string(),
        }),
        resolved_remote_addr: None,
    }));

    dns_server_inst
        .data
        .update_dns_record(
            ctrl,
            UpdateDnsRecordRequest {
                zone: DEFAULT_ET_DNS_ZONE.to_string(),
                routes: vec![],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let dns_records = dns_server_inst
        .data
        .get_dns_record(
            BaseController::default(),
            crate::proto::common::Void::default(),
        )
        .await
        .unwrap();
    assert!(!dns_records.records.contains_key(DEFAULT_ET_DNS_ZONE));
}

#[test]
fn normalize_host_zone_trims_and_lowercases() {
    use super::server_instance::normalize_host_zone;
    assert_eq!(normalize_host_zone(" App.Internal. "), "app.internal.");
    assert_eq!(normalize_host_zone("host.et.net"), "host.et.net.");
    assert_eq!(normalize_host_zone("   "), "");
    // Wildcards are not exact zones; normalize returns empty.
    assert_eq!(normalize_host_zone("*.corp.example"), "");
}

#[tokio::test]
async fn test_static_hosts_wildcard_a_record() {
    use easytier_core::config::{DnsConfig, DnsHostEntry};

    let tun_ip = Ipv4Inet::from_str("10.144.144.40/24").unwrap();
    let (global_ctx, core_instance, virtual_nic) = prepare_env("wild-node", tun_ip).await;
    let tun_name = virtual_nic.ifname().await.unwrap();

    global_ctx.config.set_dns_config(Some(DnsConfig {
        hosts: vec![
            DnsHostEntry {
                name: "*.corp.example".to_string(),
                ips: vec!["10.9.9.9".to_string()],
                ttl_secs: Some(300),
            },
            // Exact apex can share the parent zone with the wildcard RR.
            DnsHostEntry {
                name: "corp.example".to_string(),
                ips: vec!["10.9.9.1".to_string()],
                ttl_secs: Some(300),
            },
        ],
        forwarders: vec![],
        upstream_dns: vec![],
    }));

    let fake_ip = Ipv4Addr::from_str(MAGIC_DNS_FAKE_IP).unwrap();
    let _dns_server_inst = MagicDnsServerInstance::new(
        core_instance.packet_plane(),
        global_ctx,
        Some(tun_name),
        tun_ip,
        fake_ip,
    )
    .await
    .unwrap();

    check_dns_record(&fake_ip, "corp.example", "10.9.9.1").await;
    check_dns_record(&fake_ip, "foo.corp.example", "10.9.9.9").await;
    check_dns_record(&fake_ip, "bar.corp.example", "10.9.9.9").await;
}

#[tokio::test]
async fn test_static_hosts_wildcard_skipped_for_route_tld() {
    use easytier_core::config::{DnsConfig, DnsHostEntry};

    let tun_ip = Ipv4Inet::from_str("10.144.144.41/24").unwrap();
    let (global_ctx, core_instance, virtual_nic) = prepare_env("wild-skip", tun_ip).await;
    let tun_name = virtual_nic.ifname().await.unwrap();

    // `*.et.net` would replace the MagicDNS route TLD zone — must be skipped.
    global_ctx.config.set_dns_config(Some(DnsConfig {
        hosts: vec![DnsHostEntry {
            name: "*.et.net".to_string(),
            ips: vec!["10.8.8.8".to_string()],
            ttl_secs: Some(300),
        }],
        forwarders: vec![],
        upstream_dns: vec![],
    }));

    let fake_ip = Ipv4Addr::from_str(MAGIC_DNS_FAKE_IP).unwrap();
    let dns_server_inst = MagicDnsServerInstance::new(
        core_instance.packet_plane(),
        global_ctx,
        Some(tun_name),
        tun_ip,
        fake_ip,
    )
    .await
    .unwrap();

    let mut ctrl = BaseController::default();
    ctrl.set_tunnel_info(Some(crate::proto::common::TunnelInfo {
        tunnel_type: "tcp".to_string(),
        local_addr: None,
        remote_addr: Some(crate::proto::common::Url {
            url: "tcp://127.0.0.1:54322".to_string(),
        }),
        resolved_remote_addr: None,
    }));
    dns_server_inst
        .data
        .update_dns_record(
            ctrl,
            UpdateDnsRecordRequest {
                zone: DEFAULT_ET_DNS_ZONE.to_string(),
                routes: vec![Route {
                    hostname: "node1".to_string(),
                    ipv4_addr: Some(Ipv4Inet::from_str("10.144.144.41/32").unwrap().into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Route hostname still works; wildcard did not own et.net.
    check_dns_record(&fake_ip, "node1.et.net", "10.144.144.41").await;

    // The skipped wildcard must not answer anything: an installed `*.et.net.`
    // would synthesize an A record for any unregistered label under the TLD.
    let stream = UdpClientStream::builder(
        SocketAddr::new(fake_ip.into(), 53),
        TokioRuntimeProvider::default(),
    )
    .build();
    let (mut client, background) = Client::connect(stream).await.unwrap();
    let background_task = tokio::spawn(background);
    let response = client
        .query(
            rr::Name::from_str("random.et.net").unwrap(),
            rr::DNSClass::IN,
            rr::RecordType::A,
        )
        .await
        .unwrap();
    background_task.abort();
    let _ = background_task.await;
    assert!(
        response.answers().is_empty(),
        "skipped wildcard must not synthesize answers: {:?}",
        response.answers()
    );

    // …and it must not be advertised through the status RPC either.
    let advertised = dns_server_inst
        .data
        .get_dns_record(
            BaseController::default(),
            crate::proto::common::Void::default(),
        )
        .await
        .unwrap();
    let a_names: Vec<&str> = advertised
        .records
        .values()
        .flat_map(|list| list.records.iter())
        .filter_map(|record| match record.record.as_ref() {
            Some(dns_record::Record::A(a)) => Some(a.name.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        a_names.iter().all(|name| !name.contains('*')),
        "skipped wildcard must not be advertised: {a_names:?}"
    );
    assert!(
        a_names.contains(&"node1.et.net."),
        "route hostname should still be advertised: {a_names:?}"
    );
}

/// An exact static host must not steal a zone that routes already own:
/// `MagicDnsRecordStore::update()` would revert it on the next route change.
#[tokio::test]
async fn test_static_host_exact_zone_cannot_steal_route_zone() {
    use easytier_core::config::{DnsConfig, DnsHostEntry};

    let tun_ip = Ipv4Inet::from_str("10.144.144.43/24").unwrap();
    let (global_ctx, core_instance, virtual_nic) = prepare_env("route-zone-clash", tun_ip).await;
    let tun_name = virtual_nic.ifname().await.unwrap();

    let hosts = vec![DnsHostEntry {
        name: "custom.local".to_string(),
        ips: vec!["10.1.1.1".to_string()],
        ttl_secs: Some(300),
    }];
    global_ctx.config.set_dns_config(Some(DnsConfig {
        hosts: hosts.clone(),
        forwarders: vec![],
        upstream_dns: vec![],
    }));

    let fake_ip = Ipv4Addr::from_str(MAGIC_DNS_FAKE_IP).unwrap();
    let dns_server_inst = MagicDnsServerInstance::new(
        core_instance.packet_plane(),
        global_ctx,
        Some(tun_name),
        tun_ip,
        fake_ip,
    )
    .await
    .unwrap();

    // A peer routes a zone whose name happens to equal the static host's zone.
    let mut ctrl = BaseController::default();
    ctrl.set_tunnel_info(Some(crate::proto::common::TunnelInfo {
        tunnel_type: "tcp".to_string(),
        local_addr: None,
        remote_addr: Some(crate::proto::common::Url {
            url: "tcp://127.0.0.1:54323".to_string(),
        }),
        resolved_remote_addr: None,
    }));
    dns_server_inst
        .data
        .update_dns_record(
            ctrl,
            UpdateDnsRecordRequest {
                zone: "custom.local.".to_string(),
                routes: vec![Route {
                    hostname: "peer1".to_string(),
                    ipv4_addr: Some(Ipv4Inet::from_str("10.144.144.43/32").unwrap().into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Re-applying the static hosts (any dns_config edit does this) must not take
    // the route-owned zone back — otherwise the two owners would flap.
    dns_server_inst.data.apply_static_hosts(&hosts).await.unwrap();

    check_dns_record(&fake_ip, "peer1.custom.local", "10.144.144.43").await;

    let status = dns_server_inst
        .data
        .get_dns_record(
            BaseController::default(),
            crate::proto::common::Void::default(),
        )
        .await
        .unwrap();
    let a_names: Vec<&str> = status
        .records
        .values()
        .flat_map(|list| list.records.iter())
        .filter_map(|record| match record.record.as_ref() {
            Some(dns_record::Record::A(a)) => Some(a.name.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !a_names.contains(&"custom.local."),
        "static apex must not be installed over a route-owned zone: {a_names:?}"
    );
}

#[tokio::test]
async fn test_static_hosts_win_over_route_hostname() {
    use easytier_core::config::{DnsConfig, DnsHostEntry};

    let tun_ip = Ipv4Inet::from_str("10.144.144.10/24").unwrap();
    let (global_ctx, core_instance, virtual_nic) = prepare_env("test1", tun_ip).await;
    let tun_name = virtual_nic.ifname().await.unwrap();

    global_ctx.config.set_dns_config(Some(DnsConfig {
        hosts: vec![DnsHostEntry {
            name: "test1.et.net".to_string(),
            ips: vec!["9.9.9.9".to_string()],
            ttl_secs: Some(300),
        }],
        forwarders: vec![],
        upstream_dns: vec![],
    }));

    let fake_ip = Ipv4Addr::from_str(MAGIC_DNS_FAKE_IP).unwrap();
    let dns_server_inst = MagicDnsServerInstance::new(
        core_instance.packet_plane(),
        global_ctx,
        Some(tun_name),
        tun_ip,
        fake_ip,
    )
    .await
    .unwrap();

    // Route publishes the same name with a different IP; hosts zone must win.
    let mut ctrl = BaseController::default();
    ctrl.set_tunnel_info(Some(crate::proto::common::TunnelInfo {
        tunnel_type: "tcp".to_string(),
        local_addr: None,
        remote_addr: Some(crate::proto::common::Url {
            url: "tcp://127.0.0.1:54321".to_string(),
        }),
        resolved_remote_addr: None,
    }));
    dns_server_inst
        .data
        .update_dns_record(
            ctrl,
            UpdateDnsRecordRequest {
                zone: DEFAULT_ET_DNS_ZONE.to_string(),
                routes: vec![Route {
                    hostname: "test1".to_string(),
                    ipv4_addr: Some(Ipv4Inet::from_str("10.144.144.10/32").unwrap().into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    check_dns_record(&fake_ip, "test1.et.net", "9.9.9.9").await;
}

#[tokio::test]
async fn test_static_hosts_channel_via_update_dns_record() {
    use crate::proto::magic_dns::StaticDnsHost;

    let tun_ip = Ipv4Inet::from_str("10.144.144.20/24").unwrap();
    let (global_ctx, core_instance, virtual_nic) = prepare_env("channel-node", tun_ip).await;
    let tun_name = virtual_nic.ifname().await.unwrap();

    let fake_ip = Ipv4Addr::from_str(MAGIC_DNS_FAKE_IP).unwrap();
    let dns_server_inst = MagicDnsServerInstance::new(
        core_instance.packet_plane(),
        global_ctx,
        Some(tun_name),
        tun_ip,
        fake_ip,
    )
    .await
    .unwrap();

    let mut ctrl = BaseController::default();
    ctrl.set_tunnel_info(Some(crate::proto::common::TunnelInfo {
        tunnel_type: "tcp".to_string(),
        local_addr: None,
        remote_addr: Some(crate::proto::common::Url {
            url: "tcp://127.0.0.1:55555".to_string(),
        }),
        resolved_remote_addr: None,
    }));

    // R1 channel: client="static-hosts" installs long-TTL zone (B2 over routes).
    dns_server_inst
        .data
        .update_dns_record(
            ctrl.clone(),
            UpdateDnsRecordRequest {
                zone: DEFAULT_ET_DNS_ZONE.to_string(),
                routes: vec![Route {
                    hostname: "app".to_string(),
                    ipv4_addr: Some(Ipv4Inet::from_str("10.144.144.20/32").unwrap().into()),
                    ..Default::default()
                }],
                client: Some(
                    crate::instance::dns_server::MAGIC_DNS_STATIC_HOSTS_CLIENT.to_string(),
                ),
                static_hosts: vec![StaticDnsHost {
                    name: "app.et.net".to_string(),
                    ips: vec!["8.8.4.4".to_string()],
                    ttl_secs: Some(600),
                }],
            },
        )
        .await
        .unwrap();

    check_dns_record(&fake_ip, "app.et.net", "8.8.4.4").await;

    // Clearing static hosts falls back to route IP.
    dns_server_inst
        .data
        .update_dns_record(
            ctrl,
            UpdateDnsRecordRequest {
                zone: DEFAULT_ET_DNS_ZONE.to_string(),
                routes: vec![Route {
                    hostname: "app".to_string(),
                    ipv4_addr: Some(Ipv4Inet::from_str("10.144.144.20/32").unwrap().into()),
                    ..Default::default()
                }],
                client: Some(
                    crate::instance::dns_server::MAGIC_DNS_STATIC_HOSTS_CLIENT.to_string(),
                ),
                static_hosts: vec![],
            },
        )
        .await
        .unwrap();

    check_dns_record(&fake_ip, "app.et.net", "10.144.144.20").await;
}

#[tokio::test]
async fn test_static_hosts_cleared_on_client_disconnect() {
    use crate::proto::magic_dns::StaticDnsHost;
    use crate::proto::rpc::standalone::RpcServerHook;
    use easytier_core::config::{DnsConfig, DnsHostEntry};

    let tun_ip = Ipv4Inet::from_str("10.144.144.30/24").unwrap();
    let (global_ctx, core_instance, virtual_nic) = prepare_env("disconnect-node", tun_ip).await;
    let tun_name = virtual_nic.ifname().await.unwrap();

    let fake_ip = Ipv4Addr::from_str(MAGIC_DNS_FAKE_IP).unwrap();
    let dns_server_inst = MagicDnsServerInstance::new(
        core_instance.packet_plane(),
        global_ctx,
        Some(tun_name),
        tun_ip,
        fake_ip,
    )
    .await
    .unwrap();

    let tunnel = crate::proto::common::TunnelInfo {
        tunnel_type: "tcp".to_string(),
        local_addr: None,
        remote_addr: Some(crate::proto::common::Url {
            url: "tcp://127.0.0.1:55666".to_string(),
        }),
        resolved_remote_addr: None,
    };
    let mut ctrl = BaseController::default();
    ctrl.set_tunnel_info(Some(tunnel.clone()));

    dns_server_inst
        .data
        .update_dns_record(
            ctrl,
            UpdateDnsRecordRequest {
                zone: DEFAULT_ET_DNS_ZONE.to_string(),
                routes: vec![Route {
                    hostname: "gone".to_string(),
                    ipv4_addr: Some(Ipv4Inet::from_str("10.144.144.30/32").unwrap().into()),
                    ..Default::default()
                }],
                client: Some(
                    crate::instance::dns_server::MAGIC_DNS_STATIC_HOSTS_CLIENT.to_string(),
                ),
                static_hosts: vec![StaticDnsHost {
                    name: "gone.et.net".to_string(),
                    ips: vec!["7.7.7.7".to_string()],
                    ttl_secs: Some(300),
                }],
            },
        )
        .await
        .unwrap();
    check_dns_record(&fake_ip, "gone.et.net", "7.7.7.7").await;

    dns_server_inst
        .data
        .on_client_disconnected(Some(tunnel))
        .await;

    // After disconnect, static channel entry is gone; route entry also removed →
    // fall back to empty authoritative zone / NX. Query route hostname after
    // re-publishing route only.
    let mut ctrl2 = BaseController::default();
    ctrl2.set_tunnel_info(Some(crate::proto::common::TunnelInfo {
        tunnel_type: "tcp".to_string(),
        local_addr: None,
        remote_addr: Some(crate::proto::common::Url {
            url: "tcp://127.0.0.1:55667".to_string(),
        }),
        resolved_remote_addr: None,
    }));
    dns_server_inst
        .data
        .update_dns_record(
            ctrl2,
            UpdateDnsRecordRequest {
                zone: DEFAULT_ET_DNS_ZONE.to_string(),
                routes: vec![Route {
                    hostname: "gone".to_string(),
                    ipv4_addr: Some(Ipv4Inet::from_str("10.144.144.30/32").unwrap().into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    check_dns_record(&fake_ip, "gone.et.net", "10.144.144.30").await;

    // Also cover reload_dns_policy (local hosts hot path).
    dns_server_inst
        .data
        .reload_dns_policy(Some(&DnsConfig {
            hosts: vec![DnsHostEntry {
                name: "gone.et.net".to_string(),
                ips: vec!["5.5.5.5".to_string()],
                ttl_secs: Some(300),
            }],
            forwarders: vec![],
            upstream_dns: vec![],
        }))
        .await
        .unwrap();
    check_dns_record(&fake_ip, "gone.et.net", "5.5.5.5").await;
}
