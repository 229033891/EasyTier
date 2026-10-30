import type { TrayRunState } from './tray_run_state'
import type { ServiceStatus } from '~/composables/backend'
import { Menu, MenuItem, PredefinedMenuItem, Submenu } from '@tauri-apps/api/menu'
import { resolveResource } from '@tauri-apps/api/path'
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

/**
 * Bring the main window to the front.
 *
 * Tray-driven flows call this right before a `confirm.require` / toast: the window
 * is usually hidden then, so without it the user never sees the dialog.
 *
 * Best-effort on purpose — this is a *presentation* step, not part of the action.
 * Letting a window-API failure reject would turn a tray click into an unhandled
 * rejection and make the action look like it did nothing (e.g. "退出" after the
 * window was already destroyed), so it warns and continues.
 */
export async function showMainWindow() {
  try {
    const win = getCurrentWindow()
    await win.show()
    await win.setFocus()
  }
  catch (e) {
    console.warn('Failed to show main window', e)
  }
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

/** 「网络配置」子菜单的文案（全部由调用方格式化，含计数）。 */
export interface TrayConfigMenuLabels {
  /** 子菜单标题，例如「网络配置 (2/5 运行中)」。 */
  title: string
  /** 一条配置都没有时的占位行。 */
  empty: string
  /** 内核不可达时的占位行（比空列表更能说明问题）。 */
  offline: string
  /** 被上限截断时的提示行，例如「还有 11 个未列出…」。 */
  more: string
  /** 兜底入口：打开主窗口去管理。 */
  openWindow: string
}

export interface TrayConfigMenuOptions {
  /** 已排序、已截断的条目（见 `tray_config_menu.ts::buildTrayConfigMenu`）。 */
  entries: Array<{ instanceId: string, label: string, running: boolean }>
  /** 被上限截掉的条数；> 0 时追加提示行。 */
  hidden: number
  labels: TrayConfigMenuLabels
  /** false = 内核不可达 / 列表拿不到，条目降级为占位行（不留点了没反应的路径）。 */
  ready: boolean
  onStart: (instanceId: string, label: string) => void | Promise<void>
  onStop: (instanceId: string, label: string) => void | Promise<void>
  onOpenWindow: () => void | Promise<void>
}

export interface TrayMenuBuildOptions {
  showLabel: string
  exitLabel: string
  serviceMode?: boolean
  serviceStatus?: ServiceStatus
  serviceLabels?: TrayServiceLabels
  /** 「网络配置」子菜单；桌面端总会传，测试/兜底可省。 */
  configMenu?: TrayConfigMenuOptions
  actions: TrayMenuActions
}

/** 默认托盘项（未接页面时的最小集）。 */
export async function generateMenuItem(labels?: { show?: string, exit?: string }) {
  return [
    await MenuItemShow(labels?.show ?? 'Show / Hide'),
    await PredefinedMenuItem.new({ item: 'Separator' }),
    await MenuItemExit(labels?.exit ?? 'Exit', defaultExitAction),
  ]
}

/**
 * 「网络配置」子菜单：一处入口列出全部配置，● 运行中（点 = 停止）/ ○ 未运行（点 =
 * 启动）。
 *
 * 为什么是一个子菜单而不是「启动配置 / 停止配置」两个：两个子菜单要求用户先知道目标
 * 配置当前的运行态才选得对入口，而且条目会在两个子菜单之间来回跳；混排 + 状态前缀把
 * 「现在是什么、点一下会怎样」放在同一行里。误停的代价只是再点一下。
 *
 * 用文本前缀而不是 `CheckMenuItem`：muda 的勾选在 Linux AppIndicator 下渲染不稳，
 * 文本前缀在 Windows / macOS / Linux 都可控。
 */
async function buildTrayConfigSubmenu(opts: TrayConfigMenuOptions) {
  const { labels } = opts
  const children: Array<MenuItem | PredefinedMenuItem | Submenu> = []

  if (!opts.ready) {
    children.push(await MenuItem.new({
      id: 'config-offline',
      text: labels.offline,
      enabled: false,
    }))
  }
  else if (!opts.entries.length) {
    children.push(await MenuItem.new({
      id: 'config-empty',
      text: labels.empty,
      enabled: false,
    }))
  }
  else {
    for (const entry of opts.entries) {
      children.push(await MenuItem.new({
        id: `config:${entry.instanceId}`,
        text: `${entry.running ? '●' : '○'} ${entry.label}`,
        // 启动/停止在这里由快照决定；页面侧仍会复核实时状态（见 runTrayConfigAction）。
        action: async () => {
          if (entry.running)
            await opts.onStop(entry.instanceId, entry.label)
          else
            await opts.onStart(entry.instanceId, entry.label)
        },
      }))
    }
    if (opts.hidden > 0) {
      children.push(await MenuItem.new({
        id: 'config-more',
        text: labels.more,
        enabled: false,
      }))
    }
  }

  children.push(await PredefinedMenuItem.new({ item: 'Separator' }))
  children.push(await MenuItem.new({
    id: 'config-open-window',
    text: labels.openWindow,
    action: async () => {
      await opts.onOpenWindow()
    },
  }))

  return await Submenu.new({
    id: 'configs',
    text: labels.title,
    items: children,
  })
}

export async function buildTrayMenuItems(opts: TrayMenuBuildOptions) {
  const items: Array<MenuItem | PredefinedMenuItem | Submenu> = [
    await MenuItemShow(opts.showLabel),
  ]

  if (opts.configMenu) {
    items.push(await PredefinedMenuItem.new({ item: 'Separator' }))
    items.push(await buildTrayConfigSubmenu(opts.configMenu))
  }

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

export async function setTrayMenu(items: (MenuItem | PredefinedMenuItem | Submenu)[] | undefined = undefined) {
  const tray = await useTray()
  if (!tray)
    return
  const menu = await Menu.new({
    id: 'main',
    items: items || await generateMenuItem(),
  })
  tray.setMenu(menu)
}

/**
 * OpenVPN-like tray chrome: grey idle / yellow connecting / green running /
 * red needs attention. The state itself is derived in `tray_run_state.ts` from
 * the device's instances — `clientRunning` only enters as one input, so an app
 * that is up with no network running shows grey, not green.
 */
export type { TrayRunState }

const TRAY_ICONS: Record<TrayRunState, string> = {
  idle: 'icons/tray-idle.ico',
  connecting: 'icons/tray-connecting.ico',
  running: 'icons/tray-running.ico',
  error: 'icons/tray-error.ico',
}

let appliedTrayState: TrayRunState | null = null
/** Nested begin/end so 1s poll / clientRunning watch cannot wipe yellow mid-connect. */
let trayConnectingHold = 0
/** Newest settled state + tooltip; applied as soon as no connecting hold is left. */
let pendingTrayState: { state: TrayRunState, tooltip?: string } | undefined

async function resolveTrayIconPath(state: TrayRunState): Promise<string> {
  const relative = TRAY_ICONS[state]
  try {
    return await resolveResource(relative)
  }
  catch {
    // Dev / unpackaged builds: same relative path the old inactive icon used.
    return relative
  }
}

async function applyTrayIcon(state: TrayRunState) {
  if (appliedTrayState === state)
    return
  const tray = await useTray()
  if (!tray)
    return
  // Only remember state after setIcon succeeds so a failed swap can be retried.
  await tray.setIcon(await resolveTrayIconPath(state))
  appliedTrayState = state
}

async function applyPendingTrayState() {
  if (!pendingTrayState)
    return
  await applyTrayIcon(pendingTrayState.state)
  if (pendingTrayState.tooltip)
    await setTrayTooltip(pendingTrayState.tooltip)
}

/**
 * Update the tray icon (and optional tooltip). Rejects booleans on purpose: the
 * state is a network judgement, so callers must go through the aggregator in
 * `tray_run_state.ts` instead of passing "the client is up" as a run state.
 * Settled updates are deferred while a {@link beginTrayConnecting} hold is active.
 */
export async function setTrayRunState(state: TrayRunState, tooltip?: string) {
  pendingTrayState = { state, tooltip }
  if (trayConnectingHold > 0)
    return
  await applyPendingTrayState()
}

/**
 * Show yellow connecting icon until matching {@link endTrayConnecting}.
 * `tooltip` describes the transition, since the settled text is stale meanwhile.
 */
export async function beginTrayConnecting(tooltip?: string) {
  trayConnectingHold++
  try {
    await applyTrayIcon('connecting')
    await setTrayTooltip(tooltip ?? pendingTrayState?.tooltip ?? '')
  }
  catch (e) {
    trayConnectingHold = Math.max(0, trayConnectingHold - 1)
    throw e
  }
}

/**
 * Drop one connecting hold; when none remain, apply the newest settled state
 * (or `state`, when the caller refreshed while the hold was active).
 */
export async function endTrayConnecting(state?: TrayRunState) {
  trayConnectingHold = Math.max(0, trayConnectingHold - 1)
  if (trayConnectingHold > 0)
    return
  if (state)
    pendingTrayState = { state, tooltip: pendingTrayState?.tooltip }
  await applyPendingTrayState()
}

let appliedTooltip: string | undefined

export async function setTrayTooltip(tooltip: string) {
  if (!tooltip || tooltip === appliedTooltip)
    return
  const tray = await useTray()
  if (!tray)
    return
  const text = `ET\n${pkg.version}\n${tooltip}`
  tray.setTooltip(text)
  tray.setTitle(text)
  appliedTooltip = tooltip
}
