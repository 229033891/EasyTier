<script setup lang="ts">

import { type } from '@tauri-apps/plugin-os'
import { invoke } from '@tauri-apps/api/core'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { open } from '@tauri-apps/plugin-shell'
import { exit } from '@tauri-apps/plugin-process'
import { I18nUtils, RemoteManagement, LoggingSettingsDialog, Utils, TOAST_LIFE, type LoggingSettingsApi } from 'easytier-frontend-lib'
import type { MenuItem } from 'primevue/menuitem'
import { useTray, setTrayRunState } from '~/composables/tray'
import {
  consumePendingMobileVpnTileAction,
  initMobileVpnService,
  setMobileVpnTileActionHandler,
  syncMobileVpnService,
} from '~/composables/mobile_vpn'
import { executeVpnTileAction } from '~/composables/mobile_vpn_tile'
import { GUIRemoteClient } from '~/modules/api'

import { useToast, useConfirm } from 'primevue'
import { loadMode, saveMode, normalizeServiceRpcUrl, type Mode } from '~/composables/mode'
import { saveLastNetworkInstanceId, loadLastNetworkInstanceId } from '~/composables/config'
import ModeSwitcher from '~/components/ModeSwitcher.vue'
import { getEasytierVersion, getServiceStatus, getLoggingLevel, listLogFiles, readLogFile, setLoggingLevel, type ServiceStatus } from '~/composables/backend'

const { t, locale } = useI18n()
const confirm = useConfirm()
const aboutVisible = ref(false)
const modeDialogVisible = ref(false)
const configServerConnected = ref(false)
const configServerLastError = ref('')
const currentMode = ref<Mode>({ mode: 'normal' })
const editingMode = ref<Mode>({ mode: 'normal' })
const isModeSaving = ref(false)
const manualDisconnect = ref(false)

const showAutostartHint = ref(false)

type ConfigServerStatus = 'connected' | 'disconnected' | 'connecting' | 'failed' | 'remote'

const configServerStatus = computed<ConfigServerStatus>(() => {
  const mode = currentMode.value
  if (mode.mode === 'remote')
    return 'remote'
  if (!(mode.mode === 'normal' || mode.mode === 'service') || !mode.config_server_url?.trim())
    return 'disconnected'
  if (configServerLastError.value)
    return 'failed'
  return configServerConnected.value ? 'connected' : 'connecting'
})

const configServerStatusSeverity = computed(() => {
  switch (configServerStatus.value) {
    case 'connected':
      return 'success'
    case 'failed':
      return 'danger'
    case 'connecting':
      return 'warn'
    default:
      return 'secondary'
  }
})

const configServerStatusLabel = computed(() => t(`config-server.status_${configServerStatus.value}`))

async function refreshConfigServerConnection() {
  try {
    const mode = currentMode.value
    if (!(mode.mode === 'normal' || mode.mode === 'service') || !mode.config_server_url?.trim()) {
      configServerConnected.value = false
      configServerLastError.value = ''
      return
    }

    const status = await getConfigServerStatus()
    configServerConnected.value = !!status.connected
    if (status.connected) {
      configServerLastError.value = ''
    }
    else if (status.lastError) {
      configServerLastError.value = status.lastError
    }
    else if (status.enabled) {
      // Client is retrying without a recorded failure — clear a stale red badge.
      configServerLastError.value = ''
    }
  }
  catch (e) {
    configServerConnected.value = false
    if (currentMode.value.mode === 'service') {
      configServerLastError.value = Utils.formatApiErrorDetail(e, t)
    }
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
    if (configServerLastError.value) {
      return 'failed'
    }
    await new Promise(resolve => setTimeout(resolve, 500))
  }
  await refreshConfigServerConnection()
  if (configServerConnected.value) {
    return 'connected'
  }
  if (configServerLastError.value) {
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
  return next
}

function configServerEndpointOf(mode: Mode): { url?: string, secure: boolean } {
  if (mode.mode === 'normal' || mode.mode === 'service')
    return { url: mode.config_server_url?.trim() || undefined, secure: !!mode.secure_mode }
  return { url: undefined, secure: false }
}

async function openModeDialog() {
  editingMode.value = JSON.parse(JSON.stringify(loadMode()))
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
      await initWithMode(next)

      // Validate config-server dial only when Normal mode activates/changes the endpoint.
      const shouldWaitForConfigServer = next.mode === 'normal'
        && !!nextEndpoint.url
        && (configServerChanged || prev.mode !== 'normal')
      if (shouldWaitForConfigServer) {
        if (configServerLastError.value && !configServerConnected.value) {
          const detail = configServerLastError.value
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
          const detail = outcome === 'timeout'
            ? t('config-server.connect_timeout')
            : (configServerLastError.value || t('config-server.status_failed'))
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
    }
  }

  // Service reinstall/restart applies config-server changes — confirm when that endpoint changes.
  if (next.mode === 'service' && configServerChanged) {
    confirm.require({
      message: t('config-server.update_service_confirm'),
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

  await doSave()
}

async function onUninstallService() {
  confirm.require({
    message: t('mode.uninstall_service_confirm'),
    header: t('mode.uninstall_service'),
    icon: 'pi pi-exclamation-triangle',
    rejectProps: {
      label: t('web.common.cancel'),
      severity: 'secondary',
      outlined: true
    },
    acceptProps: {
      label: t('mode.uninstall_service'),
      severity: 'danger'
    },
    accept: async () => {
      isModeSaving.value = true
      try {
        await initWithMode({ ...currentMode.value, mode: 'normal' });
        await initService(undefined)
        toast.add({ severity: 'success', summary: t('web.common.success'), detail: t('mode.uninstall_service_success'), life: TOAST_LIFE.success })
        modeDialogVisible.value = false
      } catch (e: any) {
        toast.add({
          severity: 'error',
          summary: t('error'),
          detail: Utils.formatApiErrorDetail(e, t),
          life: TOAST_LIFE.severe,
        })
        console.error("Error uninstalling service", e)
      } finally {
        isModeSaving.value = false
      }
    },
  });
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
  isModeSaving.value = true
  manualDisconnect.value = true
  try {
    await setServiceStatus(false)
    toast.add({ severity: 'success', summary: t('web.common.success'), detail: t('mode.stop_service_success'), life: TOAST_LIFE.success })
    modeDialogVisible.value = false
  }
  catch (e: any) {
    toast.add({
      severity: 'error',
      summary: t('error'),
      detail: Utils.formatApiErrorDetail(e, t),
      life: TOAST_LIFE.severe,
    })
    console.error("Error stopping service", e)
  }
  finally {
    isModeSaving.value = false
  }
}

function sleep(ms: number) {
  return new Promise(resolve => setTimeout(resolve, ms))
}

/** Wait until Windows/macOS service reaches an expected status (or timeout). */
async function waitForServiceStatus(expected: ServiceStatus, attempts = 30, intervalMs = 200): Promise<ServiceStatus> {
  let status = await getServiceStatus()
  for (let i = 0; i < attempts; i++) {
    if (status === expected)
      return status
    await sleep(intervalMs)
    status = await getServiceStatus()
  }
  return status
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
  const running_inst_ids = (await remoteClient.value.list_network_instance_ids().catch(() => undefined))?.running_inst_ids ?? []

  if (currentMode.value.mode === 'service' && mode.mode !== 'service') {
    let serviceStatus = await getServiceStatus()
    if (serviceStatus === "Running") {
      manualDisconnect.value = true
      await setServiceStatus(false)
      serviceStatus = await waitForServiceStatus('Stopped', 10, 100)
    }
    if (serviceStatus === "Stopped") {
      await initService(undefined)
    }
  }

  let url: string | undefined = undefined
  let retrys = 1
  switch (mode.mode) {
    case 'remote':
      if (!mode.remote_rpc_address) {
        toast.add({ severity: 'error', summary: t('error'), detail: t('mode.remote_rpc_address_empty'), life: TOAST_LIFE.severe })
        return initWithMode({ ...mode, mode: 'normal' });
      }
      url = mode.remote_rpc_address
      // 远程是网络拨号（TCP + RPC 建链）：一两次连不上很常见（移动网络尤甚），给几次重试。
      retrys = 3
      break;
    case 'service': {
      if (!mode.config_dir || !mode.file_log_dir || !mode.file_log_level || !mode.rpc_portal) {
        toast.add({ severity: 'error', summary: t('error'), detail: t('mode.service_config_empty'), life: TOAST_LIFE.severe })
        return initWithMode({ ...mode, mode: 'normal' });
      }
      let serviceStatus = await getServiceStatus()
      const coreVersion = await getEasytierVersion()
      if (serviceStatus === "NotInstalled" || modeConfigChanged(mode) || mode.installed_core_version !== coreVersion) {
        mode.config_server_url = mode.config_server_url || undefined
        await initService({
          config_dir: mode.config_dir,
          file_log_dir: mode.file_log_dir,
          file_log_level: mode.file_log_level,
          rpc_portal: mode.rpc_portal,
          config_server: mode.config_server_url,
          secure_mode: !!mode.secure_mode,
        })
        mode.installed_core_version = coreVersion
        serviceStatus = await getServiceStatus()
      }
      if (serviceStatus === "Stopped") {
        await setServiceStatus(true)
      }
      url = normalizeServiceRpcUrl(mode.rpc_portal)
      retrys = 5
      break;
    }
    case 'normal':
      url = mode.rpc_portal;
      // 带 portal 时同样要过网络；ring（无 portal）是进程内直连，一次即可。
      retrys = url ? 3 : 1
      break;
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
      configServerLastError.value = Utils.formatApiErrorDetail(e, t)
      console.error('Failed to init web client', e)
    }
  }
  else if (mode.mode === 'service') {
    // Config-server client runs inside the service process; poll its status via RPC.
    configServerLastError.value = ''
  }
  else {
    configServerConnected.value = false
    configServerLastError.value = ''
  }
  if (mode.mode === 'normal' || mode.mode === 'service') {
    await refreshConfigServerConnection()
  }
  currentMode.value = mode
  saveMode(mode)
  clientRunning.value = await isClientRunning()
  await setTrayRunState(clientRunning.value)
}

onMounted(async () => {
  const cleanupFns: Array<() => void> = []

  if (type() === 'android') {
    try {
      await initMobileVpnService()
    } catch (e: any) {
      console.error("easytier init vpn service failed", e)
    }
  }

  cleanupFns.push(await listenGlobalEvents())
  currentMode.value = loadMode()
  await initWithMode(currentMode.value);

  if (type() === 'android') {
    setMobileVpnTileActionHandler(handleMobileVpnTileAction)
    cleanupFns.push(() => setMobileVpnTileActionHandler())
    try {
      await consumePendingMobileVpnTileAction()
      await syncMobileVpnService()
    } catch (e: any) {
      console.error("easytier sync vpn service failed", e)
    }
  }

  const configServerTimer = window.setInterval(() => {
    void refreshConfigServerConnection()
  }, 1000)
  cleanupFns.push(() => clearInterval(configServerTimer))

  onUnmounted(() => {
    cleanupFns.forEach(unlisten => unlisten())
  })
});

useTray(true)
let toast = useToast();

const remoteClient = computed(() => new GUIRemoteClient());
const instanceId = ref<string | undefined>(undefined);
const clientRunning = ref(false);

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
    saveLastNetworkInstanceId(newVal);
  }
});

watch(clientRunning, async (newVal, oldVal) => {
  await setTrayRunState(!!newVal)
  if (!newVal && oldVal) {
    if (manualDisconnect.value) {
      manualDisconnect.value = false
      return
    }
    // Avoid overlapping reconnect / mode-save races (common on Windows service bounce).
    if (isModeSaving.value)
      return
    await reconnectClient()
  } else if (newVal && !oldVal) {
    const lastInstanceId = loadLastNetworkInstanceId();
    if (lastInstanceId) {
      instanceId.value = lastInstanceId;
    }
  }
})

onMounted(async () => {
  clientRunning.value = await isClientRunning().catch(() => false)
  await setTrayRunState(clientRunning.value)
  const timer = setInterval(async () => {
    try {
      clientRunning.value = await isClientRunning()
    } catch (e) {
      clientRunning.value = false
      console.error("Error checking client running status", e)
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
    await setTrayMenu([
      await MenuItemShow(t('tray.show')),
      await MenuItemExit(t('tray.exit')),
    ])
  }, 1000)
})

const loggingDialogVisible = ref(false)

async function getLogDirPath(): Promise<string> {
  return await invoke<string>('get_log_dir_path')
}

const loggingApi = computed<LoggingSettingsApi>(() => ({
  getLoggerLevel: getLoggingLevel,
  setLoggerLevel: async (level: string) => {
    await setLoggingLevel(level)
  },
  getLogDir: getLogDirPath,
  listLogFiles,
  readLogFile,
  canOpenLogDir: type() !== 'android',
  openLogDir: async () => {
    await open(await getLogDirPath())
  },
  copyLogDir: async () => {
    await writeText(await getLogDirPath())
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
      const modeLabel = t('mode.' + currentMode.value.mode)
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
      await setTrayMenu([
        await MenuItemShow(t('tray.show')),
        await MenuItemExit(t('tray.exit')),
      ])
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
      await exit(1)
    },
  },
])

async function connectRpcClient(isNormalMode: boolean, url?: string) {
  await initRpcConnection(isNormalMode, url)
  console.log("easytier rpc connection established, isNormalMode: ", isNormalMode)
}

</script>

<template>
  <div id="root" class="flex flex-col">
    <Dialog v-model:visible="aboutVisible" modal :header="t('about.title')" :style="settingsDialogStyle"
      class="app-dialog">
      <About />
      <template #footer>
        <Button :label="t('close')" icon="pi pi-times" @click="aboutVisible = false" text autofocus />
      </template>
    </Dialog>
    <Dialog v-model:visible="modeDialogVisible" modal :header="t('mode.runtime_settings')"
      :style="settingsDialogStyle" class="app-dialog">
      <Message v-if="showAutostartHint" severity="info" :closable="false" class="mb-3">
        {{ t('mode.autostart_hint') }}
      </Message>
      <ModeSwitcher
        v-model="editingMode"
        :normal-mode-only="type() === 'android'"
        :config-server-status-label="configServerStatusLabel"
        :config-server-status-severity="configServerStatusSeverity"
        :config-server-last-error="configServerStatus === 'failed' ? configServerLastError : ''"
        @uninstall-service="onUninstallService"
        @stop-service="onStopService"
      />
      <template #footer>
        <Button :label="t('web.common.cancel')" icon="pi pi-times" @click="modeDialogVisible = false" text
          :disabled="isModeSaving" />
        <Button :label="t('web.common.save')" icon="pi pi-save" @click="onModeSave" autofocus :loading="isModeSaving" />
      </template>
    </Dialog>
    <LoggingSettingsDialog v-model:visible="loggingDialogVisible" :api="loggingApi" />

    <RemoteManagement v-if="clientRunning" class="flex-1 overflow-y-auto" :api="remoteClient"
      :pause-auto-refresh="isModeSaving" v-model:instance-id="instanceId">
      <!-- 与共享底部栏同一行：样式与禁用网络完全一致 -->
      <template #footer-extra>
        <Button :label="t('system_settings')" icon="pi pi-cog" iconPos="left" severity="secondary"
          class="network-footer-btn network-footer-btn--muted" @click="settings_menu.toggle($event)" />
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
      <i class="pi pi-server text-5xl text-secondary mb-4 opacity-50"></i>
      <div class="text-xl text-center font-medium mb-3">{{ t('client.not_running') }}
      </div>
      <Button @click="reconnectClient" :loading="isModeSaving" :label="t('client.retry')" icon="pi pi-replay"
        iconPos="left" />
    </div>

    <!-- RPC 未连接时的兜底：保持设置可用 -->
    <div v-if="!clientRunning" class="bottom-action-bar">
      <Button :label="t('system_settings')" icon="pi pi-cog" iconPos="left" severity="secondary" class="bottom-bar-btn"
        @click="settings_menu.toggle($event)" />
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
  font-size: var(--et-fs-body, 0.875rem) !important;
  font-weight: 600 !important;
  border-radius: var(--et-radius, 0.75rem) !important;
  box-sizing: border-box;
  justify-content: center;
  background: var(--surface-100, #f1f5f9) !important;
  border: 1px solid var(--et-border-color, #e2e8f0) !important;
  color: var(--text-color, #1e293b) !important;
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
  font-size: var(--et-fs-body, 0.875rem) !important;
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
  font-size: var(--et-fs-body, 0.875rem);
  font-weight: 600;
  color: var(--text-color, #1e293b);
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
