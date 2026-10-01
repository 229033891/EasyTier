mod admin_users;
mod auth;
pub(crate) mod captcha;
mod network;
pub(crate) mod oidc;
mod peer_history;
mod rpc;
mod users;

use std::{net::SocketAddr, sync::Arc};

use axum::extract::Path;
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self as axum_mw, Next};
use axum::response::Response;
use axum::routing::{delete, post};
use axum::{Extension, Json, Router, extract::State, routing::get};
use axum_login::tower_sessions::{ExpiredDeletion, SessionManagerLayer};
use axum_login::{AuthManagerLayerBuilder, AuthUser, login_required};
use axum_messages::MessagesManagerLayer;
use easytier::common::config::{ConfigLoader, NetworkConfig, NetworkConfigExt, TomlConfigLoader};
use easytier::proto::rpc_types;
use network::NetworkApi;
use sea_orm::DbErr;
use tokio::net::TcpListener;
use tokio_util::task::AbortOnDropHandle;
use tower_sessions::Expiry;
use tower_sessions::cookie::time::Duration;
use tower_sessions::cookie::{Key, SameSite};
use tower_sessions_sqlx_store::SqliteStore;
use users::{AuthSession, Backend};

use crate::FeatureFlags;
use crate::client_manager::ClientManager;
use crate::client_manager::storage::StorageToken;
use crate::db::{Db, UserIdInDb};
use crate::webhook::SharedWebhookConfig;

pub struct RestfulServer {
    bind_addr: SocketAddr,
    client_mgr: Arc<ClientManager>,
    feature_flags: Arc<FeatureFlags>,
    webhook_config: SharedWebhookConfig,
    db: Db,
    oidc_config: oidc::OidcConfig,
    web_router: Option<Router>,
}

type AppStateInner = Arc<ClientManager>;
type AppState = State<AppStateInner>;

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct ListSessionJsonResp(Vec<StorageToken>);

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct GetSummaryJsonResp {
    device_count: u32,
    network_count: u32,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeviceArchiveItem {
    device_id: String,
    hostname: String,
    last_easytier_version: String,
    last_client_url: String,
    last_seen_at: i64,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct ListDevicesJsonResp {
    devices: Vec<DeviceArchiveItem>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct GenerateConfigRequest {
    config: NetworkConfig,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct GenerateConfigResponse {
    error: Option<String>,
    toml_config: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct ParseConfigRequest {
    toml_config: String,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct ParseConfigResponse {
    error: Option<String>,
    config: Option<NetworkConfig>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct Error {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_config_revision: Option<String>,
}
type RpcError = rpc_types::error::Error;
type HttpHandleError = (StatusCode, Json<Error>);

pub fn other_error<T: ToString>(error_message: T) -> Error {
    Error {
        message: error_message.to_string(),
        code: None,
        current_config_revision: None,
    }
}

pub fn convert_db_error(e: DbErr) -> HttpHandleError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        other_error(format!("DB Error: {:#}", e)).into(),
    )
}

impl RestfulServer {
    pub async fn new(
        bind_addr: SocketAddr,
        client_mgr: Arc<ClientManager>,
        db: Db,
        web_router: Option<Router>,
        feature_flags: Arc<FeatureFlags>,
        oidc_config: oidc::OidcConfig,
        webhook_config: SharedWebhookConfig,
    ) -> anyhow::Result<Self> {
        assert!(client_mgr.is_running());

        Ok(RestfulServer {
            bind_addr,
            client_mgr,
            feature_flags,
            webhook_config,
            db,
            oidc_config,
            web_router,
        })
    }

    async fn handle_list_all_sessions(
        auth_session: AuthSession,
        State(client_mgr): AppState,
    ) -> Result<Json<ListSessionJsonResp>, HttpHandleError> {
        let Some(user) = auth_session.user else {
            return Err((StatusCode::UNAUTHORIZED, other_error("No such user").into()));
        };
        let ret = client_mgr.list_sessions_by_user_id(user.id()).await;
        Ok(ListSessionJsonResp(ret).into())
    }

    async fn handle_get_summary(
        auth_session: AuthSession,
        State(client_mgr): AppState,
    ) -> Result<Json<GetSummaryJsonResp>, HttpHandleError> {
        let Some(user) = auth_session.user else {
            return Err((StatusCode::UNAUTHORIZED, other_error("No such user").into()));
        };

        let machines = client_mgr.list_machine_by_user_id(user.id()).await;
        let mut network_count = 0u32;
        for client_url in machines.iter() {
            if let Some(session) = client_mgr.get_heartbeat_requests(client_url).await {
                network_count += session.running_network_instances.len() as u32;
            }
        }

        Ok(GetSummaryJsonResp {
            device_count: machines.len() as u32,
            network_count,
        }
        .into())
    }

    async fn handle_list_devices(
        auth_session: AuthSession,
        Extension(db): Extension<Db>,
    ) -> Result<Json<ListDevicesJsonResp>, HttpHandleError> {
        let Some(user) = auth_session.user else {
            return Err((StatusCode::UNAUTHORIZED, other_error("No such user").into()));
        };

        let devices = db
            .list_user_devices(user.id())
            .await
            .map_err(convert_db_error)?;

        Ok(Json(ListDevicesJsonResp {
            devices: devices
                .into_iter()
                .map(|d| DeviceArchiveItem {
                    device_id: d.device_id,
                    hostname: d.hostname,
                    last_easytier_version: d.last_easytier_version,
                    last_client_url: d.last_client_url,
                    last_seen_at: d.last_seen_at,
                })
                .collect(),
        }))
    }

    async fn handle_generate_config(
        Json(req): Json<GenerateConfigRequest>,
    ) -> Result<Json<GenerateConfigResponse>, HttpHandleError> {
        let config = req.config.gen_config();
        match config {
            Ok(c) => Ok(GenerateConfigResponse {
                error: None,
                toml_config: Some(c.dump()),
            }
            .into()),
            Err(e) => Ok(GenerateConfigResponse {
                error: Some(format!("{:?}", e)),
                toml_config: None,
            }
            .into()),
        }
    }

    async fn handle_parse_config(
        Json(req): Json<ParseConfigRequest>,
    ) -> Result<Json<ParseConfigResponse>, HttpHandleError> {
        let config = TomlConfigLoader::new_from_str(&req.toml_config)
            .and_then(|config| NetworkConfig::new_from_config(&config));
        match config {
            Ok(c) => Ok(ParseConfigResponse {
                error: None,
                config: Some(c),
            }
            .into()),
            Err(e) => Ok(ParseConfigResponse {
                error: Some(format!("{:?}", e)),
                config: None,
            }
            .into()),
        }
    }

    #[allow(unused_mut)]
    pub async fn start(
        mut self,
    ) -> Result<
        (
            AbortOnDropHandle<()>,
            AbortOnDropHandle<tower_sessions::session_store::Result<()>>,
        ),
        anyhow::Error,
    > {
        let listener = TcpListener::bind(self.bind_addr).await?;

        // Session layer.
        //
        // This uses `tower-sessions` to establish a layer that will provide the session
        // as a request extension.
        let session_store = SqliteStore::new(self.db.inner());
        session_store.migrate().await?;

        let delete_task = AbortOnDropHandle::new(tokio::task::spawn(
            session_store
                .clone()
                .continuously_delete_expired(tokio::time::Duration::from_secs(60)),
        ));

        // Persist session signing key next to the DB so restarts don't invalidate cookies.
        let key = load_or_create_session_key(self.db.db_path());
        let cookie_secure = session_cookie_secure_from_env();

        let session_layer = SessionManagerLayer::new(session_store)
            .with_secure(cookie_secure)
            .with_same_site(SameSite::Lax)
            .with_expiry(Expiry::OnInactivity(Duration::days(1)))
            .with_signed(key);

        // Auth service.
        //
        // This combines the session layer with our backend to establish the auth
        // service which will provide the auth session as a request extension.
        let backend = Backend::new(self.db.clone());
        let auth_layer = AuthManagerLayerBuilder::new(backend, session_layer).build();
        let compression_layer = tower_http::compression::CompressionLayer::new()
            .br(true)
            .deflate(true)
            .gzip(true)
            .zstd(true)
            .quality(tower_http::compression::CompressionLevel::Default);

        // Token-authenticated management routes that bypass session auth.
        let internal_app = if self.webhook_config.has_internal_auth() {
            let internal_token = self.webhook_config.internal_auth_token.clone().unwrap();
            let internal_routes = Router::new()
                .route(
                    "/api/internal/sessions",
                    get(Self::handle_list_all_sessions_internal),
                )
                .route(
                    "/api/internal/users/{user-id}/sessions/{machine-id}",
                    delete(Self::handle_disconnect_session_internal),
                )
                .merge(NetworkApi::build_route_internal())
                .merge(rpc::router_internal())
                .with_state(self.client_mgr.clone())
                .layer(axum_mw::from_fn(move |req, next| {
                    let token = internal_token.clone();
                    internal_auth_middleware(token, req, next)
                }));
            Some(internal_routes)
        } else {
            None
        };

        let mut app = Router::new()
            .route("/api/v1/summary", get(Self::handle_get_summary))
            .route("/api/v1/sessions", get(Self::handle_list_all_sessions))
            .route("/api/v1/devices", get(Self::handle_list_devices))
            .merge(NetworkApi::build_route())
            .merge(peer_history::PeerHistoryApi::build_route())
            .merge(rpc::router())
            .route_layer(login_required!(Backend))
            .merge(auth::router())
            .merge(admin_users::router())
            .merge(oidc::router())
            .with_state(self.client_mgr.clone())
            .route(
                "/api/v1/generate-config",
                post(Self::handle_generate_config),
            )
            .route("/api/v1/parse-config", post(Self::handle_parse_config))
            .layer(Extension(self.oidc_config.clone()))
            .layer(Extension(self.db.clone()))
            .layer(MessagesManagerLayer)
            .layer(auth_layer)
            .layer(tower_http::cors::CorsLayer::very_permissive())
            .layer(compression_layer);

        if let Some(internal_routes) = internal_app {
            app = app.merge(internal_routes);
        }

        #[cfg(feature = "embed")]
        let app = if let Some(web_router) = self.web_router.take() {
            app.merge(web_router)
        } else {
            app
        };

        let serve_task = AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));

        Ok((serve_task, delete_task))
    }

    /// Session listing endpoint for token-authenticated management clients.
    async fn handle_list_all_sessions_internal(
        State(client_mgr): AppState,
    ) -> Result<Json<ListSessionJsonResp>, HttpHandleError> {
        let ret = client_mgr.list_all_sessions().await;
        Ok(ListSessionJsonResp(ret).into())
    }

    async fn handle_disconnect_session_internal(
        Path((user_id, machine_id)): Path<(UserIdInDb, uuid::Uuid)>,
        State(client_mgr): AppState,
    ) -> Result<StatusCode, HttpHandleError> {
        if client_mgr
            .disconnect_session_by_machine_id(user_id, &machine_id)
            .await
        {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err((
                StatusCode::NOT_FOUND,
                other_error("session not found").into(),
            ))
        }
    }
}

/// Middleware that validates X-Internal-Auth for token-authenticated routes.
async fn internal_auth_middleware(
    expected_token: String,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let auth_header = req
        .headers()
        .get("X-Internal-Auth")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(token) if token == expected_token => next.run(req).await,
        _ => Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                r#"{"error":"unauthorized: invalid or missing X-Internal-Auth header"}"#,
            ))
            .unwrap(),
    }
}

fn session_cookie_secure_from_env() -> bool {
    match std::env::var("ET_SESSION_COOKIE_SECURE") {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"),
        Err(_) => false,
    }
}

fn load_or_create_session_key(db_path: &str) -> Key {
    const MIN_LEN: usize = 64;
    if let Ok(from_env) = std::env::var("ET_SESSION_SECRET") {
        let bytes = from_env.into_bytes();
        if bytes.len() >= MIN_LEN {
            if let Ok(key) = Key::try_from(bytes.as_slice()) {
                return key;
            }
        } else {
            tracing::warn!(
                "ET_SESSION_SECRET is too short (need at least {MIN_LEN} bytes); generating a key file instead"
            );
        }
    }

    let Some(path) = filesystem_path_from_sqlite_url(db_path) else {
        return Key::generate();
    };
    let key_path = path.with_file_name(format!(
        "{}.session.key",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("et.db")
    ));

    if let Ok(bytes) = std::fs::read(&key_path) {
        if bytes.len() >= MIN_LEN {
            if let Ok(key) = Key::try_from(bytes.as_slice()) {
                return key;
            }
        }
    }

    let key = Key::generate();
    if let Err(e) = std::fs::write(&key_path, key.master()) {
        tracing::warn!("Failed to persist session signing key to {}: {e}", key_path.display());
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        }
    }
    key
}

fn filesystem_path_from_sqlite_url(db_path: &str) -> Option<std::path::PathBuf> {
    if db_path.ends_with(":memory:") || db_path.contains("mode=memory") {
        return None;
    }
    let path = db_path
        .strip_prefix("sqlite://")
        .or_else(|| db_path.strip_prefix("sqlite:"))
        .unwrap_or(db_path);
    let path = path
        .strip_prefix("file:")
        .unwrap_or(path)
        .split('?')
        .next()
        .filter(|p| !p.is_empty())?;
    Some(std::path::PathBuf::from(path))
}
