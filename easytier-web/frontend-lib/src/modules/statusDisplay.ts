import {
  NatType,
  type PeerConnInfo,
  type PeerInfo,
  type PeerRoutePair,
  type PeerVipReachabilityInfo,
  type StunInfo,
} from '../types/network'

const udpNatTypeStrMap: Record<NatType, string> = {
  [NatType.Unknown]: 'Unknown',
  [NatType.OpenInternet]: 'Open Internet',
  [NatType.NoPAT]: 'No PAT',
  [NatType.FullCone]: 'Full Cone',
  [NatType.Restricted]: 'Restricted',
  [NatType.PortRestricted]: 'Port Restricted',
  [NatType.Symmetric]: 'Symmetric',
  [NatType.SymUdpFirewall]: 'Symmetric UDP Firewall',
  [NatType.SymmetricEasyInc]: 'Symmetric Easy Inc',
  [NatType.SymmetricEasyDec]: 'Symmetric Easy Dec',
}

export function numericValue(value: unknown): number | undefined {
  if (typeof value === 'number')
    return Number.isFinite(value) ? value : undefined

  if (typeof value !== 'string' || value.trim() === '')
    return undefined

  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : undefined
}

export function peerConns(info: PeerRoutePair) {
  return info.peer?.conns || []
}

function defaultConnId(info: PeerRoutePair) {
  const defaultConn = info.peer?.default_conn_id
  if (!defaultConn)
    return undefined

  const part1 = defaultConn.part1 ?? 0
  const part2 = defaultConn.part2 ?? 0
  const part3 = defaultConn.part3 ?? 0
  const part4 = defaultConn.part4 ?? 0
  if (part1 === 0 && part2 === 0 && part3 === 0 && part4 === 0)
    return undefined

  const toHex = (value: number) => value.toString(16).padStart(8, '0')
  const part1Hex = toHex(part1)
  const part2Hex = toHex(part2)
  const part3Hex = toHex(part3)
  const part4Hex = toHex(part4)
  return `${part1Hex}-${part2Hex.slice(0, 4)}-${part2Hex.slice(4, 8)}-${part3Hex.slice(0, 4)}-${part3Hex.slice(4, 8)}${part4Hex}`
}

function defaultConnFirst(info: PeerRoutePair) {
  const conns = peerConns(info)
  const connId = defaultConnId(info)
  if (!connId)
    return conns

  const defaultConn = conns.find(conn => conn.conn_id === connId)
  return defaultConn ? [defaultConn, ...conns.filter(conn => conn !== defaultConn)] : conns
}

/**
 * A path containing a peer that refused relay duty (`disable_relay_data`, or a
 * `relay_network_whitelist` miss) carries `AVOID_RELAY_COST` on one of its legs.
 * The backend reports that penalty truthfully instead of erasing it, so a
 * reported cost at or above this threshold means "policy-forced detour", not a
 * genuinely huge latency. Mirrors `AVOID_RELAY_COST` in
 * `easytier-core/src/peers/route/peer_ospf_route.rs`.
 */
export const POLICY_FORCED_PATH_COST = 1_000_000_000

export function latencyMs(info: PeerRoutePair, opts?: { policyForcedLabel?: string }) {
  const connId = defaultConnId(info)
  let minLatencyUs: number | undefined

  for (const conn of peerConns(info)) {
    if (!conn.stats)
      continue

    const latencyUs = numericValue(conn.stats.latency_us)
    if (latencyUs === undefined)
      continue

    if (connId === conn.conn_id)
      return `${Math.ceil(latencyUs / 1000)}ms`

    minLatencyUs = Math.min(minLatencyUs ?? latencyUs, latencyUs)
  }

  if (minLatencyUs !== undefined)
    return `${Math.ceil(minLatencyUs / 1000)}ms`

  // 中转无直连 tunnel：回退到路由表汇总的路径延迟（毫秒）
  const pathLatency = numericValue(info.route?.path_latency_latency_first)
    ?? numericValue(info.route?.path_latency)
  if (pathLatency !== undefined && pathLatency > 0) {
    // The backend keeps the relay-policy penalty in `path_latency` instead of
    // erasing it, so a cost this large is a policy-forced detour. Render it as
    // such rather than printing "1000000000ms".
    if (pathLatency >= POLICY_FORCED_PATH_COST)
      return opts?.policyForcedLabel || 'policy detour'
    return `${Math.ceil(pathLatency)}ms`
  }

  return ''
}

export type RoutePeerLabel = {
  peerId?: number
  hostname?: string
  ipv4?: string
}

export type RoutePathHop = {
  kind: 'node' | 'ellipsis'
  /** node: 展示名；ellipsis: 省略的中间跳数 */
  label: string
  peerId?: number
}

export type RoutePathResult = {
  /** p2p | relay | local | unknown */
  kind: 'p2p' | 'relay' | 'local' | 'unknown'
  /** 跳数（边数）；local 为 0 */
  cost: number
  hops: RoutePathHop[]
  /** 表格主文案 */
  text: string
  /** 悬停说明 */
  tip?: string
}

function peerLabel(meta?: RoutePeerLabel | null, fallback = '?'): string {
  const host = meta?.hostname?.trim()
  if (host)
    return host
  const ip = meta?.ipv4?.trim()
  if (ip)
    return ip
  if (meta?.peerId)
    return `#${meta.peerId}`
  return fallback
}

function nextHopPeerId(info: PeerRoutePair): number | undefined {
  const route = info.route
  if (!route)
    return undefined

  const latencyFirst = route.next_hop_peer_id_latency_first
  if (typeof latencyFirst === 'number' && latencyFirst > 0)
    return latencyFirst

  const nextHop = route.next_hop_peer_id
  if (typeof nextHop === 'number' && nextHop > 0)
    return nextHop

  return undefined
}

/**
 * 路由路径展示。
 * 协议只下发 next_hop + cost（跳数），完整拓扑不可得：
 * - cost=1：直连
 * - cost=2：本机 → 下一跳 → 目标（完整）
 * - cost>2：本机 → 下一跳 → … → 目标（中间节点未知）
 */
export function resolveRoutePath(
  info: PeerRoutePair,
  opts?: {
    localLabel?: string
    peersMetaById?: Map<number, RoutePeerLabel>
    /** i18n：经 {path} */
    viaPath?: (path: string) => string
    /** i18n：中间省略 */
    ellipsisLabel?: (count: number) => string
    /** i18n：路径不完整提示 */
    incompleteTip?: string
    p2pLabel?: string
    localText?: string
  },
): RoutePathResult {
  const cost = info.route?.cost
  if (cost === undefined || cost === null || cost === 0) {
    return {
      kind: 'local',
      cost: 0,
      hops: [{ kind: 'node', label: opts?.localLabel || 'Local' }],
      text: opts?.localText || 'Local',
    }
  }

  if (cost === 1) {
    return {
      kind: 'p2p',
      cost: 1,
      hops: [],
      text: opts?.p2pLabel || 'p2p',
    }
  }

  const peersMetaById = opts?.peersMetaById
  const localLabel = opts?.localLabel || 'Local'
  const destId = info.route?.peer_id
  const hopId = nextHopPeerId(info)

  const fromMap = destId ? peersMetaById?.get(destId) : undefined
  const destMeta: RoutePeerLabel = {
    peerId: destId,
    hostname: info.route?.hostname || fromMap?.hostname,
    ipv4: (typeof info.route?.ipv4_addr === 'string' ? info.route.ipv4_addr : undefined)
      || fromMap?.ipv4,
  }

  const nextMeta = hopId ? peersMetaById?.get(hopId) : undefined
  const hops: RoutePathHop[] = [
    { kind: 'node', label: localLabel },
  ]

  if (hopId && hopId !== destId) {
    hops.push({
      kind: 'node',
      label: peerLabel(nextMeta, `#${hopId}`),
      peerId: hopId,
    })
  }

  const middleCount = Math.max(0, cost - 2)
  if (middleCount > 0) {
    hops.push({
      kind: 'ellipsis',
      label: opts?.ellipsisLabel?.(middleCount) || `…(+${middleCount})`,
    })
  }

  hops.push({
    kind: 'node',
    label: peerLabel(destMeta),
    peerId: destId,
  })

  const pathText = hops.map(h => h.label).join(' → ')
  const text = opts?.viaPath?.(pathText) || pathText
  const tip = middleCount > 0
    ? [opts?.incompleteTip, pathText].filter(Boolean).join('\n')
    : pathText

  return {
    kind: 'relay',
    cost,
    hops,
    text,
    tip,
  }
}

/** 从 tunnel URL 提取 host:port（IPv6 带方括号） */
export function formatTunnelHostPort(url?: string): string {
  if (!url)
    return ''

  try {
    const normalized = url.includes('://') ? url : `tcp://${url}`
    const parsed = new URL(normalized)
    const host = parsed.hostname
    if (!host)
      return url.replace(/^[a-z0-9+]+:\/\//i, '')

    const hostDisp = host.includes(':') ? `[${host}]` : host
    return parsed.port ? `${hostDisp}:${parsed.port}` : hostDisp
  }
  catch {
    return url.replace(/^[a-z0-9+]+:\/\//i, '')
  }
}

function collectTunnelRemoteAddrs(peer?: PeerInfo | null, preferConnId?: string): string[] {
  const conns = peer?.conns || []
  if (!conns.length)
    return []

  const ordered = preferConnId
    ? (() => {
        const preferred = conns.find(conn => conn.conn_id === preferConnId)
        return preferred ? [preferred, ...conns.filter(conn => conn !== preferred)] : conns
      })()
    : conns

  const addrs: string[] = []
  const seen = new Set<string>()
  for (const conn of ordered) {
    const formatted = formatTunnelHostPort(conn.tunnel?.remote_addr?.url)
    if (!formatted || seen.has(formatted))
      continue
    seen.add(formatted)
    addrs.push(formatted)
  }
  return addrs
}

export type PeerRemoteAddrResult = {
  /** 展示用地址，多连接逗号拼接 */
  text: string
  /** 是否来自中转下一跳（非目标 peer 直连） */
  viaNextHop: boolean
}

/**
 * 对端隧道层地址：
 * - 直连：目标 peer 的 tunnel.remote_addr
 * - 中转：本机到下一跳（中转节点）的 tunnel.remote_addr（目标 peer 无直连隧道）
 */
export function resolvePeerRemoteAddr(
  info: PeerRoutePair,
  peersById?: Map<number, PeerInfo>,
): PeerRemoteAddrResult {
  const direct = collectTunnelRemoteAddrs(info.peer, defaultConnId(info))
  if (direct.length)
    return { text: direct.join(', '), viaNextHop: false }

  const cost = info.route?.cost
  if (!cost || cost <= 1 || !peersById)
    return { text: '', viaNextHop: false }

  const hopId = nextHopPeerId(info)
  if (!hopId)
    return { text: '', viaNextHop: false }

  // 下一跳若等于目标本身，不再标 via
  if (hopId === info.route?.peer_id)
    return { text: '', viaNextHop: false }

  const hopPeer = peersById.get(hopId)
  const hopAddrs = collectTunnelRemoteAddrs(hopPeer)
  if (!hopAddrs.length)
    return { text: '', viaNextHop: false }

  return { text: hopAddrs.join(', '), viaNextHop: true }
}

/** 兼容旧调用：仅返回文本；中转场景请用 resolvePeerRemoteAddr */
export function peerRemoteAddr(info: PeerRoutePair, peersById?: Map<number, PeerInfo>): string {
  return resolvePeerRemoteAddr(info, peersById).text
}

export function lossRate(info: PeerRoutePair) {
  for (const conn of defaultConnFirst(info)) {
    // protobuf JSON omits zero loss_rate; treat missing as 0 for an existing conn
    const loss = numericValue(conn.loss_rate ?? 0)
    if (loss === undefined)
      continue

    return `${Math.round(loss * 100)}%`
  }

  return ''
}

/** Resolve UDP NAT type from protobuf JSON (numeric or enum name). */
export function udpNatTypeName(stunInfo: StunInfo | undefined) {
  if (!stunInfo)
    return ''

  const value = stunInfo.udp_nat_type ?? NatType.Unknown
  const natType = numericValue(value) ?? NatType[value as keyof typeof NatType]
  return udpNatTypeStrMap[natType as NatType] ?? udpNatTypeStrMap[NatType.Unknown]
}

/** Jitter from default (or best) PeerConnStats, in milliseconds. */
export function jitterMs(info: PeerRoutePair) {
  const connId = defaultConnId(info)
  let minJitterUs: number | undefined

  for (const conn of peerConns(info)) {
    if (!conn.stats)
      continue

    const jitterUs = numericValue(conn.stats.jitter_us)
    if (jitterUs === undefined)
      continue

    if (connId === conn.conn_id)
      return `${Math.ceil(jitterUs / 1000)}ms`

    minJitterUs = Math.min(minJitterUs ?? jitterUs, jitterUs)
  }

  if (minJitterUs !== undefined)
    return `${Math.ceil(minJitterUs / 1000)}ms`

  return ''
}

export type ConnQualityLine = {
  /** Tunnel type label, e.g. udp / tcp */
  proto: string
  /** True when this conn is PeerInfo.default_conn_id */
  isDefault: boolean
  /** Lower is better; undefined if backend omitted score */
  score?: number
  fused: boolean
  /** True when backend flags this conn in the bond send set (Phase 2b) */
  inBond: boolean
  /** Diversity summary from backend (`bond_class`), e.g. udp */
  bondClass?: string
  latencyMs?: number
  jitterMs?: number
  lossPct?: number
  remote?: string
}

function oneConnProto(tunnel?: { tunnel_type?: string }): string {
  return tunnel?.tunnel_type || '?'
}

/** Per-conn quality rows for status tooltips / path analysis (P2.1). */
export function connQualityLines(info: PeerRoutePair): ConnQualityLine[] {
  const preferId = defaultConnId(info)
  return defaultConnFirst(info).map((conn) => {
    const latencyUs = numericValue(conn.stats?.latency_us)
    const jitterUs = numericValue(conn.stats?.jitter_us)
    // protobuf JSON omits zero loss_rate; treat missing as 0 (same as lossRate()).
    const loss = numericValue(conn.loss_rate ?? 0)
    const score = typeof conn.quality_score === 'number' && Number.isFinite(conn.quality_score)
      ? conn.quality_score
      : undefined
    const bondClass = typeof conn.bond_class === 'string' && conn.bond_class.trim()
      ? conn.bond_class.trim()
      : undefined
    return {
      proto: oneConnProto(conn.tunnel),
      isDefault: !!preferId && conn.conn_id === preferId,
      score,
      fused: !!conn.quality_fused,
      inBond: !!conn.in_bond_set,
      bondClass,
      latencyMs: latencyUs === undefined ? undefined : Math.ceil(latencyUs / 1000),
      jitterMs: jitterUs === undefined ? undefined : Math.ceil(jitterUs / 1000),
      lossPct: loss === undefined ? undefined : Math.round(loss * 100),
      remote: conn.tunnel?.remote_addr?.url,
    }
  })
}

function formatPathQualityTipLine(parts: {
  role: string
  proto: string
  score: string
  latency: string
  jitter: string
  loss: string
  fused: boolean
  bondSuffix: string
}): string {
  const fused = parts.fused ? ' fused' : ''
  return `${parts.role} ${parts.proto} score=${parts.score} rtt=${parts.latency} jitter=${parts.jitter} loss=${parts.loss}${fused}${parts.bondSuffix}`
}

function bondTipSuffix(inBond: boolean, bondClass?: string): string {
  if (!inBond)
    return ''
  return bondClass ? ` bond(${bondClass})` : ' bond'
}

/** Compact cell: default score + standby count, e.g. `0.042 · +1`, plus `· bond×2` when bonded. */
export function pathQualityCell(info: PeerRoutePair): string {
  const lines = connQualityLines(info)
  if (!lines.length)
    return ''
  const primary = lines.find(l => l.isDefault) ?? lines[0]
  const standby = Math.max(0, lines.length - 1)
  const scoreText = primary.score === undefined
    ? '—'
    : primary.score.toFixed(3)
  const fusedMark = primary.fused ? '!' : ''
  const base = standby > 0
    ? `${scoreText}${fusedMark} · +${standby}`
    : `${scoreText}${fusedMark}`
  const bonded = lines.filter(l => l.inBond).length
  return bonded > 0 ? `${base} · bond×${bonded}` : base
}

/** Multi-line tip explaining each PeerConn for operators. */
export function pathQualityTip(info: PeerRoutePair): string {
  const lines = connQualityLines(info)
  if (!lines.length)
    return ''
  return lines.map((line) => {
    const score = line.score === undefined ? '—' : line.score.toFixed(3)
    const lat = line.latencyMs === undefined ? '—' : `${line.latencyMs}ms`
    const jit = line.jitterMs === undefined ? '—' : `${line.jitterMs}ms`
    const loss = line.lossPct === undefined ? '—' : `${line.lossPct}%`
    return formatPathQualityTipLine({
      role: line.isDefault ? 'default' : 'standby',
      proto: line.proto,
      score,
      latency: lat,
      jitter: jit,
      loss,
      fused: line.fused,
      bondSuffix: bondTipSuffix(line.inBond, line.bondClass),
    })
  }).join('\n')
}

/** One DataTable row: either a single PeerConn or a peer with no direct tunnels. */
export type PeerConnTableRow = {
  key: string
  pair: PeerRoutePair
  conn?: PeerConnInfo
  isDefault: boolean
  connIndex: number
  connCount: number
}

/**
 * Flatten peer_route_pairs so each direct tunnel is its own row.
 * Local / relay-only peers (no conns) stay as a single row with `conn` unset.
 *
 * Use `connIndex` / `connCount` for intra-peer separators: when
 * `connCount > 1 && connIndex < connCount - 1`, more tunnels of the same peer
 * follow (dashed border); otherwise this row ends the peer group (solid).
 */
export function flattenPeerConnRows(pairs: PeerRoutePair[]): PeerConnTableRow[] {
  const rows: PeerConnTableRow[] = []

  for (let pi = 0; pi < pairs.length; pi++) {
    const pair = pairs[pi]
    const conns = defaultConnFirst(pair)
    const preferId = defaultConnId(pair)
    // peer_id is numeric in protobuf; stringify so row keys stay stable.
    const peerKey = String(pair.peer?.peer_id ?? pair.route?.peer_id ?? `idx-${pi}`)

    if (!conns.length) {
      rows.push({
        key: `peer-${peerKey}`,
        pair,
        conn: undefined,
        isDefault: false,
        connIndex: 0,
        connCount: 0,
      })
      continue
    }

    const hasPreferred = !!preferId && conns.some(c => c.conn_id === preferId)
    conns.forEach((conn, i) => {
      // Prefer backend default_conn_id; if missing/stale, first ordered conn is primary.
      const isDefault = hasPreferred ? conn.conn_id === preferId : i === 0
      rows.push({
        key: `conn-${conn.conn_id || `${peerKey}-${i}`}`,
        pair,
        conn,
        isDefault,
        connIndex: i,
        connCount: conns.length,
      })
    })
  }

  return rows
}

export function connLatencyMs(conn?: PeerConnInfo): string {
  const latencyUs = numericValue(conn?.stats?.latency_us)
  return latencyUs === undefined ? '' : `${Math.ceil(latencyUs / 1000)}ms`
}

export function connJitterMs(conn?: PeerConnInfo): string {
  const jitterUs = numericValue(conn?.stats?.jitter_us)
  return jitterUs === undefined ? '' : `${Math.ceil(jitterUs / 1000)}ms`
}

export function connLossRate(conn?: PeerConnInfo): string {
  if (!conn)
    return ''
  // protobuf JSON omits zero loss_rate; missing on an existing conn means 0%.
  const loss = numericValue(conn.loss_rate ?? 0)
  return loss === undefined ? '' : `${Math.round(loss * 100)}%`
}

export function connRemoteAddr(conn?: PeerConnInfo): string {
  return formatTunnelHostPort(conn?.tunnel?.remote_addr?.url)
}

export function connLocalAddr(conn?: PeerConnInfo): string {
  return formatTunnelHostPort(conn?.tunnel?.local_addr?.url)
}

/** Single-conn quality cell for flattened rows (score / bond / fuse).
 * Default vs standby is shown by whole-row bold (`peer-row-default`), not ★/·. */
export function connPathQualityCell(conn?: PeerConnInfo, _isDefault = false): string {
  if (!conn)
    return ''
  const score = typeof conn.quality_score === 'number' && Number.isFinite(conn.quality_score)
    ? conn.quality_score.toFixed(3)
    : '—'
  const fused = conn.quality_fused ? '!' : ''
  const bond = conn.in_bond_set
    ? (conn.bond_class?.trim() ? ` · bond(${conn.bond_class.trim()})` : ' · bond')
    : ''
  return `${score}${fused}${bond}`
}

export function connPathQualityTip(conn?: PeerConnInfo, isDefault = false): string {
  if (!conn)
    return ''
  const score = typeof conn.quality_score === 'number' && Number.isFinite(conn.quality_score)
    ? conn.quality_score.toFixed(3)
    : '—'
  return formatPathQualityTipLine({
    role: isDefault ? 'default' : 'standby',
    proto: oneConnProto(conn.tunnel),
    score,
    latency: connLatencyMs(conn) || '—',
    jitter: connJitterMs(conn) || '—',
    loss: connLossRate(conn) || '—',
    fused: !!conn.quality_fused,
    bondSuffix: bondTipSuffix(!!conn.in_bond_set, conn.bond_class?.trim() || undefined),
  })
}

// ---------------------------------------------------------------------------
// VIP dataplane reachability (docs/roadmap/gui-connected-vs-reachable.md).
//
// Independent of the control-plane keepalive: a peer can be "connected" while
// its VIP dataplane probe fails. Old cores never send `peer_vip_reachability`,
// so absence means "unknown" for every row — never "unreachable".
// ---------------------------------------------------------------------------

/** PeerVipState numeric values (api.instance.PeerVipState). */
export const VipState = {
  Unknown: 0,
  Probing: 1,
  Reachable: 2,
  Unreachable: 3,
} as const

export type VipStateKey
  = | 'unknown'
    | 'probing'
    | 'reachable'
    | 'unreachable'
    /** The prober never produced a record for this link (see `vipLinkTotal`). */
    | 'not_probed'

/**
 * Normalize `PeerVipReachability.state` from the Tauri / pbjson wire form.
 *
 * pbjson serializes the enum as a string (`"PEER_VIP_STATE_REACHABLE"`) and
 * **omits** the field entirely when the value is 0 (Unknown). Comparing the
 * raw value to numeric `VipState.*` therefore classified every link as
 * unknown and kept the badge at `[0/N]` even when probes answered.
 */
export function normalizeVipState(state: unknown): number {
  if (typeof state === 'number' && Number.isFinite(state))
    return state
  if (typeof state === 'string') {
    const trimmed = state.trim()
    switch (trimmed) {
      case 'PEER_VIP_STATE_UNKNOWN':
      case 'Unknown':
      case 'unknown':
        return VipState.Unknown
      case 'PEER_VIP_STATE_PROBING':
      case 'Probing':
      case 'probing':
        return VipState.Probing
      case 'PEER_VIP_STATE_REACHABLE':
      case 'Reachable':
      case 'reachable':
        return VipState.Reachable
      case 'PEER_VIP_STATE_UNREACHABLE':
      case 'Unreachable':
      case 'unreachable':
        return VipState.Unreachable
      default: {
        const asNum = Number(trimmed)
        if (Number.isFinite(asNum))
          return asNum
      }
    }
  }
  // Field absent (pbjson skips default 0) ⇒ Unknown.
  return VipState.Unknown
}

export function vipStateKey(info?: PeerVipReachabilityInfo): VipStateKey {
  if (!info)
    return 'unknown'
  switch (normalizeVipState(info.state)) {
    case VipState.Reachable: return 'reachable'
    case VipState.Unreachable: return 'unreachable'
    case VipState.Probing: return 'probing'
    default: return 'unknown'
  }
}

/**
 * Presentation severity per VIP state.
 *
 * Unreachable is deliberately **warn (amber)**, not danger (red): a peer that
 * merely blocks ping looks identical to one whose dataplane is broken, so
 * escalating it to red on the whole row would cry wolf. The row-level Tag is
 * the only place this severity is used.
 */
export function vipStateSeverity(key: VipStateKey): 'success' | 'warn' | 'secondary' {
  if (key === 'reachable') return 'success'
  if (key === 'unreachable') return 'warn'
  // `not_probed` is deliberately `secondary` (gray), same as unknown: the link
  // was never attempted, which is not evidence of a fault.
  return 'secondary'
}

/**
 * Per-row VIP Tag label: the bare rtt when reachable, and a reason suffix when
 * the state is a non-result (`未知 · 探测已关闭`).
 *
 * Reachable deliberately prints **only** the round-trip time (`7ms`) instead of
 * `可达 7ms`: the number is itself the proof that the dataplane answered, so the
 * word adds width without adding information. The Tag's colour still carries the
 * verdict, and the column header says 「VIP」. When the backend reports
 * reachable without an rtt, the state word is kept rather than printing nothing.
 *
 * The reason belongs inline, not only in the tooltip: a table full of bare
 * "未知" tells the operator nothing about whether probing is off, the node has
 * no VIP yet, or the peer never answered.
 */
export function vipStateLabel(
  info?: PeerVipReachabilityInfo,
  labels?: Partial<Record<VipStateKey, string>>,
  reasons?: Record<string, string>,
): string {
  const key = vipStateKey(info)
  const base = labels?.[key] ?? key
  // Only the measurement, no state word: a time can only come from a reply.
  if (key === 'reachable' && info?.rtt_ms !== undefined)
    return `${info.rtt_ms}ms`
  if (key === 'unknown' && info?.error) {
    const reason = reasons?.[info.error]
    // Old core / absent field: no error string, so the bare state is all there is.
    if (reason)
      return `${base} · ${reason}`
  }
  return base
}

/** Reason strings for `PeerVipReachability.error`, for tooltips and labels. */
export function vipReasonLabels(i18n: {
  disabled: string
  noVip: string
  noTun: string
  timeout: string
  sendFailed: string
}): Record<string, string> {
  return {
    disabled: i18n.disabled,
    no_vip: i18n.noVip,
    no_tun: i18n.noTun,
    timeout: i18n.timeout,
    send_failed: i18n.sendFailed,
  }
}

/**
 * Tooltip for a row's VIP Tag: state plus failure reason / boundary note.
 *
 * The backend `error` field explains *any* non-result state, not just failures:
 * `disabled` (probing off), `no_vip` (no local VIP yet), `no_tun`, `timeout`,
 * `send_failed`. Showing it only for `unreachable` is what made a screen full of
 * "未知" unexplainable — the reason was there and got dropped here.
 *
 * Returns HTML (PrimeVue `v-tooltip` does not honour `\n`), so every part is
 * escaped here — the backend `error` string is machine-generated but is still
 * treated as untrusted text.
 */
export function vipStateTip(
  info: PeerVipReachabilityInfo | undefined,
  i18n?: { boundary?: string, probeFailed?: string, reason?: string },
): string | undefined {
  const escapeHtml = (text: string) => text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
  const key = vipStateKey(info)
  const lines: string[] = []
  if (info?.error) {
    const prefix = key === 'unreachable'
      ? (i18n?.probeFailed ?? 'probe failed')
      : (i18n?.reason ?? 'no result')
    lines.push(escapeHtml(`${prefix} (${info.error})`))
  }
  if (i18n?.boundary)
    lines.push(escapeHtml(i18n.boundary))
  return lines.length ? lines.join('<br>') : undefined
}

/**
 * Index the per-link reachability records by peer, then by conn.
 *
 * Rebuilt on every snapshot so lookups stay O(1) per row: the detail table can
 * have many rows and each one asks twice (Tag + tooltip).
 */
export function vipReachabilityIndex(reachability: PeerVipReachabilityInfo[] | undefined) {
  const byPeer = new Map<number, PeerVipReachabilityInfo[]>()
  for (const record of reachability || []) {
    // Coerce: protobuf JSON / Tauri may deliver peer_id as a string; Map keys
    // are compared with SameValueZero, so `"12" !== 12` would look like 未探测.
    const peerId = Number(record.peer_id)
    if (!Number.isFinite(peerId))
      continue
    const list = byPeer.get(peerId)
    if (list)
      list.push(record)
    else
      byPeer.set(peerId, [record])
  }

  /** The record for one specific link, or the peer-level fallback (`conn_id === ''`). */
  const forConn = (peerId: number, connId?: string): PeerVipReachabilityInfo | undefined => {
    const list = byPeer.get(Number(peerId))
    if (!list)
      return undefined
    if (connId) {
      const exact = list.find(r => r.conn_id === connId)
      if (exact)
        return exact
    }
    return list.find(r => !r.conn_id)
  }

  /**
   * Whether the prober actually enumerated this link.
   *
   * Distinguishes the two ways a row can end up with no record:
   * - the backend sent **no** reachability field at all (old core) → nothing is
   *   probed, so every row is 「未知」;
   * - the backend sent records, but none for this row → the row is outside the
   *   prober's link enumeration and was never attempted → 「未探测」.
   *
   * Without this the second case is indistinguishable from a failed probe, which
   * is what made `[0/18]` next to a 19-row table look like a bug.
   */
  const hasData = reachability !== undefined
  const wasProbed = (peerId: number, connId?: string): boolean => {
    const list = byPeer.get(Number(peerId))
    if (!list)
      return false
    if (!connId)
      return list.some(r => !r.conn_id)
    return list.some(r => r.conn_id === connId || !r.conn_id)
  }

  /**
   * True when at least one record is not a definitive verdict (i.e. still
   * `probing`, or `unknown` with a reason such as "probing disabled"). Callers
   * use it to avoid painting a summary green/amber when nothing is measuring.
   */
  const anyUnknown = (reachability || []).some((r) => {
    const state = normalizeVipState(r.state)
    return state === VipState.Probing || state === VipState.Unknown
  })

  return { byPeer, forConn, wasProbed, hasData, anyUnknown }
}

/**
 * Denominator for the `[M/N]` badge: the number of links the prober actually
 * probed (`peer_vip_reachability.length`).
 *
 * Chosen over the table row count because `M` and `N` must come from the same
 * measurement: putting rows in the denominator would let a row that has no probe
 * record sit inside `N` while being unable to contribute to `M`, so the badge
 * could never reach `[N/N]` even on a perfectly healthy mesh.
 *
 * The counts can legitimately differ (`collect_links` enumerates the route
 * table, and a peer with no enumerable PeerConn collapses into a single
 * node-level record). Rows the prober never saw are labelled **「未探测」**
 * rather than 「未知」 — that is what keeps e.g. `18` and `19` from reading as a
 * contradiction: the table itself marks which row is outside `N`.
 */
export function vipLinkTotal(_rowCount: number, recordCount: number): number {
  // The row count is intentionally ignored — see the note above on why rows must
  // not enter the denominator. It stays in the signature so the call site has to
  // name both figures and the trade-off stays visible.
  return recordCount
}
/**
 * The `[M/N]` score, counted **per link**: M = links whose probe answered,
 * N = links probed.
 *
 * Rationale: the row-level Tags next to this badge are also per link, so the
 * summary and the table now speak the same unit — "primary dead, standby fine"
 * shows as `[1/2]` instead of being rounded up to a node-level "reachable".
 *
 * What counts as one link:
 * - one live **PeerConn** (the normal case), and
 * - the **node-level entry** the backend emits for peers with no enumerable
 *   conn (relay-only peers, `conn_id === ''`). It counts as one link so such a
 *   peer is never silently absent — an all-relay mesh must not read as `[0/0]`
 *   ("no data") when every relay actually answers.
 *
 * `hasData: false` means the backend sent no reachability field at all (old
 * core): callers must hide the badge rather than show `[0/0]` or `[?/0]`.
 * `anyUnknown` is true when at least one link is still `probing`/`unknown`, i.e.
 * nothing definitive was measured — callers render that neutrally instead of
 * green/amber.
 */
export function vipReachabilityScore(
  reachability: PeerVipReachabilityInfo[] | undefined,
): { reachable: number, total: number, hasData: boolean, anyUnknown: boolean } {
  if (!reachability)
    return { reachable: 0, total: 0, hasData: false, anyUnknown: false }
  let reachable = 0
  let anyUnknown = false
  for (const record of reachability) {
    const state = normalizeVipState(record.state)
    if (state === VipState.Reachable)
      reachable += 1
    else if (state !== VipState.Unreachable)
      anyUnknown = true
  }
  return { reachable, total: reachability.length, hasData: true, anyUnknown }
}
