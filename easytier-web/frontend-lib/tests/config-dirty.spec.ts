import { describe, expect, it, vi } from 'vitest'
import {
  CONFIG_DIRTY_IGNORE_FIELDS,
  captureNormalizedDirtySnapshot,
  configDirtySnapshot,
  ensureFormDnsConfig,
  isConfigSnapshotDirty,
} from '../src/modules/config-dirty'
import * as NetworkModule from '../src/types/network'
import { DEFAULT_NETWORK_CONFIG, type NetworkConfig } from '../src/types/network'

function baseConfig(overrides: Partial<NetworkConfig> = {}): NetworkConfig {
  return {
    ...DEFAULT_NETWORK_CONFIG(),
    instance_id: '00000000-0000-0000-0000-000000000001',
    network_name: 'dirty-mesh',
    ...overrides,
  }
}

describe('configDirtySnapshot', () => {
  it('treats identical configs as clean after normalize', () => {
    const { config, snapshot } = captureNormalizedDirtySnapshot(baseConfig({ dhcp: true }))
    expect(isConfigSnapshotDirty(config, snapshot)).toBe(false)
  })

  it('flags real field changes as dirty', () => {
    const { config, snapshot } = captureNormalizedDirtySnapshot(baseConfig({ network_name: 'a' }))
    config.network_name = 'b'
    expect(isConfigSnapshotDirty(config, snapshot)).toBe(true)
  })

  it('ignores dev_name changes', () => {
    const { config, snapshot } = captureNormalizedDirtySnapshot(
      baseConfig({ dev_name: '' }),
    )
    config.dev_name = 'et0'
    expect(CONFIG_DIRTY_IGNORE_FIELDS).toContain('dev_name')
    expect(isConfigSnapshotDirty(config, snapshot)).toBe(false)
    expect(configDirtySnapshot(config)).toBe(snapshot)
  })

  it('fail-dirties when snapshot serialization throws', () => {
    const { config, snapshot } = captureNormalizedDirtySnapshot(baseConfig())
    expect(isConfigSnapshotDirty(config, snapshot)).toBe(false)

    const spy = vi.spyOn(NetworkModule, 'toBackendNetworkConfig').mockImplementation(() => {
      throw new Error('malformed')
    })
    try {
      expect(configDirtySnapshot(config)).toBeNull()
      expect(isConfigSnapshotDirty(config, snapshot)).toBe(true)
    } finally {
      spy.mockRestore()
    }
  })

  it('returns false when snapshot is missing', () => {
    expect(isConfigSnapshotDirty(baseConfig(), null)).toBe(false)
    expect(isConfigSnapshotDirty(undefined, 'x')).toBe(false)
  })

  it('baseline includes form-injected empty dns_config (no false dirty)', () => {
    const raw = baseConfig()
    delete (raw as { dns_config?: unknown }).dns_config
    const { config, snapshot } = captureNormalizedDirtySnapshot(raw)
    expect(config.dns_config).toBeTruthy()
    // Simulate Config.vue ensureDnsConfig on the same object
    ensureFormDnsConfig(config)
    expect(isConfigSnapshotDirty(config, snapshot)).toBe(false)
  })
})
