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

/// Default file-log directory when `--file-log-dir` is omitted.
const DEFAULT_FILE_LOG_DIR: &str = "logs";
/// Default file-log level when file logging is enabled.
const DEFAULT_FILE_LOG_LEVEL: &str = "warn";

/// Probe that the process can create/write/delete under `dir`.
///
/// The probe file name is per-process (pid + nanos): several `easytier-web`
/// instances may share one log volume, and a fixed name lets one process delete
/// the probe file another is still writing, which would be misread as
/// "directory not writable" and silently drop file logging.
fn probe_writable(dir: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write as _;
    let unique = format!(
        ".easytier-log-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let probe = dir.join(unique);
    let written = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&probe)?;
        f.write_all(b"probe")?;
        drop(f);
        Ok::<(), std::io::Error>(())
    })();
    // A concurrent clean-up of a stale probe is not a writability failure.
    match std::fs::remove_file(&probe) {
        Ok(()) => written,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => written,
        Err(e) if written.is_ok() => Err(e),
        Err(_) => written,
    }
}

/// Resolve file logger config: default `./logs` + `warn`, with writable probe.
/// Unwritable dir → `level=off`, so `FileSink::from_config` keeps the sink
/// closed instead of falling back to the cwd `easytier.log` default.
fn resolve_file_logger_config(cli: &Cli) -> FileLoggerConfig {
    let level = cli
        .file_log_level
        .clone()
        .unwrap_or_else(|| DEFAULT_FILE_LOG_LEVEL.to_string());
    if level.eq_ignore_ascii_case("off") {
        return FileLoggerConfig {
            dir: None,
            level: Some(level),
            ..Default::default()
        };
    }
    let dir = cli
        .file_log_dir
        .clone()
        .unwrap_or_else(|| DEFAULT_FILE_LOG_DIR.to_string());
    let writable = std::fs::create_dir_all(&dir)
        .and_then(|_| probe_writable(std::path::Path::new(&dir)))
        .is_ok();
    if !writable {
        eprintln!(
            "easytier-web: file log dir '{dir}' not writable, falling back to in-memory ring only"
        );
        return FileLoggerConfig {
            dir: None,
            level: Some("off".to_string()),
            ..Default::default()
        };
    }
    FileLoggerConfig {
        dir: Some(dir),
        level: Some(level),
        ..Default::default()
    }
}

struct WebLoggingConfig {
    console: ConsoleLoggerConfig,
    file: FileLoggerConfig,
}

impl LoggingConfigLoader for &WebLoggingConfig {
    fn get_console_logger_config(&self) -> ConsoleLoggerConfig {
        self.console.clone()
    }

    fn get_file_logger_config(&self) -> FileLoggerConfig {
        self.file.clone()
    }
}

#[cfg(test)]
mod file_log_resolve_tests {
    use super::*;

    fn cli_with(dir: Option<&str>, level: Option<&str>) -> Cli {
        Cli {
            file_log_dir: dir.map(|s| s.to_string()),
            file_log_level: level.map(|s| s.to_string()),
            ..Cli::parse_from(["easytier-web"])
        }
    }

    #[test]
    fn resolve_defaults_to_logs_warn_when_writable() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        let cli = cli_with(Some(dir.to_str().unwrap()), None);
        let cfg = resolve_file_logger_config(&cli);
        assert_eq!(cfg.dir.as_deref(), Some(dir.to_str().unwrap()));
        assert_eq!(cfg.level.as_deref(), Some("warn"));
    }

    #[test]
    fn resolve_off_disables_file_logging() {
        let cli = cli_with(Some("/tmp/whatever"), Some("off"));
        let cfg = resolve_file_logger_config(&cli);
        assert!(cfg.dir.is_none());
        assert_eq!(cfg.level.as_deref(), Some("off"));
    }

    #[test]
    fn resolve_unwritable_dir_falls_back_to_off() {
        // Use a regular file as the log directory instead of permissions:
        // `create_dir_all` then fails for every user (including root, which
        // ignores directory mode bits), making the test deterministic.
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let dir = blocker.join("logs");

        let cli = cli_with(Some(dir.to_str().unwrap()), Some("warn"));
        let cfg = resolve_file_logger_config(&cli);
        assert!(cfg.dir.is_none());
        assert_eq!(cfg.level.as_deref(), Some("off"));
    }

    #[test]
    fn resolve_probe_tolerates_concurrent_stale_probe_cleanup() {
        // A pre-existing probe file from another process must not make an
        // otherwise writable directory look unwritable.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".easytier-log-probe-999-1"), b"stale").unwrap();
        assert!(probe_writable(tmp.path()).is_ok());
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
    let file_logger = resolve_file_logger_config(&cli);
    let logging = WebLoggingConfig {
        console: ConsoleLoggerConfig {
            level: cli.console_log_level.clone(),
        },
        file: file_logger.clone(),
    };
    // A probe that passes can still race the real open (disk full, `easytier.log`
    // created as a directory, permission change). Never abort startup for logs:
    // retry once with file logging disabled, keeping console + memory ring.
    let file_logging_active = match log::init_with_default_console_targets(
        &logging,
        false,
        &["CORE", "easytier_web"],
    ) {
        Ok(()) => file_logger.dir.is_some(),
        Err(error) => {
            eprintln!(
                "easytier-web: file logging init failed ({error:#}); retrying with console + in-memory ring only"
            );
            let fallback = WebLoggingConfig {
                console: logging.console.clone(),
                file: FileLoggerConfig {
                    dir: None,
                    level: Some("off".to_string()),
                    ..Default::default()
                },
            };
            log::init_with_default_console_targets(&fallback, false, &["CORE", "easytier_web"])
                .expect("console-only logging must be initializable");
            false
        }
    };
    let _ = log::enable_memory_buffer(1000);
    if !file_logging_active
        && !cli
            .file_log_level
            .as_deref()
            .is_some_and(|l| l.eq_ignore_ascii_case("off"))
    {
        tracing::warn!(
            "file logging disabled (directory not writable or unavailable); using in-memory ring only"
        );
    }
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
        file_log_dir = ?file_logger.dir,
        file_log_level = ?file_logger.level,
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

    let oidc_configured = cli.oidc.oidc_issuer_url.is_some();
    let oidc_config = if oidc_configured {
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

    let diagnostics = Arc::new(restful::DiagnosticsState {
        intent: restful::RuntimeIntentSnapshot {
            version: EASYTIER_VERSION.to_string(),
            api_listen: format!("{}:{}", cli.api_server_addr, cli.api_server_port),
            config_server_port: cli.config_server_port,
            config_server_protocol: cli.config_server_protocol.clone(),
            heartbeat_min_response_ms: cli.heartbeat_min_response_ms,
            heartbeat_timeout_ms: cli.heartbeat_timeout_ms,
            db_path: cli.db.clone(),
            console_log_level: cli.console_log_level.clone(),
            // Live sink only — not the pre-init probe result (init may have fallen back).
            file_log_dir: log::file_log_config().map(|(dir, _)| dir.display().to_string()),
            webhook_configured: webhook_config
                .webhook_url
                .as_deref()
                .is_some_and(|url| !url.trim().is_empty()),
            oidc_configured,
        },
        client_mgr: mgr.clone(),
        db: db.clone(),
    });

    let _restful_server_tasks = restful::RestfulServer::new(
        std::net::SocketAddr::new(cli.api_server_addr, cli.api_server_port),
        mgr.clone(),
        db,
        web_router_restful,
        feature_flags,
        oidc_config,
        webhook_config,
        diagnostics,
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
