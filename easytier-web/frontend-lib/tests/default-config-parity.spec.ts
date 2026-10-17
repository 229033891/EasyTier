import { describe, expect, it } from 'vitest'
import { DEFAULT_NETWORK_CONFIG } from '../src/types/network'

/**
 * A new network created from the GUI must land on the same effective values as
 * a fresh TOML/CLI deployment. Each expectation below cites the core default it
 * mirrors; see `easytier-core/src/config/toml.rs` (`gen_default_flags` /
 * `ConfigLoader::get_dhcp`) and `easytier/src/core.rs` (`Cli::DEFAULT_LISTENERS`).
 *
 * These are cross-language invariants, so they are asserted literally here — a
 * core-side default change has to be made in both places on purpose.
 */
describe('new-network defaults match the core defaults', () => {
  // NOTE: `dhcp` is intentionally NOT asserted here. The GUI ships `dhcp: true`
  // (an end-user network-creation flow that always runs with a TUN adapter)
  // while the CLI/core default stays `false` (a bare TOML config may
  // legitimately be address-less, e.g. relay-only / gateway nodes). Flipping
  // the core default to `true` would make every such config require a host
  // adapter and fail to start, so the two ends are NOT identical on this field
  // — that is context-specific, not a bug to sync away.
  it('protocol preference is udp,tcp', () => {
    // Mirrors `gen_default_flags().default_protocol` (toml.rs).
    expect(DEFAULT_NETWORK_CONFIG().default_protocol).toBe('udp,tcp')
  })

  it('bond count is 2', () => {
    // Mirrors `gen_default_flags().peer_link_bond_count` / `_replica_fill_max`.
    const config = DEFAULT_NETWORK_CONFIG()
    expect(config.peer_link_bond_count).toBe(2)
    expect(config.peer_link_replica_fill_max).toBe(2)
  })

  it('mtu is spelled out rather than left null', () => {
    // Mirrors `gen_default_flags().mtu`. `null` used to *resolve* to 1300 anyway
    // (api_input only applies `if let Some(mtu)`), so this is about the two ends
    // displaying/persisting the same number, not about behavior changing.
    expect(DEFAULT_NETWORK_CONFIG().mtu).toBe(1300)
  })

  it('keepalive pacing match the core defaults (ping_interval_max_sec 32 / ping_fail_close_count 10)', () => {
    // Mirrors `gen_default_flags()` (toml.rs: DEFAULT_PING_INTERVAL_MAX_SEC /
    // DEFAULT_PING_FAIL_CLOSE_COUNT). 10s is the NAT-safe default: 32s used to
    // outlive a ~30s carrier-NAT UDP mapping on *direct* tunnels and blackhole
    // them between keepalives (tailscale-gap audit §5) — hole-punched conns
    // already had a 10s per-conn cap, direct conns were the gap. Set 32
    // explicitly to restore the old pacing.
    const config = DEFAULT_NETWORK_CONFIG()
    expect(config.ping_interval_max_sec).toBe(10)
    expect(config.ping_fail_close_count).toBe(10)
  })

  it('listeners match the CLI default set', () => {
    // Mirrors `Cli::DEFAULT_LISTENERS` (easytier/src/core.rs). Deliberately NOT
    // the full IpScheme::VARIANTS expansion — the CLI used to open quic/ws/wss/
    // faketcp ports that the udp,tcp dialing preference never uses.
    expect(DEFAULT_NETWORK_CONFIG().listener_urls).toEqual([
      'tcp://0.0.0.0:11010',
      'udp://0.0.0.0:11010',
      'wg://0.0.0.0:11011',
    ])
  })

  it('starts with no network secret (the warning covers this, not a default)', () => {
    // Kept empty on purpose: inventing a secret the operator cannot see would be
    // worse. `Config.vue` renders an inline warning via `showEmptySecretWarning`.
    expect(DEFAULT_NETWORK_CONFIG().network_secret).toBe('')
  })
})