import { describe, expect, it } from 'vitest'
import { pruneAbsentWebConfigs } from './prune_web_configs'

function entry(id: string, source: 'user' | 'web' | 'legacy') {
  return {
    config: { instance_id: id } as never,
    source,
  }
}

describe('pruneAbsentWebConfigs', () => {
  it('drops web configs absent from console W', () => {
    const kept = pruneAbsentWebConfigs(
      [
        entry('keep-web', 'web'),
        entry('gone-web', 'web'),
        entry('user-draft', 'user'),
      ],
      ['keep-web'],
    )
    expect(kept.map(e => e.config.instance_id)).toEqual(['keep-web', 'user-draft'])
  })

  it('keeps user and legacy rows even when not in W', () => {
    const kept = pruneAbsentWebConfigs(
      [entry('user-only', 'user'), entry('legacy-only', 'legacy')],
      [],
    )
    expect(kept.map(e => e.config.instance_id)).toEqual(['user-only', 'legacy-only'])
  })

  it('removes all web rows when W is empty (console has no web rows)', () => {
    // Callers must omit W when unknown; empty W means console deleted everything.
    const kept = pruneAbsentWebConfigs(
      [entry('gone-web', 'web'), entry('user-draft', 'user')],
      [],
    )
    expect(kept.map(e => e.config.instance_id)).toEqual(['user-draft'])
  })

  it('keeps stopped web rows that are still in W (console disable, not delete)', () => {
    const kept = pruneAbsentWebConfigs(
      [entry('stopped-web', 'web'), entry('running-web', 'web')],
      ['stopped-web', 'running-web'],
    )
    expect(kept.map(e => e.config.instance_id)).toEqual(['stopped-web', 'running-web'])
  })
})
