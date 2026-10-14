import type { NetworkTypes } from 'easytier-frontend-lib'
import type { ConfigSource } from './config_source'

export interface StoredGuiConfig {
  config: NetworkTypes.NetworkConfig
  source: ConfigSource
  /** Last seen running-instance fingerprint (Service/Remote console-sync adopt). */
  last_observed_runtime_fp?: string
}

/**
 * Drop console-owned (`web`) configs absent from keep-set **W**.
 *
 * `consoleWebOwnedInstanceIds` must be the console SQLite Web-row set for this
 * machine (enabled + disabled). Do not pass device running ids — cold-start
 * `disabled` from GUIStorage is empty and would mis-delete stopped web drafts.
 * User/legacy drafts are kept. See `docs/roadmap/local-config-residue.md` §4.1.
 */
export function pruneAbsentWebConfigs(
  networkList: StoredGuiConfig[],
  consoleWebOwnedInstanceIds: ReadonlySet<string> | readonly string[],
): StoredGuiConfig[] {
  const known = consoleWebOwnedInstanceIds instanceof Set
    ? consoleWebOwnedInstanceIds
    : new Set(consoleWebOwnedInstanceIds)
  return networkList.filter(({ config, source }) => {
    const id = config.instance_id
    if (!id) {
      return false
    }
    if (source === 'web' && !known.has(id)) {
      return false
    }
    return true
  })
}
