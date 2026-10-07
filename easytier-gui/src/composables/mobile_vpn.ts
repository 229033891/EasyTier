import type { NetworkTypes } from 'easytier-frontend-lib'
import { addPluginListener } from '@tauri-apps/api/core'
import { Utils } from 'easytier-frontend-lib'
import {
  consume_vpn_tile_action,
  get_vpn_status,
  prepare_vpn,
  start_vpn,
  stop_vpn,
  type VpnTileAction,
} from 'tauri-plugin-vpnservice-api'
import { collectNetworkInfo, getConfig, listNetworkInstanceIds, setTunFd } from './backend'

type Route = NetworkTypes.Route

interface vpnStatus {
  running: boolean
  ipv4Addr: string | null | undefined
  ipv4Cidr: number | null | undefined
  /** Virtual IPv6 with prefix, e.g. `fd00::1/64`. */
  ipv6Addr: string | null | undefined
  routes: string[]
  dns: string | null | undefined
}

let vpnReconcileTimer: ReturnType<typeof setTimeout> | null = null
const VPN_RECONCILE_INTERVAL_MS = 2000
const VPN_RECONCILE_MAX_ATTEMPTS = 60

/**
 * Grace window after a start/stop begins. Native TUN teardown during a normal
 * replace closes the fd on purpose, which makes the core report a TUN error —
 * that must not be mistaken for a failure and trigger another restart.
 */
const VPN_TRANSITION_GRACE_MS = 15000
/** Floor between two TUN-error driven rebuilds, so a permanently bad fd cannot flap. */
const VPN_TUN_ERROR_REBUILD_INTERVAL_MS = 30000

let desiredVpnInstanceId: string | undefined
let activeVpnInstanceId: string | undefined
let vpnReconcileGeneration = 0
let vpnReconcileAttempts = 0
let vpnReconcileQueue: Promise<void> = Promise.resolve()
let vpnPermissionRequest: Promise<boolean> | null = null
let vpnTileActionHandler: ((action: VpnTileAction) => Promise<void>) | undefined
let vpnTileActionQueue: Promise<void> = Promise.resolve()
let vpnTransitionDeadline = 0
let lastTunErrorRebuildAt = 0

const curVpnStatus: vpnStatus = {
  running: false,
  ipv4Addr: undefined,
  ipv4Cidr: undefined,
  ipv6Addr: undefined,
  routes: [],
  dns: undefined,
}

/**
 * Routes currently applied via Android VpnService (empty when VPN is down).
 * When `forInstanceId` is set and another instance owns the VPN, returns [].
 */
export function getMobileVpnInstalledRoutes(forInstanceId?: string): string[] {
  if (!curVpnStatus.running)
    return []
  if (forInstanceId && activeVpnInstanceId && forInstanceId !== activeVpnInstanceId)
    return []

  const routes = [...normalizeRouteList(curVpnStatus.routes)]
  // VpnService installs the virtual address via addAddress; surface the subnet
  // alongside addRoute prefixes so Status ROUTE matches what users expect.
  if (curVpnStatus.ipv4Addr && curVpnStatus.ipv4Cidr != null) {
    routes.push(`${curVpnStatus.ipv4Addr}/${curVpnStatus.ipv4Cidr}`)
  }
  return Array.from(new Set(routes)).sort()
}

/** DNS server VpnService pushed (empty when none / VPN down / wrong owner). */
export function getMobileVpnPushedDns(forInstanceId?: string): string {
  if (!curVpnStatus.running)
    return ''
  if (forInstanceId && activeVpnInstanceId && forInstanceId !== activeVpnInstanceId)
    return ''
  return typeof curVpnStatus.dns === 'string' ? curVpnStatus.dns.trim() : ''
}

/**
 * Coerce plugin/native route payloads to string[].
 *
 * Fail-closed: only Array values are accepted. A bare string (including Java
 * Array.toString() junk like "[Ljava.lang.String;@…") is discarded — never
 * treated as a single route and never spread into characters. Do not "fix"
 * this to `typeof routes === 'string' ? [routes] : …`.
 */
export function normalizeRouteList(routes: unknown): string[] {
  if (Array.isArray(routes)) {
    return routes
      .filter((route): route is string => typeof route === 'string')
      .map(route => route.trim())
      .filter(route => route.length > 0)
  }
  return []
}

/** True when `addr/prefix` is safe for Android VpnService.Builder.addRoute. */
export function isValidVpnRouteCidr(cidr: string): boolean {
  const parts = cidr.split('/')
  if (parts.length !== 2)
    return false
  const [addr, prefixRaw] = parts
  if (!addr || !prefixRaw || !/^\d+$/.test(prefixRaw))
    return false
  const prefix = Number(prefixRaw)
  if (!Number.isInteger(prefix) || prefix < 0)
    return false

  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(addr)) {
    if (prefix > 32)
      return false
    return addr.split('.').every((octet) => {
      const n = Number(octet)
      return Number.isInteger(n) && n >= 0 && n <= 255
    })
  }

  // Loose IPv6: require a colon and a plausible prefix.
  return addr.includes(':') && !addr.includes(' ') && prefix <= 128
}

/**
 * Normalize one route entry for VpnService. Bare IPv4 may get `/32`;
 * anything that is not a valid CIDR afterwards is dropped (warn).
 */
function toVpnRouteCidr(raw: string): string | undefined {
  let cidr = raw.trim()
  if (!cidr)
    return undefined
  if (!cidr.includes('/')) {
    if (!/^\d{1,3}(\.\d{1,3}){3}$/.test(cidr)) {
      console.warn('skip vpn route: not a CIDR', raw)
      return undefined
    }
    cidr = `${cidr}/32`
  }
  if (!isValidVpnRouteCidr(cidr)) {
    console.warn('skip vpn route: invalid CIDR', raw)
    return undefined
  }
  return cidr
}

/** Format VpnService routes into the L2 proxy_cidr_route_sync summary string. */
export function formatMobileVpnRouteSync(
  routes: string[] = getMobileVpnInstalledRoutes(),
): string {
  const joined = routes.length ? routes.join(',') : '-'
  const exit = routes.some(route => route === '0.0.0.0/0')
  return `desired=[${joined}] installed=[${joined}] exit=${exit}`
}

/**
 * Android L2 ifcfg is a no-op, so core always reports desired=[-] installed=[-].
 * Replace that placeholder with VpnService-authoritative routes (mirrors OHOS annotate).
 * Only attributes routes to the instance that currently owns the VPN.
 */
export function annotateNetworkInfoWithMobileVpnRoutes<T extends { proxy_cidr_route_sync?: string | null }>(
  info: T,
  instanceId?: string,
): T {
  info.proxy_cidr_route_sync = formatMobileVpnRouteSync(
    getMobileVpnInstalledRoutes(instanceId),
  )
  return info
}

/** Refresh native VpnService snapshot then annotate running info for Status display. */
export async function annotateNetworkInfoFromVpnService<T extends { proxy_cidr_route_sync?: string | null }>(
  info: T,
  instanceId?: string,
): Promise<T> {
  try {
    syncVpnStatusFromNative(await get_vpn_status())
  }
  catch (e) {
    console.warn('refresh vpn status before route annotate failed', e)
  }
  return annotateNetworkInfoWithMobileVpnRoutes(info, instanceId)
}

export function setMobileVpnTileActionHandler(
  handler?: (action: VpnTileAction) => Promise<void>,
) {
  vpnTileActionHandler = handler
}

export async function consumePendingMobileVpnTileAction() {
  const handler = vpnTileActionHandler
  if (!handler) {
    return false
  }

  const action = (await consume_vpn_tile_action())?.action
  if (action !== 'start' && action !== 'stop') {
    return false
  }

  const run = vpnTileActionQueue
    .catch(error => console.error('previous VPN tile action failed', error))
    .then(() => handler(action))
  vpnTileActionQueue = run.catch(error => console.error('VPN tile action failed', error))
  await run
  return true
}

async function requestVpnPermissionOnce() {
  console.log('prepare vpn')
  const prepare_ret = await prepare_vpn()
  console.log('prepare vpn', JSON.stringify((prepare_ret)))
  if (prepare_ret?.errorMsg?.length) {
    throw new Error(prepare_ret.errorMsg)
  }

  const granted = prepare_ret?.granted ?? true
  if (!granted) {
    console.info('vpn permission request was denied or dismissed')
  }

  return granted
}

async function requestVpnPermission() {
  if (vpnPermissionRequest) {
    console.log('reuse pending vpn permission request')
    return await vpnPermissionRequest
  }

  const request = requestVpnPermissionOnce()
  vpnPermissionRequest = request
  try {
    return await request
  }
  finally {
    if (vpnPermissionRequest === request) {
      vpnPermissionRequest = null
    }
  }
}

function clearVpnReconcileTimer() {
  if (vpnReconcileTimer) {
    clearTimeout(vpnReconcileTimer)
    vpnReconcileTimer = null
  }
}

function beginVpnReconcile(instanceId?: string) {
  clearVpnReconcileTimer()
  desiredVpnInstanceId = instanceId
  vpnReconcileAttempts = 0
  vpnReconcileGeneration += 1
  return vpnReconcileGeneration
}

function isCurrentVpnReconcile(instanceId: string, generation: number) {
  return desiredVpnInstanceId === (instanceId || undefined) && vpnReconcileGeneration === generation
}

function scheduleVpnReconcile(instanceId: string, generation: number, reason: string) {
  if (!isCurrentVpnReconcile(instanceId, generation))
    return

  if (vpnReconcileAttempts >= VPN_RECONCILE_MAX_ATTEMPTS) {
    console.error(
      'vpn service reconcile stopped after maximum attempts',
      instanceId,
      VPN_RECONCILE_MAX_ATTEMPTS,
      reason,
    )
    return
  }

  clearVpnReconcileTimer()
  vpnReconcileAttempts += 1
  console.log(
    'vpn service is not ready, retrying',
    JSON.stringify({
      instanceId,
      attempt: vpnReconcileAttempts,
      maxAttempts: VPN_RECONCILE_MAX_ATTEMPTS,
      delayMs: VPN_RECONCILE_INTERVAL_MS,
      reason,
    }),
  )
  vpnReconcileTimer = setTimeout(() => {
    vpnReconcileTimer = null
    void enqueueVpnReconcile(instanceId, generation)
  }, VPN_RECONCILE_INTERVAL_MS)
}

function resetVpnConfigStatus() {
  curVpnStatus.ipv4Addr = undefined
  curVpnStatus.ipv4Cidr = undefined
  curVpnStatus.ipv6Addr = undefined
  curVpnStatus.routes = []
  curVpnStatus.dns = undefined
}

function syncVpnStatusFromNative(status: Awaited<ReturnType<typeof get_vpn_status>>) {
  curVpnStatus.running = status?.running ?? false
  if (!curVpnStatus.running) {
    activeVpnInstanceId = undefined
    resetVpnConfigStatus()
    return
  }

  const ipv4WithCidr = status?.ipv4Addr
  if (ipv4WithCidr?.length) {
    const [ipv4Addr, cidr] = ipv4WithCidr.split('/')
    curVpnStatus.ipv4Addr = ipv4Addr

    const parsedCidr = Number(cidr)
    curVpnStatus.ipv4Cidr = Number.isInteger(parsedCidr) ? parsedCidr : undefined
  }
  else {
    curVpnStatus.ipv4Addr = undefined
    curVpnStatus.ipv4Cidr = undefined
  }

  curVpnStatus.routes = normalizeRouteList(status?.routes)
  curVpnStatus.dns = status?.dns ?? undefined
}

async function waitVpnStatus(target_status: boolean, timeout_sec: number) {
  const start_time = Date.now()
  while (curVpnStatus.running !== target_status) {
    if (Date.now() - start_time > timeout_sec * 1000) {
      // Event may have been missed; recheck native before failing (A5).
      try {
        syncVpnStatusFromNative(await get_vpn_status())
      }
      catch (e) {
        console.warn('vpn status recheck failed', e)
      }
      if (curVpnStatus.running === target_status) {
        return
      }
      throw new Error('wait vpn status timeout')
    }
    await new Promise(r => setTimeout(r, 50))
  }
}

/** Match desktop TUN MTU: config.mtu (default 1380), minus 20 when encryption is on (A11). */
function resolveVpnMtu(config: { mtu?: number | null, disable_encryption?: boolean | null }) {
  let mtu = typeof config.mtu === 'number' && config.mtu > 0 ? config.mtu : 1380
  if (!config.disable_encryption) {
    mtu = Math.max(mtu - 20, 576)
  }
  return mtu
}

/** Format `MyNodeInfo.virtual_ipv6` for Android VpnService `addAddress` (R3). */
export function formatVirtualIpv6ForVpn(
  virtualIpv6?: NetworkTypes.Ipv6Inet | null,
): string | undefined {
  if (!virtualIpv6?.address)
    return undefined
  const addr = Utils.ipv6ToString(virtualIpv6.address)
  if (!addr?.length)
    return undefined
  const prefix = virtualIpv6.network_length ?? 128
  return `${addr}/${prefix}`
}

async function doStopVpn(force = false) {
  const wasRunning = curVpnStatus.running
  if (!force && !wasRunning) {
    activeVpnInstanceId = undefined
    return
  }
  // Native teardown closes the TUN fd, which makes the core report a TUN error.
  // Suppress error-driven rebuilds while a deliberate transition is in flight (R1).
  vpnTransitionDeadline = Date.now() + VPN_TRANSITION_GRACE_MS
  console.log('stop vpn')
  const stop_ret = await stop_vpn()
  console.log('stop vpn', JSON.stringify((stop_ret)))
  if (wasRunning) {
    await waitVpnStatus(false, 8)
  }

  activeVpnInstanceId = undefined
  resetVpnConfigStatus()
}

async function doStartVpn(
  instanceId: string,
  ipv4Addr: string,
  cidr: number,
  routes: string[],
  dns?: string,
  mtu = 1360,
  ipv6WithPrefix?: string,
) {
  if (curVpnStatus.running) {
    return
  }

  // `start_vpn` internally replaces the TUN (stopInternal → establish), so the
  // old fd close will surface as a core TUN error. Cover the whole transition (R1).
  vpnTransitionDeadline = Date.now() + VPN_TRANSITION_GRACE_MS

  console.log('start vpn service', ipv4Addr, cidr, routes, dns, mtu, ipv6WithPrefix)
  const request = {
    ipv4Addr: `${ipv4Addr}/${cidr}`,
    ...(ipv6WithPrefix ? { ipv6Addr: ipv6WithPrefix } : {}),
    routes,
    dns,
    disallowedApplications: ['com.kkrainbow.easytier'],
    mtu,
  }

  // Claim ownership before waiting for vpn_service_start so a slow establish
  // cannot be mis-read as "owner changed" by the next reconcile (A5).
  // Note: start_vpn → stopInternal emits vpn_service_stop which clears the
  // owner via onVpnServiceStop; re-assert after each start_vpn call.
  activeVpnInstanceId = instanceId

  try {
    let start_ret = await start_vpn(request)
    activeVpnInstanceId = instanceId
    console.log('start vpn response', JSON.stringify(start_ret))
    if (start_ret?.errorMsg === 'need_prepare') {
      const granted = await requestVpnPermission()
      if (!granted) {
        throw new Error('vpn_permission_denied')
      }
      start_ret = await start_vpn(request)
      activeVpnInstanceId = instanceId
      console.log('start vpn retry response', JSON.stringify(start_ret))
    }

    if (start_ret?.errorMsg?.length) {
      throw new Error(start_ret.errorMsg)
    }
    await waitVpnStatus(true, 8)

    curVpnStatus.ipv4Addr = ipv4Addr
    curVpnStatus.ipv4Cidr = cidr
    curVpnStatus.ipv6Addr = ipv6WithPrefix
    curVpnStatus.routes = normalizeRouteList(routes)
    curVpnStatus.dns = dns
    activeVpnInstanceId = instanceId
  }
  catch (e) {
    if (activeVpnInstanceId === instanceId && !curVpnStatus.running) {
      activeVpnInstanceId = undefined
    }
    throw e
  }
}

async function onVpnServiceStart(payload: any) {
  console.log('vpn service start', JSON.stringify(payload))
  curVpnStatus.running = true
  if (payload.fd) {
    try {
      await setTunFd(payload.fd, activeVpnInstanceId)
    }
    catch (e) {
      // Do not leave running=true when core never got the fd (R1 partial).
      console.error('set tun fd failed', e)
      curVpnStatus.running = false
      activeVpnInstanceId = undefined
      resetVpnConfigStatus()
    }
  }
}

async function onVpnServiceStop(payload: any) {
  console.log('vpn service stop', JSON.stringify(payload))
  curVpnStatus.running = false
  // Keep activeVpnInstanceId: start_vpn's stopInternal emits this event while
  // replacing the TUN, and clearing the owner here races with vpn_service_start
  // → setTunFd. Intentional stops clear the owner in doStopVpn.
  resetVpnConfigStatus()
}

async function registerVpnServiceListener() {
  console.log('register vpn service listener')
  await addPluginListener(
    'vpnservice',
    'vpn_service_start',
    onVpnServiceStart,
  )

  await addPluginListener(
    'vpnservice',
    'vpn_service_stop',
    onVpnServiceStop,
  )

  await addPluginListener(
    'vpnservice',
    'vpn_tile_action',
    () => {
      void consumePendingMobileVpnTileAction().catch((error) => {
        console.error('consume VPN tile action failed', error)
      })
    },
  )
}

function isDefaultIpv4Route(cidr: string): boolean {
  const normalized = cidr.includes('/') ? cidr : `${cidr}/32`
  const [ip, len] = normalized.split('/')
  return ip === '0.0.0.0' && Number(len) === 0
}

function routeVip(route: Route): string | null {
  const addr = route.ipv4_addr
  if (!addr)
    return null
  if (typeof addr === 'string')
    return addr.split('/')[0] ?? null
  const address = (addr as { address?: { toString?: () => string } | string }).address
  if (typeof address === 'string')
    return address
  if (address && typeof address.toString === 'function')
    return address.toString()
  return null
}

/** Mirror desktop: only treat exit as usable when its VIP appears in live routes with a next hop. */
function hasReachableExit(routes: Route[] | undefined, exitNodes: string[]): boolean {
  for (const exit of exitNodes) {
    const exitIp = exit.trim().split('/')[0]
    if (!exitIp)
      continue
    for (const route of routes ?? []) {
      if (routeVip(route) === exitIp && Number(route.next_hop_peer_id) > 0)
        return true
    }
  }
  return false
}

function getRoutesForVpn(routes: Route[] | undefined, node_config: NetworkTypes.NetworkConfig): string[] {
  const ret = []
  const exitNodes = node_config.exit_nodes ?? []
  const localExitDefault = hasReachableExit(routes, exitNodes)
  const allowPeerDefault = node_config.allow_peer_default_without_exit === true

  for (const r of routes ?? []) {
    for (const raw of normalizeRouteList(r.proxy_cidrs)) {
      const cidr = toVpnRouteCidr(raw)
      if (!cidr)
        continue
      if (!localExitDefault && !allowPeerDefault && isDefaultIpv4Route(cidr)) {
        continue
      }
      ret.push(cidr)
    }
  }

  for (const raw of normalizeRouteList(node_config.routes)) {
    const cidr = toVpnRouteCidr(raw)
    if (cidr)
      ret.push(cidr)
  }

  if (localExitDefault) {
    ret.push('0.0.0.0/0')
  }

  if (node_config.enable_magic_dns) {
    ret.push('100.100.100.53/32')
  }

  // sort and dedup
  return Array.from(new Set(ret)).sort()
}

async function stopVpnOwnedByOtherInstance(instanceId: string, generation: number) {
  if (!isCurrentVpnReconcile(instanceId, generation))
    return false

  if (curVpnStatus.running && activeVpnInstanceId !== instanceId) {
    console.warn('vpn service owner changed', activeVpnInstanceId, instanceId)
    await doStopVpn()
  }

  return isCurrentVpnReconcile(instanceId, generation)
}

async function reconcileNetworkInstance(instanceId: string, generation: number) {
  if (!isCurrentVpnReconcile(instanceId, generation))
    return

  clearVpnReconcileTimer()

  if (!instanceId) {
    console.warn('vpn service skipped because instance id is empty')
    if (curVpnStatus.running) {
      await doStopVpn()
    }
    return
  }
  const config = await getConfig(instanceId)
  if (!isCurrentVpnReconcile(instanceId, generation))
    return

  console.log('vpn service loaded config', instanceId, JSON.stringify({
    no_tun: config.no_tun,
    dhcp: config.dhcp,
    enable_magic_dns: config.enable_magic_dns,
  }))
  if (config.no_tun) {
    console.log('vpn service skipped because no_tun is enabled', instanceId)
    if (activeVpnInstanceId === instanceId) {
      await doStopVpn()
    }
    return
  }

  if (!await stopVpnOwnedByOtherInstance(instanceId, generation))
    return

  let curNetworkInfo
  try {
    curNetworkInfo = (await collectNetworkInfo(instanceId))?.info?.map?.[instanceId]
  }
  catch (e) {
    console.warn('vpn service network info query failed', instanceId, e)
    scheduleVpnReconcile(instanceId, generation, 'network_info_query_failed')
    return
  }

  if (!isCurrentVpnReconcile(instanceId, generation))
    return

  if (!curNetworkInfo) {
    scheduleVpnReconcile(instanceId, generation, 'network_info_unavailable')
    return
  }

  if (curNetworkInfo.error_msg?.length) {
    console.warn('vpn service skipped because network instance failed', instanceId, curNetworkInfo.error_msg)
    vpnReconcileAttempts = 0
    await doStopVpn()
    return
  }

  const virtualIpv4 = curNetworkInfo.my_node_info?.virtual_ipv4
  const virtual_ip = virtualIpv4?.address?.addr ? Utils.ipv4ToString(virtualIpv4.address) : undefined

  if (!virtual_ip || !virtual_ip.length) {
    scheduleVpnReconcile(
      instanceId,
      generation,
      config.dhcp ? 'dhcp_ipv4_unavailable' : 'static_ipv4_unavailable',
    )
    return
  }

  vpnReconcileAttempts = 0

  let network_length = virtualIpv4?.network_length
  if (!network_length) {
    network_length = 24
  }

  const routes = getRoutesForVpn(curNetworkInfo?.routes, config)

  const dns = config.enable_magic_dns ? '100.100.100.53' : undefined
  const virtualIpv6WithPrefix = formatVirtualIpv6ForVpn(curNetworkInfo.my_node_info?.virtual_ipv6)

  const ipChanged = virtual_ip !== curVpnStatus.ipv4Addr
  const cidrChanged = network_length !== curVpnStatus.ipv4Cidr
  const ipv6Changed = virtualIpv6WithPrefix !== curVpnStatus.ipv6Addr
  const routesChanged = JSON.stringify(routes) !== JSON.stringify(curVpnStatus.routes)
  const dnsChanged = dns != curVpnStatus.dns
  const configChanged = ipChanged || cidrChanged || ipv6Changed || routesChanged || dnsChanged
  const shouldStartVpn = !curVpnStatus.running

  if (shouldStartVpn || configChanged) {
    console.info('vpn service virtual ip changed', JSON.stringify(curVpnStatus), virtual_ip)
    if (curVpnStatus.running) {
      try {
        await doStopVpn()
      }
      catch (e) {
        console.error(e)
      }
    }

    try {
      if (!isCurrentVpnReconcile(instanceId, generation))
        return

      await doStartVpn(
        instanceId,
        virtual_ip,
        network_length,
        routes,
        dns,
        resolveVpnMtu(config),
        virtualIpv6WithPrefix,
      )
      if (!isCurrentVpnReconcile(instanceId, generation) && activeVpnInstanceId === instanceId) {
        await doStopVpn()
      }
    }
    catch (e) {
      if (e instanceof Error && e.message === 'need_prepare') {
        console.info('vpn permission is required before starting the Android VPN service')
        return
      }
      if (e instanceof Error && e.message === 'vpn_permission_denied') {
        console.info('vpn permission request was denied or dismissed')
        return
      }
      console.error('start vpn service failed', e)
    }
  }
}

function enqueueVpnTask(task: () => Promise<void>) {
  const run = vpnReconcileQueue
    .catch((e) => {
      console.error('previous vpn service reconcile failed', e)
    })
    .then(task)
  vpnReconcileQueue = run.catch((e) => {
    console.error('vpn service reconcile failed', e)
  })
  return run
}

function enqueueVpnReconcile(instanceId: string, generation: number) {
  return enqueueVpnTask(() => reconcileNetworkInstance(instanceId, generation))
}

export async function onNetworkInstanceChange(instanceId: string) {
  const generation = beginVpnReconcile(instanceId || undefined)

  if (instanceId && await isNoTunEnabled(instanceId)) {
    if (vpnReconcileGeneration !== generation)
      return

    if (activeVpnInstanceId === instanceId) {
      desiredVpnInstanceId = undefined
      await enqueueVpnReconcile('', generation)
      return
    }

    desiredVpnInstanceId = activeVpnInstanceId
    if (activeVpnInstanceId) {
      await enqueueVpnReconcile(activeVpnInstanceId, generation)
    }
    return
  }

  if (vpnReconcileGeneration !== generation)
    return

  await enqueueVpnReconcile(instanceId, generation)
}

export async function onNetworkInstanceUpdate(instanceId: string) {
  if (!instanceId || instanceId !== desiredVpnInstanceId)
    return

  const generation = beginVpnReconcile(instanceId)
  await enqueueVpnReconcile(instanceId, generation)
}

/**
 * The core lost its TUN device while the native VpnService may still report
 * `running` (see `GlobalCtxEvent::TunDeviceError` → `tun_device_error`). The
 * only recovery is a full teardown + rebuild, because a dead fd cannot be
 * revived by the core itself.
 *
 * Two guards keep this from turning into a restart loop:
 * - `vpnTransitionDeadline`: a normal replace closes the old fd on purpose, so
 *   errors reported during a start/stop window are expected and ignored.
 * - `VPN_TUN_ERROR_REBUILD_INTERVAL_MS`: a permanently bad fd would otherwise
 *   flap forever, so rebuilds are rate-limited.
 */
export async function handleMobileTunDeviceError(instanceId: string) {
  if (!instanceId)
    return

  const now = Date.now()
  if (now < vpnTransitionDeadline) {
    console.info('ignore TUN device error during VPN transition', instanceId)
    return
  }

  // `lastTunErrorRebuildAt` starts at 0 (= "never rebuilt"), so the sentinel
  // check is required — otherwise the very first TUN error after launch would
  // be swallowed by the throttle window.
  if (lastTunErrorRebuildAt > 0 && now - lastTunErrorRebuildAt < VPN_TUN_ERROR_REBUILD_INTERVAL_MS) {
    console.warn(
      'throttle TUN device error rebuild',
      instanceId,
      VPN_TUN_ERROR_REBUILD_INTERVAL_MS,
    )
    return
  }

  // Only act on the instance we actually drive; a stale instance that the GUI
  // already stopped must not resurrect the VPN.
  if (instanceId !== activeVpnInstanceId && instanceId !== desiredVpnInstanceId) {
    console.info('ignore TUN device error from unrelated instance', instanceId)
    return
  }

  lastTunErrorRebuildAt = now
  console.warn('core TUN device failed; tearing down and rebuilding VPN', instanceId)

  await enqueueVpnTask(async () => {
    try {
      await doStopVpn(true)
    }
    catch (e) {
      console.error('stop vpn after TUN device error failed', e)
    }
  })

  if (instanceId === desiredVpnInstanceId) {
    await onNetworkInstanceUpdate(instanceId)
  }
}

async function isNoTunEnabled(instanceId: string | undefined) {
  if (!instanceId) {
    return false
  }
  return (await getConfig(instanceId)).no_tun ?? false
}

async function findRunningTunInstanceId() {
  const instanceIds = await listNetworkInstanceIds()
  const runningIds = (instanceIds.running_inst_ids ?? []).map(Utils.UuidToStr)
  console.log('vpn service sync running instances', JSON.stringify(runningIds))

  for (const instanceId of runningIds) {
    if (await isNoTunEnabled(instanceId)) {
      continue
    }

    return instanceId
  }

  return undefined
}

export async function initMobileVpnService() {
  await registerVpnServiceListener()
  startBackgroundVpnSync()
}

/**
 * Background sync body. Must NOT go through `enqueueVpnTask`: that helper
 * chains onto the serial queue, and re-entering it from inside a queued task
 * would deadlock. `syncMobileVpnService` is safe to run directly — its
 * follow-up work (`onNetworkInstanceChange`) enqueues itself, and overlapping
 * runs are disambiguated by reconcile generations.
 */
function runBackgroundVpnSync() {
  void syncMobileVpnService().catch((error) => {
    console.error('background vpn service sync failed', error)
  })
}

let backgroundVpnSyncTimer: ReturnType<typeof setInterval> | null = null
/**
 * Interval (ms) for background VPN reconciliation on mobile.
 *
 * Native-side changes that bypass the GUI command path — e.g. disabling a
 * network from the web console, which destroys the core instance without
 * emitting any tauri event — would otherwise leave a stale VpnService
 * (routes + pushed DNS) behind. The sync is cheap (one status + one
 * instance-list query) and every run is idempotent: no drift means no-op.
 */
const BACKGROUND_VPN_SYNC_INTERVAL_MS = 10000

function startBackgroundVpnSync() {
  if (backgroundVpnSyncTimer) {
    return
  }
  backgroundVpnSyncTimer = setInterval(runBackgroundVpnSync, BACKGROUND_VPN_SYNC_INTERVAL_MS)
}

export function stopBackgroundVpnSync() {
  if (backgroundVpnSyncTimer) {
    clearInterval(backgroundVpnSyncTimer)
    backgroundVpnSyncTimer = null
  }
}

export async function prepareVpnService(instanceId: string) {
  if (await isNoTunEnabled(instanceId)) {
    return
  }

  const generation = beginVpnReconcile(instanceId)
  const stopPreviousOwner = enqueueVpnTask(async () => {
    await stopVpnOwnedByOtherInstance(instanceId, generation)
  })
  await Promise.all([requestVpnPermission(), stopPreviousOwner])
}

export async function syncMobileVpnService() {
  syncVpnStatusFromNative(await get_vpn_status())
  const instanceId = await findRunningTunInstanceId()
  if (instanceId) {
    console.log('vpn service sync selected instance', instanceId)
    await onNetworkInstanceChange(instanceId)
    return
  }

  await onNetworkInstanceChange('')
}
