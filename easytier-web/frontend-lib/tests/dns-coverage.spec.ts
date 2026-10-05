import { describe, expect, it } from 'vitest'
import {
  DNS_POLICY_MIN_VERSION,
  expectedDnsCoverage,
  isVersionOlderThan,
  platformDnsCapability,
} from '../src/modules/dnsCoverage'

describe('expectedDnsCoverage', () => {
  it('returns off when magic dns disabled', () => {
    expect(
      expectedDnsCoverage({
        enable_magic_dns: false,
        os_type: 'windows',
        easytier_version: '2.7.4',
      }),
    ).toBe('off')
  })

  it('marks versions before dns policy as version_too_old', () => {
    expect(
      expectedDnsCoverage({
        enable_magic_dns: true,
        os_type: 'windows',
        easytier_version: '2.7.3',
      }),
    ).toBe('version_too_old')
    expect(isVersionOlderThan('2.7.3', DNS_POLICY_MIN_VERSION)).toBe(true)
    expect(isVersionOlderThan('2.7.4', DNS_POLICY_MIN_VERSION)).toBe(false)
  })

  it('covers windows / macos / android automatically', () => {
    expect(
      expectedDnsCoverage({
        enable_magic_dns: true,
        os_type: 'windows',
        easytier_version: '2.7.4',
      }),
    ).toBe('covered')
    expect(
      expectedDnsCoverage({
        enable_magic_dns: true,
        os_type: 'macos',
        easytier_version: '2.7.4',
      }),
    ).toBe('covered')
    expect(
      expectedDnsCoverage({
        enable_magic_dns: true,
        os_type: 'android',
        easytier_version: '2.7.4',
      }),
    ).toBe('covered')
  })

  it('requires manual wiring on linux', () => {
    expect(
      expectedDnsCoverage({
        enable_magic_dns: true,
        os_type: 'linux',
        easytier_version: '2.7.4',
      }),
    ).toBe('manual_required')
  })

  it('phase2 magic_dns_os_wired overrides OS matrix', () => {
    expect(
      expectedDnsCoverage({
        enable_magic_dns: true,
        os_type: 'linux',
        easytier_version: '2.7.4',
        magic_dns_os_wired: true,
      }),
    ).toBe('covered')
    expect(
      expectedDnsCoverage({
        enable_magic_dns: true,
        os_type: 'windows',
        easytier_version: '2.7.4',
        magic_dns_os_wired: false,
      }),
    ).toBe('manual_required')
  })

  it('platformDnsCapability ignores enable_magic_dns off gate', () => {
    expect(
      platformDnsCapability({
        os_type: 'linux',
        easytier_version: '2.7.4',
      }),
    ).toBe('manual_required')
  })

  it('marks no_tun and ios as unsupported', () => {
    expect(
      expectedDnsCoverage({
        enable_magic_dns: true,
        os_type: 'windows',
        easytier_version: '2.7.4',
        no_tun: true,
      }),
    ).toBe('unsupported')
    expect(
      expectedDnsCoverage({
        enable_magic_dns: true,
        os_type: 'ios',
        easytier_version: '2.7.4',
      }),
    ).toBe('unsupported')
  })
})
