<script setup lang="ts">
import { AutoComplete, Button, InputNumber, InputText } from 'primevue'
import { useI18n } from 'vue-i18n'
import {
  addDnsHostRow,
  DEFAULT_DNS_HOST_TTL_SECS,
  removeDnsHostRow,
  type DnsHostEntry,
} from '../../types/network'

const hosts = defineModel('hosts', {
  type: Array as () => DnsHostEntry[],
  required: true,
})

const { t } = useI18n()

/** Stable per-row keys (object identity); avoids focus jump on mid-list delete. */
const rowKeyMap = new WeakMap<object, string>()
let rowKeySeq = 0

function rowKey(row: DnsHostEntry): string {
  let key = rowKeyMap.get(row)
  if (!key) {
    rowKeySeq += 1
    key = `dns-host-${rowKeySeq}`
    rowKeyMap.set(row, key)
  }
  return key
}

function addHost() {
  addDnsHostRow(hosts.value)
}

function removeHost(index: number) {
  removeDnsHostRow(index, hosts.value)
}
</script>

<template>
  <div class="dns-hosts-editor flex flex-col gap-y-2">
    <div
      v-for="(row, index) in hosts"
      :key="rowKey(row)"
      class="dns-host-row flex flex-col gap-1.5"
    >
      <div class="config-inline-field">
        <label :for="`dns_host_name_${index}`" class="config-inline-label config-inline-label--sm">
          {{ t('dns.hosts.name') }}
        </label>
        <div class="config-inline-control dns-row-with-action">
          <InputText
            :id="`dns_host_name_${index}`"
            v-model="row.name"
            fluid
            :placeholder="t('dns.hosts.name_placeholder')"
          />
          <Button
            icon="pi pi-trash"
            severity="danger"
            text
            rounded
            class="et-icon-action-btn"
            :aria-label="t('dns.hosts.remove')"
            v-tooltip.top="t('dns.hosts.remove')"
            @click="removeHost(index)"
          />
        </div>
      </div>

      <div class="config-inline-field">
        <label :for="`dns_host_ips_${index}`" class="config-inline-label config-inline-label--sm">
          {{ t('dns.hosts.ips') }}
        </label>
        <div class="config-inline-control">
          <AutoComplete
            :id="`dns_host_ips_${index}`"
            v-model="row.ips"
            :placeholder="t('dns.hosts.ips_placeholder')"
            multiple
            fluid
            :typeahead="false"
          />
        </div>
      </div>

      <div class="config-inline-field">
        <label :for="`dns_host_ttl_${index}`" class="config-inline-label config-inline-label--sm">
          {{ t('dns.hosts.ttl') }}
        </label>
        <div class="config-inline-control dns-host-row__ttl">
          <InputNumber
            :id="`dns_host_ttl_${index}`"
            v-model="row.ttl_secs"
            :min="1"
            :max="86400"
            :placeholder="String(DEFAULT_DNS_HOST_TTL_SECS)"
            :allow-empty="true"
            fluid
          />
          <span class="et-meta">{{ t('dns.hosts.ttl_unit') }}</span>
        </div>
      </div>
    </div>

    <div class="flex justify-start">
      <Button
        class="et-panel-action-btn"
        icon="pi pi-plus"
        :label="t('dns.hosts.add')"
        severity="success"
        v-tooltip.top="t('dns.hosts.add_tip')"
        @click="addHost"
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
.dns-row-with-action :deep(.p-autocomplete),
.dns-row-with-action :deep(.p-inputwrapper) {
  flex: 1 1 auto;
  min-width: 0;
  width: auto;
  max-width: 100%;
}

.dns-host-row__ttl {
  display: flex;
  align-items: center;
  gap: 0.5rem;
}

.dns-host-row__ttl :deep(.p-inputnumber) {
  max-width: 8rem;
}
</style>
