import type { NetworkInstanceRunningInfo, PeerInfo, PeerRoutePair } from '../types/network'

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
    const loss = numericValue(conn.loss_rate)
    if (loss === undefined)
      continue

    return `${Math.round(loss * 100)}%`
  }

  return ''
}

/** Parse `installed=[a,b]` from desktop L2 proxy_cidr_route_sync summary. */
export function parseInstalledProxyCidrs(syncSummary?: string | null): string[] {
  if (!syncSummary)
    return []

  const match = syncSummary.match(/installed=\[([^\]]*)\]/)
  if (!match)
    return []

  const raw = match[1]?.trim()
  if (!raw || raw === '-')
    return []

  return raw
    .split(',')
    .map(s => s.trim())
    .filter(s => s.length > 0 && s !== '-')
}

/**
 * Default/uninitialized L2 summary before the desktop route updater reports,
 * or on platforms (Android) where ifcfg is a no-op and VpnService owns routes.
 * Showing this string as "sync status" is misleading.
 */
export function isMeaningfulProxyCidrRouteSync(syncSummary?: string | null): boolean {
  if (!syncSummary?.trim())
    return false
  // Empty placeholder with no last_error — not useful observability.
  if (/^desired=\[-\]\s*installed=\[-\]\s*exit=(true|false)$/.test(syncSummary.trim()))
    return false
  return true
}

function normalizeCidr(cidr: string): string {
  const trimmed = cidr.trim()
  if (!trimmed)
    return ''
  return trimmed.includes('/')
    ? trimmed
    : `${trimmed}${trimmed.includes(':') ? '/128' : '/32'}`
}

/**
 * Local OS / VPN routes currently applied for this instance.
 *
 * - `overrideRoutes === undefined`: no platform source → use meaningful L2 `installed=` only.
 * - `overrideRoutes` is an array (including `[]`): platform-authoritative (Android VpnService).
 * - Never fall back to route-table `proxy_cidrs` (those are advertised, not locally installed;
 *   wrong for no_tun / mobile without override).
 */
export function collectLocalInstalledRoutes(
  detail?: Pick<NetworkInstanceRunningInfo, 'proxy_cidr_route_sync' | 'routes'> | null,
  overrideRoutes?: string[] | null,
): string[] {
  // undefined/null = no override; [] = platform says nothing installed.
  if (overrideRoutes !== undefined && overrideRoutes !== null) {
    const fromOverride = overrideRoutes.map(normalizeCidr).filter(Boolean)
    return [...new Set(fromOverride)].sort()
  }

  if (!detail)
    return []

  const sync = detail.proxy_cidr_route_sync
  if (isMeaningfulProxyCidrRouteSync(sync))
    return parseInstalledProxyCidrs(sync)

  return []
}
