<script setup lang="ts">
import { computed } from 'vue'
import { AutoComplete, Button, Checkbox, InputNumber, InputText, Select } from 'primevue'
import { useI18n } from 'vue-i18n'
import { onChipsFocusOut, onChipsKeydown } from '../../modules/chipsInput'
import {
  addPolicyRouteRow,
  removePolicyRouteRow,
  type PolicyRouteConfig,
} from '../../types/network'

const routes = defineModel('routes', {
  type: Array as () => PolicyRouteConfig[],
  required: true,
})

const { t } = useI18n()

/** Stable per-row keys (object identity); avoids focus jump on mid-list delete. */
const rowKeyMap = new WeakMap<object, string>()
let rowKeySeq = 0

function rowKey(row: PolicyRouteConfig): string {
  let key = rowKeyMap.get(row)
  if (!key) {
    rowKeySeq += 1
    key = `policy-route-${rowKeySeq}`
    rowKeyMap.set(row, key)
  }
  return key
}

/** Replace array so defineModel emits even under one-way parent bindings. */
function touchRoutes(next: PolicyRouteConfig[]) {
  routes.value = next
}

function addRoute() {
  const next = [...routes.value]
  addPolicyRouteRow(next)
  touchRoutes(next)
}

function removeRoute(index: number) {
  const next = [...routes.value]
  removePolicyRouteRow(index, next)
  touchRoutes(next)
}

const selectByOptions = computed(() => [
  { value: 'priority', label: t('policy_routes_select_by_priority') },
  { value: 'quality', label: t('policy_routes_select_by_quality') },
  { value: 'balance', label: t('policy_routes_select_by_balance') },
])

const failoverOptions = computed(() => [
  { value: 'unreachable', label: t('policy_routes_failover_unreachable') },
  { value: 'degraded', label: t('policy_routes_failover_degraded') },
  { value: 'never', label: t('policy_routes_failover_never') },
])

const allUnavailableOptions = computed(() => [
  { value: 'open', label: t('policy_routes_all_unavailable_open') },
  { value: 'closed', label: t('policy_routes_all_unavailable_closed') },
])
</script>

<template>
  <div class="policy-routes-editor flex flex-col gap-y-3">
    <div
      v-for="(row, index) in routes"
      :key="rowKey(row)"
      class="policy-route-row flex flex-col gap-1.5"
    >
      <div class="config-inline-field">
        <label :for="`policy_route_cidr_${index}`" class="config-inline-label config-inline-label--sm">
          {{ t('policy_routes_cidr') }}
        </label>
        <div class="config-inline-control policy-route-row__with-action">
          <InputText
            :id="`policy_route_cidr_${index}`"
            v-model="row.cidr"
            fluid
            :placeholder="t('policy_routes_cidr_placeholder')"
          />
          <Button
            icon="pi pi-trash"
            severity="danger"
            text
            rounded
            class="et-icon-action-btn"
            :aria-label="t('policy_routes_remove')"
            v-tooltip.top="t('policy_routes_remove')"
            @click="removeRoute(index)"
          />
        </div>
      </div>

      <div class="config-inline-field">
        <label :for="`policy_route_domains_${index}`" class="config-inline-label config-inline-label--sm">
          {{ t('policy_routes_domains') }}
        </label>
        <div class="config-inline-control">
          <AutoComplete
            :id="`policy_route_domains_${index}`"
            v-model="row.domains"
            :placeholder="t('policy_routes_domains_placeholder')"
            multiple
            fluid
            :typeahead="false"
            @keydown.capture="onChipsKeydown($event, row.domains ??= [])"
            @focusout.capture="onChipsFocusOut($event, row.domains ??= [])"
          />
        </div>
      </div>

      <div class="config-inline-field">
        <label :for="`policy_route_peers_${index}`" class="config-inline-label config-inline-label--sm">
          {{ t('policy_routes_peers') }}
        </label>
        <div class="config-inline-control">
          <AutoComplete
            :id="`policy_route_peers_${index}`"
            v-model="row.peers"
            :placeholder="t('policy_routes_peers_placeholder')"
            multiple
            fluid
            :typeahead="false"
            @keydown.capture="onChipsKeydown($event, row.peers ??= [])"
            @focusout.capture="onChipsFocusOut($event, row.peers ??= [])"
          />
        </div>
      </div>

      <div class="policy-route-row__grid">
        <div class="config-inline-field">
          <label :for="`policy_route_select_by_${index}`" class="config-inline-label config-inline-label--sm">
            {{ t('policy_routes_select_by') }}
          </label>
          <div class="config-inline-control">
            <Select
              :input-id="`policy_route_select_by_${index}`"
              v-model="row.select_by"
              :options="selectByOptions"
              option-label="label"
              option-value="value"
              fluid
            />
          </div>
        </div>

        <div class="config-inline-field">
          <label :for="`policy_route_failover_${index}`" class="config-inline-label config-inline-label--sm">
            {{ t('policy_routes_failover_when') }}
          </label>
          <div class="config-inline-control">
            <Select
              :input-id="`policy_route_failover_${index}`"
              v-model="row.failover_when"
              :options="failoverOptions"
              option-label="label"
              option-value="value"
              fluid
            />
          </div>
        </div>
      </div>

      <div class="policy-route-row__grid">
        <div class="config-inline-field">
          <label :for="`policy_route_all_unavailable_${index}`" class="config-inline-label config-inline-label--sm">
            {{ t('policy_routes_if_all_unavailable') }}
          </label>
          <div class="config-inline-control">
            <Select
              :input-id="`policy_route_all_unavailable_${index}`"
              v-model="row.if_all_unavailable"
              :options="allUnavailableOptions"
              option-label="label"
              option-value="value"
              fluid
            />
          </div>
        </div>

        <div v-if="row.failover_when === 'degraded'" class="config-inline-field">
          <label :for="`policy_route_degraded_cost_${index}`" class="config-inline-label config-inline-label--sm">
            {{ t('policy_routes_quality_degraded_cost') }}
          </label>
          <div class="config-inline-control">
            <InputNumber
              :input-id="`policy_route_degraded_cost_${index}`"
              v-model="row.quality_degraded_cost"
              :min="1"
              :allow-empty="true"
              fluid
            />
          </div>
        </div>
      </div>

      <div
        v-if="row.if_all_unavailable === 'closed'"
        class="policy-route-row__confirm flex items-center gap-2"
      >
        <Checkbox
          :input-id="`policy_route_require_confirmed_${index}`"
          v-model="row.require_confirmed"
          binary
        />
        <label :for="`policy_route_require_confirmed_${index}`" class="et-meta">
          {{ t('policy_routes_require_confirmed') }}
        </label>
      </div>
    </div>

    <div class="flex justify-start">
      <Button
        class="et-panel-action-btn"
        icon="pi pi-plus"
        :label="t('policy_routes_add')"
        severity="success"
        @click="addRoute"
      />
    </div>
  </div>
</template>

<style scoped>
.policy-route-row {
  padding: 0.5rem 0.625rem;
  border: 1px solid var(--p-content-border-color);
  border-radius: 6px;
}

.policy-route-row__with-action {
  display: flex;
  align-items: center;
  gap: 0.25rem;
  min-width: 0;
}

.policy-route-row__with-action :deep(.p-inputtext),
.policy-route-row__with-action :deep(.p-autocomplete),
.policy-route-row__with-action :deep(.p-inputwrapper) {
  flex: 1 1 auto;
  min-width: 0;
  width: auto;
  max-width: 100%;
}

.policy-route-row__grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(12rem, 1fr));
  gap: 0.5rem;
}

.policy-route-row__confirm {
  min-height: 1.75rem;
}
</style>
