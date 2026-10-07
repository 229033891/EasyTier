import { invoke } from '@tauri-apps/api/core'
import { Api, NetworkTypes } from 'easytier-frontend-lib'
import { type ConfigSource, normalizeConfigSource } from './config_source'

type NetworkConfig = NetworkTypes.NetworkConfig
type ValidateConfigResponse = Api.ValidateConfigResponse
type ListNetworkInstanceIdResponse = Api.ListNetworkInstanceIdResponse
type GetNetworkMetasResponse = Api.GetNetworkMetasResponse
interface ServiceOptions {
  config_dir: string
  rpc_portal: string
  file_log_level: string
  file_log_dir: string
  config_server?: string
  secure_mode?: boolean
}

export type ServiceStatus = "Running" | "Stopped" | "NotInstalled"

interface StoredGuiConfig {
  config: NetworkConfig
  source: ConfigSource
}

function parseStoredConfigs(raw: string | null): StoredGuiConfig[] {
  let parsed: unknown
  try {
    parsed = JSON.parse(raw || '[]')
  }
  catch (e) {
    console.error('Failed to parse stored network configs', e)
    return []
  }
  if (!Array.isArray(parsed)) {
    return []
  }

  return parsed.flatMap((entry): StoredGuiConfig[] => {
    if (entry && typeof entry === 'object' && 'config' in entry) {
      const { config, source } = entry as {
        config?: NetworkConfig
        source?: unknown
      }
      if (!config) {
        return []
      }
      return [{
        config: NetworkTypes.normalizeNetworkConfig(config),
        source: normalizeConfigSource(source),
      }]
    }
    // legacy: bare NetworkConfig
    if (entry && typeof entry === 'object') {
      try {
        return [{
          config: NetworkTypes.normalizeNetworkConfig(entry as NetworkConfig),
          source: 'legacy',
        }]
      }
      catch {
        return []
      }
    }
    return []
  })
}

export async function parseNetworkConfig(cfg: NetworkConfig) {
  return invoke<string>('parse_network_config', { cfg: NetworkTypes.toBackendNetworkConfig(cfg) })
}

export async function generateNetworkConfig(tomlConfig: string) {
  const config = await invoke<NetworkConfig>('generate_network_config', { tomlConfig })
  return NetworkTypes.normalizeNetworkConfig(config)
}

export async function runNetworkInstance(cfg: NetworkConfig, save: boolean) {
  return invoke('run_network_instance', { cfg: NetworkTypes.toBackendNetworkConfig(cfg), save })
}

export async function collectNetworkInfo(instanceId: string) {
  return await invoke<Api.CollectNetworkInfoResponse>('collect_network_info', { instanceId })
}

export async function getVpnPortalInfo(instanceId: string) {
  const info = await invoke<NetworkTypes.VpnPortalInfo | undefined>('get_vpn_portal_info', { instanceId })
  return info ? NetworkTypes.normalizeVpnPortalInfo(info) : undefined
}

export async function addVpnPortalClient(instanceId: string, client: { name: string, virtual_ip: string, groups: string[] }) {
  return invoke('patch_vpn_portal_clients', { instanceId, action: 'add', name: client.name, virtualIp: client.virtual_ip, groups: client.groups })
}

export async function removeVpnPortalClient(instanceId: string, name: string) {
  return invoke('patch_vpn_portal_clients', { instanceId, action: 'remove', name })
}

export async function clearVpnPortalClients(instanceId: string) {
  return invoke('patch_vpn_portal_clients', { instanceId, action: 'clear' })
}

export async function getLoggingLevel() {
  return await invoke<string>('get_logging_level')
}

export async function setLoggingLevel(level: string) {
  return await invoke('set_logging_level', { level })
}

export interface LogFileInfo {
  fileName: string
  sizeBytes: number
  modifiedMs: number
  active: boolean
}

export async function listLogFiles() {
  return await invoke<LogFileInfo[]>('list_log_files')
}

export async function readLogFile(fileName: string, maxBytes?: number) {
  return await invoke<string>('read_log_file', { fileName, maxBytes })
}

export interface ClearLogFilesResult {
  cleared: number
  errors: string[]
  dirs: string[]
}

/** Delete rotated logs and truncate active easytier.log in GUI + optional extra dirs. */
export async function clearLogFiles(extraDirs?: string[]) {
  return await invoke<ClearLogFilesResult>('clear_log_files', {
    extraDirs: extraDirs?.length ? extraDirs : null,
  })
}

export async function setTunFd(fd: number, instanceId?: string) {
  return await invoke('set_tun_fd', {
    fd,
    instanceId: instanceId ?? null,
  })
}

export async function getEasytierVersion() {
  return await invoke<string>('easytier_version')
}

export async function listNetworkInstanceIds() {
  return await invoke<ListNetworkInstanceIdResponse>('list_network_instance_ids')
}

export async function deleteNetworkInstance(instanceId: string) {
  return await invoke('remove_network_instance', { instanceId })
}

export async function updateNetworkConfigState(instanceId: string, disabled: boolean) {
  return await invoke('update_network_config_state', { instanceId, disabled })
}

export async function saveNetworkConfig(cfg: NetworkConfig) {
  return await invoke('save_network_config', { cfg: NetworkTypes.toBackendNetworkConfig(cfg) })
}

export async function validateConfig(cfg: NetworkConfig) {
  return await invoke<ValidateConfigResponse>('validate_config', { cfg: NetworkTypes.toBackendNetworkConfig(cfg) })
}

export async function getConfig(instanceId: string) {
  const config = await invoke<NetworkConfig>('get_config', { instanceId })
  return NetworkTypes.normalizeNetworkConfig(config)
}

export async function sendConfigs(enabledNetworks: string[]) {
  const networkList = parseStoredConfigs(localStorage.getItem('networkList'))
  return await invoke('load_configs', {
    configs: networkList.map(({ config, source }) => ({
      config: NetworkTypes.toBackendNetworkConfig(config),
      source,
    })),
    enabledNetworks
  })
}

export async function getNetworkMetas(instanceIds: string[]) {
  return await invoke<GetNetworkMetasResponse>('get_network_metas', { instanceIds })
}

export async function initService(opts?: ServiceOptions) {
  return await invoke('init_service', { opts })
}

export async function setServiceStatus(enable: boolean) {
  return await invoke('set_service_status', { enable })
}

export async function getServiceStatus() {
  return await invoke<ServiceStatus>('get_service_status')
}

export async function initRpcConnection(isNormalMode: boolean, url?: string) {
  return await invoke('init_rpc_connection', { isNormalMode, url })
}

export async function isClientRunning() {
  return await invoke<boolean>('is_client_running')
}

export async function initWebClient(url?: string, secureMode?: boolean) {
  return await invoke('init_web_client', { url, secureMode })
}

export async function isWebClientConnected() {
  return await invoke<boolean>('is_web_client_connected')
}

export interface ConfigServerStatus {
  enabled: boolean
  connected: boolean
  lastError: string
}

export async function getConfigServerStatus() {
  return await invoke<ConfigServerStatus>('get_config_server_status')
}
