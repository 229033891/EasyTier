import type { StoredGuiConfig } from './prune_web_configs'

/** Proto ConfigSource.Web — keep in sync with `ConfigResidual.isMislabeledFormerWebGhost`. */
const WEB_SOURCE = 2

/**
 * Device meta may use proto int or pbjson string (`ConfigSourceWeb`).
 * Keep this free of `easytier-frontend-lib` so unit tests stay package-entry free.
 */
function metaSourceIsWeb(source: unknown): boolean {
  return source === WEB_SOURCE
    || source === 'ConfigSourceWeb'
    || source === 'CONFIG_SOURCE_WEB'
    || source === 'Web'
    || source === 'web'
    || source === 'webhook'
    || source === '2'
}

/** Candidates for P1.2 prompt: local user/legacy, absent from console W. */
export function findFormerWebGhostCandidates(
  networkList: StoredGuiConfig[],
  consoleWebOwnedInstanceIds: ReadonlySet<string> | readonly string[],
): StoredGuiConfig[] {
  const w = consoleWebOwnedInstanceIds instanceof Set
    ? consoleWebOwnedInstanceIds
    : new Set(consoleWebOwnedInstanceIds)
  return networkList.filter(({ config, source }) => {
    const id = config.instance_id
    if (!id) {
      return false
    }
    if (source !== 'user' && source !== 'legacy') {
      return false
    }
    return !w.has(id)
  })
}

/**
 * Keep ids where device meta still says Web while console W has no row —
 * never treat plain User drafts (meta User / missing) as ghosts.
 *
 * Predicate mirrors `ConfigResidual.isMislabeledFormerWebGhost` (frontend-lib)
 * so GUI unit tests stay free of the built package entry.
 */
export function filterProvenFormerWebGhosts(
  candidates: StoredGuiConfig[],
  metas: Record<string, { source?: number | string } | undefined>,
  consoleWebOwnedIds: ReadonlySet<string> | readonly string[] = [],
): string[] {
  const w = consoleWebOwnedIds instanceof Set
    ? consoleWebOwnedIds
    : new Set(consoleWebOwnedIds)
  const ids: string[] = []
  for (const { config, source } of candidates) {
    const id = config.instance_id
    if (!id) {
      continue
    }
    if (source !== 'user' && source !== 'legacy') {
      continue
    }
    if (w.has(id)) {
      continue
    }
    if (metaSourceIsWeb(metas[id]?.source)) {
      ids.push(id)
    }
  }
  return ids
}

export function dropStoredConfigsByIds(
  networkList: StoredGuiConfig[],
  ids: ReadonlySet<string>,
): StoredGuiConfig[] {
  return networkList.filter(({ config }) => {
    const id = config.instance_id
    return !!id && !ids.has(id)
  })
}
