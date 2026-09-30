<script setup lang="ts">
import { computed, ref } from 'vue';
import { Button } from 'primevue';
import { Utils, tooltipDirective } from 'easytier-frontend-lib';
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

interface NetworkRow {
    machine_id: string;
    hostname: string;
    instance_id: string;
    network_name: string;
}

const networkNameByKey = ref<Record<string, string>>({});
const lastMetaSignature = ref('');

const networkRows = computed<NetworkRow[]>(() => {
    const rows: NetworkRow[] = [];
    for (const device of deviceList.value ?? []) {
        for (const instanceId of device.running_network_instances ?? []) {
            const key = `${device.machine_id}:${instanceId}`;
            rows.push({
                machine_id: device.machine_id,
                hostname: device.hostname,
                instance_id: instanceId,
                network_name: networkNameByKey.value[key] || instanceId,
            });
        }
    }
    return rows.sort((a, b) =>
        a.network_name.localeCompare(b.network_name) || a.hostname.localeCompare(b.hostname)
    );
});

const refreshNetworkMetas = async (devices: Utils.DeviceInfo[]) => {
    const signature = devices
        .map((d) => `${d.machine_id}:${(d.running_network_instances ?? []).join(',')}`)
        .sort()
        .join('|');
    if (signature === lastMetaSignature.value) {
        return;
    }
    lastMetaSignature.value = signature;

    const next: Record<string, string> = { ...networkNameByKey.value };
    await Promise.all(devices.map(async (device) => {
        const ids = device.running_network_instances ?? [];
        if (!ids.length || !props.api) {
            return;
        }
        try {
            const resp = await props.api.get_remote_client(device.machine_id).get_network_metas(ids);
            for (const id of ids) {
                const key = `${device.machine_id}:${id}`;
                next[key] = resp.metas?.[id]?.network_name || id;
            }
        } catch (e) {
            console.debug('load network metas failed', device.machine_id, e);
        }
    }));
    networkNameByKey.value = next;
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
        query: { mode },
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
                <tr class="surface-ground text-left">
                    <th class="px-3 py-2 font-semibold">{{ t('web.device.network_name') }}</th>
                    <th class="px-3 py-2 font-semibold">{{ t('web.device.belonging_device') }}</th>
                    <th class="px-3 py-2 font-semibold">{{ t('web.device.status') }}</th>
                    <th class="px-3 py-2 font-semibold text-right">{{ t('web.device.management') }}</th>
                </tr>
            </thead>
            <tbody>
                <tr v-for="row in networkRows" :key="`${row.machine_id}-${row.instance_id}`"
                    class="border-t surface-border">
                    <td class="px-3 py-2">
                        <div class="font-medium truncate max-w-[16rem]" v-tooltip.top="row.network_name">
                            {{ row.network_name }}
                        </div>
                        <div class="et-meta truncate max-w-[16rem]" v-tooltip.top="row.instance_id">
                            {{ row.instance_id }}
                        </div>
                    </td>
                    <td class="px-3 py-2 truncate max-w-[10rem]" v-tooltip.top="row.hostname">{{ row.hostname }}</td>
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
                                class="network-action-btn"
                                @click="openNetworkRow(row, 'status')"
                                :aria-label="t('web.device.open_network_status')" />
                            <Button v-tooltip.top="t('web.device.open_network_config')"
                                icon="pi pi-cog" severity="secondary" rounded text
                                class="network-action-btn"
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
    color: var(--green-700, #15803d);
}

.network-action-btn {
    width: var(--et-btn-sm) !important;
    height: var(--et-btn-sm) !important;
}

@media (prefers-color-scheme: dark) {
    .status-running {
        color: var(--green-400, #4ade80);
    }
}
</style>
