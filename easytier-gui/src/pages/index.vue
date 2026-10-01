<script setup lang="ts">

import { type } from '@tauri-apps/plugin-os'

import { invoke } from '@tauri-apps/api/core'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { open } from '@tauri-apps/plugin-shell'
import { exit } from '@tauri-apps/plugin-process'
import { I18nUtils, RemoteManagement, Utils } from "easytier-frontend-lib"
import type { MenuItem } from 'primevue/menuitem'
import { useTray } from '~/composables/tray'
import {
  consumePendingMobileVpnTileAction,
  initMobileVpnService,
  setMobileVpnTileActionHandler,
  syncMobileVpnService,
} from '~/composables/mobile_vpn'
import { executeVpnTileAction } from '~/composables/mobile_vpn_tile'
import { GUIRemoteClient } from '~/modules/api'

import { useToast, useConfirm } from 'primevue'
import { loadMode, saveMode, WebClientConfig, type Mode } from '~/composables/mode'
import { saveLastNetworkInstanceId, loadLastNetworkInstanceId } from '~/composables/config'
import ModeSwitcher from '~/components/ModeSwitcher.vue'
import { getEasytierVersion, getServiceStatus } from '~/composables/backend'

const { t, locale } = useI18n()
const confirm = useConfirm()
const aboutVisible = ref(false)
const modeDialogVisible = ref(false)
const currentMode = ref<Mode>({ mode: 'normal' })
const editingMode = ref<Mode>({ mode: 'normal' })
const isModeSaving = ref(false)
const manualDisconnect = ref(false)

const configServerDialogVisible = ref(false)
const configServerConnected = ref(false)

const showAutostartHint = ref(false)

async function openModeDialog() {
  editingMode.value = JSON.parse(JSON.stringify(loadMode()))
  showAutostartHint.value = false
  modeDialogVisible.value = true
}

async function onModeSave() {
  if (isModeSaving.value) {
    return;
  }
  isModeSaving.value = true
  try {
    await initWithMode(editingMode.value);
    modeDialogVisible.value = false
  }
  catch (e: any) {
    toast.add({ severity: 'error', summary: t('error'), detail: e, life: 10000 })
    console.error("Error switching mode", e, currentMode.value, editingMode.value)
    await initWithMode(currentMode.value);
  }
  finally {
    isModeSaving.value = false
  }
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
        toast.add({ severity: 'success', summary: t('web.common.success'), detail: t('mode.uninstall_service_success'), life: 3000 })
        modeDialogVisible.value = false
      } catch (e: any) {
        toast.add({ severity: 'error', summary: t('error'), detail: e, life: 10000 })
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
    toast.add({ severity: 'success', summary: t('web.common.success'), detail: t('mode.stop_service_success'), life: 3000 })
    modeDialogVisible.value = false
  }
  catch (e: any) {
    toast.add({ severity: 'error', summary: t('error'), detail: e, life: 10000 })
    console.error("Error stopping service", e)
  }
  finally {
    isModeSaving.value = false
  }
}

async function initWithMode(mode: Mode) {
  const running_inst_ids = (await remoteClient.value.list_network_instance_ids().catch(() => undefined))?.running_inst_ids ?? []

  if (currentMode.value.mode === 'service' && mode.mode !== 'service') {
    let serviceStatus = await getServiceStatus()
    if (serviceStatus === "Running") {
      manualDisconnect.value = true
      await setServiceStatus(false)
      serviceStatus = await getServiceStatus()
      for (let i = 0; i < 10; i++) { // macOS takes a while to stop the service
        if (serviceStatus === "Stopped") {
          break;
        }
        await new Promise(resolve => setTimeout(resolve, 100))
        serviceStatus = await getServiceStatus()
      }
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
        toast.add({ severity: 'error', summary: t('error'), detail: t('mode.remote_rpc_address_empty'), life: 10000 })
        return initWithMode({ ...mode, mode: 'normal' });
      }
      url = mode.remote_rpc_address
      break;
    case 'service': {
      if (!mode.config_dir || !mode.file_log_dir || !mode.file_log_level || !mode.rpc_portal) {
        toast.add({ severity: 'error', summary: t('error'), detail: t('mode.service_config_empty'), life: 10000 })
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
        })
        mode.installed_core_version = coreVersion
        serviceStatus = await getServiceStatus()
      }
      if (serviceStatus === "Stopped") {
        await setServiceStatus(true)
      }
      url = "tcp://" + mode.rpc_portal.replace("0.0.0.0", "127.0.0.1")
      retrys = 5
      break;
    }
    case 'normal':
      url = mode.rpc_portal;
      break;
  }
  for (let i = 0; i < retrys; i++) {
    try {
      await connectRpcClient(mode.mode === 'normal', url)
      break;
    } catch (e) {
      if (i === retrys - 1) {
        const errMsg = e instanceof Error ? e.message : String(e)
        toast.add({
          severity: 'error',
          summary: t('error'),
          detail: t('mode.rpc_connection_failed', { error: errMsg }),
          life: 1000,
        })
        throw e;
      }
      console.error("Error connecting rpc client, retrying...", e)
      await new Promise(resolve => setTimeout(resolve, 1000))
    }
  }
  await sendConfigs(running_inst_ids.map(Utils.UuidToStr))
  if (mode.mode === 'normal') {
    mode.config_server_url = mode.config_server_url || undefined
    initWebClient(mode.config_server_url)
  }
  currentMode.value = mode
  saveMode(mode)
  clientRunning.value = await isClientRunning()
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
        life: 5000,
      })
      return
    }

    instanceId.value = result.instanceId
    saveLastNetworkInstanceId(result.instanceId)
    toast.add({
      severity: action === 'start' ? 'success' : 'secondary',
      summary: t(action === 'start' ? 'vpn_tile_started' : 'vpn_tile_stopped'),
      life: 3000,
    })
  }
  catch (error) {
    console.error('VPN tile action failed', action, error)
    toast.add({
      severity: 'error',
      summary: t('error'),
      detail: t('vpn_tile_action_failed', { error: String(error) }),
      life: 8000,
    })
  }
}

watch(instanceId, (newVal) => {
  if (newVal) {
    saveLastNetworkInstanceId(newVal);
  }
});

watch(clientRunning, async (newVal, oldVal) => {
  if (!newVal && oldVal) {
    if (manualDisconnect.value) {
      manualDisconnect.value = false
      return
    }
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
  editingMode.value = JSON.parse(JSON.stringify(loadMode()));
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

let current_log_level = 'off'

// 从后端获取正确的日志路径
async function getLogDirPath(): Promise<string> {
  return await invoke<string>('get_log_dir_path')
}

function getLabel(item: MenuItem) {
  return typeof item.label === 'function' ? item.label() : item.label
}

const settings_menu = ref()
// 底部只剩两个按钮：[切换模式] 独立按钮 + [设置] 弹出菜单（开机自启动已并入切换模式对话框，不再单独占一项）
const setting_menu_items: Ref<MenuItem[]> = ref([
  {
    label: () => t('exchange_language'),
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
    label: () => `${t('config-server.title')}${t('config-server.' + configServerConnectionStatus.value)}`,
    icon: 'pi pi-globe',
    command: openConfigServerDialog,
    visible: () => ["normal", "service"].includes(currentMode.value.mode),
  },
  {
    label: () => t('logging'),
    icon: 'pi pi-file',
    items: [
      ...['off', 'warn', 'info', 'debug', 'trace'].map(level => ({
        label: () => t(`logging_level_${level}`) + (current_log_level === level ? ' ✓' : ''),
        command: async () => {
          current_log_level = level
          await setLoggingLevel(level)
        },
      })),
      {
        separator: true,
      },
      {
        label: () => t('logging_open_dir'),
        icon: 'pi pi-folder-open',
        command: async () => {
          await open(await getLogDirPath())
        },
        visible: () => type() !== 'android',
      },
      {
        label: () => t('logging_copy_dir'),
        icon: 'pi pi-tablet',
        command: async () => {
          await writeText(await getLogDirPath())
        },
      },
    ],
  },
  {
    label: () => t('about.title'),
    icon: 'pi pi-at',
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

async function openConfigServerDialog() {
  editingMode.value = JSON.parse(JSON.stringify(loadMode()))
  configServerDialogVisible.value = true
}
async function onConfigServerSave() {
  if (JSON.stringify(currentMode.value) === JSON.stringify(editingMode.value)) {
    configServerDialogVisible.value = false
    return;
  }
  if (editingMode.value.mode === 'service') {
    await new Promise<void>((resolve, reject) => {
      confirm.require({
        message: t('config-server.update_service_confirm'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
          label: t('web.common.cancel'),
          severity: 'secondary',
          outlined: true
        },
        acceptProps: {
          label: t('web.common.confirm'),
        },
        accept: async () => {
          resolve()
        },
        reject: () => {
          reject()
        }
      });
    })
  }
  console.log("Saving config server url", (editingMode.value as WebClientConfig).config_server_url)
  await onModeSave();
  configServerDialogVisible.value = false
}
onMounted(() => {
  const timer = setInterval(async () => {
    if (currentMode.value.mode !== 'normal') return;
    if (!currentMode.value.config_server_url) return;
    configServerConnected.value = await isWebClientConnected();
  }, 1000)

  onUnmounted(() => {
    clearInterval(timer)
  })
})
const configServerConnectionStatus = computed(() => {
  if (currentMode.value.mode !== 'normal') {
    return 'unknown'
  }
  if (!currentMode.value.config_server_url) {
    return 'disconnected'
  }
  return configServerConnected.value ? 'connected' : 'connecting'
})

</script>

<template>
  <div id="root" class="flex flex-col">
    <Dialog v-model:visible="aboutVisible" modal :header="t('about.title')" :style="{ width: '70%' }" class="app-dialog">
      <About />
    </Dialog>
    <Dialog v-model:visible="modeDialogVisible" modal :header="t('mode.switch_mode')" :style="{ width: '50vw' }" class="app-dialog">
      <Message v-if="showAutostartHint" severity="info" :closable="false" class="mb-4">
        {{ t('mode.autostart_hint') }}
      </Message>
      <ModeSwitcher v-model="editingMode" @uninstall-service="onUninstallService" @stop-service="onStopService" />
      <template #footer>
        <Button :label="t('web.common.cancel')" icon="pi pi-times" @click="modeDialogVisible = false" text />
        <Button :label="t('web.common.save')" icon="pi pi-save" @click="onModeSave" autofocus :loading="isModeSaving" />
      </template>
    </Dialog>

    <Dialog v-model:visible="configServerDialogVisible" modal :header="t('config-server.title')"
      :style="{ width: '50vw' }" class="app-dialog">
      <div class="flex flex-col gap-3">
        <label for="config-server-address">{{ t('config-server.address') }}</label>
        <InputText id="config-server-address" v-model="(editingMode as WebClientConfig).config_server_url"
          :placeholder="t('config-server.address_placeholder')" />
        <small class="p-text-secondary whitespace-pre-wrap">{{ t('config-server.description') }}</small>
      </div>
      <template #footer>
        <Button :label="t('web.common.cancel')" icon="pi pi-times" @click="configServerDialogVisible = false" text />
        <Button :label="t('web.common.save')" icon="pi pi-save" @click="onConfigServerSave" autofocus
          :loading="isModeSaving" />
      </template>
    </Dialog>

    <RemoteManagement v-if="clientRunning" class="flex-1 overflow-y-auto" :api="remoteClient"
      :pause-auto-refresh="isModeSaving" v-model:instance-id="instanceId" />
    <div v-else class="empty-state flex-1 flex flex-col items-center py-12">
      <i class="pi pi-server text-5xl text-secondary mb-4 opacity-50"></i>
      <div class="text-xl text-center font-medium mb-3">{{ t('client.not_running') }}
      </div>
      <Button @click="reconnectClient" :loading="isModeSaving" :label="t('client.retry')" icon="pi pi-replay"
        iconPos="left" />
    </div>

    <div class="bottom-action-bar">
      <Button v-if="type() !== 'android'" :label="`${t('mode.switch_mode')}: ${t('mode.' + currentMode.mode)}`"
        icon="pi pi-sync" iconPos="left" severity="danger" class="bottom-bar-btn" @click="openModeDialog" />
      <Button :label="t('settings')" icon="pi pi-cog" iconPos="left" severity="danger" class="bottom-bar-btn"
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
  background: var(--surface-card, #ffffff);
  border: 1px solid var(--et-border-color, #e2e8f0);
  border-radius: var(--et-radius, 0.75rem);
}

/* 两个按钮与「禁用网络」同一样式体系（浅红 danger） */
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
  background: color-mix(in srgb, #ef4444 12%, #ffffff) !important;
  border: 1px solid color-mix(in srgb, #ef4444 35%, #e2e8f0) !important;
  color: #b91c1c !important;
}

.bottom-bar-btn.p-button:hover:not(:disabled) {
  background: color-mix(in srgb, #ef4444 20%, #ffffff) !important;
}

.bottom-bar-btn.p-button .p-button-label {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 设置弹出菜单：与主界面卡片一致 */
.settings-popup.p-menu {
  background: var(--surface-card, #ffffff) !important;
  border: 1px solid var(--et-border-color, #e2e8f0) !important;
  border-radius: var(--et-radius, 0.75rem) !important;
  box-shadow: 0 12px 28px rgba(15, 23, 42, 0.14) !important;
  padding: 0.35rem !important;
  min-width: 13rem;
}

.settings-popup .p-menu-item-content {
  border-radius: 0.5rem;
}

.settings-popup .p-menu-item-link {
  padding: 0.6rem 0.8rem !important;
  border-radius: 0.5rem;
  font-size: var(--et-fs-body, 0.875rem) !important;
  font-weight: 600;
  gap: 0.6rem;
}

.settings-item-label {
  flex: 1;
  white-space: nowrap;
}

.settings-submenu-icon {
  font-size: 0.7rem;
  opacity: 0.6;
}

/* 设置相关弹窗：与主界面卡片一致 */
.app-dialog.p-dialog {
  border-radius: var(--et-radius, 0.75rem) !important;
  border: 1px solid var(--et-border-color, #e2e8f0) !important;
  max-width: calc(100vw - 2rem);
}

.app-dialog .p-dialog-header {
  font-weight: 700;
}

.p-select-overlay {
  max-width: calc(100% - 2rem);
}

/*

.p-tabview-panel {
  height: 100%;
} */
</style>
