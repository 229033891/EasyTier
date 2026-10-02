<script setup lang="ts">
import { Button, Column, DataTable, Dialog, InputText, MultiSelect, Password } from 'primevue';
import { computed, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { GroupIdentity, GroupInfo, ensureGroupInfo } from '../../types/network';

const props = defineProps<{
  groupNames?: string[]
}>()

const group = defineModel<GroupInfo>({ required: true })
const emit = defineEmits(['rename-group'])

const { t } = useI18n()

const editingGroup = ref<GroupIdentity | null>(null)
const editingGroupIndex = ref(-1)
const showGroupDialog = ref(false)
const oldGroupName = ref('')

function groupInfo() {
  return ensureGroupInfo(group.value)
}

const members = computed({
  get: () => groupInfo().members,
  set: value => {
    groupInfo().members = value
  },
})

function addGroup() {
  editingGroupIndex.value = -1
  editingGroup.value = {
    group_name: '',
    group_secret: '',
  }
  oldGroupName.value = ''
  showGroupDialog.value = true
}

function editGroup(index: number) {
  editingGroupIndex.value = index
  editingGroup.value = JSON.parse(JSON.stringify(groupInfo().declares[index]))
  oldGroupName.value = editingGroup.value?.group_name || ''
  showGroupDialog.value = true
}

function deleteGroup(index: number) {
  groupInfo().declares.splice(index, 1)
}

function saveGroup() {
  if (!editingGroup.value) return
  const newName = editingGroup.value.group_name

  if (editingGroupIndex.value === -1) {
    groupInfo().declares.push(editingGroup.value)
  } else {
    if (oldGroupName.value && oldGroupName.value !== newName) {
      groupInfo().members = groupInfo().members.map(m => m === oldGroupName.value ? newName : m)
      emit('rename-group', { oldName: oldGroupName.value, newName })
    }
    groupInfo().declares[editingGroupIndex.value] = editingGroup.value
  }
  showGroupDialog.value = false
}

</script>

<template>
  <div class="acl-groups flex flex-col gap-4">
    <section class="acl-card flex flex-col gap-3">
      <div class="flex justify-between items-start gap-3">
        <div class="min-w-0">
          <div class="acl-section-title">{{ t('acl.group.declares') }}</div>
          <p class="acl-help">{{ t('acl.group.help') }}</p>
        </div>
        <Button icon="pi pi-plus" :label="t('web.common.add')" severity="success" size="small" @click="addGroup" />
      </div>

      <DataTable :value="groupInfo().declares" responsiveLayout="scroll" class="acl-groups-table">
        <Column field="group_name" :header="t('acl.group.name')" />
        <Column field="group_secret" :header="t('acl.group.secret')">
          <template #body="{ data }">
            <Password v-model="data.group_secret" :feedback="false" toggleMask readonly plain class="w-full" />
          </template>
        </Column>
        <Column :header="t('web.common.edit')" headerStyle="width: 6rem">
          <template #body="{ index }">
            <div class="flex gap-1">
              <Button icon="pi pi-pencil" severity="secondary" text rounded class="et-icon-action-btn"
                @click="editGroup(index)" />
              <Button icon="pi pi-trash" severity="danger" text rounded class="et-icon-action-btn"
                @click="deleteGroup(index)" />
            </div>
          </template>
        </Column>
      </DataTable>
    </section>

    <section class="acl-card flex flex-col gap-2">
      <div class="acl-section-title">{{ t('acl.group.members') }}</div>
      <p class="acl-help">{{ t('acl.group.members_help') }}</p>
      <MultiSelect v-model="members" :options="props.groupNames" multiple fluid filter
        :placeholder="t('acl.group.members')" />
    </section>

    <Dialog v-model:visible="showGroupDialog" modal :header="t('acl.groups')"
      class="et-dialog"
      :style="{ width: '90vw', maxWidth: '400px' }">
      <div v-if="editingGroup" class="flex flex-col gap-3">
        <div class="flex flex-col gap-1.5">
          <label class="acl-label">{{ t('acl.group.name') }}</label>
          <InputText v-model="editingGroup.group_name" fluid />
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="acl-label">{{ t('acl.group.secret') }}</label>
          <Password v-model="editingGroup.group_secret" :feedback="false" toggleMask fluid />
        </div>
      </div>
      <template #footer>
        <Button :label="t('web.common.cancel')" icon="pi pi-times" severity="secondary" outlined
          @click="showGroupDialog = false" />
        <Button :label="t('web.common.save')" icon="pi pi-save" @click="saveGroup" />
      </template>
    </Dialog>
  </div>
</template>

<style scoped>
.acl-card {
  padding: 0.75rem 0.85rem;
  border: 1px solid var(--et-border-color, #e2e8f0);
  border-radius: var(--et-radius, 0.75rem);
  background: var(--surface-50, #f8fafc);
}

.acl-section-title {
  font-size: var(--et-fs-section, 1rem);
  font-weight: 600;
  color: var(--text-color, #1e293b);
}

.acl-help {
  margin: 0.2rem 0 0;
  font-size: var(--et-fs-body, 0.875rem);
  color: var(--text-color-secondary, #64748b);
  line-height: 1.4;
}

.acl-label {
  font-size: var(--et-fs-meta, 0.75rem);
  font-weight: 600;
  color: var(--text-color-secondary, #64748b);
}

.acl-groups-table :deep(.p-datatable-thead > tr > th) {
  padding: 0.4rem 0.55rem !important;
  font-size: var(--et-fs-meta, 0.75rem);
}

.acl-groups-table :deep(.p-datatable-tbody > tr > td) {
  padding: 0.4rem 0.55rem !important;
  font-size: var(--et-fs-body, 0.875rem);
}

@media (prefers-color-scheme: dark) {
  .acl-card {
    border-color: var(--surface-border, #334155);
    background: var(--surface-card, #1e293b);
  }

  .acl-section-title {
    color: var(--text-color, #f1f5f9);
  }

  .acl-help,
  .acl-label {
    color: var(--text-color-secondary, #94a3b8);
  }
}
</style>
