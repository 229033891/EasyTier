<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { Button, Checkbox, InputText, Password, useConfirm, useToast } from 'primevue';
import { tooltipDirective } from '../modules/tooltip';
import { useI18n } from 'vue-i18n';
import ApiClient, { UserInfo } from '../modules/api';
import ListPageShell from './ListPageShell.vue';
import FormField from './FormField.vue';

const vTooltip = tooltipDirective;
const { t } = useI18n();
const toast = useToast();
const confirm = useConfirm();

const props = defineProps<{
    api: ApiClient;
}>();

const users = ref<UserInfo[] | undefined>(undefined);
const creating = ref(false);
const resettingId = ref<number | null>(null);
const newUsername = ref('');
const newPassword = ref('');
const newIsAdmin = ref(false);
const resetFor = ref<UserInfo | null>(null);
const resetPassword = ref('');

const loadUsers = async () => {
    users.value = await props.api.list_users();
};

const createUser = async () => {
    const username = newUsername.value.trim();
    if (!username || !newPassword.value) {
        toast.add({
            severity: 'warn',
            summary: t('web.users.create_failed'),
            detail: t('web.users.username_password_required'),
            life: 2500,
        });
        return;
    }
    creating.value = true;
    try {
        await props.api.create_user({
            username,
            password: newPassword.value,
            is_admin: newIsAdmin.value,
        });
        newUsername.value = '';
        newPassword.value = '';
        newIsAdmin.value = false;
        toast.add({
            severity: 'success',
            summary: t('web.users.create_success'),
            life: 2000,
        });
        await loadUsers();
    } catch (e: any) {
        toast.add({
            severity: 'error',
            summary: t('web.users.create_failed'),
            detail: e?.response?.data?.message || String(e),
            life: 3500,
        });
    } finally {
        creating.value = false;
    }
};

const startReset = (user: UserInfo) => {
    resetFor.value = user;
    resetPassword.value = '';
};

const cancelReset = () => {
    resetFor.value = null;
    resetPassword.value = '';
};

const submitReset = async () => {
    const user = resetFor.value;
    if (!user || !resetPassword.value) {
        toast.add({
            severity: 'warn',
            summary: t('web.users.reset_failed'),
            detail: t('web.users.password_required'),
            life: 2500,
        });
        return;
    }
    resettingId.value = user.id;
    try {
        await props.api.reset_user_password(user.id, resetPassword.value);
        toast.add({
            severity: 'success',
            summary: t('web.users.reset_success'),
            life: 2000,
        });
        cancelReset();
    } catch (e: any) {
        toast.add({
            severity: 'error',
            summary: t('web.users.reset_failed'),
            detail: e?.response?.data?.message || String(e),
            life: 3500,
        });
    } finally {
        resettingId.value = null;
    }
};

const confirmDelete = (user: UserInfo) => {
    confirm.require({
        message: t('web.users.delete_confirm', { username: user.username }),
        header: t('web.users.delete'),
        icon: 'pi pi-exclamation-triangle',
        acceptClass: 'p-button-danger',
        accept: async () => {
            try {
                await props.api.delete_user(user.id);
                toast.add({
                    severity: 'success',
                    summary: t('web.users.delete_success'),
                    life: 2000,
                });
                await loadUsers();
            } catch (e: any) {
                toast.add({
                    severity: 'error',
                    summary: t('web.users.delete_failed'),
                    detail: e?.response?.data?.message || String(e),
                    life: 3500,
                });
            }
        },
    });
};

onMounted(async () => {
    try {
        await loadUsers();
    } catch (e: any) {
        toast.add({
            severity: 'error',
            summary: t('web.users.load_failed'),
            detail: e?.response?.data?.message || String(e),
            life: 3500,
        });
        users.value = [];
    }
});
</script>

<template>
    <div class="et-page">
        <h1 class="et-page-title">{{ t('web.main.user_list') }}</h1>

        <div class="user-create-form">
            <FormField class="field" :label="t('web.users.username')" label-for="new-username">
                <InputText id="new-username" v-model="newUsername" class="w-full" autocomplete="off" />
            </FormField>
            <FormField class="field" :label="t('web.users.password')" label-for="new-password">
                <Password id="new-password" v-model="newPassword" class="w-full" toggleMask :feedback="false"
                    autocomplete="new-password" />
            </FormField>
            <FormField class="field field-admin" :label="t('web.users.is_admin')" label-for="new-is-admin">
                <Checkbox inputId="new-is-admin" v-model="newIsAdmin" :binary="true" />
            </FormField>
            <div class="field field-action">
                <Button :label="t('web.users.create')" icon="pi pi-user-plus" :loading="creating"
                    @click="createUser" />
            </div>
        </div>

        <div v-if="resetFor" class="user-create-form">
            <FormField class="field" :label="t('web.users.reset_for')">
                <div class="reset-target">{{ resetFor.username }}</div>
            </FormField>
            <FormField class="field" :label="t('web.users.new_password')" label-for="reset-password">
                <Password id="reset-password" v-model="resetPassword" class="w-full" toggleMask :feedback="false"
                    autocomplete="new-password" />
            </FormField>
            <div class="field field-action">
                <Button :label="t('web.users.reset_password')" icon="pi pi-key"
                    :loading="resettingId === resetFor.id" @click="submitReset" />
            </div>
            <div class="field field-action">
                <Button :label="t('web.users.cancel')" severity="secondary" outlined @click="cancelReset" />
            </div>
        </div>

        <ListPageShell :loading="users === undefined" :empty="users?.length === 0">
            <template #empty>{{ t('web.users.empty') }}</template>
            <thead>
                <tr class="surface-ground text-left">
                    <th class="px-3 py-2 font-semibold">{{ t('web.users.username') }}</th>
                    <th class="px-3 py-2 font-semibold">{{ t('web.users.role') }}</th>
                    <th class="px-3 py-2 font-semibold text-right">{{ t('web.users.actions') }}</th>
                </tr>
            </thead>
            <tbody>
                <tr v-for="user in users" :key="user.id" class="border-t surface-border">
                    <td class="px-3 py-2 font-medium">{{ user.username }}</td>
                    <td class="px-3 py-2">
                        <span :class="user.is_admin ? 'role-admin' : 'role-user'">
                            {{ user.is_admin ? t('web.users.role_admin') : t('web.users.role_user') }}
                        </span>
                    </td>
                    <td class="px-3 py-2">
                        <div class="flex justify-end gap-1">
                            <Button v-tooltip.top="t('web.users.reset_password')"
                                icon="pi pi-key" severity="secondary" rounded text
                                class="user-action-btn"
                                @click="startReset(user)"
                                :aria-label="t('web.users.reset_password')" />
                            <Button v-tooltip.top="t('web.users.delete')"
                                icon="pi pi-trash" severity="danger" rounded text
                                class="user-action-btn"
                                @click="confirmDelete(user)"
                                :aria-label="t('web.users.delete')" />
                        </div>
                    </td>
                </tr>
            </tbody>
        </ListPageShell>
    </div>
</template>

<style scoped>
.user-create-form {
    display: flex;
    flex-wrap: wrap;
    gap: 0.75rem 1rem;
    align-items: flex-end;
    background: var(--surface-ground, #f8fafc);
    border: var(--et-border);
    border-radius: var(--et-radius);
    padding: var(--et-pad-card);
}

.field {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    min-width: 10rem;
    flex: 1 1 10rem;
}

/* label 位于 FormField 组件内部，需用 :deep 穿透 */
:deep(.field label) {
    font-size: var(--et-fs-meta);
    color: var(--text-color-secondary, #64748b);
}

.field-admin {
    flex: 0 0 auto;
    min-width: auto;
    align-items: flex-start;
}

.field-action {
    flex: 0 0 auto;
    min-width: auto;
}

.reset-target {
    min-height: 2.5rem;
    display: flex;
    align-items: center;
    font-weight: 600;
}

.role-admin {
    color: var(--primary-color, #0ea5e9);
    font-weight: 600;
}

.role-user {
    color: var(--text-color-secondary, #64748b);
}

.user-action-btn {
    width: var(--et-btn-sm) !important;
    height: var(--et-btn-sm) !important;
}

:deep(.p-password) {
    width: 100%;
}

:deep(.p-password input) {
    width: 100%;
}
</style>
