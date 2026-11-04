import type { TrayConfigMenuOptions, TrayMenuBuildOptions } from './tray'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { buildTrayMenuItems } from './tray'

interface MenuRow {
  kind: 'item' | 'predefined' | 'submenu' | 'menu'
  id?: string
  text?: string
  item?: string
  enabled?: boolean
  action?: () => void | Promise<void>
}

const menu = vi.hoisted(() => ({
  Menu: { new: vi.fn(async (o: Record<string, unknown>) => ({ kind: 'menu', ...o })) },
  MenuItem: { new: vi.fn(async (o: Record<string, unknown>) => ({ kind: 'item', ...o })) },
  PredefinedMenuItem: { new: vi.fn(async (o: Record<string, unknown>) => ({ kind: 'predefined', ...o })) },
  Submenu: { new: vi.fn(async (o: Record<string, unknown>) => ({ kind: 'submenu', ...o })) },
}))

vi.mock('@tauri-apps/api/menu', () => menu)
vi.mock('@tauri-apps/api/path', () => ({ resolveResource: vi.fn(async (p: string) => p) }))
vi.mock('@tauri-apps/api/tray', () => ({ TrayIcon: { getById: vi.fn(), new: vi.fn() } }))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: vi.fn() }))

function baseOpts(overrides: Partial<TrayMenuBuildOptions> = {}): TrayMenuBuildOptions {
  return {
    openWindowLabel: 'Open window…',
    onOpenWindow: () => {},
    exitLabel: 'Exit',
    actions: { onExit: () => {} },
    ...overrides,
  }
}

function configMenu(overrides: Partial<TrayConfigMenuOptions> = {}): TrayConfigMenuOptions {
  return {
    entries: [
      { instanceId: 'a', label: 'Office', running: true },
      { instanceId: 'b', label: 'Home', running: false },
    ],
    hidden: 0,
    ready: true,
    labels: { empty: 'No configs yet', offline: 'Core not connected', more: '3 more not listed…' },
    onStart: () => {},
    onStop: () => {},
    ...overrides,
  }
}

async function rowsOf(opts: TrayMenuBuildOptions): Promise<MenuRow[]> {
  return await buildTrayMenuItems(opts) as unknown as MenuRow[]
}

function shape(rows: MenuRow[]) {
  return rows.map(row => row.kind === 'predefined' ? 'separator' : row.id)
}

beforeEach(() => {
  vi.clearAllMocks()
})

describe('buildTrayMenuItems', () => {
  it('puts the config list at the top and keeps exit last, with no submenu', async () => {
    const rows = await rowsOf(baseOpts({
      serviceMode: true,
      serviceStatus: 'Running',
      serviceLabels: {
        status: 'Service status',
        start: 'Start service',
        stop: 'Stop service',
        restart: 'Restart service',
        uninstall: 'Uninstall service',
      },
      configMenu: configMenu(),
    }))

    expect(shape(rows)).toEqual([
      'config:a',
      'config:b',
      'separator',
      'open-window',
      'separator',
      'service-status',
      'start-service',
      'stop-service',
      'restart-service',
      'uninstall-service',
      'separator',
      'exit',
    ])
    expect(rows.every(row => row.kind !== 'submenu')).toBe(true)
    expect(menu.Submenu.new).not.toHaveBeenCalled()
  })

  it('renders the run-state prefix and routes each click by the snapshot state', async () => {
    const onStart = vi.fn()
    const onStop = vi.fn()
    const rows = await rowsOf(baseOpts({ configMenu: configMenu({ onStart, onStop }) }))

    expect(rows[0].text).toBe('● Office')
    expect(rows[1].text).toBe('○ Home')
    await rows[0].action?.()
    expect(onStop).toHaveBeenCalledWith('a', 'Office')
    await rows[1].action?.()
    expect(onStart).toHaveBeenCalledWith('b', 'Home')
  })

  it('appends a disabled hint when the list was truncated', async () => {
    const rows = await rowsOf(baseOpts({ configMenu: configMenu({ hidden: 3 }) }))

    expect(shape(rows)).toEqual([
      'config:a',
      'config:b',
      'config-more',
      'separator',
      'open-window',
      'separator',
      'exit',
    ])
    expect(rows[2]).toMatchObject({ id: 'config-more', enabled: false, text: '3 more not listed…' })
  })

  it('collapses to one disabled placeholder when the core is unreachable', async () => {
    const rows = await rowsOf(baseOpts({ configMenu: configMenu({ ready: false }) }))

    expect(shape(rows)).toEqual(['config-offline', 'separator', 'open-window', 'separator', 'exit'])
    expect(rows[0]).toMatchObject({ id: 'config-offline', text: 'Core not connected', enabled: false })
  })

  it('collapses to one disabled placeholder when there are no configs', async () => {
    const rows = await rowsOf(baseOpts({ configMenu: configMenu({ entries: [] }) }))

    expect(shape(rows)).toEqual(['config-empty', 'separator', 'open-window', 'separator', 'exit'])
    expect(rows[0]).toMatchObject({ id: 'config-empty', text: 'No configs yet', enabled: false })
  })

  it('falls back to the minimal menu when no config list is supplied', async () => {
    const rows = await rowsOf(baseOpts())

    expect(shape(rows)).toEqual(['open-window', 'separator', 'exit'])
  })
})
