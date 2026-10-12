/**
 * Console residual classification (local-config-residue.md §5 P1).
 *
 * Requires list enrichment: `db_inst_ids` + `db_disabled_inst_ids`. Without those
 * fields (older servers), callers must leave residual UI off — never infer from R alone.
 */

export type ResidualKind = 'device_only' | 'db_disabled_running'

export interface ResidualListSets {
  runningIds: ReadonlySet<string>
  /** All storage-backed ids (enabled + disabled). */
  dbInstIds: ReadonlySet<string>
  /** Storage rows with disabled=true (may intersect running). */
  dbDisabledInstIds: ReadonlySet<string>
}

/** Classify one instance id; `null` when not a residual. */
export function classifyResidual(
  instanceId: string,
  sets: ResidualListSets,
): ResidualKind | null {
  if (!instanceId || !sets.runningIds.has(instanceId)) {
    return null
  }
  if (!sets.dbInstIds.has(instanceId)) {
    return 'device_only'
  }
  if (sets.dbDisabledInstIds.has(instanceId)) {
    return 'db_disabled_running'
  }
  return null
}

const IGNORE_PREFIX = 'easytier.ignoredResiduals.'

export function ignoredResidualStorageKey(scope: string): string {
  return `${IGNORE_PREFIX}${scope}`
}

export function loadIgnoredResiduals(scope: string | undefined): Set<string> {
  if (!scope || typeof localStorage === 'undefined') {
    return new Set()
  }
  try {
    const raw = localStorage.getItem(ignoredResidualStorageKey(scope))
    if (!raw) {
      return new Set()
    }
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) {
      return new Set()
    }
    return new Set(parsed.filter((x): x is string => typeof x === 'string' && !!x))
  }
  catch {
    return new Set()
  }
}

export function persistIgnoredResiduals(scope: string | undefined, ids: ReadonlySet<string>): void {
  if (!scope || typeof localStorage === 'undefined') {
    return
  }
  localStorage.setItem(ignoredResidualStorageKey(scope), JSON.stringify([...ids]))
}

/**
 * P2: after Save returns pending+disabled, auto-Start only if the device was
 * already running that id (avoid starting brand-new / intentionally stopped rows).
 */
export function shouldAutoStartAfterDisabledSave(
  result: { action?: string, reason?: string } | void | null | undefined,
  wasRunning: boolean,
): boolean {
  return !!wasRunning
    && result?.action === 'pending'
    && result?.reason === 'disabled'
}

/**
 * GUI P1.2: local `user`/`legacy` row that device meta still reports as Web,
 * while console W no longer lists it — former Web ghost, prompt (never silent-delete).
 */
export function isMislabeledFormerWebGhost(opts: {
  localSource: string | undefined
  instanceId: string | undefined
  consoleWebOwnedIds: ReadonlySet<string>
  deviceMetaSource: number | undefined
  /** Proto ConfigSource.Web = 2 */
  webSourceValue?: number
}): boolean {
  const {
    localSource,
    instanceId,
    consoleWebOwnedIds,
    deviceMetaSource,
    webSourceValue = 2,
  } = opts
  if (!instanceId) {
    return false
  }
  if (localSource !== 'user' && localSource !== 'legacy') {
    return false
  }
  if (consoleWebOwnedIds.has(instanceId)) {
    return false
  }
  return deviceMetaSource === webSourceValue
}
