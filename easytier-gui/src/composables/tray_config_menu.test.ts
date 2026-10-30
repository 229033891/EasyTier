import { describe, expect, it } from 'vitest'
import { buildTrayConfigMenu, TRAY_CONFIG_LABEL_MAX, TRAY_CONFIG_MENU_MAX, trayConfigActionIsNoop, trayConfigMenuKey } from './tray_config_menu'

function src(instanceId: string, name?: string, running?: boolean) {
  return { instanceId, name, running }
}

describe('buildTrayConfigMenu', () => {
  it('returns an empty model when the device knows no config', () => {
    expect(buildTrayConfigMenu({ entries: [], untitledLabel: 'Untitled' }))
      .toEqual({ entries: [], total: 0, hidden: 0, runningCount: 0 })
  })

  it('lists stopped configs as startable and running ones as running', () => {
    const model = buildTrayConfigMenu({
      entries: [src('a', 'Office', true), src('b', 'Home', false)],
      untitledLabel: 'Untitled',
    })
    expect(model.entries.map(e => [e.label, e.running])).toEqual([
      ['Office', true],
      ['Home', false],
    ])
    expect(model.runningCount).toBe(1)
    expect(model.total).toBe(2)
    expect(model.hidden).toBe(0)
  })

  it('puts running configs first, then sorts by name', () => {
    const model = buildTrayConfigMenu({
      entries: [src('1', 'zeta'), src('2', 'alpha'), src('3', 'beta', true)],
      untitledLabel: 'Untitled',
    })
    expect(model.entries.map(e => e.label)).toEqual(['beta', 'alpha', 'zeta'])
  })

  it('falls back to the placeholder for an empty name', () => {
    const model = buildTrayConfigMenu({
      entries: [src('a', '   '), src('b')],
      untitledLabel: 'Untitled',
    })
    // 两条都叫 Untitled → 重名消歧也会生效。
    expect(model.entries).toHaveLength(2)
    expect(model.entries.every(e => e.label.startsWith('Untitled'))).toBe(true)
  })

  it('disambiguates duplicate names with a short instance id', () => {
    const model = buildTrayConfigMenu({
      entries: [src('1111aaaa-0000-0000-0000-000000000000', 'Home'), src('2222bbbb-0000-0000-0000-000000000000', 'Home')],
      untitledLabel: 'Untitled',
    })
    expect(model.entries.map(e => e.label).sort()).toEqual(['Home · 1111', 'Home · 2222'])
  })

  it('keeps a unique name untouched (no id suffix noise)', () => {
    const model = buildTrayConfigMenu({
      entries: [src('1111aaaa-0000-0000-0000-000000000000', 'Home'), src('2222bbbb-0000-0000-0000-000000000000', 'Office')],
      untitledLabel: 'Untitled',
    })
    expect(model.entries.map(e => e.label).sort()).toEqual(['Home', 'Office'])
  })

  it('drops entries with no instance id', () => {
    const model = buildTrayConfigMenu({
      entries: [src('  ', 'ghost'), src('a', 'Home')],
      untitledLabel: 'Untitled',
    })
    expect(model.entries.map(e => e.label)).toEqual(['Home'])
    expect(model.total).toBe(1)
  })

  it('deduplicates an id listed in both the running and the disabled set', () => {
    const model = buildTrayConfigMenu({
      entries: [src('a', 'Home', false), src('a', 'Home', true)],
      untitledLabel: 'Untitled',
    })
    expect(model.entries).toEqual([{ instanceId: 'a', label: 'Home', running: true }])
    expect(model.runningCount).toBe(1)
  })

  it('caps the list and reports how many are hidden', () => {
    const entries = Array.from({ length: TRAY_CONFIG_MENU_MAX + 3 }, (_, i) => src(`id-${i}`, `net-${String(i).padStart(2, '0')}`))
    const model = buildTrayConfigMenu({ entries, untitledLabel: 'Untitled' })
    expect(model.entries).toHaveLength(TRAY_CONFIG_MENU_MAX)
    expect(model.total).toBe(TRAY_CONFIG_MENU_MAX + 3)
    expect(model.hidden).toBe(3)
  })

  it('keeps running configs when truncating', () => {
    const entries = [
      ...Array.from({ length: TRAY_CONFIG_MENU_MAX }, (_, i) => src(`a-${i}`, `aaa-${i}`)),
      src('zzz', 'zzz-running', true),
    ]
    const model = buildTrayConfigMenu({ entries, untitledLabel: 'Untitled' })
    expect(model.entries[0].label).toBe('zzz-running')
    expect(model.hidden).toBe(1)
    // 被截掉的是一条未运行的（排序后位于末尾）。
    expect(model.entries.some(e => e.running)).toBe(true)
  })

  it('treats max <= 0 as unlimited', () => {
    const entries = Array.from({ length: TRAY_CONFIG_MENU_MAX + 1 }, (_, i) => src(`id-${i}`, `net-${i}`))
    const model = buildTrayConfigMenu({ entries, untitledLabel: 'Untitled', max: 0 })
    expect(model.entries).toHaveLength(TRAY_CONFIG_MENU_MAX + 1)
    expect(model.hidden).toBe(0)
  })

  it('truncates a very long name', () => {
    const long = 'x'.repeat(TRAY_CONFIG_LABEL_MAX + 10)
    const model = buildTrayConfigMenu({ entries: [src('a', long)], untitledLabel: 'Untitled' })
    expect(model.entries[0].label).toHaveLength(TRAY_CONFIG_LABEL_MAX)
    expect(model.entries[0].label.endsWith('…')).toBe(true)
  })

  it('never treats an unknown running flag as running', () => {
    const model = buildTrayConfigMenu({ entries: [src('a', 'Home')], untitledLabel: 'Untitled' })
    expect(model.entries[0].running).toBe(false)
    expect(model.runningCount).toBe(0)
  })
})

describe('trayConfigActionIsNoop', () => {
  it('treats stop as a no-op when the config is not in the instance table', () => {
    expect(trayConfigActionIsNoop('stop', false)).toBe(true)
  })

  it('stops a config that is in the instance table', () => {
    expect(trayConfigActionIsNoop('stop', true)).toBe(false)
  })

  it('starts a config that is not in the instance table', () => {
    expect(trayConfigActionIsNoop('start', false)).toBe(false)
  })

  it('treats start as a no-op only when the instance is really running', () => {
    // 在跑：启动 = overwrite 重启（拆网卡），必须挡掉。
    expect(trayConfigActionIsNoop('start', true, true)).toBe(true)
  })

  it('lets a failed start be retried from the tray', () => {
    // 启动失败/被取消的实例仍留在实例表里 —— 「启动」是恢复路径，不能当空操作。
    expect(trayConfigActionIsNoop('start', true, false)).toBe(false)
  })

  it('assumes running when the probe could not tell', () => {
    // 不对称代价：误重启比漏一次启动更贵。
    expect(trayConfigActionIsNoop('start', true, undefined)).toBe(true)
  })
})

describe('trayConfigMenuKey', () => {
  it('is stable regardless of input order', () => {
    const a = trayConfigMenuKey([src('1', 'a', true), src('2', 'b', false)])
    const b = trayConfigMenuKey([src('2', 'b', false), src('1', 'a', true)])
    expect(a).toBe(b)
  })

  it('changes when the run state changes', () => {
    const running = trayConfigMenuKey([src('1', 'a', true)])
    const stopped = trayConfigMenuKey([src('1', 'a', false)])
    expect(running).not.toBe(stopped)
  })

  it('changes when a config is renamed', () => {
    const before = trayConfigMenuKey([src('1', 'a')])
    const after = trayConfigMenuKey([src('1', 'renamed')])
    expect(before).not.toBe(after)
  })
})
