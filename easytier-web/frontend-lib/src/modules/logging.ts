export interface LogFileInfo {
  fileName: string
  sizeBytes: number
  modifiedMs: number
  active: boolean
}

export interface ClearLogFilesResult {
  cleared: number
  errors: string[]
  dirs: string[]
}

/** Result of reading the logger level (live RPC vs preference-only fallback). */
export interface LoggerLevelState {
  level: string
  /** False when the value is a local preference because RPC was unreachable. */
  live: boolean
}

export interface LoggingSettingsApi {
  getLoggerLevel?: () => Promise<string | LoggerLevelState>
  setLoggerLevel: (level: string) => Promise<void>
  getLogDir?: () => Promise<string>
  listLogFiles?: () => Promise<LogFileInfo[]>
  readLogFile?: (fileName: string, maxBytes?: number) => Promise<string>
  /** Clear local log files (GUI / hosts that expose file access). */
  clearLogFiles?: () => Promise<ClearLogFilesResult | number>
  openLogDir?: () => Promise<void>
  copyLogDir?: () => Promise<void>
  /** Copy arbitrary text (preferred over navigator.clipboard in Tauri). */
  copyText?: (text: string) => Promise<void>
  canOpenLogDir?: boolean
  remoteOnly?: boolean
}

/** Latest N lines from a log viewer buffer (keeps trailing newline behavior simple). */
export function takeLastLogLines(content: string, maxLines = 100): string {
  const normalized = content.replace(/\r\n/g, '\n').replace(/\r/g, '\n')
  if (!normalized.trim())
    return ''
  const lines = normalized.split('\n')
  // Drop a single trailing empty line from a final newline so "100 lines" means 100 records.
  if (lines.length > 0 && lines[lines.length - 1] === '')
    lines.pop()
  return lines.slice(-maxLines).join('\n')
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
