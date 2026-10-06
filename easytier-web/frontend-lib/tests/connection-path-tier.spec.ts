import { describe, expect, it } from 'vitest'
import {
  ConnectionPathTier,
  DEFAULT_NETWORK_CONFIG,
  applyConnectionPathTier,
  inferConnectionPathTier,
  normalizeNetworkConfig,
} from '../src/types/network'

describe('connection path tier', () => {
  it('infers relay-only from disable_p2p', () => {
    const config = { ...DEFAULT_NETWORK_CONFIG(), disable_p2p: true }
    expect(inferConnectionPathTier(config)).toBe(ConnectionPathTier.RELAY_ONLY)
  })

  it('apply relay-only clears p2p_only and sets legacy flags', () => {
    const config = applyConnectionPathTier(
      { ...DEFAULT_NETWORK_CONFIG(), p2p_only: true },
      ConnectionPathTier.RELAY_ONLY,
    )
    expect(config.disable_p2p).toBe(true)
    expect(config.prefer_peer_relay).toBe(true)
    expect(config.p2p_only).toBe(false)
    expect(config.connection_path_tier).toBe(ConnectionPathTier.RELAY_ONLY)
  })

  it('normalize fills tier from legacy flags when unset', () => {
    const config = normalizeNetworkConfig({
      ...DEFAULT_NETWORK_CONFIG(),
      connection_path_tier: ConnectionPathTier.UNSPECIFIED,
      prefer_peer_relay: true,
    })
    expect(config.connection_path_tier).toBe(ConnectionPathTier.PREFER_RELAY)
  })

  it('normalize keeps an explicit tier and only warns on conflict', () => {
    const config = normalizeNetworkConfig({
      ...DEFAULT_NETWORK_CONFIG(),
      connection_path_tier: ConnectionPathTier.DIRECT_FIRST,
      disable_p2p: true,
      p2p_only: true,
    })
    // Explicit SoT wins the display; conflicting legacy flags are kept
    // for tip-only warnings (never silently rewritten on load).
    expect(config.connection_path_tier).toBe(ConnectionPathTier.DIRECT_FIRST)
    expect(config.disable_p2p).toBe(true)
    expect(config.p2p_only).toBe(true)
  })
})
