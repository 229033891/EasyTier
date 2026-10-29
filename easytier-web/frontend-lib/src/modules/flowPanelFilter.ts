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

/**
 * Map wire `PeerConnFlow.direction` (0 = tx, 1 = rx) to a panel key.
 *
 * pbjson omits default proto3 scalars, so TX (`0`) often arrives as
 * `undefined`/`null` over the web JSON RPC. Treat missing like `0` (tx);
 * only an explicit non-zero value is rx — otherwise every TX row is misread
 * as RX and the default「发送」filter looks empty.
 *
 * Also accept numeric strings (`"0"` / `"1"`): other PeerConnFlow counters are
 * already `number | string` on the wire; keep direction parsing consistent.
 */
export function wireFlowDirectionKey(
  direction: number | string | null | undefined,
): FlowPanelDirectionKey {
  if (direction == null || direction === '')
    return 'tx'
  const n = typeof direction === 'number' ? direction : Number(direction)
  if (!Number.isFinite(n) || n === 0)
    return 'tx'
  return 'rx'
}

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
