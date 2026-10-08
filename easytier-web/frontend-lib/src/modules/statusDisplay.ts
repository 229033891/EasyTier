import { NatType, type PeerInfo, type PeerRoutePair, type StunInfo } from '../types/network'

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

export function latencyMs(info: PeerRoutePair) {
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
  if (pathLatency !== undefined && pathLatency > 0)
    return `${Math.ceil(pathLatency)}ms`

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
function formatTunnelHostPort(url?: string): string {
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
    const loss = numericValue(conn.loss_rate)
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
    const role = line.isDefault ? '★' : '·'
    const score = line.score === undefined ? '—' : line.score.toFixed(3)
    const lat = line.latencyMs === undefined ? '—' : `${line.latencyMs}ms`
    const jit = line.jitterMs === undefined ? '—' : `${line.jitterMs}ms`
    const loss = line.lossPct === undefined ? '—' : `${line.lossPct}%`
    const fused = line.fused ? ' fused' : ''
    const bond = line.inBond ? (line.bondClass ? ` bond(${line.bondClass})` : ' bond') : ''
    return `${role} ${line.proto} score=${score} rtt=${lat} jitter=${jit} loss=${loss}${fused}${bond}`
  }).join('\n')
}
