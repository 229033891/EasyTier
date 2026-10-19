import { describe, expect, it } from 'vitest'
import {
  VipState,
  normalizeVipState,
  vipReachabilityIndex,
  vipReachabilityScore,
  vipLinkTotal,
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

describe('normalizeVipState', () => {
  it('accepts the pbjson string form Tauri actually emits', () => {
    // Regression: comparing raw `"PEER_VIP_STATE_REACHABLE"` to numeric 2 left
    // every row as 「未知」 and the badge stuck at `[0/N]`.
    expect(normalizeVipState('PEER_VIP_STATE_UNKNOWN')).toBe(VipState.Unknown)
    expect(normalizeVipState('PEER_VIP_STATE_PROBING')).toBe(VipState.Probing)
    expect(normalizeVipState('PEER_VIP_STATE_REACHABLE')).toBe(VipState.Reachable)
    expect(normalizeVipState('PEER_VIP_STATE_UNREACHABLE')).toBe(VipState.Unreachable)
  })

  it('treats a missing state as Unknown (pbjson omits default 0)', () => {
    expect(normalizeVipState(undefined)).toBe(VipState.Unknown)
    expect(normalizeVipState(null)).toBe(VipState.Unknown)
  })

  it('keeps numeric values unchanged', () => {
    expect(normalizeVipState(VipState.Reachable)).toBe(VipState.Reachable)
    expect(normalizeVipState(VipState.Unreachable)).toBe(VipState.Unreachable)
  })
})

describe('vipStateKey', () => {
  it('maps every backend state to a key', () => {
    expect(vipStateKey(undefined)).toBe('unknown')
    expect(vipStateKey(record(1, VipState.Unknown))).toBe('unknown')
    expect(vipStateKey(record(1, VipState.Probing))).toBe('probing')
    expect(vipStateKey(record(1, VipState.Reachable))).toBe('reachable')
    expect(vipStateKey(record(1, VipState.Unreachable))).toBe('unreachable')
  })

  it('maps pbjson string states the same way as numbers', () => {
    expect(vipStateKey(record(1, 'PEER_VIP_STATE_REACHABLE' as unknown as number))).toBe('reachable')
    expect(vipStateKey(record(1, 'PEER_VIP_STATE_UNREACHABLE' as unknown as number))).toBe('unreachable')
    expect(vipStateKey({ peer_id: 1, state: undefined })).toBe('unknown')
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
  it('shows the bare rtt when reachable, no state word', () => {
    // The number IS the proof that the dataplane answered; the Tag colour and
    // the 「VIP」 column header carry the verdict instead.
    expect(vipStateLabel(record(1, VipState.Reachable, { rtt_ms: 12 }), LABELS))
      .toBe('12ms')
    // Reachable but no rtt on the wire: keep the state word rather than nothing.
    expect(vipStateLabel(record(1, VipState.Reachable), LABELS)).toBe('可达')
    expect(vipStateLabel(record(1, VipState.Unreachable, { rtt_ms: 9 }), LABELS))
      .toBe('不可达')
    expect(vipStateLabel(undefined, LABELS)).toBe('未知')
  })

  it('renders a 0ms rtt instead of dropping it', () => {
    // Sub-millisecond round trips are reported as 0 by the core.
    expect(vipStateLabel(record(1, VipState.Reachable, { rtt_ms: 0 }), LABELS))
      .toBe('0ms')
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

  it('explains an unknown state instead of showing a bare 未知', () => {
    // Regression: the reason used to be shown only for UNREACHABLE, so a screen
    // of "未知" gave the operator nothing to act on.
    const tip = vipStateTip(
      record(1, VipState.Unknown, { error: 'no_vip' }),
      { reason: '暂无结果' },
    )
    expect(tip).toBe('暂无结果 (no_vip)')
  })
})

describe('vipStateLabel reasons', () => {
  const REASONS = {
    disabled: '探测已关闭',
    no_vip: '本机尚未分配虚拟 IP',
    no_tun: 'no_tun 模式不支持探测',
    timeout: '探测超时',
    send_failed: '探测包发送失败',
  }

  it('appends the reason to an unknown row', () => {
    expect(vipStateLabel(record(1, VipState.Unknown, { error: 'disabled' }), LABELS, REASONS))
      .toBe('未知 · 探测已关闭')
  })

  it('keeps the bare state when the core sent no reason (old core)', () => {
    expect(vipStateLabel(record(1, VipState.Unknown), LABELS, REASONS)).toBe('未知')
  })

  it('does not append a reason to definitive states', () => {
    expect(vipStateLabel(record(1, VipState.Reachable, { rtt_ms: 3 }), LABELS, REASONS))
      .toBe('3ms')
    expect(vipStateLabel(record(1, VipState.Unreachable, { error: 'timeout' }), LABELS, REASONS))
      .toBe('不可达')
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

  it('reports hasData=false for an empty list (no round yet)', () => {
    // Same as omitted: do not treat [] as "prober ran", or every row becomes 未探测.
    expect(vipReachabilityScore([])).toEqual({ reachable: 0, total: 0, hasData: false, anyUnknown: false })
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

  it('counts pbjson string states correctly (Tauri wire form)', () => {
    const score = vipReachabilityScore([
      link(1, 'conn-a', 'PEER_VIP_STATE_REACHABLE' as unknown as number),
      link(1, 'conn-b', 'PEER_VIP_STATE_UNREACHABLE' as unknown as number),
      // Unknown is omitted on the wire ⇒ `state` is undefined.
      { peer_id: 2, conn_id: 'conn-c' } as PeerVipReachabilityInfo,
    ])
    expect(score).toEqual({ reachable: 1, total: 3, hasData: true, anyUnknown: true })
  })
})

describe('vipLinkTotal', () => {
  it('is the number of links the prober actually probed', () => {
    // 19 table rows but only 18 probe records: the denominator follows the
    // records, because M can only come from records. The 19th row is rendered
    // as 「未探测」, so the two numbers no longer contradict each other.
    expect(vipLinkTotal(19, 18)).toBe(18)
    expect(vipLinkTotal(19, 16)).toBe(16)
  })

  it('is 0 only when nothing was probed', () => {
    expect(vipLinkTotal(0, 0)).toBe(0)
    expect(vipLinkTotal(19, 0)).toBe(0)
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

  it('tells "never probed" apart from "no probe data at all"', () => {
    // Old core: no field ⇒ nothing is in scope, every row is merely unknown.
    const noData = vipReachabilityIndex(undefined)
    expect(noData.hasData).toBe(false)
    expect(noData.wasProbed(1, 'conn-a')).toBe(false)

    // Empty list (before the first round) is the same as "no data".
    expect(vipReachabilityIndex([]).hasData).toBe(false)

    // Prober ran, but this row's conn is outside its enumeration ⇒ 「未探测」,
    // not 「未知」: the badge's N never included it.
    const partial = vipReachabilityIndex([link(1, 'conn-a', VipState.Reachable)])
    expect(partial.hasData).toBe(true)
    expect(partial.wasProbed(1, 'conn-a')).toBe(true)
    expect(partial.wasProbed(2, 'conn-b')).toBe(false)
  })

  it('counts a peer-level record as covering that peer', () => {
    const index = vipReachabilityIndex([record(2, VipState.Reachable)])
    expect(index.wasProbed(2)).toBe(true)
    expect(index.wasProbed(2, 'conn-x')).toBe(true)
  })

  it('joins when peer_id arrives as a string', () => {
    // SameValueZero Map lookup: `"7" !== 7` would otherwise mark every row 未探测.
    const index = vipReachabilityIndex([
      { peer_id: '7', conn_id: 'conn-a', state: VipState.Reachable },
    ])
    expect(index.wasProbed(7, 'conn-a')).toBe(true)
    expect(index.forConn(7, 'conn-a')?.state).toBe(VipState.Reachable)
  })
})
