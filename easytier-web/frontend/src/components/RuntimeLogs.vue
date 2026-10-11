<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { Button, Checkbox, InputNumber, InputText, Select, useToast } from 'primevue';
import { TOAST_LIFE, Utils } from 'easytier-frontend-lib';
import { useI18n } from 'vue-i18n';
import ApiClient, {
    LogFileInfo,
    LogFilesResponse,
    RuntimeLogLine,
} from '../modules/api';

const props = defineProps<{
    api: ApiClient;
}>();

const { t } = useI18n();
const toast = useToast();

type SourceMode = 'realtime' | 'file';

const loading = ref(true);
const lines = ref<RuntimeLogLine[]>([]);
const capacity = ref(0);
const matched = ref(0);
const totalScanned = ref(0);
const truncatedBytes = ref(false);
const tail = ref(500);
const autoRefresh = ref(false);
const loadError = ref<string | undefined>(undefined);
const logBox = ref<HTMLElement | null>(null);
const source = ref<SourceMode>('realtime');
const filesMeta = ref<LogFilesResponse | null>(null);
/** Set when `/admin/logs/files` fails — distinct from "file logging disabled". */
const filesListError = ref<string | undefined>(undefined);
const selectedFile = ref<string>('easytier.log');
const minLevel = ref<string | null>(null);
const sinceLocal = ref<string>('');
const untilLocal = ref<string>('');
const grep = ref('');

let timer: ReturnType<typeof setInterval> | undefined;
/** Serializes loads so a slow response cannot overwrite a newer one / clear the wrong spinner. */
let loadSeq = 0;
let loadInFlight = false;
let logsAlive = true;

const sourceOptions = computed(() => [
    { label: t('web.runtime_logs.source_realtime'), value: 'realtime' as SourceMode },
    { label: t('web.runtime_logs.source_file'), value: 'file' as SourceMode },
]);

const levelOptions = computed(() => [
    { label: t('web.runtime_logs.level_any'), value: null },
    { label: 'ERROR', value: 'error' },
    { label: 'WARN', value: 'warn' },
    { label: 'INFO', value: 'info' },
    { label: 'DEBUG', value: 'debug' },
    { label: 'TRACE', value: 'trace' },
]);

const fileOptions = computed(() => {
    const files = filesMeta.value?.files ?? [];
    return files.map((f: LogFileInfo) => ({
        label: f.active ? `${f.file_name} *` : f.file_name,
        value: f.file_name,
    }));
});

const filesEnabled = computed(() => !!filesMeta.value?.enabled);

/** 是否已滚到底部（留 32px 容差）。只有本来就在底部才自动跟随，
 *  否则自动刷新会把正在翻历史的用户每 3 秒拽回底部。 */
const isAtBottom = () => {
    const el = logBox.value;
    if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight < 32;
};

/** Convert `datetime-local` value to UTC ISO, or throw if non-empty but invalid.
 *  `until` is widened to the end of that minute: the picker has minute
 *  granularity, so "to 02:03" should not silently drop 02:03:59 lines. */
const localToUtcIso = (local: string, endOfMinute = false): string | undefined => {
    if (!local) return undefined;
    const d = new Date(local);
    if (Number.isNaN(d.getTime())) {
        throw new Error('invalid_datetime');
    }
    if (endOfMinute) {
        d.setSeconds(59, 999);
    }
    return d.toISOString();
};

const refreshFileList = async () => {
    try {
        const resp = await props.api.list_log_files();
        if (!logsAlive) return;
        filesMeta.value = resp;
        filesListError.value = undefined;
        if (resp.enabled && resp.files.length > 0) {
            const stillThere = resp.files.some((f) => f.file_name === selectedFile.value);
            if (!stillThere) {
                const active = resp.files.find((f) => f.active);
                selectedFile.value = active?.file_name ?? resp.files[0].file_name;
            }
        }
        if (!resp.enabled && source.value === 'file') {
            source.value = 'realtime';
        }
    } catch (e) {
        if (!logsAlive) return;
        filesListError.value = Utils.formatApiErrorDetail(e, t);
        // Do not leave stale "enabled" meta after a failed refresh.
        filesMeta.value = null;
        if (source.value === 'file') {
            loadError.value = filesListError.value;
        }
    }
};

const refresh = async () => {
    if (source.value === 'file') {
        await refreshFileList();
    }
    await load();
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
        const n =
            source.value === 'realtime'
                ? Math.min(1000, Math.max(50, Number(tail.value) || 500))
                : Math.min(2000, Math.max(100, Number(tail.value) || 500));
        tail.value = n;

        if (source.value === 'realtime') {
            const resp = await props.api.get_runtime_logs(n);
            if (!logsAlive || seq !== loadSeq) {
                return;
            }
            capacity.value = resp.capacity;
            lines.value = resp.lines;
            matched.value = resp.lines.length;
            totalScanned.value = resp.lines.length;
            truncatedBytes.value = false;
        } else {
            if (!filesEnabled.value) {
                lines.value = [];
                matched.value = 0;
                totalScanned.value = 0;
                truncatedBytes.value = false;
                loadError.value = undefined;
                return;
            }
            let since: string | undefined;
            let until: string | undefined;
            try {
                since = localToUtcIso(sinceLocal.value);
                until = localToUtcIso(untilLocal.value, true);
            } catch {
                if (!logsAlive || seq !== loadSeq) {
                    return;
                }
                loadError.value = t('web.runtime_logs.invalid_datetime');
                if (!silent) {
                    toast.add({
                        severity: 'warn',
                        summary: t('web.runtime_logs.load_failed'),
                        detail: t('web.runtime_logs.invalid_datetime'),
                        life: TOAST_LIFE.warn,
                    });
                }
                return;
            }
            const resp = await props.api.read_log_file({
                file: selectedFile.value || 'easytier.log',
                tail: n,
                min_level: minLevel.value ?? undefined,
                since,
                until,
                grep: grep.value.trim() || undefined,
            });
            if (!logsAlive || seq !== loadSeq) {
                return;
            }
            capacity.value = 0;
            lines.value = resp.lines;
            matched.value = resp.matched;
            totalScanned.value = resp.total_scanned;
            truncatedBytes.value = resp.truncated_bytes;
        }
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
    if (on && source.value === 'realtime') {
        timer = setInterval(() => {
            void load(true);
        }, 3000);
    }
});

watch(source, async (mode) => {
    if (mode !== 'realtime' && autoRefresh.value) {
        autoRefresh.value = false;
    }
    // Ensure inventory is loaded before file-mode early-return on !filesEnabled.
    if (mode === 'file' && filesMeta.value === null && !filesListError.value) {
        await refreshFileList();
    }
    void load();
});

watch([selectedFile, minLevel], () => {
    if (source.value === 'file') {
        void load();
    }
});

onMounted(async () => {
    logsAlive = true;
    await refreshFileList();
    // If the user switched to file while the list was in flight, reload now that
    // filesEnabled reflects the server truth.
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
                <label class="logs-field et-meta">
                    {{ t('web.runtime_logs.source') }}
                    <Select
                        v-model="source"
                        :options="sourceOptions"
                        optionLabel="label"
                        optionValue="value"
                        class="logs-select"
                    />
                </label>
                <label v-if="source === 'file'" class="logs-field et-meta">
                    {{ t('web.runtime_logs.file') }}
                    <Select
                        v-model="selectedFile"
                        :options="fileOptions"
                        optionLabel="label"
                        optionValue="value"
                        class="logs-select"
                        :disabled="!filesEnabled || fileOptions.length === 0"
                    />
                </label>
                <label v-if="source === 'file'" class="logs-field et-meta">
                    {{ t('web.runtime_logs.level') }}
                    <Select
                        v-model="minLevel"
                        :options="levelOptions"
                        optionLabel="label"
                        optionValue="value"
                        class="logs-select"
                        showClear
                    />
                </label>
                <label class="logs-tail et-meta">
                    {{ t('web.runtime_logs.tail') }}
                    <InputNumber
                        v-model="tail"
                        :min="source === 'file' ? 100 : 50"
                        :max="source === 'file' ? 2000 : 1000"
                        :step="50"
                        showButtons
                    />
                </label>
                <label v-if="source === 'realtime'" class="logs-auto">
                    <Checkbox v-model="autoRefresh" binary />
                    <span>{{ t('web.runtime_logs.auto_refresh') }}</span>
                </label>
                <Button
                    :label="t('web.runtime_logs.refresh')"
                    icon="pi pi-refresh"
                    severity="secondary"
                    :loading="loading"
                    @click="refresh()"
                />
            </div>
        </div>

        <div
            v-if="source === 'file' && filesListError"
            class="logs-banner et-meta"
        >
            <p>{{ t('web.runtime_logs.files_list_failed') }}</p>
            <p>{{ filesListError }}</p>
        </div>
        <div
            v-else-if="source === 'file' && !filesEnabled"
            class="logs-banner et-meta"
        >
            <p>{{ t('web.runtime_logs.files_not_enabled') }}</p>
            <p>{{ t('web.runtime_logs.files_not_enabled_hint') }}</p>
        </div>

        <div v-if="source === 'file' && filesEnabled" class="logs-filters">
            <label class="logs-field et-meta">
                {{ t('web.runtime_logs.time_from') }}
                <input v-model="sinceLocal" type="datetime-local" class="logs-datetime" />
            </label>
            <label class="logs-field et-meta">
                {{ t('web.runtime_logs.time_to') }}
                <input v-model="untilLocal" type="datetime-local" class="logs-datetime" />
            </label>
            <label class="logs-field et-meta logs-grep">
                {{ t('web.runtime_logs.keyword') }}
                <InputText v-model="grep" class="w-full" @keyup.enter="load()" />
            </label>
            <Button
                :label="t('web.runtime_logs.apply_filters')"
                severity="secondary"
                outlined
                @click="load()"
            />
        </div>

        <div class="logs-meta et-meta">
            <template v-if="source === 'realtime'">
                {{ t('web.runtime_logs.capacity', { capacity, count: lines.length }) }}
            </template>
            <template v-else-if="filesEnabled">
                {{
                    t('web.runtime_logs.matched_summary', {
                        matched,
                        scanned: totalScanned,
                        level: filesMeta?.level || '-',
                        dir: filesMeta?.dir || '-',
                    })
                }}
                <span v-if="truncatedBytes"> · {{ t('web.runtime_logs.truncated_bytes') }}</span>
            </template>
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
                {{
                    source === 'file' && (minLevel || sinceLocal || untilLocal || grep)
                        ? t('web.runtime_logs.no_match')
                        : t('web.runtime_logs.empty')
                }}
            </div>
            <pre v-else class="logs-pre"><template v-for="(line, _i) in lines" :key="(line.ts || 'u') + '-' + _i"><span class="log-line" :class="'log-level--' + line.level.toLowerCase()">{{ line.ts ? line.ts + ' ' : '' }}{{ line.level.padEnd(5) }}{{ line.target ? ' ' + line.target + ':' : '' }} {{ line.message }}
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

.logs-actions,
.logs-filters {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
}

.logs-filters {
    margin-bottom: 0.65rem;
}

.logs-field,
.logs-tail {
    display: flex;
    align-items: center;
    gap: 0.4rem;
}

.logs-select {
    min-width: 8.5rem;
}

.logs-datetime {
    border: var(--et-border);
    border-radius: var(--et-radius);
    padding: 0.35rem 0.5rem;
    background: var(--p-surface-0, #fff);
    color: inherit;
}

.logs-grep {
    min-width: 12rem;
    flex: 1 1 12rem;
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

.logs-banner {
    margin-bottom: 0.75rem;
    padding: 0.75rem 1rem;
    border: var(--et-border);
    border-radius: var(--et-radius);
    background: color-mix(in srgb, var(--p-orange-100, #ffedd5) 70%, transparent);
}

.logs-banner p {
    margin: 0.15rem 0;
}

.logs-error-detail {
    margin-top: 0.5rem;
    opacity: 0.85;
}

.logs-box {
    max-height: calc(100dvh - 14rem);
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
.log-level--trace,
.log-level--unknown {
    color: #94a3b8;
}
</style>
