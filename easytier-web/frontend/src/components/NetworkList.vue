<script setup lang="ts">
import { computed, ref } from 'vue';
import { Button, ProgressSpinner } from 'primevue';
import { Utils, tooltipDirective, NetworkTypes } from 'easytier-frontend-lib';
import { useRouter } from 'vue-router';
import { useI18n } from 'vue-i18n';
import ApiClient from '../modules/api';
import { usePollingList } from '../modules/usePollingList';
import ListPageShell from './ListPageShell.vue';

const vTooltip = tooltipDirective;
const { t } = useI18n();
const router = useRouter();

const props = defineProps({
    api: ApiClient,
});

/** 虚拟 IP 刷新间隔；网络名仅在实例集合变化时重拉 */
const META_TTL_MS = 15_000;
/** 多设备并行上限，避免设备多时打满浏览器/服务端连接 */
const DEVICE_FETCH_CONCURRENCY = 4;

interface NetworkRow {
    machine_id: string;
    hostname: string;
    connection_addr: string;
    connection_addr_raw: string;
    virtual_ip: string;
    instance_id: string;
    network_name: string;
}

const networkNameByKey = ref<Record<string, string>>({});
const virtualIpByKey = ref<Record<string, string>>({});
const lastMetaSignature = ref('');
const lastMetaFetchAt = ref(0);
/** 仅在尚无虚拟 IP 缓存的首次/补齐拉取时为 true，后台 TTL 刷新不闪烁 */
const metaLoading = ref(false);

let metaFetchGen = 0;
let metaFetchInFlight = false;
let metaFetchQueued: Utils.DeviceInfo[] | null = null;

const formatVirtualIp = (ip: NetworkTypes.Ipv4Inet | undefined): string => {
    if (ip?.address === undefined) {
        return '';
    }
    return Utils.ipv4InetToString(ip);
};

const activeInstanceKeys = (devices: Utils.DeviceInfo[]): string[] => {
    const keys: string[] = [];
    for (const device of devices) {
        for (const instanceId of device.running_network_instances ?? []) {
            keys.push(`${device.machine_id}:${instanceId}`);
        }
    }
    return keys;
};

const pruneToActive = (
    cache: Record<string, string>,
    activeKeys: string[],
): Record<string, string> => {
    const next: Record<string, string> = {};
    for (const key of activeKeys) {
        if (cache[key]) {
            next[key] = cache[key];
        }
    }
    return next;
};

/** 有限并发 map，保持结果顺序 */
const mapPool = async <T, R>(
    items: T[],
    concurrency: number,
    fn: (item: T) => Promise<R>,
): Promise<R[]> => {
    if (!items.length) {
        return [];
    }
    const results = new Array<R>(items.length);
    let cursor = 0;
    const worker = async () => {
        while (cursor < items.length) {
            const index = cursor++;
            results[index] = await fn(items[index]);
        }
    };
    const n = Math.min(Math.max(concurrency, 1), items.length);
    await Promise.all(Array.from({ length: n }, () => worker()));
    return results;
};

const networkRows = computed<NetworkRow[]>(() => {
    const rows: NetworkRow[] = [];
    for (const device of deviceList.value ?? []) {
        for (const instanceId of device.running_network_instances ?? []) {
            const key = `${device.machine_id}:${instanceId}`;
            const rawAddr = device.public_ip || '';
            rows.push({
                machine_id: device.machine_id,
                hostname: device.hostname,
                connection_addr: Utils.formatClientUrl(rawAddr) || rawAddr,
                connection_addr_raw: rawAddr,
                virtual_ip: virtualIpByKey.value[key] || '',
                instance_id: instanceId,
                network_name: networkNameByKey.value[key] || instanceId,
            });
        }
    }
    return rows.sort((a, b) =>
        a.network_name.localeCompare(b.network_name) || a.hostname.localeCompare(b.hostname)
    );
});

const fetchMetasForDevices = async (
    devices: Utils.DeviceInfo[],
    opts: { fetchNames: boolean; fetchVips: boolean },
) => {
    const activeKeys = activeInstanceKeys(devices);
    const nextNames = pruneToActive(networkNameByKey.value, activeKeys);
    const nextVips = pruneToActive(virtualIpByKey.value, activeKeys);

    await mapPool(devices, DEVICE_FETCH_CONCURRENCY, async (device) => {
        const ids = device.running_network_instances ?? [];
        if (!ids.length || !props.api) {
            return;
        }
        const client = props.api.get_remote_client(device.machine_id);

        if (opts.fetchNames) {
            try {
                const resp = await client.get_network_metas(ids);
                for (const id of ids) {
                    const key = `${device.machine_id}:${id}`;
                    nextNames[key] = resp.metas?.[id]?.network_name || id;
                }
            } catch (e) {
                console.debug('load network metas failed', device.machine_id, e);
            }
        }

        if (opts.fetchVips) {
            try {
                const infos = client.get_network_infos
                    ? await client.get_network_infos(ids)
                    : Object.fromEntries(
                        await Promise.all(ids.map(async (id) => [id, await client.get_network_info(id)] as const)),
                    );
                for (const id of ids) {
                    const key = `${device.machine_id}:${id}`;
                    const vip = formatVirtualIp(infos[id]?.my_node_info?.virtual_ipv4);
                    // 空结果保留旧值，避免瞬时失败把已有 VIP 冲掉
                    if (vip) {
                        nextVips[key] = vip;
                    }
                }
            } catch (e) {
                console.debug('load network infos failed', device.machine_id, e);
            }
        }
    });

    return { nextNames, nextVips };
};

const refreshNetworkMetas = async (devices: Utils.DeviceInfo[]) => {
    const signature = devices
        .map((d) => `${d.machine_id}:${(d.running_network_instances ?? []).join(',')}`)
        .sort()
        .join('|');
    const now = Date.now();
    const signatureChanged = signature !== lastMetaSignature.value;
    const ttlExpired = now - lastMetaFetchAt.value >= META_TTL_MS;
    const activeKeys = activeInstanceKeys(devices);
    const missingVip = activeKeys.some((key) => !virtualIpByKey.value[key]);
    // 缺 VIP 时不受 TTL 限制，避免首次拉取失败后卡住至下个 TTL
    if (!signatureChanged && !ttlExpired && !missingVip) {
        return;
    }

    if (metaFetchInFlight) {
        metaFetchQueued = devices;
        return;
    }

    metaFetchInFlight = true;
    // 仅缺 VIP 时显示加载态；后台 TTL 刷新有缓存则不闪 spinner
    metaLoading.value = missingVip;

    const gen = ++metaFetchGen;
    try {
        let pending: Utils.DeviceInfo[] | null = devices;
        let pendingFetchNames = signatureChanged;
        while (pending) {
            const batch = pending;
            pending = null;
            metaFetchQueued = null;

            const batchSignature = batch
                .map((d) => `${d.machine_id}:${(d.running_network_instances ?? []).join(',')}`)
                .sort()
                .join('|');
            const batchNamesChanged = batchSignature !== lastMetaSignature.value;
            const fetchNames = pendingFetchNames || batchNamesChanged;

            const { nextNames, nextVips } = await fetchMetasForDevices(batch, {
                fetchNames,
                fetchVips: true,
            });
            if (gen !== metaFetchGen) {
                return;
            }

            networkNameByKey.value = nextNames;
            virtualIpByKey.value = nextVips;
            lastMetaSignature.value = batchSignature;
            lastMetaFetchAt.value = Date.now();
            pendingFetchNames = false;

            pending = metaFetchQueued;
        }
    } finally {
        if (gen === metaFetchGen) {
            metaFetchInFlight = false;
            metaLoading.value = false;
        }
    }
};

const loadDevices = async (): Promise<Array<Utils.DeviceInfo>> => {
    const resp = await props.api?.list_machines();
    const devices: Array<Utils.DeviceInfo> = [];
    for (const device of (resp || [])) {
        devices.push(Utils.buildDeviceInfo(device));
    }
    void refreshNetworkMetas(devices);
    return devices;
};

const { data: deviceList } = usePollingList<Array<Utils.DeviceInfo>>({
    fetcher: loadDevices,
    // 网络列表依赖二次 RPC；1s 过密，略放宽减轻 list_machines 压力
    interval: 2000,
});

const openNetworkRow = (row: NetworkRow, mode: 'status' | 'config') => {
    router.push({
        name: 'deviceManagement',
        params: {
            deviceId: row.machine_id,
            instanceId: row.instance_id,
        },
        query: { mode, from: 'networkList' },
    });
};
</script>

<template>
    <div class="et-page">
        <div class="et-page-header">
            <h1 class="et-page-title">{{ t('web.main.network_list') }}</h1>
        </div>

        <!-- 桌面：表格 -->
        <div class="network-list-desktop">
            <ListPageShell :loading="deviceList === undefined" :empty="networkRows.length === 0">
                <template #empty>{{ t('web.device.no_networks') }}</template>
                <thead>
                    <tr class="bg-surface-50 text-left">
                        <th class="px-3 py-2 font-semibold">{{ t('web.device.network_name') }}</th>
                        <th class="px-3 py-2 font-semibold">{{ t('web.device.belonging_device') }}</th>
                        <th class="px-3 py-2 font-semibold">{{ t('web.device.connection_addr') }}</th>
                        <th class="px-3 py-2 font-semibold">{{ t('virtual_ipv4') }}</th>
                        <th class="px-3 py-2 font-semibold">{{ t('web.device.status') }}</th>
                        <th class="px-3 py-2 font-semibold text-right">{{ t('web.device.management') }}</th>
                    </tr>
                </thead>
                <tbody>
                    <tr v-for="row in networkRows" :key="`${row.machine_id}-${row.instance_id}`"
                        class="border-t border-surface">
                        <td class="px-3 py-2">
                            <div class="font-medium truncate max-w-[16rem]" v-tooltip.top="row.network_name">
                                {{ row.network_name }}
                            </div>
                            <div class="et-meta truncate max-w-[16rem]" v-tooltip.top="row.instance_id">
                                {{ row.instance_id }}
                            </div>
                        </td>
                        <td class="px-3 py-2 truncate max-w-[10rem]" v-tooltip.top="row.hostname">{{ row.hostname }}</td>
                        <td class="px-3 py-2 truncate max-w-[14rem]"
                            v-tooltip.top="row.connection_addr_raw || undefined">
                            {{ row.connection_addr || '—' }}
                        </td>
                        <td class="px-3 py-2 truncate max-w-[12rem]">
                            <span v-if="row.virtual_ip" v-tooltip.top="row.virtual_ip">{{ row.virtual_ip }}</span>
                            <span v-else-if="metaLoading" class="inline-flex items-center gap-1 et-meta">
                                <ProgressSpinner style="width: 0.85rem; height: 0.85rem"
                                    strokeWidth="6" aria-hidden="true" />
                                <span class="sr-only">{{ t('web.device_management.loading_network_status') }}</span>
                            </span>
                            <span v-else class="et-meta">—</span>
                        </td>
                        <td class="px-3 py-2">
                            <span class="inline-flex items-center gap-1 status-running">
                                <i class="pi pi-circle-fill text-[0.45rem]"></i>
                                {{ t('network_running') }}
                            </span>
                        </td>
                        <td class="px-3 py-2">
                            <div class="flex justify-end gap-2">
                                <Button v-tooltip.top="t('web.device.open_network_status')"
                                    icon="pi pi-chart-line" severity="info" rounded text
                                    class="et-icon-action-btn"
                                    @click="openNetworkRow(row, 'status')"
                                    :aria-label="t('web.device.open_network_status')" />
                                <Button v-tooltip.top="t('web.device.open_network_config')"
                                    icon="pi pi-cog" severity="secondary" rounded text
                                    class="et-icon-action-btn"
                                    @click="openNetworkRow(row, 'config')"
                                    :aria-label="t('web.device.open_network_config')" />
                            </div>
                        </td>
                    </tr>
                </tbody>
            </ListPageShell>
        </div>

        <!-- 移动：卡片，避免六列宽表横滑 -->
        <div class="network-list-mobile">
            <div v-if="deviceList === undefined" class="w-full flex justify-center py-8">
                <ProgressSpinner />
            </div>
            <div v-else-if="networkRows.length === 0" class="et-list-empty et-meta py-10 px-4">
                <i class="pi pi-inbox text-2xl" aria-hidden="true"></i>
                <span>{{ t('web.device.no_networks') }}</span>
            </div>
            <div v-else class="network-card-list">
                <article v-for="row in networkRows" :key="`m-${row.machine_id}-${row.instance_id}`"
                    class="network-card">
                    <div class="network-card-head">
                        <div class="min-w-0 flex-1">
                            <div class="network-card-title truncate" v-tooltip.top="row.network_name">
                                {{ row.network_name }}
                            </div>
                            <div class="et-meta truncate" v-tooltip.top="row.instance_id">{{ row.instance_id }}</div>
                        </div>
                        <span class="inline-flex items-center gap-1 status-running shrink-0">
                            <i class="pi pi-circle-fill text-[0.45rem]"></i>
                            {{ t('network_running') }}
                        </span>
                    </div>
                    <dl class="network-card-meta">
                        <div>
                            <dt>{{ t('web.device.belonging_device') }}</dt>
                            <dd class="truncate" v-tooltip.top="row.hostname">{{ row.hostname || '—' }}</dd>
                        </div>
                        <div>
                            <dt>{{ t('web.device.connection_addr') }}</dt>
                            <dd class="truncate" v-tooltip.top="row.connection_addr_raw || undefined">
                                {{ row.connection_addr || '—' }}
                            </dd>
                        </div>
                        <div>
                            <dt>{{ t('virtual_ipv4') }}</dt>
                            <dd>
                                <span v-if="row.virtual_ip">{{ row.virtual_ip }}</span>
                                <span v-else-if="metaLoading" class="inline-flex items-center gap-1 et-meta">
                                    <ProgressSpinner style="width: 0.85rem; height: 0.85rem"
                                        strokeWidth="6" aria-hidden="true" />
                                </span>
                                <span v-else class="et-meta">—</span>
                            </dd>
                        </div>
                    </dl>
                    <div class="network-card-actions">
                        <Button :label="t('web.device.page_title_status')"
                            icon="pi pi-chart-line" severity="info" outlined size="small"
                            class="network-card-btn"
                            @click="openNetworkRow(row, 'status')" />
                        <Button :label="t('web.device.page_title_config')"
                            icon="pi pi-cog" severity="secondary" outlined size="small"
                            class="network-card-btn"
                            @click="openNetworkRow(row, 'config')" />
                    </div>
                </article>
            </div>
        </div>
    </div>
</template>

<style scoped>
.status-running {
    color: var(--et-success, #10b981);
}

@media (prefers-color-scheme: dark) {
    .status-running {
        color: var(--et-success, #34d399);
    }
}

.sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
}

.network-list-mobile {
    display: none;
}

.network-card-list {
    display: flex;
    flex-direction: column;
    gap: var(--et-space-3, 0.75rem);
}

.network-card {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    padding: var(--et-pad-card, 0.9rem);
    background: var(--surface-card, #ffffff);
    border: var(--et-border);
    border-radius: var(--et-radius);
    box-shadow: var(--et-shadow-card, 0 1px 2px rgba(15, 23, 42, 0.03));
}

.network-card-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 0.75rem;
}

.network-card-title {
    font-weight: 600;
    color: var(--text-color, #1e293b);
}

.network-card-meta {
    display: grid;
    grid-template-columns: 1fr;
    gap: 0.45rem 0.75rem;
    margin: 0;
}

.network-card-meta > div {
    display: grid;
    grid-template-columns: 5.5rem 1fr;
    gap: 0.5rem;
    align-items: baseline;
    min-width: 0;
}

.network-card-meta dt {
    margin: 0;
    color: var(--text-color-secondary, #64748b);
    font-size: var(--et-fs-meta, 0.75rem);
    font-weight: 600;
}

.network-card-meta dd {
    margin: 0;
    min-width: 0;
    font-size: 0.875rem;
    color: var(--text-color, #1e293b);
}

.network-card-actions {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.5rem;
}

.network-card-btn {
    width: 100%;
}

.et-list-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    min-height: 10rem;
    text-align: center;
    background: var(--surface-card, #ffffff);
    border: var(--et-border);
    border-radius: var(--et-radius);
}

.et-list-empty i {
    color: var(--primary-color, #0ea5e9);
    opacity: 0.72;
}

@media (max-width: 639px) {
    .network-list-desktop {
        display: none;
    }

    .network-list-mobile {
        display: block;
    }
}
</style>
