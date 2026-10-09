import type { PeerConnHistoryPoint } from './api'

/**
 * 把「累计计数器」序列转成速率序列（单位：字节/秒）。
 *
 * 历史表里存的是 rx_bytes / tx_bytes 的累计值（不是速率），所以这里对相邻桶做差分。
 * 两个边界情况：
 * - 第一个点没有前一个采样，返回 null（图表上留空，不要画成 0）
 * - 连接重建会让计数器归零，此时差分为负，按 0 处理，避免出现负的尖峰
 */
export function rateSeries(
  points: Array<PeerConnHistoryPoint>,
  key: 'rx_bytes' | 'tx_bytes',
): Array<number | null> {
  const out: Array<number | null> = []

  for (let i = 0; i < points.length; i++) {
    if (i === 0) {
      out.push(null)
      continue
    }

    const dt = points[i].t - points[i - 1].t
    const delta = points[i][key] - points[i - 1][key]
    out.push(dt > 0 && delta > 0 ? delta / dt : 0)
  }

  return out
}

/** 毫秒保留两位小数；拿不到延迟时返回 null 让图表断线 */
export function latencyMsSeries(points: Array<PeerConnHistoryPoint>): Array<number | null> {
  return points.map(p => (p.latency_us == null ? null : Number((p.latency_us / 1000).toFixed(2))))
}

/**
 * 丢包率转百分比（0–100）；拿不到时返回 null 让图表断线。
 * 后端存的是 0–1 小数（与实时节点表 loss_rate 一致）。
 */
export function lossPctSeries(points: Array<PeerConnHistoryPoint>): Array<number | null> {
  return points.map((p) => {
    if (p.loss_rate == null || !Number.isFinite(p.loss_rate) || p.loss_rate < 0)
      return null
    return Number((p.loss_rate * 100).toFixed(2))
  })
}

/** 毫秒保留两位小数；拿不到抖动时返回 null 让图表断线 */
export function jitterMsSeries(points: Array<PeerConnHistoryPoint>): Array<number | null> {
  return points.map(p => (p.jitter_us == null ? null : Number((p.jitter_us / 1000).toFixed(2))))
}
