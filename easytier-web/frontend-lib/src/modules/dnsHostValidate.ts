/**
 * Mirror of easytier-core `classify_host_name` + `HostZoneTarget::reject_reason`
 * and easytier `parse_upstream_nameserver` / `normalize_forward_zone`
 * for console skip feedback (dns-policy-audit O2).
 * Keep in sync with config/dns.rs and instance/dns_server/server.rs.
 *
 * Case folding is **ASCII-only** (exact mirror of Rust `str::to_ascii_lowercase`):
 * Rust leaves every non-ASCII byte unchanged, while `String.toLowerCase()`
 * applies Unicode mappings (`İstanbul` → `i̇stanbul`, `ΣΣ` → `σσ`), so the two
 * sides would normalize the same name to different strings — fine for today's
 * checks, but a divergence trap for any future equality/serialization use of a
 * "mirror, keep in sync" module. Normalize only through `asciiLower` below.
 *
 * Scope is syntax only: runtime route/split conflicts need live MagicDNS
 * state and are intentionally not checked here.
 */

export const MIN_STATIC_HOST_ZONE_LABELS = 2

/**
 * ASCII-only lowercase — exact mirror of Rust `str::to_ascii_lowercase`.
 *
 * Exported so the mirror test can pin ASCII-only folding directly: the verdicts
 * below happen to coincide with `toLowerCase()` on today's checks, so only the
 * normalization result itself distinguishes the two (see the file header).
 */
export function asciiLower(s: string): string {
  return s.replace(/[A-Z]/g, (c) => String.fromCharCode(c.charCodeAt(0) + 32))
}

export type DnsHostSkipReason =
  | 'empty'
  | 'wildcard_needs_parent'
  | 'multi_wildcard'
  | 'invalid_wildcard_parent'
  | 'invalid_star'
  | 'empty_zone'
  | 'single_label_zone'

export interface DnsHostValidation {
  ok: boolean
  reason?: DnsHostSkipReason
}

function zoneLabelCount(zone: string): number {
  return zone
    .replace(/\.$/, '')
    .split('.')
    .filter((label) => label.length > 0).length
}

/**
 * Why a static host name would be skipped at apply time.
 * Returns null when the name is syntactically acceptable (runtime route/split
 * conflicts are not checked here — those need live MagicDNS state).
 */
export function staticHostSkipReason(name: string): DnsHostSkipReason | null {
  const trimmed = name.trim().replace(/\.+$/, '').trim()
  if (!trimmed) return 'empty'

  const lower = asciiLower(trimmed)
  if (lower.startsWith('*.')) {
    const parent = lower.slice(2)
    if (!parent) return 'wildcard_needs_parent'
    if (parent.includes('*')) return 'multi_wildcard'
    if (parent.startsWith('.') || parent.endsWith('.') || parent.includes('..')) {
      return 'invalid_wildcard_parent'
    }
    const zone = `${parent}.`
    if (!zone || zone === '.') return 'empty_zone'
    if (zoneLabelCount(zone) < MIN_STATIC_HOST_ZONE_LABELS) return 'single_label_zone'
    return null
  }

  if (lower.includes('*')) return 'invalid_star'

  const zone = `${lower}.`
  if (zoneLabelCount(zone) < MIN_STATIC_HOST_ZONE_LABELS) return 'single_label_zone'
  return null
}

/** IPv4 literal check for host ips chips (phase-1 hosts are A-only). */
export function isValidHostIpv4(ip: string): boolean {
  const parts = ip.trim().split('.')
  if (parts.length !== 4) return false
  return parts.every((p) => {
    if (!/^\d{1,3}$/.test(p)) return false
    const n = Number(p)
    return n >= 0 && n <= 255
  })
}

/** Minimal IPv6 literal check (mirrors Rust `IpAddr::from_str` for UI hints). */
export function isValidIpv6(s: string): boolean {
  if (!s) return false
  let addr = s
  let tailGroups = 0
  // An embedded IPv4 tail ("::ffff:1.2.3.4") counts as two groups.
  const tailMatch = /:((?:\d{1,3}\.){3}\d{1,3})$/.exec(addr)
  if (tailMatch) {
    if (!isValidHostIpv4(tailMatch[1])) return false
    addr = addr.slice(0, addr.length - tailMatch[1].length - 1)
    tailGroups = 2
  } else if (addr.includes('.')) {
    return false
  }
  const halves = addr.split('::')
  if (halves.length > 2) return false
  const checkSide = (side: string): string[] | null => {
    if (side === '') return []
    const groups = side.split(':')
    if (groups.some((g) => !/^[0-9a-fA-F]{1,4}$/.test(g))) return null
    return groups
  }
  if (halves.length === 1) {
    const groups = checkSide(halves[0])
    return groups !== null && groups.length + tailGroups === 8
  }
  const left = checkSide(halves[0])
  const right = checkSide(halves[1])
  if (left === null || right === null) return false
  // '::' must compress at least one group.
  return left.length + right.length + tailGroups <= 7
}

/** u16 port; empty string rejected (mirrors `str::parse::<u16>`). */
function isValidPort(portPart: string): boolean {
  if (!/^\d+$/.test(portPart)) return false
  const port = Number(portPart)
  return port >= 0 && port <= 65535
}

/** Mirror of `normalize_forward_zone`: only blank input is skipped. */
export function isValidForwarderDomain(domain: string): boolean {
  return domain.trim().replace(/\.+$/, '').trim().length > 0
}

/** Phase-1 upstream: IP literal, optional udp:// or tcp:// and port. */
export function isValidUpstreamDnsEntry(raw: string): boolean {
  const s = raw.trim()
  if (!s) return false
  let rest = s
  if (rest.startsWith('udp://')) rest = rest.slice(6)
  else if (rest.startsWith('tcp://')) rest = rest.slice(6)
  else if (rest.includes('://')) return false

  if (rest.startsWith('[')) {
    // Bracketed literal with optional :port.
    const end = rest.indexOf(']')
    if (end < 0) return false
    const addr = rest.slice(1, end)
    if (!isValidHostIpv4(addr) && !isValidIpv6(addr)) return false
    const after = rest.slice(end + 1)
    if (after === '') return true
    if (!after.startsWith(':')) return false
    return isValidPort(after.slice(1))
  }

  const lastColon = rest.lastIndexOf(':')
  let host = rest
  let portPart: string | null = null
  if (lastColon > 0 && !rest.slice(0, lastColon).includes(':')) {
    host = rest.slice(0, lastColon)
    portPart = rest.slice(lastColon + 1)
  }
  if (portPart !== null && !isValidPort(portPart)) return false
  // IPv4
  if (isValidHostIpv4(host)) return true
  // Bare IPv6 literal (contains ':'); phase 1 is IP-only, never hostnames.
  return host.includes(':') && isValidIpv6(host)
}
