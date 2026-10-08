<script setup lang="ts">
import { AutoComplete, Button, Checkbox, Dialog, InputNumber, InputText, MultiSelect, Panel, SelectButton, ToggleButton } from 'primevue';
import { computed, reactive, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { AclAction, AclProtocol, AclRule, ensureAclRuleLists } from '../../types/network';
import { isPanelHeaderInteractiveTarget } from '../../modules/panel';

const props = defineProps<{
  visible: boolean
  groupNames?: string[]
}>()

const emit = defineEmits(['update:visible', 'save'])

const rule = defineModel<AclRule>('rule', { required: true })

const { t } = useI18n()

/** 整块标题栏可点展开/折叠（触摸友好）；加减号自身已切换，需跳过以免翻两次 */
const panelCollapsed = reactive({
  rules: false,
  advanced: true,
})

function onToggleablePanelHeaderClick(
  key: keyof typeof panelCollapsed,
  event: Event,
) {
  const target = event.target as HTMLElement | null
  if (isPanelHeaderInteractiveTarget(target, event)) {
    return
  }
  panelCollapsed[key] = !panelCollapsed[key]
}

function panelHeaderPt(key: keyof typeof panelCollapsed) {
  return {
    header: {
      class: 'cursor-pointer select-none touch-manipulation',
      onClick: (event: Event) => onToggleablePanelHeaderClick(key, event),
    },
    headerActions: {
      onClick: (event: Event) => event.stopPropagation(),
    },
  }
}

const protocolOptions = [
  { label: () => t('acl.any'), value: AclProtocol.Any },
  { label: 'TCP', value: AclProtocol.TCP },
  { label: 'UDP', value: AclProtocol.UDP },
  { label: 'ICMP', value: AclProtocol.ICMP },
  { label: 'ICMPv6', value: AclProtocol.ICMPv6 },
]

const actionOptions = [
  { label: () => t('acl.allow'), value: AclAction.Allow },
  { label: () => t('acl.drop'), value: AclAction.Drop },
]

const showPorts = computed(() => {
  return rule.value.protocol === AclProtocol.TCP || rule.value.protocol === AclProtocol.UDP || rule.value.protocol === AclProtocol.Any
})

watch(() => rule.value, ensureAclRuleLists, { immediate: true })

function close() {
  emit('update:visible', false)
}

function save() {
  emit('save', rule.value)
  close()
}

function onChipsKeydown(event: KeyboardEvent, values: string[]) {
  if (event.isComposing) return
  if (event.key !== 'Enter' && event.key !== ',' && event.key !== '，') return
  const input = event.target as HTMLInputElement | null
  if (!input || input.tagName !== 'INPUT') return
  const raw = input.value.trim()
  if (!raw) return
  event.preventDefault()
  event.stopImmediatePropagation()
  const next = [...values]
  let changed = false
  for (const part of raw.split(/[,，]+/)) {
    const token = part.trim()
    if (token && !next.includes(token)) {
      next.push(token)
      changed = true
    }
  }
  if (changed) values.splice(0, values.length, ...next)
  input.value = ''
  input.dispatchEvent(new Event('input', { bubbles: true }))
}
</script>

<template>
  <Dialog :visible="visible" @update:visible="emit('update:visible', $event)" modal :header="t('acl.edit_rule')"
    class="et-dialog"
    :style="{ width: '90vw', maxWidth: '600px' }">
    <div class="flex flex-col gap-4">
      <div class="flex flex-row gap-4 items-center">
        <div class="flex flex-col gap-2 grow">
          <label class="acl-dlg-label">{{ t('acl.rule.name') }}</label>
          <InputText v-model="rule.name" fluid />
        </div>
        <div class="flex flex-col gap-2">
          <label class="acl-dlg-label">{{ t('acl.rule.enabled') }}</label>
          <ToggleButton v-model="rule.enabled" on-icon="pi pi-check" off-icon="pi pi-times"
            :on-label="t('web.common.enable')" :off-label="t('web.common.disable')" class="w-24" />
        </div>
      </div>

      <div class="flex flex-col gap-2">
        <label class="acl-dlg-label">{{ t('acl.rule.description') }}</label>
        <InputText v-model="rule.description" fluid />
      </div>

      <div class="flex flex-row gap-4 flex-wrap">
        <div class="flex flex-col gap-2 grow">
          <label class="acl-dlg-label">{{ t('acl.rule.action') }}</label>
          <SelectButton v-model="rule.action" :options="actionOptions" :option-label="opt => opt.label()"
            option-value="value" :allow-empty="false" />
        </div>
        <div class="flex flex-col gap-2 grow">
          <label class="acl-dlg-label">{{ t('acl.rule.protocol') }}</label>
          <SelectButton v-model="rule.protocol" :options="protocolOptions"
            :option-label="opt => typeof opt.label === 'function' ? opt.label() : opt.label" option-value="value"
            :allow-empty="false" />
        </div>
      </div>

      <Panel v-model:collapsed="panelCollapsed.rules" :header="t('acl.rules')" toggleable
        :pt="panelHeaderPt('rules')">
        <div class="flex flex-col gap-3">
          <div class="flex flex-col gap-1.5">
            <label class="acl-dlg-label">{{ t('acl.rule.src_ips') }}</label>
            <AutoComplete v-model="rule.source_ips" multiple fluid :typeahead="false"
              :placeholder="t('chips_placeholder', ['10.126.126.0/24'])"
              @keydown.capture="onChipsKeydown($event, rule.source_ips)" />
          </div>
          <div class="flex flex-col gap-1.5">
            <label class="acl-dlg-label">{{ t('acl.rule.dst_ips') }}</label>
            <AutoComplete v-model="rule.destination_ips" multiple fluid :typeahead="false"
              :placeholder="t('chips_placeholder', ['10.126.126.2/32'])"
              @keydown.capture="onChipsKeydown($event, rule.destination_ips)" />
          </div>

          <div v-if="showPorts" class="flex flex-row gap-4 flex-wrap">
            <div class="flex flex-col gap-1.5 grow">
              <label class="acl-dlg-label">{{ t('acl.rule.src_ports') }}</label>
              <AutoComplete v-model="rule.source_ports" multiple fluid :typeahead="false"
                :placeholder="t('chips_placeholder', ['80, 1000-2000'])"
                @keydown.capture="onChipsKeydown($event, rule.source_ports)" />
            </div>
            <div class="flex flex-col gap-1.5 grow">
              <label class="acl-dlg-label">{{ t('acl.rule.dst_ports') }}</label>
              <AutoComplete v-model="rule.ports" multiple fluid :typeahead="false"
                :placeholder="t('chips_placeholder', ['80, 1000-2000'])"
                @keydown.capture="onChipsKeydown($event, rule.ports)" />
            </div>
          </div>
        </div>
      </Panel>

      <Panel v-model:collapsed="panelCollapsed.advanced" :header="t('advanced_settings')" toggleable
        :pt="panelHeaderPt('advanced')">
        <div class="flex flex-col gap-3">
          <div class="flex items-center gap-2">
            <Checkbox v-model="rule.stateful" :binary="true" inputId="rule-stateful" />
            <label for="rule-stateful" class="acl-dlg-label">{{ t('acl.rule.stateful') }}</label>
          </div>

          <div class="flex flex-row gap-4 flex-wrap">
            <div class="flex flex-col gap-1.5 grow">
              <label class="acl-dlg-label">{{ t('acl.rule.rate_limit') }}</label>
              <InputNumber v-model="rule.rate_limit" :min="0" placeholder="0" fluid />
            </div>
            <div class="flex flex-col gap-1.5 grow">
              <label class="acl-dlg-label">{{ t('acl.rule.burst_limit') }}</label>
              <InputNumber v-model="rule.burst_limit" :min="0" placeholder="0" fluid />
            </div>
          </div>

          <div class="flex flex-col gap-1.5">
            <label class="acl-dlg-label">{{ t('acl.rule.src_groups') }}</label>
            <MultiSelect v-model="rule.source_groups" :options="props.groupNames" multiple fluid filter
              :placeholder="t('acl.rule.src_groups')" />
          </div>
          <div class="flex flex-col gap-1.5">
            <label class="acl-dlg-label">{{ t('acl.rule.dst_groups') }}</label>
            <MultiSelect v-model="rule.destination_groups" :options="props.groupNames" multiple fluid filter
              :placeholder="t('acl.rule.dst_groups')" />
          </div>
        </div>
      </Panel>
    </div>

    <template #footer>
      <Button :label="t('web.common.cancel')" icon="pi pi-times" severity="secondary" outlined @click="close" />
      <Button :label="t('web.common.save')" icon="pi pi-save" @click="save" />
    </template>
  </Dialog>
</template>

<style scoped>
.acl-dlg-label {
  font-size: var(--et-fs-meta, 0.75rem);
  font-weight: 600;
  color: var(--text-color-secondary, #64748b);
}
</style>
