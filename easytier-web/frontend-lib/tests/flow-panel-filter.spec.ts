import { describe, expect, it } from 'vitest'
import {
  filterFlowPanelRows,
  flowPanelRowMatchesDirection,
  flowPanelRowMatchesQuery,
  type FlowPanelDirectionKey,
} from '../src/modules/flowPanelFilter'

const rows = [
  { peer: 'node-a (10.10.10.1)', ip: '20.205.243.166', direction: '接收', dirKey: 'rx' as FlowPanelDirectionKey },
  { peer: 'node-b (10.10.10.2)', ip: '10.10.10.51', direction: '发送', dirKey: 'tx' as FlowPanelDirectionKey },
  { peer: 'relay (10.10.10.3)', ip: 'ff02::16', direction: '接收', dirKey: 'rx' as FlowPanelDirectionKey },
]

describe('flowPanelRowMatchesQuery', () => {
  it('keeps all rows when query is blank', () => {
    expect(flowPanelRowMatchesQuery(rows[0], '  ')).toBe(true)
  })

  it('matches peer, ip, or direction (case-insensitive)', () => {
    expect(flowPanelRowMatchesQuery(rows[0], 'NODE-A')).toBe(true)
    expect(flowPanelRowMatchesQuery(rows[1], '10.10.10.51')).toBe(true)
    expect(flowPanelRowMatchesQuery(rows[1], '发送')).toBe(true)
    expect(flowPanelRowMatchesQuery(rows[0], '发送')).toBe(false)
  })
})

describe('flowPanelRowMatchesDirection', () => {
  it('keeps only the selected direction', () => {
    expect(flowPanelRowMatchesDirection('tx', 'tx')).toBe(true)
    expect(flowPanelRowMatchesDirection('rx', 'tx')).toBe(false)
    expect(flowPanelRowMatchesDirection('rx', 'rx')).toBe(true)
  })
})

describe('filterFlowPanelRows', () => {
  it('filters the list by the shared query within the selected direction', () => {
    expect(filterFlowPanelRows(rows, '接收', 'rx', r => r.dirKey).map(r => r.peer)).toEqual([
      'node-a (10.10.10.1)',
      'relay (10.10.10.3)',
    ])
    expect(filterFlowPanelRows(rows, '', 'rx', r => r.dirKey)).toHaveLength(2)
  })

  it('defaults to tx and can switch to rx', () => {
    expect(filterFlowPanelRows(rows, '', 'tx', r => r.dirKey).map(r => r.peer)).toEqual([
      'node-b (10.10.10.2)',
    ])
    expect(filterFlowPanelRows(rows, '', 'rx', r => r.dirKey).map(r => r.peer)).toEqual([
      'node-a (10.10.10.1)',
      'relay (10.10.10.3)',
    ])
  })
})
