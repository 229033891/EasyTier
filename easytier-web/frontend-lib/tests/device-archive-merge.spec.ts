import { describe, expect, it } from 'vitest'
import { mergeDevicesWithArchive, type DeviceInfo } from '../src/modules/utils'

function onlineDevice(overrides: Partial<DeviceInfo> & Pick<DeviceInfo, 'machine_id' | 'hostname'>): DeviceInfo {
  return {
    public_ip: 'udp://1.2.3.4:22020',
    running_network_count: 1,
    report_time: 'now',
    easytier_version: '2.7.0',
    running_network_instances: ['aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'],
    location: undefined,
    reported_hostname: overrides.hostname,
    ...overrides,
  }
}

describe('mergeDevicesWithArchive', () => {
  it('overlays display_name onto online devices', () => {
    const online = [onlineDevice({ machine_id: 'dev-1', hostname: 'APP' })]
    const merged = mergeDevicesWithArchive(online, [
      {
        device_id: 'dev-1',
        hostname: 'APP',
        display_name: 'APP-SERVER',
      },
    ])
    expect(merged).toHaveLength(1)
    expect(merged[0].hostname).toBe('APP-SERVER')
    expect(merged[0].reported_hostname).toBe('APP')
    expect(merged[0].display_name).toBe('APP-SERVER')
  })

  it('falls back to reported hostname when display_name is empty', () => {
    const online = [onlineDevice({ machine_id: 'dev-1', hostname: 'APP' })]
    const merged = mergeDevicesWithArchive(online, [
      {
        device_id: 'dev-1',
        hostname: 'APP',
        display_name: '',
      },
    ])
    expect(merged[0].hostname).toBe('APP')
    expect(merged[0].reported_hostname).toBe('APP')
    expect(merged[0].display_name).toBeUndefined()
  })

  it('appends offline archive rows with display alias', () => {
    const online = [onlineDevice({ machine_id: 'dev-1', hostname: 'APP' })]
    const merged = mergeDevicesWithArchive(online, [
      {
        device_id: 'dev-1',
        hostname: 'APP',
        display_name: 'APP-SERVER',
      },
      {
        device_id: 'dev-2',
        hostname: 'fn-nas',
        display_name: 'NAS',
        last_easytier_version: '2.6.4',
        last_client_url: 'udp://9.9.9.9:22020',
        last_seen_at: 1_700_000_000,
      },
    ])
    expect(merged).toHaveLength(2)
    expect(merged[0].hostname).toBe('APP-SERVER')
    expect(merged[1].machine_id).toBe('dev-2')
    expect(merged[1].hostname).toBe('NAS')
    expect(merged[1].reported_hostname).toBe('fn-nas')
    expect(merged[1].running_network_count).toBe(0)
  })

  it('hides empty rename stub rows that are not online', () => {
    const merged = mergeDevicesWithArchive([], [
      {
        device_id: 'stub-only',
        hostname: '',
        display_name: '',
      },
    ])
    expect(merged).toHaveLength(0)
  })
})
