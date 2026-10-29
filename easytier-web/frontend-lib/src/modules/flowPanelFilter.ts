/** Fields used by the Status「实时流量」search box. */
export type FlowPanelFilterFields = {
  peer: string
  ip: string
  /** Localized TX/RX label; kept for text search, not shown as a table column. */
  direction: string
}

/** Direction selected in the panel dropdown (no “all” — list is always one-sided). */
export type FlowPanelDirectionFilter = 'tx' | 'rx'

export type FlowPanelDirectionKey = 'tx' | 'rx'

/** Case-insensitive substring match on peer / IP / direction. Empty query ⇒ keep all. */
export function flowPanelRowMatchesQuery(row: FlowPanelFilterFields, query: string): boolean {
  const q = query.trim().toLowerCase()
  if (!q)
    return true
  return [row.peer, row.ip, row.direction].some(field => field.toLowerCase().includes(q))
}

export function flowPanelRowMatchesDirection(
  dirKey: FlowPanelDirectionKey,
  directionFilter: FlowPanelDirectionFilter,
): boolean {
  return dirKey === directionFilter
}

export function filterFlowPanelRows<T extends FlowPanelFilterFields>(
  rows: T[],
  query: string,
  directionFilter: FlowPanelDirectionFilter = 'tx',
  dirKeyOf: (row: T) => FlowPanelDirectionKey = () => 'tx',
): T[] {
  return rows.filter((row) => {
    if (!flowPanelRowMatchesDirection(dirKeyOf(row), directionFilter))
      return false
    return flowPanelRowMatchesQuery(row, query)
  })
}
