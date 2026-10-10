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
import { normalizeLoggerLevel, takeLastLogLines, type LoggingSettingsApi, type LoggerLevelState, type ClearLogFilesResult } from '../modules/logging'
import { localizeLogTimestamps, formatApiErrorDetail } from '../modules/utils'

const props = defineProps<{
  api: LoggingSettingsApi
}>()

const visible = defineModel<boolean>('visible', { default: false })

const { t } = useI18n()
const toast = useToast()

const loggingLevel = ref('warn')
const loggingLevelLive = ref(true)
const loggingPath = ref('')
const loggingFiles = ref<Array<{ label: string, value: string }>>([])
const selectedLogFile = ref('')
const loggingContent = ref('')
const isSaving = ref(false)
const isLoading = ref(false)
const isClearing = ref(false)
const isCopying = ref(false)

/** Trailing lines copied by the footer "Copy Logs" action. */
const RECENT_LOG_LINES = 100

const loggingLevelOptions = computed(() =>
  ['off', 'error', 'warn', 'info', 'debug', 'trace'].map(level => ({
    label: t(`logging_level_${level}`),
    value: level,
  })),
)

const canShowLogContent = computed(() =>
  !props.api.remoteOnly && !!props.api.listLogFiles && !!props.api.readLogFile,
)

const canClearLogs = computed(() =>
  canShowLogContent.value && typeof props.api.clearLogFiles === 'function',
)

const hasLogFiles = computed(() => loggingFiles.value.length > 0)

const canCopyRecentLogs = computed(() => canShowLogContent.value && hasLogFiles.value)

function applyLoggerLevelResult(result: string | LoggerLevelState) {
  if (typeof result === 'string') {
    loggingLevel.value = normalizeLoggerLevel(result)
    loggingLevelLive.value = true
    return
  }
  loggingLevel.value = normalizeLoggerLevel(result.level)
  loggingLevelLive.value = result.live
}

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
    // Files stay UTC on disk; localize leading …Z stamps for the viewer only.
    const raw = preferred ? await props.api.readLogFile!(preferred) : ''
    loggingContent.value = localizeLogTimestamps(raw)
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
  loggingLevelLive.value = true
  try {
    if (props.api.getLoggerLevel)
      applyLoggerLevelResult(await props.api.getLoggerLevel())
  }
  catch (e) {
    loggingLevelLive.value = false
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

async function writeClipboard(text: string) {
  if (props.api.copyText) {
    await props.api.copyText(text)
    return
  }
  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text)
    return
  }
  throw new Error('clipboard unavailable')
}

async function onCopyRecentLogs() {
  if (!canCopyRecentLogs.value || isCopying.value || isClearing.value || isSaving.value)
    return
  isCopying.value = true
  try {
    // Refresh from disk so the clipboard gets the latest lines, not a stale viewer buffer.
    if (selectedLogFile.value && props.api.readLogFile) {
      const raw = await props.api.readLogFile(selectedLogFile.value)
      loggingContent.value = localizeLogTimestamps(raw)
    }
    const text = takeLastLogLines(loggingContent.value, RECENT_LOG_LINES)
    if (!text) {
      toast.add({
        severity: 'info',
        summary: t('logging_copy_recent'),
        detail: t('logging_copy_recent_empty'),
        life: TOAST_LIFE.info,
      })
      return
    }
    await writeClipboard(text)
    const count = text.split('\n').length
    toast.add({
      severity: 'success',
      summary: t('web.common.success'),
      detail: t('logging_copy_recent_success', { count }),
      life: TOAST_LIFE.success,
    })
  }
  catch (e) {
    console.error('Failed to copy recent logs', e)
    toast.add({
      severity: 'error',
      summary: t('error'),
      detail: `${t('logging_copy_recent_failed')}: ${formatApiErrorDetail(e, t)}`,
      life: TOAST_LIFE.severe,
    })
  }
  finally {
    isCopying.value = false
  }
}

async function onClearLogs() {
  if (!canClearLogs.value || isClearing.value || isSaving.value) {
    return
  }
  isClearing.value = true
  try {
        const raw = await props.api.clearLogFiles!()
        const result: ClearLogFilesResult = typeof raw === 'number'
          ? { cleared: raw, errors: [], dirs: [] }
          : raw
        selectedLogFile.value = ''
        await loadLoggingContent()
        if (result.errors.length > 0) {
          const errorDetail = result.errors.join('; ')
          if (result.cleared > 0) {
            toast.add({
              severity: 'warn',
              summary: t('logging_clear'),
              detail: t('logging_clear_partial', {
                count: result.cleared,
                error: errorDetail,
              }),
              life: TOAST_LIFE.severe,
            })
          }
          else {
            toast.add({
              severity: 'error',
              summary: t('error'),
              detail: `${t('logging_clear_failed')}: ${errorDetail}`,
              life: TOAST_LIFE.severe,
            })
          }
        }
        else {
          toast.add({
            severity: result.cleared > 0 ? 'success' : 'info',
            summary: result.cleared > 0 ? t('web.common.success') : t('logging_clear'),
            detail: result.cleared > 0
              ? t('logging_clear_success', { count: result.cleared })
              : t('logging_clear_empty'),
            life: TOAST_LIFE.success,
          })
        }
      }
      catch (e) {
        console.error('Failed to clear logs', e)
        toast.add({
          severity: 'error',
          summary: t('error'),
          detail: `${t('logging_clear_failed')}: ${formatApiErrorDetail(e, t)}`,
          life: TOAST_LIFE.severe,
        })
      }
      finally {
        isClearing.value = false
      }
}
</script>

<template>
  <Dialog v-model:visible="visible" modal :header="t('logging')"
    :style="{ width: 'min(936px, calc(100vw - 1.5rem))' }" class="app-dialog">
    <div class="logging-dialog flex flex-col gap-3">
      <div class="logging-inline-field">
        <div class="logging-inline-label">
          <label for="logging-level">{{ t('logging_level') }}</label>
          <i
            class="pi pi-question-circle config-help-tip" tabindex="0"
            v-tooltip.top="t('logging_retention_hint')"
            :aria-label="t('logging_retention_hint')"
            role="img"
          />
          <i
            v-if="api.remoteOnly"
            class="pi pi-question-circle config-help-tip" tabindex="0"
            v-tooltip.top="t('logging_remote_hint')"
            :aria-label="t('logging_remote_hint')"
            role="img"
          />
        </div>
        <Select id="logging-level" v-model="loggingLevel" :options="loggingLevelOptions" option-label="label"
          option-value="value" class="logging-inline-control" :disabled="!loggingLevelLive && !!api.remoteOnly" />
      </div>
      <p v-if="!loggingLevelLive" class="logging-level-hint m-0">{{ t('logging_level_not_live') }}</p>

      <div v-if="api.getLogDir" class="logging-inline-field">
        <label class="logging-inline-label">{{ t('logging_path') }}</label>
        <div class="logging-path-row">
          <InputText :model-value="loggingPath" class="logging-inline-control" readonly />
          <Button
            v-if="api.copyLogDir"
            size="small"
            severity="secondary"
            text
            rounded
            icon="pi pi-copy"
            class="et-icon-action-btn"
            :aria-label="t('logging_copy_dir')"
            v-tooltip.top="t('logging_copy_dir')"
            @click="copyLoggingDir"
          />
          <Button
            v-if="api.canOpenLogDir && api.openLogDir"
            size="small"
            severity="secondary"
            text
            rounded
            icon="pi pi-folder-open"
            class="et-icon-action-btn"
            :aria-label="t('logging_open_dir')"
            v-tooltip.top="t('logging_open_dir')"
            @click="openLoggingDir"
          />
        </div>
      </div>

      <div v-if="canShowLogContent" class="logging-content flex flex-col gap-2">
        <div class="logging-inline-field">
          <div class="logging-inline-label">
            <label for="logging-file">{{ t('logging_content') }}</label>
            <Button
              size="small"
              severity="secondary"
              text
              rounded
              icon="pi pi-refresh"
              class="et-icon-action-btn"
              :loading="isLoading"
              :aria-label="t('logging_refresh')"
              v-tooltip.top="t('logging_refresh')"
              @click="loadLoggingContent()"
            />
          </div>
          <Select id="logging-file" v-model="selectedLogFile" :options="loggingFiles" option-label="label"
            option-value="value" class="logging-inline-control" :placeholder="t('logging_file')"
            :disabled="!loggingFiles.length"
            @update:model-value="(value: string) => loadLoggingContent(value)" />
        </div>
        <Textarea
          :model-value="loggingContent || (loggingFiles.length ? '' : t('logging_empty'))"
          class="w-full logging-textarea"
          rows="21"
          readonly
        />
      </div>
    </div>
    <template #footer>
      <div class="logging-footer">
        <div class="logging-footer-left">
          <Button
            v-if="canShowLogContent"
            :label="t('logging_copy_recent')"
            severity="secondary"
            text
            :loading="isCopying"
            :disabled="!canCopyRecentLogs || isSaving || isLoading || isClearing"
            @click="onCopyRecentLogs"
          />
          <Button
            v-if="canClearLogs"
            :label="t('logging_clear')"
            severity="danger"
            text
            :loading="isClearing"
            :disabled="!hasLogFiles || isSaving || isLoading || isCopying"
            @click="onClearLogs"
          />
        </div>
        <div class="logging-footer-actions">
          <Button :label="t('web.common.cancel')" @click="visible = false" text
            :disabled="isClearing || isCopying" />
          <Button :label="t('web.common.save')" @click="onSave" autofocus :loading="isSaving"
            :disabled="isClearing || isCopying || (!loggingLevelLive && !!api.remoteOnly)" />
        </div>
      </div>
    </template>
  </Dialog>
</template>

<style scoped>
.logging-inline-field {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  min-width: 0;
}

.logging-inline-label {
  display: inline-flex;
  align-items: center;
  gap: 0.35rem;
  flex: 0 0 auto;
  white-space: nowrap;
}

.logging-inline-control {
  flex: 1 1 auto;
  min-width: 0;
  width: 100%;
}

.logging-path-row {
  display: flex;
  align-items: center;
  gap: 0.25rem;
  flex: 1 1 auto;
  min-width: 0;
}

.logging-level-hint {
  font-size: 0.8rem;
  line-height: 1.35;
  opacity: 0.75;
}

.logging-textarea {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 0.78rem;
  line-height: 1.25;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  word-break: break-word;
  overflow-x: hidden;
  overflow-y: auto;
  max-height: min(71.5vh, 36.4rem);
  resize: none;
}

.logging-textarea :deep(textarea) {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  word-break: break-word;
  overflow-x: hidden;
  line-height: 1.25;
}

.logging-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.75rem;
  width: 100%;
}

.logging-footer-left,
.logging-footer-actions {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  flex: 1 1 0;
  min-width: 0;
}

/* 复制/删除是 v-if 的，全藏时塌掉左区，否则左半行空白 */
.logging-footer-left:empty {
  display: none;
}

/* 四钮平分整行；可见按钮数 2~4 动态变化，剩余自动均分 */
.logging-footer-left > :deep(.p-button),
.logging-footer-actions > :deep(.p-button) {
  flex: 1 1 0;
  min-width: 0;
}

.logging-footer-actions {
  margin-left: auto;
}
</style>
