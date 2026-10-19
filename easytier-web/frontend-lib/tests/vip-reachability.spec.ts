import { describe, expect, it } from 'vitest'
import {
  VipState,
  vipReachabilityIndex,
  vipReachabilityScore,
  vipStateKey,
  vipStateLabel,
  vipStateSeverity,
  vipStateTip,
} from '../src/modules/statusDisplay'
import type { PeerVipReachabilityInfo } from '../src/types/network'

function record(
  peerId: number,
  state: number,
  extra: Partial<PeerVipReachabilityInfo> = {},
) {
  return { peer_id: peerId, state, ...extra } as PeerVipReachabilityInfo
}

/** Per-link record (what the backend actually emits). */
function link(peerId: number, connId: string, state: number) {
  return record(peerId, state, { conn_id: connId })
}

const LABELS = {
  unknown: '未知',
  probing: '探测中',
  reachable: '可达',
  unreachable: '不可达',
}

describe('vipStateKey', () => {
  it('maps every backend state to a key', () => {
    expect(vipStateKey(undefined)).toBe('unknown')
    expect(vipStateKey(record(1, VipState.Unknown))).toBe('unknown')
    expect(vipStateKey(record(1, VipState.Probing))).toBe('probing')
    expect(vipStateKey(record(1, VipState.Reachable))).toBe('reachable')
    expect(vipStateKey(record(1, VipState.Unreachable))).toBe('unreachable')
  })

  it('treats an unrecognised state as unknown rather than guessing', () => {
    expect(vipStateKey(record(1, 99))).toBe('unknown')
  })
})

describe('vipStateSeverity', () => {
  it('uses amber (warn) for unreachable, never danger', () => {
    // A peer that blocks ping looks exactly like this; red would cry wolf.
    expect(vipStateSeverity('unreachable')).toBe('warn')
    expect(vipStateSeverity('reachable')).toBe('success')
    expect(vipStateSeverity('unknown')).toBe('secondary')
    expect(vipStateSeverity('probing')).toBe('secondary')
  })
})

describe('vipStateLabel', () => {
  it('appends rtt only when reachable', () => {
    expect(vipStateLabel(record(1, VipState.Reachable, { rtt_ms: 12 }), LABELS))
      .toBe('可达 12ms')
    expect(vipStateLabel(record(1, VipState.Reachable), LABELS)).toBe('可达')
    expect(vipStateLabel(record(1, VipState.Unreachable, { rtt_ms: 9 }), LABELS))
      .toBe('不可达')
    expect(vipStateLabel(undefined, LABELS)).toBe('未知')
  })

  it('renders a 0ms rtt instead of dropping it', () => {
    // Sub-millisecond round trips are reported as 0 by the core.
    expect(vipStateLabel(record(1, VipState.Reachable, { rtt_ms: 0 }), LABELS))
      .toBe('可达 0ms')
  })
})

describe('vipStateTip', () => {
  it('returns undefined when there is nothing to say', () => {
    expect(vipStateTip(record(1, VipState.Reachable))).toBeUndefined()
  })

  it('separates lines with <br> so the tooltip actually wraps', () => {
    const tip = vipStateTip(
      record(1, VipState.Unreachable, { error: 'timeout' }),
      { boundary: '边界说明', probeFailed: '探测失败' },
    )
    expect(tip).toBe('探测失败 (timeout)<br>边界说明')
    expect(tip).not.toContain('\n')
  })

  it('escapes untrusted error text', () => {
    const tip = vipStateTip(
      record(1, VipState.Unreachable, { error: '<img src=x>' }),
      { probeFailed: 'probe failed' },
    )
    expect(tip).toBe('probe failed (&lt;img src=x&gt;)')
  })
})

describe('vipReachabilityScore (per link)', () => {
  it('counts one link per reachability record', () => {
    const score = vipReachabilityScore([
      link(1, 'conn-a', VipState.Reachable),
      link(1, 'conn-b', VipState.Unreachable),
      link(2, 'conn-c', VipState.Reachable),
    ])
    // Three records ⇒ three links, regardless of how many peers they belong to.
    expect(score).toEqual({ reachable: 2, total: 3, hasData: true, anyUnknown: false })
  })

  it('does NOT collapse a multi-link peer into one unit', () => {
    // This is the contract change: a peer with a dead primary and a live standby
    // reads [1/2], not [1/1] — the summary now speaks the same unit as the rows.
    const score = vipReachabilityScore([
      link(1, 'conn-a', VipState.Unreachable),
      link(1, 'conn-b', VipState.Reachable),
    ])
    expect(score).toEqual({ reachable: 1, total: 2, hasData: true, anyUnknown: false })
  })

  it('counts the node-level (relay) entry as one link', () => {
    // Relay-only peer: conn_id is empty, still one link so it is never invisible.
    const score = vipReachabilityScore([
      record(5, VipState.Reachable),
      link(1, 'conn-a', VipState.Reachable),
      link(1, 'conn-b', VipState.Unreachable),
    ])
    expect(score).toEqual({ reachable: 2, total: 3, hasData: true, anyUnknown: false })
  })

  it('an all-relay mesh is not reported as empty', () => {
    const score = vipReachabilityScore([
      record(5, VipState.Reachable),
      record(6, VipState.Reachable),
    ])
    expect(score).toEqual({ reachable: 2, total: 2, hasData: true, anyUnknown: false })
  })

  it('reports hasData=false when the core sent no field (old core)', () => {
    expect(vipReachabilityScore(undefined)).toEqual({ reachable: 0, total: 0, hasData: false, anyUnknown: false })
  })

  it('counts probing / unknown links in the denominator but not as reachable', () => {
    const score = vipReachabilityScore([
      link(1, 'conn-a', VipState.Probing),
      link(1, 'conn-b', VipState.Reachable),
      link(2, 'conn-c', VipState.Unknown),
    ])
    expect(score).toEqual({ reachable: 1, total: 3, hasData: true, anyUnknown: true })
  })

  it('flags anyUnknown only when some link has no definitive verdict', () => {
    // All links answered (yes or no) ⇒ definitive, so green/amber is meaningful.
    const definitive = vipReachabilityScore([
      link(1, 'conn-a', VipState.Reachable),
      link(1, 'conn-b', VipState.Unreachable),
    ])
    expect(definitive.anyUnknown).toBe(false)

    // "Probing disabled" arrives as UNKNOWN + reason, i.e. nothing is measuring:
    // callers must render that neutrally instead of amber "everything is down".
    const disabled = vipReachabilityScore([
      link(1, 'conn-a', VipState.Unknown),
      link(1, 'conn-b', VipState.Unknown),
    ])
    expect(disabled).toEqual({ reachable: 0, total: 2, hasData: true, anyUnknown: true })
  })

  it('handles an empty link set', () => {
    expect(vipReachabilityScore([])).toEqual({ reachable: 0, total: 0, hasData: true, anyUnknown: false })
  })
})

describe('vipReachabilityIndex (per-link)', () => {
  it('returns the record for the row own conn, not the peer average', () => {
    // Peer 1 has a dead primary and a healthy standby.
    const index = vipReachabilityIndex([
      link(1, 'conn-a', VipState.Unreachable),
      link(1, 'conn-b', VipState.Reachable),
    ])
    expect(index.forConn(1, 'conn-a')?.state).toBe(VipState.Unreachable)
    expect(index.forConn(1, 'conn-b')?.state).toBe(VipState.Reachable)
  })

  it('falls back to the peer-level entry when the conn has none', () => {
    // Relay-only peer: backend reports one entry with an empty conn_id.
    const index = vipReachabilityIndex([record(2, VipState.Reachable)])
    expect(index.forConn(2, 'conn-x')?.state).toBe(VipState.Reachable)
    expect(index.forConn(2)?.state).toBe(VipState.Reachable)
  })

  it('returns undefined for an unknown peer', () => {
    const index = vipReachabilityIndex([link(1, 'conn-a', VipState.Reachable)])
    expect(index.forConn(9, 'conn-a')).toBeUndefined()
  })

  it('prefers the exact conn over the peer-level fallback', () => {
    const index = vipReachabilityIndex([
      record(1, VipState.Unreachable),
      link(1, 'conn-a', VipState.Reachable),
    ])
    expect(index.forConn(1, 'conn-a')?.state).toBe(VipState.Reachable)
    // Unknown conn ⇒ peer-level entry.
    expect(index.forConn(1, 'conn-zzz')?.state).toBe(VipState.Unreachable)
  })
})
