<script setup lang="ts">
import { AutoComplete, Button } from 'primevue'
import { useI18n } from 'vue-i18n'
import type { DnsForwarder } from '../../types/network'

const forwarders = defineModel('forwarders', {
  type: Array as () => DnsForwarder[],
  required: true,
})

const { t } = useI18n()

function addForwarder() {
  forwarders.value.push({ domains: [], servers: [] })
}

function removeForwarder(index: number) {
  forwarders.value.splice(index, 1)
}
</script>

<template>
  <div class="dns-forwarders-editor flex flex-col gap-y-2">
    <p class="dns-forwarders-editor__help et-meta m-0">{{ t('dns.forwarders.editor_help') }}</p>

    <div v-if="!forwarders.length" class="et-meta py-2">
      {{ t('dns.forwarders.empty') }}
    </div>

    <div
      v-for="(row, index) in forwarders"
      :key="index"
      class="dns-forwarder-row form-row flex flex-col gap-2 p-2 border border-surface rounded-lg"
    >
      <div class="config-inline-field">
        <div class="config-inline-label">
          <label :for="`dns_fwd_domains_${index}`">{{ t('dns.forwarders.domains') }}</label>
        </div>
        <div class="config-inline-control">
          <AutoComplete
            :id="`dns_fwd_domains_${index}`"
            v-model="row.domains"
            :placeholder="t('dns.forwarders.domains_placeholder')"
            multiple
            fluid
            :typeahead="false"
          />
        </div>
      </div>

      <div class="config-inline-field">
        <div class="config-inline-label">
          <label :for="`dns_fwd_servers_${index}`">{{ t('dns.forwarders.servers') }}</label>
        </div>
        <div class="config-inline-control">
          <AutoComplete
            :id="`dns_fwd_servers_${index}`"
            v-model="row.servers"
            :placeholder="t('dns.forwarders.servers_placeholder')"
            multiple
            fluid
            :typeahead="false"
          />
        </div>
      </div>

      <div class="flex justify-end">
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

    <div class="flex justify-start mt-2">
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
.dns-forwarders-editor__help {
  color: var(--text-color-secondary, #64748b);
}
</style>
