import type { ServiceStatus } from '~/composables/backend'
import { Menu, MenuItem, PredefinedMenuItem } from '@tauri-apps/api/menu'
import { TrayIcon } from '@tauri-apps/api/tray'
import { getCurrentWindow } from '@tauri-apps/api/window'
import pkg from '~/../package.json'

const DEFAULT_TRAY_NAME = 'main'

/** Wired by the app page so early Exit also stops the service when needed. */
let trayExitHandler: (() => void | Promise<void>) | null = null

export function registerTrayExitHandler(handler: () => void | Promise<void>) {
  trayExitHandler = handler
}

async function toggleVisibility() {
  if (await getCurrentWindow().isVisible()) {
    await getCurrentWindow().hide()
  }
  else {
    await getCurrentWindow().show()
    await getCurrentWindow().setFocus()
  }
}

export async function showMainWindow() {
  const win = getCurrentWindow()
  await win.show()
  await win.setFocus()
}

/**
 * Best-effort stop of a Running ET-Gui service before a hard process exit.
 * Used only when the app page has not yet registered {@link registerTrayExitHandler}.
 */
async function stopRunningServiceBestEffort() {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const status = await invoke<string>('get_service_status')
    if (status === 'Running')
      await invoke('set_service_status', { enable: false })
  }
  catch (e) {
    console.warn('Best-effort service stop before exit failed', e)
  }
}

async function defaultExitAction() {
  if (trayExitHandler) {
    await trayExitHandler()
    return
  }
  // Fail closed for service mode: never hard-exit while leaving a Running service.
  await stopRunningServiceBestEffort()
  const { exit } = await import('@tauri-apps/plugin-process')
  await exit(0)
}

export async function useTray(init: boolean = false) {
  let tray
  try {
    tray = await TrayIcon.getById(DEFAULT_TRAY_NAME)
    if (!tray) {
      tray = await TrayIcon.new({
        tooltip: `ET\n${pkg.version}`,
        title: `ET\n${pkg.version}`,
        id: DEFAULT_TRAY_NAME,
        menu: await Menu.new({
          id: 'main',
          items: await generateMenuItem(),
        }),
        action: async () => {
          toggleVisibility()
        },
      })
    }
  }
  catch (error) {
    console.warn('Error while creating tray icon:', error)
    return null
  }

  if (init) {
    tray.setTooltip(`ET\n${pkg.version}`)
    tray.setMenuOnLeftClick(false)
    tray.setMenu(await Menu.new({
      id: 'main',
      items: await generateMenuItem(),
    }))
  }

  return tray
}

export interface TrayServiceLabels {
  status: string
  start: string
  stop: string
  restart: string
  uninstall: string
}

export interface TrayMenuActions {
  onExit: () => void | Promise<void>
  onStartService?: () => void | Promise<void>
  onStopService?: () => void | Promise<void>
  onRestartService?: () => void | Promise<void>
  onUninstallService?: () => void | Promise<void>
}

export interface TrayMenuBuildOptions {
  showLabel: string
  exitLabel: string
  serviceMode?: boolean
  serviceStatus?: ServiceStatus
  serviceLabels?: TrayServiceLabels
  actions: TrayMenuActions
}

/** Default tray items before the app page wires mode-aware actions. */
export async function generateMenuItem(labels?: { show?: string, exit?: string }) {
  return [
    await MenuItemShow(labels?.show ?? 'Show / Hide'),
    await PredefinedMenuItem.new({ item: 'Separator' }),
    await MenuItemExit(labels?.exit ?? 'Exit', defaultExitAction),
  ]
}

export async function buildTrayMenuItems(opts: TrayMenuBuildOptions) {
  const items: Array<MenuItem | PredefinedMenuItem> = [
    await MenuItemShow(opts.showLabel),
  ]

  if (opts.serviceMode && opts.serviceLabels && opts.serviceStatus) {
    const status = opts.serviceStatus
    const labels = opts.serviceLabels
    items.push(await PredefinedMenuItem.new({ item: 'Separator' }))
    items.push(await MenuItem.new({
      id: 'service-status',
      text: labels.status,
      enabled: false,
    }))
    items.push(await MenuItem.new({
      id: 'start-service',
      text: labels.start,
      enabled: status === 'Stopped',
      action: async () => {
        await opts.actions.onStartService?.()
      },
    }))
    items.push(await MenuItem.new({
      id: 'stop-service',
      text: labels.stop,
      enabled: status === 'Running',
      action: async () => {
        await opts.actions.onStopService?.()
      },
    }))
    items.push(await MenuItem.new({
      id: 'restart-service',
      text: labels.restart,
      // Restart only when already running (Stopped → use Start).
      enabled: status === 'Running',
      action: async () => {
        await opts.actions.onRestartService?.()
      },
    }))
    items.push(await MenuItem.new({
      id: 'uninstall-service',
      text: labels.uninstall,
      enabled: status !== 'NotInstalled',
      action: async () => {
        await opts.actions.onUninstallService?.()
      },
    }))
  }

  items.push(await PredefinedMenuItem.new({ item: 'Separator' }))
  items.push(await MenuItemExit(opts.exitLabel, opts.actions.onExit))
  return items
}

export async function MenuItemExit(text: string, action: () => void | Promise<void>) {
  return await MenuItem.new({
    id: 'exit',
    text,
    action: async () => {
      await action()
    },
  })
}

export async function MenuItemShow(text: string) {
  return await MenuItem.new({
    id: 'show',
    text,
    action: async () => {
      await toggleVisibility()
    },
  })
}

export async function setTrayMenu(items: (MenuItem | PredefinedMenuItem)[] | undefined = undefined) {
  const tray = await useTray()
  if (!tray)
    return
  const menu = await Menu.new({
    id: 'main',
    items: items || await generateMenuItem(),
  })
  tray.setMenu(menu)
}

export async function setTrayRunState(isRunning: boolean = false) {
  const tray = await useTray()
  if (!tray)
    return
  // Running → active icon; stopped → inactive icon
  tray.setIcon(isRunning ? 'icons/icon.ico' : 'icons/icon-inactive.ico')
}

export async function setTrayTooltip(tooltip: string) {
  if (tooltip) {
    const tray = await useTray()
    if (!tray)
      return
    tray.setTooltip(`ET\n${pkg.version}\n${tooltip}`)
    tray.setTitle(`ET\n${pkg.version}\n${tooltip}`)
  }
}
