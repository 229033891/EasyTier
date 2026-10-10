//! DNS policy types for TOML/CLI config (no `proto::api` dependency; WASM-safe).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DnsHostEntry {
    pub name: String,
    pub ips: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<u32>,
}

/// Catalog zone + RR owner name for one static hosts entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostZoneTarget {
    pub zone: String,
    pub rr_name: String,
    pub is_wildcard: bool,
}

/// Minimum label count for a static-hosts Catalog zone.
///
/// A static host owns an authority for its zone. A single-label zone (`com.`)
/// would answer for — or, as a wildcard, hijack — every name under that TLD.
/// RFC 4592 leaves this to policy, so we require at least two labels.
pub const MIN_STATIC_HOST_ZONE_LABELS: usize = 2;

/// Shared reason string so the CLI parser, the runtime apply path and the status
/// RPC all report the same thing.
pub const SINGLE_LABEL_ZONE_REASON: &str =
    "single-label zone: static hosts must own at least two labels (e.g. corp.example)";

impl HostZoneTarget {
    /// Number of labels in `zone` (`corp.example.` → 2).
    pub fn zone_label_count(&self) -> usize {
        self.zone
            .trim_end_matches('.')
            .split('.')
            .filter(|label| !label.is_empty())
            .count()
    }

    /// Policy verdict shared by every entry point: `Some(reason)` when this
    /// target must not be installed. `None` means "syntactically fine and deep
    /// enough"; conflict checks against route/split zones live in the caller.
    pub fn reject_reason(&self) -> Option<&'static str> {
        if self.zone.is_empty() || self.zone == "." {
            return Some("empty zone");
        }
        if self.zone_label_count() < MIN_STATIC_HOST_ZONE_LABELS {
            return Some(SINGLE_LABEL_ZONE_REASON);
        }
        None
    }
}

/// Classify a hosts name into Catalog zone + RR name.
///
/// - Exact: `app.internal` → zone/rr `app.internal.`
/// - Wildcard syntax (leftmost `*` only): `*.corp.example` → zone `corp.example.`,
///   rr `*.corp.example.` (Catalog matching is RFC 4592 multi-label via hickory).
pub fn classify_host_name(name: &str) -> Result<HostZoneTarget, String> {
    let trimmed = name.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        return Err("empty name".to_string());
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(parent) = lower.strip_prefix("*.") {
        if parent.is_empty() {
            return Err("wildcard requires a parent domain (e.g. *.corp.example)".to_string());
        }
        if parent.contains('*') {
            return Err(
                "only single-label wildcards are supported (e.g. *.corp.example)".to_string(),
            );
        }
        if parent.starts_with('.') || parent.ends_with('.') || parent.contains("..") {
            return Err("invalid wildcard parent domain".to_string());
        }
        let zone = format!("{parent}.");
        let rr_name = format!("*.{parent}.");
        return Ok(HostZoneTarget {
            zone,
            rr_name,
            is_wildcard: true,
        });
    }
    if lower.contains('*') {
        return Err("invalid '*': use *.suffix for wildcards, or an exact hostname".to_string());
    }
    let zone = format!("{lower}.");
    Ok(HostZoneTarget {
        zone: zone.clone(),
        rr_name: zone,
        is_wildcard: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_exact_and_wildcard() {
        let exact = classify_host_name(" App.Internal. ").unwrap();
        assert_eq!(exact.zone, "app.internal.");
        assert_eq!(exact.rr_name, "app.internal.");
        assert!(!exact.is_wildcard);

        let wild = classify_host_name("*.corp.example").unwrap();
        assert_eq!(wild.zone, "corp.example.");
        assert_eq!(wild.rr_name, "*.corp.example.");
        assert!(wild.is_wildcard);

        assert!(classify_host_name("").is_err());
        assert!(classify_host_name("*.").is_err());
        assert!(classify_host_name("*.a.*.b").is_err());
        assert!(classify_host_name("foo*bar").is_err());
        assert!(classify_host_name("a.*.b").is_err());
    }

    #[test]
    fn single_label_zones_are_rejected() {
        // Wildcard on a public TLD would own `com.` and swallow every `.com`.
        let wildcard = classify_host_name("*.com").unwrap();
        assert_eq!(wildcard.zone, "com.");
        assert_eq!(wildcard.zone_label_count(), 1);
        assert_eq!(wildcard.reject_reason(), Some(SINGLE_LABEL_ZONE_REASON));

        // An exact single-label zone has the same failure mode.
        assert_eq!(
            classify_host_name("com").unwrap().reject_reason(),
            Some(SINGLE_LABEL_ZONE_REASON)
        );

        // Two labels are enough, wildcard or exact; deeper zones count all labels.
        assert_eq!(
            classify_host_name("*.corp.example")
                .unwrap()
                .reject_reason(),
            None
        );
        assert_eq!(
            classify_host_name("corp.example").unwrap().reject_reason(),
            None
        );
        assert_eq!(
            classify_host_name("app.internal.")
                .unwrap()
                .zone_label_count(),
            2
        );
        assert_eq!(classify_host_name("a.b.c.d").unwrap().zone_label_count(), 4);
    }

    #[test]
    fn invalid_hosts_reports_reasons() {
        let dns = DnsConfig {
            hosts: vec![
                DnsHostEntry {
                    name: "app.internal.".to_string(),
                    ips: vec!["10.0.0.1".to_string()],
                    ttl_secs: None,
                },
                DnsHostEntry {
                    name: "*.com".to_string(),
                    ips: vec!["10.0.0.2".to_string()],
                    ttl_secs: None,
                },
                DnsHostEntry {
                    name: "a.*.b".to_string(),
                    ips: vec!["10.0.0.3".to_string()],
                    ttl_secs: None,
                },
            ],
            forwarders: vec![],
            upstream_dns: vec![],
        };

        let invalid = dns.invalid_hosts();
        assert_eq!(invalid.len(), 2, "{invalid:?}");
        assert_eq!(invalid[0].0, "*.com");
        assert_eq!(invalid[0].1, SINGLE_LABEL_ZONE_REASON);
        assert_eq!(invalid[1].0, "a.*.b");
    }
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

impl DnsConfig {
    /// Hosts entries that must be skipped, with the reason (syntax + policy).
    ///
    /// Single source of truth for the three entry points: the CLI parser
    /// rejects them outright, config-file load and the runtime apply path warn
    /// and skip (a remote/managed config must never stop the node).
    pub fn invalid_hosts(&self) -> Vec<(&str, String)> {
        self.hosts
            .iter()
            .filter_map(|host| {
                classify_host_name(&host.name)
                    .and_then(|target| match target.reject_reason() {
                        Some(reason) => Err(reason.to_string()),
                        None => Ok(target),
                    })
                    .err()
                    .map(|reason| (host.name.as_str(), reason))
            })
            .collect()
    }
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
