// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod elevate;

use anyhow::Context;
#[cfg(target_os = "android")]
use easytier::instance::factory::subscribe_native_instance_event;
use easytier::proto::api::config::{
    ConfigPatchAction, ConfigRpc, ConfigRpcClientFactory, InstanceConfigPatch, PatchConfigRequest,
    VpnPortalClientPatch,
};
use easytier::proto::api::instance::{
    GetVpnPortalInfoRequest, InstanceIdentifier, VpnPortalInfo, VpnPortalRpc,
    VpnPortalRpcClientFactory, instance_identifier,
};
use easytier::proto::api::manage::{
    CollectNetworkInfoResponse, GetConfigServerStatusRequest, ValidateConfigResponse,
    VpnPortalClientConfig, WebClientService, WebClientServiceClientFactory,
};
use easytier::proto::rpc_types::controller::BaseController;
use easytier::web_client::{self, WebClient};
use easytier::{
    common::config::{NetworkConfig, NetworkConfigExt},
    common::{
        config::{ConfigLoader, ConfigSource, FileLoggerConfig, LoggingConfig, TomlConfigLoader},
        log,
    },
    instance::factory::{NativeInstanceManager, native_instance_manager},
    proto::rpc::standalone::{runtime_rpc_dialer, runtime_rpc_listener},
    rpc_service::ApiRpcServer,
    utils::panic::setup_panic_handler,
};
use easytier_core::management::config_source_to_rpc;
use easytier_core::management::remote_client::{
    GetNetworkMetasResponse, ListNetworkInstanceIdsJsonResp, ListNetworkProps, RemoteClientManager,
    Storage,
};
use easytier_core::{
    connectivity::protocol::raw::TunnelDialer as _, process_runtime::CoreProcessRuntime,
    socket::SocketListener, tunnel::Tunnel,
};
use std::ops::Deref;
use std::sync::{Arc, LazyLock};
use tokio::sync::{Mutex, RwLock, RwLockReadGuard};
use uuid::Uuid;

use tauri::{AppHandle, Emitter, Manager as _};

#[cfg(not(target_os = "android"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

static INSTANCE_MANAGER: LazyLock<RwLock<Option<Arc<NativeInstanceManager>>>> =
    LazyLock::new(|| RwLock::new(None));

static RPC_RING_UUID: LazyLock<uuid::Uuid> = LazyLock::new(uuid::Uuid::new_v4);

static CLIENT_MANAGER: LazyLock<RwLock<Option<manager::GUIClientManager>>> =
    LazyLock::new(|| RwLock::new(None));

type BoxedTunnelListener = Box<dyn SocketListener<Accepted = Box<dyn Tunnel>>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RpcServerKind {
    Ring,
    Tcp,
}

struct RpcServer {
    kind: RpcServerKind,
    _server: ApiRpcServer<BoxedTunnelListener>,
    bind_url: Option<url::Url>,
}
static RPC_SERVER: LazyLock<Mutex<Option<RpcServer>>> =
    LazyLock::new(|| Mutex::new(None));

static WEB_CLIENT: LazyLock<RwLock<Option<WebClient>>> =
    LazyLock::new(|| RwLock::new(None));

macro_rules! get_client_manager {
    () => {{
        let guard = CLIENT_MANAGER
            .try_read()
            .map_err(|_| "Failed to acquire read lock for client manager")?;
        RwLockReadGuard::try_map(guard, |cm| cm.as_ref())
            .map_err(|_| "RPC connection not initialized".to_string())
    }};
}

#[tauri::command]
fn easytier_version() -> Result<String, String> {
    Ok(easytier::VERSION.to_string())
}

#[tauri::command]
fn set_dock_visibility(app: tauri::AppHandle, visible: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use tauri::ActivationPolicy;
        app.set_activation_policy(if visible {
            ActivationPolicy::Regular
        } else {
            ActivationPolicy::Accessory
        })
        .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, visible);
    Ok(())
}

#[tauri::command]
fn parse_network_config(cfg: NetworkConfig) -> Result<String, String> {
    let toml = cfg.gen_config().map_err(|e| e.to_string())?;
    Ok(toml.dump())
}

#[tauri::command]
fn generate_network_config(toml_config: String) -> Result<NetworkConfig, String> {
    let config = TomlConfigLoader::new_from_str(&toml_config).map_err(|e| e.to_string())?;
    let cfg = NetworkConfig::new_from_config(&config).map_err(|e| e.to_string())?;
    Ok(cfg)
}

#[tauri::command]
async fn run_network_instance(
    app: AppHandle,
    cfg: NetworkConfig,
    save: bool,
) -> Result<(), String> {
    let client_manager = get_client_manager!()?;
    let toml_config = cfg.gen_config().map_err(|e| e.to_string())?;
    client_manager
        .pre_run_network_instance_hook(&app, &toml_config, manager::PersistedConfigSource::User)
        .await?;
    client_manager
        .handle_run_network_instance(app.clone(), cfg, save)
        .await
        .map_err(|e| e.to_string())?;
    client_manager
        .post_run_network_instance_hook(&app, &toml_config.get_id())
        .await?;
    Ok(())
}

#[tauri::command]
async fn collect_network_info(
    app: AppHandle,
    instance_id: String,
) -> Result<CollectNetworkInfoResponse, String> {
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;
    get_client_manager!()?
        .handle_collect_network_info(app, Some(vec![instance_id]))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_vpn_portal_info(instance_id: String) -> Result<Option<VpnPortalInfo>, String> {
    let instance_id = instance_id
        .parse::<uuid::Uuid>()
        .map_err(|e| e.to_string())?;
    let client_manager = get_client_manager!()?;
    let client = client_manager
        .rpc_manager
        .rpc_client()
        .scoped_client::<VpnPortalRpcClientFactory<BaseController>>(1, 1, "".to_string());
    let response = client
        .get_vpn_portal_info(
            BaseController::default(),
            GetVpnPortalInfoRequest {
                instance: Some(InstanceIdentifier {
                    selector: Some(instance_identifier::Selector::Id(instance_id.into())),
                }),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(response.vpn_portal_info)
}

#[tauri::command]
async fn patch_vpn_portal_clients(
    instance_id: String,
    action: String,
    name: Option<String>,
    virtual_ip: Option<String>,
    groups: Option<Vec<String>>,
) -> Result<(), String> {
    let instance_id = instance_id
        .parse::<uuid::Uuid>()
        .map_err(|e| e.to_string())?;
    let action = match action.as_str() {
        "add" => ConfigPatchAction::Add,
        "remove" => ConfigPatchAction::Remove,
        "clear" => ConfigPatchAction::Clear,
        other => return Err(format!("invalid vpn portal client patch action: {other}")),
    };
    let client = if action == ConfigPatchAction::Clear {
        None
    } else {
        Some(VpnPortalClientConfig {
            name: name.unwrap_or_default(),
            virtual_ip: virtual_ip.unwrap_or_default(),
            groups: groups.unwrap_or_default(),
        })
    };

    let client_manager = get_client_manager!()?;
    let rpc = client_manager
        .rpc_manager
        .rpc_client()
        .scoped_client::<ConfigRpcClientFactory<BaseController>>(1, 1, "".to_string());
    rpc.patch_config(
        BaseController::default(),
        PatchConfigRequest {
            instance: Some(InstanceIdentifier {
                selector: Some(instance_identifier::Selector::Id(instance_id.into())),
            }),
            patch: Some(InstanceConfigPatch {
                vpn_portal_clients: vec![VpnPortalClientPatch {
                    action: action as i32,
                    client,
                }],
                ..Default::default()
            }),
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn set_logging_level(level: String) -> Result<(), String> {
    get_client_manager!()?
        .set_logging_level(level.clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn set_tun_fd(fd: i32) -> Result<(), String> {
    let Some(instance_manager) = INSTANCE_MANAGER.read().await.clone() else {
        return Err("set_tun_fd is not supported in remote mode".to_string());
    };
    if let Some(uuid) = get_client_manager!()?
        .get_enabled_instances_with_tun_ids()
        .next()
    {
        instance_manager
            .attach_tun_fd(uuid, fd)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn list_network_instance_ids(
    app: AppHandle,
) -> Result<ListNetworkInstanceIdsJsonResp, String> {
    get_client_manager!()?
        .handle_list_network_instance_ids(app)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn remove_network_instance(app: AppHandle, instance_id: String) -> Result<(), String> {
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;
    let client_manager = get_client_manager!()?;
    client_manager
        .handle_remove_network_instances(app.clone(), vec![instance_id])
        .await
        .map_err(|e| e.to_string())?;
    client_manager
        .post_stop_network_instances_hook(&app)
        .await?;

    Ok(())
}

#[tauri::command]
async fn update_network_config_state(
    app: AppHandle,
    instance_id: String,
    disabled: bool,
) -> Result<(), String> {
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;
    let client_manager = get_client_manager!()?;
    if !disabled {
        let (cfg, source) = client_manager
            .handle_get_network_config_with_source(app.clone(), instance_id)
            .await
            .map_err(|e| e.to_string())?;
        let toml_config = cfg.gen_config().map_err(|e| e.to_string())?;
        client_manager
            .pre_run_network_instance_hook(
                &app,
                &toml_config,
                manager::PersistedConfigSource::from_runtime_source(source),
            )
            .await?;
    }
    client_manager
        .handle_update_network_state(app.clone(), instance_id, disabled)
        .await
        .map_err(|e| e.to_string())?;

    if disabled {
        client_manager
            .post_stop_network_instances_hook(&app)
            .await?;
    } else {
        client_manager
            .post_run_network_instance_hook(&app, &instance_id)
            .await?;
    }

    Ok(())
}

#[tauri::command]
async fn save_network_config(app: AppHandle, cfg: NetworkConfig) -> Result<(), String> {
    let instance_id = cfg
        .instance_id()
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;
    get_client_manager!()?
        .handle_save_network_config(app, instance_id, cfg)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn validate_config(
    app: AppHandle,
    config: NetworkConfig,
) -> Result<ValidateConfigResponse, String> {
    get_client_manager!()?
        .handle_validate_config(app, config)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_config(app: AppHandle, instance_id: String) -> Result<NetworkConfig, String> {
    let instance_id = instance_id
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;
    let cfg = get_client_manager!()?
        .handle_get_network_config(app, instance_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(cfg)
}

#[tauri::command]
async fn load_configs(
    app: AppHandle,
    configs: Vec<manager::StoredGuiConfig>,
    enabled_networks: Vec<String>,
) -> Result<(), String> {
    get_client_manager!()?
        .load_configs(app, configs, enabled_networks)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn get_network_metas(
    app: AppHandle,
    instance_ids: Vec<uuid::Uuid>,
) -> Result<GetNetworkMetasResponse, String> {
    get_client_manager!()?
        .handle_get_network_metas(app, instance_ids)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "android")]
#[tauri::command]
fn init_service() -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
fn init_service(opts: Option<service::ServiceOptions>) -> Result<(), String> {
    match opts {
        Some(args) => {
            let path = std::path::Path::new(&args.config_dir);
            if !path.exists() {
                std::fs::create_dir_all(&args.config_dir).map_err(|e| e.to_string())?;
            } else if !path.is_dir() {
                return Err("config_dir exists but is not a directory".to_string());
            }
            let path = std::path::Path::new(&args.file_log_dir);
            if !path.exists() {
                std::fs::create_dir_all(&args.file_log_dir).map_err(|e| e.to_string())?;
            } else if !path.is_dir() {
                return Err("file_log_dir exists but is not a directory".to_string());
            }

            service::install(args).map_err(|e| format!("{:#}", e))?;
        }
        None => {
            service::uninstall().map_err(|e| format!("{:#}", e))?;
        }
    }
    Ok(())
}

#[tauri::command]
fn set_service_status(_enable: bool) -> Result<(), String> {
    #[cfg(not(target_os = "android"))]
    {
        service::set_status(_enable).map_err(|e| format!("{:#}", e))?;
    }
    Ok(())
}

#[tauri::command]
fn get_service_status() -> Result<&'static str, String> {
    #[cfg(not(target_os = "android"))]
    {
        use easytier::service_manager::ServiceStatus;
        let status = service::status().map_err(|e| format!("{:#}", e))?;
        match status {
            ServiceStatus::NotInstalled => Ok("NotInstalled"),
            ServiceStatus::Stopped(_) => Ok("Stopped"),
            ServiceStatus::Running => Ok("Running"),
        }
    }
    #[cfg(target_os = "android")]
    {
        Ok("NotInstalled")
    }
}

/// 建立 RPC 客户端的预算。ring 是进程内直连；给了地址就是真的网络拨号
/// （TCP 连接 + RPC 建链，见 runtime_rpc_dialer），公网与移动网络下 1 秒必然超时，
/// 所以按目标区分：本机回环只需要够本地握手，跨网络要给足 RTT 余量。
const RPC_CONNECT_TIMEOUT_RING: std::time::Duration = std::time::Duration::from_secs(2);
const RPC_CONNECT_TIMEOUT_LOOPBACK: std::time::Duration = std::time::Duration::from_secs(3);
const RPC_CONNECT_TIMEOUT_NETWORK: std::time::Duration = std::time::Duration::from_secs(10);

/// 目标是否指向本机（localhost / 127.0.0.0/8 / ::1）。
fn is_loopback_rpc_target(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(addr)) => addr.is_loopback(),
        Some(url::Host::Ipv6(addr)) => addr.is_loopback(),
        // Non-special schemes (tcp/ws/...) may keep dotted IPs as Domain.
        Some(url::Host::Domain(host)) => host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|addr| addr.is_loopback()),
        None => false,
    }
}

/// `None` = ring（进程内），`Some` = 网络拨号。
fn rpc_connect_budget(rpc_url: Option<&url::Url>) -> std::time::Duration {
    match rpc_url {
        None => RPC_CONNECT_TIMEOUT_RING,
        Some(url) if is_loopback_rpc_target(url) => RPC_CONNECT_TIMEOUT_LOOPBACK,
        Some(_) => RPC_CONNECT_TIMEOUT_NETWORK,
    }
}

/// 失败信息里要写清真正尝试的目标，否则用户只看到 "timed out" 无从下手。
fn rpc_target_label(rpc_url: Option<&url::Url>) -> String {
    match rpc_url {
        Some(url) => url.to_string(),
        None => format!("ring://{}", *RPC_RING_UUID.deref()),
    }
}

#[cfg(test)]
mod rpc_connect_budget_tests {
    use super::{
        RPC_CONNECT_TIMEOUT_LOOPBACK, RPC_CONNECT_TIMEOUT_NETWORK, RPC_CONNECT_TIMEOUT_RING,
        is_loopback_rpc_target, rpc_connect_budget,
    };

    fn parse_url(target: &str) -> url::Url {
        target.parse().unwrap()
    }

    #[test]
    fn ring_uses_process_local_budget() {
        assert_eq!(rpc_connect_budget(None), RPC_CONNECT_TIMEOUT_RING);
    }

    #[test]
    fn loopback_targets_use_local_budget() {
        for target in [
            "tcp://127.0.0.1:15888",
            "tcp://localhost:15888",
            "ws://[::1]:15888",
        ] {
            let target_url = parse_url(target);
            assert!(is_loopback_rpc_target(&target_url), "{target}");
            assert_eq!(
                rpc_connect_budget(Some(&target_url)),
                RPC_CONNECT_TIMEOUT_LOOPBACK,
                "{target}"
            );
        }
    }

    #[test]
    fn remote_targets_use_network_budget() {
        for target in [
            "tcp://10.0.0.5:15888",
            "tcp://example.com:15888",
            "tcp://[2001:db8::1]:15888",
        ] {
            let target_url = parse_url(target);
            assert!(!is_loopback_rpc_target(&target_url), "{target}");
            assert_eq!(
                rpc_connect_budget(Some(&target_url)),
                RPC_CONNECT_TIMEOUT_NETWORK,
                "{target}"
            );
        }
    }
}

fn normalize_normal_mode_rpc_portal(portal: &str) -> Result<(url::Url, url::Url), String> {
    let portal_url: url::Url = portal
        .parse()
        .map_err(|e| format!("invalid rpc portal: {:#}", e))?;
    let bind_url = portal_url.clone();
    let mut connect_url = portal_url.clone();
    // if bind addr is 0.0.0.0, should convert to 127.0.0.1
    if connect_url.host_str() == Some("0.0.0.0") {
        connect_url.set_host(Some("127.0.0.1")).unwrap();
    }
    Ok((bind_url, connect_url))
}

async fn resolve_rpc_bind_url(url: &url::Url) -> Result<std::net::SocketAddr, String> {
    if url.scheme() != "tcp" {
        return Err(format!("RPC portal requires tcp URL: {url}"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| format!("RPC portal has no host: {url}"))?;
    let port = url.port().unwrap_or(11010);
    tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("failed to resolve RPC portal {url}: {error}"))?
        .next()
        .ok_or_else(|| format!("RPC portal has no resolved address: {url}"))
}

/// Dropping StandAloneServer aborts its JoinSet asynchronously; the ring registry
/// entry can briefly remain until the aborted task drops the listener.
async fn bind_ring_tunnel_with_retry(
    process_runtime: &easytier_core::process_runtime::CoreProcessRuntime,
    ring_id: uuid::Uuid,
) -> Result<BoxedTunnelListener, String> {
    let mut last_error = None;
    for attempt in 0..20 {
        if attempt > 0 {
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        match process_runtime.bind_ring_tunnel(ring_id) {
            Ok(tunnel) => return Ok(tunnel),
            Err(error) => {
                let message = error.to_string();
                let retryable = message.contains("already registered");
                last_error = Some(message);
                if !retryable {
                    break;
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| "failed to bind ring RPC tunnel".to_string()))
}

#[tauri::command]
async fn init_rpc_connection(
    _app: AppHandle,
    is_normal_mode: bool,
    url: Option<String>,
) -> Result<(), String> {
    let mut client_manager_guard =
        tokio::time::timeout(std::time::Duration::from_secs(5), CLIENT_MANAGER.write())
            .await
            .map_err(|_| "Failed to acquire write lock for client manager")?;
    let mut instance_manager_guard = INSTANCE_MANAGER
        .try_write()
        .map_err(|_| "Failed to acquire write lock for instance manager")?;
    let mut rpc_server_guard = RPC_SERVER
        .try_lock()
        .map_err(|_| "Failed to acquire lock for rpc server")?;

    let mut client_url = url.clone();
    let mut local_process_runtime = None;
    if is_normal_mode {
        let instance_manager = if let Some(im) = instance_manager_guard.take() {
            im
        } else {
            Arc::new(native_instance_manager())
        };

        let portal = url.and_then(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });

        let (desired_kind, bind_url, connect_url) = if let Some(portal) = portal {
            let (bind_url, connect_url) = normalize_normal_mode_rpc_portal(&portal)?;
            (RpcServerKind::Tcp, Some(bind_url), Some(connect_url))
        } else {
            (RpcServerKind::Ring, None, None)
        };

        // Only rebuild the RPC server when transport identity changes. If the GUI
        // client tunnel dies but the server is still listening, recreate the client
        // only — forcing a ring rebind races StandAloneServer's JoinSet abort and
        // fails with "ring listener already registered".
        let need_restart = rpc_server_guard
            .as_ref()
            .map(|x| x.kind != desired_kind || x.bind_url != bind_url)
            .unwrap_or(true);

        if need_restart {
            *rpc_server_guard = None;
            // Drop the dead client before rebinding so the old tunnel is gone.
            *client_manager_guard = None;

            let tunnel: BoxedTunnelListener = match desired_kind {
                RpcServerKind::Ring => bind_ring_tunnel_with_retry(
                    instance_manager.process_runtime().as_ref(),
                    *RPC_RING_UUID.deref(),
                )
                .await?,
                RpcServerKind::Tcp => {
                    let bind_url = bind_url.as_ref().expect("tcp rpc must have bind url");
                    Box::new(runtime_rpc_listener(resolve_rpc_bind_url(bind_url).await?))
                }
            };

            // IP whitelist only applies to TCP portals. Ring tunnels use ring://uuid
            // (no IP host); ManagementRpcServerHook would reject every client and leave
            // the GUI stuck on "无法连接至远程客户端". Match main: no whitelist for ring.
            let mut rpc_server = ApiRpcServer::from_tunnel(tunnel, instance_manager.clone())
                .with_rx_timeout(None);
            if desired_kind == RpcServerKind::Tcp {
                let allow_lan = bind_url
                    .as_ref()
                    .and_then(|u| u.host_str())
                    .is_some_and(|h| h == "0.0.0.0" || h == "::");
                rpc_server = rpc_server.with_localhost_or_lan_whitelist(allow_lan);
            }
            let rpc_server = rpc_server.serve().await.map_err(|e| e.to_string())?;
            *rpc_server_guard = Some(RpcServer {
                kind: desired_kind,
                _server: rpc_server,
                bind_url,
            });
        }

        local_process_runtime = Some(instance_manager.process_runtime());
        *instance_manager_guard = Some(instance_manager);
        client_url = connect_url.map(|u| u.to_string());
    } else {
        *rpc_server_guard = None;
    }

    // 预算和报错都要先知道目标是谁；client_url 之后会被 move 进 GUIClientManager。
    let dial_url = client_url
        .as_deref()
        .map(str::parse::<url::Url>)
        .transpose()
        .map_err(|e| format!("invalid rpc url {client_url:?}: {e}"))?;
    let dial_target = rpc_target_label(dial_url.as_ref());
    let connect_budget = rpc_connect_budget(dial_url.as_ref());

    let client_manager = tokio::time::timeout(
        connect_budget,
        manager::GUIClientManager::new(client_url, local_process_runtime),
    )
    .await
    .map_err(|_| {
        format!(
            "connect {dial_target} timed out after {}s",
            connect_budget.as_secs()
        )
    })?
    .with_context(|| format!("Failed to connect remote rpc {dial_target}"))
    .map_err(|e| format!("{:#}", e))?;
    *client_manager_guard = Some(client_manager);

    if !is_normal_mode {
        drop(WEB_CLIENT.write().await.take());
        if let Some(instance_manager) = instance_manager_guard.take() {
            instance_manager
                .retain_network_instances(&[])
                .await
                .map_err(|e| e.to_string())?;
            drop(instance_manager);
        }
    }

    Ok(())
}

#[tauri::command]
async fn is_client_running() -> Result<bool, String> {
    Ok(get_client_manager!()?.rpc_manager.is_running())
}

#[tauri::command]
async fn init_web_client(
    app: AppHandle,
    url: Option<String>,
    secure_mode: Option<bool>,
) -> Result<(), String> {
    let mut web_client_guard = WEB_CLIENT.write().await;
    let Some(url) = url else {
        *web_client_guard = None;
        easytier_core::management::clear_config_server_status();
        return Ok(());
    };
    let instance_manager = INSTANCE_MANAGER
        .try_read()
        .map_err(|_| "Failed to acquire read lock for instance manager")?
        .clone()
        .ok_or_else(|| "Instance manager is not available".to_string())?;

    let hooks = Arc::new(manager::GuiHooks { app: app.clone() });
    let machine_id_state_dir = app
        .path()
        .app_data_dir()
        .with_context(|| "Failed to resolve machine id state directory")
        .map_err(|e| format!("{:#}", e))?;

    let web_client = web_client::run_web_client(
        url.as_str(),
        easytier::common::MachineIdOptions {
            explicit_machine_id: None,
            state_dir: Some(machine_id_state_dir),
        },
        None,
        secure_mode.unwrap_or(false),
        instance_manager,
        Some(hooks),
    )
    .await
    .with_context(|| "Failed to initialize web client")
    .map_err(|e| format!("{:#}", e))?;
    *web_client_guard = Some(web_client);
    Ok(())
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigServerStatusDto {
    enabled: bool,
    connected: bool,
    last_error: String,
}

#[tauri::command]
async fn get_config_server_status(app: AppHandle) -> Result<ConfigServerStatusDto, String> {
    // Normal mode: WebClient lives in the GUI process.
    {
        let web_client_guard = WEB_CLIENT.read().await;
        if let Some(web_client) = web_client_guard.as_ref() {
            let status = easytier_core::management::config_server_status();
            return Ok(ConfigServerStatusDto {
                enabled: true,
                connected: web_client.is_connected(),
                last_error: status.last_error.unwrap_or_default(),
            });
        }
    }

    // Service / remote mode: query the process that owns the WebClient via RPC.
    let client_manager = get_client_manager!()?;
    let Some(client) = client_manager.get_rpc_client(app) else {
        return Ok(ConfigServerStatusDto {
            enabled: false,
            connected: false,
            last_error: String::new(),
        });
    };
    let response = client
        .get_config_server_status(BaseController::default(), GetConfigServerStatusRequest {})
        .await
        .map_err(|e| e.to_string())?;
    Ok(ConfigServerStatusDto {
        enabled: response.enabled,
        connected: response.connected,
        last_error: response.last_error,
    })
}

#[tauri::command]
async fn is_web_client_connected() -> Result<bool, String> {
    let web_client_guard = WEB_CLIENT.read().await;
    if let Some(web_client) = web_client_guard.as_ref() {
        Ok(web_client.is_connected())
    } else {
        Ok(false)
    }
}

// 获取日志目录的辅助函数
fn get_log_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, tauri::Error> {
    if cfg!(target_os = "android") {
        // Android: cache_dir + logs 子目录
        app.path().cache_dir().map(|p| p.join("logs"))
    } else {
        // 其他平台: 默认日志目录
        app.path().app_log_dir()
    }
}

#[tauri::command]
async fn get_log_dir_path(app: tauri::AppHandle) -> Result<String, String> {
    match get_log_dir(&app) {
        Ok(log_dir) => {
            std::fs::create_dir_all(&log_dir).ok();
            Ok(log_dir.to_string_lossy().to_string())
        }
        Err(e) => Err(format!("Failed to get log directory: {}", e)),
    }
}

#[cfg(not(target_os = "android"))]
fn toggle_window_visibility(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or_default();
        let minimized = window.is_minimized().unwrap_or_default();
        let focused = window.is_focused().unwrap_or_default();

        let should_show = !visible || minimized || !focused;
        if should_show {
            if !visible {
                let _ = window.show();
            }
            if minimized {
                let _ = window.unminimize();
            }
            if !focused {
                let _ = window.set_focus();
            }
            let _ = set_dock_visibility(app.clone(), true);
        } else {
            let _ = window.hide();
            let _ = set_dock_visibility(app.clone(), false);
        }
    }
}

fn get_exe_path() -> String {
    if let Ok(appimage_path) = std::env::var("APPIMAGE")
        && !appimage_path.is_empty()
    {
        return appimage_path;
    }
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(not(target_os = "android"))]
fn check_sudo() -> bool {
    let is_elevated = elevate::Command::is_elevated();
    if !is_elevated {
        let exe_path = get_exe_path();
        let stdcmd = std::process::Command::new(&exe_path);
        elevate::Command::new(stdcmd)
            .output()
            .expect("Failed to run elevated command");
    }
    is_elevated
}

mod manager;

#[cfg(not(target_os = "android"))]
mod service;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run_gui() -> std::process::ExitCode {
    #[cfg(not(target_os = "android"))]
    if !check_sudo() {
        use std::process;
        process::exit(0);
    }

    setup_panic_handler();

    let mut builder = tauri::Builder::default();

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            app.webview_windows()
                .values()
                .next()
                .expect("Sorry, no window found")
                .set_focus()
                .expect("Can't Bring Window to Focus");
        }));
    }

    builder = builder
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_vpnservice::init());

    let app = builder
        .setup(|app| {
            // for logging config
            let Ok(log_dir) = get_log_dir(app.app_handle()) else {
                return Ok(());
            };
            let config = LoggingConfig::builder()
                .file_logger(FileLoggerConfig {
                    dir: Some(log_dir.to_string_lossy().to_string()),
                    // Default warn so startup/RPC failures are visible without
                    // requiring the user to open Settings → Logging first.
                    level: Some("warn".to_string()),
                    file: None,
                    size_mb: None,
                    count: None,
                })
                .build();
            let Ok(_) = log::init(&config, true) else {
                return Ok(());
            };

            // for tray icon, menu need to be built in js
            #[cfg(not(target_os = "android"))]
            let _tray_menu = TrayIconBuilder::with_id("main")
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        toggle_window_visibility(app);
                    }
                })
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/icon.png"
                ))?)
                .icon_as_template(true)
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            parse_network_config,
            generate_network_config,
            run_network_instance,
            collect_network_info,
            get_vpn_portal_info,
            patch_vpn_portal_clients,
            set_logging_level,
            set_tun_fd,
            easytier_version,
            set_dock_visibility,
            list_network_instance_ids,
            remove_network_instance,
            update_network_config_state,
            save_network_config,
            validate_config,
            get_config,
            load_configs,
            get_network_metas,
            init_service,
            set_service_status,
            get_service_status,
            init_rpc_connection,
            is_client_running,
            init_web_client,
            is_web_client_connected,
            get_config_server_status,
            get_log_dir_path,
        ])
        .on_window_event(|_win, event| match event {
            #[cfg(not(target_os = "android"))]
            tauri::WindowEvent::CloseRequested { api, .. } => {
                let _ = _win.hide();
                let _ = set_dock_visibility(_win.app_handle().clone(), false);
                api.prevent_close();
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .unwrap();

    app.run(|_app, _event| {});

    std::process::ExitCode::SUCCESS
}

pub fn run_cli() -> std::process::ExitCode {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { easytier::core::main().await })
}
