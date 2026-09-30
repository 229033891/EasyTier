import type { PeerRoutePair } from '../types/network'

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

  if (minLatencyUs === undefined)
    return ''

  return `${Math.ceil(minLatencyUs / 1000)}ms`
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

/** 对端物理地址 IP:端口（默认连接优先，多连接去重后逗号拼接） */
export function peerRemoteAddr(info: PeerRoutePair): string {
  const addrs: string[] = []
  const seen = new Set<string>()

  for (const conn of defaultConnFirst(info)) {
    const formatted = formatTunnelHostPort(conn.tunnel?.remote_addr?.url)
    if (!formatted || seen.has(formatted))
      continue
    seen.add(formatted)
    addrs.push(formatted)
  }

  return addrs.join(', ')
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
