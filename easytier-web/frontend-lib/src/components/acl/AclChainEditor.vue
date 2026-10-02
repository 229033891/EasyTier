<script setup lang="ts">
import { Button, Column, DataTable, InputText, Select, SelectButton, Tag, ToggleButton } from 'primevue'
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { AclAction, AclChain, AclChainType, AclProtocol, AclRule, ensureAclChain, ensureAclRuleLists } from '../../types/network'
import AclRuleDialog from './AclRuleDialog.vue'

const props = defineProps<{
  groupNames?: string[]
}>()

const chain = defineModel<AclChain>({ required: true })

const { t } = useI18n()

function rules() {
  return ensureAclChain(chain.value).rules
}

watch(() => rules(), (newRules) => {
  if (!newRules) return
  const isSorted = newRules.every((rule, i) => i === 0 || (rule.priority || 0) <= (newRules[i - 1].priority || 0))
  if (!isSorted) {
    chain.value.rules.sort((a, b) => (b.priority || 0) - (a.priority || 0))
  }
}, { deep: true, immediate: true })

const actionOptions = [
  { label: () => t('acl.allow'), value: AclAction.Allow },
  { label: () => t('acl.drop'), value: AclAction.Drop },
]

const chainTypeOptions = [
  { label: () => t('acl.inbound'), value: AclChainType.Inbound },
  { label: () => t('acl.outbound'), value: AclChainType.Outbound },
  { label: () => t('acl.forward'), value: AclChainType.Forward },
]

const editingRule = ref<AclRule | null>(null)
const editingRuleIndex = ref(-1)
const showRuleDialog = ref(false)

function getProtocolLabel(proto: AclProtocol) {
  switch (proto) {
    case AclProtocol.Any: return t('acl.any')
    case AclProtocol.TCP: return 'TCP'
    case AclProtocol.UDP: return 'UDP'
    case AclProtocol.ICMP: return 'ICMP'
    case AclProtocol.ICMPv6: return 'ICMPv6'
    default: return t('event.Unknown')
  }
}

function getActionLabel(action: AclAction) {
  switch (action) {
    case AclAction.Allow: return t('acl.allow')
    case AclAction.Drop: return t('acl.drop')
    default: return t('event.Unknown')
  }
}

function addRule() {
  editingRuleIndex.value = -1
  editingRule.value = {
    name: '',
    description: '',
    priority: rules().length,
    enabled: true,
    protocol: AclProtocol.Any,
    ports: [],
    source_ips: [],
    destination_ips: [],
    source_ports: [],
    action: AclAction.Allow,
    rate_limit: 0,
    burst_limit: 0,
    stateful: false,
    source_groups: [],
    destination_groups: [],
  }
  showRuleDialog.value = true
}

function editRule(index: number) {
  editingRuleIndex.value = index
  editingRule.value = ensureAclRuleLists(JSON.parse(JSON.stringify(rules()[index])))
  showRuleDialog.value = true
}

function deleteRule(index: number) {
  rules().splice(index, 1)
}

function saveRule(rule: AclRule) {
  const chainRules = rules()
  ensureAclRuleLists(rule)
  if (editingRuleIndex.value === -1) {
    chainRules.push(rule)
  } else {
    chainRules[editingRuleIndex.value] = rule
  }
  chainRules.sort((a, b) => (b.priority || 0) - (a.priority || 0))
}

function onRowReorder(event: any) {
  chain.value.rules = event.value ?? []
  const chainRules = rules()
  chainRules.forEach((rule, index) => {
    rule.priority = chainRules.length - index - 1
  })
}
</script>

<template>
  <div class="acl-chain flex flex-col gap-3">
    <div class="acl-card grid grid-cols-1 md:grid-cols-2 gap-3">
      <div class="flex flex-col gap-1.5">
        <label class="acl-label">{{ t('acl.chain.name') }}</label>
        <InputText v-model="chain.name" size="small" />
      </div>
      <div class="flex flex-col gap-1.5">
        <label class="acl-label">{{ t('acl.rule.description') }}</label>
        <InputText v-model="chain.description" size="small" />
      </div>

      <div class="acl-card-footer col-span-full flex flex-wrap items-center gap-x-4 gap-y-2">
        <div class="flex items-center gap-2">
          <label class="acl-label">{{ t('acl.rule.enabled') }}</label>
          <ToggleButton v-model="chain.enabled" on-icon="pi pi-check" off-icon="pi pi-times"
            :on-label="t('web.common.enable')" :off-label="t('web.common.disable')" class="w-24" />
        </div>
        <div class="flex items-center gap-2">
          <label class="acl-label">{{ t('acl.chain.type') }}</label>
          <Select v-model="chain.chain_type" :options="chainTypeOptions" :option-label="opt => opt.label()"
            option-value="value" size="small" class="w-36" />
        </div>
        <div class="flex items-center gap-2 md:ml-auto">
          <label class="acl-label">{{ t('acl.default_action') }}</label>
          <SelectButton v-model="chain.default_action" :options="actionOptions" :option-label="opt => opt.label()"
            option-value="value" :allow-empty="false" />
        </div>
      </div>
    </div>

    <div class="flex flex-row items-center gap-3 justify-between">
      <h4 class="acl-section-title">{{ t('acl.rules') }}</h4>
      <Button icon="pi pi-plus" :label="t('acl.add_rule')" severity="success" size="small" @click="addRule" />
    </div>

    <DataTable :value="rules()" @row-reorder="onRowReorder" responsiveLayout="scroll" class="acl-rules-table">
      <Column rowReorder headerStyle="width: 3rem" />
      <Column field="enabled" :header="t('acl.rule.enabled')" headerStyle="width: 4.5rem">
        <template #body="{ data }">
          <i class="pi text-sm"
            :class="data.enabled ? 'pi-check-circle acl-ok' : 'pi-times-circle acl-off'"></i>
        </template>
      </Column>
      <Column field="name" :header="t('acl.rule.name')" />
      <Column :header="t('acl.match')">
        <template #body="{ data }">
          <div class="flex flex-col gap-1.5 py-0.5">
            <span class="acl-proto">{{ getProtocolLabel(data.protocol) }}</span>
            <div class="flex flex-col sm:flex-row sm:items-center gap-1 sm:gap-2">
              <div class="flex items-center gap-1.5 min-w-0">
                <span class="acl-endpoint">{{ t('acl.match_src') }}</span>
                <div class="flex flex-wrap gap-1 items-center">
                  <span v-for="ip in data.source_ips" :key="ip" class="acl-chip">{{ ip }}</span>
                  <span v-for="grp in data.source_groups" :key="grp" class="acl-group">@{{ grp }}</span>
                  <span v-if="data.source_ports.length" class="acl-ports">:{{ data.source_ports.join(',') }}</span>
                  <span v-if="!data.source_ips.length && !data.source_groups.length" class="acl-any">*</span>
                </div>
              </div>
              <i class="pi pi-arrow-right hidden sm:block acl-arrow"></i>
              <div class="flex items-center gap-1.5 min-w-0">
                <span class="acl-endpoint">{{ t('acl.match_dst') }}</span>
                <div class="flex flex-wrap gap-1 items-center">
                  <span v-for="ip in data.destination_ips" :key="ip" class="acl-chip">{{ ip }}</span>
                  <span v-for="grp in data.destination_groups" :key="grp" class="acl-group">@{{ grp }}</span>
                  <span v-if="data.ports.length" class="acl-ports">:{{ data.ports.join(',') }}</span>
                  <span v-if="!data.destination_ips.length && !data.destination_groups.length" class="acl-any">*</span>
                </div>
              </div>
            </div>
          </div>
        </template>
      </Column>
      <Column field="action" :header="t('acl.rule.action')" headerStyle="width: 5.5rem">
        <template #body="{ data }">
          <Tag :severity="data.action === AclAction.Allow ? 'success' : 'danger'" :value="getActionLabel(data.action)" />
        </template>
      </Column>
      <Column :header="t('web.common.edit')" headerStyle="width: 6rem">
        <template #body="{ index }">
          <div class="flex gap-1">
            <Button icon="pi pi-pencil" severity="secondary" rounded text class="et-icon-action-btn"
              @click="editRule(index)" />
            <Button icon="pi pi-trash" severity="danger" rounded text class="et-icon-action-btn"
              @click="deleteRule(index)" />
          </div>
        </template>
      </Column>
    </DataTable>

    <AclRuleDialog v-if="showRuleDialog && editingRule" v-model:visible="showRuleDialog" v-model:rule="editingRule"
      :group-names="props.groupNames" @save="saveRule" />
  </div>
</template>

<style scoped>
.acl-card {
  padding: 0.75rem 0.85rem;
  border: 1px solid var(--et-border-color, #e2e8f0);
  border-radius: var(--et-radius, 0.75rem);
  background: var(--surface-50, #f8fafc);
}

.acl-card-footer {
  border-top: 1px solid var(--et-border-color, #e2e8f0);
  padding-top: 0.65rem;
  margin-top: 0.15rem;
}

.acl-label {
  font-size: var(--et-fs-meta, 0.75rem);
  font-weight: 600;
  color: var(--text-color-secondary, #64748b);
}

.acl-section-title {
  margin: 0;
  font-size: var(--et-fs-section, 1rem);
  font-weight: 600;
  color: var(--text-color, #1e293b);
}

.acl-proto {
  display: inline-flex;
  align-self: flex-start;
  padding: 0.1rem 0.4rem;
  border-radius: 0.375rem;
  font-size: var(--et-fs-meta, 0.75rem);
  font-weight: 600;
  letter-spacing: 0.02em;
  text-transform: uppercase;
  color: var(--primary-color, var(--et-primary, #0ea5e9));
  background: color-mix(in srgb, var(--primary-color, #0ea5e9) 12%, transparent);
}

.acl-endpoint {
  flex: 0 0 auto;
  width: 1.75rem;
  font-size: var(--et-fs-meta, 0.75rem);
  font-weight: 600;
  color: var(--text-color-secondary, #64748b);
}

.acl-chip {
  display: inline-block;
  padding: 0.05rem 0.35rem;
  border-radius: 0.3rem;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: var(--et-fs-meta, 0.75rem);
  color: var(--text-color, #1e293b);
  background: var(--surface-100, #f1f5f9);
  border: 1px solid var(--et-border-color, #e2e8f0);
}

.acl-group {
  font-size: var(--et-fs-meta, 0.75rem);
  font-weight: 600;
  color: var(--primary-color, var(--et-primary, #0ea5e9));
}

.acl-ports {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: var(--et-fs-meta, 0.75rem);
  color: var(--primary-color, var(--et-primary, #0ea5e9));
}

.acl-any,
.acl-arrow {
  font-size: var(--et-fs-meta, 0.75rem);
  color: var(--text-color-secondary, #94a3b8);
}

.acl-ok {
  color: var(--et-success, #10b981);
}

.acl-off {
  color: var(--et-danger, #ef4444);
}

.acl-rules-table :deep(.p-datatable-thead > tr > th) {
  padding: 0.4rem 0.55rem !important;
  font-size: var(--et-fs-meta, 0.75rem);
}

.acl-rules-table :deep(.p-datatable-tbody > tr > td) {
  padding: 0.4rem 0.55rem !important;
  font-size: var(--et-fs-body, 0.875rem);
  vertical-align: top;
}

@media (prefers-color-scheme: dark) {
  .acl-card {
    border-color: var(--surface-border, #334155);
    background: var(--surface-card, #1e293b);
  }

  .acl-card-footer {
    border-top-color: var(--surface-border, #334155);
  }

  .acl-section-title,
  .acl-chip {
    color: var(--text-color, #f1f5f9);
  }

  .acl-chip {
    background: var(--surface-100, #334155);
    border-color: var(--surface-border, #475569);
  }
}
</style>
