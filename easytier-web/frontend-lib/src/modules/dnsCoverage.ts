/**
 * Expected MagicDNS / DNS-policy coverage (dns-policy.md §11 B6).
 * Phase 1: OS/version inference. Phase 2: optional `magic_dns_os_wired` overrides.
 * Do NOT treat heartbeat liveness as coverage.
 */

export type DnsCoverageState =
  | 'off'
  | 'covered'
  | 'manual_required'
  | 'unsupported'
  | 'version_too_old'

export interface DnsCoverageInput {
  enable_magic_dns?: boolean | null
  /** From heartbeat DeviceOsInfo.os_type (e.g. windows, linux, macos, android). */
  os_type?: string | null
  easytier_version?: string | null
  no_tun?: boolean | null
  /**
   * B6 phase 2: optional heartbeat `magic_dns_os_wired`.
   * Unset → phase-1 OS/version inference; true → covered; false → wiring skipped/failed.
   */
  magic_dns_os_wired?: boolean | null
}

/**
 * First release that understands `DnsConfig` / managed DNS patch.
 * Released 2.7.0–2.7.3 ignore unknown fields; do not treat them as capable.
 * Bump together with the shipping crate version when DnsConfig first ships.
 */
export const DNS_POLICY_MIN_VERSION = '2.7.4'

function parseVersionParts(version: string): number[] | null {
  const cleaned = version.trim().replace(/^v/i, '').split(/[+-]/)[0]
  if (!cleaned) return null
  const parts = cleaned.split('.').map((p) => Number.parseInt(p, 10))
  if (parts.some((n) => Number.isNaN(n))) return null
  return parts
}

/** Returns true when `version` is strictly older than `minVersion`. */
export function isVersionOlderThan(version: string, minVersion: string): boolean {
  const a = parseVersionParts(version)
  const b = parseVersionParts(minVersion)
  if (!a || !b) return false
  const len = Math.max(a.length, b.length)
  for (let i = 0; i < len; i++) {
    const av = a[i] ?? 0
    const bv = b[i] ?? 0
    if (av < bv) return true
    if (av > bv) return false
  }
  return false
}

function normalizeOs(osType: string | null | undefined): string {
  return (osType ?? '').trim().toLowerCase()
}

/**
 * Phase-1 expected coverage for hosts push / MagicDNS wiring.
 * `version_too_old` means the node can still resolve mesh names but will not
 * apply DnsConfig.hosts from managed config.
 */
export function expectedDnsCoverage(input: DnsCoverageInput): DnsCoverageState {
  if (!input.enable_magic_dns) {
    return 'off'
  }

  const version = input.easytier_version ?? ''
  if (version && isVersionOlderThan(version, DNS_POLICY_MIN_VERSION)) {
    return 'version_too_old'
  }

  if (input.no_tun) {
    return 'unsupported'
  }

  // Phase 2: reported OS wiring overrides phase-1 OS matrix when present.
  if (input.magic_dns_os_wired === true) {
    return 'covered'
  }
  if (input.magic_dns_os_wired === false) {
    return 'manual_required'
  }

  const os = normalizeOs(input.os_type)
  if (os.includes('ios') || os.includes('iphone') || os.includes('ipad')) {
    return 'unsupported'
  }
  if (
    os.includes('windows') ||
    os === 'macos' ||
    os === 'darwin' ||
    os.includes('android')
  ) {
    return 'covered'
  }

  if (os.includes('linux') || os.includes('openwrt') || os.includes('freebsd')) {
    return 'manual_required'
  }

  if (!os) {
    // Unknown OS: do not silently treat as covered.
    return 'manual_required'
  }

  return 'unsupported'
}

/**
 * Platform wiring expectation when Magic DNS is enabled on the network.
 * Used on the device list where per-network `enable_magic_dns` is unknown.
 */
export function platformDnsCapability(
  input: Omit<DnsCoverageInput, 'enable_magic_dns'>,
): Exclude<DnsCoverageState, 'off'> {
  return expectedDnsCoverage({ ...input, enable_magic_dns: true }) as Exclude<
    DnsCoverageState,
    'off'
  >
}

/** CSS modifier suffix for coverage badges (`dns-coverage-badge--{state}`). */
export function dnsCoverageBadgeModifier(state: DnsCoverageState): string {
  return state
}
