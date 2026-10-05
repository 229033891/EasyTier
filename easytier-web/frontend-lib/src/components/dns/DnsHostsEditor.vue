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

function addHost() {
  addDnsHostRow(hosts.value)
}

function removeHost(index: number) {
  removeDnsHostRow(index, hosts.value)
}
</script>

<template>
  <div class="dns-hosts-editor flex flex-col gap-y-2">
    <div v-if="!hosts.length" class="et-meta py-2">
      {{ t('dns.hosts.empty') }}
    </div>

    <div
      v-for="(row, index) in hosts"
      :key="index"
      class="dns-host-row form-row flex flex-col gap-2 p-2 border border-surface rounded-lg"
    >
      <div class="config-inline-field">
        <div class="config-inline-label">
          <label :for="`dns_host_name_${index}`">{{ t('dns.hosts.name') }}</label>
        </div>
        <div class="config-inline-control">
          <InputText
            :id="`dns_host_name_${index}`"
            v-model="row.name"
            fluid
            :placeholder="t('dns.hosts.name_placeholder')"
          />
        </div>
      </div>

      <div class="config-inline-field">
        <div class="config-inline-label">
          <label :for="`dns_host_ips_${index}`">{{ t('dns.hosts.ips') }}</label>
        </div>
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
        <div class="config-inline-label">
          <label :for="`dns_host_ttl_${index}`">{{ t('dns.hosts.ttl') }}</label>
        </div>
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

      <div class="flex justify-end">
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

    <div class="flex justify-start mt-2">
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
.dns-host-row__ttl {
  display: flex;
  align-items: center;
  gap: 0.5rem;
}

.dns-host-row__ttl :deep(.p-inputnumber) {
  max-width: 8rem;
}
</style>
