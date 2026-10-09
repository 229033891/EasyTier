<script setup lang="ts">
import { AutoComplete, Button } from 'primevue'
import { useI18n } from 'vue-i18n'
import { onChipsFocusOut, onChipsKeydown } from '../../modules/chipsInput'
import type { DnsForwarder } from '../../types/network'

const forwarders = defineModel('forwarders', {
  type: Array as () => DnsForwarder[],
  required: true,
})

const { t } = useI18n()

/** Stable per-row keys (object identity); avoids focus jump on mid-list delete. */
const rowKeyMap = new WeakMap<object, string>()
let rowKeySeq = 0

function rowKey(row: DnsForwarder): string {
  let key = rowKeyMap.get(row)
  if (!key) {
    rowKeySeq += 1
    key = `dns-fwd-${rowKeySeq}`
    rowKeyMap.set(row, key)
  }
  return key
}

function touchForwarders(next: DnsForwarder[]) {
  forwarders.value = next
}

function addForwarder() {
  const next = [...forwarders.value, { domains: [], servers: [] }]
  touchForwarders(next)
}

function removeForwarder(index: number) {
  const next = [...forwarders.value]
  next.splice(index, 1)
  touchForwarders(next)
}
</script>

<template>
  <div class="dns-forwarders-editor flex flex-col gap-y-2">
    <div
      v-for="(row, index) in forwarders"
      :key="rowKey(row)"
      class="dns-forwarder-row flex flex-col gap-1.5"
    >
      <div class="config-inline-field">
        <label :for="`dns_fwd_domains_${index}`" class="config-inline-label config-inline-label--sm">
          {{ t('dns.forwarders.domains') }}
        </label>
        <div class="config-inline-control dns-row-with-action">
          <AutoComplete
            :id="`dns_fwd_domains_${index}`"
            v-model="row.domains"
            :placeholder="t('chips_placeholder', ['corp.example.'])"
            multiple
            fluid
            :typeahead="false"
            @keydown.capture="onChipsKeydown($event, row.domains ??= [])"
            @focusout.capture="onChipsFocusOut($event, row.domains ??= [])"
          />
          <Button
            icon="pi pi-trash"
            severity="danger"
            text
            rounded
            class="et-icon-action-btn"
            :aria-label="t('dns.forwarders.remove')"
            v-tooltip.top="t('dns.forwarders.remove')"
            @click="removeForwarder(index)"
          />
        </div>
      </div>

      <div class="config-inline-field">
        <label :for="`dns_fwd_servers_${index}`" class="config-inline-label config-inline-label--sm">
          {{ t('dns.forwarders.servers') }}
        </label>
        <div class="config-inline-control">
          <AutoComplete
            :id="`dns_fwd_servers_${index}`"
            v-model="row.servers"
            :placeholder="t('chips_placeholder', ['10.0.0.53'])"
            multiple
            fluid
            :typeahead="false"
            @keydown.capture="onChipsKeydown($event, row.servers ??= [])"
            @focusout.capture="onChipsFocusOut($event, row.servers ??= [])"
          />
        </div>
      </div>
    </div>

    <div class="flex justify-start">
      <Button
        class="et-panel-action-btn"
        icon="pi pi-plus"
        :label="t('dns.forwarders.add')"
        severity="success"
        v-tooltip.top="t('dns.forwarders.add_tip')"
        @click="addForwarder"
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

.dns-row-with-action :deep(.p-autocomplete),
.dns-row-with-action :deep(.p-inputwrapper) {
  flex: 1 1 auto;
  min-width: 0;
  width: auto;
  max-width: 100%;
}
</style>
