<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { Button, ProgressSpinner, useToast } from 'primevue';
import { tooltipDirective } from '../modules/tooltip';
import { useRouter } from 'vue-router';
import { Utils } from 'easytier-frontend-lib';
import { useI18n } from 'vue-i18n';
import ApiClient from '../modules/api';

const vTooltip = tooltipDirective;
const { t } = useI18n();
const router = useRouter();
const toast = useToast();

const props = defineProps({
    api: ApiClient,
});

interface NetworkRow {
    machine_id: string;
    hostname: string;
    instance_id: string;
    network_name: string;
}

const deviceList = ref<Array<Utils.DeviceInfo> | undefined>(undefined);
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

const loadDevices = async () => {
    const resp = await props.api?.list_machines();
    const devices: Array<Utils.DeviceInfo> = [];
    for (const device of (resp || [])) {
        devices.push(Utils.buildDeviceInfo(device));
    }
    deviceList.value = devices;
    void refreshNetworkMetas(devices);
};

const periodFunc = new Utils.PeriodicTask(async () => {
    try {
        await loadDevices();
    } catch (e) {
        toast.add({ severity: 'error', summary: t('web.device.load_list_failed'), detail: String(e), life: 2000 });
        console.error(e);
    }
}, 1000);

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

onMounted(() => {
    periodFunc.start();
});

onUnmounted(() => {
    periodFunc.stop();
});
</script>

<template>
    <div class="et-page">
        <h1 class="et-page-title">{{ t('web.main.network_list') }}</h1>

        <div v-if="deviceList === undefined" class="w-full flex justify-center py-8">
            <ProgressSpinner />
        </div>

        <div v-else-if="networkRows.length === 0" class="network-list-empty et-meta py-6 px-4">
            {{ t('web.device.no_networks') }}
        </div>

        <div v-else class="network-list-table overflow-x-auto">
                <table class="w-full">
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
                                <div class="font-medium truncate max-w-[16rem]" :title="row.network_name">
                                    {{ row.network_name }}
                                </div>
                                <div class="et-meta truncate max-w-[16rem]" :title="row.instance_id">
                                    {{ row.instance_id }}
                                </div>
                            </td>
                            <td class="px-3 py-2 truncate max-w-[10rem]" :title="row.hostname">{{ row.hostname }}</td>
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
                </table>
        </div>
    </div>
</template>

<style scoped>
.network-list-table,
.network-list-empty {
    background: var(--surface-ground, #f8fafc);
    border: var(--et-border);
    border-radius: var(--et-radius);
}

.network-list-table th,
.network-list-table td {
    vertical-align: middle;
}

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
