import { describe, expect, it } from 'vitest'
import {
  jitterMs,
  latencyMs,
  lossRate,
  pathQualityCell,
  pathQualityTip,
  udpNatTypeName,
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

  it('skips missing latency and invalid loss values', () => {
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
      { conn_id: 'valid', loss_rate: '0.25' },
      { conn_id: 'invalid', loss_rate: 'unknown' },
    ]))).toBe('25%')
    expect(lossRate(peerRoutePair([
      { conn_id: 'invalid', loss_rate: 'unknown' },
    ]))).toBe('')
  })

  it('renders omitted protobuf loss as zero only for an existing connection', () => {
    expect(lossRate(peerRoutePair([{ conn_id: 'zero' }]))).toBe('0%')
    expect(lossRate(peerRoutePair([{ conn_id: 'zero', loss_rate: 0 }]))).toBe('0%')
    expect(lossRate(peerRoutePair([
      { conn_id: 'zero' },
      { conn_id: 'lossy', loss_rate: 0.7 },
    ]))).toBe('0%')
    expect(lossRate(peerRoutePair([]))).toBe('')
    expect(lossRate({ route: {} } as any)).toBe('')
  })

  it('formats jitter from PeerConnStats', () => {
    expect(jitterMs(peerRoutePair([
      { conn_id: 'missing', stats: {} },
      { conn_id: 'valid', stats: { jitter_us: '1500' } },
    ]))).toBe('2ms')
    expect(jitterMs(peerRoutePair([
      { conn_id: 'missing', stats: {} },
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

  it('keeps an omitted zero loss on the default connection', () => {
    const defaultConnId = '00000001-0002-0003-0004-000000000005'
    expect(lossRate(peerRoutePairWithDefaultConn([
      { conn_id: 'fallback', loss_rate: 0.25 },
      { conn_id: defaultConnId },
    ], defaultConnId))).toBe('0%')
  })

  it.each([
    [0, 'Unknown', 'Unknown'],
    [1, 'OpenInternet', 'Open Internet'],
    [2, 'NoPAT', 'No PAT'],
    [3, 'FullCone', 'Full Cone'],
    [4, 'Restricted', 'Restricted'],
    [5, 'PortRestricted', 'Port Restricted'],
    [6, 'Symmetric', 'Symmetric'],
    [7, 'SymUdpFirewall', 'Symmetric UDP Firewall'],
    [8, 'SymmetricEasyInc', 'Symmetric Easy Inc'],
    [9, 'SymmetricEasyDec', 'Symmetric Easy Dec'],
  ])('renders numeric and protobuf enum NAT type %s', (value, name, label) => {
    expect(udpNatTypeName({ udp_nat_type: value })).toBe(label)
    expect(udpNatTypeName({ udp_nat_type: name })).toBe(label)
    expect(udpNatTypeName({ udp_nat_type: String(value) })).toBe(label)
  })

  it('distinguishes missing STUN data from default or unrecognized NAT types', () => {
    expect(udpNatTypeName(undefined)).toBe('')
    expect(udpNatTypeName({})).toBe('Unknown')
    expect(udpNatTypeName({ udp_nat_type: 100 })).toBe('Unknown')
    expect(udpNatTypeName({ udp_nat_type: 'FutureNatType' })).toBe('Unknown')
  })

  it('summarizes path quality with default score and standby count', () => {
    const defaultConnId = '00000001-0002-0003-0004-000000000005'
    const conns = [
      {
        conn_id: 'standby',
        tunnel: { tunnel_type: 'tcp' },
        stats: { latency_us: '2000', jitter_us: '500' },
        loss_rate: '0.01',
        quality_score: 0.1,
        quality_fused: false,
      },
      {
        conn_id: defaultConnId,
        tunnel: { tunnel_type: 'udp' },
        stats: { latency_us: '9000', jitter_us: '1000' },
        loss_rate: '0.02',
        quality_score: 0.042,
        quality_fused: false,
      },
    ]
    const pair = peerRoutePairWithDefaultConn(conns, defaultConnId)
    expect(pathQualityCell(pair)).toBe('0.042 · +1')
    expect(pathQualityTip(pair)).toContain('★ udp score=0.042')
    expect(pathQualityTip(pair)).toContain('· tcp score=0.100')
  })

  it('marks fused default_conn in the quality cell', () => {
    const defaultConnId = '00000001-0002-0003-0004-000000000005'
    const pair = peerRoutePairWithDefaultConn([
      {
        conn_id: defaultConnId,
        tunnel: { tunnel_type: 'udp' },
        stats: { latency_us: '1000' },
        loss_rate: '0.5',
        quality_score: 2.1,
        quality_fused: true,
      },
    ], defaultConnId)
    expect(pathQualityCell(pair)).toBe('2.100!')
  })

  it('marks bond members in the quality cell and tooltip', () => {
    const defaultConnId = '00000001-0002-0003-0004-000000000005'
    const conns = [
      {
        conn_id: defaultConnId,
        tunnel: { tunnel_type: 'udp' },
        stats: { latency_us: '9000', jitter_us: '1000' },
        loss_rate: '0.02',
        quality_score: 0.042,
        quality_fused: false,
        in_bond_set: true,
        bond_class: 'udp',
      },
      {
        conn_id: 'standby',
        tunnel: { tunnel_type: 'tcp' },
        stats: { latency_us: '2000', jitter_us: '500' },
        loss_rate: '0.01',
        quality_score: 0.1,
        quality_fused: false,
        in_bond_set: true,
        bond_class: 'tcp',
      },
    ]
    const pair = peerRoutePairWithDefaultConn(conns, defaultConnId)
    expect(pathQualityCell(pair)).toBe('0.042 · +1 · bond×2')
    expect(pathQualityTip(pair)).toContain('★ udp score=0.042 rtt=9ms jitter=1ms loss=2% bond(udp)')
    expect(pathQualityTip(pair)).toContain('· tcp score=0.100 rtt=2ms jitter=1ms loss=1% bond(tcp)')
  })

  it('omits bond marks when backend does not flag members', () => {
    const defaultConnId = '00000001-0002-0003-0004-000000000005'
    const pair = peerRoutePairWithDefaultConn([
      {
        conn_id: defaultConnId,
        tunnel: { tunnel_type: 'udp' },
        stats: { latency_us: '9000' },
        loss_rate: '0.02',
        quality_score: 0.042,
        quality_fused: false,
      },
    ], defaultConnId)
    expect(pathQualityCell(pair)).toBe('0.042')
    expect(pathQualityTip(pair)).not.toContain('bond')
  })
})
