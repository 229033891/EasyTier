import { describe, expect, it } from 'vitest'
import {
  normalizeDefaultProtocol,
  normalizeNetworkConfig,
  parseDefaultProtocolList,
  DEFAULT_NETWORK_CONFIG,
} from '../src/types/network'

describe('default_protocol preference CSV (P-AUTO.L1)', () => {
  it('parses legacy single values and empty input', () => {
    expect(parseDefaultProtocolList('tcp')).toEqual(['tcp'])
    expect(parseDefaultProtocolList('UDP')).toEqual(['udp'])
    expect(parseDefaultProtocolList('')).toEqual(['udp', 'tcp'])
    expect(parseDefaultProtocolList(undefined)).toEqual(['udp', 'tcp'])
  })

  it('preserves order, lowercases, and dedupes', () => {
    expect(parseDefaultProtocolList('wss, tcp, QUIC, tcp, udp')).toEqual([
      'wss',
      'tcp',
      'quic',
      'udp',
    ])
  })

  it('drops unknown schemes', () => {
    expect(parseDefaultProtocolList('wg,tcp,ring,wss,faketcp')).toEqual(['wg', 'tcp', 'wss', 'faketcp'])
    expect(parseDefaultProtocolList('ring')).toEqual(['udp', 'tcp'])
  })

  it('normalizes via NetworkConfig round-trip', () => {
    const config = {
      ...DEFAULT_NETWORK_CONFIG(),
      default_protocol: 'WSS, TCP, tcp, quic',
    }
    const normalized = normalizeNetworkConfig(config)
    expect(normalized.default_protocol).toBe('wss,tcp,quic')
    expect(normalizeDefaultProtocol('WSS, TCP')).toBe('wss,tcp')
  })

  it('stable-merges MultiSelect updates without scrambling prior order', () => {
    // Simulate Config.vue setter: keep previous order, append newly selected.
    const prev = parseDefaultProtocolList('wss,tcp')
    const selected = new Set(['tcp', 'wss', 'udp'])
    const kept = prev.filter(scheme => selected.has(scheme))
    const added = (['tcp', 'udp', 'ws', 'wss', 'quic', 'wg', 'faketcp'] as const).filter(
      scheme => selected.has(scheme) && !kept.includes(scheme),
    )
    expect([...kept, ...added]).toEqual(['wss', 'tcp', 'udp'])
  })
})
