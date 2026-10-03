<script setup lang="ts">
import { NetworkTypes, Utils, Api, RemoteManagement, TOAST_LIFE } from 'easytier-frontend-lib';
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { ProgressSpinner, useToast } from 'primevue';
import { useI18n } from 'vue-i18n';
import ApiClient from '../modules/api';
import { loadMergedDevices } from '../modules/deviceArchive';

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

/** 进入来源：用于侧栏高亮与返回列表 */
const managementFrom = computed(() => {
    return route.query.from === 'networkList' ? 'networkList' : 'deviceList';
});

const managementQuery = computed(() => {
    const q: Record<string, string> = { from: managementFrom.value };
    if (managementMode.value) q.mode = managementMode.value;
    return q;
});

const pageTitle = computed(() => {
    const host = deviceInfo.value?.hostname ?? deviceId.value ?? '';
    if (managementMode.value === 'config') {
        return `${t('web.device.page_title_config')} · ${host}`;
    }
    if (managementMode.value === 'status') {
        return `${t('web.device.page_title_status')} · ${host}`;
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
            query: managementQuery.value,
        });
    }
});

const remoteClient = computed<Api.RemoteClient>(() => props.api.get_remote_client(deviceId.value));

const newConfigGenerator = () => {
    const config = NetworkTypes.DEFAULT_NETWORK_CONFIG();
    config.hostname =
        deviceInfo.value?.reported_hostname || deviceInfo.value?.hostname;
    return config;
}

const switchManagementMode = (mode: 'status' | 'config') => {
    router.push({
        name: 'deviceManagement',
        params: {
            deviceId: deviceId.value,
            instanceId: instanceId.value,
        },
        query: { ...managementQuery.value, mode },
    });
}

const backToList = () => {
    router.push({ name: managementFrom.value });
}

const loadDevices = async () => {
    deviceList.value = await loadMergedDevices(props.api, { toast, t });
};

const periodFunc = new Utils.PeriodicTask(async () => {
    try {
        await loadDevices();
    } catch (e) {
        toast.add({
            severity: 'error',
            summary: t('web.device.load_list_failed'),
            detail: Utils.formatApiErrorDetail(e, t),
            life: TOAST_LIFE.error,
        });
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
    <div v-if="deviceList === undefined" class="w-full flex justify-center py-8">
        <ProgressSpinner />
    </div>
    <RemoteManagement v-else :api="remoteClient" v-model:instance-id="selectedInstanceId"
        :new-config-generator="newConfigGenerator" :full-page="true"
        :page-title="pageTitle"
        :drawer-close="backToList"
        :leave-button-label="managementFrom === 'networkList' ? t('web.device.back_to_network_list') : t('web.device.back_to_list')"
        :leave-button-tooltip="managementFrom === 'networkList' ? t('web.device.back_to_network_list_tip') : t('web.device.back_to_list_tip')"
        leave-button-icon="pi pi-arrow-left"
        :mode="managementMode" @switch-mode="switchManagementMode" />
</template>
