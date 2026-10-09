import { describe, expect, it } from 'vitest'
import { captureNormalizedDirtySnapshot, isConfigSnapshotDirty } from '../src/modules/config-dirty'
import { DEFAULT_NETWORK_CONFIG, type NetworkConfig } from '../src/types/network'

function base(): NetworkConfig {
  return {
    ...DEFAULT_NETWORK_CONFIG(),
    instance_id: '00000000-0000-0000-0000-000000000001',
    network_name: 'dirty-mesh',
  }
}

describe('dns draft dirty gaps', () => {
  it('blank forwarder row is not dirty (filtered on serialize)', () => {
    const { config, snapshot } = captureNormalizedDirtySnapshot(base())
    config.dns_config!.forwarders.push({ domains: [], servers: [] })
    expect(isConfigSnapshotDirty(config, snapshot)).toBe(false)
  })

  it('forwarder row with a filled side is dirty', () => {
    const { config, snapshot } = captureNormalizedDirtySnapshot(base())
    config.dns_config!.forwarders.push({ domains: ['corp.example.'], servers: [] })
    expect(isConfigSnapshotDirty(config, snapshot)).toBe(true)
  })

  it('blank upstream draft alone is not dirty (filtered on serialize)', () => {
    const { config, snapshot } = captureNormalizedDirtySnapshot(base())
    config.dns_config!.upstream_dns.push('')
    expect(isConfigSnapshotDirty(config, snapshot)).toBe(false)
  })

  it('typed upstream value is dirty', () => {
    const { config, snapshot } = captureNormalizedDirtySnapshot(base())
    config.dns_config!.upstream_dns.push('1.1.1.1')
    expect(isConfigSnapshotDirty(config, snapshot)).toBe(true)
  })
})
