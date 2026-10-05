import { onMounted, onUnmounted, ref, shallowRef } from 'vue';
import { TOAST_LIFE, Utils } from 'easytier-frontend-lib';
import { useToast } from 'primevue';
import { useI18n } from 'vue-i18n';

export interface PollingListOptions<T> {
    /** 拉取数据的函数，返回值即列表内容 */
    fetcher: () => Promise<T | undefined>;
    /** 轮询间隔（毫秒），默认 1000 */
    interval?: number;
    /** 失败时 toast 的标题文案 key，默认 web.device.load_list_failed */
    errorSummaryKey?: string;
}

const ERROR_TOAST_INTERVAL_MS = 30_000;

/**
 * 列表页通用轮询：负责 PeriodicTask 生命周期 + 失败 toast + loading 判定。
 *
 * `loading` 只覆盖尚无快照的首屏；手动 reload 用 `reloading`（重试按钮忙态）。
 * 后台轮询失败写入 `error` 并保留上次成功的 data。
 */
export function usePollingList<T>(options: PollingListOptions<T>) {
    const { fetcher, interval = 1000, errorSummaryKey = 'web.device.load_list_failed' } = options;
    const { t } = useI18n();
    const toast = useToast();

    // 列表页面只替换整批结果，不需要为每个设备/网络建立深层 Proxy。
    // shallowRef 保持 data.value 的替换响应式，同时避免每秒递归代理整个列表。
    const data = shallowRef<T | undefined>(undefined);
    const loading = ref(true);
    const reloading = ref(false);
    const error = ref<unknown>(null);
    let lastErrorToastAt = 0;
    let reloadInFlight: Promise<void> | undefined;

    const maybeToast = (e: unknown) => {
        const now = Date.now();
        if (now - lastErrorToastAt < ERROR_TOAST_INTERVAL_MS) {
            return;
        }
        lastErrorToastAt = now;
        toast.add({
            severity: 'error',
            summary: t(errorSummaryKey),
            detail: Utils.formatApiErrorDetail(e, t),
            life: TOAST_LIFE.error,
        });
    };

    const fetchOnce = async (showLoading: boolean) => {
        if (showLoading && reloadInFlight) {
            return reloadInFlight;
        }
        if (showLoading && data.value === undefined) {
            loading.value = true;
        }
        if (showLoading) {
            reloading.value = true;
        }
        const run = (async () => {
            try {
                data.value = await fetcher();
                error.value = null;
            } catch (e) {
                error.value = e;
                maybeToast(e);
                throw e;
            } finally {
                loading.value = false;
                if (showLoading) {
                    reloading.value = false;
                    reloadInFlight = undefined;
                }
            }
        })();
        if (showLoading) {
            reloadInFlight = run;
        }
        return run;
    };

    const periodFunc = new Utils.PeriodicTask(async () => {
        // 手动 reload 进行中时跳过后台轮询，避免并发写 data / 提前清掉 loading
        if (reloading.value || reloadInFlight) {
            return;
        }
        try {
            await fetchOnce(false);
        } catch (e) {
            console.error(e);
        }
    }, interval);

    onMounted(() => {
        periodFunc.start();
    });

    onUnmounted(() => {
        periodFunc.stop();
    });

    return {
        data,
        loading,
        reloading,
        error,
        reload: () => fetchOnce(true),
    };
}
