import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => {
  const listeners = new Map<string, (payload: unknown) => Promise<void>>()
  const configs = new Map<string, Record<string, unknown>>()
  const networkInfo = new Map<string, unknown>()

  return {
    listeners,
    configs,
    networkInfo,
    addPluginListener: vi.fn(async (_plugin: string, event: string, listener: (payload: unknown) => Promise<void>) => {
      listeners.set(event, listener)
    }),
    collectNetworkInfo: vi.fn(async (instanceId: string) => ({
      info: { map: { [instanceId]: networkInfo.get(instanceId) } },
    })),
    consumeVpnTileAction: vi.fn(async () => ({})),
    getConfig: vi.fn(async (instanceId: string) => configs.get(instanceId)),
    getVpnStatus: vi.fn<() => Promise<Record<string, unknown>>>(async () => ({ running: false })),
    listNetworkInstanceIds: vi.fn<() => Promise<{ running_inst_ids: unknown[] }>>(async () => ({ running_inst_ids: [] })),
    prepareVpn: vi.fn(async () => ({ granted: true })),
    setTunFd: vi.fn(async () => undefined),
    notifyUnderlayNetworkChanged: vi.fn(async () => 0),
    updateNetworkConfigState: vi.fn(async () => undefined),
    startVpn: vi.fn(async () => {
      await listeners.get('vpn_service_start')?.({ fd: 1 })
      return {}
    }),
    stopVpn: vi.fn(async () => {
      // App-initiated stop must carry reason=app so we do not disable the instance.
      await listeners.get('vpn_service_stop')?.({ reason: 'app' })
      return {}
    }),
  }
})

vi.mock('@tauri-apps/api/core', () => ({
  addPluginListener: mocks.addPluginListener,
}))

vi.mock('easytier-frontend-lib', async () => {
  const { IPv6 } = await import('ip-num/IPNumber')
  return {
    Utils: {
      UuidToStr: (value: unknown) => String(value),
      ipv4ToString: (address: { addr: string }) => address.addr,
      ipv6ToString: (address: { part1: number, part2: number, part3: number, part4: number }) => {
        return IPv6.fromBigInt(
          (BigInt(address.part1 ?? 0) << BigInt(96))
          + (BigInt(address.part2 ?? 0) << BigInt(64))
          + (BigInt(address.part3 ?? 0) << BigInt(32))
          + BigInt(address.part4 ?? 0),
        ).toString()
      },
    },
  }
})

vi.mock('tauri-plugin-vpnservice-api', () => ({
  consume_vpn_tile_action: mocks.consumeVpnTileAction,
  get_vpn_status: mocks.getVpnStatus,
  prepare_vpn: mocks.prepareVpn,
  start_vpn: mocks.startVpn,
  stop_vpn: mocks.stopVpn,
}))

vi.mock('./backend', () => ({
  collectNetworkInfo: mocks.collectNetworkInfo,
  getConfig: mocks.getConfig,
  listNetworkInstanceIds: mocks.listNetworkInstanceIds,
  notifyUnderlayNetworkChanged: mocks.notifyUnderlayNetworkChanged,
  setTunFd: mocks.setTunFd,
  updateNetworkConfigState: mocks.updateNetworkConfigState,
}))

function setConfig(instanceId: string, noTun = false) {
  mocks.configs.set(instanceId, {
    no_tun: noTun,
    dhcp: false,
    enable_magic_dns: false,
    routes: [],
  })
}

function setReady(instanceId: string, ipv4: string) {
  mocks.networkInfo.set(instanceId, {
    my_node_info: {
      virtual_ipv4: {
        address: { addr: ipv4 },
        network_length: 24,
      },
    },
    routes: [],
  })
}

async function loadVpnModule() {
  const mobileVpn = await import('./mobile_vpn')
  await mobileVpn.initMobileVpnService()
  return mobileVpn
}

beforeEach(() => {
  vi.useFakeTimers()
  // Each loadVpnModule() starts a background sync interval. resetModules() does
  // not dispose it, so without this the intervals pile up across tests and fire
  // during a later test's clock advance.
  vi.clearAllTimers()
  vi.resetModules()
  mocks.listeners.clear()
  mocks.configs.clear()
  mocks.networkInfo.clear()
  mocks.addPluginListener.mockClear()
  mocks.collectNetworkInfo.mockClear()
  mocks.consumeVpnTileAction.mockReset()
  mocks.consumeVpnTileAction.mockResolvedValue({})
  mocks.getConfig.mockClear()
  mocks.getVpnStatus.mockReset()
  mocks.getVpnStatus.mockResolvedValue({ running: false })
  mocks.listNetworkInstanceIds.mockReset()
  mocks.listNetworkInstanceIds.mockResolvedValue({ running_inst_ids: [] })
  mocks.prepareVpn.mockClear()
  mocks.setTunFd.mockClear()
  mocks.notifyUnderlayNetworkChanged.mockReset()
  mocks.notifyUnderlayNetworkChanged.mockResolvedValue(0)
  mocks.startVpn.mockClear()
  mocks.stopVpn.mockClear()
  mocks.updateNetworkConfigState.mockReset()
  mocks.updateNetworkConfigState.mockResolvedValue(undefined)
})

describe('mobile VPN virtual IPv6', () => {
  it('formats MyNodeInfo.virtual_ipv6 for VpnService addAddress', async () => {
    const vpn = await loadVpnModule()
    expect(vpn.formatVirtualIpv6ForVpn(undefined)).toBeUndefined()
    // Proto Ipv6Addr stores 16 bytes as four big-endian u32 chunks (see common.proto).
    expect(vpn.formatVirtualIpv6ForVpn({
      address: { part1: 0xFD00_0000, part2: 0, part3: 0, part4: 1 },
      network_length: 64,
    })).toBe('fd00:0:0:0:0:0:0:1/64')
  })

  it('passes ipv6Addr to start_vpn when network info includes virtual_ipv6', async () => {
    setConfig('A')
    setReady('A', '10.0.0.1')
    mocks.networkInfo.set('A', {
      my_node_info: {
        virtual_ipv4: {
          address: { addr: '10.0.0.1' },
          network_length: 24,
        },
        virtual_ipv6: {
          address: { part1: 0xFD00_0000, part2: 0, part3: 0, part4: 1 },
          network_length: 64,
        },
      },
      routes: [],
    })
    const vpn = await loadVpnModule()
    await vpn.onNetworkInstanceChange('A')
    expect(mocks.startVpn).toHaveBeenCalledWith(expect.objectContaining({
      ipv4Addr: '10.0.0.1/24',
      ipv6Addr: 'fd00:0:0:0:0:0:0:1/64',
    }))
  })
})

describe('mobile VPN route sync annotate', () => {
  it('formats VpnService routes like L2 proxy_cidr_route_sync', async () => {
    const vpn = await loadVpnModule()
    expect(vpn.formatMobileVpnRouteSync([])).toBe('desired=[-] installed=[-] exit=false')
    expect(vpn.formatMobileVpnRouteSync(['10.0.0.0/24', '0.0.0.0/0'])).toBe(
      'desired=[10.0.0.0/24,0.0.0.0/0] installed=[10.0.0.0/24,0.0.0.0/0] exit=true',
    )
    expect(vpn.formatMobileVpnRouteSync(['10.0.0.0/24'])).toBe(
      'desired=[10.0.0.0/24] installed=[10.0.0.0/24] exit=false',
    )
    expect(vpn.annotateNetworkInfoWithMobileVpnRoutes({
      proxy_cidr_route_sync: 'desired=[-] installed=[-] exit=false',
    }, 'other-instance').proxy_cidr_route_sync).toBe('desired=[-] installed=[-] exit=false')
  })

  it('does not treat Java Array.toString junk as route characters', async () => {
    const vpn = await loadVpnModule()
    // Regression: get_vpn_status used to return Kotlin Array.toString()
    // ("[Ljava.lang.String;@…") and spreading that string produced ./32 chips.
    expect(vpn.normalizeRouteList('[Ljava.lang.String;@1689abe')).toEqual([])
    expect(vpn.normalizeRouteList('10.0.0.0/24')).toEqual([])
    expect(vpn.normalizeRouteList(['10.0.0.0/24', 1, null, ' 1.2.3.0/24 '])).toEqual([
      '10.0.0.0/24',
      '1.2.3.0/24',
    ])
    expect(vpn.isValidVpnRouteCidr('10.0.0.0/24')).toBe(true)
    expect(vpn.isValidVpnRouteCidr('10.0.0.0')).toBe(false)
    expect(vpn.isValidVpnRouteCidr('not-a-cidr/32')).toBe(false)
    expect(vpn.isValidVpnRouteCidr('10.0.0.0/99')).toBe(false)

    setConfig('A')
    setReady('A', '10.0.0.1')
    await vpn.onNetworkInstanceChange('A')
    expect(mocks.startVpn).toHaveBeenCalledTimes(1)

    mocks.getVpnStatus.mockResolvedValue({
      running: true,
      ipv4Addr: '10.0.0.1/24',
      routes: '[Ljava.lang.String;@1689abe',
      dns: '10.255.255.254',
    })
    const annotated = await vpn.annotateNetworkInfoFromVpnService({
      proxy_cidr_route_sync: 'desired=[-] installed=[-] exit=false',
    }, 'A')
    expect(vpn.getMobileVpnInstalledRoutes('A')).toEqual(['10.0.0.1/24'])
    expect(vpn.getMobileVpnPushedDns('A')).toBe('10.255.255.254')
    expect(annotated.proxy_cidr_route_sync).toBe(
      'desired=[10.0.0.1/24] installed=[10.0.0.1/24] exit=false',
    )
  })

  it('attributes installed routes to the VPN owner instance only', async () => {
    setConfig('A')
    setConfig('B')
    setReady('A', '10.0.0.1')
    const vpn = await loadVpnModule()

    await vpn.onNetworkInstanceChange('A')
    expect(mocks.startVpn).toHaveBeenCalledTimes(1)

    // Owner sees its routes (interface subnet appended); others see none.
    expect(vpn.getMobileVpnInstalledRoutes('A')).toEqual(['10.0.0.1/24'])
    expect(vpn.getMobileVpnInstalledRoutes('B')).toEqual([])
    expect(vpn.getMobileVpnInstalledRoutes()).toEqual(['10.0.0.1/24'])
    expect(vpn.formatMobileVpnRouteSync(vpn.getMobileVpnInstalledRoutes('B'))).toBe(
      'desired=[-] installed=[-] exit=false',
    )
  })
})

describe('mobile VPN reconciliation ownership', () => {
  it('stops A before retrying an unavailable B, then starts B when it becomes ready', async () => {
    setConfig('A')
    setConfig('B')
    setReady('A', '10.0.0.1')
    const vpn = await loadVpnModule()

    await vpn.onNetworkInstanceChange('A')
    expect(mocks.startVpn).toHaveBeenCalledTimes(1)

    mocks.startVpn.mockClear()
    await vpn.onNetworkInstanceChange('B')

    expect(mocks.stopVpn).toHaveBeenCalledTimes(1)
    expect(mocks.startVpn).not.toHaveBeenCalled()

    setReady('B', '10.0.0.2')
    await vpn.onNetworkInstanceUpdate('B')

    expect(mocks.startVpn).toHaveBeenCalledTimes(1)
    expect(mocks.startVpn).toHaveBeenCalledWith(expect.objectContaining({ ipv4Addr: '10.0.0.2/24' }))
  })

  it('stops the previous owner during pre-run even if the new instance never reaches post-run', async () => {
    setConfig('A')
    setConfig('B')
    setReady('A', '10.0.0.1')
    const vpn = await loadVpnModule()

    await vpn.onNetworkInstanceChange('A')
    mocks.stopVpn.mockClear()

    await vpn.prepareVpnService('B')

    expect(mocks.stopVpn).toHaveBeenCalledTimes(1)
  })

  it('preserves the VPN while retrying the same instance', async () => {
    setConfig('A')
    setReady('A', '10.0.0.1')
    const vpn = await loadVpnModule()

    await vpn.onNetworkInstanceChange('A')
    mocks.stopVpn.mockClear()
    mocks.networkInfo.delete('A')

    await vpn.onNetworkInstanceUpdate('A')

    expect(mocks.stopVpn).not.toHaveBeenCalled()
  })

  it('ignores an update from an instance that no longer owns the VPN', async () => {
    setConfig('A')
    setConfig('B')
    setReady('A', '10.0.0.1')
    const vpn = await loadVpnModule()

    await vpn.onNetworkInstanceChange('A')
    await vpn.onNetworkInstanceChange('B')
    mocks.collectNetworkInfo.mockClear()

    await vpn.onNetworkInstanceUpdate('A')

    expect(mocks.collectNetworkInfo).not.toHaveBeenCalled()
  })

  it('does not apply an in-flight result after the desired instance changes', async () => {
    setConfig('A')
    setConfig('B')
    setReady('A', '10.0.0.1')
    const vpn = await loadVpnModule()

    await vpn.onNetworkInstanceChange('A')
    mocks.startVpn.mockClear()
    mocks.stopVpn.mockClear()

    interface NetworkInfoResponse { info: { map: Record<string, unknown> } }
    let resolveNetworkInfo: (value: NetworkInfoResponse) => void = () => undefined
    let markCollectStarted: () => void = () => undefined
    const collectStarted = new Promise<void>((resolve) => {
      markCollectStarted = resolve
    })
    mocks.collectNetworkInfo.mockImplementationOnce(async () => await new Promise<NetworkInfoResponse>((resolve) => {
      resolveNetworkInfo = resolve
      markCollectStarted()
    }))

    const staleUpdate = vpn.onNetworkInstanceUpdate('A')
    await collectStarted
    const switchToB = vpn.onNetworkInstanceChange('B')
    resolveNetworkInfo({
      info: {
        map: {
          A: {
            my_node_info: {
              virtual_ipv4: {
                address: { addr: '10.0.0.99' },
                network_length: 24,
              },
            },
            routes: [],
          },
        },
      },
    })

    await Promise.all([staleUpdate, switchToB])

    expect(mocks.startVpn).not.toHaveBeenCalled()
    expect(mocks.stopVpn).toHaveBeenCalledTimes(1)
  })

  it('stops a native VPN with unknown ownership before retrying the selected instance', async () => {
    setConfig('A')
    mocks.getVpnStatus.mockResolvedValue({
      running: true,
      ipv4Addr: '10.0.0.1/24',
      routes: [],
    })
    mocks.listNetworkInstanceIds.mockResolvedValue({ running_inst_ids: ['A'] })
    const vpn = await loadVpnModule()

    await vpn.syncMobileVpnService()

    expect(mocks.stopVpn).toHaveBeenCalledTimes(1)
    expect(mocks.startVpn).not.toHaveBeenCalled()
  })
})

describe('mobile VPN tile action delivery', () => {
  it('does not consume a pending action before a handler is ready', async () => {
    const vpn = await loadVpnModule()

    expect(await vpn.consumePendingMobileVpnTileAction()).toBe(false)
    expect(mocks.consumeVpnTileAction).not.toHaveBeenCalled()
  })

  it('consumes and dispatches a pending action once a handler is registered', async () => {
    const vpn = await loadVpnModule()
    const handler = vi.fn(async () => undefined)
    mocks.consumeVpnTileAction.mockResolvedValue({ action: 'start' })
    vpn.setMobileVpnTileActionHandler(handler)

    expect(await vpn.consumePendingMobileVpnTileAction()).toBe(true)
    expect(handler).toHaveBeenCalledWith('start')
  })
})

describe('mobile VPN TUN device error recovery', () => {
  /** Simulate a running VPN whose native side keeps reporting healthy. */
  function setRunningVpn() {
    setConfig('A')
    setReady('A', '10.0.0.1')
    mocks.getVpnStatus.mockResolvedValue({
      running: true,
      ipv4Addr: '10.0.0.1/24',
      routes: [],
    })
    mocks.listNetworkInstanceIds.mockResolvedValue({ running_inst_ids: ['A'] })
  }

  let clockBase = 0
  beforeEach(() => {
    clockBase = Date.now()
  })

  /**
   * Move `Date.now()` forward without firing timers. The grace window and the
   * rebuild throttle are pure wall-clock logic, so this isolates them from the
   * background sync interval.
   */
  function setClockOffset(offsetMs: number) {
    vi.setSystemTime(clockBase + offsetMs)
  }

  it('rebuilds the VPN when the core reports a TUN device error', async () => {
    setRunningVpn()
    const vpn = await loadVpnModule()
    await vpn.onNetworkInstanceChange('A')
    expect(mocks.startVpn).toHaveBeenCalledTimes(1)

    // Leave the start/stop transition grace window (15s) first.
    setClockOffset(20000)
    mocks.startVpn.mockClear()
    mocks.stopVpn.mockClear()

    await vpn.handleMobileTunDeviceError('A')

    expect(mocks.stopVpn).toHaveBeenCalledTimes(1)
    expect(mocks.startVpn).toHaveBeenCalledTimes(1)
  })

  it('ignores a TUN device error reported during a VPN transition', async () => {
    setRunningVpn()
    const vpn = await loadVpnModule()
    await vpn.onNetworkInstanceChange('A')
    mocks.startVpn.mockClear()
    mocks.stopVpn.mockClear()

    // No clock offset: the start transition grace window is still open, so a
    // deliberate fd close must not be mistaken for a failure.
    await vpn.handleMobileTunDeviceError('A')

    expect(mocks.stopVpn).not.toHaveBeenCalled()
    expect(mocks.startVpn).not.toHaveBeenCalled()
  })

  it('ignores a TUN device error from an instance that does not own the VPN', async () => {
    setRunningVpn()
    setConfig('B')
    const vpn = await loadVpnModule()
    await vpn.onNetworkInstanceChange('A')
    setClockOffset(20000)
    mocks.startVpn.mockClear()
    mocks.stopVpn.mockClear()

    await vpn.handleMobileTunDeviceError('B')

    expect(mocks.stopVpn).not.toHaveBeenCalled()
    expect(mocks.startVpn).not.toHaveBeenCalled()
  })

  it('throttles repeated rebuilds so a permanently bad fd cannot flap', async () => {
    setRunningVpn()
    const vpn = await loadVpnModule()
    await vpn.onNetworkInstanceChange('A')

    setClockOffset(20000)
    await vpn.handleMobileTunDeviceError('A')
    expect(mocks.stopVpn).toHaveBeenCalledTimes(1)
    mocks.startVpn.mockClear()
    mocks.stopVpn.mockClear()

    // Past the 15s transition grace, but still inside the 30s rebuild floor.
    setClockOffset(36000)
    await vpn.handleMobileTunDeviceError('A')
    expect(mocks.stopVpn).not.toHaveBeenCalled()

    // Once the floor elapses the recovery must be allowed again.
    setClockOffset(56000)
    await vpn.handleMobileTunDeviceError('A')
    expect(mocks.stopVpn).toHaveBeenCalledTimes(1)
  })

  it('disables the ET instance when TUN dies after the OS VPN is already gone', async () => {
    setRunningVpn()
    const vpn = await loadVpnModule()
    await vpn.onNetworkInstanceChange('A')
    setClockOffset(20000)
    mocks.startVpn.mockClear()
    mocks.stopVpn.mockClear()

    // System disconnect already tore down VpnService; core still reports TUN error.
    mocks.getVpnStatus.mockResolvedValue({ running: false })
    await vpn.handleMobileTunDeviceError('A')

    expect(mocks.updateNetworkConfigState).toHaveBeenCalledWith('A', true)
    expect(mocks.startVpn).not.toHaveBeenCalled()
  })
})

describe('mobile VPN external disconnect (system VPN UI)', () => {
  it('disables the owning instance on vpn_service_stop reason=revoke', async () => {
    setConfig('A')
    setReady('A', '10.0.0.1')
    mocks.getVpnStatus.mockResolvedValue({
      running: true,
      ipv4Addr: '10.0.0.1/24',
      routes: [],
    })
    const vpn = await loadVpnModule()
    await vpn.onNetworkInstanceChange('A')
    expect(mocks.startVpn).toHaveBeenCalledTimes(1)

    await mocks.listeners.get('vpn_service_stop')?.({ reason: 'revoke' })

    expect(mocks.updateNetworkConfigState).toHaveBeenCalledWith('A', true)
  })

  it('does not disable the instance on app-initiated stop (reason=app)', async () => {
    setConfig('A')
    setReady('A', '10.0.0.1')
    mocks.getVpnStatus.mockResolvedValue({
      running: true,
      ipv4Addr: '10.0.0.1/24',
      routes: [],
    })
    const vpn = await loadVpnModule()
    await vpn.onNetworkInstanceChange('A')
    mocks.updateNetworkConfigState.mockClear()

    await mocks.listeners.get('vpn_service_stop')?.({ reason: 'app' })

    expect(mocks.updateNetworkConfigState).not.toHaveBeenCalled()
  })
})

describe('mobile VPN route-change debounce', () => {
  it('debounces route-only VpnService rebuilds instead of remounting immediately', async () => {
    setConfig('A')
    setReady('A', '10.0.0.1')
    mocks.getVpnStatus.mockResolvedValue({
      running: true,
      ipv4Addr: '10.0.0.1/24',
      routes: [],
    })
    const vpn = await loadVpnModule()
    await vpn.onNetworkInstanceChange('A')
    expect(mocks.startVpn).toHaveBeenCalledTimes(1)

    mocks.startVpn.mockClear()
    mocks.stopVpn.mockClear()

    // Grow proxy routes like OSPF sync after connect (Status remount source).
    mocks.networkInfo.set('A', {
      my_node_info: {
        virtual_ipv4: {
          address: { addr: '10.0.0.1' },
          network_length: 24,
        },
      },
      routes: [{ proxy_cidrs: ['192.168.8.0/22'], ipv4_addr: null, next_hop_peer_id: 1 }],
    })

    await vpn.onNetworkInstanceUpdate('A')
    expect(mocks.stopVpn).not.toHaveBeenCalled()
    expect(mocks.startVpn).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(2499)
    expect(mocks.stopVpn).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(1)
    expect(mocks.stopVpn).toHaveBeenCalledTimes(1)
    expect(mocks.startVpn).toHaveBeenCalledTimes(1)
  })
})

describe('mobile VPN underlay network change (A9)', () => {
  it('registers a default_network_changed plugin listener on init', async () => {
    await loadVpnModule()
    expect(mocks.addPluginListener).toHaveBeenCalledWith(
      'vpnservice',
      'default_network_changed',
      expect.any(Function),
    )
  })

  it('forwards generation to notifyUnderlayNetworkChanged when the underlay switches', async () => {
    await loadVpnModule()
    const listener = mocks.listeners.get('default_network_changed')
    expect(listener).toBeTypeOf('function')

    await listener?.({ generation: 7, networkId: 42 })

    expect(mocks.notifyUnderlayNetworkChanged).toHaveBeenCalledTimes(1)
    expect(mocks.notifyUnderlayNetworkChanged).toHaveBeenCalledWith(7)
  })

  it('still notifies Rust when generation is omitted', async () => {
    await loadVpnModule()
    const listener = mocks.listeners.get('default_network_changed')
    await listener?.({ networkId: 1 })

    expect(mocks.notifyUnderlayNetworkChanged).toHaveBeenCalledWith(undefined)
  })

  it('swallows notifyUnderlayNetworkChanged rejection without throwing', async () => {
    mocks.notifyUnderlayNetworkChanged.mockRejectedValueOnce(new Error('backend down'))
    await loadVpnModule()
    const listener = mocks.listeners.get('default_network_changed')

    await expect(listener?.({ generation: 2 })).resolves.toBeUndefined()
    expect(mocks.notifyUnderlayNetworkChanged).toHaveBeenCalledWith(2)
  })
})
