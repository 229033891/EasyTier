<script setup lang="ts">
import { computed, ref } from 'vue';
import { Button } from 'primevue';
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

/** 虚拟 IP 等运行态字段的最短刷新间隔，避免每秒拉全量 network info */
const META_TTL_MS = 15_000;

interface NetworkRow {
    machine_id: string;
    hostname: string;
    public_ip: string;
    virtual_ip: string;
    instance_id: string;
    network_name: string;
}

const networkNameByKey = ref<Record<string, string>>({});
const virtualIpByKey = ref<Record<string, string>>({});
const lastMetaSignature = ref('');
const lastMetaFetchAt = ref(0);

/** 请求代数：只应用最新一次结果，失败不推进 signature，便于下次重试 */
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

const networkRows = computed<NetworkRow[]>(() => {
    const rows: NetworkRow[] = [];
    for (const device of deviceList.value ?? []) {
        for (const instanceId of device.running_network_instances ?? []) {
            const key = `${device.machine_id}:${instanceId}`;
            rows.push({
                machine_id: device.machine_id,
                hostname: device.hostname,
                public_ip: device.public_ip || '',
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

const fetchMetasForDevices = async (devices: Utils.DeviceInfo[]) => {
    const activeKeys = activeInstanceKeys(devices);
    const nextNames = pruneToActive(networkNameByKey.value, activeKeys);
    const nextVips = pruneToActive(virtualIpByKey.value, activeKeys);

    await Promise.all(devices.map(async (device) => {
        const ids = device.running_network_instances ?? [];
        if (!ids.length || !props.api) {
            return;
        }
        const client = props.api.get_remote_client(device.machine_id);
        try {
            const resp = await client.get_network_metas(ids);
            for (const id of ids) {
                const key = `${device.machine_id}:${id}`;
                nextNames[key] = resp.metas?.[id]?.network_name || id;
            }
        } catch (e) {
            console.debug('load network metas failed', device.machine_id, e);
        }
        await Promise.all(ids.map(async (id) => {
            const key = `${device.machine_id}:${id}`;
            try {
                const info = await client.get_network_info(id);
                nextVips[key] = formatVirtualIp(info?.my_node_info?.virtual_ipv4);
            } catch (e) {
                console.debug('load network virtual ip failed', device.machine_id, id, e);
            }
        }));
    }));

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
    if (!signatureChanged && !ttlExpired) {
        return;
    }

    if (metaFetchInFlight) {
        metaFetchQueued = devices;
        return;
    }

    metaFetchInFlight = true;
    const gen = ++metaFetchGen;
    try {
        let pending: Utils.DeviceInfo[] | null = devices;
        while (pending) {
            const batch = pending;
            pending = null;
            metaFetchQueued = null;

            const batchSignature = batch
                .map((d) => `${d.machine_id}:${(d.running_network_instances ?? []).join(',')}`)
                .sort()
                .join('|');

            const { nextNames, nextVips } = await fetchMetasForDevices(batch);
            if (gen !== metaFetchGen) {
                return;
            }

            networkNameByKey.value = nextNames;
            virtualIpByKey.value = nextVips;
            lastMetaSignature.value = batchSignature;
            lastMetaFetchAt.value = Date.now();

            pending = metaFetchQueued;
        }
    } finally {
        metaFetchInFlight = false;
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

const { data: deviceList } = usePollingList<Array<Utils.DeviceInfo>>({ fetcher: loadDevices });

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

        <ListPageShell :loading="deviceList === undefined" :empty="networkRows.length === 0">
            <template #empty>{{ t('web.device.no_networks') }}</template>
            <thead>
                <tr class="bg-surface-50 text-left">
                    <th class="px-3 py-2 font-semibold">{{ t('web.device.network_name') }}</th>
                    <th class="px-3 py-2 font-semibold">{{ t('web.device.belonging_device') }}</th>
                    <th class="px-3 py-2 font-semibold">{{ t('web.device.public_ip') }}</th>
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
                    <td class="px-3 py-2 truncate max-w-[14rem]" v-tooltip.top="row.public_ip || undefined">
                        {{ row.public_ip || '—' }}
                    </td>
                    <td class="px-3 py-2 truncate max-w-[12rem]" v-tooltip.top="row.virtual_ip || undefined">
                        {{ row.virtual_ip || '—' }}
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
</style>
