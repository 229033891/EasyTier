import { describe, expect, it } from 'vitest'
import { ConfigSource, normalizeConfigSource } from '../src/modules/api'

describe('normalizeConfigSource', () => {
  it('passes through known proto ints', () => {
    expect(normalizeConfigSource(ConfigSource.User)).toBe(ConfigSource.User)
    expect(normalizeConfigSource(ConfigSource.Web)).toBe(ConfigSource.Web)
    expect(normalizeConfigSource(ConfigSource.Unspecified)).toBe(ConfigSource.Unspecified)
  })

  it('accepts pbjson string variants used by REST', () => {
    expect(normalizeConfigSource('ConfigSourceWeb')).toBe(ConfigSource.Web)
    expect(normalizeConfigSource('ConfigSourceUser')).toBe(ConfigSource.User)
    expect(normalizeConfigSource('ConfigSourceUnspecified')).toBe(ConfigSource.Unspecified)
  })

  it('accepts numeric strings and short aliases', () => {
    expect(normalizeConfigSource('2')).toBe(ConfigSource.Web)
    expect(normalizeConfigSource('web')).toBe(ConfigSource.Web)
    expect(normalizeConfigSource('user')).toBe(ConfigSource.User)
  })

  it('returns undefined for missing or unknown values', () => {
    expect(normalizeConfigSource(undefined)).toBeUndefined()
    expect(normalizeConfigSource(null)).toBeUndefined()
    expect(normalizeConfigSource('')).toBeUndefined()
    expect(normalizeConfigSource('nope')).toBeUndefined()
    expect(normalizeConfigSource(99)).toBeUndefined()
  })
})
