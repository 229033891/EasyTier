<script setup lang="ts">
import type { MenuItem } from 'primevue/menuitem'
import { invoke } from '@tauri-apps/api/core'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { type } from '@tauri-apps/plugin-os'
import { exit } from '@tauri-apps/plugin-process'
import { open } from '@tauri-apps/plugin-shell'
import { I18nUtils, type LoggingSettingsApi, LoggingSettingsDialog, RemoteManagement, TOAST_LIFE, Utils } from 'easytier-frontend-lib'
import { useConfirm, useToast } from 'primevue'
import ModeSwitcher from '~/components/ModeSwitcher.vue'
import {
  clearLogFiles,
  getEasytierVersion,
  getLoggingLevel,
  getNetworkMetas,
  getServiceStatus,
  listLogFiles,
  parseStoredConfigs,
  readLogFile,
  type ServiceStatus,
  setLoggingLevel,
} from '~/composables/backend'
import { clearLastNetworkInstanceId, loadLastNetworkInstanceId, saveLastNetworkInstanceId } from '~/composables/config'
import {
  dropStoredConfigsByIds,
  filterProvenFormerWebGhosts,
  findFormerWebGhostCandidates,
} from '~/composables/former_web_ghosts'
import { syncGuiAutostart } from '~/composables/gui_autostart'
import {
  consumePendingMobileVpnTileAction,
  initMobileVpnService,
  setMobileVpnTileActionHandler,
  syncMobileVpnService,
} from '~/composables/mobile_vpn'
import { executeVpnTileAction } from '~/composables/mobile_vpn_tile'
import {
  loadFileLogLevel,
  loadMode,
  type Mode,
  normalizeServiceRpcUrl,
  saveFileLogLevel,
  saveMode,
} from '~/composables/mode'
import {
  beginTrayConnecting,
  buildTrayMenuItems,
  endTrayConnecting,
  registerTrayExitHandler,
  setTrayMenu,
  setTrayRunState,
  showMainWindow,
  useTray,
} from '~/composables/tray'
import { GUIRemoteClient } from '~/modules/api'

const { t, locale } = useI18n()
const confirm = useConfirm()
// Declared up front because the functions below reference them: keeping the
// declarations at the bottom tripped `ts/no-use-before-define` in 23 places.
const toast = useToast()
const remoteClient = computed(() => new GUIRemoteClient())
const instanceId = ref<string | undefined>(undefined)
const clientRunning = ref(false)
const aboutVisible = ref(false)
const modeDialogVisible = ref(false)
const configServerConnected = ref(false)
const configServerEnabled = ref(false)
const configServerLastError = ref('')
/** Non-fatal connecting progress (`phase:*`); separate from lastError failures. */
const configServerConnectingDetail = ref('')
/** Set when GUI cannot query status (e.g. service RPC down) — not a config-server dial error. */
const configServerProbeError = ref('')
const currentMode = ref<Mode>({ mode: 'normal' })
const editingMode = ref<Mode>({ mode: 'normal' })
const isModeSaving = ref(false)
const manualDisconnect = ref(false)
/** Last service status shown in the tray menu (avoids unnecessary rebuilds). */
let lastTrayServiceStatus: ServiceStatus | undefined
/** Monotonic token so concurrent refreshTrayMenu calls cannot apply stale menus. */
let trayMenuGeneration = 0
/** Service mode: owner process is stopped / not installed (not a config-server outage). */
const configServerOwnerDown = ref(false)
/** Fingerprint of last applied console W prune (sorted ids joined). */
let lastPrunedConsoleWebOwnedKey: string | undefined
/** Device-known instance ids after last successful RPC connect (running ∪ disabled). */
const knownDeviceInstanceIds = ref<string[]>([])

const showAutostartHint = ref(false)

type ConfigServerStatus = 'connected' | 'disconnected' | 'connecting' | 'failed' | 'unavailable' | 'owner_down' | 'remote'

const configServerStatus = computed<ConfigServerStatus>(() => {
  const mode = currentMode.value
  if (mode.mode === 'remote')
    return 'remote'
  if (!(mode.mode === 'normal' || mode.mode === 'service'))
    return 'disconnected'
  // Do not gate the status on the locally stored URL: binding is decided by
  // `status.enabled`, so a client that is already running must keep reporting
  // its real state even when the settings field is empty (§1).
  if (!mode.config_server_url?.trim() && !configServerEnabled.value && !configServerConnected.value)
    return 'disconnected'
  if (configServerOwnerDown.value)
    return 'owner_down'
  // Cannot reach the process that owns the client — do not pretend the
  // config-server itself failed (common when the Windows service is stopped).
  if (configServerProbeError.value)
    return 'unavailable'
  if (configServerConnected.value)
    return 'connected'
  // Client is installed and retrying (may still carry last dial error as detail).
  if (configServerEnabled.value)
    return 'connecting'
  // URL configured but no client yet, or init failed before the dialer started.
  if (configServerLastError.value)
    return 'failed'
  return 'connecting'
})

const configServerStatusSeverity = computed(() => {
  switch (configServerStatus.value) {
    case 'connected':
      return 'success'
    case 'failed':
    case 'unavailable':
      return 'danger'
    case 'connecting':
    case 'owner_down':
      return 'warn'
    default:
      return 'secondary'
  }
})

const configServerStatusLabel = computed(() => t(`config-server.status_${configServerStatus.value}`))

/** Map stable `phase:*` / failure-prefix codes from the dialer to i18n when possible. */
function localizeConfigServerDetail(raw: string): string {
  const trimmed = raw.trim()
  if (!trimmed)
    return ''
  const phaseKey: Record<string, string> = {
    'phase:negotiating_features': 'config-server.phase_negotiating_features',
    'phase:secure_redial': 'config-server.phase_secure_redial',
    'phase:secure_handshake': 'config-server.phase_secure_handshake',
  }
  const phase = phaseKey[trimmed]
  if (phase)
    return t(phase)
  if (trimmed.startsWith('secure_redial_failed:'))
    return `${t('config-server.phase_secure_redial')} ${trimmed.slice('secure_redial_failed:'.length).trim()}`
  if (trimmed.startsWith('secure_handshake_failed:'))
    return `${t('config-server.phase_secure_handshake')} ${trimmed.slice('secure_handshake_failed:'.length).trim()}`
  return trimmed
}

/** Real failure text (probe / dial). Never includes connecting phase codes. */
const configServerFailureDetail = computed(() => {
  if (configServerOwnerDown.value)
    return ''
  return localizeConfigServerDetail(
    configServerProbeError.value || configServerLastError.value || '',
  )
})

/** Non-fatal handshake progress; only when there is no failure to show. */
const configServerPhaseDetail = computed(() => {
  if (configServerOwnerDown.value || configServerFailureDetail.value)
    return ''
  return localizeConfigServerDetail(configServerConnectingDetail.value)
})

function formatConfigServerProbeError(e: unknown): string {
  const raw = Utils.formatApiErrorDetail(e, t)
  if (raw.includes('rpc_client_unavailable'))
    return t('config-server.rpc_unavailable')
  return raw
}

function notifyOperationInProgress() {
  toast.add({
    severity: 'warn',
    summary: t('web.common.warning'),
    detail: t('mode.operation_in_progress'),
    life: TOAST_LIFE.warn,
  })
}

/**
 * Probe cadence while nothing hints at a config-server client: the binding
 * signal is `status.enabled`, never the locally stored URL (§1「绑定 = WebClient
 * 已创建」). Skipping the probe outright on an empty URL would leave a window
 * where the WebClient is up but the delete button stays unlocked, so keep a
 * slow heartbeat probe instead of a hard short-circuit.
 */
const CONFIG_SERVER_IDLE_PROBE_EVERY_TICKS = 15
let configServerIdleProbeTick = 0

/** Session: already prompted (accept/ignore) for former-Web ghost clear. */
const promptedFormerWebGhostIds = new Set<string>()

/**
 * P1.2: mislabeled user/legacy cache proven Web on device and absent from W →
 * prompt clear (never silent-delete pure local drafts).
 */
async function maybePromptFormerWebGhosts(
  consoleWebOwnedIds: string[],
  runningEnabled: string[],
) {
  const w = new Set(consoleWebOwnedIds)
  const networkList = parseStoredConfigs(localStorage.getItem('networkList'))
  const candidates = findFormerWebGhostCandidates(networkList, w)
    .filter(e => e.config.instance_id && !promptedFormerWebGhostIds.has(e.config.instance_id))
  if (!candidates.length)
    return
  let metas: Record<string, { source?: number | string } | undefined> = {}
  try {
    const resp = await getNetworkMetas(candidates.map(c => c.config.instance_id!))
    metas = resp?.metas ?? {}
  }
  catch (e) {
    console.warn('Failed to load metas for former-web ghost check', e)
    return
  }
  const ghostIds = filterProvenFormerWebGhosts(candidates, metas, w)
  // One dialog at a time is enough; queue the rest only after accept/reject.
  // Avoid awaiting inside config-server poll/init (see maybePruneWebConfigsFromConsoleW).
  const showNext = (index: number) => {
    if (index >= ghostIds.length)
      return
    const id = ghostIds[index]!
    if (promptedFormerWebGhostIds.has(id)) {
      showNext(index + 1)
      return
    }
    promptedFormerWebGhostIds.add(id)
    confirm.require({
      message: t('web.device_management.confirm_clear_former_web_ghost'),
      header: t('web.device_management.clear_former_web_ghost_header'),
      icon: 'pi pi-exclamation-triangle',
      rejectProps: {
        label: t('web.common.cancel'),
        severity: 'secondary',
        outlined: true,
      },
      acceptProps: {
        label: t('web.device_management.clear_device_copy'),
        severity: 'danger',
      },
      accept: () => {
        try {
          const next = dropStoredConfigsByIds(
            parseStoredConfigs(localStorage.getItem('networkList')),
            new Set([id]),
          )
          localStorage.setItem('networkList', JSON.stringify(next))
          void sendConfigs(runningEnabled)
        }
        catch (e) {
          console.warn('Failed to clear former-web ghost', id, e)
        }
        showNext(index + 1)
      },
      reject: () => showNext(index + 1),
    })
  }
  showNext(0)
}

/**
 * Prune GUI `localStorage` web rows using console W only.
 * No-op until heartbeat marks W valid (fail-safe). Empty W clears all web cache.
 */
async function maybePruneWebConfigsFromConsoleW(status: {
  connected?: boolean
  webOwnedInstanceIdsValid?: boolean
  webOwnedInstanceIds?: string[]
}) {
  if (!status.connected || !status.webOwnedInstanceIdsValid)
    return
  const w = status.webOwnedInstanceIds ?? []
  const key = [...w].sort().join(',')
  if (key === lastPrunedConsoleWebOwnedKey)
    return
  lastPrunedConsoleWebOwnedKey = key
  try {
    const listed = await remoteClient.value.list_network_instance_ids().catch(() => undefined)
    const running = (listed?.running_inst_ids ?? []).map(Utils.UuidToStr)
    if (listed) {
      knownDeviceInstanceIds.value = [
        ...running,
        ...(listed.disabled_inst_ids ?? []).map(Utils.UuidToStr),
      ]
    }
    await sendConfigs(running, { consoleWebOwnedInstanceIds: w })
    // Do not await prompts: refresh/init/waitForConfigServerOutcome must not
    // block on ConfirmDialog (and reject may never fire on dismiss).
    void maybePromptFormerWebGhosts(w, running)
  }
  catch (e) {
    lastPrunedConsoleWebOwnedKey = undefined
    console.warn('Failed to prune GUI web configs from console W', e)
  }
}

async function refreshConfigServerConnection() {
  try {
    const mode = currentMode.value
    const hostMode = mode.mode === 'normal' || mode.mode === 'service'
    // Remote mode owns no local config-server client, and a stopped service
    // owner cannot hold one: both are genuinely "not bound" → full reset.
    // An empty URL is **not** — the client may already be running (§1).
    // Narrow inline: `config_server_url` only exists on normal/service modes.
    const hasUrl = (mode.mode === 'normal' || mode.mode === 'service')
      ? !!mode.config_server_url?.trim()
      : false
    // Idle tick only: requires !hasUrl. waitForConfigServerOutcome (has URL)
    // never increments this counter — paths do not race (S9).
    const idlePoll = hostMode
      && !hasUrl
      && !configServerEnabled.value
      && !configServerOwnerDown.value
      && ++configServerIdleProbeTick >= CONFIG_SERVER_IDLE_PROBE_EVERY_TICKS
    const canSkip = !hostMode || (!hasUrl && !configServerEnabled.value && !idlePoll)
    if (canSkip) {
      configServerIdleProbeTick = 0
      configServerConnected.value = false
      configServerEnabled.value = false
      configServerLastError.value = ''
      configServerConnectingDetail.value = ''
      configServerProbeError.value = ''
      // `configServerOwnerDown` stays sticky here. Reaching this branch with a
      // URL configured is impossible (URL ⇒ always probe), so "owner down" is
      // the cheapest proof that no client can be bound; keeping it stops the
      // idle probe from poking an already-stopped service every 15s. The next
      // successful probe clears it.
      return
    }
    configServerIdleProbeTick = 0

    const status = await getConfigServerStatus()
    configServerProbeError.value = ''
    configServerOwnerDown.value = false
    configServerEnabled.value = !!status.enabled
    configServerConnected.value = !!status.connected
    if (status.connected) {
      configServerLastError.value = ''
      configServerConnectingDetail.value = ''
      await maybePruneWebConfigsFromConsoleW(status)
    }
    else {
      // Keep empty string when retrying without a recorded failure.
      configServerLastError.value = status.lastError || ''
      configServerConnectingDetail.value = status.connectingDetail || ''
      lastPrunedConsoleWebOwnedKey = undefined
    }
  }
  catch (e) {
    configServerConnected.value = false
    configServerConnectingDetail.value = ''
    lastPrunedConsoleWebOwnedKey = undefined
    // Probe/RPC failure is not the config-server dial error.
    // Keep last known `configServerEnabled` across transient RPC blips so the
    // delete button does not unlock/lock every 1s poll (§1 binding, not online).
    if (currentMode.value.mode === 'service') {
      const svc = await getServiceStatus().catch(() => null)
      if (svc === 'Stopped' || svc === 'NotInstalled') {
        configServerEnabled.value = false
        configServerOwnerDown.value = true
        configServerProbeError.value = ''
        return
      }
    }
    const detail = formatConfigServerProbeError(e)
    if (detail === 'rpc_client_unavailable' || String(e).includes('rpc_client_unavailable')) {
      configServerEnabled.value = false
    }
    configServerOwnerDown.value = false
    configServerProbeError.value = detail
    console.error('Failed to refresh config server connection', e)
  }
}

/**
 * Wait until the config-server client reports connected or a first failure.
 * `initWebClient` only parses the URL and starts a background dialer, so the
 * UI must wait before claiming success.
 */
async function waitForConfigServerOutcome(
  timeoutMs = 25_000,
): Promise<'connected' | 'failed' | 'timeout' | 'disabled'> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    await refreshConfigServerConnection()
    const mode = currentMode.value
    if (!(mode.mode === 'normal' || mode.mode === 'service') || !mode.config_server_url?.trim()) {
      return 'disabled'
    }
    if (configServerConnected.value) {
      return 'connected'
    }
    if (configServerProbeError.value) {
      return 'failed'
    }
    // Fail fast only when the client never started (init error). While enabled,
    // lastError means a dial attempt failed and the background dialer is retrying —
    // keep waiting until connected or timeout.
    if (configServerLastError.value && !configServerEnabled.value) {
      return 'failed'
    }
    await new Promise(resolve => setTimeout(resolve, 500))
  }
  await refreshConfigServerConnection()
  if (configServerConnected.value) {
    return 'connected'
  }
  if (configServerProbeError.value || configServerLastError.value) {
    return 'failed'
  }
  return 'timeout'
}

function normalizeEditingMode(mode: Mode): Mode {
  if (mode.mode === 'remote') {
    return {
      mode: 'remote',
      remote_rpc_address: mode.remote_rpc_address,
    }
  }
  const next = JSON.parse(JSON.stringify(mode)) as Mode
  if (next.mode === 'normal' || next.mode === 'service') {
    next.config_server_url = next.config_server_url?.trim() || undefined
    next.secure_mode = !!next.secure_mode
  }
  if (next.mode === 'service')
    next.file_log_level = loadFileLogLevel()
  return next
}

function configServerEndpointOf(mode: Mode): { url?: string, secure: boolean } {
  if (mode.mode === 'normal' || mode.mode === 'service')
    return { url: mode.config_server_url?.trim() || undefined, secure: !!mode.secure_mode }
  return { url: undefined, secure: false }
}

async function openModeDialog() {
  const loaded = JSON.parse(JSON.stringify(loadMode())) as Mode
  if (loaded.mode === 'service')
    loaded.file_log_level = loadFileLogLevel()
  editingMode.value = loaded
  showAutostartHint.value = false
  modeDialogVisible.value = true
}

async function onModeSave() {
  if (isModeSaving.value) {
    return
  }

  const next = normalizeEditingMode(editingMode.value)
  editingMode.value = next
  const prev = currentMode.value
  const prevEndpoint = configServerEndpointOf(prev)
  const nextEndpoint = configServerEndpointOf(next)
  const configServerChanged = nextEndpoint.url !== prevEndpoint.url
    || nextEndpoint.secure !== prevEndpoint.secure
    || (prev.mode !== next.mode && !!nextEndpoint.url)

  const doSave = async () => {
    isModeSaving.value = true
    try {
      await beginTrayConnecting()
      await initWithMode(next)

      // Validate config-server dial only when Normal mode activates/changes the endpoint.
      const shouldWaitForConfigServer = next.mode === 'normal'
        && !!nextEndpoint.url
        && (configServerChanged || prev.mode !== 'normal')
      if (shouldWaitForConfigServer) {
        // Fail-fast only for sync init failure (enabled=false + lastError).
        // Connecting phase text must not trigger revert.
        const initFailed = !configServerConnected.value
          && !configServerEnabled.value
          && !!configServerLastError.value
        if (initFailed || !!configServerProbeError.value) {
          const detail = localizeConfigServerDetail(
            configServerProbeError.value || configServerLastError.value,
          ) || t('config-server.status_failed')
          await initWithMode(prev)
          editingMode.value = next
          toast.add({
            severity: 'error',
            summary: t('error'),
            detail,
            life: TOAST_LIFE.severe,
          })
          return
        }

        const outcome = await waitForConfigServerOutcome()
        if (outcome !== 'connected' && outcome !== 'disabled') {
          const failureDetail = localizeConfigServerDetail(
            configServerProbeError.value || configServerLastError.value,
          )
          const detail = outcome === 'timeout'
            ? t('config-server.connect_timeout')
            : (failureDetail || t('config-server.status_failed'))
          await initWithMode(prev)
          editingMode.value = next
          toast.add({
            severity: 'error',
            summary: t('error'),
            detail: prevEndpoint.url
              ? `${detail} (${t('config-server.reverted')})`
              : detail,
            life: TOAST_LIFE.severe,
          })
          return
        }
      }

      modeDialogVisible.value = false
    }
    catch (e: any) {
      toast.add({
        severity: 'error',
        summary: t('error'),
        detail: Utils.formatApiErrorDetail(e, t),
        life: TOAST_LIFE.severe,
      })
      console.error('Error switching mode', e, prev, next)
      try {
        await initWithMode(prev)
      }
      catch (revertError) {
        console.error('Failed to revert mode after save error', revertError)
      }
      editingMode.value = next
    }
    finally {
      isModeSaving.value = false
      try {
        await endTrayConnecting(clientRunning.value)
      }
      catch (e) {
        console.warn('Failed to settle tray icon after mode save', e)
      }
      // Service owns the network at boot; GUI must also autostart for the tray.
      await syncGuiAutostart(currentMode.value.mode === 'service')
    }
  }

  const unbinding = !!prevEndpoint.url && !nextEndpoint.url
  const rebinding = !!nextEndpoint.url && configServerChanged && !unbinding

  // Service reinstall/restart applies config-server changes — confirm when that endpoint changes.
  if (next.mode === 'service' && configServerChanged) {
    const serviceMsg = unbinding
      ? `${t('config-server.update_service_confirm')}\n${t('config-server.unbind_keep_local_copies')}`
      : rebinding
        ? `${t('config-server.update_service_confirm')}\n${t('config-server.rebind_console_authoritative')}`
        : t('config-server.update_service_confirm')
    confirm.require({
      message: serviceMsg,
      header: t('config-server.title'),
      icon: 'pi pi-exclamation-triangle',
      rejectProps: {
        label: t('web.common.cancel'),
        severity: 'secondary',
        outlined: true,
      },
      acceptProps: {
        label: t('web.common.save'),
        severity: 'danger',
      },
      accept: () => { void doSave() },
    })
    return
  }

  // Any non-service path that binds/unbinds: confirm copy (config-delete-authority G4).
  // Service path already included the same copy above.
  if (unbinding || rebinding) {
    confirm.require({
      message: unbinding
        ? t('config-server.unbind_keep_local_copies')
        : t('config-server.rebind_console_authoritative'),
      header: t('config-server.title'),
      icon: 'pi pi-exclamation-triangle',
      rejectProps: {
        label: t('web.common.cancel'),
        severity: 'secondary',
        outlined: true,
      },
      acceptProps: {
        label: t('web.common.save'),
        severity: unbinding ? 'warning' : 'danger',
      },
      accept: () => { void doSave() },
    })
    return
  }

  await doSave()
}

function serviceStatusLabel(status: ServiceStatus) {
  return t(`mode.service_status_${status.toLowerCase()}`)
}

async function refreshTrayMenu(forceStatus?: ServiceStatus) {
  if (type() === 'android')
    return

  const generation = ++trayMenuGeneration
  const serviceMode = currentMode.value.mode === 'service'
  let serviceStatus: ServiceStatus | undefined
  if (serviceMode) {
    serviceStatus = forceStatus ?? await getServiceStatus().catch(() => lastTrayServiceStatus ?? 'NotInstalled')
    if (generation !== trayMenuGeneration)
      return
    lastTrayServiceStatus = serviceStatus
  }
  else {
    lastTrayServiceStatus = undefined
  }

  const items = await buildTrayMenuItems({
    showLabel: t('tray.show'),
    exitLabel: t('tray.exit'),
    serviceMode,
    serviceStatus,
    serviceLabels: serviceMode && serviceStatus
      ? {
          status: t('tray.service_status', { status: serviceStatusLabel(serviceStatus) }),
          start: t('tray.start_service'),
          stop: t('tray.stop_service'),
          restart: t('tray.restart_service'),
          uninstall: t('tray.uninstall_service'),
        }
      : undefined,
    actions: {
      onExit: () => void exitApp(),
      onStartService: () => void onStartService(),
      onStopService: () => void onStopService(),
      onRestartService: () => void onRestartService(),
      onUninstallService: () => void onUninstallService(),
    },
  })
  if (generation !== trayMenuGeneration)
    return
  await setTrayMenu(items)
}

async function reconnectServiceRpc() {
  if (currentMode.value.mode !== 'service')
    return
  const url = normalizeServiceRpcUrl(currentMode.value.rpc_portal)
  await connectRpcWithRetries(false, url, 5)
  clientRunning.value = await isClientRunning().catch(() => false)
  await setTrayRunState(clientRunning.value ? 'running' : 'idle')
  await refreshConfigServerConnection()
}

async function onStartService() {
  if (currentMode.value.mode !== 'service')
    return
  if (isModeSaving.value) {
    notifyOperationInProgress()
    return
  }
  isModeSaving.value = true
  try {
    await beginTrayConnecting()
    await setServiceStatus(true)
    await waitForServiceStatus('Running')
    await reconnectServiceRpc()
    toast.add({
      severity: 'success',
      summary: t('web.common.success'),
      detail: t('mode.start_service_success'),
      life: TOAST_LIFE.success,
    })
  }
  catch (e: any) {
    toast.add({
      severity: 'error',
      summary: t('error'),
      detail: Utils.formatApiErrorDetail(e, t),
      life: TOAST_LIFE.severe,
    })
    console.error('Error starting service', e)
  }
  finally {
    isModeSaving.value = false
    try {
      await endTrayConnecting(clientRunning.value)
    }
    catch (e) {
      console.warn('Failed to settle tray icon after start service', e)
    }
    await refreshTrayMenu()
  }
}

async function onRestartService() {
  if (currentMode.value.mode !== 'service')
    return
  if (isModeSaving.value) {
    notifyOperationInProgress()
    return
  }
  await showMainWindow()
  confirm.require({
    message: t('mode.restart_service_confirm'),
    header: t('mode.restart_service'),
    icon: 'pi pi-exclamation-triangle',
    rejectProps: {
      label: t('web.common.cancel'),
      severity: 'secondary',
      outlined: true,
    },
    acceptProps: {
      label: t('mode.restart_service'),
      severity: 'warn',
    },
    accept: async () => {
      isModeSaving.value = true
      try {
        await beginTrayConnecting()
        await bounceService()
        await reconnectServiceRpc()
        toast.add({
          severity: 'success',
          summary: t('web.common.success'),
          detail: t('mode.restart_service_success'),
          life: TOAST_LIFE.success,
        })
      }
      catch (e: any) {
        toast.add({
          severity: 'error',
          summary: t('error'),
          detail: Utils.formatApiErrorDetail(e, t),
          life: TOAST_LIFE.severe,
        })
        console.error('Error restarting service', e)
      }
      finally {
        isModeSaving.value = false
        try {
          await endTrayConnecting(clientRunning.value)
        }
        catch (e) {
          console.warn('Failed to settle tray icon after restart service', e)
        }
        await refreshTrayMenu()
      }
    },
  })
}

async function onUninstallService() {
  if (isModeSaving.value) {
    notifyOperationInProgress()
    return
  }
  await showMainWindow()
  confirm.require({
    message: t('mode.uninstall_service_confirm'),
    header: t('mode.uninstall_service'),
    icon: 'pi pi-exclamation-triangle',
    rejectProps: {
      label: t('web.common.cancel'),
      severity: 'secondary',
      outlined: true,
    },
    acceptProps: {
      label: t('mode.uninstall_service'),
      severity: 'danger',
    },
    accept: async () => {
      isModeSaving.value = true
      try {
        await beginTrayConnecting()
        await initWithMode({ ...currentMode.value, mode: 'normal' })
        await initService(undefined)
        toast.add({
          severity: 'success',
          summary: t('web.common.success'),
          detail: t('mode.uninstall_service_success'),
          life: TOAST_LIFE.success,
        })
        modeDialogVisible.value = false
      }
      catch (e: any) {
        toast.add({
          severity: 'error',
          summary: t('error'),
          detail: Utils.formatApiErrorDetail(e, t),
          life: TOAST_LIFE.severe,
        })
        console.error('Error uninstalling service', e)
      }
      finally {
        isModeSaving.value = false
        try {
          await endTrayConnecting(clientRunning.value)
        }
        catch (e) {
          console.warn('Failed to settle tray icon after uninstall', e)
        }
        await syncGuiAutostart(currentMode.value.mode === 'service')
        await refreshTrayMenu()
      }
    },
  })
}

async function exitApp() {
  if (isModeSaving.value) {
    notifyOperationInProgress()
    await showMainWindow()
    return
  }
  if (currentMode.value.mode === 'service') {
    try {
      const status = await getServiceStatus()
      if (status === 'Running') {
        manualDisconnect.value = true
        await setServiceStatus(false)
        await waitForServiceStatus('Stopped', 40, 250)
      }
      const after = await getServiceStatus()
      if (after === 'Running') {
        throw new Error(t('mode.service_stop_before_exit_failed'))
      }
    }
    catch (e) {
      console.error('Failed to stop service before exit', e)
      toast.add({
        severity: 'error',
        summary: t('error'),
        detail: Utils.formatApiErrorDetail(e, t),
        life: TOAST_LIFE.severe,
      })
      await showMainWindow()
      return
    }
  }
  await exit(0)
}

function stripModeMetadata(mode: Mode) {
  if (mode.mode !== 'service') {
    return mode
  }

  const serviceConfig = { ...mode }
  delete serviceConfig.installed_core_version
  return serviceConfig
}

function modeConfigChanged(next: Mode) {
  return JSON.stringify(stripModeMetadata(next)) !== JSON.stringify(stripModeMetadata(currentMode.value))
}

async function onStopService() {
  if (currentMode.value.mode !== 'service')
    return
  if (isModeSaving.value) {
    notifyOperationInProgress()
    return
  }
  await showMainWindow()
  confirm.require({
    message: t('mode.stop_service_confirm'),
    header: t('mode.stop_service'),
    icon: 'pi pi-exclamation-triangle',
    rejectProps: {
      label: t('web.common.cancel'),
      severity: 'secondary',
      outlined: true,
    },
    acceptProps: {
      label: t('mode.stop_service'),
      severity: 'warn',
    },
    accept: async () => {
      isModeSaving.value = true
      manualDisconnect.value = true
      try {
        await setServiceStatus(false)
        await waitForServiceStatus('Stopped')
        clientRunning.value = false
        await setTrayRunState('idle')
        toast.add({
          severity: 'success',
          summary: t('web.common.success'),
          detail: t('mode.stop_service_success'),
          life: TOAST_LIFE.success,
        })
      }
      catch (e: any) {
        toast.add({
          severity: 'error',
          summary: t('error'),
          detail: Utils.formatApiErrorDetail(e, t),
          life: TOAST_LIFE.severe,
        })
        console.error('Error stopping service', e)
      }
      finally {
        isModeSaving.value = false
        await refreshTrayMenu()
      }
    },
  })
}

function sleep(ms: number) {
  return new Promise(resolve => setTimeout(resolve, ms))
}

/** Wait until Windows/macOS service reaches an expected status; throws on timeout. */
async function waitForServiceStatus(expected: ServiceStatus, attempts = 40, intervalMs = 250): Promise<ServiceStatus> {
  let status = await getServiceStatus()
  for (let i = 0; i < attempts; i++) {
    if (status === expected)
      return status
    await sleep(intervalMs)
    status = await getServiceStatus()
  }
  throw new Error(t('mode.service_status_timeout', {
    expected: serviceStatusLabel(expected),
    actual: serviceStatusLabel(status),
  }))
}

/**
 * Bounce a hung service that still reports Running while its RPC portal is dead.
 * Soft reconnect / initWithMode previously only started Stopped services, so
 * Windows users could get stuck on "无法连接至远程客户端" forever.
 */
async function bounceService(): Promise<void> {
  const status = await getServiceStatus()
  if (status === 'NotInstalled')
    throw new Error('Service not installed')
  if (status === 'Running') {
    manualDisconnect.value = true
    await setServiceStatus(false)
    await waitForServiceStatus('Stopped')
  }
  await setServiceStatus(true)
  await waitForServiceStatus('Running')
}

async function connectRpcWithRetries(isNormalMode: boolean, url: string | undefined, retrys: number) {
  for (let i = 0; i < retrys; i++) {
    try {
      await connectRpcClient(isNormalMode, url)
      return
    }
    catch (e) {
      if (i === retrys - 1)
        throw e
      console.error('Error connecting rpc client, retrying...', e)
      await sleep(1000)
    }
  }
}

async function initWithMode(mode: Mode) {
  if (currentMode.value.mode === 'service' && mode.mode !== 'service') {
    let serviceStatus = await getServiceStatus()
    if (serviceStatus === 'Running') {
      manualDisconnect.value = true
      await setServiceStatus(false)
      try {
        serviceStatus = await waitForServiceStatus('Stopped')
      }
      catch {
        // Soft-fail: mode switch still proceeds if uninstallable (Stopped).
        serviceStatus = await getServiceStatus()
      }
    }
    if (serviceStatus === 'Stopped') {
      await initService(undefined)
    }
  }

  let url: string | undefined
  let retrys = 1
  switch (mode.mode) {
    case 'remote':
      if (!mode.remote_rpc_address) {
        toast.add({ severity: 'error', summary: t('error'), detail: t('mode.remote_rpc_address_empty'), life: TOAST_LIFE.severe })
        return initWithMode({ ...mode, mode: 'normal' })
      }
      url = mode.remote_rpc_address
      // 远程是网络拨号（TCP + RPC 建链）：一两次连不上很常见（移动网络尤甚），给几次重试。
      retrys = 3
      break
    case 'service': {
      // Log level is owned by the Logging dialog preference; keep install args in sync.
      // Compare against preferred *before* writing mode.file_log_level — onMounted passes
      // currentMode by reference, so an early assign would alias-mutate and make
      // modeConfigChanged() blind to log-level drift.
      const preferredLogLevel = loadFileLogLevel()
      const logLevelChanged = mode.file_log_level !== preferredLogLevel
      if (!mode.config_dir || !mode.file_log_dir || !mode.rpc_portal) {
        toast.add({ severity: 'error', summary: t('error'), detail: t('mode.service_config_empty'), life: TOAST_LIFE.severe })
        return initWithMode({ ...mode, mode: 'normal' })
      }
      let serviceStatus = await getServiceStatus()
      const coreVersion = await getEasytierVersion()
      if (serviceStatus === 'NotInstalled' || modeConfigChanged(mode) || mode.installed_core_version !== coreVersion || logLevelChanged) {
        mode.config_server_url = mode.config_server_url || undefined
        await initService({
          config_dir: mode.config_dir,
          file_log_dir: mode.file_log_dir,
          file_log_level: preferredLogLevel,
          rpc_portal: mode.rpc_portal,
          config_server: mode.config_server_url,
          secure_mode: !!mode.secure_mode,
        })
        mode.installed_core_version = coreVersion
        serviceStatus = await getServiceStatus()
      }
      mode.file_log_level = preferredLogLevel
      if (serviceStatus === 'Stopped') {
        await setServiceStatus(true)
      }
      url = normalizeServiceRpcUrl(mode.rpc_portal)
      retrys = 5
      break
    }
    case 'normal':
      url = mode.rpc_portal
      // 带 portal 时同样要过网络；ring（无 portal）是进程内直连，一次即可。
      retrys = url ? 3 : 1
      break
  }
  try {
    await connectRpcWithRetries(mode.mode === 'normal', url, retrys)
  }
  catch (e) {
    // Service still "Running" but RPC portal dead — bounce once, then retry.
    if (mode.mode === 'service') {
      console.error('Service RPC connect failed; bouncing service', e)
      await bounceService()
      try {
        await connectRpcWithRetries(false, url, retrys)
      }
      catch (e2) {
        toast.add({
          severity: 'error',
          summary: t('error'),
          detail: t('mode.rpc_connection_failed', { error: Utils.formatApiErrorDetail(e2, t) }),
          life: TOAST_LIFE.severe,
        })
        throw e2
      }
    }
    else {
      toast.add({
        severity: 'error',
        summary: t('error'),
        detail: t('mode.rpc_connection_failed', { error: Utils.formatApiErrorDetail(e, t) }),
        life: TOAST_LIFE.severe,
      })
      throw e
    }
  }
  // List AFTER RPC connect. knownIds only when list succeeds — a failed list
  // must not clear lastInstanceId via [].
  //
  // Web-cache prune uses console W (not R∪D); see maybePruneWebConfigsFromConsoleW
  // after config-server heartbeat (local-config-residue.md §4.1).
  const listed = await remoteClient.value.list_network_instance_ids().catch(() => undefined)
  const running_inst_ids = listed?.running_inst_ids ?? []
  const disabled_inst_ids = listed?.disabled_inst_ids ?? []
  if (listed) {
    knownDeviceInstanceIds.value = [
      ...running_inst_ids.map(Utils.UuidToStr),
      ...disabled_inst_ids.map(Utils.UuidToStr),
    ]
  }
  await sendConfigs(running_inst_ids.map(Utils.UuidToStr))
  if (mode.mode === 'normal') {
    mode.config_server_url = mode.config_server_url || undefined
    try {
      await initWebClient(mode.config_server_url, mode.secure_mode)
      // Connection happens asynchronously in the background dialer.
      // Do not clear lastError here — waitForConfigServerOutcome / refresh owns that.
    }
    catch (e: any) {
      configServerConnected.value = false
      configServerEnabled.value = false
      configServerProbeError.value = ''
      configServerOwnerDown.value = false
      configServerLastError.value = Utils.formatApiErrorDetail(e, t)
      console.error('Failed to init web client', e)
    }
  }
  else if (mode.mode === 'service') {
    // Config-server client runs inside the service process; poll its status via RPC.
    configServerProbeError.value = ''
    configServerOwnerDown.value = false
  }
  else {
    configServerConnected.value = false
    configServerEnabled.value = false
    configServerLastError.value = ''
    configServerProbeError.value = ''
    configServerOwnerDown.value = false
  }
  if (mode.mode === 'normal' || mode.mode === 'service') {
    await refreshConfigServerConnection()
    // Apply preferred file log level to the live process (Logging dialog is the editor).
    try {
      await setLoggingLevel(loadFileLogLevel())
    }
    catch (e) {
      console.warn('Failed to apply preferred log level', e)
    }
  }
  currentMode.value = mode
  saveMode(mode)
  clientRunning.value = await isClientRunning()
  await setTrayRunState(clientRunning.value ? 'running' : 'idle')
  await refreshTrayMenu()
}

onMounted(async () => {
  const cleanupFns: Array<() => void> = []

  if (type() === 'android') {
    try {
      await initMobileVpnService()
    }
    catch (e: any) {
      console.error('easytier init vpn service failed', e)
    }
  }

  cleanupFns.push(await listenGlobalEvents())
  const onSyncedOverwrite = (event: Event) => {
    const detail = (event as CustomEvent<{ instanceId?: string }>).detail
    toast.add({
      severity: 'info',
      summary: t('web.common.success'),
      detail: t('web.device_management.managed_config_synced_overwrite', {
        id: detail?.instanceId || '',
      }),
      life: TOAST_LIFE.success,
    })
  }
  window.addEventListener('managed_config_synced_overwrite', onSyncedOverwrite)
  cleanupFns.push(() => {
    window.removeEventListener('managed_config_synced_overwrite', onSyncedOverwrite)
  })
  currentMode.value = loadMode()
  try {
    await beginTrayConnecting()
    await initWithMode(currentMode.value)
  }
  finally {
    try {
      await endTrayConnecting(clientRunning.value)
    }
    catch (e) {
      console.warn('Failed to settle tray icon after startup init', e)
    }
    // Re-assert login autostart so an already-installed service also gets a tray after reboot.
    await syncGuiAutostart(currentMode.value.mode === 'service')
  }

  if (type() === 'android') {
    setMobileVpnTileActionHandler(handleMobileVpnTileAction)
    cleanupFns.push(() => setMobileVpnTileActionHandler())
    try {
      await consumePendingMobileVpnTileAction()
      await syncMobileVpnService()
    }
    catch (e: any) {
      console.error('easytier sync vpn service failed', e)
    }
  }

  const configServerTimer = window.setInterval(() => {
    void refreshConfigServerConnection()
  }, 1000)
  cleanupFns.push(() => clearInterval(configServerTimer))

  onUnmounted(() => {
    cleanupFns.forEach(unlisten => unlisten())
  })
})

// Register before building the tray so early Exit always goes through exitApp
// (stop service in service mode) instead of a bare process exit.
registerTrayExitHandler(() => exitApp())
useTray(true)

async function handleMobileVpnTileAction(action: 'start' | 'stop') {
  try {
    const result = await executeVpnTileAction(action, remoteClient.value, {
      lastInstanceId: loadLastNetworkInstanceId(),
      syncVpnService: syncMobileVpnService,
    })

    if (!result.instanceId) {
      toast.add({
        severity: 'warn',
        summary: t('vpn_tile_no_network'),
        detail: t('vpn_tile_no_network_description'),
        life: TOAST_LIFE.warn,
      })
      return
    }

    instanceId.value = result.instanceId
    saveLastNetworkInstanceId(result.instanceId)
    toast.add({
      severity: action === 'start' ? 'success' : 'info',
      summary: t(action === 'start' ? 'vpn_tile_started' : 'vpn_tile_stopped'),
      life: TOAST_LIFE.success,
    })
  }
  catch (error) {
    console.error('VPN tile action failed', action, error)
    toast.add({
      severity: 'error',
      summary: t('error'),
      detail: t('vpn_tile_action_failed', { error: Utils.formatApiErrorDetail(error, t) }),
      life: TOAST_LIFE.severe,
    })
  }
}

watch(instanceId, (newVal) => {
  if (newVal) {
    saveLastNetworkInstanceId(newVal)
  }
})

watch(clientRunning, async (newVal, oldVal) => {
  await setTrayRunState(newVal ? 'running' : 'idle')
  if (!newVal && oldVal) {
    if (manualDisconnect.value) {
      manualDisconnect.value = false
      return
    }
    // Avoid overlapping reconnect / mode-save races (common on Windows service bounce).
    if (isModeSaving.value)
      return
    await reconnectClient()
  }
  else if (newVal && !oldVal) {
    const lastInstanceId = loadLastNetworkInstanceId()
    // Do not restore a selection the device no longer has (console delete +
    // GUI reopen would otherwise reopen a ghost "已停止" editor from cache).
    if (lastInstanceId && knownDeviceInstanceIds.value.includes(lastInstanceId)) {
      instanceId.value = lastInstanceId
    }
    else if (lastInstanceId) {
      clearLastNetworkInstanceId()
    }
  }
})

onMounted(async () => {
  clientRunning.value = await isClientRunning().catch(() => false)
  await setTrayRunState(clientRunning.value ? 'running' : 'idle')
  const timer = setInterval(async () => {
    try {
      clientRunning.value = await isClientRunning()
    }
    catch (e) {
      clientRunning.value = false
      console.error('Error checking client running status', e)
    }
    if (currentMode.value.mode === 'service' && !isModeSaving.value) {
      try {
        const status = await getServiceStatus()
        if (status !== lastTrayServiceStatus)
          await refreshTrayMenu(status)
      }
      catch (e) {
        console.error('Error checking service status for tray', e)
      }
    }
  }, 1000)

  onUnmounted(() => {
    clearInterval(timer)
  })
})
async function reconnectClient() {
  // Match main: always full init. Soft reconnect (3856ef51) left Android/Windows
  // stuck on "无法连接至远程客户端" even after later recovery patches.
  if (isModeSaving.value)
    return
  editingMode.value = JSON.parse(JSON.stringify(loadMode()))
  await onModeSave()
}

onMounted(async () => {
  window.setTimeout(async () => {
    await refreshTrayMenu()
  }, 1000)
})

const loggingDialogVisible = ref(false)

async function getLogDirPath(): Promise<string> {
  return await invoke<string>('get_log_dir_path')
}

const loggingApi = computed<LoggingSettingsApi>(() => ({
  getLoggerLevel: async () => {
    try {
      return { level: await getLoggingLevel(), live: true }
    }
    catch {
      // RPC down — preference only; Dialog must not present this as the live level.
      return { level: loadFileLogLevel(), live: false }
    }
  },
  setLoggerLevel: async (level: string) => {
    // Remote mode edits the remote process only; do not overwrite local preference.
    // Keep mode.file_log_level as the last *installed* service args; preference
    // diverging from it triggers a reinstall on the next initWithMode.
    if (currentMode.value.mode !== 'remote')
      saveFileLogLevel(level)
    try {
      await setLoggingLevel(level)
    }
    catch (e) {
      if (currentMode.value.mode === 'remote')
        throw e
      const running = await isClientRunning().catch(() => false)
      if (running)
        throw e
      // Preference is saved; core is not running yet — apply on next connect.
    }
  },
  getLogDir: getLogDirPath,
  listLogFiles,
  readLogFile,
  clearLogFiles: async () => {
    const extraDirs = currentMode.value.mode === 'service' && currentMode.value.file_log_dir?.trim()
      ? [currentMode.value.file_log_dir.trim()]
      : []
    return clearLogFiles(extraDirs)
  },
  canOpenLogDir: type() !== 'android',
  openLogDir: async () => {
    await open(await getLogDirPath())
  },
  copyLogDir: async () => {
    await writeText(await getLogDirPath())
  },
  copyText: async (text: string) => {
    await writeText(text)
  },
}))

function openLoggingDialog() {
  loggingDialogVisible.value = true
}

function getLabel(item: MenuItem) {
  return typeof item.label === 'function' ? item.label() : item.label
}

const settingsDialogStyle = { width: 'min(480px, calc(100vw - 1.5rem))' }

const settings_menu = ref()
// 底部设置弹出菜单：各项统一为一级入口，复杂配置走弹窗
const setting_menu_items: Ref<MenuItem[]> = ref([
  {
    label: () => {
      const modeLabel = t(`mode.${currentMode.value.mode}`)
      if (currentMode.value.mode === 'remote')
        return `${t('mode.runtime_settings')}: ${modeLabel}`
      return `${t('mode.runtime_settings')}: ${modeLabel} · ${configServerStatusLabel.value}`
    },
    icon: 'pi pi-cog',
    command: openModeDialog,
  },
  {
    label: () => `${t('exchange_language')}: ${locale.value === 'cn' ? t('language_zh') : t('language_en')}`,
    icon: 'pi pi-language',
    command: async () => {
      await I18nUtils.loadLanguageAsync((locale.value === 'en' ? 'cn' : 'en'))
      await refreshTrayMenu()
    },
  },
  {
    label: () => t('logging'),
    icon: 'pi pi-file',
    command: openLoggingDialog,
  },
  {
    label: () => t('about.title'),
    icon: 'pi pi-info-circle',
    command: async () => {
      aboutVisible.value = true
    },
  },
  {
    label: () => t('exit_app'),
    icon: 'pi pi-power-off',
    command: async () => {
      await exitApp()
    },
  },
])

async function connectRpcClient(isNormalMode: boolean, url?: string) {
  await initRpcConnection(isNormalMode, url)
  console.log('easytier rpc connection established, isNormalMode: ', isNormalMode)
}
</script>

<template>
  <div id="root" class="flex flex-col">
    <Dialog
      v-model:visible="aboutVisible" modal :header="t('about.title')" :style="settingsDialogStyle"
      class="app-dialog"
    >
      <About />
      <template #footer>
        <Button :label="t('close')" icon="pi pi-times" text autofocus @click="aboutVisible = false" />
      </template>
    </Dialog>
    <Dialog
      v-model:visible="modeDialogVisible" modal :header="t('mode.runtime_settings')"
      :style="settingsDialogStyle" class="app-dialog"
    >
      <Message v-if="showAutostartHint" severity="info" :closable="false" class="mb-3">
        {{ t('mode.autostart_hint') }}
      </Message>
      <ModeSwitcher
        v-model="editingMode"
        :normal-mode-only="type() === 'android'"
        :config-server-status-label="configServerStatusLabel"
        :config-server-status-severity="configServerStatusSeverity"
        :config-server-last-error="configServerFailureDetail"
        :config-server-connecting-detail="configServerPhaseDetail"
      />
      <template #footer>
        <Button
          :label="t('web.common.cancel')" icon="pi pi-times" text :disabled="isModeSaving"
          @click="modeDialogVisible = false"
        />
        <Button :label="t('web.common.save')" icon="pi pi-save" autofocus :loading="isModeSaving" @click="onModeSave" />
      </template>
    </Dialog>
    <LoggingSettingsDialog v-model:visible="loggingDialogVisible" :api="loggingApi" />

    <RemoteManagement
      v-if="clientRunning" v-model:instance-id="instanceId" class="flex-1 overflow-y-auto"
      :api="remoteClient" :pause-auto-refresh="isModeSaving"
      host-surface="gui-client"
      :delete-authority="configServerEnabled ? 'console' : 'local'"
    >
      <!-- 与共享底部栏同一行：样式与禁用网络完全一致 -->
      <template #footer-extra>
        <Button
          :label="t('system_settings')" severity="secondary"
          class="network-footer-btn network-footer-btn--muted" @click="settings_menu.toggle($event)"
        />
        <Menu ref="settings_menu" :model="setting_menu_items" :popup="true" class="settings-popup">
          <template #item="{ item, props }">
            <a v-bind="props.action">
              <span v-if="item.icon" :class="item.icon" />
              <span class="settings-item-label">{{ getLabel(item) }}</span>
              <span v-if="item.items?.length" class="pi pi-angle-right settings-submenu-icon" />
            </a>
          </template>
        </Menu>
      </template>
    </RemoteManagement>
    <div v-else class="empty-state flex-1 flex flex-col items-center py-12">
      <i class="pi pi-server text-5xl text-secondary mb-4 opacity-50" />
      <div class="text-xl text-center font-medium mb-3">
        {{ t('client.not_running') }}
      </div>
      <Button
        :loading="isModeSaving" :label="t('client.retry')" icon="pi pi-replay" icon-pos="left"
        @click="reconnectClient"
      />
    </div>

    <!-- RPC 未连接时的兜底：保持设置可用 -->
    <div v-if="!clientRunning" class="bottom-action-bar">
      <Button
        :label="t('system_settings')" icon="pi pi-cog" icon-pos="left" severity="secondary" class="bottom-bar-btn"
        @click="settings_menu.toggle($event)"
      />
      <Menu ref="settings_menu" :model="setting_menu_items" :popup="true" class="settings-popup">
        <template #item="{ item, props }">
          <a v-bind="props.action">
            <span v-if="item.icon" :class="item.icon" />
            <span class="settings-item-label">{{ getLabel(item) }}</span>
            <span v-if="item.items?.length" class="pi pi-angle-right settings-submenu-icon" />
          </a>
        </template>
      </Menu>
    </div>
  </div>
</template>

<style scoped lang="postcss">
#root {
  height: 100vh;
  width: 100vw;
}

.p-dropdown :deep(.p-dropdown-panel .p-dropdown-items .p-dropdown-item) {
  padding: 0 0.5rem;
}
</style>

<style>
body {
  height: 100vh;
  width: 100vw;
  padding: 0;
  margin: 0;
  overflow: hidden;
}

.p-menubar .p-menuitem {
  margin: 0;
}

/* 底部操作条：与上方网络底部栏同一套卡片 token */
.bottom-action-bar {
  flex-shrink: 0;
  display: flex;
  gap: 0.5rem;
  padding: 0.55rem 0.75rem;
  padding-bottom: max(0.55rem, env(safe-area-inset-bottom, 0px));
  background: var(--surface-card, #ffffff);
  border: 1px solid var(--et-border-color, #e2e8f0);
  border-radius: var(--et-radius, 0.75rem);
}

/* 未连接时的设置按钮：与底部栏 muted 体系一致 */
.bottom-bar-btn.p-button {
  flex: 1 1 0;
  min-width: 0;
  max-width: 14rem;
  height: var(--et-btn, 2.5rem) !important;
  padding: 0 0.9rem !important;
  font-size: var(--et-fs-body, 0.9375rem) !important;
  font-weight: 600 !important;
  border-radius: var(--et-radius, 0.75rem) !important;
  box-sizing: border-box;
  justify-content: center;
  background: var(--surface-100, #f1f5f9) !important;
  border: 1px solid var(--et-border-color, #e2e8f0) !important;
  color: var(--text-color, #0f172a) !important;
}

.bottom-bar-btn.p-button:hover:not(:disabled) {
  background: var(--surface-200, #e2e8f0) !important;
}

.bottom-bar-btn.p-button .p-button-label {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 设置弹出菜单：与主界面卡片一致；行距/内边距约收紧 25% */
.settings-popup.p-menu {
  background: var(--surface-card, #ffffff) !important;
  border: 1px solid var(--et-border-color, #e2e8f0) !important;
  border-radius: var(--et-radius, 0.75rem) !important;
  box-shadow: 0 12px 28px rgba(15, 23, 42, 0.14) !important;
  padding: 0.25rem !important;
  min-width: 13rem;
  max-width: min(22rem, calc(100vw - 1rem));
}

.settings-popup .p-menu-item-content {
  border-radius: 0.45rem;
}

.settings-popup .p-menu-item-link {
  padding: 0.45rem 0.65rem !important;
  border-radius: 0.45rem;
  font-size: var(--et-fs-body, 0.9375rem) !important;
  font-weight: 600;
  gap: 0.45rem;
}

.settings-item-label {
  flex: 1;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.settings-submenu-icon {
  font-size: 0.7rem;
  opacity: 0.6;
}

/* 系统设置相关弹窗：统一宽度与外观 */
.app-dialog.p-dialog {
  width: min(480px, calc(100vw - 1.5rem)) !important;
  max-width: calc(100vw - 1.5rem);
  border-radius: var(--et-radius, 0.75rem) !important;
  border: 1px solid var(--et-border-color, #e2e8f0) !important;
}

.app-dialog .p-dialog-header {
  font-weight: 700;
}

.app-dialog .p-dialog-content {
  min-height: 4.5rem;
}

.app-dialog .p-dialog-content label {
  font-size: var(--et-fs-body, 0.9375rem);
  font-weight: 600;
  color: var(--text-color, #0f172a);
}

.app-dialog .p-dialog-footer {
  gap: 0.5rem;
}

.app-dialog .p-dialog-content .p-inputtext[readonly] {
  font-size: 0.8125rem;
  overflow-x: auto;
}

.p-select-overlay {
  max-width: calc(100% - 2rem);
}

/* Android / 窄屏：加大触控、防裁切、适配安全区 */
@media (max-width: 640px) {
  .settings-popup.p-menu {
    min-width: min(18rem, calc(100vw - 1rem));
    max-width: calc(100vw - 1rem);
    max-height: min(70dvh, calc(100dvh - 5rem));
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  .settings-popup .p-menu-item-link {
    min-height: 2.15rem;
    padding: 0.55rem 0.7rem !important;
    align-items: center;
  }

  .app-dialog.p-dialog {
    width: calc(100vw - 1rem) !important;
    max-width: calc(100vw - 1rem);
    margin: 0.5rem !important;
    max-height: calc(100dvh - env(safe-area-inset-top, 0px) - env(safe-area-inset-bottom, 0px) - 1rem);
    display: flex;
    flex-direction: column;
  }

  .app-dialog .p-dialog-content {
    flex: 1 1 auto;
    max-height: calc(100dvh - 12rem);
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  .app-dialog .p-dialog-footer {
    flex-wrap: wrap;
    padding-bottom: max(1rem, env(safe-area-inset-bottom, 0px)) !important;
  }

  .app-dialog .p-dialog-footer .p-button {
    flex: 1 1 auto;
    min-width: 0 !important;
  }

  .bottom-action-bar {
    margin: 0.35rem;
    border-radius: calc(var(--et-radius, 0.75rem) - 0.1rem);
  }

  .bottom-bar-btn.p-button {
    max-width: none;
    min-height: 2.75rem !important;
  }
}

/*

.p-tabview-panel {
  height: 100%;
} */
</style>
