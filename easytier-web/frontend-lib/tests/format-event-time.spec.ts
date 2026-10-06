import { describe, expect, it } from 'vitest'
import { formatEventTime, localizeLogTimestamps, parseEventTimeMs } from '../src/modules/utils'

describe('parseEventTimeMs / formatEventTime', () => {
  it('parses unix seconds and milliseconds', () => {
    expect(parseEventTimeMs(1_700_000_000)).toBe(1_700_000_000_000)
    expect(parseEventTimeMs(1_700_000_000_000)).toBe(1_700_000_000_000)
    expect(parseEventTimeMs('1700000000')).toBe(1_700_000_000_000)
  })

  it('parses RFC3339 with Z and offset', () => {
    expect(parseEventTimeMs('2023-11-14T22:13:20.000Z')).toBe(Date.parse('2023-11-14T22:13:20.000Z'))
    expect(parseEventTimeMs('2023-11-15T06:13:20+08:00')).toBe(Date.parse('2023-11-15T06:13:20+08:00'))
  })

  it('formatEventTime returns local absolute time for epoch seconds', () => {
    const formatted = formatEventTime(1_700_000_000)
    expect(formatted).toMatch(/\d/)
    expect(formatted).not.toBe('1700000000')
  })

  it('returns raw string when unparseable', () => {
    expect(formatEventTime('not-a-date')).toBe('not-a-date')
    expect(formatEventTime('')).toBe('')
  })
})

describe('localizeLogTimestamps', () => {
  it('rewrites leading UTC Z stamps without changing the rest of the line', () => {
    const line = '2023-11-14T22:13:20.123Z INFO easytier: hello'
    const out = localizeLogTimestamps(line)
    expect(out.endsWith(' INFO easytier: hello')).toBe(true)
    expect(out).not.toContain('2023-11-14T22:13:20.123Z')
  })

  it('leaves non-UTC prefixes alone', () => {
    const line = 'plain text without stamp'
    expect(localizeLogTimestamps(line)).toBe(line)
  })
})
