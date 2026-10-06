import type { NetworkConfig } from '../types/network'

export type ConfigConflictSeverity = 'warn' | 'info'

export interface ConfigConflictWarning {
  code: string
  severity: ConfigConflictSeverity
  /** i18n key under locales root, e.g. `disable_p2p_conflict_help` */
  i18nKey: string
  fields: Array<keyof NetworkConfig>
}

/**
 * Detect mutually-incoherent option combinations.
 * Never mutates config — legacy saves may keep conflicting values; UI only warns.
 */
export function collectConfigConflictWarnings(config: NetworkConfig): ConfigConflictWarning[] {
  const warnings: ConfigConflictWarning[] = []

  if (config.disable_p2p && (config.p2p_only || config.lazy_p2p || config.need_p2p)) {
    warnings.push({
      code: 'disable_p2p_conflict',
      severity: 'warn',
      i18nKey: 'disable_p2p_conflict_help',
      fields: ['disable_p2p', 'p2p_only', 'lazy_p2p', 'need_p2p'],
    })
  }

  if (config.p2p_only && config.latency_first) {
    warnings.push({
      code: 'p2p_only_latency_first',
      severity: 'warn',
      i18nKey: 'p2p_only_latency_first_conflict_help',
      fields: ['p2p_only', 'latency_first'],
    })
  }

  if (
    config.p2p_only &&
    (config.disable_relay_data || config.prefer_peer_relay || config.enable_relay_network_whitelist)
  ) {
    warnings.push({
      code: 'p2p_only_relay',
      severity: 'warn',
      i18nKey: 'p2p_only_relay_conflict_help',
      fields: ['p2p_only', 'disable_relay_data', 'prefer_peer_relay', 'enable_relay_network_whitelist'],
    })
  }

  if (config.disable_tcp_hole_punching && config.disable_udp_hole_punching) {
    warnings.push({
      code: 'all_hole_punching_disabled',
      severity: 'info',
      i18nKey: 'all_hole_punching_disabled_help',
      fields: ['disable_tcp_hole_punching', 'disable_udp_hole_punching'],
    })
  }

  if (config.enable_kcp_proxy && config.disable_kcp_input) {
    warnings.push({
      code: 'kcp_proxy_no_input',
      severity: 'warn',
      i18nKey: 'kcp_proxy_no_input_conflict_help',
      fields: ['enable_kcp_proxy', 'disable_kcp_input'],
    })
  }

  if (config.enable_kcp_proxy && config.enable_quic_proxy) {
    warnings.push({
      code: 'dual_proxy',
      severity: 'info',
      i18nKey: 'dual_proxy_conflict_help',
      fields: ['enable_kcp_proxy', 'enable_quic_proxy'],
    })
  }

  if (
    config.disable_ipv6 &&
    (config.ipv6_public_addr_auto || config.ipv6_public_addr_provider)
  ) {
    warnings.push({
      code: 'disable_ipv6_conflict',
      severity: 'warn',
      i18nKey: 'disable_ipv6_conflict_help',
      fields: ['disable_ipv6', 'ipv6_public_addr_auto', 'ipv6_public_addr_provider'],
    })
  }

  if (config.no_tun && (Boolean(config.dev_name) || config.mtu != null)) {
    warnings.push({
      code: 'no_tun_dev_mtu',
      severity: 'info',
      i18nKey: 'no_tun_dev_mtu_conflict_help',
      fields: ['no_tun', 'dev_name', 'mtu'],
    })
  }

  if (config.disable_encryption && config.encryption_algorithm && config.encryption_algorithm !== 'aes-gcm') {
    warnings.push({
      code: 'disable_encryption_algo',
      severity: 'info',
      i18nKey: 'disable_encryption_algo_conflict_help',
      fields: ['disable_encryption', 'encryption_algorithm'],
    })
  }

  return warnings
}

/**
 * Block turning a flag ON when a parent option makes it meaningless.
 * If the flag is already true (legacy conflict), leave it editable so the user can uncheck.
 */
export function isAdvancedFlagDisabled(config: NetworkConfig, field: keyof NetworkConfig): boolean {
  if (config[field] === true) {
    return false
  }

  if (config.disable_p2p && (field === 'p2p_only' || field === 'lazy_p2p' || field === 'need_p2p')) {
    return true
  }
  if (config.p2p_only && field === 'disable_p2p') {
    return true
  }
  if (
    config.p2p_only &&
    (field === 'latency_first' ||
      field === 'disable_relay_data' ||
      field === 'prefer_peer_relay' ||
      field === 'enable_relay_network_whitelist')
  ) {
    return true
  }
  return false
}

/** Hide flags that are meaningless under the current parent option. */
export function isAdvancedFlagHidden(config: NetworkConfig, field: keyof NetworkConfig): boolean {
  return (
    config.disable_ipv6 === true &&
    (field === 'ipv6_public_addr_auto' || field === 'ipv6_public_addr_provider')
  )
}

/** Tooltip override when a flag is blocked by a parent option. */
export function advancedFlagConflictHelpKey(
  config: NetworkConfig,
  field: keyof NetworkConfig,
): string | null {
  if (config.disable_p2p && (field === 'p2p_only' || field === 'lazy_p2p' || field === 'need_p2p')) {
    return 'disable_p2p_blocks_help'
  }
  if (config.p2p_only && field === 'disable_p2p') {
    return 'p2p_only_blocks_disable_p2p_help'
  }
  if (config.p2p_only && field === 'latency_first') {
    return 'p2p_only_blocks_latency_first_help'
  }
  if (
    config.p2p_only &&
    (field === 'disable_relay_data' ||
      field === 'prefer_peer_relay' ||
      field === 'enable_relay_network_whitelist')
  ) {
    return 'p2p_only_blocks_relay_help'
  }
  return null
}
