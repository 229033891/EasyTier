/** Fields used by the Status「实时流量」search box. */
export type FlowPanelFilterFields = {
  peer: string
  ip: string
  direction: string
}

/** Stable direction key for the panel chip filter (not the localized label). */
export type FlowPanelDirectionFilter = 'all' | 'tx' | 'rx'

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
  if (directionFilter === 'all')
    return true
  return dirKey === directionFilter
}

export function filterFlowPanelRows<T extends FlowPanelFilterFields>(
  rows: T[],
  query: string,
  directionFilter: FlowPanelDirectionFilter = 'all',
  dirKeyOf: (row: T) => FlowPanelDirectionKey = () => 'tx',
): T[] {
  return rows.filter((row) => {
    if (!flowPanelRowMatchesDirection(dirKeyOf(row), directionFilter))
      return false
    return flowPanelRowMatchesQuery(row, query)
  })
}
