import type { ServiceStatus } from './backend'
import type { Mode } from './mode'

/**
 * Tray run state machine.
 *
 * The tray icon answers one question — *"am I on a network right now?"* — not
 * "is the client process alive". The icon being present already says the app is
 * running, so colour is spent on the network, not on the process (see
 * docs/current/... §tray). `connecting` is applied by the connecting hold in
 * `tray.ts` (`beginTrayConnecting` / `endTrayConnecting`); this module only ever
 * returns settled states.
 *
 * Pure on purpose: no Tauri imports, so the aggregation rules are unit-testable.
 */
export type TrayRunState = 'idle' | 'connecting' | 'running' | 'error'

/** Per-instance snapshot. `running` undefined means "probe failed" — not "stopped". */
export interface TrayInstanceSnapshot {
  instanceId: string
  running?: boolean
  errorMsg?: string
}

export interface TrayRunStateInput {
  /** L0: the GUI's RPC client to the core/service is reachable. */
  clientRunning: boolean
  /**
   * Only meaningful when `clientRunning` is false: is that an actionable fault
   * (red) or a deliberate/normal "nothing running" (grey)?
   */
  clientFailureIsFault: boolean
  /**
   * Instances from the device instance list. `undefined` = list unavailable;
   * callers must then keep the previous icon instead of inventing a grey one.
   */
  instances?: TrayInstanceSnapshot[]
}

export type TrayErrorKind = 'client' | 'instance'

export interface TrayRunStateResult {
  state: TrayRunState
  /** Instances treated as up. Unknown snapshots count — never report grey on a doubt. */
  runningCount: number
  /** Instances that reported a start error. */
  failedCount: number
  /** Set when `state === 'error'`, so the tooltip can name the cause. */
  errorKind?: TrayErrorKind
}

/**
 * Is an unreachable client something the user must act on?
 *
 * - `normal`: the GUI owns its core, so an unreachable client is a real fault.
 * - `remote`: nothing local can be managed while the peer is down — check the
 *   address / link (the tooltip names the remote cause, the colour stays red).
 * - `service`: `Stopped` / `NotInstalled` is a state the tray menu itself can
 *   produce, so it stays grey; only a `Running` service that cannot be reached
 *   is a fault.
 */
export function isRunFault(mode: Mode['mode'], serviceStatus?: ServiceStatus): boolean {
  if (mode === 'remote')
    return true
  if (mode === 'service')
    return serviceStatus === 'Running'
  return true
}

export function deriveTrayRunState(input: TrayRunStateInput): TrayRunStateResult {
  const instances = input.instances ?? []
  const failedCount = instances.filter(i => i.running === false && !!i.errorMsg?.trim()).length
  const runningCount = instances.filter(i => i.running !== false).length

  // L0 wins: an unreachable core says nothing trustworthy about instances.
  if (!input.clientRunning) {
    return input.clientFailureIsFault
      ? { state: 'error', runningCount, failedCount, errorKind: 'client' }
      : { state: 'idle', runningCount, failedCount }
  }

  // A start failure is the only signal that surfaces it (the instance stays in
  // the device list), so it outranks a healthy sibling.
  if (failedCount)
    return { state: 'error', runningCount, failedCount, errorKind: 'instance' }

  if (runningCount)
    return { state: 'running', runningCount, failedCount: 0 }

  return { state: 'idle', runningCount: 0, failedCount: 0 }
}

/**
 * Refresh hook so state-changing API calls (start/stop/save/delete) can push a
 * tray update without `index.vue` handing its refresher to every caller.
 */
let refresher: (() => void) | null = null

export function registerTrayRunStateRefresher(fn: (() => void) | null) {
  refresher = fn
}

export function requestTrayRunStateRefresh() {
  refresher?.()
}
