import { type } from '@tauri-apps/plugin-os'

/** Preferred file log level — single source of truth (edited only in Logging dialog). */
export type FileLogLevel = 'off' | 'error' | 'warn' | 'info' | 'debug' | 'trace'

const FILE_LOG_LEVEL_KEY = 'file_log_level'
const FILE_LOG_LEVELS: readonly FileLogLevel[] = ['off', 'error', 'warn', 'info', 'debug', 'trace']
const DEFAULT_FILE_LOG_LEVEL: FileLogLevel = 'warn'

export function parseFileLogLevel(raw: unknown): FileLogLevel {
    if (typeof raw === 'string') {
        const lower = raw.toLowerCase()
        if ((FILE_LOG_LEVELS as readonly string[]).includes(lower))
            return lower as FileLogLevel
        if (lower === 'warning')
            return 'warn'
        if (lower === 'disabled')
            return 'off'
    }
    return DEFAULT_FILE_LOG_LEVEL
}

/** Load the preferred file log level (default warn). */
export function loadFileLogLevel(): FileLogLevel {
    const stored = localStorage.getItem(FILE_LOG_LEVEL_KEY)
    if (stored != null)
        return parseFileLogLevel(stored)
    // Migrate from legacy service-mode field if present.
    try {
        const modeStr = localStorage.getItem('app_mode')
        if (modeStr) {
            const mode = JSON.parse(modeStr) as { mode?: string, file_log_level?: string }
            if (mode.mode === 'service' && mode.file_log_level) {
                const migrated = parseFileLogLevel(mode.file_log_level)
                localStorage.setItem(FILE_LOG_LEVEL_KEY, migrated)
                return migrated
            }
        }
    }
    catch {
        // ignore corrupt app_mode
    }
    return DEFAULT_FILE_LOG_LEVEL
}

export function saveFileLogLevel(level: string): FileLogLevel {
    const parsed = parseFileLogLevel(level)
    localStorage.setItem(FILE_LOG_LEVEL_KEY, parsed)
    return parsed
}

export interface WebClientConfig {
    config_server_url?: string
    /** Require encrypted config-server tunnel when true. */
    secure_mode?: boolean
}

export interface NormalMode extends WebClientConfig {
    mode: 'normal'
    // if not provided will use ring tunnel rpc server
    rpc_portal?: string
    enable_rpc_port_listen?: boolean
    rpc_listen_port?: number
    /** When true, bind 0.0.0.0; otherwise bind 127.0.0.1 (safer default). */
    rpc_listen_all_interfaces?: boolean
}

export interface ServiceMode extends WebClientConfig {
    mode: 'service'
    config_dir: string
    rpc_portal: string
    /** Last level baked into the service install args (not edited in Mode UI). */
    file_log_level: FileLogLevel
    file_log_dir: string
    installed_core_version?: string
}

export interface RemoteMode {
    mode: 'remote'
    remote_rpc_address: string
}

export function saveMode(mode: Mode) {
    localStorage.setItem('app_mode', JSON.stringify(mode))
}


export function loadMode(): Mode {
    const modeStr = localStorage.getItem('app_mode')
    if (modeStr) {
        try {
            const mode = JSON.parse(modeStr) as Mode
            if (type() === 'android') {
                // Android always uses in-process ring RPC; keep a stale tcp portal and
                // "重试" can leave the UI stuck on "无法连接至远程客户端".
                return migrateNormalRpcListenFlag({
                    ...mode,
                    mode: 'normal',
                    rpc_portal: undefined,
                    enable_rpc_port_listen: false,
                    rpc_listen_port: undefined,
                    rpc_listen_all_interfaces: undefined,
                })
            }
            return migrateNormalRpcListenFlag(mode)
        }
        catch (e) {
            console.error('Failed to parse app_mode from localStorage', e)
            localStorage.removeItem('app_mode')
        }
    }
    return { mode: 'normal' }
}

/** Preserve LAN RPC bind when upgrading from builds that only stored `tcp://0.0.0.0:port`. */
function migrateNormalRpcListenFlag(mode: Mode): Mode {
    if (mode.mode !== 'normal')
        return mode
    const portal = mode.rpc_portal?.trim() ?? ''
    const listensAll = /\/\/0\.0\.0\.0(?::|\/|$)/.test(portal)
        || /\/\/\[::\](?::|\/|$)/.test(portal)
    if (listensAll && !mode.rpc_listen_all_interfaces) {
        return { ...mode, rpc_listen_all_interfaces: true }
    }
    return mode
}

export type Mode = NormalMode | ServiceMode | RemoteMode

/** Normalize service-mode RPC portal to a tcp:// URL without double schemes. */
export function normalizeServiceRpcUrl(portal: string): string {
    // Only wildcards are rewritten to loopback: 0.0.0.0 and [::].
    // NOTE: never blanket-replace "::" — that would mangle real IPv6
    // addresses such as tcp://[2001:db8::1]:11010 or ::1.
    let s = portal.trim().replace(/0\.0\.0\.0/g, '127.0.0.1').replace(/\[::\]/g, '127.0.0.1')
    if (!/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(s))
        s = `tcp://${s}`
    return s
}
