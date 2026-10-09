use anyhow::{Context, Result};
use hickory_proto::rr;
use hickory_proto::rr::LowerName;
use hickory_proto::xfer::Protocol;
use hickory_resolver::config::{
    NameServerConfig, NameServerConfigGroup, ResolverOpts, ServerOrderingStrategy,
};
use hickory_resolver::system_conf::read_system_conf;
use hickory_server::ServerFuture;
use hickory_server::authority::{AuthorityObject, Catalog, ZoneType};
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::store::forwarder::ForwardConfig;
use hickory_server::store::{forwarder::ForwardAuthority, in_memory::InMemoryAuthority};
use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{RwLock, RwLockReadGuard};

use crate::common::dns::{get_default_resolver_config, magic_dns_forward_connector};

use super::config::{
    GeneralConfig, GeneralConfigBuilder, Record, RunConfig, RunConfigBuilder, SplitForwarderConfig,
};

pub struct Server {
    server: ServerFuture<CatalogRequestHandler>,
    catalog: Arc<RwLock<Catalog>>,
    general_config: GeneralConfig,
    udp_local_addr: Option<SocketAddr>,
    excluded_forward_nameservers: Vec<IpAddr>,
    applied_split_zones: Mutex<BTreeSet<String>>,
}

struct CatalogRequestHandler {
    catalog: Arc<RwLock<Catalog>>,
}

impl CatalogRequestHandler {
    fn new(catalog: Arc<RwLock<Catalog>>) -> CatalogRequestHandler {
        Self { catalog }
    }
}

#[async_trait::async_trait]
impl RequestHandler for CatalogRequestHandler {
    async fn handle_request<R: ResponseHandler>(
        &self,
        request: &Request,
        response_handle: R,
    ) -> ResponseInfo {
        self.catalog
            .read()
            .await
            .handle_request(request, response_handle)
            .await
    }
}

pub fn build_authority(domain: &str, records: &[Record]) -> Result<InMemoryAuthority> {
    let zone = rr::Name::from_str(domain)?;
    let mut authority = InMemoryAuthority::empty(zone, ZoneType::Primary, false);
    for record in records.iter() {
        let r = record.try_into()?;
        authority.upsert_mut(r, 0);
    }
    Ok(authority)
}

/// Parse one upstream entry (`1.1.1.1`, `1.1.1.1:53`, `udp://1.1.1.1:53`, `tcp://…`).
pub fn parse_upstream_nameserver(raw: &str) -> Result<Vec<NameServerConfig>> {
    let raw = raw.trim();
    if raw.is_empty() {
        anyhow::bail!("empty upstream DNS entry");
    }

    let (proto_hint, hostport) = if let Some(rest) = raw.strip_prefix("udp://") {
        (Some(Protocol::Udp), rest)
    } else if let Some(rest) = raw.strip_prefix("tcp://") {
        (Some(Protocol::Tcp), rest)
    } else if raw.contains("://") {
        anyhow::bail!(
            "unsupported upstream DNS scheme in `{raw}` (first phase: udp:// or tcp:// only)"
        );
    } else {
        (None, raw)
    };

    // Bare IPv6 (e.g. `fd00::1`) contains `:`; only treat `host:port` when the
    // host side has no colon (IPv4 / hostname), or when brackets are used.
    let (host, port) = if let Some(h) = hostport.strip_prefix('[') {
        if let Some((addr, rest)) = h.split_once("]:") {
            (
                addr,
                rest.parse::<u16>()
                    .with_context(|| format!("invalid upstream DNS port in `{raw}`"))?,
            )
        } else {
            (h.trim_end_matches(']'), 53u16)
        }
    } else if let Some((h, p)) = hostport.rsplit_once(':')
        && !h.is_empty()
        && !h.contains(':')
        && p.parse::<u16>().is_ok()
    {
        (
            h,
            p.parse::<u16>()
                .with_context(|| format!("invalid upstream DNS port in `{raw}`"))?,
        )
    } else {
        (hostport, 53)
    };

    let ip: IpAddr = host
        .parse()
        .with_context(|| format!("invalid upstream DNS address `{raw}`"))?;
    let addr = SocketAddr::new(ip, port);

    let protocols = match proto_hint {
        Some(Protocol::Udp) => vec![Protocol::Udp],
        Some(Protocol::Tcp) => vec![Protocol::Tcp],
        Some(_) => vec![Protocol::Udp],
        None => vec![Protocol::Udp, Protocol::Tcp],
    };

    Ok(protocols
        .into_iter()
        .map(|protocol| {
            let mut cfg = NameServerConfig::new(addr, protocol);
            // NXDOMAIN from an authoritative upstream must stick (dns-policy §6).
            cfg.trust_negative_responses = true;
            cfg
        })
        .collect())
}

fn ordered_failover_opts() -> ResolverOpts {
    let mut opts = ResolverOpts::default();
    opts.server_ordering_strategy = ServerOrderingStrategy::UserProvidedOrder;
    opts.num_concurrent_reqs = 1;
    opts.timeout = Duration::from_secs(2);
    opts.attempts = 2;
    opts
}

fn build_forward_config_from_servers(
    servers: &[String],
    excluded: &[IpAddr],
) -> Result<ForwardConfig> {
    let mut group = NameServerConfigGroup::new();
    for entry in servers {
        for ns in parse_upstream_nameserver(entry)? {
            if excluded.contains(&ns.socket_addr.ip()) {
                tracing::warn!(
                    upstream = %ns.socket_addr,
                    "skipping upstream DNS that matches excluded/fake MagicDNS address"
                );
                continue;
            }
            group.push(ns);
        }
    }
    if group.is_empty() {
        anyhow::bail!("all configured upstream DNS entries were empty or excluded");
    }
    Ok(ForwardConfig {
        name_servers: group,
        options: Some(ordered_failover_opts()),
    })
}

fn build_forward_config(config: &RunConfig) -> Result<ForwardConfig> {
    let excluded = config.excluded_forward_nameservers();

    if !config.forward_nameservers().is_empty() {
        return build_forward_config_from_servers(config.forward_nameservers(), excluded);
    }

    // B1: empty upstream_dns → today's system DNS behavior (incl. system ResolverOpts).
    let system_conf =
        read_system_conf().unwrap_or((get_default_resolver_config(), ResolverOpts::default()));
    let servers: Vec<_> = system_conf
        .0
        .name_servers()
        .iter()
        .filter(|&x| !excluded.contains(&x.socket_addr.ip()))
        .cloned()
        .collect();
    if servers.is_empty() {
        anyhow::bail!("no usable system upstream DNS after exclusions (or system DNS unavailable)");
    }
    Ok(ForwardConfig {
        name_servers: servers.into(),
        options: Some(system_conf.1),
    })
}

fn normalize_forward_zone(domain: &str) -> Result<String> {
    let trimmed = domain.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        anyhow::bail!("empty split forwarder domain");
    }
    Ok(format!("{}.", trimmed.to_ascii_lowercase()))
}

impl Server {
    pub fn new(config: RunConfig) -> Result<Self> {
        Self::try_new(config)
    }

    fn try_new(config: RunConfig) -> Result<Self> {
        let mut catalog = Catalog::new();
        for (domain, records) in config.zones().iter() {
            let zone = rr::Name::from_str(domain.as_str())?;
            let authroty = build_authority(domain, records)?;
            catalog.upsert(zone.clone().into(), vec![Arc::new(authroty)]);
        }

        // R2: split forwarders (domain suffix → dedicated upstreams). Catalog
        // longest-match makes these win over the root forwarder but lose to
        // more-specific hosts / MagicDNS route zones when names overlap.
        let excluded = config.excluded_forward_nameservers().clone();
        let mut applied_split_zones = BTreeSet::new();
        for split in config.split_forwarders() {
            if split.domains.is_empty() || split.servers.is_empty() {
                continue;
            }
            let forward_config = build_forward_config_from_servers(&split.servers, &excluded)?;
            for domain in &split.domains {
                let zone = normalize_forward_zone(domain)?;
                let auth = ForwardAuthority::builder_with_config(
                    forward_config.clone(),
                    magic_dns_forward_connector(),
                )
                .build()
                .map_err(|e| anyhow::anyhow!("split ForwardAuthority build failed: {e}"))?;
                catalog.upsert(
                    LowerName::from_str(&zone)
                        .with_context(|| format!("invalid split forwarder zone: {zone}"))?,
                    vec![Arc::new(auth)],
                );
                tracing::info!(
                    zone = %zone,
                    servers = ?split.servers,
                    "MagicDNS split forwarder zone installed"
                );
                applied_split_zones.insert(zone);
            }
        }

        let forward_config = build_forward_config(&config)?;
        let auth =
            ForwardAuthority::builder_with_config(forward_config, magic_dns_forward_connector())
                .build()
                .map_err(|e| anyhow::anyhow!("root ForwardAuthority build failed: {e}"))?;

        catalog.upsert(rr::Name::from_str(".")?.into(), vec![Arc::new(auth)]);

        let catalog = Arc::new(RwLock::new(catalog));
        let handler = CatalogRequestHandler::new(catalog.clone());
        let server = ServerFuture::new(handler);

        Ok(Self {
            server,
            catalog,
            general_config: config.general().clone(),
            udp_local_addr: None,
            excluded_forward_nameservers: excluded,
            applied_split_zones: Mutex::new(applied_split_zones),
        })
    }

    #[cfg(test)]
    pub fn udp_local_addr(&self) -> Option<SocketAddr> {
        self.udp_local_addr
    }

    pub async fn register_udp_socket(&mut self, address: String) -> Result<SocketAddr> {
        let bind_addr = SocketAddr::from_str(&address)
            .with_context(|| format!("DNS Server failed to parse address {}", address))?;
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .with_context(|| {
            format!(
                "DNS Server failed to create UDP socket for address {}",
                address
            )
        })?;
        socket2::SockRef::from(&socket)
            .set_reuse_address(true)
            .with_context(|| {
                format!(
                    "DNS Server failed to set reuse address on socket {}",
                    address
                )
            })?;
        socket.bind(&bind_addr.into()).with_context(|| {
            format!("DNS Server failed to bind socket to address {}", bind_addr)
        })?;
        socket
            .set_nonblocking(true)
            .with_context(|| "DNS Server failed to set socket to non-blocking".to_string())?;
        let socket = UdpSocket::from_std(socket.into()).with_context(|| {
            format!(
                "DNS Server failed to convert socket to UdpSocket for address {}",
                address
            )
        })?;

        let local_addr = socket
            .local_addr()
            .with_context(|| "DNS Server failed to get local address".to_string())?;
        self.server.register_socket(socket);

        Ok(local_addr)
    }

    pub async fn run(&mut self) -> Result<()> {
        if let Some(address) = self.general_config.listen_tcp() {
            let tcp_listener = TcpListener::bind(address.clone())
                .await
                .with_context(|| format!("DNS Server failed to bind TCP address {}", address))?;
            self.server
                .register_listener(tcp_listener, Duration::from_secs(5));
        }

        if let Some(address) = self.general_config.listen_udp() {
            let local_addr = self.register_udp_socket(address.clone()).await?;
            self.udp_local_addr = Some(local_addr);
        };

        Ok(())
    }

    #[cfg(test)]
    pub async fn shutdown(&mut self) -> Result<()> {
        self.server.shutdown_gracefully().await?;
        Ok(())
    }

    pub async fn upsert(&self, name: LowerName, authority: Arc<dyn AuthorityObject>) {
        self.catalog.write().await.upsert(name, vec![authority]);
    }

    pub async fn remove(&self, name: &LowerName) {
        self.catalog.write().await.remove(name);
    }

    pub fn split_zones(&self) -> BTreeSet<String> {
        self.applied_split_zones.lock().unwrap().clone()
    }

    /// Hot-reload split forwarder zones (R2) without recreating the UDP listener.
    ///
    /// Zones in `retain_zones` (e.g. route / static hosts) are not removed when
    /// they disappear from the split config.
    pub async fn reload_split_forwarders(
        &self,
        splits: &[SplitForwarderConfig],
        retain_zones: &BTreeSet<String>,
    ) -> Result<()> {
        let excluded = &self.excluded_forward_nameservers;
        let mut next_zones = BTreeSet::new();
        let mut pending: Vec<(String, ForwardConfig)> = Vec::new();

        for split in splits {
            if split.domains.is_empty() || split.servers.is_empty() {
                continue;
            }
            let forward_config = build_forward_config_from_servers(&split.servers, excluded)?;
            for domain in &split.domains {
                let zone = normalize_forward_zone(domain)?;
                next_zones.insert(zone.clone());
                pending.push((zone, forward_config.clone()));
            }
        }

        let stale: Vec<String> = {
            let applied = self.applied_split_zones.lock().unwrap();
            applied.difference(&next_zones).cloned().collect()
        };
        for zone in &stale {
            if retain_zones.contains(zone) {
                tracing::debug!(
                    zone = %zone,
                    "keeping catalog zone owned by routes/static hosts while reloading splits"
                );
                continue;
            }
            if let Ok(name) = LowerName::from_str(zone) {
                self.remove(&name).await;
            }
        }

        for (zone, forward_config) in pending {
            let auth = ForwardAuthority::builder_with_config(
                forward_config,
                magic_dns_forward_connector(),
            )
            .build()
            .map_err(|e| anyhow::anyhow!("split ForwardAuthority rebuild failed: {e}"))?;
            self.upsert(
                LowerName::from_str(&zone)
                    .with_context(|| format!("invalid split forwarder zone: {zone}"))?,
                Arc::new(auth),
            )
            .await;
            tracing::info!(zone = %zone, "MagicDNS split forwarder zone reloaded");
        }

        *self.applied_split_zones.lock().unwrap() = next_zones;
        Ok(())
    }

    /// Hot-reload root `.` forwarder (R3 / B1).
    pub async fn reload_root_forwarder(&self, upstream_dns: &[String]) -> Result<()> {
        let cfg = RunConfigBuilder::default()
            .general(GeneralConfigBuilder::default().build()?)
            .forward_nameservers(upstream_dns.to_vec())
            .excluded_forward_nameservers(self.excluded_forward_nameservers.clone())
            .build()?;
        let forward_config = build_forward_config(&cfg)?;
        let auth =
            ForwardAuthority::builder_with_config(forward_config, magic_dns_forward_connector())
                .build()
                .map_err(|e| anyhow::anyhow!("root ForwardAuthority rebuild failed: {e}"))?;
        self.upsert(rr::Name::from_str(".")?.into(), Arc::new(auth))
            .await;
        tracing::info!(
            upstreams = ?upstream_dns,
            "MagicDNS root forwarder reloaded"
        );
        Ok(())
    }

    pub async fn read_catalog(&self) -> RwLockReadGuard<'_, Catalog> {
        self.catalog.read().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::dns_server::config::{
        GeneralConfigBuilder, RecordBuilder, RecordType, RunConfigBuilder, SplitForwarderConfig,
    };
    use anyhow::Result;
    use hickory_client::client::{Client, ClientHandle};
    use hickory_proto::rr;
    use hickory_proto::runtime::TokioRuntimeProvider;
    use hickory_proto::udp::UdpClientStream;
    use hickory_proto::xfer::Protocol;
    use hickory_resolver::config::ServerOrderingStrategy;
    use maplit::hashmap;
    use std::time::Duration;

    #[tokio::test]
    async fn it_works() -> Result<()> {
        let mut server = Server::new(
            RunConfigBuilder::default()
                .general(GeneralConfigBuilder::default().build()?)
                .build()?,
        )?;
        server.run().await?;
        server.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn can_resolve_records() -> Result<()> {
        let configured_record = RecordBuilder::default()
            .rr_type(RecordType::A)
            .name("www.et.internal.".to_string())
            .value("123.123.123.123".to_string())
            .ttl(Duration::from_secs(60))
            .build()?;
        let configured_record2 = RecordBuilder::default()
            .rr_type(RecordType::A)
            .name("中文.et.internal.".to_string())
            .value("123.123.123.123".to_string())
            .ttl(Duration::from_secs(60))
            .build()?;
        let soa_record = RecordBuilder::default()
            .rr_type(RecordType::SOA)
            .name("et.internal.".to_string())
            .value(
                "ns.et.internal. hostmaster.et.internal. 2023101001 7200 3600 1209600 86400"
                    .to_string(),
            )
            .ttl(Duration::from_secs(60))
            .build()?;
        let config = RunConfigBuilder::default()
            .general(
                GeneralConfigBuilder::default()
                    .listen_udp("127.0.0.1:0")
                    .build()?,
            )
            .zones(hashmap! {
                "et.internal.".to_string() => vec![configured_record.clone(), soa_record.clone(), configured_record2.clone()],
            })
            .build()?;

        let mut server = Server::new(config)?;
        server.run().await?;

        let local_addr = server.udp_local_addr().unwrap();
        let stream = UdpClientStream::builder(local_addr, TokioRuntimeProvider::default()).build();
        let (mut client, background) = Client::connect(stream).await?;
        let background_task = tokio::spawn(background);
        let response = client
            .query(
                rr::Name::from_str("www.et.internal")?,
                rr::DNSClass::IN,
                rr::RecordType::A,
            )
            .await?;
        drop(background_task);

        println!("Response: {:?}", response);

        assert_eq!(response.answers().len(), 1);
        let expected_record: rr::Record = configured_record.try_into()?;
        assert_eq!(response.answers().first().unwrap(), &expected_record);

        server.shutdown().await?;
        Ok(())
    }

    /// Static hosts wildcards install as parent-zone `*.suffix.` A records.
    #[tokio::test]
    async fn can_resolve_wildcard_a_record() -> Result<()> {
        let wildcard = RecordBuilder::default()
            .rr_type(RecordType::A)
            .name("*.corp.example.".to_string())
            .value("10.9.9.9".to_string())
            .ttl(Duration::from_secs(60))
            .build()?;
        let apex = RecordBuilder::default()
            .rr_type(RecordType::A)
            .name("corp.example.".to_string())
            .value("10.9.9.1".to_string())
            .ttl(Duration::from_secs(60))
            .build()?;
        let soa = RecordBuilder::default()
            .rr_type(RecordType::SOA)
            .name("corp.example.".to_string())
            .value(
                "ns.corp.example. hostmaster.corp.example. 2023101001 7200 3600 1209600 86400"
                    .to_string(),
            )
            .ttl(Duration::from_secs(60))
            .build()?;
        let config = RunConfigBuilder::default()
            .general(
                GeneralConfigBuilder::default()
                    .listen_udp("127.0.0.1:0")
                    .build()?,
            )
            .zones(hashmap! {
                "corp.example.".to_string() => vec![wildcard, apex.clone(), soa],
            })
            .build()?;

        let mut server = Server::new(config)?;
        server.run().await?;

        let local_addr = server.udp_local_addr().unwrap();
        let stream = UdpClientStream::builder(local_addr, TokioRuntimeProvider::default()).build();
        let (mut client, background) = Client::connect(stream).await?;
        let background_task = tokio::spawn(background);

        let apex_resp = client
            .query(
                rr::Name::from_str("corp.example")?,
                rr::DNSClass::IN,
                rr::RecordType::A,
            )
            .await?;
        assert_eq!(apex_resp.answers().len(), 1);
        assert_eq!(
            apex_resp
                .answers()
                .first()
                .unwrap()
                .clone()
                .into_parts()
                .rdata
                .into_a()
                .unwrap()
                .0,
            "10.9.9.1".parse::<std::net::Ipv4Addr>()?
        );

        let wild_resp = client
            .query(
                rr::Name::from_str("foo.corp.example")?,
                rr::DNSClass::IN,
                rr::RecordType::A,
            )
            .await?;
        assert_eq!(wild_resp.answers().len(), 1);
        assert_eq!(
            wild_resp
                .answers()
                .first()
                .unwrap()
                .clone()
                .into_parts()
                .rdata
                .into_a()
                .unwrap()
                .0,
            "10.9.9.9".parse::<std::net::Ipv4Addr>()?
        );

        // RFC 4592 / hickory inner_lookup_wildcard: multi-label under the parent
        // also synthesizes from `*.corp.example.` (peels labels until match).
        let deep_resp = client
            .query(
                rr::Name::from_str("a.b.corp.example")?,
                rr::DNSClass::IN,
                rr::RecordType::A,
            )
            .await?;
        assert_eq!(
            deep_resp.answers().len(),
            1,
            "multi-label query should match *.corp.example: {deep_resp:?}"
        );
        assert_eq!(
            deep_resp
                .answers()
                .first()
                .unwrap()
                .clone()
                .into_parts()
                .rdata
                .into_a()
                .unwrap()
                .0,
            "10.9.9.9".parse::<std::net::Ipv4Addr>()?
        );

        background_task.abort();
        let _ = background_task.await;
        server.shutdown().await?;
        Ok(())
    }

    #[test]
    fn parse_upstream_accepts_ip_port_and_url_forms() -> Result<()> {
        let plain = parse_upstream_nameserver("1.1.1.1")?;
        assert_eq!(plain.len(), 2);
        assert_eq!(plain[0].socket_addr, "1.1.1.1:53".parse()?);
        assert_eq!(plain[0].protocol, Protocol::Udp);
        assert_eq!(plain[1].protocol, Protocol::Tcp);

        let with_port = parse_upstream_nameserver("8.8.8.8:5353")?;
        assert_eq!(with_port[0].socket_addr, "8.8.8.8:5353".parse()?);

        let udp_only = parse_upstream_nameserver("udp://9.9.9.9:53")?;
        assert_eq!(udp_only.len(), 1);
        assert_eq!(udp_only[0].protocol, Protocol::Udp);

        let tcp_only = parse_upstream_nameserver("tcp://9.9.9.9")?;
        assert_eq!(tcp_only.len(), 1);
        assert_eq!(tcp_only[0].protocol, Protocol::Tcp);
        assert_eq!(tcp_only[0].socket_addr.port(), 53);

        assert!(parse_upstream_nameserver("https://1.1.1.1/dns-query").is_err());

        // Bare IPv6 must not be split on the last `:`.
        let v6 = parse_upstream_nameserver("fd00::1")?;
        assert_eq!(v6.len(), 2);
        assert_eq!(v6[0].socket_addr, "[fd00::1]:53".parse()?);
        let v6_bracket = parse_upstream_nameserver("[fd00::1]:5353")?;
        assert_eq!(v6_bracket[0].socket_addr.port(), 5353);
        assert_eq!(
            v6_bracket[0].socket_addr.ip(),
            "fd00::1".parse::<std::net::IpAddr>()?
        );
        Ok(())
    }

    #[test]
    fn build_forward_config_uses_user_order_for_custom_upstreams() -> Result<()> {
        let cfg = RunConfigBuilder::default()
            .general(GeneralConfigBuilder::default().build()?)
            .forward_nameservers(vec!["1.1.1.1".to_string(), "8.8.8.8".to_string()])
            .excluded_forward_nameservers(vec!["10.255.255.254".parse()?])
            .build()?;
        let forward = build_forward_config(&cfg)?;
        let opts = forward.options.expect("custom upstream must set opts");
        assert_eq!(
            opts.server_ordering_strategy,
            ServerOrderingStrategy::UserProvidedOrder
        );
        assert_eq!(opts.num_concurrent_reqs, 1);
        assert_eq!(opts.timeout, Duration::from_secs(2));
        assert_eq!(opts.attempts, 2);
        let expected_ip: std::net::IpAddr = "1.1.1.1".parse()?;
        assert!(
            forward
                .name_servers
                .iter()
                .any(|ns| ns.socket_addr.ip() == expected_ip)
        );
        Ok(())
    }

    #[test]
    fn normalize_forward_zone_adds_trailing_dot_and_lowercases() -> Result<()> {
        assert_eq!(normalize_forward_zone("Corp.Example")?, "corp.example.");
        assert_eq!(normalize_forward_zone("corp.example.")?, "corp.example.");
        assert!(normalize_forward_zone("  ").is_err());
        Ok(())
    }

    #[test]
    fn split_forwarder_config_builds_per_suffix_zones() -> Result<()> {
        let cfg = RunConfigBuilder::default()
            .general(GeneralConfigBuilder::default().build()?)
            .split_forwarders(vec![SplitForwarderConfig {
                domains: vec!["corp.example".to_string(), "INTRA.".to_string()],
                servers: vec!["10.0.0.53".to_string()],
            }])
            .excluded_forward_nameservers(vec!["10.255.255.254".parse()?])
            .build()?;
        // Must not panic; installs suffix forward zones + root forwarder.
        let _server = Server::new(cfg)?;
        Ok(())
    }
}
