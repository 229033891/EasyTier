import { onMounted, onUnmounted, ref, type Ref } from 'vue';
import { useToast } from 'primevue';
import { Utils } from 'easytier-frontend-lib';
import { useI18n } from 'vue-i18n';

export interface PollingListOptions<T> {
    /** 拉取数据的函数，返回值即列表内容 */
    fetcher: () => Promise<T | undefined>;
    /** 轮询间隔（毫秒），默认 1000 */
    interval?: number;
    /** 失败时 toast 的标题文案 key，默认 web.device.load_list_failed */
    errorSummaryKey?: string;
}

/**
 * 列表页通用轮询：负责 PeriodicTask 生命周期 + 失败 toast + loading 判定。
 *
 * `data` 为 undefined 表示尚未加载完成（用于显示 ProgressSpinner）。
 */
export function usePollingList<T>(options: PollingListOptions<T>) {
    const { fetcher, interval = 1000, errorSummaryKey = 'web.device.load_list_failed' } = options;
    const { t } = useI18n();
    const toast = useToast();

    const data = ref<T | undefined>(undefined) as Ref<T | undefined>;
    const loading = ref(true);

    const load = async () => {
        data.value = await fetcher();
        loading.value = false;
    };

    const periodFunc = new Utils.PeriodicTask(async () => {
        try {
            await load();
        } catch (e) {
            toast.add({ severity: 'error', summary: t(errorSummaryKey), detail: String(e), life: 2000 });
            console.error(e);
        }
    }, interval);

    onMounted(() => {
        periodFunc.start();
    });

    onUnmounted(() => {
        periodFunc.stop();
    });

    return { data, loading, reload: load };
}
