import type { Api, NetworkTypes } from 'easytier-frontend-lib'
import { invoke } from '@tauri-apps/api/core'
import { type } from '@tauri-apps/plugin-os'
import * as backend from '~/composables/backend'
import { annotateNetworkInfoFromVpnService } from '~/composables/mobile_vpn'

export class GUIRemoteClient implements Api.RemoteClient {
  async validate_config(config: NetworkTypes.NetworkConfig): Promise<Api.ValidateConfigResponse> {
    return backend.validateConfig(config)
  }

  async run_network(config: NetworkTypes.NetworkConfig, save: boolean): Promise<undefined> {
    await backend.runNetworkInstance(config, save)
  }

  async get_network_info(inst_id: string): Promise<NetworkTypes.NetworkInstanceRunningInfo | undefined> {
    const info = (await backend.collectNetworkInfo(inst_id)).info?.map?.[inst_id]
    if (!info)
      return undefined
    // Android DummyIfConfiger never updates L2 sync — fill from VpnService.
    if (type() === 'android')
      return await annotateNetworkInfoFromVpnService(info, inst_id)
    return info
  }

  async get_vpn_portal_info(inst_id: string): Promise<NetworkTypes.VpnPortalInfo | undefined> {
    return backend.getVpnPortalInfo(inst_id)
  }

  async add_vpn_portal_client(inst_id: string, client: { name: string, virtual_ip: string, groups: string[] }): Promise<undefined> {
    await backend.addVpnPortalClient(inst_id, client)
  }

  async remove_vpn_portal_client(inst_id: string, name: string): Promise<undefined> {
    await backend.removeVpnPortalClient(inst_id, name)
  }

  async clear_vpn_portal_clients(inst_id: string): Promise<undefined> {
    await backend.clearVpnPortalClients(inst_id)
  }

  async list_network_instance_ids(): Promise<Api.ListNetworkInstanceIdResponse> {
    return backend.listNetworkInstanceIds()
  }

  async delete_network(inst_id: string): Promise<undefined> {
    await backend.deleteNetworkInstance(inst_id)
  }

  async update_network_instance_state(inst_id: string, disabled: boolean): Promise<undefined> {
    await backend.updateNetworkConfigState(inst_id, disabled)
  }

  async save_config(config: NetworkTypes.NetworkConfig): Promise<undefined> {
    await backend.saveNetworkConfig(config)
  }

  async get_network_config(inst_id: string): Promise<NetworkTypes.NetworkConfig> {
    return backend.getConfig(inst_id)
  }

  async generate_config(config: NetworkTypes.NetworkConfig): Promise<Api.GenerateConfigResponse> {
    try {
      return { toml_config: await backend.parseNetworkConfig(config) }
    }
    catch (e) {
      return { error: `${e}` }
    }
  }

  async parse_config(toml_config: string): Promise<Api.ParseConfigResponse> {
    try {
      return { config: await backend.generateNetworkConfig(toml_config) }
    }
    catch (e) {
      return { error: `${e}` }
    }
  }

  async get_network_metas(instance_ids: string[]): Promise<Api.GetNetworkMetasResponse> {
    return await backend.getNetworkMetas(instance_ids)
  }

  async get_logger_level(): Promise<string> {
    return backend.getLoggingLevel()
  }

  async set_logger_level(level: string): Promise<void> {
    await backend.setLoggingLevel(level)
  }

  async list_log_files(): Promise<Api.LogFileInfo[]> {
    return backend.listLogFiles()
  }

  async read_log_file(fileName: string, maxBytes?: number): Promise<string> {
    return backend.readLogFile(fileName, maxBytes)
  }

  async get_log_dir(): Promise<string> {
    return invoke<string>('get_log_dir_path')
  }
}
