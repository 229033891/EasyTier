import { describe, expect, it } from 'vitest'
import {
  collectLocalInstalledRoutes,
  collectRouteProxyCidrs,
  latencyMs,
  lossRate,
  parseInstalledProxyCidrs,
} from '../src/modules/statusDisplay'
import { ipv4ToString, ipv6ToString } from '../src/modules/utils'

function peerRoutePair(conns: any[]) {
  return {
    route: {
      ipv4_addr: '10.0.0.2',
      hostname: 'peer',
      version: 'test',
    },
    peer: {
      conns,
    },
  } as any
}

function peerRoutePairWithDefaultConn(conns: any[], defaultConnId: string) {
  const [part1, part2, part3, part4] = defaultConnId
    .replaceAll('-', '')
    .match(/.{8}/g)!
    .map((part) => Number.parseInt(part, 16))

  return {
    ...peerRoutePair(conns),
    peer: {
      default_conn_id: {
        part1,
        part2,
        part3,
        part4,
      },
      conns,
    },
  } as any
}

describe('status display helpers', () => {
  it('does not render missing IP values as zero addresses', () => {
    expect(ipv4ToString(undefined)).toBe('')
    expect(ipv4ToString(null)).toBe('')
    expect(ipv4ToString({} as any)).toBe('0.0.0.0')
    expect(ipv4ToString({ addr: 0 })).toBe('0.0.0.0')

    expect(ipv6ToString(undefined)).toBe('')
    expect(ipv6ToString(null)).toBe('')
    expect(ipv6ToString({} as any)).toBe('::0')
    expect(ipv6ToString({ part1: 0, part2: 0, part3: 0, part4: 0 })).toBe('::0')
    expect(ipv6ToString({ part4: 1 } as any)).toBe('::1')
  })

  it('skips missing latency and loss values', () => {
    expect(latencyMs(peerRoutePair([
      { conn_id: 'missing', stats: {} },
      { conn_id: 'valid', stats: { latency_us: '2500' } },
      { conn_id: 'invalid', stats: { latency_us: 'unknown' } },
    ]))).toBe('3ms')
    expect(latencyMs(peerRoutePair([
      { conn_id: 'missing', stats: {} },
      { conn_id: 'invalid', stats: { latency_us: 'unknown' } },
    ]))).toBe('')

    expect(lossRate(peerRoutePair([
      { conn_id: 'missing' },
      { conn_id: 'valid', loss_rate: '0.25' },
      { conn_id: 'invalid', loss_rate: 'unknown' },
    ]))).toBe('25%')
    expect(lossRate(peerRoutePair([
      { conn_id: 'missing' },
      { conn_id: 'invalid', loss_rate: 'unknown' },
    ]))).toBe('')
  })

  it('prefers the default connection when its metric is valid', () => {
    const defaultConnId = '00000001-0002-0003-0004-000000000005'
    const conns = [
      { conn_id: 'fallback', stats: { latency_us: '1000' }, loss_rate: '0.01' },
      { conn_id: defaultConnId, stats: { latency_us: '9000' }, loss_rate: '0.5' },
    ]

    expect(latencyMs(peerRoutePairWithDefaultConn(conns, defaultConnId))).toBe('9ms')
    expect(lossRate(peerRoutePairWithDefaultConn(conns, defaultConnId))).toBe('50%')
  })

  it('parses installed proxy CIDRs from L2 sync summary', () => {
    expect(parseInstalledProxyCidrs(undefined)).toEqual([])
    expect(parseInstalledProxyCidrs('desired=[-] installed=[-] exit=false')).toEqual([])
    expect(parseInstalledProxyCidrs(
      'desired=[10.0.0.0/24] installed=[10.0.0.0/24,192.168.1.0/24] exit=true',
    )).toEqual(['10.0.0.0/24', '192.168.1.0/24'])
  })

  it('collects route-table proxy CIDRs as fallback', () => {
    expect(collectRouteProxyCidrs([
      { proxy_cidrs: ['10.1.0.0/24', '10.2.0.1'] },
      { proxy_cidrs: ['10.1.0.0/24'] },
    ] as any)).toEqual(['10.1.0.0/24', '10.2.0.1/32'])
    expect(collectRouteProxyCidrs([
      { proxy_cidrs: ['fd00::1', '2001:db8::/64'] },
    ] as any)).toEqual(['2001:db8::/64', 'fd00::1/128'])
  })

  it('prefers L2 installed routes over route-table proxy CIDRs', () => {
    expect(collectLocalInstalledRoutes({
      proxy_cidr_route_sync: 'desired=[1.0.0.0/8] installed=[1.0.0.0/8] exit=false',
      routes: [{ proxy_cidrs: ['9.9.9.0/24'] } as any],
    })).toEqual(['1.0.0.0/8'])

    // Sync present but installed empty: do NOT fall back (would misreport peers' CIDRs as installed)
    expect(collectLocalInstalledRoutes({
      proxy_cidr_route_sync: 'desired=[1.0.0.0/8] installed=[-] exit=false',
      routes: [{ proxy_cidrs: ['9.9.9.0/24'] } as any],
    })).toEqual([])

    expect(collectLocalInstalledRoutes({
      routes: [{ proxy_cidrs: ['9.9.9.0/24'] } as any],
    })).toEqual(['9.9.9.0/24'])
  })
})
