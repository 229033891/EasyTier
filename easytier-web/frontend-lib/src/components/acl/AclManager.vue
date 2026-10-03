<script setup lang="ts">
import { Button, Menu, Tab, TabList, TabPanel, TabPanels, Tabs } from 'primevue'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Acl, AclAction, AclChainType, ensureAclV1 } from '../../types/network'
import AclChainEditor from './AclChainEditor.vue'
import AclGroupEditor from './AclGroupEditor.vue'

const acl = defineModel<Acl>({ required: true })

const { t } = useI18n()

const activeTab = ref(0)
const menu = ref()
const aclV1 = computed(() => ensureAclV1(acl.value))

const addMenuModel = ref([
  { label: () => t('acl.inbound'), command: () => addChain(AclChainType.Inbound) },
  { label: () => t('acl.outbound'), command: () => addChain(AclChainType.Outbound) },
  { label: () => t('acl.forward'), command: () => addChain(AclChainType.Forward) },
])

function addChain(type: AclChainType) {
  let defaultName = ''
  switch (type) {
    case AclChainType.Inbound: defaultName = 'Inbound'; break;
    case AclChainType.Outbound: defaultName = 'Outbound'; break;
    case AclChainType.Forward: defaultName = 'Forward'; break;
  }

  aclV1.value.chains.push({
    name: defaultName,
    chain_type: type,
    description: '',
    enabled: true,
    rules: [],
    default_action: AclAction.Allow
  })

  activeTab.value = aclV1.value.chains.length - 1
}

function removeChain(index: number) {
  if (confirm(t('acl.delete_chain_confirm'))) {
    aclV1.value.chains.splice(index, 1)
    if (activeTab.value >= aclV1.value.chains.length) {
      activeTab.value = Math.max(0, aclV1.value.chains.length)
    }
  }
}

function handleRenameGroup({ oldName, newName }: { oldName: string, newName: string }) {
  aclV1.value.chains.forEach(chain => {
    chain.rules.forEach(rule => {
      rule.source_groups = rule.source_groups.map(g => g === oldName ? newName : g)
      rule.destination_groups = rule.destination_groups.map(g => g === oldName ? newName : g)
    })
  })
}

const groupNames = computed(() => {
  return aclV1.value.group?.declares.map(g => g.group_name) || []
})

const tabs = computed(() => {
  const chains = aclV1.value.chains
  const result: { type: string, label: string, index: number }[] = []

  if (chains.length === 0) {
    result.push({ type: 'empty', label: t('acl.chains'), index: 0 })
  }
  else {
    chains.forEach((c, index) => {
      result.push({
        type: 'chain',
        label: c.name || `Chain ${index}`,
        index
      })
    })
  }

  result.push({ type: 'groups', label: t('acl.groups'), index: result.length })
  return result
})
</script>

<template>
  <div class="acl-manager flex flex-col gap-3">
    <Tabs v-model:value="activeTab">
      <div class="acl-tabs-bar flex items-center border-b border-surface-200 dark:border-surface-700">
        <TabList class="flex-grow min-w-0 overflow-x-auto" style="border-bottom: none;">
          <Tab v-for="tab in tabs" :key="tab.type + tab.index" :value="tab.index">
            <div class="flex items-center gap-1.5 whitespace-nowrap">
              <span class="text-sm font-medium">{{ tab.label }}</span>
              <Button v-if="tab.type === 'chain'" icon="pi pi-times" severity="danger" text rounded size="small"
                class="et-icon-action-btn" :aria-label="t('web.common.delete')"
                @click.stop="removeChain(tab.index)" />
            </div>
          </Tab>
        </TabList>
        <div class="flex-shrink-0 flex items-center pl-1">
          <Button icon="pi pi-plus" severity="secondary" text rounded size="small" class="et-icon-action-btn"
            :aria-label="t('acl.add_chain')" @click="(event) => menu.toggle(event)" />
          <Menu ref="menu" :model="addMenuModel" :popup="true" />
        </div>
      </div>
      <TabPanels>
        <TabPanel v-for="tab in tabs" :key="'panel' + tab.type + tab.index" :value="tab.index">
          <div v-if="tab.type === 'empty'" class="acl-empty">
            <i class="pi pi-shield acl-empty-icon" />
            <div class="acl-section-title">{{ t('acl.chains') }}</div>
            <p class="acl-help">{{ t('acl.empty_chains') }}</p>
            <div class="flex flex-wrap gap-2 justify-center">
              <Button class="et-panel-action-btn acl-chain-add-btn" :label="t('acl.inbound')"
                icon="pi pi-arrow-down-left" severity="secondary"
                @click="addChain(AclChainType.Inbound)" />
              <Button class="et-panel-action-btn acl-chain-add-btn" :label="t('acl.outbound')"
                icon="pi pi-arrow-up-right" severity="secondary"
                @click="addChain(AclChainType.Outbound)" />
              <Button class="et-panel-action-btn acl-chain-add-btn" :label="t('acl.forward')"
                icon="pi pi-directions" severity="info"
                @click="addChain(AclChainType.Forward)" />
            </div>
          </div>

          <div v-if="tab.type === 'chain' && aclV1.chains[tab.index]" class="pt-3">
            <AclChainEditor v-model="aclV1.chains[tab.index]" :group-names="groupNames" />
          </div>

          <div v-if="tab.type === 'groups'" class="pt-3">
            <AclGroupEditor v-model="aclV1.group" :group-names="groupNames" @rename-group="handleRenameGroup" />
          </div>
        </TabPanel>
      </TabPanels>
    </Tabs>
  </div>
</template>

<style scoped>
.acl-manager :deep(.p-tablist) {
  background: transparent;
  border: none;
  gap: 0.35rem;
}

.acl-manager :deep(.p-tablist-content),
.acl-manager :deep(.p-tablist-tab-list) {
  border: none;
  background: transparent;
  gap: 0.35rem;
}

.acl-tabs-bar {
  gap: 0.35rem;
  padding: 0.2rem;
  margin-bottom: 0.15rem;
  border: 1px solid var(--et-border-color, #e2e8f0) !important;
  border-radius: var(--et-radius, 0.75rem);
  background: var(--surface-50, #f8fafc);
}

.acl-manager :deep(.p-tab) {
  margin: 0;
  padding: 0.45rem 0.75rem !important;
  border: 1px solid transparent !important;
  border-radius: calc(var(--et-radius, 0.75rem) - 0.2rem) !important;
  background: transparent !important;
  color: var(--text-color-secondary, #64748b) !important;
  box-shadow: none !important;
}

.acl-manager :deep(.p-tab:not(.p-tab-active):hover) {
  background: color-mix(in srgb, var(--primary-color, #0ea5e9) 8%, #ffffff) !important;
  color: var(--primary-color, #0284c7) !important;
}

.acl-manager :deep(.p-tab.p-tab-active),
.acl-manager :deep(.p-tab[data-p-active="true"]) {
  background: color-mix(in srgb, var(--primary-color, #0ea5e9) 12%, #ffffff) !important;
  border-color: color-mix(in srgb, var(--primary-color, #0ea5e9) 32%, var(--et-border-color, #e2e8f0)) !important;
  color: var(--primary-color, #0284c7) !important;
  font-weight: 600;
}

.acl-manager :deep(.p-tabpanels),
.acl-manager :deep(.p-tabpanel) {
  padding: 0;
  background: transparent;
  border: none;
  box-shadow: none;
}

.acl-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.5rem;
  padding: 1.25rem 1rem;
  border: 1px dashed var(--et-border-color, #e2e8f0);
  border-radius: var(--et-radius, 0.75rem);
  background: var(--surface-50, #f8fafc);
}

.acl-empty-icon {
  font-size: 1.5rem;
  color: var(--primary-color, var(--et-primary, #0ea5e9));
  margin-bottom: 0.15rem;
}

.acl-section-title {
  font-size: var(--et-fs-section, 1rem);
  font-weight: 600;
  color: var(--text-color, #1e293b);
}

.acl-help {
  margin: 0 0 0.5rem;
  max-width: 22rem;
  text-align: center;
  font-size: var(--et-fs-body, 0.875rem);
  color: var(--text-color-secondary, #64748b);
  line-height: 1.45;
  padding: 0 0.5rem;
}

.acl-chain-add-btn {
  /* keep panel action height; allow slightly narrower on wrap */
  min-width: 7.5rem !important;
  width: auto !important;
}

@media (prefers-color-scheme: dark) {
  .acl-tabs-bar {
    border-color: var(--surface-border, #334155) !important;
    background: var(--surface-card, #1e293b);
  }

  .acl-manager :deep(.p-tab) {
    color: var(--text-color-secondary, #94a3b8) !important;
  }

  .acl-manager :deep(.p-tab:not(.p-tab-active):hover) {
    background: color-mix(in srgb, var(--primary-color, #0ea5e9) 16%, transparent) !important;
  }

  .acl-manager :deep(.p-tab.p-tab-active),
  .acl-manager :deep(.p-tab[data-p-active="true"]) {
    background: color-mix(in srgb, var(--primary-color, #0ea5e9) 20%, transparent) !important;
    border-color: color-mix(in srgb, var(--primary-color, #0ea5e9) 40%, #334155) !important;
    color: var(--primary-color, #38bdf8) !important;
  }

  .acl-empty {
    border-color: var(--surface-border, #334155);
    background: var(--surface-card, #1e293b);
  }

  .acl-section-title {
    color: var(--text-color, #f1f5f9);
  }

  .acl-help {
    color: var(--text-color-secondary, #94a3b8);
  }
}
</style>
