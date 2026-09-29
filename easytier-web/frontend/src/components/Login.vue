<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { Card, InputText, Password, Button, AutoComplete } from 'primevue';
import { useRouter } from 'vue-router';
import { useToast } from 'primevue/usetoast';
import { I18nUtils } from 'easytier-frontend-lib';
import { getInitialApiHost, cleanAndLoadApiHosts, saveApiHost } from "../modules/api-host"
import { useI18n } from 'vue-i18n'
import ApiClient, { Credential } from '../modules/api';

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

        const enabled = (await new ApiClient(host).getOidcConfig()).enabled;
        if (apiHost.value !== host) return;

        lastCheckedHost.value = host;
        oidcEnabled.value = enabled;
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
    <div class="flex items-center justify-center min-h-screen">
        <Card class="w-full max-w-md p-6">
            <template #header>
                <h2 class="text-2xl font-semibold text-center">{{ t('web.login.login') }}</h2>
            </template>
            <template #content>
                <div class="p-field mb-4">
                    <label for="api-host" class="block text-sm font-medium">{{ t('web.login.api_host') }}</label>
                    <AutoComplete id="api-host" v-model="apiHost" dropdown :suggestions="apiHostSuggestions"
                        @complete="apiHostSearch" class="w-full" />
                </div>
                <form @submit.prevent="onSubmit" class="space-y-4">
                    <div class="p-field">
                        <label for="username" class="block text-sm font-medium">{{ t('web.login.username') }}</label>
                        <InputText id="username" v-model="username" required class="w-full" />
                    </div>
                    <div class="p-field">
                        <label for="password" class="block text-sm font-medium">{{ t('web.login.password') }}</label>
                        <Password id="password" v-model="password" required toggleMask :feedback="false" />
                    </div>
                    <Button :label="t('web.login.login')" type="submit" class="w-full" />
                    <Button v-if="oidcEnabled" :label="t('web.login.sso_login')" type="button" class="w-full" severity="info"
                        @click="onSsoLogin" />
                </form>

                <Button icon="pi pi-language" type="button" class="rounded-full absolute top-4 right-4 z-10"
                    style="box-shadow: 0 2px 8px rgba(0,0,0,0.08);" severity="contrast"
                    @click="I18nUtils.toggleLanguage" :aria-label="t('web.main.language')"
                    :v-tooltip="t('web.main.language')" />
            </template>
        </Card>
    </div>
</template>

<style scoped></style>
