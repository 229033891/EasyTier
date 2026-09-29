<script setup lang="ts">
import { NetworkTypes, Utils, Api, RemoteManagement } from 'easytier-frontend-lib';
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { ProgressSpinner, useToast } from 'primevue';
import { useI18n } from 'vue-i18n';
import ApiClient from '../modules/api';

const props = defineProps<{
    api: ApiClient;
}>();

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const toast = useToast();

const deviceList = ref<Array<Utils.DeviceInfo> | undefined>(undefined);

const deviceId = computed<string>(() => {
    return route.params.deviceId as string;
});

const instanceId = computed<string>(() => {
    return route.params.instanceId as string;
});

const deviceInfo = computed<Utils.DeviceInfo | undefined | null>(() => {
    return deviceId.value ? deviceList.value?.find((device) => device.machine_id === deviceId.value) : null;
});

/** 设备列表双入口：status | config；缺省时不传（兼容旧链接） */
const managementMode = computed(() => {
    const mode = route.query.mode;
    if (mode === 'status' || mode === 'config') {
        return mode;
    }
    return undefined;
});

const pageTitle = computed(() => {
    const host = deviceInfo.value?.hostname ?? deviceId.value ?? '';
    if (managementMode.value === 'config') {
        return `${t('web.device.open_network_config')} · ${host}`;
    }
    if (managementMode.value === 'status') {
        return `${t('web.device.open_network_status')} · ${host}`;
    }
    return `${t('web.device.management')} · ${host}`;
});

const selectedInstanceId = computed({
    get() {
        return instanceId.value;
    },
    set(value: string) {
        router.push({
            name: 'deviceManagement',
            params: { deviceId: deviceId.value, instanceId: value },
            query: managementMode.value ? { mode: managementMode.value } : {},
        });
    }
});

const remoteClient = computed<Api.RemoteClient>(() => props.api.get_remote_client(deviceId.value));

const newConfigGenerator = () => {
    const config = NetworkTypes.DEFAULT_NETWORK_CONFIG();
    config.hostname = deviceInfo.value?.hostname;
    return config;
}

const switchManagementMode = (mode: 'status' | 'config') => {
    router.push({
        name: 'deviceManagement',
        params: {
            deviceId: deviceId.value,
            instanceId: instanceId.value,
        },
        query: { mode },
    });
}

const backToList = () => {
    router.push({ name: 'deviceList' });
}

const loadDevices = async () => {
    const resp = await props.api?.list_machines();
    const devices: Array<Utils.DeviceInfo> = [];
    for (const device of (resp || [])) {
        devices.push(Utils.buildDeviceInfo(device));
    }
    deviceList.value = devices;
};

const periodFunc = new Utils.PeriodicTask(async () => {
    try {
        await loadDevices();
    } catch (e) {
        toast.add({ severity: 'error', summary: t('web.device.load_list_failed'), detail: String(e), life: 2000 });
        console.error(e);
    }
}, 3000);

onMounted(() => {
    periodFunc.start();
});

onUnmounted(() => {
    periodFunc.stop();
});

</script>

<template>
    <div class="device-management-page et-page">
        <h1 class="et-page-title">{{ pageTitle }}</h1>
        <div v-if="deviceList === undefined" class="w-full flex justify-center py-8">
            <ProgressSpinner />
        </div>
        <RemoteManagement v-else :api="remoteClient" v-model:instance-id="selectedInstanceId"
            :new-config-generator="newConfigGenerator" :full-page="true"
            :drawer-close="backToList"
            :leave-button-label="t('web.device.back_to_list')"
            leave-button-icon="pi pi-arrow-left"
            :mode="managementMode" @switch-mode="switchManagementMode" />
    </div>
</template>

<style scoped>
.device-management-page {
    width: 100%;
    gap: 0.5rem;
}
</style>
