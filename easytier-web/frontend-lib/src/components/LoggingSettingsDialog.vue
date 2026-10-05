<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useToast } from 'primevue'
import Button from 'primevue/button'
import Dialog from 'primevue/dialog'
import InputText from 'primevue/inputtext'
import Select from 'primevue/select'
import Textarea from 'primevue/textarea'
import { TOAST_LIFE } from '../modules/toast'
import type { LoggingSettingsApi } from '../modules/logging'

const props = defineProps<{
  api: LoggingSettingsApi
}>()

const visible = defineModel<boolean>('visible', { default: false })

const { t } = useI18n()
const toast = useToast()

const loggingLevel = ref('warn')
const loggingPath = ref('')
const loggingFiles = ref<Array<{ label: string, value: string }>>([])
const selectedLogFile = ref('')
const loggingContent = ref('')
const isSaving = ref(false)
const isLoading = ref(false)

const loggingLevelOptions = computed(() =>
  ['off', 'error', 'warn', 'info', 'debug', 'trace'].map(level => ({
    label: t(`logging_level_${level}`),
    value: level,
  })),
)

const canShowLogContent = computed(() =>
  !props.api.remoteOnly && !!props.api.listLogFiles && !!props.api.readLogFile,
)

async function loadLoggingContent(fileName?: string) {
  if (!canShowLogContent.value || isLoading.value) {
    return
  }
  isLoading.value = true
  try {
    const files = await props.api.listLogFiles!()
    loggingFiles.value = files.map(file => ({
      label: file.active ? `${file.fileName} (${t('logging_content')})` : file.fileName,
      value: file.fileName,
    }))
    const preferred = fileName
      || selectedLogFile.value
      || files.find(file => file.active)?.fileName
      || files[0]?.fileName
      || ''
    selectedLogFile.value = preferred
    loggingContent.value = preferred
      ? await props.api.readLogFile!(preferred)
      : ''
  }
  catch (e) {
    loggingContent.value = ''
    console.error('Failed to load logs', e)
    toast.add({
      severity: 'error',
      summary: t('error'),
      detail: t('logging_load_failed'),
      life: TOAST_LIFE.severe,
    })
  }
  finally {
    isLoading.value = false
  }
}

async function openDialogState() {
  try {
    loggingLevel.value = props.api.getLoggerLevel
      ? await props.api.getLoggerLevel()
      : loggingLevel.value
  }
  catch (e) {
    console.error('Failed to get logger level', e)
  }
  try {
    loggingPath.value = props.api.getLogDir ? await props.api.getLogDir() : ''
  }
  catch (e) {
    loggingPath.value = ''
    console.error('Failed to get log dir path', e)
  }
  if (canShowLogContent.value) {
    await loadLoggingContent()
  }
  else {
    loggingFiles.value = []
    selectedLogFile.value = ''
    loggingContent.value = ''
  }
}

watch(visible, async (open) => {
  if (open) {
    await openDialogState()
  }
})

async function onSave() {
  if (isSaving.value) {
    return
  }
  isSaving.value = true
  try {
    await props.api.setLoggerLevel(loggingLevel.value)
    visible.value = false
    toast.add({ severity: 'success', summary: t('web.common.success'), life: TOAST_LIFE.success })
  }
  catch (e) {
    console.error('Error saving logging level', e)
    toast.add({
      severity: 'error',
      summary: t('error'),
      detail: String(e),
      life: TOAST_LIFE.severe,
    })
  }
  finally {
    isSaving.value = false
  }
}

async function openLoggingDir() {
  if (!props.api.openLogDir) {
    return
  }
  try {
    await props.api.openLogDir()
  }
  catch (e) {
    toast.add({
      severity: 'error',
      summary: t('error'),
      detail: String(e),
      life: TOAST_LIFE.severe,
    })
  }
}

async function copyLoggingDir() {
  if (!props.api.copyLogDir) {
    return
  }
  try {
    await props.api.copyLogDir()
    toast.add({ severity: 'success', summary: t('logging_copied'), life: TOAST_LIFE.success })
  }
  catch (e) {
    toast.add({
      severity: 'error',
      summary: t('error'),
      detail: String(e),
      life: TOAST_LIFE.severe,
    })
  }
}
</script>

<template>
  <Dialog v-model:visible="visible" modal :header="t('logging')"
    :style="{ width: 'min(720px, calc(100vw - 1.5rem))' }" class="app-dialog">
    <div class="flex flex-col gap-3">
      <div class="flex flex-col gap-2">
        <label for="logging-level">{{ t('logging_level') }}</label>
        <Select id="logging-level" v-model="loggingLevel" :options="loggingLevelOptions" option-label="label"
          option-value="value" class="w-full" />
        <small class="text-color-secondary">{{ t('logging_retention_hint') }}</small>
        <small v-if="api.remoteOnly" class="text-color-secondary">{{ t('logging_remote_hint') }}</small>
      </div>
      <div v-if="api.getLogDir" class="flex flex-col gap-2">
        <label>{{ t('logging_path') }}</label>
        <InputText :model-value="loggingPath" class="w-full" readonly />
        <div class="flex flex-wrap gap-2">
          <Button v-if="api.canOpenLogDir && api.openLogDir" :label="t('logging_open_dir')" icon="pi pi-folder-open"
            severity="secondary" outlined @click="openLoggingDir" />
          <Button v-if="api.copyLogDir" :label="t('logging_copy_dir')" icon="pi pi-copy" severity="secondary" outlined
            @click="copyLoggingDir" />
        </div>
      </div>
      <div v-if="canShowLogContent" class="flex flex-col gap-2">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <label for="logging-file">{{ t('logging_content') }}</label>
          <Button :label="t('logging_refresh')" icon="pi pi-refresh" severity="secondary" text :loading="isLoading"
            @click="loadLoggingContent()" />
        </div>
        <Select id="logging-file" v-model="selectedLogFile" :options="loggingFiles" option-label="label"
          option-value="value" class="w-full" :placeholder="t('logging_file')" :disabled="!loggingFiles.length"
          @update:model-value="(value: string) => loadLoggingContent(value)" />
        <Textarea :model-value="loggingContent || (loggingFiles.length ? '' : t('logging_empty'))" class="w-full"
          rows="14" readonly auto-resize
          style="font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace; white-space: pre; overflow: auto; max-height: 40vh;" />
      </div>
    </div>
    <template #footer>
      <Button :label="t('web.common.cancel')" icon="pi pi-times" @click="visible = false" text />
      <Button :label="t('web.common.save')" icon="pi pi-save" @click="onSave" autofocus :loading="isSaving" />
    </template>
  </Dialog>
</template>
