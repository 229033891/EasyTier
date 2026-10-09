import { UUID } from './utils';
import { NetworkConfig, NetworkInstanceRunningInfo, VpnPortalInfo } from '../types/network';
import type { LogFileInfo } from './logging';

export type { LogFileInfo } from './logging';

export interface ValidateConfigResponse {
    toml_config: string;
}

export interface ListNetworkInstanceIdResponse {
    running_inst_ids: Array<UUID>,
    disabled_inst_ids: Array<UUID>,
}

export interface GenerateConfigResponse {
    toml_config?: string;
    error?: string;
}

export interface ParseConfigResponse {
    config?: NetworkConfig;
    error?: string;
}

export interface CollectNetworkInfoResponse {
    info: {
        map: Record<string, NetworkInstanceRunningInfo | undefined>;
    }
}

export namespace ConfigFilePermission {
    export type Flags = number;
    export const READ_ONLY: Flags = 1 << 0;
    export const NO_DELETE: Flags = 1 << 1;
    export function hasPermission(perm: Flags, flag: Flags): boolean {
        return (perm & flag) === flag;
    }
    export function isRemoveSaveable(perm: Flags): boolean {
        return !hasPermission(perm, NO_DELETE);
    }
    export function isEditable(perm: Flags): boolean {
        return !hasPermission(perm, READ_ONLY);
    }
    export function isDeletable(perm: Flags): boolean {
        return !hasPermission(perm, NO_DELETE);
    }
}

export interface NetworkMeta {
    network_name: string;
    config_permission: ConfigFilePermission.Flags;
}

export interface GetNetworkMetasResponse {
    metas: Record<string, NetworkMeta>;
}

/** 对端连接历史：一个聚合桶 */
export interface PeerConnHistoryPoint {
    /** 桶起始时间（unix 秒） */
    t: number;
    /** 桶内平均延迟（微秒）；拿不到延迟时为 null */
    latency_us: number | null;
    /** 桶内平均丢包率；拿不到时为 null */
    loss_rate: number | null;
    /** 桶内平均抖动（微秒）；拿不到或旧后端未返回时为 null/缺省 */
    jitter_us?: number | null;
    /** 桶内累计计数器最大值（不是速率，速率需对相邻桶差分） */
    rx_bytes: number;
    tx_bytes: number;
    /** 桶内样本数 */
    samples: number;
}

/** 对端连接历史：一个对端 peer 的曲线 */
export interface PeerConnHistorySeries {
    peer_id: number;
    hostname: string;
    remote_addr: string;
    tunnel_type: string;
    /** 最近一次采样时间（unix 秒） */
    last_seen: number;
    points: Array<PeerConnHistoryPoint>;
}

export interface PeerConnHistoryResponse {
    /** 聚合桶大小（秒） */
    bucket_seconds: number;
    /** 查询窗口起点 / 终点（unix 秒） */
    from: number;
    to: number;
    peers: Array<PeerConnHistorySeries>;
}

export interface RemoteClient {
    validate_config(config: NetworkConfig): Promise<ValidateConfigResponse>;
    run_network(config: NetworkConfig, save: boolean): Promise<undefined>;
    get_network_info(inst_id: string): Promise<NetworkInstanceRunningInfo | undefined>;
    /** Batch collect running info for multiple instances (one RPC round-trip). */
    get_network_infos?(inst_ids: string[]): Promise<Record<string, NetworkInstanceRunningInfo | undefined>>;
    get_vpn_portal_info(inst_id: string): Promise<VpnPortalInfo | undefined>;
    add_vpn_portal_client(inst_id: string, client: { name: string, virtual_ip: string, groups: string[] }): Promise<undefined>;
    remove_vpn_portal_client(inst_id: string, name: string): Promise<undefined>;
    clear_vpn_portal_clients(inst_id: string): Promise<undefined>;
    list_network_instance_ids(): Promise<ListNetworkInstanceIdResponse>;
    delete_network(inst_id: string): Promise<undefined>;
    update_network_instance_state(inst_id: string, disabled: boolean): Promise<undefined>;
    save_config(config: NetworkConfig): Promise<undefined>;
    get_network_config(inst_id: string): Promise<NetworkConfig>;
    generate_config(config: NetworkConfig): Promise<GenerateConfigResponse>;
    parse_config(toml_config: string): Promise<ParseConfigResponse>;
    get_network_metas(instance_ids: string[]): Promise<GetNetworkMetasResponse>;
    /**
     * 对端连接历史（延迟 / 丢包 / 抖动 / 流量趋势）。
     *
     * 只有「配置服务器」形态的实现（web 控制台）才有历史表可查，
     * GUI 直连内核时没有，因此这里是可选方法；UI 需自行判空后隐藏入口。
     */
    get_peer_conn_history?(inst_id: string, hours: number): Promise<PeerConnHistoryResponse | undefined>;
    get_logger_level?(): Promise<string>;
    set_logger_level?(level: string): Promise<void>;
    list_log_files?(): Promise<LogFileInfo[]>;
    read_log_file?(fileName: string, maxBytes?: number): Promise<string>;
    get_log_dir?(): Promise<string>;
}
