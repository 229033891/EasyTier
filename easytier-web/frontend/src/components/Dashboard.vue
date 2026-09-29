<script setup lang="ts">
import { Card, useToast } from 'primevue';
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { Utils } from 'easytier-frontend-lib';
import { useI18n } from 'vue-i18n';
import ApiClient, { Summary } from '../modules/api';

const props = defineProps({
    api: ApiClient,
});

const { t } = useI18n();
const toast = useToast();

const summary = ref<Summary | undefined>(undefined);

const loadSummary = async () => {
    const resp = await props.api?.get_summary();
    summary.value = resp;
};

const periodFunc = new Utils.PeriodicTask(async () => {
    try {
        await loadSummary();
    } catch (e) {
        toast.add({ severity: 'error', summary: t('web.dashboard.load_failed'), detail: String(e), life: 2000 });
        console.error(e);
    }
}, 1000);

onMounted(async () => {
    periodFunc.start();
});

onUnmounted(() => {
    periodFunc.stop();
});

const deviceCount = computed<number | undefined>(() => summary.value?.device_count);
const networkCount = computed<number | undefined>(() => summary.value?.network_count);

</script>

<template>
    <div class="et-page">
        <h1 class="et-page-title">{{ t('web.main.dashboard') }}</h1>

        <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
            <Card class="h-full summary-card">
                <template #title>
                    <span class="et-section-title">{{ t('web.main.device_count') }}</span>
                </template>
                <template #content>
                    <div class="summary-value">
                        {{ deviceCount ?? '—' }}
                    </div>
                </template>
            </Card>
            <Card class="h-full summary-card">
                <template #title>
                    <span class="et-section-title">{{ t('web.main.network_count') }}</span>
                </template>
                <template #content>
                    <div class="summary-value">
                        {{ networkCount ?? '—' }}
                    </div>
                </template>
            </Card>
        </div>
    </div>
</template>

<style scoped>
.summary-card {
    min-height: 8.5rem;
    border: var(--et-border);
    border-radius: var(--et-radius);
}

.summary-value {
    width: 100%;
    display: flex;
    justify-content: center;
    margin-top: var(--et-space-3);
    font-size: 3rem;
    line-height: 1;
    font-weight: 700;
    color: var(--primary-color, #0ea5e9);
}
</style>
