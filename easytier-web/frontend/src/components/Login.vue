<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { Card, InputText, Password, Button, AutoComplete } from 'primevue';
import { useRouter } from 'vue-router';
import { useToast } from 'primevue/usetoast';
import { getInitialApiHost, cleanAndLoadApiHosts, saveApiHost } from "../modules/api-host"
import { useI18n } from 'vue-i18n'
import ApiClient, { Credential } from '../modules/api';
import FormField from './FormField.vue';
import Icon from '../assets/easytier.png';

const { t } = useI18n()

const api = computed<ApiClient>(() => new ApiClient(apiHost.value));
const router = useRouter();
const toast = useToast();

const username = ref('');
const password = ref('');
const submitting = ref(false);

const onSubmit = async () => {
    if (submitting.value) return;
    submitting.value = true;
    try {
        saveApiHost(apiHost.value);
        const credential: Credential = { username: username.value, password: password.value, };
        let ret = await api.value?.login(credential);
        if (ret.success) {
            localStorage.setItem('apiHost', btoa(apiHost.value));
            router.push({
                name: 'dashboard',
                params: { apiHost: btoa(apiHost.value) },
            });
        } else {
            toast.add({ severity: 'error', summary: 'Login Failed', detail: ret.message, life: 2000 });
        }
    } finally {
        submitting.value = false;
    }
};

const apiHost = ref<string>(getInitialApiHost())
const apiHostSuggestions = ref<Array<string>>([])
const apiHostSearch = async (event: { query: string }) => {
    const suggestions: string[] = [];
    const hosts = cleanAndLoadApiHosts();
    if (event.query) {
        suggestions.push(event.query);
    }
    // 当前访问地址优先出现在下拉
    if (!suggestions.includes(apiHost.value)) {
        suggestions.push(apiHost.value);
    }
    hosts.forEach((host) => {
        if (!suggestions.includes(host.value)) {
            suggestions.push(host.value);
        }
    });
    apiHostSuggestions.value = suggestions;
}

const oidcEnabled = ref(false);
const lastCheckedHost = ref('');
const oidcCheckTimer = ref<ReturnType<typeof setTimeout> | null>(null);
const checkOidcConfig = () => {
    if (oidcCheckTimer.value) clearTimeout(oidcCheckTimer.value);
    oidcCheckTimer.value = setTimeout(async () => {
        const host = apiHost.value;
        if (host === lastCheckedHost.value) return;

        try {
            const enabled = (await new ApiClient(host).getOidcConfig()).enabled;
            if (apiHost.value !== host) return;

            lastCheckedHost.value = host;
            oidcEnabled.value = enabled;
        } catch (error) {
            // A transient host/network error must not leave stale SSO state visible.
            if (apiHost.value === host) {
                lastCheckedHost.value = host;
                oidcEnabled.value = false;
            }
            console.debug('OIDC config check failed', error);
        }
    }, 300);
};

watch(apiHost, () => {
    checkOidcConfig();
});

const onSsoLogin = () => {
    saveApiHost(apiHost.value);
    localStorage.setItem('apiHost', btoa(apiHost.value));
    window.location.href = api.value.oidcLoginUrl();
};

onMounted(() => {
    checkOidcConfig();
});

onBeforeUnmount(() => {
    if (oidcCheckTimer.value) {
        clearTimeout(oidcCheckTimer.value);
        oidcCheckTimer.value = null;
    }
});
</script>

<template>
    <div class="login-page">
        <div class="login-shell">
            <Card class="login-card">
                <template #header>
                    <div class="login-brand">
                        <img :src="Icon" :alt="t('web.main.logo_alt')" class="login-logo" />
                        <div class="login-brand-text">
                            <div class="login-brand-name">EasyTier</div>
                            <p class="login-brand-sub">{{ t('web.login.login') }}</p>
                        </div>
                    </div>
                </template>
                <template #content>
                    <div class="login-form">
                        <FormField class="login-field" :label="t('web.login.api_host')" label-for="api-host">
                            <AutoComplete id="api-host" v-model="apiHost" dropdown :suggestions="apiHostSuggestions"
                                @complete="apiHostSearch" class="w-full login-control" />
                        </FormField>
                        <form class="login-form-stack" @submit.prevent="onSubmit">
                            <FormField class="login-field" :label="t('web.login.username')" label-for="username">
                                <InputText id="username" v-model="username" required autocomplete="username"
                                    class="w-full login-control" />
                            </FormField>
                            <FormField class="login-field" :label="t('web.login.password')" label-for="password">
                                <Password id="password" v-model="password" required toggleMask :feedback="false"
                                    input-class="w-full" class="w-full login-control"
                                    :input-props="{ autocomplete: 'current-password' }" />
                            </FormField>
                            <div class="login-actions">
                                <Button :label="t('web.login.login')" type="submit" class="w-full login-submit"
                                    :loading="submitting" />
                                <Button v-if="oidcEnabled" :label="t('web.login.sso_login')" type="button"
                                    class="w-full login-sso" severity="secondary" outlined @click="onSsoLogin" />
                            </div>
                        </form>
                    </div>
                </template>
            </Card>
        </div>
    </div>
</template>

<style scoped>
.login-page {
    min-height: 100dvh;
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 1.25rem;
    box-sizing: border-box;
    background:
        radial-gradient(ellipse 70% 55% at 12% 8%, color-mix(in srgb, var(--primary-color, #0ea5e9) 16%, transparent), transparent 58%),
        radial-gradient(ellipse 55% 45% at 88% 92%, color-mix(in srgb, var(--primary-color, #0ea5e9) 10%, transparent), transparent 55%),
        linear-gradient(180deg, #f8fbff 0%, var(--surface-ground, #f6f8fb) 100%);
}

.login-shell {
    width: 100%;
    max-width: 26rem;
}

.login-card {
    border: 1px solid var(--et-border-color, #e2e8f0) !important;
    border-radius: 1rem !important;
    background: var(--surface-card, #ffffff) !important;
    box-shadow:
        0 1px 2px rgba(15, 23, 42, 0.04),
        0 18px 40px rgba(15, 23, 42, 0.08);
    overflow: hidden;
}

.login-brand {
    display: flex;
    align-items: center;
    gap: 0.9rem;
    padding: 1.35rem 1.35rem 0.85rem;
}

.login-logo {
    width: 3rem;
    height: 3rem;
    border-radius: 0.75rem;
    box-shadow: 0 6px 16px color-mix(in srgb, var(--primary-color, #0ea5e9) 22%, transparent);
}

.login-brand-text {
    min-width: 0;
}

.login-brand-name {
    margin: 0;
    color: var(--text-color, #0f172a);
    font-size: 1.55rem;
    font-weight: 750;
    letter-spacing: -0.03em;
    line-height: 1.15;
}

.login-brand-sub {
    margin: 0.2rem 0 0;
    color: var(--text-color-secondary, #64748b);
    font-size: var(--et-fs-body, 0.875rem);
    font-weight: 500;
    line-height: 1.35;
}

.login-form {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    padding: 0 1.35rem 1.5rem;
}

.login-form-stack,
.login-actions {
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
}

.login-field :deep(label),
.login-field :deep(.form-field-label) {
    display: block;
    margin-bottom: 0.4rem;
    color: var(--text-color, #1e293b);
    font-size: 0.8125rem;
    font-weight: 600;
}

:deep(.p-card-body),
:deep(.p-card-content) {
    padding: 0 !important;
}

:deep(.p-card-header) {
    padding: 0 !important;
}

:deep(.login-control.p-inputtext),
:deep(.login-control .p-inputtext),
:deep(.login-control.p-autocomplete),
:deep(.login-control.p-password),
:deep(.login-control .p-password-input) {
    width: 100%;
    min-height: 2.6rem;
}

:deep(.login-control.p-password) {
    display: block;
}

:deep(.login-control.p-password .p-password-input) {
    width: 100%;
}

:deep(.login-submit.p-button) {
    min-height: var(--et-btn-lg, 2.75rem) !important;
    border-radius: var(--et-radius, 0.75rem) !important;
    font-weight: 700 !important;
}

:deep(.login-sso.p-button) {
    min-height: var(--et-btn, 2.5rem) !important;
    border-radius: var(--et-radius, 0.75rem) !important;
    font-weight: 600 !important;
    background: var(--surface-0, #ffffff) !important;
    border: 1px solid var(--et-border-color, #e2e8f0) !important;
    color: var(--text-color, #1e293b) !important;
}

:deep(.login-sso.p-button:hover:not(:disabled)) {
    background: var(--surface-hover, #f1f5f9) !important;
    border-color: color-mix(in srgb, var(--primary-color, #0ea5e9) 35%, var(--et-border-color, #e2e8f0)) !important;
}
</style>
