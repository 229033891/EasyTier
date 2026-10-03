import { IPv4, IPv6 } from 'ip-num/IPNumber'
import { Ipv4Addr, Ipv4Inet, Ipv6Addr } from '../types/network'

export function ipv4ToString(ip: Ipv4Addr | null | undefined) {
    if (!ip) {
        return ''
    }
    return IPv4.fromNumber(ip.addr ?? 0).toString()
}

export function ipv4InetToString(ip: Ipv4Inet | undefined) {
    if (ip?.address === undefined) {
        return 'undefined'
    }
    return `${ipv4ToString(ip.address)}/${ip.network_length ?? 0}`
}

export function ipv6ToString(ip: Ipv6Addr | null | undefined) {
    if (!ip) {
        return ''
    }
    return IPv6.fromBigInt(
        (BigInt(ip.part1 ?? 0) << BigInt(96))
        + (BigInt(ip.part2 ?? 0) << BigInt(64))
        + (BigInt(ip.part3 ?? 0) << BigInt(32))
        + BigInt(ip.part4 ?? 0),
    ).toString()
}

function toHexString(uint64: bigint, padding = 9): string {
    let hexString = uint64.toString(16);
    while (hexString.length < padding) {
        hexString = '0' + hexString;
    }
    return hexString;
}

function uint32ToUuid(part1: number, part2: number, part3: number, part4: number): string {
    // 将两个 uint64 转换为 16 进制字符串
    const part1Hex = toHexString(BigInt(part1), 8);
    const part2Hex = toHexString(BigInt(part2), 8);
    const part3Hex = toHexString(BigInt(part3), 8);
    const part4Hex = toHexString(BigInt(part4), 8);

    // 构造 UUID 格式字符串
    const uuid = `${part1Hex.substring(0, 8)}-${part2Hex.substring(0, 4)}-${part2Hex.substring(4, 8)}-${part3Hex.substring(0, 4)}-${part3Hex.substring(4, 8)}${part4Hex.substring(0, 12)}`;

    return uuid;
}

export interface UUID {
    part1?: number;
    part2?: number;
    part3?: number;
    part4?: number;
}

export function UuidToStr(uuid: UUID | null | undefined): string {
    if (!uuid) {
        return '';
    }
    return uint32ToUuid(uuid.part1 ?? 0, uuid.part2 ?? 0, uuid.part3 ?? 0, uuid.part4 ?? 0);
}

/** Format machine client_url for list display (host:port); full URL stays in tooltip. */
export function formatClientUrl(clientUrl: string | null | undefined): string {
    if (!clientUrl) {
        return '';
    }
    try {
        const u = new URL(clientUrl);
        if (u.hostname && u.port) {
            return `${u.hostname}:${u.port}`;
        }
        if (u.hostname) {
            return u.hostname;
        }
    } catch {
        // fall through
    }
    return clientUrl;
}

export type ApiErrorKind =
    | 'timeout'
    | 'client_not_found'
    | 'not_found'
    | 'revision_conflict'
    | 'ownership_conflict'
    | 'unauthorized'
    | 'db_error'
    | 'internal_error'
    | 'unknown'

export interface ApiErrorPayload {
    message: string
    code?: string
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
    if (value && typeof value === 'object') {
        return value as Record<string, unknown>
    }
    return undefined
}

function readStringField(record: Record<string, unknown> | undefined, key: string): string | undefined {
    const value = record?.[key]
    return typeof value === 'string' && value.trim() ? value : undefined
}

/** Unwrap axios / Tauri / plain API error bodies into `{ message, code? }`. */
export function extractApiErrorPayload(error: unknown): ApiErrorPayload {
    const axiosData = asRecord(asRecord(error)?.response)?.data
    const candidates: unknown[] = [
        axiosData,
        asRecord(error)?.data,
        error,
    ]

    for (const candidate of candidates) {
        if (typeof candidate === 'string') {
            const trimmed = candidate.trim()
            if (!trimmed) {
                continue
            }
            if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
                try {
                    const parsed = JSON.parse(trimmed) as unknown
                    const record = asRecord(parsed)
                    const message = readStringField(record, 'message')
                    if (message) {
                        return {
                            message,
                            code: readStringField(record, 'code'),
                        }
                    }
                } catch {
                    // keep raw string
                }
            }
            return { message: trimmed }
        }

        const record = asRecord(candidate)
        const message = readStringField(record, 'message')
        if (message) {
            return {
                message,
                code: readStringField(record, 'code'),
            }
        }
    }

    if (error instanceof Error && error.message.trim()) {
        return { message: error.message }
    }

    return { message: String(error ?? '') }
}

export function classifyApiError(payload: ApiErrorPayload): ApiErrorKind {
    const code = (payload.code ?? '').trim().toLowerCase()
    switch (code) {
        case 'rpc_timeout':
            return 'timeout'
        case 'client_not_found':
            return 'client_not_found'
        case 'not_found':
            return 'not_found'
        case 'managed_config_revision_conflict':
            return 'revision_conflict'
        case 'managed_config_ownership_conflict':
            return 'ownership_conflict'
        case 'db_error':
            return 'db_error'
        case 'internal_error':
        case 'auth_error':
            return 'internal_error'
        default:
            break
    }

    const message = payload.message
    if (/timeout|Elapsed\(\(\)\)|deadline has elapsed|timed?\s*out/i.test(message)) {
        return 'timeout'
    }
    if (/client not found/i.test(message)) {
        return 'client_not_found'
    }
    if (/unauthorized|not logged in|no such user/i.test(message)) {
        return 'unauthorized'
    }
    if (/revision conflict|config revision/i.test(message)) {
        return 'revision_conflict'
    }
    if (/ownership conflict/i.test(message)) {
        return 'ownership_conflict'
    }
    return 'unknown'
}

const API_ERROR_I18N_KEYS: Record<Exclude<ApiErrorKind, 'unknown'>, string> = {
    timeout: 'web.device_management.error_timeout',
    client_not_found: 'web.device_management.error_device_offline',
    not_found: 'web.device_management.error_not_found',
    revision_conflict: 'web.device_management.error_revision_conflict',
    ownership_conflict: 'web.device_management.error_ownership_conflict',
    unauthorized: 'web.device_management.error_unauthorized',
    db_error: 'web.device_management.error_db',
    internal_error: 'web.device_management.error_internal',
}

/** Map API/RPC failures to localized, actionable copy for toasts. */
export function formatApiErrorDetail(
    error: unknown,
    t: (key: string) => string,
): string {
    const payload = extractApiErrorPayload(error)
    const kind = classifyApiError(payload)
    if (kind !== 'unknown') {
        return t(API_ERROR_I18N_KEYS[kind])
    }
    return payload.message.trim() || t('web.device_management.error_unknown')
}

export function StrToUuid(uuid: string): UUID {
    const hex = uuid.replace(/-/g, '');
    if (!/^[0-9a-fA-F]{32}$/.test(hex)) {
        throw new Error(`Invalid UUID: ${uuid}`);
    }

    return {
        part1: Number.parseInt(hex.slice(0, 8), 16),
        part2: Number.parseInt(hex.slice(8, 16), 16),
        part3: Number.parseInt(hex.slice(16, 24), 16),
        part4: Number.parseInt(hex.slice(24, 32), 16),
    };
}

export interface Location {
    country: string | undefined;
    city: string | undefined;
    region: string | undefined;
}

export interface DeviceInfo {
    /** Display name shown in the console (alias or reported hostname). */
    hostname: string;
    /** Hostname reported by the client heartbeat / archive. */
    reported_hostname?: string;
    /** Explicit console alias when set; empty/undefined means no alias. */
    display_name?: string;
    public_ip: string;
    running_network_count: number;
    report_time: string;
    easytier_version: string;
    running_network_instances?: Array<string>;
    machine_id: string;
    location: Location | undefined;
    /** True when present in live `list_machines`; false for archive-only offline rows. */
    online?: boolean;
}

/** Archive row from `/api/v1/devices` (offline devices + display aliases). */
export interface DeviceArchiveRow {
    device_id: string;
    hostname: string;
    display_name?: string;
    last_easytier_version?: string;
    last_client_url?: string;
    last_seen_at?: number;
}

export function buildDeviceInfo(device: any): DeviceInfo {
    const runningInstances = device.info?.running_network_instances ?? [];
    const reported = device.info?.hostname ?? '';
    let dev_info: DeviceInfo = {
        hostname: reported,
        reported_hostname: reported,
        public_ip: device.client_url,
        running_network_instances: runningInstances.map((instance: any) => UuidToStr(instance)),
        running_network_count: runningInstances.length,
        report_time: device.info?.report_time,
        easytier_version: device.info?.easytier_version,
        machine_id: UuidToStr(device.info?.machine_id),
        location: device.location,
        online: true,
    };

    return dev_info;
}

function archiveDisplayName(row: DeviceArchiveRow | undefined): string {
    return (row?.display_name ?? '').trim();
}

/**
 * Merge online `list_machines` results with `/devices` archive:
 * - overlay `display_name` onto online devices
 * - append offline archive rows not currently connected
 */
export function mergeDevicesWithArchive(
    online: DeviceInfo[],
    archived: DeviceArchiveRow[] | null | undefined,
): DeviceInfo[] {
    const archiveById = new Map<string, DeviceArchiveRow>();
    for (const row of archived || []) {
        if (row?.device_id) {
            archiveById.set(row.device_id, row);
        }
    }

    const devices: DeviceInfo[] = [];
    const seen = new Set<string>();

    for (const info of online) {
        const machineId = info.machine_id;
        if (machineId) {
            seen.add(machineId);
        }
        const alias = archiveDisplayName(archiveById.get(machineId));
        const reported = info.reported_hostname || info.hostname;
        devices.push({
            ...info,
            reported_hostname: reported,
            display_name: alias || undefined,
            hostname: alias || reported,
            online: true,
        });
    }

    for (const row of archived || []) {
        if (!row?.device_id || seen.has(row.device_id)) {
            continue;
        }
        const reported = row.hostname || row.device_id;
        const alias = archiveDisplayName(row);
        // Skip stub rows created only for rename before any heartbeat (empty hostname).
        if (!row.hostname && !alias) {
            continue;
        }
        devices.push({
            hostname: alias || reported,
            reported_hostname: reported,
            display_name: alias || undefined,
            public_ip: formatClientUrl(row.last_client_url) || row.last_client_url || '',
            running_network_count: 0,
            report_time: row.last_seen_at
                ? new Date(row.last_seen_at * 1000).toLocaleString()
                : '',
            easytier_version: row.last_easytier_version || '',
            running_network_instances: [],
            machine_id: row.device_id,
            location: undefined,
            online: false,
        });
    }

    return devices;
}

// write a class to run a function periodically and can be stopped by calling stop(), use setTimeout to trigger the function
export class PeriodicTask {
    private interval: number;
    private task: (() => Promise<void>) | undefined;
    private timer: any;

    constructor(task: () => Promise<void>, interval: number) {
        this.interval = interval;
        this.task = task;
    }

    _runTaskHelper(nextInterval: number) {
        this.timer = setTimeout(async () => {
            if (this.task) {
                // 页面切到后台时不发请求，避免隐藏标签页积压请求、回到前台时集中爆发
                if (typeof document !== 'undefined' && document.hidden) {
                    this._runTaskHelper(this.interval);
                    return;
                }
                await this.task();
                this._runTaskHelper(this.interval);
            }
        }, nextInterval);
    }

    start() {
        this._runTaskHelper(0);
    }

    stop() {
        this.task = undefined;
        clearTimeout(this.timer);
    }
}
