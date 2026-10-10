<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { Button, Checkbox, InputNumber, useToast } from 'primevue';
import { TOAST_LIFE, Utils } from 'easytier-frontend-lib';
import { useI18n } from 'vue-i18n';
import ApiClient, { RuntimeLogLine } from '../modules/api';

const props = defineProps<{
    api: ApiClient;
}>();

const { t } = useI18n();
const toast = useToast();

const loading = ref(true);
const lines = ref<RuntimeLogLine[]>([]);
const capacity = ref(0);
const tail = ref(500);
const autoRefresh = ref(false);
const loadError = ref<string | undefined>(undefined);
const logBox = ref<HTMLElement | null>(null);
let timer: ReturnType<typeof setInterval> | undefined;
/** Serializes loads so a slow response cannot overwrite a newer one / clear the wrong spinner. */
let loadSeq = 0;
let loadInFlight = false;
let logsAlive = true;

/** 是否已滚到底部（留 32px 容差）。只有本来就在底部才自动跟随，
 *  否则自动刷新会把正在翻历史的用户每 3 秒拽回底部。 */
const isAtBottom = () => {
    const el = logBox.value;
    if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight < 32;
};

const load = async (silent = false) => {
    if (loadInFlight && silent) {
        return;
    }
    const seq = ++loadSeq;
    loadInFlight = true;
    if (!silent) {
        loading.value = true;
    }
    const stickToBottom = isAtBottom();
    try {
        const n = Math.min(1000, Math.max(50, Number(tail.value) || 500));
        tail.value = n;
        const resp = await props.api.get_runtime_logs(n);
        if (!logsAlive || seq !== loadSeq) {
            return;
        }
        capacity.value = resp.capacity;
        lines.value = resp.lines;
        loadError.value = undefined;
        await nextTick();
        if (logBox.value && stickToBottom) {
            logBox.value.scrollTop = logBox.value.scrollHeight;
        }
    } catch (e) {
        if (!logsAlive || seq !== loadSeq) {
            return;
        }
        const detail = Utils.formatApiErrorDetail(e, t);
        // First load / manual refresh: distinguish failure from empty buffer.
        if (!silent || lines.value.length === 0) {
            loadError.value = detail;
        }
        if (!silent) {
            toast.add({
                severity: 'error',
                summary: t('web.runtime_logs.load_failed'),
                detail,
                life: TOAST_LIFE.error,
            });
        }
    } finally {
        if (seq === loadSeq) {
            loadInFlight = false;
            // Only the active request clears the spinner (manual sets it; silent never does).
            if (!silent) {
                loading.value = false;
            }
        }
    }
};

watch(autoRefresh, (on) => {
    if (timer) {
        clearInterval(timer);
        timer = undefined;
    }
    if (on) {
        timer = setInterval(() => {
            void load(true);
        }, 3000);
    }
});

onMounted(() => {
    logsAlive = true;
    void load();
});

onUnmounted(() => {
    logsAlive = false;
    ++loadSeq;
    if (timer) clearInterval(timer);
});
</script>

<template>
    <div class="et-page">
        <div class="et-page-header logs-header">
            <div>
                <h1 class="et-page-title">{{ t('web.main.runtime_logs') }}</h1>
                <p class="logs-hint et-meta">{{ t('web.runtime_logs.hint') }}</p>
            </div>
            <div class="logs-actions">
                <label class="logs-tail et-meta">
                    {{ t('web.runtime_logs.tail') }}
                    <InputNumber v-model="tail" :min="50" :max="1000" :step="50" showButtons />
                </label>
                <label class="logs-auto">
                    <Checkbox v-model="autoRefresh" binary />
                    <span>{{ t('web.runtime_logs.auto_refresh') }}</span>
                </label>
                <Button
                    :label="t('web.runtime_logs.refresh')"
                    icon="pi pi-refresh"
                    severity="secondary"
                    :loading="loading"
                    @click="load()"
                />
            </div>
        </div>

        <div class="logs-meta et-meta">
            {{ t('web.runtime_logs.capacity', { capacity, count: lines.length }) }}
        </div>

        <div ref="logBox" class="logs-box">
            <div v-if="loading && lines.length === 0 && !loadError" class="et-meta py-8 text-center">
                {{ t('web.common.loading') }}
            </div>
            <div v-else-if="loadError && lines.length === 0" class="et-meta py-8 text-center logs-error">
                <p>{{ t('web.runtime_logs.load_failed') }}</p>
                <p class="logs-error-detail">{{ loadError }}</p>
                <Button
                    class="mt-3"
                    :label="t('web.runtime_logs.refresh')"
                    icon="pi pi-refresh"
                    severity="secondary"
                    @click="load()"
                />
            </div>
            <div v-else-if="lines.length === 0" class="et-meta py-8 text-center">
                {{ t('web.runtime_logs.empty') }}
            </div>
            <pre v-else class="logs-pre"><template v-for="(line, _i) in lines" :key="line.ts + '-' + _i"><span class="log-line" :class="'log-level--' + line.level.toLowerCase()">{{ line.ts }} {{ line.level.padEnd(5) }} {{ line.target }}: {{ line.message }}
</span></template></pre>
        </div>
    </div>
</template>

<style scoped>
.logs-header {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
    justify-content: space-between;
    gap: 0.75rem;
}

.logs-hint {
    margin: 0.35rem 0 0;
    max-width: 40rem;
}

.logs-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
}

.logs-tail {
    display: flex;
    align-items: center;
    gap: 0.4rem;
}

.logs-auto {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.9rem;
}

.logs-meta {
    margin-bottom: 0.5rem;
}

.logs-error-detail {
    margin-top: 0.5rem;
    opacity: 0.85;
}

.logs-box {
    max-height: calc(100dvh - 12rem);
    overflow: auto;
    border: var(--et-border);
    border-radius: var(--et-radius);
    background: #0f172a;
    color: #e2e8f0;
}

.logs-pre {
    margin: 0;
    padding: 0.85rem 1rem;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 0.78rem;
    line-height: 1.45;
    white-space: pre-wrap;
    word-break: break-word;
}

.log-level--error {
    color: #fca5a5;
}

.log-level--warn {
    color: #fcd34d;
}

.log-level--info {
    color: #bbf7d0;
}

.log-level--debug,
.log-level--trace {
    color: #94a3b8;
}
</style>
