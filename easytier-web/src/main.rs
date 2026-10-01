#![allow(dead_code)]

#[macro_use]
extern crate rust_i18n;

use std::net::IpAddr;
use std::sync::Arc;

use clap::Parser;
use easytier::tunnel::websocket::WsTunnelListener;
use easytier::{
    common::{
        config::{ConsoleLoggerConfig, FileLoggerConfig, LoggingConfigLoader},
        constants::EASYTIER_VERSION,
        error::Error,
        log,
        network::{local_ipv4, local_ipv6},
    },
    proto::rpc::standalone::{runtime_rpc_listener, runtime_udp_tunnel_listener},
    utils::panic::setup_panic_handler,
};
use easytier_core::{socket::SocketListener, tunnel::Tunnel};

use easytier::tunnel::IpScheme;
use mimalloc::MiMalloc;

mod client_manager;
mod db;
mod migrator;
mod peer_history;
mod restful;
mod webhook;

#[cfg(feature = "embed")]
mod web;

#[global_allocator]
static GLOBAL_MIMALLOC: MiMalloc = MiMalloc;

rust_i18n::i18n!("locales", fallback = "en");

#[derive(Parser, Debug)]
#[command(name = "easytier-web", author, version = EASYTIER_VERSION , about, long_about = None)]
struct Cli {
    #[arg(
        short,
        long,
        env = "ET_WEB_DB",
        default_value = "et.db",
        help = t!("cli.db").to_string()
    )]
    db: String,

    #[arg(
        long,
        env = "ET_WEB_CONSOLE_LOG_LEVEL",
        help = t!("cli.console_log_level").to_string(),
    )]
    console_log_level: Option<String>,

    #[arg(
        long,
        env = "ET_WEB_FILE_LOG_LEVEL",
        help = t!("cli.file_log_level").to_string(),
    )]
    file_log_level: Option<String>,

    #[arg(
        long,
        env = "ET_WEB_FILE_LOG_DIR",
        help = t!("cli.file_log_dir").to_string(),
    )]
    file_log_dir: Option<String>,

    #[arg(
        long,
        short='c',
        env = "ET_CONFIG_SERVER_PORT",
        default_value = "22020",
        help = t!("cli.config_server_port").to_string(),
    )]
    config_server_port: u16,

    #[arg(
        long,
        short='p',
        env = "ET_CONFIG_SERVER_PROTOCOL",
        default_value = "udp,tcp",
        help = t!("cli.config_server_protocol").to_string(),
    )]
    config_server_protocol: String,

    #[arg(
        long,
        short='a',
        env = "ET_API_SERVER_PORT",
        default_value = "11211",
        help = t!("cli.api_server_port").to_string(),
    )]
    api_server_port: u16,

    #[arg(
        long,
        env = "ET_API_SERVER_ADDR",
        default_value = "0.0.0.0",
        help = t!("cli.api_server_addr").to_string(),
    )]
    api_server_addr: IpAddr,

    #[arg(
        long,
        env = "ET_GEOIP_DB",
        help = t!("cli.geoip_db").to_string(),
    )]
    geoip_db: Option<String>,

    #[arg(
        long,
        env = "ET_HEARTBEAT_MIN_RESPONSE_MS",
        default_value = "3500",
        help = t!("cli.heartbeat_min_response_ms").to_string(),
    )]
    heartbeat_min_response_ms: u64,

    #[arg(
        long,
        env = "ET_HEARTBEAT_TIMEOUT_MS",
        default_value = "15000",
        help = t!("cli.heartbeat_timeout_ms").to_string(),
    )]
    heartbeat_timeout_ms: u64,

    #[cfg(feature = "embed")]
    #[arg(
        long,
        short='l',
        env = "ET_WEB_SERVER_PORT",
        help = t!("cli.web_server_port").to_string(),
    )]
    web_server_port: Option<u16>,

    #[cfg(feature = "embed")]
    #[arg(
        long,
        env = "ET_WEB_SERVER_ADDR",
        default_value = "0.0.0.0",
        help = t!("cli.web_server_addr").to_string(),
    )]
    web_server_addr: IpAddr,

    #[cfg(feature = "embed")]
    #[arg(
        long,
        env = "ET_NO_WEB",
        help = t!("cli.no_web").to_string(),
        default_value = "false"
    )]
    no_web: bool,

    #[cfg(feature = "embed")]
    #[arg(
        long,
        env = "ET_API_HOST",
        help = t!("cli.api_host").to_string()
    )]
    api_host: Option<url::Url>,

    #[arg(
        long,
        env = "ET_PEER_HISTORY_INTERVAL_SECS",
        default_value = "60",
        help = t!("cli.peer_history_interval_secs").to_string(),
    )]
    peer_history_interval_secs: u64,

    #[arg(
        long,
        env = "ET_PEER_HISTORY_RETENTION_DAYS",
        default_value = "7",
        help = t!("cli.peer_history_retention_days").to_string(),
    )]
    peer_history_retention_days: i64,

    #[command(flatten)]
    feature_flags: FeatureFlags,

    /// Reset a user's password and exit (does not start the server).
    /// Use the same plaintext the user would type on the login page.
    #[arg(
        long,
        env = "ET_RESET_PASSWORD_USER",
        requires = "new_password",
        help = t!("cli.reset_password_user").to_string()
    )]
    reset_password_user: Option<String>,

    #[arg(
        long,
        env = "ET_NEW_PASSWORD",
        hide_env_values = true,
        requires = "reset_password_user",
        help = t!("cli.new_password").to_string()
    )]
    new_password: Option<String>,

    #[command(flatten)]
    oidc: restful::oidc::OidcOptions,

    #[command(flatten)]
    webhook: WebhookOptions,
}

#[derive(Debug, Clone, Default, clap::Args)]
pub struct WebhookOptions {
    /// Base URL of the webhook endpoint for token validation and event delivery.
    /// When set, incoming tokens are validated via this webhook before local fallback.
    #[arg(long, env = "ET_WEBHOOK_URL")]
    pub webhook_url: Option<String>,

    /// Shared secret used to authenticate outbound webhook calls.
    #[arg(long, env = "ET_WEBHOOK_SECRET", hide_env_values = true)]
    pub webhook_secret: Option<String>,

    /// Token for X-Internal-Auth header. When set, API requests with this header
    /// bypass session authentication.
    #[arg(long, env = "ET_INTERNAL_AUTH_TOKEN", hide_env_values = true)]
    pub internal_auth_token: Option<String>,

    /// Stable identifier for this easytier-web instance when routing webhook callbacks.
    #[arg(long, env = "ET_WEB_INSTANCE_ID")]
    pub web_instance_id: Option<String>,

    /// Reachable base URL for this easytier-web instance's internal REST API.
    #[arg(long, env = "ET_WEB_INSTANCE_API_BASE_URL")]
    pub web_instance_api_base_url: Option<String>,
}

#[derive(Debug, Clone, Default, clap::Args)]
pub struct FeatureFlags {
    /// Deprecated no-op: public self-registration was removed; kept so old flags/env still parse.
    #[arg(
        long,
        env = "ET_DISABLE_REGISTRATION",
        default_value = "true",
        hide = true
    )]
    pub disable_registration: bool,

    /// Whether to auto-create users when they connect via heartbeat with an unknown token.
    #[arg(
        long,
        env = "ET_ALLOW_AUTO_CREATE_USER",
        default_value = "false",
        help = t!("cli.allow_auto_create_user").to_string()
    )]
    pub allow_auto_create_user: bool,
}

impl LoggingConfigLoader for &Cli {
    fn get_console_logger_config(&self) -> ConsoleLoggerConfig {
        ConsoleLoggerConfig {
            level: self.console_log_level.clone(),
        }
    }

    fn get_file_logger_config(&self) -> FileLoggerConfig {
        FileLoggerConfig {
            dir: self.file_log_dir.clone(),
            level: self.file_log_level.clone(),
            file: None,
            size_mb: None,
            count: None,
        }
    }
}

pub fn get_listener_by_url(
    scheme: IpScheme,
    l: &url::Url,
) -> Option<Box<dyn SocketListener<Accepted = Box<dyn Tunnel>>>> {
    Some(match scheme {
        IpScheme::Tcp => {
            let addr = l.socket_addrs(|| Some(11010)).ok()?.into_iter().next()?;
            Box::new(runtime_rpc_listener(addr))
        }
        IpScheme::Udp => {
            let addr = l.socket_addrs(|| Some(11010)).ok()?.into_iter().next()?;
            Box::new(runtime_udp_tunnel_listener(l.clone(), addr))
        }
        IpScheme::Ws => Box::new(WsTunnelListener::new(l.clone())),
        _ => return None,
    })
}

/// Parse `--config-server-protocol`, e.g. `udp`, `tcp`, `ws`, or `udp,tcp`.
fn parse_config_server_protocols(protocol: &str) -> Result<Vec<IpScheme>, Error> {
    let mut schemes = Vec::new();
    for part in protocol.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let scheme: IpScheme = part
            .parse()
            .map_err(|_| Error::InvalidUrl(part.to_string()))?;
        if !matches!(scheme, IpScheme::Udp | IpScheme::Tcp | IpScheme::Ws) {
            return Err(Error::InvalidUrl(part.to_string()));
        }
        if !schemes.contains(&scheme) {
            schemes.push(scheme);
        }
    }
    if schemes.is_empty() {
        return Err(Error::InvalidUrl(protocol.to_string()));
    }
    // TCP and WS both bind a TCP port; sharing one port will fail.
    if schemes.contains(&IpScheme::Tcp) && schemes.contains(&IpScheme::Ws) {
        return Err(Error::InvalidUrl(
            "tcp and ws cannot share the same config-server port; pick one or use separate ports"
                .to_string(),
        ));
    }
    Ok(schemes)
}

async fn get_dual_stack_listener(
    protocol: &str,
    port: u16,
) -> Result<
    (
        Option<Box<dyn SocketListener<Accepted = Box<dyn Tunnel>>>>,
        Option<Box<dyn SocketListener<Accepted = Box<dyn Tunnel>>>>,
    ),
    Error,
> {
    let scheme = protocol
        .parse()
        .map_err(|_| Error::InvalidUrl(protocol.to_string()))?;
    let v6_listener =
        if local_ipv6().await.is_ok() && matches!(scheme, IpScheme::Tcp | IpScheme::Udp) {
            get_listener_by_url(
                scheme,
                &format!("{protocol}://[::]:{port}").parse().unwrap(),
            )
        } else {
            None
        };
    let v4_listener = if local_ipv4().await.is_ok() {
        get_listener_by_url(
            scheme,
            &format!("{protocol}://0.0.0.0:{port}").parse().unwrap(),
        )
    } else {
        None
    };
    Ok((v6_listener, v4_listener))
}

async fn add_config_server_listeners(
    mgr: &mut client_manager::ClientManager,
    protocol_spec: &str,
    port: u16,
) -> Result<(), Error> {
    let schemes = parse_config_server_protocols(protocol_spec)?;
    let mut added = 0usize;

    for scheme in schemes {
        let protocol = match scheme {
            IpScheme::Udp => "udp",
            IpScheme::Tcp => "tcp",
            IpScheme::Ws => "ws",
            _ => continue,
        };
        let (v6_listener, v4_listener) = get_dual_stack_listener(protocol, port).await?;
        let mut scheme_added = 0usize;
        if let Some(listener) = v6_listener {
            let url = mgr
                .add_listener(listener)
                .await
                .map_err(|e| Error::InvalidUrl(format!("listen {protocol} v6 failed: {e}")))?;
            tracing::info!(%url, protocol, "config-server listener started");
            scheme_added += 1;
        }
        if let Some(listener) = v4_listener {
            let url = mgr
                .add_listener(listener)
                .await
                .map_err(|e| Error::InvalidUrl(format!("listen {protocol} v4 failed: {e}")))?;
            tracing::info!(%url, protocol, "config-server listener started");
            scheme_added += 1;
        }
        if scheme_added == 0 {
            return Err(Error::InvalidUrl(format!(
                "Failed to listen {protocol} on port {port} (neither IPv4 nor IPv6)"
            )));
        }
        added += scheme_added;
    }

    if added == 0 {
        return Err(Error::InvalidUrl(
            "Listen to both IPv4 and IPv6 failed for all config-server protocols".to_string(),
        ));
    }
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let locale = sys_locale::get_locale().unwrap_or_else(|| String::from("en-US"));
    rust_i18n::set_locale(&locale);
    setup_panic_handler();

    let cli = Cli::parse();
    log::init_with_default_console_targets(&cli, false, &["CORE", "easytier_web"]).unwrap();
    tracing::info!(
        version = EASYTIER_VERSION,
        web_instance_id = ?cli.webhook.web_instance_id,
        api_address = %cli.api_server_addr,
        api_port = cli.api_server_port,
        config_protocol = %cli.config_server_protocol,
        config_port = cli.config_server_port,
        heartbeat_min_response_ms = cli.heartbeat_min_response_ms,
        heartbeat_timeout_ms = cli.heartbeat_timeout_ms,
        webhook_enabled = cli.webhook.webhook_url.as_deref().is_some_and(|url| !url.trim().is_empty()),
        rust_log_override = std::env::var_os("RUST_LOG").is_some(),
        console_log_override = cli.console_log_level.is_some(),
        "easytier-web starting"
    );

    // let db = db::Db::new(":memory:").await.unwrap();
    let db = db::Db::new(cli.db.clone()).await.unwrap();

    if let (Some(username), Some(password)) = (&cli.reset_password_user, &cli.new_password) {
        match db
            .set_user_password_by_username(username, db::hash_web_login_password(password))
            .await
        {
            Ok(()) => {
                println!("Password updated for user '{username}'.");
                return;
            }
            Err(e) => {
                eprintln!("Failed to reset password for '{username}': {e}");
                std::process::exit(1);
            }
        }
    }

    // Validate OIDC configuration: check split-deploy specific requirements
    // Basic OIDC parameter validation is handled in OidcConfig::from_params
    if cli.oidc.any_param_provided() {
        let is_split_deploy = {
            #[cfg(feature = "embed")]
            {
                let embed_split_by_port = cli.web_server_port.is_some()
                    && cli.web_server_port != Some(cli.api_server_port);
                cli.no_web || embed_split_by_port
            }
            #[cfg(not(feature = "embed"))]
            {
                true
            }
        };

        if is_split_deploy && cli.oidc.oidc_frontend_base_url.is_none() {
            eprintln!("Error: --oidc-frontend-base-url is required in split-deploy mode");
            eprintln!(
                "When frontend and API are deployed separately, you must specify the frontend URL"
            );
            eprintln!("Example: --oidc-frontend-base-url http://your-frontend-domain.com");
            std::process::exit(1);
        }
    }

    let feature_flags = Arc::new(cli.feature_flags);
    let webhook_config = Arc::new(webhook::WebhookConfig::new(
        cli.webhook.webhook_url,
        cli.webhook.webhook_secret,
        cli.webhook.internal_auth_token,
        cli.webhook.web_instance_id,
        cli.webhook.web_instance_api_base_url,
    ));
    let heartbeat_policy = client_manager::HeartbeatPolicy::from_millis(
        cli.heartbeat_min_response_ms,
        cli.heartbeat_timeout_ms,
    )
    .unwrap_or_else(|error| {
        eprintln!("Invalid heartbeat configuration: {error}");
        std::process::exit(2);
    });
    let mut mgr = client_manager::ClientManager::new(
        db.clone(),
        cli.geoip_db,
        heartbeat_policy,
        feature_flags.clone(),
        webhook_config.clone(),
    );
    add_config_server_listeners(
        &mut mgr,
        &cli.config_server_protocol,
        cli.config_server_port,
    )
    .await
    .unwrap_or_else(|e| panic!("Failed to start config-server listeners: {e}"));

    let mgr = Arc::new(mgr);

    // 对端连接历史采样（延迟 / 流量趋势），默认 60s 采样、保留 7 天
    let _peer_history_task = peer_history::spawn_peer_history_sampler(
        mgr.clone(),
        db.clone(),
        peer_history::PeerHistoryOptions {
            enabled: cli.peer_history_interval_secs > 0,
            sample_interval: std::time::Duration::from_secs(cli.peer_history_interval_secs.max(1)),
            retention_days: cli.peer_history_retention_days,
        },
    );

    #[cfg(feature = "embed")]
    let (web_router_restful, web_router_static) = if cli.no_web {
        (None, None)
    } else {
        let web_router = web::build_router(cli.api_host.clone());
        if cli.web_server_port.is_none()
            || (cli.web_server_port == Some(cli.api_server_port)
                && cli.web_server_addr == cli.api_server_addr)
        {
            (Some(web_router), None)
        } else {
            (None, Some(web_router))
        }
    };
    #[cfg(not(feature = "embed"))]
    let web_router_restful = None;

    let oidc_config = if cli.oidc.oidc_issuer_url.is_some() {
        match restful::oidc::OidcConfig::from_params(cli.oidc).await {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Failed to initialize OIDC: {:?}", e);
                eprintln!("Please check your OIDC configuration (issuer URL, client ID, etc.)");
                std::process::exit(1);
            }
        }
    } else {
        restful::oidc::OidcConfig::disabled()
    };

    let _restful_server_tasks = restful::RestfulServer::new(
        std::net::SocketAddr::new(cli.api_server_addr, cli.api_server_port),
        mgr.clone(),
        db,
        web_router_restful,
        feature_flags,
        oidc_config,
        webhook_config,
    )
    .await
    .unwrap()
    .start()
    .await
    .unwrap();

    #[cfg(feature = "embed")]
    let _web_server_task = if let Some(web_router) = web_router_static {
        Some(
            web::WebServer::new(
                std::net::SocketAddr::new(cli.web_server_addr, cli.web_server_port.unwrap_or(0)),
                web_router,
            )
            .await
            .unwrap()
            .start()
            .await
            .unwrap(),
        )
    } else {
        None
    };

    tokio::signal::ctrl_c().await.unwrap();
}
