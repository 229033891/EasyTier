<script lang="ts" setup>
import { computed, inject, ref } from 'vue';
import { Card, Password, Button } from 'primevue';
import { useI18n } from 'vue-i18n';
import ApiClient from '../modules/api';

const { t } = useI18n();
const dialogRef = inject<any>('dialogRef');

const api = computed<ApiClient>(() => dialogRef.value.data.api);

const password = ref('');
const submitting = ref(false);

const changePassword = async () => {
    if (submitting.value || !password.value) return;
    submitting.value = true;
    try {
        await api.value.change_password(password.value);
        dialogRef.value.close();
    } finally {
        submitting.value = false;
    }
}
</script>

<template>
    <div class="change-password">
        <Card class="change-password-card">
            <template #header>
                <h2 class="change-password-title">{{ t('web.main.change_password') }}</h2>
            </template>
            <template #content>
                <div class="change-password-form">
                    <Password v-model="password" :placeholder="t('web.login.password')" :feedback="false" toggleMask
                        class="w-full" input-class="w-full"
                        :input-props="{ autocomplete: 'new-password' }" />
                    <div class="change-password-actions">
                        <Button severity="secondary" outlined :label="t('web.common.cancel')"
                            @click="dialogRef.close()" />
                        <Button :label="t('web.common.confirm')" :loading="submitting" :disabled="!password"
                            @click="changePassword" />
                    </div>
                </div>
            </template>
        </Card>
    </div>
</template>

<style scoped>
.change-password {
    display: flex;
    align-items: stretch;
    justify-content: center;
}

.change-password-card {
    width: 100%;
    max-width: 24rem;
    border: 1px solid var(--et-border-color, #e2e8f0) !important;
    border-radius: var(--et-radius, 0.75rem) !important;
    background: var(--surface-card, #ffffff) !important;
    box-shadow: var(--et-shadow-card, none);
    overflow: hidden;
}

.change-password-title {
    margin: 0;
    padding: 1.1rem 1.25rem 0.35rem;
    color: var(--text-color, #1e293b);
    font-size: var(--et-fs-page-title);
    font-weight: 700;
    letter-spacing: -0.02em;
}

.change-password-form {
    display: flex;
    flex-direction: column;
    gap: var(--et-space-4);
    padding: var(--et-space-2) 1.25rem 1.25rem;
}

.change-password-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--et-space-2);
}

:deep(.p-card-body),
:deep(.p-card-content),
:deep(.p-card-header) {
    padding: 0 !important;
}

:deep(.p-password),
:deep(.p-password-input) {
    width: 100%;
}

:deep(.p-button) {
    min-height: var(--et-btn, 2.5rem);
    border-radius: var(--et-radius, 0.75rem) !important;
    font-weight: 600;
}
</style>
