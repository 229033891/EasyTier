import { type } from '@tauri-apps/plugin-os';

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
    file_log_level: 'off' | 'warn' | 'info' | 'debug' | 'trace'
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
                return { ...mode, mode: 'normal' }
            }
            return mode
        }
        catch (e) {
            console.error('Failed to parse app_mode from localStorage', e)
            localStorage.removeItem('app_mode')
        }
    }
    return { mode: 'normal' }
}

export type Mode = NormalMode | ServiceMode | RemoteMode

/** Normalize service-mode RPC portal to a tcp:// URL without double schemes. */
export function normalizeServiceRpcUrl(portal: string): string {
    let s = portal.trim().replace(/0\.0\.0\.0/g, '127.0.0.1')
    if (!/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(s))
        s = `tcp://${s}`
    return s
}
