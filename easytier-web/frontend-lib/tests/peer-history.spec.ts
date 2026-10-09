import { describe, expect, it } from 'vitest'
import { jitterMsSeries, latencyMsSeries, lossPctSeries, rateSeries } from '../src/modules/peerHistory'
import type { PeerConnHistoryPoint } from '../src/modules/api'

function point(
  t: number,
  rx: number,
  tx: number,
  latency_us: number | null = 10_000,
  loss_rate: number | null = 0,
  jitter_us: number | null = 0,
): PeerConnHistoryPoint {
  return { t, latency_us, loss_rate, jitter_us, rx_bytes: rx, tx_bytes: tx, samples: 1 }
}

describe('rateSeries', () => {
  it('diffs cumulative counters into bytes per second', () => {
    // 60s 一个桶，累计值每次 +600 字节 => 10 B/s
    const points = [point(0, 1_000, 2_000), point(60, 1_600, 2_600), point(120, 2_200, 3_200)]
    expect(rateSeries(points, 'rx_bytes')).toEqual([null, 10, 10])
    expect(rateSeries(points, 'tx_bytes')).toEqual([null, 10, 10])
  })

  it('leaves the first bucket empty instead of faking a zero', () => {
    expect(rateSeries([point(0, 5, 5)], 'rx_bytes')).toEqual([null])
  })

  it('clamps counter resets to zero instead of emitting a negative spike', () => {
    // 连接重建：计数器从 5000 掉回 100
    const points = [point(0, 5_000, 0), point(60, 100, 0)]
    expect(rateSeries(points, 'rx_bytes')).toEqual([null, 0])
  })

  it('returns zero when there is no traffic growth', () => {
    const points = [point(0, 100, 100), point(60, 100, 100)]
    expect(rateSeries(points, 'rx_bytes')).toEqual([null, 0])
  })

  it('handles duplicated timestamps without dividing by zero', () => {
    const points = [point(0, 0, 0), point(0, 500, 0)]
    expect(rateSeries(points, 'rx_bytes')).toEqual([null, 0])
  })

  it('returns an empty series for no points', () => {
    expect(rateSeries([], 'rx_bytes')).toEqual([])
  })
})

describe('latencyMsSeries', () => {
  it('converts microseconds to milliseconds', () => {
    expect(latencyMsSeries([point(0, 0, 0, 12_345)])).toEqual([12.35])
  })

  it('keeps missing latency as null so the chart breaks the line', () => {
    expect(latencyMsSeries([point(0, 0, 0, null), point(60, 0, 0, 1_000)])).toEqual([null, 1])
  })
})

describe('lossPctSeries', () => {
  it('converts 0–1 loss_rate to percent', () => {
    expect(lossPctSeries([point(0, 0, 0, 0, 0.025)])).toEqual([2.5])
    expect(lossPctSeries([point(0, 0, 0, 0, 0)])).toEqual([0])
  })

  it('keeps missing or invalid loss as null so the chart breaks the line', () => {
    expect(lossPctSeries([
      point(0, 0, 0, 0, null),
      point(60, 0, 0, 0, -1),
      point(120, 0, 0, 0, 0.01),
    ])).toEqual([null, null, 1])
  })
})

describe('jitterMsSeries', () => {
  it('converts microseconds to milliseconds', () => {
    expect(jitterMsSeries([point(0, 0, 0, 0, 0, 1_500)])).toEqual([1.5])
  })

  it('keeps missing jitter as null so the chart breaks the line', () => {
    expect(jitterMsSeries([
      point(0, 0, 0, 0, 0, null),
      point(60, 0, 0, 0, 0, 2_000),
    ])).toEqual([null, 2])
  })
})
