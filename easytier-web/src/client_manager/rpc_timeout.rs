//! Graded RPC timeouts for console → node management calls.
//!
//! Default `BaseController` is 5s, which is too short for run/delete/patch/collect.
//! Keep status/meta at 5s; slow mutators and recollect at 60s.

use easytier::proto::rpc_types::controller::{BaseController, Controller as _};

pub(crate) const MANAGED_RPC_FAST_TIMEOUT_MS: i32 = 5_000;
pub(crate) const MANAGED_RPC_SLOW_TIMEOUT_MS: i32 = 60_000;

pub(crate) fn rpc_controller(timeout_ms: i32) -> BaseController {
    let mut ctrl = BaseController::default();
    ctrl.set_timeout_ms(timeout_ms);
    ctrl
}

pub(crate) fn slow_rpc_controller() -> BaseController {
    rpc_controller(MANAGED_RPC_SLOW_TIMEOUT_MS)
}

/// Whether this proxy-rpc method should use the slow timeout.
///
/// Aligns with revision-fence mutators in `restful/rpc.rs`, plus recollect.
pub(crate) fn proxy_rpc_needs_slow_timeout(service_name: &str, method_name: &str) -> bool {
    matches!(
        (service_name, method_name),
        (
            "api.manage.WebClientService",
            "run_network_instance"
                | "RunNetworkInstance"
                | "retain_network_instance"
                | "RetainNetworkInstance"
                | "delete_network_instance"
                | "DeleteNetworkInstance"
                | "collect_network_info"
                | "CollectNetworkInfo"
        ) | (
            "api.config.ConfigRpcService",
            "patch_config" | "PatchConfig"
        ) | (
            "api.instance.CredentialManageRpcService",
            "generate_credential"
                | "GenerateCredential"
                | "revoke_credential"
                | "RevokeCredential"
                | "upsert_credential"
                | "UpsertCredential"
        )
    )
}

/// Timeout for a proxy-rpc `(service, method)` pair.
pub(crate) fn proxy_rpc_timeout_ms(service_name: &str, method_name: &str) -> i32 {
    if proxy_rpc_needs_slow_timeout(service_name, method_name) {
        MANAGED_RPC_SLOW_TIMEOUT_MS
    } else {
        MANAGED_RPC_FAST_TIMEOUT_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_controller_uses_graded_timeout() {
        assert_eq!(
            slow_rpc_controller().timeout_ms(),
            MANAGED_RPC_SLOW_TIMEOUT_MS
        );
        assert_eq!(
            rpc_controller(MANAGED_RPC_FAST_TIMEOUT_MS).timeout_ms(),
            MANAGED_RPC_FAST_TIMEOUT_MS
        );
    }

    #[test]
    fn proxy_rpc_timeout_grades_mutators_and_collect() {
        assert_eq!(
            proxy_rpc_timeout_ms("api.manage.WebClientService", "run_network_instance"),
            MANAGED_RPC_SLOW_TIMEOUT_MS
        );
        assert_eq!(
            proxy_rpc_timeout_ms("api.manage.WebClientService", "CollectNetworkInfo"),
            MANAGED_RPC_SLOW_TIMEOUT_MS
        );
        assert_eq!(
            proxy_rpc_timeout_ms("api.manage.WebClientService", "list_network_instance"),
            MANAGED_RPC_FAST_TIMEOUT_MS
        );
        assert_eq!(
            proxy_rpc_timeout_ms("api.config.ConfigRpcService", "get_config"),
            MANAGED_RPC_FAST_TIMEOUT_MS
        );
    }
}
