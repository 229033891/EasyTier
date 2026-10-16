<script setup lang="ts">
import { Button, InputText } from 'primevue'
import { useI18n } from 'vue-i18n'
import { isValidUpstreamDnsEntry } from '../../modules/dnsHostValidate'

const servers = defineModel<string[]>('servers', { required: true })

const { t } = useI18n()

/**
 * O2 console feedback: blank draft rows are stripped on serialize, so only
 * non-blank entries that `parse_upstream_nameserver` would reject get a hint.
 */
function upstreamInvalid(value: string | undefined): boolean {
  const text = (value ?? '').trim()
  return text.length > 0 && !isValidUpstreamDnsEntry(text)
}

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

/** Replace array so defineModel emits even under one-way parent bindings. */
function touchServers(next: string[]) {
  servers.value = next
  ensureKeys()
}

function addServer() {
  touchServers([...servers.value, ''])
}

function removeServer(index: number) {
  const next = [...servers.value]
  next.splice(index, 1)
  rowKeys.splice(index, 1)
  touchServers(next)
}
</script>

<template>
  <div class="dns-upstream-editor flex flex-col gap-y-2">
    <div
      v-for="(_, index) in servers"
      :key="rowKey(index)"
      class="flex flex-col gap-1"
    >
      <div class="config-inline-field">
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
      <p
        v-if="upstreamInvalid(servers[index])"
        class="dns-skip-hint"
      >
        <i class="pi pi-exclamation-triangle" aria-hidden="true" />{{
          t('dns.upstream.invalid_entry')
        }}
      </p>
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

/* O2: runtime will skip this entry — warn inline instead of tracing-only. */
.dns-skip-hint {
  display: flex;
  align-items: flex-start;
  gap: 0.375rem;
  margin: 0;
  font-size: 0.8125rem;
  line-height: 1.4;
  color: var(--et-warning, #b45309);
}

.dns-skip-hint .pi {
  margin-top: 0.125rem;
  flex: none;
}
</style>
