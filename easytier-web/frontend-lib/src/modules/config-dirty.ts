import {
  emptyDnsConfig,
  normalizeNetworkConfig,
  toBackendNetworkConfig,
  type NetworkConfig,
} from '../types/network'

/** Fields excluded from dirty comparison (volatile / auto-written). */
export const CONFIG_DIRTY_IGNORE_FIELDS = ['dev_name'] as const

export type ConfigDirtyIgnoreField = (typeof CONFIG_DIRTY_IGNORE_FIELDS)[number]

/**
 * Mirror Config.vue `ensureDnsConfig`: the form always injects an empty
 * dns_config on mount. Snapshot must include it or the page looks dirty
 * immediately after load.
 */
export function ensureFormDnsConfig(config: NetworkConfig): NetworkConfig {
  if (!config.dns_config) {
    config.dns_config = emptyDnsConfig()
  }
  config.dns_config.hosts ??= []
  config.dns_config.forwarders ??= []
  config.dns_config.upstream_dns ??= []
  return config
}

/**
 * Canonical JSON snapshot for dirty detection.
 * Uses {@link toBackendNetworkConfig} then strips volatile fields.
 * Returns `null` if serialization throws (malformed draft) — callers fail-dirty.
 */
export function configDirtySnapshot(
  config: NetworkConfig,
  ignoreFields: readonly string[] = CONFIG_DIRTY_IGNORE_FIELDS,
): string | null {
  try {
    const backend = toBackendNetworkConfig(config) as Record<string, unknown>
    for (const field of ignoreFields) {
      delete backend[field]
    }
    return JSON.stringify(backend)
  } catch {
    return null
  }
}

/** Normalize then snapshot — call after load / save / run refresh. */
export function captureNormalizedDirtySnapshot(config: NetworkConfig): {
  config: NetworkConfig
  snapshot: string
} {
  try {
    const normalized = ensureFormDnsConfig(normalizeNetworkConfig(config))
    const snapshot = configDirtySnapshot(normalized)
    if (snapshot !== null) {
      return { config: normalized, snapshot }
    }
    // Serialization failed after normalize — keep form config, force dirty until next clean capture.
    return { config: normalized, snapshot: '\0capture-failed' }
  } catch {
    return { config, snapshot: '\0capture-failed' }
  }
}

export function isConfigSnapshotDirty(
  config: NetworkConfig | undefined | null,
  cleanSnapshot: string | null,
): boolean {
  if (!config || cleanSnapshot === null) {
    return false
  }
  const current = configDirtySnapshot(config)
  // Fail-dirty: unserializable draft is always treated as unsaved.
  if (current === null) {
    return true
  }
  return current !== cleanSnapshot
}
