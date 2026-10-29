import type { FlowHistoryPoint, FlowHistorySeries } from './api'
import { wireFlowDirectionKey } from './flowPanelFilter'

/**
 * 隧道级 IP 流量历史的纯函数工具。
 *
 * 与 `peerHistory.ts` 的关键区别：这里**不做差分**。速率由节点侧固定窗口算出
 * （决策 D11），历史表里直接存 `rate_bytes_per_sec`；流条目在 TTL / 上限淘汰后
 * 会重新计数，对累计值做差分只会得到假的尖峰。
 */

/** 一条曲线在响应里的唯一键（conn + peer + IP/网段 + 方向，与后端 series key 对齐） */
export function flowSeriesKey(series: FlowHistorySeries): string {
    // Normalize so omitted / "0" TX does not diverge from explicit 0 in the key.
    const dir = wireFlowDirectionKey(series.direction) === 'tx' ? 0 : 1
    return `${series.conn_id}:${series.peer_id}:${series.ip}:${series.prefix_len}:${dir}`
}

/** 0 / omitted = tx（发往该 IP），1 = rx（来自该 IP）；与实时面板同一套解析. */
export function flowDirectionArrow(direction: number | string | null | undefined): string {
    return wireFlowDirectionKey(direction) === 'tx' ? '↑' : '↓'
}

/** `10.0.0.0` + prefix_len 24 → `10.0.0.0/24`；精确 IP 原样返回 */
export function flowIpLabel(series: Pick<FlowHistorySeries, 'ip' | 'prefix_len'>): string {
    return series.prefix_len > 0 ? `${series.ip}/${series.prefix_len}` : series.ip
}

/** 图例 / 选择器里的一行：`↑ 10.0.0.0/24 · peer-b` */
export function flowSeriesLabel(series: FlowHistorySeries): string {
    const arrow = flowDirectionArrow(series.direction)
    const ip = flowIpLabel(series)
    const peer = series.peer_hostname?.trim()
    return peer ? `${arrow} ${ip} · ${peer}` : `${arrow} ${ip}`
}

/**
 * 多条曲线共用一条时间轴：把各自的时间戳取并集并排序。
 *
 * 不同曲线的采样桶可能错开（流条目晚出现 / 早消失），统一到并集上才能在同一张
 * 图里对比；缺失的桶在对齐时补 null（前端 `spanGaps` 断线，而不是画成 0）。
 */
export function buildFlowTimeline(series: FlowHistorySeries[]): number[] {
    const seen = new Set<number>()
    for (const item of series) {
        for (const point of item.points) {
            seen.add(point.t)
        }
    }
    return Array.from(seen).sort((a, b) => a - b)
}

/** 把一条曲线对齐到给定时间轴；缺失的桶为 null */
export function alignFlowRates(
    points: FlowHistoryPoint[],
    timeline: number[],
): Array<number | null> {
    const byTime = new Map<number, number>()
    for (const point of points) {
        byTime.set(point.t, point.rate_bytes_per_sec)
    }
    return timeline.map(t => {
        const value = byTime.get(t)
        return value === undefined ? null : value
    })
}

/** 1234 → `1.2 KB/s` */
export function formatFlowRate(bytesPerSec: number): string {
    if (!Number.isFinite(bytesPerSec) || bytesPerSec <= 0) return '0 B/s'
    const k = 1024
    const sizes = ['B/s', 'KB/s', 'MB/s', 'GB/s', 'TB/s']
    // 0 < x < 1 时 Math.log(x)/Math.log(1024) 为负，clamp 到 0 防止 sizes[-1] 为 undefined
    const i = Math.min(Math.max(Math.floor(Math.log(bytesPerSec) / Math.log(k)), 0), sizes.length - 1)
    return `${parseFloat((bytesPerSec / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`
}

/**
 * 曲线配色：按索引循环。
 *
 * 注意：**不要**用它给图表上色。服务端按窗口内总流量降序重排曲线，索引会随
 * 流量变化而漂移，同一流量的颜色就会跳变。上色请用
 * {@link flowSeriesColorForKey}（按曲线 key 哈希，与顺序无关）。
 */
export const FLOW_SERIES_COLORS = [
    'rgb(14, 165, 233)',
    'rgb(34, 197, 94)',
    'rgb(249, 115, 22)',
    'rgb(168, 85, 247)',
    'rgb(236, 72, 153)',
    'rgb(234, 179, 8)',
]

export function flowSeriesColor(index: number): string {
    return FLOW_SERIES_COLORS[index % FLOW_SERIES_COLORS.length]
}

/**
 * 稳定配色：按曲线 key 哈希取色，而不是按服务端返回顺序。
 * 服务端按窗口内总流量重排后，同一条曲线的颜色不应变化。
 */
export function flowSeriesColorForKey(key: string): string {
    let hash = 0
    for (let i = 0; i < key.length; i++) {
        hash = (hash * 31 + key.charCodeAt(i)) | 0
    }
    return FLOW_SERIES_COLORS[Math.abs(hash) % FLOW_SERIES_COLORS.length]
}
