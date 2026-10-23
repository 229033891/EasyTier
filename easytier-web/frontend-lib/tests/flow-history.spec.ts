import { describe, expect, it } from 'vitest'
import type { FlowHistoryPoint, FlowHistorySeries } from '../src/modules/api'
import {
  alignFlowRates,
  buildFlowTimeline,
  FLOW_SERIES_COLORS,
  flowDirectionArrow,
  flowIpLabel,
  flowSeriesColor,
  flowSeriesColorForKey,
  flowSeriesKey,
  flowSeriesLabel,
  formatFlowRate,
} from '../src/modules/flowHistory'

function point(t: number, rate: number, bytes = rate * 60, flow_count = 1): FlowHistoryPoint {
  return { t, rate_bytes_per_sec: rate, bytes, flow_count, sampled: false, samples: 1 }
}

function series(
  overrides: Partial<FlowHistorySeries> = {},
  points: FlowHistoryPoint[] = [point(0, 100)],
): FlowHistorySeries {
  return {
    conn_id: '42',
    peer_id: 7,
    peer_hostname: 'peer-b',
    ip: '10.0.0.0',
    prefix_len: 24,
    direction: 1,
    last_seen: 0,
    points,
    ...overrides,
  }
}

describe('flowSeriesKey', () => {
  it('separates tunnels, IPs and directions', () => {
    expect(flowSeriesKey(series())).toBe('42:7:10.0.0.0:24:1')
    // 同一 peer 的第二条隧道必须是不同曲线
    expect(flowSeriesKey(series({ conn_id: '7' }))).not.toBe(flowSeriesKey(series()))
    // 同一条隧道上的收发方向也要分开
    expect(flowSeriesKey(series({ direction: 0 }))).not.toBe(flowSeriesKey(series()))
  })
})

describe('flowIpLabel / flowSeriesLabel', () => {
  it('appends the prefix length only for aggregated rows', () => {
    expect(flowIpLabel(series())).toBe('10.0.0.0/24')
    expect(flowIpLabel(series({ ip: '10.0.0.9', prefix_len: 0 }))).toBe('10.0.0.9')
    expect(flowIpLabel(series({ ip: 'fd00::', prefix_len: 64 }))).toBe('fd00::/64')
  })

  it('marks the direction with an arrow and joins the peer hostname', () => {
    expect(flowSeriesLabel(series())).toBe('↓ 10.0.0.0/24 · peer-b')
    expect(flowSeriesLabel(series({ direction: 0 }))).toBe('↑ 10.0.0.0/24 · peer-b')
    expect(flowSeriesLabel(series({ peer_hostname: '' }))).toBe('↓ 10.0.0.0/24')
    expect(flowDirectionArrow(0)).toBe('↑')
    expect(flowDirectionArrow(1)).toBe('↓')
  })
})

describe('buildFlowTimeline / alignFlowRates', () => {
  it('unions and sorts timestamps across series', () => {
    const a = series({}, [point(60, 1), point(120, 2)])
    const b = series({ conn_id: '7' }, [point(0, 3), point(120, 4)])
    expect(buildFlowTimeline([a, b])).toEqual([0, 60, 120])
    expect(buildFlowTimeline([])).toEqual([])
  })

  it('fills missing buckets with null instead of zero', () => {
    const a = series({}, [point(0, 10), point(120, 30)])
    const timeline = [0, 60, 120]
    // 0 和 120 有值，60 缺失 → null（图表断线，而不是画成 0 速率）
    expect(alignFlowRates(a.points, timeline)).toEqual([10, null, 30])
  })

  it('uses the node-side rate directly (no differencing)', () => {
    // 累计值被 TTL 重置：1000 → 0，但速率仍是 50，不能出现负尖峰
    const a = series({}, [point(0, 50, 1_000), point(60, 50, 0)])
    expect(alignFlowRates(a.points, [0, 60])).toEqual([50, 50])
  })
})

describe('formatFlowRate', () => {
  it('formats bytes per second with binary units', () => {
    expect(formatFlowRate(0)).toBe('0 B/s')
    expect(formatFlowRate(-5)).toBe('0 B/s')
    expect(formatFlowRate(512)).toBe('512 B/s')
    expect(formatFlowRate(1_536)).toBe('1.5 KB/s')
    expect(formatFlowRate(5 * 1024 * 1024)).toBe('5 MB/s')
  })

  /**
   * 后端桶内速率是 `AVG(rate_bytes_per_sec)`（f64），所以 <1 B/s 的小数一定会
   * 出现。`Math.log(0.5)/Math.log(1024)` 为负，旧实现直接取 `sizes[-1]` 得到
   * undefined，渲染成 "512 undefined"；这条用例锁住 clamp。
   */
  it('keeps sub-1 B/s rates in B/s instead of an undefined unit', () => {
    expect(formatFlowRate(0.5)).toBe('0.5 B/s')
    expect(formatFlowRate(0.05)).toBe('0.1 B/s')
    expect(formatFlowRate(0.999)).toBe('1 B/s')
    for (const rate of [0.001, 0.5, 0.999]) {
      expect(formatFlowRate(rate)).not.toContain('undefined')
    }
  })

  it('never emits a non-finite unit for pathological input', () => {
    expect(formatFlowRate(Number.NaN)).toBe('0 B/s')
    expect(formatFlowRate(Number.POSITIVE_INFINITY)).toBe('0 B/s')
  })
})

describe('flowSeriesColor', () => {
  it('cycles through the palette so series keep stable colors', () => {
    expect(flowSeriesColor(0)).toBe(flowSeriesColor(6))
    expect(flowSeriesColor(1)).not.toBe(flowSeriesColor(0))
  })
})

describe('flowSeriesColorForKey', () => {
  /**
   * 服务端按窗口内总流量降序返回曲线，顺序会随流量变化。按索引取色会让同一条
   * 曲线在两次刷新之间换色，所以图表必须用 key 哈希取色：同一 key 恒定，且与
   * 传入顺序无关。
   */
  it('is stable per key and independent of ordering', () => {
    const keyA = flowSeriesKey(series())
    const keyB = flowSeriesKey(series({ conn_id: '99' }))
    expect(flowSeriesColorForKey(keyA)).toBe(flowSeriesColorForKey(keyA))
    // 顺序翻转不改变各自颜色
    expect([keyA, keyB].map(flowSeriesColorForKey)).toEqual(
      [keyB, keyA].map(flowSeriesColorForKey).reverse(),
    )
  })

  it('always returns a colour from the palette', () => {
    for (const key of ['', 'a', '42:7:10.0.0.0:24:1', 'x'.repeat(512)]) {
      expect(FLOW_SERIES_COLORS).toContain(flowSeriesColorForKey(key))
    }
  })
})
