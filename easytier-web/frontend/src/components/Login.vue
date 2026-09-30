<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { Card, InputText, Password, Button, AutoComplete } from 'primevue';
import { useRouter } from 'vue-router';
import { useToast } from 'primevue/usetoast';
import { I18nUtils } from 'easytier-frontend-lib';
import { getInitialApiHost, cleanAndLoadApiHosts, saveApiHost } from "../modules/api-host"
import { useI18n } from 'vue-i18n'
import ApiClient, { Credential } from '../modules/api';
import FormField from './FormField.vue';
import { tooltipDirective } from '../modules/tooltip';
import Icon from '../assets/easytier.png';

const vTooltip = tooltipDirective;
const { t } = useI18n()

const api = computed<ApiClient>(() => new ApiClient(apiHost.value));
const router = useRouter();
const toast = useToast();

const username = ref('');
const password = ref('');

const onSubmit = async () => {
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
    <div class="login-page flex items-center justify-center min-h-screen p-4">
        <Card class="login-card w-full max-w-md p-6">
            <template #header>
                <div class="login-brand">
                    <img :src="Icon" :alt="t('web.main.logo_alt')" />
                    <div>
                        <div class="login-brand-name">EasyTier</div>
                        <h2 class="login-title">{{ t('web.login.login') }}</h2>
                    </div>
                </div>
            </template>
            <template #content>
                <FormField class="mb-4" :label="t('web.login.api_host')" label-for="api-host"
                    label-class="block text-sm font-medium">
                    <AutoComplete id="api-host" v-model="apiHost" dropdown :suggestions="apiHostSuggestions"
                        @complete="apiHostSearch" class="w-full" />
                </FormField>
                <form @submit.prevent="onSubmit" class="space-y-4">
                    <FormField :label="t('web.login.username')" label-for="username"
                        label-class="block text-sm font-medium">
                        <InputText id="username" v-model="username" required class="w-full" />
                    </FormField>
                    <FormField :label="t('web.login.password')" label-for="password"
                        label-class="block text-sm font-medium">
                        <Password id="password" v-model="password" required toggleMask :feedback="false" />
                    </FormField>
                    <Button :label="t('web.login.login')" type="submit" class="w-full" />
                    <Button v-if="oidcEnabled" :label="t('web.login.sso_login')" type="button" class="w-full" severity="info"
                        @click="onSsoLogin" />
                </form>

                <Button icon="pi pi-language" type="button" class="rounded-full absolute top-4 right-4 z-10"
                    severity="contrast" @click="I18nUtils.toggleLanguage"
                    :aria-label="t('web.main.language')"
                    v-tooltip.bottom="t('web.main.language')" />
            </template>
        </Card>
    </div>
</template>

<style scoped>
.login-page {
    background:
        radial-gradient(circle at 15% 10%, color-mix(in srgb, var(--primary-color, #0ea5e9) 11%, transparent), transparent 32%),
        var(--surface-ground, #f6f8fb);
}

.login-card {
    position: relative;
    border: var(--et-border);
    border-radius: 1rem;
    box-shadow: 0 18px 50px rgba(15, 23, 42, 0.1);
}

.login-brand {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.25rem 0 1.25rem;
}

.login-brand img {
    width: 2.75rem;
    height: 2.75rem;
}

.login-brand-name {
    color: var(--text-color-secondary, #64748b);
    font-size: var(--et-fs-meta);
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
}

.login-title {
    margin: 0.1rem 0 0;
    color: var(--text-color, #1e293b);
    font-size: 1.5rem;
    font-weight: 700;
    line-height: 1.25;
}

:deep(.p-card-content) {
    padding-top: 0.25rem;
}

:deep(.p-button.w-full) {
    min-height: var(--et-btn);
    font-weight: 600;
}
</style>
