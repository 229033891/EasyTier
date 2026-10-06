<script setup lang="ts">
import { Button, InputText } from 'primevue'
import { useI18n } from 'vue-i18n'

const servers = defineModel<string[]>('servers', { required: true })

const { t } = useI18n()

/** Stable per-row keys so editing mid-list does not remount/focus-jump. */
const rowKeys: string[] = []
let rowKeySeq = 0

function ensureKeys() {
  while (rowKeys.length < servers.value.length)
    rowKeys.push(`dns-up-${++rowKeySeq}`)
  if (rowKeys.length > servers.value.length)
    rowKeys.length = servers.value.length
}

function rowKey(index: number): string {
  ensureKeys()
  return rowKeys[index]!
}

function addServer() {
  servers.value.push('')
  ensureKeys()
}

function removeServer(index: number) {
  servers.value.splice(index, 1)
  rowKeys.splice(index, 1)
}
</script>

<template>
  <div class="dns-upstream-editor flex flex-col gap-y-2">
    <div
      v-for="(_, index) in servers"
      :key="rowKey(index)"
      class="config-inline-field"
    >
      <label :for="`dns_upstream_${index}`" class="config-inline-label config-inline-label--sm">
        {{ t('dns.upstream.server') }}
      </label>
      <div class="config-inline-control dns-row-with-action">
        <InputText
          :id="`dns_upstream_${index}`"
          v-model="servers[index]"
          fluid
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          spellcheck="false"
          :placeholder="t('dns.upstream.server_placeholder')"
        />
        <Button
          icon="pi pi-trash"
          severity="danger"
          text
          rounded
          class="et-icon-action-btn"
          :aria-label="t('dns.upstream.remove')"
          v-tooltip.top="t('dns.upstream.remove')"
          @click="removeServer(index)"
        />
      </div>
    </div>

    <div class="flex justify-start">
      <Button
        class="et-panel-action-btn"
        icon="pi pi-plus"
        :label="t('dns.upstream.add')"
        severity="success"
        v-tooltip.top="t('dns.upstream.add_tip')"
        @click="addServer"
      />
    </div>
  </div>
</template>

<style scoped>
.dns-row-with-action {
  display: flex;
  align-items: center;
  gap: 0.25rem;
  min-width: 0;
}

.dns-row-with-action :deep(.p-inputtext),
.dns-row-with-action :deep(.p-inputwrapper) {
  flex: 1 1 auto;
  min-width: 0;
  width: auto;
  max-width: 100%;
}
</style>
