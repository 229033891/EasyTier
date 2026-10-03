<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { TOAST_LIFE, Utils, tooltipDirective } from 'easytier-frontend-lib'
import { Button, InputText, Select, useConfirm, useToast } from 'primevue';
import { useI18n } from 'vue-i18n';
import ApiClient, { ConfigTokenInfo, UserInfo } from '../modules/api';
import ListPageShell from './ListPageShell.vue';
import FormField from './FormField.vue';

const vTooltip = tooltipDirective;
const { t } = useI18n();
const toast = useToast();
const confirm = useConfirm();
const errorDetail = (error: unknown) => Utils.formatApiErrorDetail(error, t);

const props = defineProps<{
    api: ApiClient;
}>();

const tokens = ref<ConfigTokenInfo[] | undefined>(undefined);
const users = ref<UserInfo[]>([]);
const creating = ref(false);
const savingId = ref<number | null>(null);

const newUserId = ref<number | null>(null);
const newToken = ref('');
const newLabel = ref('');

const editId = ref<number | null>(null);
const editToken = ref('');
const editLabel = ref('');

const load = async () => {
    try {
        const [tokenList, userList] = await Promise.all([
            props.api.list_config_tokens(),
            props.api.list_users(),
        ]);
        tokens.value = tokenList;
        users.value = userList;
        if (newUserId.value == null && userList.length > 0) {
            newUserId.value = userList[0].id;
        }
    } catch (e: any) {
        tokens.value = [];
        toast.add({
            severity: 'error',
            summary: t('web.common.error'),
            detail: errorDetail(e),
            life: TOAST_LIFE.error,
        });
    }
};

const createToken = async () => {
    if (newUserId.value == null) {
        toast.add({
            severity: 'warn',
            summary: t('web.config_tokens.create_failed'),
            detail: t('web.config_tokens.user_required'),
            life: TOAST_LIFE.warn,
        });
        return;
    }
    creating.value = true;
    try {
        await props.api.create_config_token({
            user_id: newUserId.value,
            token: newToken.value.trim() || undefined,
            label: newLabel.value.trim() || undefined,
        });
        newToken.value = '';
        newLabel.value = '';
        toast.add({
            severity: 'success',
            summary: t('web.config_tokens.create_success'),
            life: TOAST_LIFE.success,
        });
        await load();
    } catch (e: any) {
        toast.add({
            severity: 'error',
            summary: t('web.config_tokens.create_failed'),
            detail: errorDetail(e),
            life: TOAST_LIFE.error,
        });
    } finally {
        creating.value = false;
    }
};

const startEdit = (row: ConfigTokenInfo) => {
    editId.value = row.id;
    editToken.value = row.token;
    editLabel.value = row.label;
};

const cancelEdit = () => {
    editId.value = null;
    editToken.value = '';
    editLabel.value = '';
};

const submitEdit = async () => {
    if (editId.value == null) return;
    savingId.value = editId.value;
    try {
        await props.api.update_config_token(editId.value, {
            token: editToken.value.trim() || undefined,
            label: editLabel.value.trim(),
        });
        toast.add({
            severity: 'success',
            summary: t('web.config_tokens.update_success'),
            life: TOAST_LIFE.success,
        });
        cancelEdit();
        await load();
    } catch (e: any) {
        toast.add({
            severity: 'error',
            summary: t('web.config_tokens.update_failed'),
            detail: errorDetail(e),
            life: TOAST_LIFE.error,
        });
    } finally {
        savingId.value = null;
    }
};

const confirmDelete = (row: ConfigTokenInfo) => {
    confirm.require({
        message: t('web.config_tokens.confirm_delete', { token: row.token }),
        header: t('web.config_tokens.delete'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('web.config_tokens.delete'),
            severity: 'danger',
        },
        accept: async () => {
            try {
                await props.api.delete_config_token(row.id);
                toast.add({
                    severity: 'success',
                    summary: t('web.config_tokens.delete_success'),
                    life: TOAST_LIFE.success,
                });
                await load();
            } catch (e: any) {
                toast.add({
                    severity: 'error',
                    summary: t('web.config_tokens.delete_failed'),
                    detail: errorDetail(e),
                    life: TOAST_LIFE.error,
                });
            }
        },
    });
};

const copyToken = async (token: string) => {
    try {
        await navigator.clipboard.writeText(token);
        toast.add({
            severity: 'success',
            summary: t('web.config_tokens.copied'),
            life: TOAST_LIFE.success,
        });
    } catch {
        toast.add({
            severity: 'error',
            summary: t('web.config_tokens.copy_failed'),
            life: TOAST_LIFE.error,
        });
    }
};

onMounted(load);
</script>

<template>
    <div class="flex flex-col gap-4 p-4">
        <div class="et-panel flex flex-col gap-3 p-4">
            <h2 class="text-lg font-semibold m-0">{{ t('web.config_tokens.create_title') }}</h2>
            <p class="et-meta m-0">{{ t('web.config_tokens.hint') }}</p>
            <div class="grid grid-cols-1 md:grid-cols-4 gap-3 items-end">
                <FormField :label="t('web.config_tokens.user')">
                    <Select
                        v-model="newUserId"
                        :options="users"
                        option-label="username"
                        option-value="id"
                        class="w-full"
                        :placeholder="t('web.config_tokens.user')"
                        :empty-message="t('web.users.empty')"
                    />
                </FormField>
                <FormField :label="t('web.config_tokens.token')">
                    <InputText
                        v-model="newToken"
                        class="w-full"
                        :placeholder="t('web.config_tokens.token_placeholder')"
                    />
                </FormField>
                <FormField :label="t('web.config_tokens.label')">
                    <InputText
                        v-model="newLabel"
                        class="w-full"
                        :placeholder="t('web.config_tokens.label_placeholder')"
                    />
                </FormField>
                <Button
                    :label="t('web.config_tokens.create')"
                    icon="pi pi-plus"
                    :loading="creating"
                    @click="createToken"
                />
            </div>
        </div>

        <ListPageShell :loading="tokens === undefined" :empty="!!tokens && tokens.length === 0">
            <template #empty>{{ t('web.config_tokens.empty') }}</template>
            <thead>
                <tr>
                    <th>{{ t('web.config_tokens.user') }}</th>
                    <th>{{ t('web.config_tokens.token') }}</th>
                    <th>{{ t('web.config_tokens.label') }}</th>
                    <th>{{ t('web.config_tokens.url_example') }}</th>
                    <th></th>
                </tr>
            </thead>
            <tbody>
                <tr v-for="row in tokens" :key="row.id">
                    <td>{{ row.username }}</td>
                    <td>
                        <template v-if="editId === row.id">
                            <InputText v-model="editToken" class="w-full" />
                        </template>
                        <template v-else>
                            <code class="et-code">{{ row.token }}</code>
                            <Button
                                icon="pi pi-copy"
                                text
                                rounded
                                size="small"
                                v-tooltip.top="t('web.config_tokens.copy')"
                                @click="copyToken(row.token)"
                            />
                        </template>
                    </td>
                    <td>
                        <InputText v-if="editId === row.id" v-model="editLabel" class="w-full" />
                        <span v-else>{{ row.label || '—' }}</span>
                    </td>
                    <td>
                        <div class="flex flex-col gap-0.5">
                            <code class="et-code text-xs">udp://&lt;host&gt;:22020/{{ row.token }}</code>
                            <code class="et-code text-xs">tcp://&lt;host&gt;:22020/{{ row.token }}</code>
                        </div>
                    </td>
                    <td class="whitespace-nowrap">
                        <template v-if="editId === row.id">
                            <Button
                                icon="pi pi-check"
                                text
                                rounded
                                :loading="savingId === row.id"
                                @click="submitEdit"
                            />
                            <Button icon="pi pi-times" text rounded @click="cancelEdit" />
                        </template>
                        <template v-else>
                            <Button icon="pi pi-pencil" text rounded @click="startEdit(row)" />
                            <Button
                                icon="pi pi-trash"
                                text
                                rounded
                                severity="danger"
                                @click="confirmDelete(row)"
                            />
                        </template>
                    </td>
                </tr>
            </tbody>
        </ListPageShell>
    </div>
</template>
