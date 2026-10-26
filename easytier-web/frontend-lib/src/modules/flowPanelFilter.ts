/** Fields used by the Status「实时流量」search box. */
export type FlowPanelFilterFields = {
  peer: string
  ip: string
  direction: string
}

/** Case-insensitive substring match on peer / IP / direction. Empty query ⇒ keep all. */
export function flowPanelRowMatchesQuery(row: FlowPanelFilterFields, query: string): boolean {
  const q = query.trim().toLowerCase()
  if (!q)
    return true
  return [row.peer, row.ip, row.direction].some(field => field.toLowerCase().includes(q))
}

export function filterFlowPanelRows<T extends FlowPanelFilterFields>(rows: T[], query: string): T[] {
  const q = query.trim()
  if (!q)
    return rows
  return rows.filter(row => flowPanelRowMatchesQuery(row, q))
}
