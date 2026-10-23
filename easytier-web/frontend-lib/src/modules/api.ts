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
    /**
     * All storage-backed ids for this machine (console SQLite / GUIStorage).
     * Present on servers that support residual detection; omit → leave residual UI off.
     */
    db_inst_ids?: Array<UUID>,
    /** Storage rows with disabled=true (may still be running). */
    db_disabled_inst_ids?: Array<UUID>,
}

export interface GenerateConfigResponse {
    toml_config?: string;
    error?: string;
}

/** Console Save response (`console-save-and-push-semantics.md` §4.3). */
export type SaveNetworkConfigAction = 'unchanged' | 'patched' | 'rebuilt' | 'pending';
export type SaveNetworkConfigPendingReason = 'disabled' | 'offline' | 'prepare_timeout';
export interface SaveNetworkConfigResponse {
    action: SaveNetworkConfigAction;
    reason?: SaveNetworkConfigPendingReason;
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

/** Mirrors `api_manage.proto` ConfigSource (proto enum int). */
export const ConfigSource = {
    Unspecified: 0,
    User: 1,
    Web: 2,
} as const;
export type ConfigSourceValue = (typeof ConfigSource)[keyof typeof ConfigSource];

/**
 * REST/pbjson serializes proto enums as strings (`ConfigSourceWeb`), while some
 * callers and tests pass the int. Normalize both so UI comparisons stay numeric.
 */
export function normalizeConfigSource(source: unknown): ConfigSourceValue | undefined {
    if (source === undefined || source === null || source === '') {
        return undefined;
    }
    if (typeof source === 'number') {
        if (
            source === ConfigSource.Unspecified
            || source === ConfigSource.User
            || source === ConfigSource.Web
        ) {
            return source;
        }
        return undefined;
    }
    if (typeof source === 'string') {
        const trimmed = source.trim();
        if (/^\d+$/.test(trimmed)) {
            return normalizeConfigSource(Number(trimmed));
        }
        switch (trimmed) {
            case 'ConfigSourceUnspecified':
            case 'CONFIG_SOURCE_UNSPECIFIED':
            case 'Unspecified':
                return ConfigSource.Unspecified;
            case 'ConfigSourceUser':
            case 'CONFIG_SOURCE_USER':
            case 'User':
            case 'user':
                return ConfigSource.User;
            case 'ConfigSourceWeb':
            case 'CONFIG_SOURCE_WEB':
            case 'Web':
            case 'web':
            case 'webhook':
                return ConfigSource.Web;
            default:
                return undefined;
        }
    }
    return undefined;
}

export interface NetworkMeta {
    network_name: string;
    config_permission: ConfigFilePermission.Flags;
    /**
     * Device/DB ownership; missing on older servers → treat as unspecified.
     * Wire format may be proto int or pbjson string (`ConfigSourceWeb`); use
     * {@link normalizeConfigSource} before comparing.
     */
    source?: ConfigSourceValue | number | string;
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

/** 隧道级 IP 流量历史：一个聚合桶 */
export interface FlowHistoryPoint {
    /** 桶起始时间（unix 秒） */
    t: number;
    /** 桶内平均速率（字节/秒），节点侧固定窗口算出，无需差分 */
    rate_bytes_per_sec: number;
    /** 桶内累计计数器最大值（流条目生命周期内，可能因 TTL 淘汰重置） */
    bytes: number;
    /** 掩码视图下合并了几个精确 IP */
    flow_count: number;
    /** 桶内样本数 */
    samples: number;
    /** 桶内是否包含采样估算值（§4.5 采样放大） */
    sampled: boolean;
}

/** 隧道级 IP 流量历史：一条隧道的某个 IP / 方向 */
export interface FlowHistorySeries {
    /** 节点侧 PeerConnId（UUID 字符串，非数字） */
    conn_id: string;
    peer_id: number;
    /** 对端主机名（同窗口内从 peer 汇总关联；拿不到时为空串） */
    peer_hostname: string;
    /** 精确 IP 或掩码网段地址（如 10.0.0.0） */
    ip: string;
    /** 0 = 精确 IP；> 0 = 聚合前缀长度 */
    prefix_len: number;
    /** 0 = tx（目的 IP），1 = rx（源 IP） */
    direction: number;
    /** 最近一次采样时间（unix 秒） */
    last_seen: number;
    points: Array<FlowHistoryPoint>;
}

export interface FlowHistoryResponse {
    /** 聚合桶大小（秒） */
    bucket_seconds: number;
    /** 查询窗口起点 / 终点（unix 秒） */
    from: number;
    to: number;
    /** 曲线数超过上限时被截断（只保留窗口内总流量最大的若干条） */
    truncated: boolean;
    flows: Array<FlowHistorySeries>;
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
    /** Web console returns action/reason; GUI may resolve with void. */
    save_config(config: NetworkConfig): Promise<SaveNetworkConfigResponse | void>;
    get_network_config(inst_id: string): Promise<NetworkConfig>;
    generate_config(config: NetworkConfig): Promise<GenerateConfigResponse>;
    parse_config(toml_config: string): Promise<ParseConfigResponse>;
    get_network_metas(instance_ids: string[]): Promise<GetNetworkMetasResponse>;
    /**
     * Machine-scoped managed config revision (console SQLite).
     * Optional: web console implements it for dirty-draft staleness probes;
     * GUI direct mode typically omits it.
     */
    get_managed_config_revision?(): Promise<string | null | undefined>;
    /**
     * 对端连接历史（延迟 / 丢包 / 抖动 / 流量趋势）。
     *
     * 只有「配置服务器」形态的实现（web 控制台）才有历史表可查，
     * GUI 直连内核时没有，因此这里是可选方法；UI 需自行判空后隐藏入口。
     */
    get_peer_conn_history?(inst_id: string, hours: number): Promise<PeerConnHistoryResponse | undefined>;
    /**
     * 隧道级 IP 流量历史（哪条隧道在跟哪些 IP/网段通信）。
     *
     * 与 `get_peer_conn_history` 一样只在控制台形态可用；节点未开启
     * `flow_stats` 时返回空数组。
     */
    get_flow_history?(inst_id: string, hours: number): Promise<FlowHistoryResponse | undefined>;
    get_logger_level?(): Promise<string>;
    set_logger_level?(level: string): Promise<void>;
    list_log_files?(): Promise<LogFileInfo[]>;
    read_log_file?(fileName: string, maxBytes?: number): Promise<string>;
    get_log_dir?(): Promise<string>;
}
