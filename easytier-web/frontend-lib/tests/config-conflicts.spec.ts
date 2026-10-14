import { describe, expect, it } from 'vitest'
import {
  advancedFlagConflictHelpKey,
  collectConfigConflictWarnings,
  isAdvancedFlagDisabled,
  isAdvancedFlagHidden,
} from '../src/modules/configConflicts'
import { DEFAULT_NETWORK_CONFIG } from '../src/types/network'

describe('configConflicts', () => {
  it('does not rewrite config and warns on disable_p2p + p2p_only', () => {
    const config = {
      ...DEFAULT_NETWORK_CONFIG(),
      disable_p2p: true,
      p2p_only: true,
    }
    const warnings = collectConfigConflictWarnings(config)
    expect(warnings.some((w) => w.code === 'disable_p2p_conflict')).toBe(true)
    expect(config.disable_p2p).toBe(true)
    expect(config.p2p_only).toBe(true)
  })

  it('blocks enabling dependents when disable_p2p is on, but keeps legacy true editable', () => {
    const clean = {
      ...DEFAULT_NETWORK_CONFIG(),
      disable_p2p: true,
      p2p_only: false,
    }
    expect(isAdvancedFlagDisabled(clean, 'p2p_only')).toBe(true)
    expect(isAdvancedFlagDisabled(clean, 'lazy_p2p')).toBe(true)
    expect(isAdvancedFlagDisabled(clean, 'need_p2p')).toBe(true)
    expect(advancedFlagConflictHelpKey(clean, 'p2p_only')).toBe('disable_p2p_blocks_help')

    const legacy = {
      ...DEFAULT_NETWORK_CONFIG(),
      disable_p2p: true,
      p2p_only: true,
    }
    expect(isAdvancedFlagDisabled(legacy, 'p2p_only')).toBe(false)
  })

  it('hides ipv6_public_addr_auto when disable_ipv6 is on', () => {
    const config = {
      ...DEFAULT_NETWORK_CONFIG(),
      disable_ipv6: true,
      ipv6_public_addr_auto: true,
    }
    expect(isAdvancedFlagHidden(config, 'ipv6_public_addr_auto')).toBe(true)
    expect(collectConfigConflictWarnings(config).some((w) => w.code === 'disable_ipv6_conflict')).toBe(true)
  })

  it('warns when both hole-punching options are disabled', () => {
    const config = {
      ...DEFAULT_NETWORK_CONFIG(),
      disable_tcp_hole_punching: true,
      disable_udp_hole_punching: true,
    }
    expect(collectConfigConflictWarnings(config).some((w) => w.code === 'all_hole_punching_disabled')).toBe(true)
  })

  it('blocks latency_first and relay whitelist under p2p_only when currently off', () => {
    const config = {
      ...DEFAULT_NETWORK_CONFIG(),
      p2p_only: true,
      latency_first: false,
      disable_relay_data: false,
      prefer_peer_relay: false,
      enable_relay_network_whitelist: false,
    }
    expect(isAdvancedFlagDisabled(config, 'latency_first')).toBe(true)
    // 已允许中转（disable_relay_data=false）：p2p_only 下仍可取消勾选
    expect(isAdvancedFlagDisabled(config, 'disable_relay_data')).toBe(false)
    expect(isAdvancedFlagDisabled(config, 'enable_relay_network_whitelist')).toBe(true)
    expect(isAdvancedFlagDisabled(config, 'disable_p2p')).toBe(true)
    expect(advancedFlagConflictHelpKey(config, 'enable_relay_network_whitelist')).toBe(
      'p2p_only_blocks_relay_help',
    )
    // 仍可取消勾选时保留正向说明，不误报「无效」
    expect(advancedFlagConflictHelpKey(config, 'disable_relay_data')).toBe(null)

    // 已禁止中转：p2p_only 下禁止重新勾选「允许作为中转节点」
    config.disable_relay_data = true
    expect(isAdvancedFlagDisabled(config, 'disable_relay_data')).toBe(true)
    expect(advancedFlagConflictHelpKey(config, 'disable_relay_data')).toBe(
      'p2p_only_blocks_relay_help',
    )
  })

  it('hides the whole ipv6 provider group when disable_ipv6 is on', () => {
    const config = {
      ...DEFAULT_NETWORK_CONFIG(),
      disable_ipv6: true,
    }
    expect(isAdvancedFlagHidden(config, 'ipv6_public_addr_auto')).toBe(true)
    expect(isAdvancedFlagHidden(config, 'ipv6_public_addr_provider')).toBe(true)
  })
})
