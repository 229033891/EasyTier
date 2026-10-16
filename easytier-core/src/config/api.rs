//! Portable conversion between the shared TOML model and management schema.

use easytier_proto::api::manage::NetworkConfig;

use super::{api_input::NetworkConfigExt, toml::TomlConfig};

/// Project a live `TomlConfig` into the management `NetworkConfig` schema.
///
/// **Must stay equivalent to [`NetworkConfig::new_from_config`]** — overwrite
/// writeback (`persist_synced_runtime_config` → `get_network_instance_config`)
/// and `ConfigRpc::get_config` go through this helper, while hot-patch writeback
/// calls `new_from_config` directly. A hand-rolled duplicate previously dropped
/// `policy_routes` / `has_policy_routes` / `manual_routes_append`, so the GUI
/// tip “config synced” could succeed while the form still showed an empty
/// policy-route table.
pub fn network_config_from_toml(config: &TomlConfig) -> NetworkConfig {
    NetworkConfig::new_from_config(config)
        .expect("Toml → NetworkConfig projection must not fail on a live loader")
}

#[cfg(test)]
mod tests {
    use easytier_proto::api::manage::{self, NetworkConfig};

    use super::*;
    use crate::config::{
        api_input::NetworkConfigExt,
        toml::{ConfigLoader as _, ManagedCredentialConfig, TomlConfig},
    };
    use crate::peers::route::local_route_table::PolicyRouteConfig;

    #[test]
    fn includes_managed_credentials() {
        let config = TomlConfig::default();
        config.set_managed_credentials(vec![ManagedCredentialConfig {
            credential_id: "managed-a".to_owned(),
            credential_secret: "credential-secret".to_owned(),
            groups: vec!["ops".to_owned()],
            allow_relay: true,
            allowed_proxy_cidrs: vec!["10.0.0.0/24".to_owned()],
            expiry_unix: 2_000_000_000,
            reusable: false,
        }]);

        let projected = network_config_from_toml(&config);

        assert_eq!(
            projected.managed_credentials,
            vec![manage::ManagedCredentialConfig {
                credential_id: "managed-a".to_owned(),
                credential_secret: "credential-secret".to_owned(),
                groups: vec!["ops".to_owned()],
                allow_relay: true,
                allowed_proxy_cidrs: vec!["10.0.0.0/24".to_owned()],
                expiry_unix: 2_000_000_000,
                reusable: Some(false),
            }]
        );
    }

    /// Regression: overwrite writeback uses this helper; dropping `policy_routes`
    /// made the GUI show “synced” while the editor stayed empty.
    #[test]
    fn projects_policy_routes_and_manual_append() {
        let config = TomlConfig::default();
        config.set_policy_routes(vec![PolicyRouteConfig {
            cidr: Some("1.1.1.0/24".parse().unwrap()),
            domains: vec!["*.example.com".to_owned()],
            peers: vec!["10.10.10.50".parse().unwrap()],
            select_by: "priority".to_owned(),
            failover_when: "unreachable".to_owned(),
            if_all_unavailable: "open".to_owned(),
            require_confirmed: false,
            quality_degraded_cost: None,
        }]);
        config.set_manual_routes_append(true);

        let projected = network_config_from_toml(&config);
        assert_eq!(projected.policy_routes.len(), 1);
        assert_eq!(
            projected.policy_routes[0].cidr.as_deref(),
            Some("1.1.1.0/24")
        );
        assert_eq!(
            projected.policy_routes[0].domains,
            vec!["*.example.com".to_owned()]
        );
        assert_eq!(
            projected.policy_routes[0].peers,
            vec!["10.10.10.50".to_owned()]
        );
        assert_eq!(projected.has_policy_routes, Some(true));
        assert_eq!(projected.manual_routes_append, Some(true));

        // Same projection as the hot-patch writeback path — keep them locked together.
        let via_ext = NetworkConfig::new_from_config(&config).unwrap();
        assert_eq!(projected.policy_routes, via_ext.policy_routes);
        assert_eq!(projected.has_policy_routes, via_ext.has_policy_routes);
        assert_eq!(projected.manual_routes_append, via_ext.manual_routes_append);
    }
}
