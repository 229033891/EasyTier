<script setup lang="ts">
import { NetworkTypes, Utils, Api, RemoteManagement, TOAST_LIFE, addThrottledToast, createToastRepeatGate } from 'easytier-frontend-lib';
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { ProgressSpinner, ToggleSwitch, useConfirm, useToast } from 'primevue';
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
const confirm = useConfirm();

/** 机器级客户端编辑锁（devices.local_edit_locked），由 5s 设备轮询带回。 */
const editLockBusy = ref(false);

/** 开启前确认：锁生效后，客户端对控制台配置只读（下一心跳周期收敛）。 */
const toggleEditLock = (locked: boolean) => {
    const apply = async () => {
        if (editLockBusy.value) {
            return;
        }
        editLockBusy.value = true;
        try {
            await props.api.set_device_local_edit_lock(deviceId.value, locked);
            await loadDevices();
            toast.add({
                severity: 'success',
                summary: t(locked ? 'web.device.local_edit_lock_on' : 'web.device.local_edit_lock_off'),
                life: TOAST_LIFE.success,
            });
        } catch (e) {
            addThrottledToast(toast, errorToastGate, 'device-edit-lock-error', {
                severity: 'error',
                summary: t('web.device.local_edit_lock_failed'),
                detail: Utils.formatApiErrorDetail(e, t),
                life: TOAST_LIFE.error,
            });
        } finally {
            editLockBusy.value = false;
        }
    };
    if (!locked) {
        void apply();
        return;
    }
    confirm.require({
        header: t('web.device.local_edit_lock_confirm_title'),
        message: t('web.device.local_edit_lock_confirm_message'),
        icon: 'pi pi-lock',
        acceptProps: { severity: 'warn', label: t('web.common.confirm') },
        rejectProps: { label: t('web.common.cancel') },
        accept: () => { void apply(); },
    });
};

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

/** 设备名并入网络选择条；标题文案仅作无障碍兜底，不再单独占一行 */
const deviceName = computed(() => {
    // hostname 可能是 ""，不能用 ??，否则不会回退到 deviceId
    return (deviceInfo.value?.hostname || deviceId.value || '').trim()
});

const pageTitle = computed(() => {
    const host = deviceName.value;
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

// 5s 轮询失败：首错即刻提示，持续失败按 30s 窗口合并（递归进入该页时不再叠条）。
const errorToastGate = createToastRepeatGate();

const periodFunc = new Utils.PeriodicTask(async () => {
    try {
        await loadDevices();
    } catch (e) {
        addThrottledToast(toast, errorToastGate, 'poll:device-list', {
            severity: 'error',
            summary: t('web.device.load_list_failed'),
            detail: Utils.formatApiErrorDetail(e, t),
            life: TOAST_LIFE.error,
        });
        console.error(e);
    }
}, 5000);

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
    <RemoteManagement
        v-else
        :key="deviceId"
        :api="remoteClient"
        v-model:instance-id="selectedInstanceId"
        :new-config-generator="newConfigGenerator"
        :full-page="true"
        host-surface="web-console"
        :residual-ignore-scope="deviceId"
        :device-os-type="deviceInfo?.os_type"
        :device-easytier-version="deviceInfo?.easytier_version"
        :device-magic-dns-os-wired="deviceInfo?.magic_dns_os_wired"
        :page-title="pageTitle"
        :device-name="deviceName"
        :drawer-close="backToList"
        :leave-button-label="managementFrom === 'networkList' ? t('web.device.back_to_network_list') : t('web.device.back_to_list')"
        :leave-button-tooltip="managementFrom === 'networkList' ? t('web.device.back_to_network_list_tip') : t('web.device.back_to_list_tip')"
        leave-button-icon="pi pi-arrow-left"
        :mode="managementMode"
        @switch-mode="switchManagementMode"
    >
        <template #footer-extra>
            <!-- 仅在已知设备上渲染：直达 URL 且设备不存在时不留一个可点的空开关。 -->
            <div v-if="deviceInfo" class="flex items-center gap-2" v-tooltip.top="t('web.device.local_edit_lock_tip')">
                <ToggleSwitch
                    :model-value="!!deviceInfo?.local_edit_locked"
                    :disabled="editLockBusy"
                    @update:model-value="toggleEditLock"
                />
                <span class="text-sm whitespace-nowrap">{{ t('web.device.local_edit_lock') }}</span>
            </div>
        </template>
    </RemoteManagement>
</template>
