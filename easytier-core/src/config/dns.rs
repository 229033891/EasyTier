//! DNS policy types for TOML/CLI config (no `proto::api` dependency; WASM-safe).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DnsHostEntry {
    pub name: String,
    pub ips: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DnsForwarder {
    pub domains: Vec<String>,
    pub servers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DnsConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<DnsHostEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwarders: Vec<DnsForwarder>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream_dns: Vec<String>,
}

// browser-config (WASM config-generator) and management-rpc both enable
// easytier-proto/api without sharing a single local feature flag.
#[cfg(any(feature = "management-rpc", feature = "browser-config"))]
mod proto_convert {
    use super::*;
    use easytier_proto::api::manage;

    impl From<manage::DnsHostEntry> for DnsHostEntry {
        fn from(value: manage::DnsHostEntry) -> Self {
            Self {
                name: value.name,
                ips: value.ips,
                ttl_secs: value.ttl_secs,
            }
        }
    }

    impl From<DnsHostEntry> for manage::DnsHostEntry {
        fn from(value: DnsHostEntry) -> Self {
            Self {
                name: value.name,
                ips: value.ips,
                ttl_secs: value.ttl_secs,
            }
        }
    }

    impl From<manage::DnsForwarder> for DnsForwarder {
        fn from(value: manage::DnsForwarder) -> Self {
            Self {
                domains: value.domains,
                servers: value.servers,
            }
        }
    }

    impl From<DnsForwarder> for manage::DnsForwarder {
        fn from(value: DnsForwarder) -> Self {
            Self {
                domains: value.domains,
                servers: value.servers,
            }
        }
    }

    impl From<manage::DnsConfig> for DnsConfig {
        fn from(value: manage::DnsConfig) -> Self {
            Self {
                hosts: value.hosts.into_iter().map(Into::into).collect(),
                forwarders: value.forwarders.into_iter().map(Into::into).collect(),
                upstream_dns: value.upstream_dns,
            }
        }
    }

    impl From<DnsConfig> for manage::DnsConfig {
        fn from(value: DnsConfig) -> Self {
            Self {
                hosts: value.hosts.into_iter().map(Into::into).collect(),
                forwarders: value.forwarders.into_iter().map(Into::into).collect(),
                upstream_dns: value.upstream_dns,
            }
        }
    }
}
