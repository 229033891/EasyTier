<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { Button, useToast } from 'primevue';
import { TOAST_LIFE, Utils } from 'easytier-frontend-lib';
import { useI18n } from 'vue-i18n';
import ApiClient, { SystemDiagnosticsResponse, DiagnosticCheck } from '../modules/api';

const props = defineProps<{
    api: ApiClient;
}>();

const { t, locale } = useI18n();
const toast = useToast();

const loading = ref(true);
const data = ref<SystemDiagnosticsResponse | undefined>(undefined);
const loadError = ref<string | undefined>(undefined);
const snapshotOpen = ref(false);

const listSep = computed(() => (String(locale.value).toLowerCase().startsWith('zh') ? '、' : ', '));
const detailParen = (inner: string) =>
    String(locale.value).toLowerCase().startsWith('zh') ? `（${inner}）` : ` (${inner})`;

const load = async () => {
    loading.value = true;
    try {
        data.value = await props.api.get_system_diagnostics();
        loadError.value = undefined;
    } catch (e) {
        data.value = undefined;
        loadError.value = Utils.formatApiErrorDetail(e, t);
        toast.add({
            severity: 'error',
            summary: t('web.system_diagnostics.load_failed'),
            detail: loadError.value,
            life: TOAST_LIFE.error,
        });
    } finally {
        loading.value = false;
    }
};

const overallClass = computed(() => {
    switch (data.value?.overall) {
        case 'healthy':
            return 'diag-overall--healthy';
        case 'degraded':
            return 'diag-overall--degraded';
        case 'unhealthy':
            return 'diag-overall--unhealthy';
        default:
            return '';
    }
});

const overallLabel = computed(() => {
    switch (data.value?.overall) {
        case 'healthy':
            return t('web.system_diagnostics.overall_healthy');
        case 'degraded':
            return t('web.system_diagnostics.overall_degraded');
        case 'unhealthy':
            return t('web.system_diagnostics.overall_unhealthy');
        default:
            return data.value?.overall ?? '—';
    }
});

const statusLabel = (status: string) => {
    if (status === 'pass') return t('web.system_diagnostics.status_pass');
    if (status === 'fail') return t('web.system_diagnostics.status_fail');
    if (status === 'warn') return t('web.system_diagnostics.status_warn');
    return status;
};

/** 检查项标题：按稳定 id 查本地化；缺失时回落到 id，绝不显示后端语言。 */
const checkTitle = (c: DiagnosticCheck) => {
    const key = `web.system_diagnostics.checks.${c.id}`;
    const label = t(key);
    return label === key ? c.id : label;
};

const displayValue = (raw: string) => {
    if (!raw || raw === 'none') {
        return t('web.system_diagnostics.value_none');
    }
    return raw;
};

/** 说明：稳定 code + 可选 param。 */
const checkDetail = (c: DiagnosticCheck) => {
    const d = c.detail;
    if (!d) return '—';
    const key = `web.system_diagnostics.details.${d.code}`;
    const label = t(key);
    const text = label === key ? d.code : label;
    return d.param ? `${text}${detailParen(d.param)}` : text;
};

/** 总览摘要：code + 失败项 id 列表。 */
const overallSummary = computed(() => {
    const s = data.value?.summary;
    if (!s) return '';
    const key = `web.system_diagnostics.overall_summary.${s.code}`;
    const label = t(key);
    const text = label === key ? '' : label;
    if (s.params.length === 0) return text;
    const names = s.params.map((id) => checkTitle({ id } as DiagnosticCheck)).join(listSep.value);
    const colon = String(locale.value).toLowerCase().startsWith('zh') ? '：' : ': ';
    return text ? `${text}${colon}${names}` : names;
});

const copySnapshot = async () => {
    if (!data.value) return;
    const text = JSON.stringify(data.value.snapshot, null, 2);
    try {
        await navigator.clipboard.writeText(text);
        toast.add({
            severity: 'success',
            summary: t('web.system_diagnostics.copied'),
            life: TOAST_LIFE.success,
        });
    } catch {
        toast.add({
            severity: 'error',
            summary: t('web.system_diagnostics.copy_failed'),
            life: TOAST_LIFE.error,
        });
    }
};

onMounted(load);
</script>

<template>
    <div class="et-page">
        <div class="et-page-header diag-header">
            <div>
                <h1 class="et-page-title">{{ t('web.main.system_diagnostics') }}</h1>
                <p class="diag-hint et-meta">{{ t('web.system_diagnostics.hint') }}</p>
            </div>
            <div class="diag-actions">
                <Button
                    :label="t('web.system_diagnostics.refresh')"
                    icon="pi pi-refresh"
                    severity="secondary"
                    :loading="loading"
                    @click="load"
                />
                <Button
                    :label="t('web.system_diagnostics.copy_snapshot')"
                    icon="pi pi-copy"
                    severity="secondary"
                    :disabled="!data"
                    @click="copySnapshot"
                />
            </div>
        </div>

        <div v-if="loading && !data" class="et-meta py-8 text-center">
            {{ t('web.common.loading') }}
        </div>

        <div v-else-if="loadError && !data" class="diag-error et-meta py-8 text-center">
            <p>{{ t('web.system_diagnostics.load_failed') }}</p>
            <p class="diag-error-detail">{{ loadError }}</p>
            <Button
                class="mt-3"
                :label="t('web.system_diagnostics.refresh')"
                icon="pi pi-refresh"
                severity="secondary"
                @click="load"
            />
        </div>

        <template v-else-if="data">
            <div class="diag-overall" :class="overallClass">
                <div class="diag-overall-status">{{ overallLabel }}</div>
                <div class="diag-overall-summary">{{ overallSummary }}</div>
                <div class="et-meta diag-generated">
                    {{ t('web.system_diagnostics.generated_at') }}: {{ data.generated_at }}
                </div>
            </div>

            <div class="diag-checks et-list-table overflow-x-auto">
                <table class="w-full">
                    <thead>
                        <tr>
                            <th>{{ t('web.system_diagnostics.col_check') }}</th>
                            <th>{{ t('web.system_diagnostics.col_status') }}</th>
                            <th>{{ t('web.system_diagnostics.col_expected') }}</th>
                            <th>{{ t('web.system_diagnostics.col_actual') }}</th>
                            <th>{{ t('web.system_diagnostics.col_detail') }}</th>
                        </tr>
                    </thead>
                    <tbody>
                        <tr v-for="c in data.checks" :key="c.id">
                            <td>{{ checkTitle(c) }}</td>
                            <td>
                                <span
                                    class="diag-badge"
                                    :class="{
                                        'diag-badge--pass': c.status === 'pass',
                                        'diag-badge--fail': c.status === 'fail',
                                        'diag-badge--warn': c.status === 'warn',
                                    }"
                                >{{ statusLabel(c.status) }}</span>
                            </td>
                            <td>{{ displayValue(c.expected) }}</td>
                            <td>{{ displayValue(c.actual) }}</td>
                            <td>{{ checkDetail(c) }}</td>
                        </tr>
                    </tbody>
                </table>
            </div>

            <div class="diag-snapshot">
                <button type="button" class="diag-snapshot-toggle et-meta" @click="snapshotOpen = !snapshotOpen">
                    {{ t('web.system_diagnostics.snapshot') }}
                    <i :class="snapshotOpen ? 'pi pi-chevron-up' : 'pi pi-chevron-down'" />
                </button>
                <pre v-if="snapshotOpen" class="diag-snapshot-pre">{{ JSON.stringify(data.snapshot, null, 2) }}</pre>
            </div>
        </template>
    </div>
</template>

<style scoped>
.diag-header {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
    justify-content: space-between;
    gap: 0.75rem;
}

.diag-hint {
    margin: 0.35rem 0 0;
    max-width: 40rem;
}

.diag-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
}

.diag-error-detail {
    margin-top: 0.5rem;
    opacity: 0.85;
}

.diag-overall {
    margin: 1rem 0;
    padding: 1rem 1.15rem;
    border-radius: var(--et-radius);
    border: var(--et-border);
}

.diag-overall--healthy {
    background: color-mix(in srgb, #16a34a 12%, transparent);
}

.diag-overall--degraded {
    background: color-mix(in srgb, #ca8a04 12%, transparent);
}

.diag-overall--unhealthy {
    background: color-mix(in srgb, #dc2626 12%, transparent);
}

.diag-overall-status {
    font-weight: 600;
    font-size: 1.1rem;
}

.diag-overall-summary {
    margin-top: 0.35rem;
}

.diag-generated {
    margin-top: 0.5rem;
}

.diag-badge {
    display: inline-block;
    padding: 0.1rem 0.45rem;
    border-radius: 0.25rem;
    font-size: 0.85rem;
}

.diag-badge--pass {
    background: color-mix(in srgb, #16a34a 18%, transparent);
}

.diag-badge--fail {
    background: color-mix(in srgb, #dc2626 18%, transparent);
}

.diag-badge--warn {
    background: color-mix(in srgb, #ca8a04 18%, transparent);
}

.diag-snapshot {
    margin-top: 1.25rem;
}

.diag-snapshot-toggle {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    background: none;
    border: none;
    cursor: pointer;
    padding: 0;
}

.diag-snapshot-pre {
    margin-top: 0.5rem;
    padding: 0.85rem 1rem;
    overflow: auto;
    max-height: 24rem;
    border: var(--et-border);
    border-radius: var(--et-radius);
    font-size: 0.78rem;
}
</style>
