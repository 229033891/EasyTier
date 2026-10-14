import { describe, expect, it } from 'vitest'
import {
  dropStoredConfigsByIds,
  filterProvenFormerWebGhosts,
  findFormerWebGhostCandidates,
} from './former_web_ghosts'

function entry(id: string, source: 'user' | 'web' | 'legacy') {
  return {
    config: { instance_id: id } as never,
    source,
  }
}

describe('former_web_ghosts', () => {
  it('candidates are user/legacy absent from W', () => {
    const c = findFormerWebGhostCandidates(
      [entry('u1', 'user'), entry('w1', 'web'), entry('in-w', 'legacy')],
      ['in-w'],
    )
    expect(c.map(e => e.config.instance_id)).toEqual(['u1'])
  })

  it('only proves ghosts when device meta source is Web', () => {
    const candidates = [entry('ghost', 'user'), entry('draft', 'user'), entry('pbjson', 'legacy')]
    expect(filterProvenFormerWebGhosts(candidates, {
      ghost: { source: 2 },
      draft: { source: 1 },
      pbjson: { source: 'ConfigSourceWeb' },
    }, new Set())).toEqual(['ghost', 'pbjson'])
  })

  it('dropStoredConfigsByIds removes selected ids', () => {
    const kept = dropStoredConfigsByIds(
      [entry('a', 'user'), entry('b', 'legacy')],
      new Set(['a']),
    )
    expect(kept.map(e => e.config.instance_id)).toEqual(['b'])
  })
})
