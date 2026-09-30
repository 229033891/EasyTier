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
        <div class="et-page-header">
            <h1 class="et-page-title">{{ t('web.main.dashboard') }}</h1>
        </div>

        <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
            <Card class="h-full summary-card">
                <template #title>
                    <div class="summary-heading">
                        <span class="summary-icon summary-icon--device"><i class="pi pi-server" aria-hidden="true"></i></span>
                        <span class="et-section-title">{{ t('web.main.device_count') }}</span>
                    </div>
                </template>
                <template #content>
                    <div class="summary-value">
                        {{ deviceCount ?? '—' }}
                    </div>
                </template>
            </Card>
            <Card class="h-full summary-card">
                <template #title>
                    <div class="summary-heading">
                        <span class="summary-icon summary-icon--network"><i class="pi pi-sitemap" aria-hidden="true"></i></span>
                        <span class="et-section-title">{{ t('web.main.network_count') }}</span>
                    </div>
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
    overflow: hidden;
    border: var(--et-border);
    border-radius: var(--et-radius);
    box-shadow: var(--et-shadow-card);
    transition: border-color 0.18s ease, box-shadow 0.18s ease, transform 0.18s ease;
}

@media (hover: hover) {
    .summary-card:hover {
        border-color: color-mix(in srgb, var(--primary-color, #0ea5e9) 35%, var(--surface-border, #e2e8f0));
        box-shadow: 0 10px 28px rgba(15, 23, 42, 0.08);
        transform: translateY(-1px);
    }
}

.summary-heading {
    display: flex;
    align-items: center;
    gap: 0.625rem;
}

.summary-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 2rem;
    height: 2rem;
    border-radius: 0.625rem;
}

.summary-icon--device {
    color: var(--primary-color, #0284c7);
    background: color-mix(in srgb, var(--primary-color, #0ea5e9) 12%, transparent);
}

.summary-icon--network {
    color: var(--p-violet-600, #7c3aed);
    background: color-mix(in srgb, #8b5cf6 12%, transparent);
}

.summary-value {
    width: 100%;
    display: flex;
    justify-content: flex-end;
    padding-right: 0.25rem;
    margin-top: 1.25rem;
    font-size: 3rem;
    line-height: 1;
    font-weight: 750;
    letter-spacing: -0.04em;
    color: var(--text-color, #1e293b);
}
</style>
