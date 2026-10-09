import { describe, expect, it } from 'vitest'
import { captureNormalizedDirtySnapshot, isConfigSnapshotDirty } from '../src/modules/config-dirty'
import { DEFAULT_NETWORK_CONFIG, addDnsHostRow, type NetworkConfig } from '../src/types/network'

function baseConfig(): NetworkConfig {
  return {
    ...DEFAULT_NETWORK_CONFIG(),
    instance_id: '00000000-0000-0000-0000-000000000001',
    network_name: 'dirty-mesh',
  }
}

describe('dns host dirty', () => {
  it('blank host draft alone is not dirty (filtered on serialize)', () => {
    const { config, snapshot } = captureNormalizedDirtySnapshot(baseConfig())
    addDnsHostRow(config.dns_config!.hosts)
    expect(isConfigSnapshotDirty(config, snapshot)).toBe(false)
  })

  it('marks dirty when host name or ips are set', () => {
    const { config, snapshot } = captureNormalizedDirtySnapshot(baseConfig())
    addDnsHostRow(config.dns_config!.hosts)
    config.dns_config!.hosts[0].ips = ['10.1.2.3']
    expect(isConfigSnapshotDirty(config, snapshot)).toBe(true)

    config.dns_config!.hosts[0].name = 'app.internal.'
    expect(isConfigSnapshotDirty(config, snapshot)).toBe(true)
  })
})
