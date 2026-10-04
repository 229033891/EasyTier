//! Process-wide handle for the active config-server client.
//!
//! Status (`config_server_status`) is observational. This registry is the
//! actionable peer used by `WebClientService.ReportManagedNetworkConfig` so a
//! GUI in service/remote mode can sync web-owned configs through the process
//! that actually owns the config-server session (the Windows service / daemon).
//!
//! Ownership model:
//! - `WebClient` installs a `ConfigServerReportClient` on start and clears it on Drop
//!   (pointer-equality safe across overlapping lifetimes).
//! - Same-process callers (normal-mode GUI) use [`config_server_report_client`].
//! - Cross-process callers (service/remote GUI) use the RPC method, which reads
//!   the same registry in the owner process.

use std::sync::Arc;

use async_trait::async_trait;
use easytier_proto::api::manage::{NetworkConfig, ReportManagedNetworkConfigResponse};
use parking_lot::RwLock;

/// Wire `error_code` values for [`ReportManagedNetworkConfigResponse`].
///
/// Keep these stable: GUI / CLI / FFI map them to user-facing copy.
pub mod error_code {
    pub const NOT_ENABLED: &str = "not_enabled";
    pub const NOT_CONNECTED: &str = "not_connected";
    pub const NOT_AUTHORIZED: &str = "not_authorized";
    pub const REVISION_CONFLICT: &str = "revision_conflict";
    pub const OWNERSHIP_CONFLICT: &str = "ownership_conflict";
    pub const INVALID: &str = "invalid";
}

/// Error from reporting a client-edited web-owned config to the console.
#[derive(Debug, thiserror::Error)]
pub enum ReportNetworkConfigError {
    #[error("config server is not connected")]
    NotConnected,
    #[error("config server rejected the session credentials")]
    NotAuthorized,
    #[error("managed config revision conflict (current={current:?})")]
    RevisionConflict { current: Option<String> },
    #[error("managed config ownership conflict")]
    OwnershipConflict,
    #[error("invalid report request: {0}")]
    Invalid(String),
    #[error("config-server client is not enabled in this process")]
    NotEnabled,
    #[error(transparent)]
    Rpc(#[from] easytier_proto::rpc_types::error::Error),
}

impl ReportNetworkConfigError {
    /// Stable wire `error_code` for cross-process RPC responses.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::NotEnabled => error_code::NOT_ENABLED,
            Self::NotConnected => error_code::NOT_CONNECTED,
            Self::NotAuthorized => error_code::NOT_AUTHORIZED,
            Self::RevisionConflict { .. } => error_code::REVISION_CONFLICT,
            Self::OwnershipConflict => error_code::OWNERSHIP_CONFLICT,
            Self::Invalid(_) | Self::Rpc(_) => error_code::INVALID,
        }
    }

    /// Operator-facing message suitable for RPC `message` / GUI toasts.
    pub fn operator_message(&self) -> String {
        match self {
            Self::NotEnabled => "config-server client is not running in this process".to_owned(),
            Self::NotConnected => "config server is not connected".to_owned(),
            Self::NotAuthorized => "config server rejected the session".to_owned(),
            Self::RevisionConflict { .. } => "managed config revision conflict".to_owned(),
            Self::OwnershipConflict => "managed config ownership conflict".to_owned(),
            Self::Invalid(message) => message.clone(),
            Self::Rpc(error) => error.to_string(),
        }
    }

    /// Convert to the RPC response shape used by `ReportManagedNetworkConfig`.
    pub fn into_managed_response(self) -> ReportManagedNetworkConfigResponse {
        let current_config_revision = match &self {
            Self::RevisionConflict { current } => current.clone().unwrap_or_default(),
            _ => String::new(),
        };
        ReportManagedNetworkConfigResponse {
            ok: false,
            error_code: self.error_code().to_owned(),
            current_config_revision,
            applied_config_revision: String::new(),
            message: self.operator_message(),
        }
    }

    /// GUI-facing copy after a local persist already succeeded.
    pub fn gui_sync_message(&self) -> String {
        match self {
            Self::NotEnabled => {
                "web-owned config saved locally but config-server client is not running; reconnect to sync"
                    .to_owned()
            }
            Self::NotConnected => {
                "web-owned config saved locally but config-server is disconnected; reconnect to sync"
                    .to_owned()
            }
            Self::NotAuthorized => {
                "web-owned config saved locally but the config server rejected this session; sign in again to sync"
                    .to_owned()
            }
            Self::RevisionConflict { .. } => {
                "config revision conflict: your local edit was kept; retry to push it, or reload from the console to discard it"
                    .to_owned()
            }
            Self::OwnershipConflict => {
                "web-owned config saved locally but ownership conflicted on the console; reload from the console"
                    .to_owned()
            }
            Self::Invalid(message) if !message.is_empty() => message.clone(),
            other => format!(
                "web-owned config saved locally but sync failed ({})",
                other.error_code()
            ),
        }
    }
}

/// Map a wire `error_code` (+ optional message) to GUI copy after local persist.
pub fn gui_sync_message_for_error_code(error_code: &str, message: &str) -> String {
    match error_code {
        error_code::NOT_ENABLED => ReportNetworkConfigError::NotEnabled.gui_sync_message(),
        error_code::NOT_CONNECTED => ReportNetworkConfigError::NotConnected.gui_sync_message(),
        error_code::NOT_AUTHORIZED => ReportNetworkConfigError::NotAuthorized.gui_sync_message(),
        error_code::REVISION_CONFLICT => {
            ReportNetworkConfigError::RevisionConflict { current: None }.gui_sync_message()
        }
        error_code::OWNERSHIP_CONFLICT => {
            ReportNetworkConfigError::OwnershipConflict.gui_sync_message()
        }
        _ if !message.is_empty() => message.to_owned(),
        other => format!("web-owned config saved locally but sync failed ({other})"),
    }
}

/// Success response for `ReportManagedNetworkConfig`.
pub fn managed_report_ok() -> ReportManagedNetworkConfigResponse {
    ReportManagedNetworkConfigResponse {
        ok: true,
        error_code: String::new(),
        current_config_revision: String::new(),
        applied_config_revision: String::new(),
        message: String::new(),
    }
}

/// Capability required to push a web-owned NetworkConfig to the console.
#[async_trait]
pub trait ConfigServerReportClient: Send + Sync {
    fn is_connected(&self) -> bool;

    async fn report_network_config(
        &self,
        config: NetworkConfig,
    ) -> Result<(), ReportNetworkConfigError>;
}

static REPORT_CLIENT: RwLock<Option<Arc<dyn ConfigServerReportClient>>> = RwLock::new(None);

/// Install or replace the process-wide report client (typically on WebClient start).
pub fn install_config_server_report_client(client: Option<Arc<dyn ConfigServerReportClient>>) {
    *REPORT_CLIENT.write() = client;
}

/// Clear the installed client only if it is still `expected` (Drop-safe with
/// overlapping client lifetimes).
pub fn clear_config_server_report_client(expected: &Arc<dyn ConfigServerReportClient>) {
    let mut guard = REPORT_CLIENT.write();
    if guard
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, expected))
    {
        *guard = None;
    }
}

/// Borrow the active report client, if any.
pub fn config_server_report_client() -> Option<Arc<dyn ConfigServerReportClient>> {
    REPORT_CLIENT.read().clone()
}

/// Report through the process-local registry, or `NotEnabled` when absent.
pub async fn report_via_process_client(
    config: NetworkConfig,
) -> Result<(), ReportNetworkConfigError> {
    let Some(client) = config_server_report_client() else {
        return Err(ReportNetworkConfigError::NotEnabled);
    };
    client.report_network_config(config).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    // Process-wide registry must not be mutated concurrently across tests.
    static TEST_REGISTRY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct DummyClient {
        connected: AtomicBool,
    }

    #[async_trait]
    impl ConfigServerReportClient for DummyClient {
        fn is_connected(&self) -> bool {
            self.connected.load(Ordering::Acquire)
        }

        async fn report_network_config(
            &self,
            _config: NetworkConfig,
        ) -> Result<(), ReportNetworkConfigError> {
            if !self.is_connected() {
                return Err(ReportNetworkConfigError::NotConnected);
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn install_clear_is_ptr_eq_safe() {
        let _guard = TEST_REGISTRY_LOCK.lock().await;
        let first: Arc<dyn ConfigServerReportClient> = Arc::new(DummyClient {
            connected: AtomicBool::new(true),
        });
        let second: Arc<dyn ConfigServerReportClient> = Arc::new(DummyClient {
            connected: AtomicBool::new(true),
        });
        install_config_server_report_client(Some(first.clone()));
        install_config_server_report_client(Some(second.clone()));
        // Stale Drop of the replaced client must not wipe the newer one.
        clear_config_server_report_client(&first);
        assert!(config_server_report_client().is_some());
        clear_config_server_report_client(&second);
        assert!(config_server_report_client().is_none());
    }

    #[test]
    fn error_mapping_is_stable() {
        let conflict = ReportNetworkConfigError::RevisionConflict {
            current: Some("rev-9".into()),
        };
        let response = conflict.into_managed_response();
        assert!(!response.ok);
        assert_eq!(response.error_code, error_code::REVISION_CONFLICT);
        assert_eq!(response.current_config_revision, "rev-9");
        assert_eq!(
            gui_sync_message_for_error_code(error_code::NOT_ENABLED, ""),
            ReportNetworkConfigError::NotEnabled.gui_sync_message()
        );
    }

    #[tokio::test]
    async fn report_via_process_client_requires_install() {
        let _guard = TEST_REGISTRY_LOCK.lock().await;
        let client: Arc<dyn ConfigServerReportClient> = Arc::new(DummyClient {
            connected: AtomicBool::new(true),
        });
        install_config_server_report_client(Some(client.clone()));
        report_via_process_client(NetworkConfig::default())
            .await
            .unwrap();
        clear_config_server_report_client(&client);
        let err = report_via_process_client(NetworkConfig::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ReportNetworkConfigError::NotEnabled));
    }
}
