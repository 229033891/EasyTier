<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { TOAST_LIFE } from 'easytier-frontend-lib'
import { Button, Checkbox, InputText, Password, useConfirm, useToast } from 'primevue';
import { tooltipDirective } from 'easytier-frontend-lib';
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
            life: TOAST_LIFE.warn,
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
            life: TOAST_LIFE.success,
        });
        await loadUsers();
    } catch (e: any) {
        toast.add({
            severity: 'error',
            summary: t('web.users.create_failed'),
            detail: e?.response?.data?.message || String(e),
            life: TOAST_LIFE.error,
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
            life: TOAST_LIFE.warn,
        });
        return;
    }
    resettingId.value = user.id;
    try {
        await props.api.reset_user_password(user.id, resetPassword.value);
        toast.add({
            severity: 'success',
            summary: t('web.users.reset_success'),
            life: TOAST_LIFE.success,
        });
        cancelReset();
    } catch (e: any) {
        toast.add({
            severity: 'error',
            summary: t('web.users.reset_failed'),
            detail: e?.response?.data?.message || String(e),
            life: TOAST_LIFE.error,
        });
    } finally {
        resettingId.value = null;
    }
};

const confirmDelete = (user: UserInfo) => {
    if (user.is_admin) {
        toast.add({
            severity: 'warn',
            summary: t('web.users.delete_failed'),
            detail: t('web.users.delete_admin_forbidden'),
            life: TOAST_LIFE.warn,
        });
        return;
    }
    confirm.require({
        message: t('web.users.delete_confirm', { username: user.username }),
        header: t('web.users.delete'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('web.users.delete'),
            severity: 'danger',
        },
        accept: async () => {
            try {
                await props.api.delete_user(user.id);
                toast.add({
                    severity: 'success',
                    summary: t('web.users.delete_success'),
                    life: TOAST_LIFE.success,
                });
                await loadUsers();
            } catch (e: any) {
                toast.add({
                    severity: 'error',
                    summary: t('web.users.delete_failed'),
                    detail: e?.response?.data?.message || String(e),
                    life: TOAST_LIFE.error,
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
            life: TOAST_LIFE.error,
        });
        users.value = [];
    }
});
</script>

<template>
    <div class="et-page">
        <div class="et-page-header">
            <h1 class="et-page-title">{{ t('web.main.user_list') }}</h1>
        </div>

        <div class="user-create-form">
            <FormField class="field field-grow" :label="t('web.users.username')" label-for="new-username">
                <InputText id="new-username" v-model="newUsername" class="w-full" autocomplete="off" />
            </FormField>
            <FormField class="field field-grow" :label="t('web.users.password')" label-for="new-password">
                <Password id="new-password" v-model="newPassword" class="w-full" toggleMask :feedback="false"
                    autocomplete="new-password" />
            </FormField>
            <div class="field field-admin">
                <span class="field-admin-spacer" aria-hidden="true">&nbsp;</span>
                <label for="new-is-admin" class="admin-check">
                    <Checkbox inputId="new-is-admin" v-model="newIsAdmin" :binary="true" />
                    <span>{{ t('web.users.is_admin') }}</span>
                </label>
            </div>
            <div class="field field-action">
                <span class="field-admin-spacer" aria-hidden="true">&nbsp;</span>
                <Button class="user-form-btn" :label="t('web.users.create')" icon="pi pi-user-plus"
                    :loading="creating" @click="createUser" />
            </div>
        </div>

        <div v-if="resetFor" class="user-create-form user-reset-form">
            <FormField class="field field-grow" :label="t('web.users.reset_for')">
                <div class="reset-target">{{ resetFor.username }}</div>
            </FormField>
            <FormField class="field field-grow" :label="t('web.users.new_password')" label-for="reset-password">
                <Password id="reset-password" v-model="resetPassword" class="w-full" toggleMask :feedback="false"
                    autocomplete="new-password" />
            </FormField>
            <div class="field field-action field-actions">
                <span class="field-admin-spacer" aria-hidden="true">&nbsp;</span>
                <div class="field-actions-row">
                    <Button class="user-form-btn" :label="t('web.users.reset_password')" icon="pi pi-key"
                        :loading="resettingId === resetFor.id" @click="submitReset" />
                    <Button class="user-form-btn" :label="t('web.users.cancel')" severity="secondary" outlined
                        @click="cancelReset" />
                </div>
            </div>
        </div>

        <ListPageShell :loading="users === undefined" :empty="users?.length === 0">
            <template #empty>{{ t('web.users.empty') }}</template>
            <thead>
                <tr class="bg-surface-50 text-left">
                    <th class="px-3 py-2 font-semibold">{{ t('web.users.username') }}</th>
                    <th class="px-3 py-2 font-semibold">{{ t('web.users.role') }}</th>
                    <th class="px-3 py-2 font-semibold text-right">{{ t('web.users.actions') }}</th>
                </tr>
            </thead>
            <tbody>
                <tr v-for="user in users" :key="user.id" class="border-t border-surface">
                    <td class="px-3 py-2 font-medium">{{ user.username }}</td>
                    <td class="px-3 py-2">
                        <span class="role-badge" :class="user.is_admin ? 'role-admin' : 'role-user'">
                            {{ user.is_admin ? t('web.users.role_admin') : t('web.users.role_user') }}
                        </span>
                    </td>
                    <td class="px-3 py-2">
                        <div class="flex justify-end gap-2">
                            <Button v-tooltip.top="t('web.users.reset_password')"
                                icon="pi pi-key" severity="secondary" rounded text
                                class="et-icon-action-btn"
                                @click="startReset(user)"
                                :aria-label="t('web.users.reset_password')" />
                            <Button v-if="!user.is_admin"
                                v-tooltip.top="t('web.users.delete')"
                                icon="pi pi-trash" severity="danger" rounded text
                                class="et-icon-action-btn"
                                @click="confirmDelete(user)"
                                :aria-label="t('web.users.delete')" />
                            <Button v-else
                                v-tooltip.top="t('web.users.delete_admin_forbidden')"
                                icon="pi pi-trash" severity="secondary" rounded text
                                class="et-icon-action-btn" disabled
                                :aria-label="t('web.users.delete_admin_forbidden')" />
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
    align-items: flex-end;
    gap: var(--et-space-3) var(--et-space-4);
    padding: var(--et-pad-card);
    background: var(--surface-card, #ffffff);
    border: var(--et-border);
    border-radius: var(--et-radius);
    box-shadow: var(--et-shadow-card);
}

.field {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    min-width: 0;
}

.field-grow {
    flex: 1 1 12rem;
    max-width: 18rem;
}

/* label 位于 FormField 组件内部，需用 :deep 穿透 */
:deep(.field label),
.field-admin-spacer {
    font-size: var(--et-fs-meta);
    line-height: 1.25;
    color: var(--text-color-secondary, #64748b);
}

.field-admin-spacer {
    display: block;
    visibility: hidden;
    user-select: none;
}

.field-admin {
    flex: 0 0 auto;
}

.admin-check {
    display: inline-flex;
    align-items: center;
    gap: var(--et-space-2);
    min-height: var(--et-btn, 2.5rem);
    margin: 0;
    font-size: var(--et-fs-body);
    color: var(--text-color, #1e293b);
    cursor: pointer;
    white-space: nowrap;
}

.field-action {
    flex: 0 0 auto;
    margin-left: auto;
}

.field-actions {
    margin-left: auto;
}

.field-actions-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 0.5rem;
}

.user-form-btn {
    width: var(--et-btn-w, 10rem) !important;
    min-width: var(--et-btn-w, 10rem) !important;
    height: var(--et-btn, 2.5rem) !important;
    min-height: var(--et-btn, 2.5rem) !important;
    justify-content: center;
    white-space: nowrap;
}

.reset-target {
    min-height: 2.5rem;
    display: flex;
    align-items: center;
    font-weight: 600;
}

.role-badge {
    display: inline-flex;
    align-items: center;
    padding: 0.15rem 0.55rem;
    border-radius: 999px;
    font-size: var(--et-fs-meta);
    font-weight: 600;
    line-height: 1.4;
}

.role-admin {
    color: var(--primary-color, #0ea5e9);
    background: color-mix(in srgb, var(--primary-color, #0ea5e9) 12%, transparent);
}

.role-user {
    color: var(--text-color-secondary, #64748b);
    background: var(--surface-100, #f1f5f9);
}

@media (max-width: 640px) {
    .field-grow {
        flex: 1 1 100%;
        max-width: none;
    }

    .field-action,
    .field-actions {
        margin-left: 0;
        width: 100%;
    }

    .field-admin-spacer {
        display: none;
    }

    .user-form-btn {
        width: 100% !important;
        min-width: 0 !important;
    }

    .field-actions-row {
        flex-direction: column;
        align-items: stretch;
    }
}

:deep(.p-password) {
    width: 100%;
}

:deep(.p-password input) {
    width: 100%;
}
</style>
