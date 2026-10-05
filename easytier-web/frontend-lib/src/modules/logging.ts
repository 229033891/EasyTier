export interface LogFileInfo {
  fileName: string
  sizeBytes: number
  modifiedMs: number
  active: boolean
}

export interface LoggingSettingsApi {
  getLoggerLevel?: () => Promise<string>
  setLoggerLevel: (level: string) => Promise<void>
  getLogDir?: () => Promise<string>
  listLogFiles?: () => Promise<LogFileInfo[]>
  readLogFile?: (fileName: string, maxBytes?: number) => Promise<string>
  openLogDir?: () => Promise<void>
  copyLogDir?: () => Promise<void>
  canOpenLogDir?: boolean
  remoteOnly?: boolean
}

const LEVEL_MAP: Record<string, string> = {
  DISABLED: 'off',
  ERROR: 'error',
  WARNING: 'warn',
  INFO: 'info',
  DEBUG: 'debug',
  TRACE: 'trace',
  off: 'off',
  error: 'error',
  warn: 'warn',
  info: 'info',
  debug: 'debug',
  trace: 'trace',
}

export function normalizeLoggerLevel(raw: unknown): string {
  if (typeof raw === 'string') {
    const upper = raw.toUpperCase()
    return LEVEL_MAP[upper] ?? LEVEL_MAP[raw.toLowerCase()] ?? 'info'
  }
  if (typeof raw === 'number') {
    return ['off', 'error', 'warn', 'info', 'debug', 'trace'][raw] ?? 'info'
  }
  return 'info'
}

export function loggerLevelToRpc(level: string): string {
  const map: Record<string, string> = {
    off: 'DISABLED',
    error: 'ERROR',
    warn: 'WARNING',
    info: 'INFO',
    debug: 'DEBUG',
    trace: 'TRACE',
  }
  return map[level.toLowerCase()] ?? 'INFO'
}
